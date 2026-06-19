use super::*;

impl SchemeEditor {
    pub(super) fn move_cursor_to(
        &mut self,
        loc: TextLocation,
        select: bool,
        cx: &mut Context<Self>,
    ) {
        let loc = self.clamp_location(loc);
        if select {
            self.selection.head = loc;
        } else {
            self.selection = TextSelection::collapsed(loc);
        }
        self.marked_range = None;
        self.cursor_blink_state = true;
        cx.emit(EditorEvent::SelectionChanged {
            scheme_id: self.scheme_id,
        });
        self.scroll_to_cursor(cx);
        cx.notify();
    }

    pub(super) fn move_left(&mut self, select: bool, cx: &mut Context<Self>) {
        if !select && !self.selection.is_empty() {
            let (start, _) = self.selection.ordered();
            self.move_cursor_to(start, false, cx);
            return;
        }
        if !select {
            if let Some(target) = self.table_boundary_horizontal_target(false) {
                self.move_cursor_to(target, false, cx);
                return;
            }
        }
        let offset = self.location_to_offset(self.selection.head);
        let prev = previous_char_boundary(&self.text, offset);
        let target = self.clamp_horizontal(self.selection.head, self.offset_to_location(prev));
        self.move_cursor_to(target, select, cx);
    }

    pub(super) fn move_right(&mut self, select: bool, cx: &mut Context<Self>) {
        if !select && !self.selection.is_empty() {
            let (_, end) = self.selection.ordered();
            self.move_cursor_to(end, false, cx);
            return;
        }
        if !select {
            if let Some(target) = self.table_boundary_horizontal_target(true) {
                self.move_cursor_to(target, false, cx);
                return;
            }
            if self.insert_trailing_line_after_table_boundary(cx) {
                return;
            }
        }
        let offset = self.location_to_offset(self.selection.head);
        let next = next_char_boundary(&self.text, offset);
        let target = self.clamp_horizontal(self.selection.head, self.offset_to_location(next));
        self.move_cursor_to(target, select, cx);
    }

    pub(super) fn move_vertical(&mut self, delta: isize, select: bool, cx: &mut Context<Self>) {
        if self.line_map.line_count() == 0 {
            let row_count = self.render_line_count();
            let row = (self.selection.head.row as isize + delta)
                .clamp(0, row_count.saturating_sub(1) as isize) as usize;
            let col = self.selection.head.col.min(self.line_len(row));
            self.move_cursor_to(TextLocation { row, col }, select, cx);
            return;
        }

        let target = self.vertical_target(self.selection.head, delta);
        self.move_cursor_to(target, select, cx);
    }

    pub(super) fn move_word_left(&mut self, select: bool, cx: &mut Context<Self>) {
        let offset = self.location_to_offset(self.selection.head);
        let target = self.clamp_horizontal(
            self.selection.head,
            self.offset_to_location(previous_word_offset(&self.text, offset)),
        );
        self.move_cursor_to(target, select, cx);
    }

    pub(super) fn move_word_right(&mut self, select: bool, cx: &mut Context<Self>) {
        let offset = self.location_to_offset(self.selection.head);
        let target = self.clamp_horizontal(
            self.selection.head,
            self.offset_to_location(next_word_offset(&self.text, offset)),
        );
        self.move_cursor_to(target, select, cx);
    }

    pub(super) fn move_line_start(&mut self, select: bool, cx: &mut Context<Self>) {
        self.move_cursor_to(
            TextLocation {
                row: self.selection.head.row,
                col: 0,
            },
            select,
            cx,
        );
    }

    pub(super) fn move_line_end(&mut self, select: bool, cx: &mut Context<Self>) {
        let row = self.selection.head.row;
        self.move_cursor_to(
            TextLocation {
                row,
                col: self.line_len(row),
            },
            select,
            cx,
        );
    }

