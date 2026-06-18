use super::*;
use chrono::{NaiveDate, TimeZone, Utc};
use knotq_model::{
    CalendarDateTime, CalendarRecurrence, OccurrenceId, OccurrenceOverride,
    OccurrenceOverrideStatus, Recurrence,
};

fn default_repeat() -> Recurrence {
    CalendarRecurrence {
        rrules: vec!["FREQ=WEEKLY;INTERVAL=1".to_string()],
        ..Default::default()
    }
}

#[test]
fn annotation_text_formats_at_and_due_items() {
    let start = Utc.with_ymd_and_hms(2026, 5, 9, 15, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2026, 5, 9, 16, 30, 0).unwrap();

    let mut event = Item::new("event").with_start(start).with_end(end);
    event.marker = ItemMarker::Checkbox;
    let mut reminder = Item::new("reminder").with_start(start);
    reminder.marker = ItemMarker::Checkbox;
    let mut assignment = Item::new("assignment").with_end(end);
    assignment.marker = ItemMarker::Checkbox;

    let event_text = annotation_text(&event, TimeFormat::TwentyFourHour).unwrap();
    assert!(!event_text.starts_with("at "));
    assert!(!event_text.contains("due "));
    assert!(event_text.contains(" \u{2192} "));
    assert!(annotation_text(&reminder, TimeFormat::TwentyFourHour)
        .unwrap()
        .starts_with("at "));
    assert!(annotation_text(&assignment, TimeFormat::TwentyFourHour)
        .unwrap()
        .starts_with("due "));
    assert!(annotation_text(&Item::new("procedure"), TimeFormat::TwentyFourHour).is_none());
}

#[test]
fn annotation_text_includes_repeats() {
    let mut item = Item::new("class");
    item.marker = ItemMarker::Checkbox;
    item.repeats = Some(CalendarRecurrence {
        rrules: vec!["FREQ=WEEKLY;INTERVAL=1;BYDAY=MO,WE,FR".to_string()],
        ..Default::default()
    });

    assert_eq!(
        annotation_text(&item, TimeFormat::TwentyFourHour).as_deref(),
        Some("repeat weekly on Mon,Wed,Fri")
    );
}

#[test]
fn annotation_text_includes_repeat_exceptions() {
    let mut item = Item::new("class");
    item.marker = ItemMarker::Checkbox;
    item.repeats = Some(CalendarRecurrence {
        rrules: vec!["FREQ=DAILY;INTERVAL=1".to_string()],
        exdates: vec![
            CalendarDateTime::Date {
                date: NaiveDate::from_ymd_opt(2026, 5, 18).unwrap(),
            },
            CalendarDateTime::Date {
                date: NaiveDate::from_ymd_opt(2026, 5, 20).unwrap(),
            },
            CalendarDateTime::Date {
                date: NaiveDate::from_ymd_opt(2026, 5, 22).unwrap(),
            },
        ],
        ..Default::default()
    });

    assert_eq!(
        format_repeat_annotation_for_year(item.repeats.as_ref().unwrap(), 2026),
        "repeat daily; May 18, May 20, May 22 skip"
    );
    assert_eq!(
        format_repeat_annotation_for_year(item.repeats.as_ref().unwrap(), 2025),
        "repeat daily; May 18, 2026, May 20, 2026, May 22, 2026 skip"
    );
}

