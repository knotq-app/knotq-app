#[derive(Clone, Copy, Debug)]
pub(super) struct DateFieldSelection {
    pub(super) anchor: usize,
    pub(super) head: usize,
}

impl DateFieldSelection {
    pub(super) fn collapsed(offset: usize) -> Self {
        Self {
            anchor: offset,
            head: offset,
        }
    }

    pub(super) fn ordered(self) -> (usize, usize) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub(super) fn is_empty(self) -> bool {
        self.anchor == self.head
    }

    pub(super) fn reversed(self) -> bool {
        self.head < self.anchor
    }
}
