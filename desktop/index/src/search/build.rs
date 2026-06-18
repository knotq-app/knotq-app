use knotq_model::Workspace;

use super::{tokenize, SearchDocument, SearchIndex};

pub fn build_search_index(workspace: &Workspace) -> SearchIndex {
    let mut documents = Vec::new();
    for scheme in workspace.iter_schemes() {
        documents.push(SearchDocument {
            scheme_id: scheme.id,
            item_id: None,
            text: scheme.name.clone(),
            tokens: tokenize(&scheme.name),
        });
        for item in &scheme.items {
            let text = item.text();
            if text.trim().is_empty() {
                continue;
            }
            documents.push(SearchDocument {
                scheme_id: scheme.id,
                item_id: Some(item.id),
                text: text.clone(),
                tokens: tokenize(&format!("{} {}", scheme.name, text)),
            });
        }
    }
    SearchIndex { documents }
}
