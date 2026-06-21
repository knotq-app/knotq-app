use knotq_model::Item;

use super::HEADER_ROW;

/// Where a buffer row lives in the document tree. The editor keeps one flat
/// text buffer so the ordinary text pipeline can edit table cells too.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::scheme_editor) enum RowKind {
    #[default]
    Doc,
    TableAnchor,
    Cell,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::scheme_editor) struct RowPath {
    pub(in crate::scheme_editor) kind: RowKind,
    /// Buffer index of the owning table's anchor row. Rebuilt on every
    /// `build_buffer`, so it is valid only against the current row vector.
    pub(in crate::scheme_editor) anchor: usize,
    pub(in crate::scheme_editor) r: usize,
    pub(in crate::scheme_editor) c: usize,
    pub(in crate::scheme_editor) sub: usize,
    pub(in crate::scheme_editor) cell_lines: usize,
}

impl RowPath {
    pub(super) fn doc() -> Self {
        Self::default()
    }

    pub(super) fn anchor() -> Self {
        Self {
            kind: RowKind::TableAnchor,
            ..Default::default()
        }
    }

    pub(super) fn cell(anchor: usize, r: usize, c: usize, sub: usize, cell_lines: usize) -> Self {
        Self {
            kind: RowKind::Cell,
            anchor,
            r,
            c,
            sub,
            cell_lines,
        }
    }

    pub(in crate::scheme_editor) fn is_cell(&self) -> bool {
        self.kind == RowKind::Cell
    }

    /// A header cell is a cell whose row index is the [`HEADER_ROW`] sentinel.
    pub(in crate::scheme_editor) fn is_header_cell(&self) -> bool {
        self.kind == RowKind::Cell && self.r == HEADER_ROW
    }

    pub(in crate::scheme_editor) fn is_doc(&self) -> bool {
        self.kind == RowKind::Doc
    }

    pub(in crate::scheme_editor) fn is_table_anchor(&self) -> bool {
        self.kind == RowKind::TableAnchor
    }

    pub(in crate::scheme_editor) fn is_first_in_cell(&self) -> bool {
        self.kind == RowKind::Cell && self.sub == 0
    }

    pub(in crate::scheme_editor) fn is_last_in_cell(&self) -> bool {
        self.kind == RowKind::Cell && self.sub + 1 >= self.cell_lines
    }
}

#[derive(Clone)]
pub(in crate::scheme_editor) struct EditorRow {
    pub(in crate::scheme_editor) item: Item,
    pub(in crate::scheme_editor) path: RowPath,
}

impl EditorRow {
    pub(in crate::scheme_editor) fn doc(item: Item) -> Self {
        Self {
            item,
            path: RowPath::doc(),
        }
    }
}
