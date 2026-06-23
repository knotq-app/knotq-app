    use super::*;
    use knotq_model::{ImageAssetFormat, ImageInline, Inline, ItemMarker, Table};
    use uuid::Uuid;

    #[test]
    fn display_lines_keep_hard_indent_out_of_text() {
        let item = Item::new("child").with_indent(2);
        let (text, rows) = build_buffer(&[item]);
        assert_eq!(text, "child");
        assert_eq!(rows[0].item.indent, 2);
        assert_eq!(clean_line_text("\t    child"), "child");
    }

    #[test]
    fn line_change_finds_middle_replacement() {
        let old = ["a", "b", "c", "d"];
        let new = ["a", "x", "y", "d"];
        assert_eq!(
            line_change(&old, &new),
            LineChange {
                prefix: 1,
                old_suffix: 3,
                new_suffix: 3,
            }
        );
    }

    #[test]
    fn empty_text_still_has_one_logical_line() {
        assert_eq!(line_ranges(""), vec![0..0]);
        assert_eq!(line_ranges("a\n"), vec![0..1, 2..2]);
    }

    #[test]
    fn table_headers_are_editable_buffer_rows() {
        let mut item = Item::new("");
        item.set_table(Table::new(1, 2));

        let (text, rows) = build_buffer(&[item.clone()]);
        let (_, rebuilt_rows) = build_buffer(&[item]);

        let lines = text.lines().take(3).collect::<Vec<_>>();
        assert_eq!(lines[0], TABLE_OBJECT_CHAR.to_string());
        assert_eq!(lines[1], "Column 1");
        assert_eq!(lines[2], "Column 2");
        assert_eq!(table_object_range(lines[0]), Some(0..TABLE_OBJECT_LEN));
        assert_eq!(clean_line_text(lines[0]), "");
        assert!(rows[1].path.is_header_cell());
        assert_eq!((rows[1].path.r, rows[1].path.c), (HEADER_ROW, 0));
        assert!(rows[2].path.is_header_cell());
        assert_eq!((rows[2].path.r, rows[2].path.c), (HEADER_ROW, 1));
        assert_eq!(rows[1].item.id, rebuilt_rows[1].item.id);
        assert_ne!(rows[1].item.id, rows[2].item.id);
    }

    #[test]
    fn edited_header_rows_reconstruct_to_column_names() {
        let mut item = Item::new("");
        item.set_table(Table::new(1, 2));
        let (_, mut rows) = build_buffer(&[item]);

        rows[1].item.set_text("Project".to_string());
        rows[2].item.set_text("Owner".to_string());
        let top = reconstruct_top_level(&rows);
        let table = top[0].table().expect("table remains the line content");

        assert_eq!(table.columns[0].name, "Project");
        assert_eq!(table.columns[1].name, "Owner");
    }

    #[test]
    fn table_line_renders_as_single_object_and_round_trips() {
        // A table is the whole content of its line: it renders as one object
        // sentinel with no surrounding text, and round-trips as a block.
        let mut item = Item::new("");
        item.set_table(Table::new(1, 1));

        let (text, rows) = build_buffer(&[item]);
        let line = text.lines().next().unwrap();
        assert_eq!(line, TABLE_OBJECT_CHAR.to_string());

        let rebuilt = reconstruct_top_level(&rows);
        assert_eq!(rebuilt.len(), 1);
        assert!(rebuilt[0].has_table());
        assert_eq!(rebuilt[0].text(), "");
        assert!(rebuilt[0].content.is_block());
    }

    #[test]
    fn empty_line_merges_into_following_table() {
        // Backspacing an empty line that sits above a table folds it away and
        // leaves the table as a single object — no content is lost.
        let empty = Item::new("");
        let empty_id = empty.id;
        let mut table = Item::new("");
        table.set_table(Table::new(1, 2));
        let table_id = table.id;

        let mut top = vec![empty, table];
        let result = merge_table_item_into(&mut top, 0, 1).expect("empty line absorbs table");

        assert_eq!(result.target, empty_id);
        assert_eq!(result.deleted, table_id);
        assert_eq!(result.target_index, 0);
        assert_eq!(top.len(), 1);
        assert!(top[0].has_table());
        assert_eq!(top[0].text(), "");
    }

    #[test]
    fn non_empty_line_refuses_to_merge_with_table() {
        // A line that still has text cannot merge with a block — they stay on
        // separate lines so nothing is silently dropped.
        let before = Item::new("Before");
        let mut table = Item::new("");
        table.set_table(Table::new(1, 2));
        let mut top = vec![before, table];

        assert!(merge_table_item_into(&mut top, 0, 1).is_none());
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].text(), "Before");
        assert!(top[1].has_table());
    }

    #[test]
    fn table_merge_does_not_combine_two_tables() {
        let mut first = Item::new("");
        first.set_table(Table::new(1, 1));
        let mut second = Item::new("");
        second.set_table(Table::new(1, 1));
        let mut top = vec![first, second];

        assert!(merge_table_item_into(&mut top, 0, 1).is_none());
        assert_eq!(top.len(), 2);
        assert!(top[0].has_table());
        assert!(top[1].has_table());
    }

    #[test]
    fn empty_line_below_table_merges_backward_into_it() {
        let mut table = Item::new("");
        table.set_table(Table::new(1, 1));
        let table_id = table.id;
        let empty = Item::new("");
        let empty_id = empty.id;
        let mut top = vec![table, empty];

        let result = append_item_into_table(&mut top, 0, 1).expect("empty line folds into table");

        assert_eq!(result.target, table_id);
        assert_eq!(result.deleted, empty_id);
        assert_eq!(result.target_index, 0);
        assert_eq!(top.len(), 1);
        assert!(top[0].has_table());
    }

    #[test]
    fn non_empty_line_refuses_backward_merge_into_table() {
        let mut table = Item::new("");
        table.set_table(Table::new(1, 1));
        let suffix = Item::new("After");
        let mut top = vec![table, suffix];

        assert!(append_item_into_table(&mut top, 0, 1).is_none());
        assert_eq!(top.len(), 2);
        assert!(top[0].has_table());
        assert_eq!(top[1].text(), "After");
    }

    #[test]
    fn table_split_at_start_inserts_blank_before_existing_table() {
        let mut item = Item::new("");
        item.indent = 1;
        item.set_table(Table::new(1, 1));
        let table_id = item.id;
        let mut top = vec![item];

        let result =
            split_table_item_at_text_col(&mut top, 0, 0).expect("blank inserts before table");

        assert_eq!(top.len(), 2);
        assert_eq!(top[0].text(), "");
        assert!(!top[0].has_table());
        assert_eq!(top[0].indent, 1);
        assert_eq!(top[1].id, table_id);
        assert!(top[1].has_table());
        assert_eq!(result.table, table_id);
        assert_eq!(result.table_index, 1);
    }

    #[test]
    fn image_line_renders_as_single_object_and_round_trips() {
        let mut item = Item::new("");
        item.set_image(test_image());

        let (text, rows) = build_buffer(&[item]);
        assert_eq!(text, TABLE_OBJECT_CHAR.to_string());
        assert_eq!(block_object_ranges(&text), vec![0..TABLE_OBJECT_LEN]);

        let rebuilt = reconstruct_top_level(&rows);
        assert_eq!(rebuilt.len(), 1);
        assert!(rebuilt[0].has_images());
        assert_eq!(rebuilt[0].text(), "");
        assert!(rebuilt[0].content.is_block());
    }

    #[test]
    fn empty_line_merges_backward_into_image() {
        let mut image = Item::new("");
        image.set_image(test_image());
        let image_id = image.id;
        let empty = Item::new("");
        let empty_id = empty.id;
        let mut top = vec![image, empty];

        let result = append_item_into_table(&mut top, 0, 1).expect("empty line folds into image");

        assert_eq!(result.target, image_id);
        assert_eq!(result.deleted, empty_id);
        assert_eq!(result.target_index, 0);
        assert_eq!(top.len(), 1);
        assert!(top[0].has_images());
    }

    #[test]
    fn selected_block_inlines_extracts_the_line_block() {
        // Each block is its own line, so selecting that line's object yields the
        // single block the line carries.
        let image = test_image();
        let mut image_item = Item::new("");
        image_item.set_image(image);
        let (image_text, _) = build_buffer(&[image_item.clone()]);
        assert_eq!(
            selected_block_inlines(&image_item, &image_text, 0..TABLE_OBJECT_LEN),
            vec![Inline::Image(image)]
        );

        let mut table_item = Item::new("");
        table_item.set_table(Table::new(1, 1));
        let (table_text, _) = build_buffer(&[table_item.clone()]);
        assert!(matches!(
            selected_block_inlines(&table_item, &table_text, 0..TABLE_OBJECT_LEN).as_slice(),
            [Inline::Table(_)]
        ));
    }

    #[test]
    fn replace_block_range_swaps_the_line_block() {
        let first = test_image();
        let second = ImageInline {
            asset: Uuid::new_v4(),
            format: ImageAssetFormat::Jpeg,
            width: Some(640),
            height: Some(480),
        };
        let mut item = Item::new("");
        item.set_image(first);

        let (text, _) = build_buffer(&[item.clone()]);
        assert!(replace_block_range_with_inlines(
            &mut item,
            &text,
            0..TABLE_OBJECT_LEN,
            vec![Inline::Image(second)]
        ));
        assert_eq!(item.content, ItemContent::Image(second));
    }

    #[test]
    fn row_equality_tracks_done_state() {
        let open = Item::new("task");
        let done = Item::new("task").done();
        assert!(!same_rows(
            &[EditorRow::doc(open)],
            &[EditorRow::doc(done.clone())]
        ));
        assert!(item_is_done(&done));
    }

    #[test]
    fn row_equality_tracks_date_metadata() {
        let mut base = Item::new("task");
        base.marker = ItemMarker::Checkbox;
        let mut dated = Item::new("task").with_end(chrono::Utc::now());
        dated.marker = ItemMarker::Checkbox;

        assert!(!same_rows(
            &[EditorRow::doc(base)],
            &[EditorRow::doc(dated)]
        ));
    }

    #[test]
    fn row_equality_tracks_media_metadata() {
        let base = Item::new("image");
        let mut with_media = Item::new("");
        with_media.set_image(ImageInline {
            asset: Uuid::new_v4(),
            format: ImageAssetFormat::Png,
            width: Some(32),
            height: Some(24),
        });

        assert!(!same_rows(
            &[EditorRow::doc(base)],
            &[EditorRow::doc(with_media)]
        ));
    }

    #[test]
    fn split_line_with_blocks_splits_text_around_a_mid_line_image() {
        let mut orig = Item::new("hello");
        orig.indent = 2;
        let id = orig.id;
        let result = split_line_with_blocks(&orig, 3, vec![Inline::Image(test_image())]);

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].id, id, "leading text keeps the original identity");
        assert_eq!(result[0].text(), "hel");
        assert_eq!(result[0].indent, 2);
        assert!(result[1].has_images());
        assert_eq!(result[1].indent, 2);
        assert_eq!(result[2].text(), "lo");
        assert_ne!(result[2].id, id);
    }

    #[test]
    fn split_line_with_blocks_at_start_keeps_text_after_block() {
        let orig = Item::new("hello");
        let id = orig.id;
        let result = split_line_with_blocks(&orig, 0, vec![Inline::Image(test_image())]);

        assert_eq!(result.len(), 2);
        assert!(result[0].has_images());
        // The original text line keeps its identity, now after the block.
        assert_eq!(result[1].id, id);
        assert_eq!(result[1].text(), "hello");
    }

    #[test]
    fn split_line_with_blocks_at_end_keeps_text_before_block() {
        let orig = Item::new("hello");
        let id = orig.id;
        let result = split_line_with_blocks(&orig, 5, vec![Inline::Image(test_image())]);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].id, id);
        assert_eq!(result[0].text(), "hello");
        assert!(result[1].has_images());
    }

    #[test]
    fn split_line_with_blocks_on_empty_line_becomes_the_block() {
        let orig = Item::new("");
        let id = orig.id;
        let result = split_line_with_blocks(&orig, 0, vec![Inline::Image(test_image())]);

        assert_eq!(result.len(), 1);
        assert!(result[0].has_images());
        assert_eq!(
            result[0].id, id,
            "empty line keeps its identity as the block"
        );
    }

    #[test]
    fn split_line_with_blocks_on_block_line_appends_after() {
        let mut orig = Item::new("");
        orig.set_table(Table::new(1, 1));
        let id = orig.id;
        let result = split_line_with_blocks(&orig, 0, vec![Inline::Image(test_image())]);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].id, id);
        assert!(result[0].has_table());
        assert!(result[1].has_images());
    }

    fn image_item() -> Item {
        let mut item = Item::new("");
        item.set_image(test_image());
        item
    }

    #[test]
    fn splice_restores_a_cut_text_image_text_run() {
        // "hello" / [image] / "world": cutting "lo[image]wo" leaves the joined
        // line "helrld" with the caret at col 3; pasting the captured run must
        // restore the three original lines (a no-op).
        let current = Item::new("helrld");
        let items = vec![Item::new("lo"), image_item(), Item::new("wo")];
        let (result, cursor_index, cursor_col) = splice_items_into_line(&current, 3, items);

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].text(), "hello");
        assert!(result[1].has_images());
        assert_eq!(result[2].text(), "world");
        // The first restored line keeps the caret line's identity.
        assert_eq!(result[0].id, current.id);
        // Caret lands at the original selection end (inside "world", after "wo").
        assert_eq!(cursor_index, 2);
        assert_eq!(cursor_col, 2);
    }

    #[test]
    fn splice_with_leading_block_restores_run() {
        // [image] / "world": cutting "[image]wor" leaves "ld" with caret at col 0.
        let current = Item::new("ld");
        let items = vec![image_item(), Item::new("wor")];
        let (result, _, _) = splice_items_into_line(&current, 0, items);

        assert_eq!(result.len(), 2);
        assert!(result[0].has_images());
        assert_eq!(result[1].text(), "world");
    }

    #[test]
    fn splice_with_trailing_block_restores_run() {
        // "hello" / [image]: cutting "lo[image]" leaves "hel" with caret at col 3.
        let current = Item::new("hel");
        let items = vec![Item::new("lo"), image_item()];
        let (result, _, _) = splice_items_into_line(&current, 3, items);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].text(), "hello");
        assert!(result[1].has_images());
    }

    fn test_image() -> ImageInline {
        ImageInline {
            asset: Uuid::nil(),
            format: ImageAssetFormat::Png,
            width: Some(320),
            height: Some(200),
        }
    }
