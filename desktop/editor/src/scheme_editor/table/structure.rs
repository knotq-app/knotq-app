use knotq_model::Table;

use super::super::*;
use super::*;

impl SchemeEditor {
    pub(in crate::scheme_editor) fn insert_table(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let mut top = reconstruct_top_level(&self.rows);
        let insert_pos = (self.current_top_level_index() + 1).min(top.len());
        let mut table_item = Item::new("");
        table_item.set_table(Table::new(2, 2));
        let table_id = table_item.id;
        top.insert(insert_pos, table_item.clone());

        let (text, rows) = build_buffer(&top);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(Some(window));
        if let Some(anchor) = self
            .rows
            .iter()
            .position(|row| row.path.is_table_anchor() && row.item.id == table_id)
        {
            if anchor + 1 < self.rows.len() && self.rows[anchor + 1].path.is_cell() {
                self.selection = TextSelection::collapsed(TextLocation {
                    row: anchor + 1,
                    col: 0,
                });
            }
        }
        self.focus(window, cx);
        self.scroll_to_cursor(cx);
        cx.emit(EditorEvent::Command(Command::InsertItem {
            scheme: self.scheme_id,
            position: insert_pos,
            item: table_item,
        }));
        cx.notify();
    }

    fn current_top_level_index(&self) -> usize {
        let row = self.current_row_index();
        let mut index: usize = 0;
        for i in 0..=row.min(self.rows.len().saturating_sub(1)) {
            if !self.rows[i].path.is_cell() {
                index += 1;
            }
        }
        index.saturating_sub(1)
    }

    pub(in crate::scheme_editor) fn apply_table_control(
        &mut self,
        hitbox: TableControlHitbox,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let Some(anchor) = self.rows.get(hitbox.anchor_row) else {
            return;
        };
        if !anchor.path.is_table_anchor() {
            return;
        }
        let table_id = anchor.item.id;
        let action = match hitbox.kind {
            TableControlKind::AddRow => TableStructureAction::AppendRow,
            TableControlKind::AddColumn => TableStructureAction::AppendColumn,
        };
        self.apply_table_structure_action(table_id, action, window, cx);
    }

    pub fn apply_table_structure_action(
        &mut self,
        table_id: ItemId,
        action: TableStructureAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let mut top = reconstruct_top_level(&self.rows);
        let Some(pos) = top.iter().position(|item| item.id == table_id) else {
            return;
        };
        let Some(table) = top[pos].table_mut() else {
            return;
        };
        match action {
            TableStructureAction::AppendRow => {
                table.insert_row(table.row_count());
            }
            TableStructureAction::AppendColumn => {
                let n = table.column_count();
                table.insert_column(n, format!("Column {}", n + 1));
            }
            TableStructureAction::InsertRowBefore(row) => {
                table.insert_row(row);
            }
            TableStructureAction::InsertRowAfter(row) => {
                table.insert_row(row.saturating_add(1));
            }
            TableStructureAction::DeleteRow(row) => table.remove_row(row),
            TableStructureAction::InsertColumnBefore(col) => {
                let n = table.column_count();
                table.insert_column(col, format!("Column {}", n + 1));
            }
            TableStructureAction::InsertColumnAfter(col) => {
                let n = table.column_count();
                table.insert_column(col.saturating_add(1), format!("Column {}", n + 1));
            }
            TableStructureAction::DeleteColumn(col) => table.remove_column(col),
        }
        table.normalize();
        let item = top[pos].clone();

        let (text, rows) = build_buffer(&top);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(Some(window));
        self.selection = TextSelection::collapsed(self.clamp_location(self.selection.head));
        self.scroll_to_cursor(cx);
        cx.emit(EditorEvent::Command(Command::ReplaceItem {
            scheme: self.scheme_id,
            item,
        }));
        cx.notify();
    }
}