    pub(super) fn move_document_start(&mut self, select: bool, cx: &mut Context<Self>) {
        if let Some((first, _)) = self.cell_line_span(self.selection.head.row) {
            self.move_cursor_to(TextLocation { row: first, col: 0 }, select, cx);
            return;
        }
        self.move_cursor_to(TextLocation { row: 0, col: 0 }, select, cx);
    }

    pub(super) fn move_document_end(&mut self, select: bool, cx: &mut Context<Self>) {
        if let Some((_, last)) = self.cell_line_span(self.selection.head.row) {
            self.move_cursor_to(
                TextLocation {
                    row: last,
                    col: self.line_len(last),
                },
                select,
                cx,
            );
            return;
        }
        self.move_cursor_to(self.offset_to_location(self.text.len()), select, cx);
    }

    pub(super) fn select_all(&mut self, cx: &mut Context<Self>) {
        if let Some((first, last)) = self.cell_line_span(self.selection.head.row) {
            self.selection = TextSelection {
                anchor: TextLocation { row: first, col: 0 },
                head: TextLocation {
                    row: last,
                    col: self.line_len(last),
                },
            };
        } else {
            self.selection = TextSelection {
                anchor: TextLocation { row: 0, col: 0 },
                head: self.offset_to_location(self.text.len()),
            };
        }
        self.reset_cursor_blink(cx);
        cx.notify();
    }

    pub(super) fn clamp_horizontal(&self, from: TextLocation, to: TextLocation) -> TextLocation {
        if to.row == from.row {
            return to;
        }
        let from_path = self
            .rows
            .get(from.row)
            .map(|row| row.path)
            .unwrap_or_default();
        let to_path = self
            .rows
            .get(to.row)
            .map(|row| row.path)
            .unwrap_or_default();
        let allowed = if from_path.is_cell() {
            to_path.is_cell()
                && to_path.anchor == from_path.anchor
                && to_path.r == from_path.r
                && to_path.c == from_path.c
        } else {
            to_path.is_doc()
        };
        if allowed {
            to
        } else if to.row < from.row {
            TextLocation {
                row: from.row,
                col: 0,
            }
        } else {
            TextLocation {
                row: from.row,
                col: self.line_len(from.row),
            }
        }
    }

    fn table_boundary_horizontal_target(&self, forward: bool) -> Option<TextLocation> {
        let head = self.selection.head;
        let row = head.row.min(self.rows.len().saturating_sub(1));
        let col = head.col.min(self.line_len(row));
        let path = self.rows.get(row)?.path;

        if forward {
            if col != self.line_len(row) {
                return None;
            }
            if path.is_table_anchor() {
                return self
                    .row_after_table(row)
                    .map(|row| TextLocation { row, col: 0 });
            }
            if self
                .rows
                .get(row + 1)
                .is_some_and(|row| row.path.is_table_anchor())
            {
                return Some(TextLocation {
                    row: row + 1,
                    col: 0,
                });
            }
            return None;
        }

        if col != 0 {
            return None;
        }
        if path.is_table_anchor() {
            return row.checked_sub(1).map(|row| TextLocation {
                row,
                col: self.line_len(row),
            });
        }
        if row > 0 {
            let previous = self.rows[row - 1].path;
            if previous.is_cell() {
                return Some(TextLocation {
                    row: previous.anchor,
                    col: self.line_len(previous.anchor),
                });
            }
        }
        None
    }

