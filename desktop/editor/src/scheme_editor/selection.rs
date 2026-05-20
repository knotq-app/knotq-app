use crate::line_map::TextLocation;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct TextSelection {
    pub(super) anchor: TextLocation,
    pub(super) head: TextLocation,
}

impl TextSelection {
    pub(super) fn collapsed(head: TextLocation) -> Self {
        Self { anchor: head, head }
    }

    pub(super) fn is_empty(self) -> bool {
        self.anchor == self.head
    }

    pub(super) fn reversed(self) -> bool {
        self.head < self.anchor
    }

    pub(super) fn ordered(self) -> (TextLocation, TextLocation) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}
