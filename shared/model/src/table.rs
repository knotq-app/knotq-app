use serde::{Deserialize, Serialize};

use crate::{ColumnId, Inline, Item, RowId};

/// A table, carried inline in a line's content as [`Inline::Table`]. Cells are
/// themselves *lists* of [`Item`]s — each cell is a small sub-document, so every
/// line in it is a full [`Item`] (checkbox, date, image, indent, …) exactly
/// like a top-level line.
///
/// Invariants (restored by [`Table::normalize`]):
/// - the table is rectangular — every [`TableRow::cells`] has exactly
///   `columns.len()` entries;
/// - every cell has at least one item;
/// - cells do not themselves contain tables (no grids-in-grids for v1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub columns: Vec<TableColumn>,
    pub rows: Vec<TableRow>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TableColumn {
    #[serde(default)]
    pub id: ColumnId,
    #[serde(default)]
    pub name: String,
    /// Optional explicit width in logical pixels. `None` means the editor sizes
    /// the column from its content / an even split.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f32>,
}

impl TableColumn {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: ColumnId::new(),
            name: name.into(),
            width: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TableRow {
    #[serde(default)]
    pub id: RowId,
    /// One cell per column.
    pub cells: Vec<TableCell>,
}

impl TableRow {
    pub fn new(column_count: usize) -> Self {
        Self {
            id: RowId::new(),
            cells: (0..column_count).map(|_| TableCell::new()).collect(),
        }
    }
}

/// A single table cell: a sub-document of line [`Item`]s. Always holds at least
/// one item (an empty cell is one empty line).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TableCell {
    #[serde(default = "default_cell_items")]
    pub items: Vec<Item>,
}

fn default_cell_items() -> Vec<Item> {
    vec![Item::new("")]
}

impl Default for TableCell {
    fn default() -> Self {
        Self::new()
    }
}

impl TableCell {
    /// An empty cell — a single blank line item.
    pub fn new() -> Self {
        Self {
            items: vec![Item::new("")],
        }
    }

    /// A cell holding a single line of the given text.
    pub fn with_text(text: impl Into<String>) -> Self {
        Self {
            items: vec![Item::new(text)],
        }
    }

    /// A cell wrapping an explicit list of items (normalized to be non-empty).
    pub fn from_items(items: Vec<Item>) -> Self {
        let mut cell = Self { items };
        cell.normalize();
        cell
    }

    /// The first line item (cells always have at least one).
    pub fn first(&self) -> &Item {
        // `normalize` guarantees non-emptiness, but be defensive at read sites
        // that may run before a normalize pass.
        self.items.first().expect("cell has at least one item")
    }

    /// A one-line plain-text summary of the cell (lines joined by spaces). Used
    /// by markdown export and the mobile bridges where a flat string is needed.
    pub fn summary_text(&self) -> String {
        self.items
            .iter()
            .map(|item| item.text())
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string()
    }

    fn normalize(&mut self) {
        // Cells never nest tables for v1: strip any table inlines from the
        // cell's line items so the document stays at most two levels deep.
        for item in &mut self.items {
            item.content
                .retain(|inline| !matches!(inline, Inline::Table(_)));
        }
        if self.items.is_empty() {
            self.items.push(Item::new(""));
        }
    }
}

impl Table {
    /// A new table with `rows` data rows and `cols` columns (named "Column 1"…),
    /// each cell an empty line item.
    pub fn new(rows: usize, cols: usize) -> Self {
        let cols = cols.max(1);
        let columns = (0..cols)
            .map(|i| TableColumn::new(format!("Column {}", i + 1)))
            .collect();
        let rows = (0..rows.max(1)).map(|_| TableRow::new(cols)).collect();
        let mut table = Self { columns, rows };
        table.normalize();
        table
    }

    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn cell(&self, row: usize, col: usize) -> Option<&TableCell> {
        self.rows.get(row).and_then(|r| r.cells.get(col))
    }

    pub fn cell_mut(&mut self, row: usize, col: usize) -> Option<&mut TableCell> {
        self.rows.get_mut(row).and_then(|r| r.cells.get_mut(col))
    }