    fn insert_trailing_line_after_table_boundary(&mut self, cx: &mut Context<Self>) -> bool {
        if self.read_only {
            return false;
        }
        if !self.selection.is_empty() {
            return false;
        }
        let row = self
            .selection
            .head
            .row
            .min(self.rows.len().saturating_sub(1));
        let Some(editor_row) = self.rows.get(row) else {
            return false;
        };
        if !editor_row.path.is_table_anchor() || self.selection.head.col != self.line_len(row) {
            return false;
        }
        if self.row_after_table(row).is_some() {
            return false;
        }

        let table_id = editor_row.item.id;
        let mut top = reconstruct_top_level(&self.rows);
        let Some(position) = top.iter().position(|item| item.id == table_id) else {
            return false;
        };
        let mut blank = Item::new("");
        blank.indent = editor_row.item.indent;
        top.insert(position + 1, blank.clone());

        let (text, rows) = build_buffer(&top);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(None);
        let row = flat_row_for_top_level_index(&self.rows, position + 1);
        self.selection = TextSelection::collapsed(TextLocation { row, col: 0 });
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        self.scroll_to_cursor(cx);
        cx.emit(EditorEvent::Command(Command::InsertItem {
            scheme: self.scheme_id,
            position: position + 1,
            item: blank,
        }));
        cx.notify();
        true
    }

    pub(super) fn cell_line_span(&self, row: usize) -> Option<(usize, usize)> {
        let path = self.rows.get(row)?.path;
        if !path.is_cell() {
            return None;
        }

        let mut first = row;
        while first > 0 {
            let p = self.rows[first - 1].path;
            if p.is_cell() && p.anchor == path.anchor && p.r == path.r && p.c == path.c {
                first -= 1;
            } else {
                break;
            }
        }

        let mut last = row;
        while last + 1 < self.rows.len() {
            let p = self.rows[last + 1].path;
            if p.is_cell() && p.anchor == path.anchor && p.r == path.r && p.c == path.c {
                last += 1;
            } else {
                break;
            }
        }

        Some((first, last))
    }

    fn vertical_target(&self, cur: TextLocation, delta: isize) -> TextLocation {
        let prefer_x = self.visual_point_for_location(cur).x;
        let pick = |row: usize| TextLocation {
            row,
            col: self.col_at_x_in_row(row, prefer_x),
        };
        let n = self.rows.len();
        let path = self
            .rows
            .get(cur.row)
            .map(|row| row.path)
            .unwrap_or_default();

        if path.is_cell() {
            if delta > 0 && !path.is_last_in_cell() {
                return pick(cur.row + 1);
            }
            if delta < 0 && !path.is_first_in_cell() {
                return pick(cur.row - 1);
            }

            if let Some(target_table_row) = self.next_visual_table_row(path.r, delta, path.anchor) {
                let want_last = delta < 0;
                if let Some(row) =
                    self.find_cell_row(path.anchor, target_table_row, path.c, want_last)
                {
                    return pick(row);
                }
            }

            return self
                .exit_table_row(path.anchor, delta)
                .map(pick)
                .unwrap_or(cur);
        }

        let next = cur.row as isize + delta;
        if next < 0 || next as usize >= n {
            return cur;
        }
        let next = next as usize;
        let next_path = self.rows[next].path;
        if next_path.is_table_anchor() {
            return pick(next);
        }
        if next_path.is_cell() {
            let anchor = next_path.anchor;
            let target_table_row = if delta > 0 {
                HEADER_ROW
            } else {
                self.table_nrows(anchor).saturating_sub(1)
            };
            let col = self.column_for_x(anchor, prefer_x);
            let want_last = delta < 0;
            if let Some(row) = self
                .find_cell_row(anchor, target_table_row, col, want_last)
                .or_else(|| self.find_cell_row(anchor, 0, col, want_last))
            {
                return pick(row);
            }
            return cur;
        }

        pick(next)
    }

    fn col_at_x_in_row(&self, row: usize, prefer_x: Pixels) -> usize {
        let (base_x, _) = self.row_base_xy(row);
        self.line_map
            .closest_col(row, point(prefer_x - base_x, px(0.0)))
            .min(self.line_len(row))
    }

    fn table_nrows(&self, anchor: usize) -> usize {
        self.rows
            .get(anchor)
            .and_then(|row| row.item.table())
            .map(|table| table.row_count())
            .unwrap_or(0)
    }

