use std::collections::HashMap;

use knotq_model::DocumentId;

use crate::{PersistedCrdtState, PersistedDocumentState};

impl PersistedCrdtState {
    pub fn from_states(states: &HashMap<DocumentId, Vec<u8>>) -> Self {
        let mut documents = states
            .iter()
            .map(|(document, state_v1)| PersistedDocumentState {
                document: *document,
                state_v1: state_v1.clone(),
            })
            .collect::<Vec<_>>();
        // Stable order keeps the on-disk file diff-friendly and deterministic.
        documents.sort_by(|a, b| a.document.0.cmp(&b.document.0));
        Self { documents }
    }

    pub fn into_states(self) -> HashMap<DocumentId, Vec<u8>> {
        self.documents
            .into_iter()
            .map(|entry| (entry.document, entry.state_v1))
            .collect()
    }
}
