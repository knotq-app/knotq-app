use crate::AccessRole;

impl AccessRole {
    pub fn can_read(self) -> bool {
        matches!(self, Self::Owner | Self::Writer | Self::Reader)
    }

    pub fn can_write(self) -> bool {
        matches!(self, Self::Owner | Self::Writer)
    }
}
