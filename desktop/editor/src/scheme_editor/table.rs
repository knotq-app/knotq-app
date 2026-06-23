//! Table grid geometry and chrome.
//!
//! A table is flattened into the editor's one text buffer: an anchor row
//! reserves vertical space and owns the grid chrome, while each cell line is a
//! real editable row positioned inside the grid via a [`CellSlot`].

use super::*;

mod chrome;
mod layout;
mod structure;

pub(super) const MIN_COL_W: f32 = 132.0;
pub(super) const CELL_PAD_X: f32 = 10.0;
pub(super) const CELL_PAD_Y: f32 = 6.0;
pub(super) const GRID_TOP_GAP: f32 = 6.0;
const ROW_MIN_H: f32 = 30.0;
const GRID_BOTTOM_GAP: f32 = 2.0;
const CONTROL_H: f32 = 16.0;
const CONTROL_BTN: f32 = 14.0;
const RIGHT_MARGIN: f32 = CONTROL_BTN + 10.0;

/// Hit-test slack below the grid (the add-row control strip), so a click at the
/// very bottom of a cell reads as that cell instead of falling through to the
/// caret at the end of the table — the cell's visual padding makes that spot
/// feel like part of the cell. The add-row "+" is matched first in
/// `on_mouse_down`, so it still wins inside this slack.
pub(super) const BOTTOM_HIT_SLACK: f32 = CONTROL_H + GRID_BOTTOM_GAP;
const CELL_LINE_HEIGHT: f32 = TEXT_LINE_HEIGHT;

pub(super) fn grid_left_content() -> Pixels {
    px(-(CHECKBOX_SIZE + CHECKBOX_GAP))
}

pub(super) fn grid_left_content_for_indent(indent_x: Pixels) -> Pixels {
    grid_left_content() + indent_x
}

pub(super) fn table_content_width_for_indent(wrap_width: Pixels, indent_x: Pixels) -> Pixels {
    (wrap_width - px(TEXT_LEFT_PAD) - indent_x + px(CHECKBOX_SIZE + CHECKBOX_GAP)
        - px(RIGHT_MARGIN))
    .max(px(MIN_COL_W))
}

#[derive(Clone, Copy, Debug)]
pub(super) struct CellSlot {
    pub(super) text_left: Pixels,
    pub(super) col_left: Pixels,
    pub(super) top: Pixels,
    pub(super) width: Pixels,
}

#[derive(Clone)]
pub(super) struct TableLayout {
    pub(super) col_x: Vec<Pixels>,
    pub(super) col_w: Vec<Pixels>,
    pub(super) grid_w: Pixels,
    pub(super) header_h: Pixels,
    pub(super) body_band_h: Vec<Pixels>,
    pub(super) block_height: Pixels,
}

impl TableLayout {
    pub(super) fn grid_h(&self) -> Pixels {
        self.header_h + self.body_band_h.iter().fold(px(0.0), |acc, h| acc + *h)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TableControlKind {
    AddRow,
    AddColumn,
}

#[derive(Clone, Copy)]
pub(super) struct TableControlHitbox {
    pub(super) bounds: Bounds<Pixels>,
    pub(super) anchor_row: usize,
    pub(super) kind: TableControlKind,
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indented_table_geometry_shifts_and_shrinks_with_anchor_indent() {
        let wrap_width = px(720.0);
        let indent = px(INDENT_WIDTH * 2.0);

        assert_eq!(
            grid_left_content_for_indent(indent),
            grid_left_content() + indent
        );
        assert_eq!(
            table_content_width_for_indent(wrap_width, indent),
            table_content_width_for_indent(wrap_width, px(0.0)) - indent
        );
    }
}