#[test]
fn annotation_text_separates_skips_from_special_cases() {
    let skipped = CalendarDateTime::Date {
        date: NaiveDate::from_ymd_opt(2026, 5, 18).unwrap(),
    };
    let overridden = CalendarDateTime::Date {
        date: NaiveDate::from_ymd_opt(2026, 5, 20).unwrap(),
    };
    let cancelled = CalendarDateTime::Date {
        date: NaiveDate::from_ymd_opt(2026, 5, 22).unwrap(),
    };
    let repeat = CalendarRecurrence {
        rrules: vec!["FREQ=DAILY;INTERVAL=1".to_string()],
        exdates: vec![skipped, overridden.clone()],
        overrides: vec![
            OccurrenceOverride {
                occurrence: OccurrenceId::Recurring {
                    original_start: overridden,
                },
                status: OccurrenceOverrideStatus::Active,
                start: Some(Utc.with_ymd_and_hms(2026, 5, 20, 13, 0, 0).unwrap()),
                end: None,
                available: None,
            },
            OccurrenceOverride {
                occurrence: OccurrenceId::Recurring {
                    original_start: cancelled,
                },
                status: OccurrenceOverrideStatus::Cancelled,
                start: None,
                end: None,
                available: None,
            },
        ],
        ..Default::default()
    };

    assert_eq!(
        format_repeat_annotation_for_year(&repeat, 2026),
        "repeat daily; May 18, May 22 skip; May 20 special"
    );
}

#[test]
fn annotation_text_formats_repeat_until_contextually() {
    let repeat = CalendarRecurrence {
        rrules: vec!["FREQ=DAILY;INTERVAL=1;UNTIL=20260522".to_string()],
        ..Default::default()
    };

    assert_eq!(
        format_repeat_annotation_for_year(&repeat, 2026),
        "repeat daily until May 22"
    );
    assert_eq!(
        format_repeat_annotation_for_year(&repeat, 2025),
        "repeat daily until May 22, 2026"
    );
}

#[test]
fn empty_line_attribute_clear_preserves_text_and_id_only() {
    let start = Utc.with_ymd_and_hms(2026, 5, 9, 15, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2026, 5, 9, 16, 0, 0).unwrap();
    let mut item = Item::new("")
        .with_indent(1)
        .with_start(start)
        .with_end(end)
        .done();
    item.repeats = Some(default_repeat());
    item.priority = Some(1);
    let id = item.id;

    assert!(item_has_line_attributes(&item));
    let clean = item_without_line_attributes(&item);

    assert_eq!(clean.id, id);
    assert_eq!(clean.text(), "");
    assert_eq!(clean.marker, ItemMarker::Blank);
    assert_eq!(clean.indent, 0);
    assert!(clean.start.is_none());
    assert!(clean.end.is_none());
    assert_eq!(clean.state.len(), 1);
    assert_eq!(clean.state[0].state.progress, 0);
    assert!(!item_has_line_attributes(&clean));
}

#[test]
fn empty_line_delete_targets_current_row_not_matching_later_empty_row() {
    assert_eq!(
        empty_line_delete_plan(1, 4, false, 6),
        Some(EmptyLineDeletePlan {
            delete_row: 1,
            cursor_after: TextLocation { row: 1, col: 0 },
        })
    );
    assert_eq!(
        empty_line_delete_plan(1, 4, true, 6),
        Some(EmptyLineDeletePlan {
            delete_row: 1,
            cursor_after: TextLocation { row: 0, col: 6 },
        })
    );
    assert_eq!(
        empty_line_delete_plan(3, 4, false, 6),
        Some(EmptyLineDeletePlan {
            delete_row: 3,
            cursor_after: TextLocation { row: 2, col: 6 },
        })
    );
    assert_eq!(empty_line_delete_plan(0, 1, false, 0), None);
}

#[test]
fn inserted_lines_inherit_marker_style_without_dates() {
    let start = Utc.with_ymd_and_hms(2026, 5, 9, 15, 0, 0).unwrap();
    let template = Item::new("task").with_indent(2).with_start(start).done();

    let inserted =
        item_for_inserted_line("next".into(), Some(InsertedLineStyle::from_item(&template)));

    assert_eq!(inserted.text(), "next");
    assert_eq!(inserted.marker, ItemMarker::Checkbox);
    assert_eq!(inserted.indent, 2);
    assert!(inserted.start.is_none());
    assert!(inserted.end.is_none());
    assert_eq!(inserted.state.len(), 1);
    assert_eq!(inserted.state[0].state.progress, 0);
}

