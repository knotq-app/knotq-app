#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TextLocation {
    pub row: usize,
    pub col: usize,
}

impl Ord for TextLocation {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.row
            .cmp(&other.row)
            .then_with(|| self.col.cmp(&other.col))
    }
}

impl PartialOrd for TextLocation {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextSelection {
    pub anchor: TextLocation,
    pub head: TextLocation,
}

impl TextSelection {
    pub fn collapsed(head: TextLocation) -> Self {
        Self { anchor: head, head }
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.head
    }

    pub fn reversed(self) -> bool {
        self.head < self.anchor
    }

    pub fn ordered(self) -> (TextLocation, TextLocation) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}
