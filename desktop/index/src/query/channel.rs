use crate::channel::{normalize_channel_ref, ChannelTarget};
use crate::IndexedWorkspace;

pub struct ChannelQuery<'a> {
    indexed: &'a IndexedWorkspace,
}

impl<'a> ChannelQuery<'a> {
    pub fn new(indexed: &'a IndexedWorkspace) -> Self {
        Self { indexed }
    }

    pub fn resolve(&self, reference: &str) -> Option<ChannelTarget> {
        let key = normalize_channel_ref(reference);
        self.indexed.channel.tasks.get(&key).cloned()
    }
}