#[test]
fn inserted_lines_inherit_numbered_marker_style() {
    let mut template = Item::new("step").with_indent(1);
    template.marker = ItemMarker::Numbered;

    let inserted =
        item_for_inserted_line("next".into(), Some(InsertedLineStyle::from_item(&template)));

    assert_eq!(inserted.text(), "next");
    assert_eq!(inserted.marker, ItemMarker::Numbered);
    assert_eq!(inserted.indent, 1);
    assert!(inserted.start.is_none());
    assert!(inserted.end.is_none());
}

#[test]
fn whole_row_selection_accepts_line_without_trailing_newline() {
    let selection = TextSelection {
        anchor: TextLocation { row: 1, col: 0 },
        head: TextLocation { row: 1, col: 4 },
    };

    assert_eq!(whole_row_selection_range(selection, &[3, 4, 5]), Some(1..2));
}

#[test]
fn whole_row_selection_accepts_trailing_newline_before_next_row() {
    let selection = TextSelection {
        anchor: TextLocation { row: 0, col: 0 },
        head: TextLocation { row: 2, col: 0 },
    };

    assert_eq!(whole_row_selection_range(selection, &[3, 4, 5]), Some(0..2));
}

#[test]
fn whole_row_selection_includes_selected_trailing_empty_row() {
    let selection = TextSelection {
        anchor: TextLocation { row: 0, col: 0 },
        head: TextLocation { row: 1, col: 0 },
    };

    assert_eq!(whole_row_selection_range(selection, &[3, 0]), Some(0..2));
}

#[test]
fn whole_row_selection_rejects_partial_lines() {
    let selection = TextSelection {
        anchor: TextLocation { row: 0, col: 1 },
        head: TextLocation { row: 0, col: 3 },
    };

    assert_eq!(whole_row_selection_range(selection, &[3]), None);
}

#[test]
fn rich_paste_duplicates_item_metadata_with_new_id() {
    let start = Utc.with_ymd_and_hms(2026, 5, 9, 15, 0, 0).unwrap();
    let mut copied = Item::new("\t task").with_indent(2).with_start(start).done();
    copied.marker = ItemMarker::Checkbox;
    let original_id = copied.id;

    let pasted = item_for_rich_paste(copied);

    assert_ne!(pasted.id, original_id);
    assert_eq!(pasted.text(), "task");
    assert_eq!(pasted.marker, ItemMarker::Checkbox);
    assert_eq!(pasted.indent, 2);
    assert_eq!(pasted.start, Some(start));
    assert!(pasted.end.is_none());
    assert!(pasted.state.iter().all(|state| state.state.is_done()));
}

#[test]
fn non_checkbox_marker_clears_checkbox_annotations() {
    let start = Utc.with_ymd_and_hms(2026, 5, 9, 15, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2026, 5, 9, 16, 0, 0).unwrap();
    let mut item = Item::new("task")
        .with_indent(3)
        .with_start(start)
        .with_end(end)
        .done();
    item.repeats = Some(default_repeat());
    item.priority = Some(2);

    let updated = item_with_marker(item, ItemMarker::Bullet);

    assert_eq!(updated.marker, ItemMarker::Bullet);
    assert_eq!(updated.indent, 3);
    assert_eq!(updated.priority, Some(2));
    assert!(updated.start.is_none());
    assert!(updated.end.is_none());
    assert!(updated.available.is_none());
    assert!(updated.repeats.is_none());
    assert_eq!(updated.state.len(), 1);
    assert_eq!(updated.state[0].state.progress, 0);
}

