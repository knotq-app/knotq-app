use std::sync::mpsc::{self, Receiver, Sender};

use knotq_commands::Command;

use crate::AppState;

pub trait ExternalModification: Send + 'static {
    fn apply(&self, state: &mut AppState) -> anyhow::Result<Option<Command>>;
}

pub struct ExternalModificationQueue {
    sender: Sender<Box<dyn ExternalModification>>,
}

impl ExternalModificationQueue {
    pub fn new() -> (Self, Receiver<Box<dyn ExternalModification>>) {
        let (sender, receiver) = mpsc::channel();
        (Self { sender }, receiver)
    }

    pub fn push(
        &self,
        modification: impl ExternalModification,
    ) -> Result<(), mpsc::SendError<Box<dyn ExternalModification>>> {
        self.sender.send(Box::new(modification))
    }
}