    pub(super) fn find_cell_row(
        &self,
        anchor: usize,
        r: usize,
        c: usize,
        want_last: bool,
    ) -> Option<usize> {
        let mut found = None;
        for (row, editor_row) in self.rows.iter().enumerate() {
            let path = editor_row.path;
            if path.is_cell() && path.anchor == anchor && path.r == r && path.c == c {
                if want_last {
                    found = Some(row);
                } else {
                    return Some(row);
                }
            }
        }
        found
    }

    fn next_visual_table_row(&self, r: usize, delta: isize, anchor: usize) -> Option<usize> {
        let nrows = self.table_nrows(anchor);
        if delta > 0 {
            if r == HEADER_ROW {
                (nrows > 0).then_some(0)
            } else if r + 1 < nrows {
                Some(r + 1)
            } else {
                None
            }
        } else if r == HEADER_ROW {
            None
        } else if r == 0 {
            Some(HEADER_ROW)
        } else {
            Some(r - 1)
        }
    }

    fn exit_table_row(&self, anchor: usize, delta: isize) -> Option<usize> {
        if delta < 0 {
            (anchor > 0).then_some(anchor - 1)
        } else {
            let mut row = anchor + 1;
            while row < self.rows.len() && self.rows[row].path.is_cell() {
                row += 1;
            }
            (row < self.rows.len()).then_some(row)
        }
    }

    fn row_after_table(&self, anchor: usize) -> Option<usize> {
        let mut row = anchor + 1;
        while row < self.rows.len() && self.rows[row].path.is_cell() {
            row += 1;
        }
        (row < self.rows.len()).then_some(row)
    }

    pub(super) fn cell_tab_nav(&mut self, forward: bool, cx: &mut Context<Self>) {
        let head = self.selection.head;
        let Some(path) = self
            .rows
            .get(head.row)
            .map(|row| row.path)
            .filter(|path| path.is_cell())
        else {
            return;
        };

        let ncols = self
            .rows
            .get(path.anchor)
            .and_then(|row| row.item.table())
            .map(|table| table.column_count())
            .unwrap_or(1);
        let nrows = self.table_nrows(path.anchor);
        let is_header = path.is_header_cell();
        let (mut r, mut c) = (path.r, path.c);
        // Tab order runs across the header row, then row-major through the body.
        // Header cells carry the HEADER_ROW sentinel, so guard `r` arithmetic.
        if forward {
            if c + 1 < ncols {
                c += 1;
            } else if is_header {
                r = 0;
                c = 0;
            } else if r + 1 < nrows {
                c = 0;
                r += 1;
            } else {
                return;
            }
        } else if c > 0 {
            c -= 1;
        } else if is_header {
            return;
        } else if r == 0 {
            r = HEADER_ROW;
            c = ncols.saturating_sub(1);
        } else {
            c = ncols.saturating_sub(1);
            r -= 1;
        }

        if let Some(first) = self.find_cell_row(path.anchor, r, c, false) {
            // Highlight the whole target cell (start to end, across its lines) so
            // typing replaces its contents — collapse at the start, then extend.
            let last = self.find_cell_row(path.anchor, r, c, true).unwrap_or(first);
            self.move_cursor_to(TextLocation { row: first, col: 0 }, false, cx);
            self.move_cursor_to(
                TextLocation {
                    row: last,
                    col: self.line_len(last),
                },
                true,
                cx,
            );
        }
    }

    fn column_for_x(&self, anchor: usize, prefer_x: Pixels) -> usize {
        let Some(layout) = self.table_layouts.get(&anchor) else {
            return 0;
        };
        let grid_left = self.table_grid_left_content(anchor);
        let local = prefer_x - grid_left;
        for c in 0..layout.col_w.len() {
            if local >= layout.col_x[c] && local < layout.col_x[c] + layout.col_w[c] {
                return c;
            }
        }
        layout.col_w.len().saturating_sub(1)
    }