    /// Yields every cell (row-major).
    pub fn cells(&self) -> impl Iterator<Item = &TableCell> {
        self.rows.iter().flat_map(|r| r.cells.iter())
    }

    pub fn cells_mut(&mut self) -> impl Iterator<Item = &mut TableCell> {
        self.rows.iter_mut().flat_map(|r| r.cells.iter_mut())
    }

    /// Every line item across every cell, in row-major / in-cell order.
    pub fn all_items(&self) -> impl Iterator<Item = &Item> {
        self.cells().flat_map(|cell| cell.items.iter())
    }

    pub fn all_items_mut(&mut self) -> impl Iterator<Item = &mut Item> {
        self.cells_mut().flat_map(|cell| cell.items.iter_mut())
    }

    pub fn insert_row(&mut self, at: usize) -> RowId {
        let row = TableRow::new(self.column_count());
        let id = row.id;
        let at = at.min(self.rows.len());
        self.rows.insert(at, row);
        id
    }

    pub fn remove_row(&mut self, at: usize) {
        if self.rows.len() > 1 && at < self.rows.len() {
            self.rows.remove(at);
        }
    }

    pub fn insert_column(&mut self, at: usize, name: impl Into<String>) -> ColumnId {
        let column = TableColumn::new(name);
        let id = column.id;
        let at = at.min(self.columns.len());
        self.columns.insert(at, column);
        for row in &mut self.rows {
            row.cells.insert(at.min(row.cells.len()), TableCell::new());
        }
        id
    }

    pub fn remove_column(&mut self, at: usize) {
        if self.columns.len() > 1 && at < self.columns.len() {
            self.columns.remove(at);
            for row in &mut self.rows {
                if at < row.cells.len() {
                    row.cells.remove(at);
                }
            }
        }
    }

    /// Restore the rectangular invariant, guarantee at least one column/row, and
    /// keep every cell non-empty and table-free.
    pub fn normalize(&mut self) {
        if self.columns.is_empty() {
            self.columns.push(TableColumn::new("Column 1"));
        }
        if self.rows.is_empty() {
            self.rows.push(TableRow::new(self.columns.len()));
        }
        let cols = self.columns.len();
        for row in &mut self.rows {
            while row.cells.len() < cols {
                row.cells.push(TableCell::new());
            }
            row.cells.truncate(cols);
            for cell in &mut row.cells {
                cell.normalize();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_rectangular(table: &Table) -> bool {
        let cols = table.columns.len();
        table.rows.iter().all(|row| row.cells.len() == cols)
            && table.cells().all(|cell| !cell.items.is_empty())
    }

    #[test]
    fn new_is_rectangular_and_nonempty() {
        let table = Table::new(3, 2);
        assert_eq!(table.row_count(), 3);
        assert_eq!(table.column_count(), 2);
        assert!(is_rectangular(&table));
    }

    #[test]
    fn new_clamps_to_at_least_one_row_and_column() {
        let table = Table::new(0, 0);
        assert_eq!(table.row_count(), 1);
        assert_eq!(table.column_count(), 1);
    }

    #[test]
    fn insert_and_remove_keep_grid_rectangular() {
        let mut table = Table::new(2, 2);
        table.insert_row(1);
        table.insert_column(0, "New");
        assert_eq!(table.row_count(), 3);
        assert_eq!(table.column_count(), 3);
        assert!(is_rectangular(&table));

        table.remove_row(0);
        table.remove_column(2);
        assert_eq!(table.row_count(), 2);
        assert_eq!(table.column_count(), 2);
        assert!(is_rectangular(&table));
    }

    #[test]
    fn remove_never_drops_below_one() {
        let mut table = Table::new(1, 1);
        table.remove_row(0);
        table.remove_column(0);
        assert_eq!(table.row_count(), 1);
        assert_eq!(table.column_count(), 1);
    }

    #[test]
    fn normalize_strips_nested_tables_from_cells() {
        let mut table = Table::new(1, 1);
        // Put a nested table inside a cell's line item, then normalize.
        let nested = Table::new(1, 1);
        table.rows[0].cells[0].items[0]
            .content
            .push(Inline::Table(nested));
        table.normalize();
        assert!(table.rows[0].cells[0]
            .items
            .iter()
            .all(|item| !item.has_table()));
    }
}
