use std::sync::mpsc::{self, Receiver, Sender};

use knotq_commands::ChangeSet;

#[derive(Clone, Debug)]
pub enum AppEvent {
    WorkspaceChanged(ChangeSet),
    SettingsChanged,
    SelectionChanged,
    ExternalModificationApplied,
}

#[derive(Default)]
pub struct EventBus {
    subscribers: Vec<Sender<AppEvent>>,
}

impl EventBus {
    pub fn subscribe(&mut self) -> Receiver<AppEvent> {
        let (sender, receiver) = mpsc::channel();
        self.subscribers.push(sender);
        receiver
    }

    pub fn emit(&mut self, event: AppEvent) {
        self.subscribers
            .retain(|subscriber| subscriber.send(event.clone()).is_ok());
    }
}