    pub(super) fn current_row_index(&self) -> usize {
        self.selection
            .head
            .row
            .min(self.render_line_count().saturating_sub(1))
    }

    pub(super) fn selected_row_range(&self) -> (usize, usize) {
        let (start, end) = self.selection.ordered();
        let last = self.render_line_count().saturating_sub(1);
        (start.row.min(last), end.row.min(last))
    }

    pub(super) fn emit_commands(&mut self, commands: Vec<Command>, cx: &mut Context<Self>) {
        if let Some(cmd) = Command::from_vec(commands) {
            cx.emit(EditorEvent::Command(cmd));
            self.reset_cursor_blink(cx);
            cx.notify();
        }
    }

    pub(super) fn clamp_location(&self, loc: TextLocation) -> TextLocation {
        let row_count = self.render_line_count();
        let row = loc.row.min(row_count.saturating_sub(1));
        let col = loc.col.min(self.line_len(row));
        TextLocation { row, col }
    }

    pub(super) fn render_line_count(&self) -> usize {
        line_ranges(&self.text).len().max(1)
    }

    pub(super) fn line_len(&self, row: usize) -> usize {
        let ranges = line_ranges(&self.text);
        ranges
            .get(row)
            .map(|range| range.end.saturating_sub(range.start))
            .unwrap_or(0)
    }

    pub(super) fn line_range(&self, row: usize) -> Option<Range<usize>> {
        line_ranges(&self.text).get(row).cloned()
    }

    pub(super) fn table_object_range_for_row(&self, row: usize) -> Option<Range<usize>> {
        let editor_row = self.rows.get(row)?;
        if !editor_row.path.is_table_anchor() || !editor_row.item.has_table() {
            return None;
        }
        let range = self.line_range(row)?;
        table_object_range(self.text.get(range)?)
    }

    pub(super) fn text_lines(&self) -> Vec<String> {
        self.text.split('\n').map(ToString::to_string).collect()
    }

    pub(super) fn location_to_offset(&self, loc: TextLocation) -> usize {
        self.location_to_offset_in(&self.text, loc)
    }

    pub(super) fn location_to_offset_in(&self, text: &str, loc: TextLocation) -> usize {
        let ranges = line_ranges(text);
        if ranges.is_empty() {
            return 0;
        }
        let row = loc.row.min(ranges.len().saturating_sub(1));
        let range = ranges[row].clone();
        let col = loc.col.min(range.end - range.start);
        let mut offset = range.start + col;
        while offset > range.start && !text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    pub(super) fn offset_to_location(&self, offset: usize) -> TextLocation {
        self.offset_to_location_in(&self.text, offset)
    }

    pub(super) fn offset_to_location_in(&self, text: &str, offset: usize) -> TextLocation {
        let ranges = line_ranges(text);
        if ranges.is_empty() {
            return TextLocation { row: 0, col: 0 };
        }
        let offset = offset.min(text.len());
        for (row, range) in ranges.iter().enumerate() {
            if offset <= range.end {
                return TextLocation {
                    row,
                    col: offset.saturating_sub(range.start),
                };
            }
        }
        let row = ranges.len().saturating_sub(1);
        TextLocation {
            row,
            col: ranges[row].end.saturating_sub(ranges[row].start),
        }
    }

    pub(super) fn selection_offsets(&self) -> (usize, usize) {
        let (start, end) = self.selection.ordered();
        (self.location_to_offset(start), self.location_to_offset(end))
    }

    pub(super) fn selected_text(&self) -> Option<String> {
        if self.selection.is_empty() {
            return None;
        }
        let (start, end) = self.selection_offsets();
        self.text.get(start..end).map(line_without_table_object)
    }

    pub(super) fn selected_whole_rows(&self) -> Option<Range<usize>> {
        let line_lens: Vec<usize> = (0..self.render_line_count())
            .map(|row| self.line_len(row))
            .collect();
        whole_row_selection_range(self.selection, &line_lens)
    }
}
