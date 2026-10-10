use crate::channel::{normalize_channel_ref, ChannelTarget};
use crate::IndexedView;

pub struct ChannelQuery<'a> {
    indexed: IndexedView<'a>,
}

impl<'a> ChannelQuery<'a> {
    pub fn new(indexed: IndexedView<'a>) -> Self {
        Self { indexed }
    }

    pub fn resolve(&self, reference: &str) -> Option<ChannelTarget> {
        let key = normalize_channel_ref(reference);
        self.indexed.channel.tasks.get(&key).cloned()
    }
}
