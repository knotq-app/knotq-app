use knotq_model::Item;

use crate::make_minimal_workspace;

pub fn make_sample_workspace() -> knotq_model::Workspace {
    let mut workspace = make_minimal_workspace();
    if let Some(scheme) = workspace.schemes.values_mut().next() {
        scheme.items = vec![Item::new("Essay 1"), Item::new("Math HW")];
    }
    workspace
}