#[test]
fn numbered_marker_ordinals_reset_at_same_indent_boundaries() {
    fn row(marker: ItemMarker, indent: u8) -> EditorRow {
        let mut item = Item::new("row").with_indent(indent);
        item.marker = marker;
        EditorRow { item }
    }

    let rows = vec![
        row(ItemMarker::Numbered, 0),
        row(ItemMarker::Numbered, 0),
        row(ItemMarker::Bullet, 1),
        row(ItemMarker::Numbered, 0),
        row(ItemMarker::Bullet, 0),
        row(ItemMarker::Numbered, 0),
        row(ItemMarker::Numbered, 1),
        row(ItemMarker::Numbered, 1),
    ];

    assert_eq!(numbered_marker_ordinal(&rows, 0), Some(1));
    assert_eq!(numbered_marker_ordinal(&rows, 1), Some(2));
    assert_eq!(numbered_marker_ordinal(&rows, 2), None);
    assert_eq!(numbered_marker_ordinal(&rows, 3), Some(3));
    assert_eq!(numbered_marker_ordinal(&rows, 5), Some(1));
    assert_eq!(numbered_marker_ordinal(&rows, 6), Some(1));
    assert_eq!(numbered_marker_ordinal(&rows, 7), Some(2));
}

#[test]
fn markdown_heading_requires_hash_separator() {
    assert!(is_markdown_heading("# Heading"));
    assert!(is_markdown_heading("## Heading"));
    assert!(!is_markdown_heading("#channel"));
}

#[test]
fn markdown_runs_mark_emphasis_without_removing_markers() {
    let runs = parse_markdown_runs("a **bold** *ital* ==hi== ~~no~~");
    let bold = MarkdownStyle {
        bold: true,
        italic: false,
        highlight: false,
        strikethrough: false,
        heading: false,
    };
    let italic = MarkdownStyle {
        bold: false,
        italic: true,
        highlight: false,
        strikethrough: false,
        heading: false,
    };
    let highlight = MarkdownStyle {
        bold: false,
        italic: false,
        highlight: true,
        strikethrough: false,
        heading: false,
    };
    let strikethrough = MarkdownStyle {
        bold: false,
        italic: false,
        highlight: false,
        strikethrough: true,
        heading: false,
    };

    // Markers stay in the text, so the run lengths still cover every byte.
    assert_eq!(
        runs.iter().map(|run| run.len).sum::<usize>(),
        "a **bold** *ital* ==hi== ~~no~~".len()
    );
    assert!(runs.iter().any(|run| run.len == 4 && run.style == bold));
    assert!(runs.iter().any(|run| run.len == 4 && run.style == italic));
    assert!(runs
        .iter()
        .any(|run| run.len == 2 && run.style == highlight));
    assert!(runs
        .iter()
        .any(|run| run.len == 2 && run.style == strikethrough));
}

#[test]
fn markdown_runs_mark_headings_as_bold_heading() {
    let runs = parse_markdown_runs("# Heading");
    let heading_style = MarkdownStyle {
        bold: true,
        italic: false,
        highlight: false,
        strikethrough: false,
        heading: true,
    };
    assert_eq!(
        runs,
        vec![
            // The "# " prefix is a marker so it can collapse off the cursor line.
            MarkdownRun {
                len: "# ".len(),
                style: heading_style,
                kind: MarkdownRunKind::Marker,
            },
            MarkdownRun {
                len: "Heading".len(),
                style: heading_style,
                kind: MarkdownRunKind::Content,
            },
        ]
    );
}

#[test]
fn markdown_runs_tag_emphasis_delimiters_as_markers() {
    let runs = parse_markdown_runs("a **bold**");
    // Marker runs hold only the delimiter bytes: two "**" of length 2 each.
    let marker_bytes: usize = runs
        .iter()
        .filter(|run| run.kind == MarkdownRunKind::Marker)
        .map(|run| run.len)
        .sum();
    assert_eq!(marker_bytes, 4);
    // The bold word itself is content, not a marker.
    assert!(runs
        .iter()
        .any(|run| run.len == 4 && run.kind == MarkdownRunKind::Content && run.style.bold));
}
