use knotq_model::{FolderId, SchemeId};

use crate::Command;

#[derive(Default, Debug, Clone)]
pub struct ChangeSet {
    pub folders: Vec<FolderId>,
    pub schemes: Vec<SchemeId>,
}

impl ChangeSet {
    pub fn touched_folder(mut self, id: FolderId) -> Self {
        if !self.folders.contains(&id) {
            self.folders.push(id);
        }
        self
    }

    pub fn touched_scheme(mut self, id: SchemeId) -> Self {
        if !self.schemes.contains(&id) {
            self.schemes.push(id);
        }
        self
    }

    pub fn merge(&mut self, other: ChangeSet) {
        for folder in other.folders {
            if !self.folders.contains(&folder) {
                self.folders.push(folder);
            }
        }
        for scheme in other.schemes {
            if !self.schemes.contains(&scheme) {
                self.schemes.push(scheme);
            }
        }
    }
}

#[derive(Debug)]
pub struct CommandReceipt {
    pub inverse: Command,
    pub touched: ChangeSet,
}
