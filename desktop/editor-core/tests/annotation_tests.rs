use chrono::{TimeZone, Utc};
use knotq_editor_core::{compute_annotations, parse_markdown_runs, Annotation};
use knotq_model::{Item, TimeFormat};

#[test]
fn markdown_runs_capture_emphasis() {
    let runs = parse_markdown_runs("a *bold* _ital_");

    assert!(runs.iter().any(|run| run.style.bold));
    assert!(runs.iter().any(|run| run.style.italic));
}

#[test]
fn date_annotations_are_generated_for_due_items() {
    let due = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
    let annotations =
        compute_annotations(&Item::new("Essay").with_end(due), TimeFormat::TwelveHour);

    assert!(annotations.iter().any(
        |annotation| matches!(annotation, Annotation::Date { text } if text.starts_with("Due "))
    ));
}
