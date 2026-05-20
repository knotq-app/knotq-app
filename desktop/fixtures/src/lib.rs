pub mod minimal;
pub mod recurring;
pub mod sample;

pub use minimal::make_minimal_workspace;
pub use recurring::make_recurring_workspace;
pub use sample::make_sample_workspace;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_workspace_links_scheme_from_root() {
        let workspace = make_minimal_workspace();
        let scheme_id = *workspace.schemes.keys().next().unwrap();
        let root = workspace.folders.get(&workspace.root).unwrap();
        assert!(root
            .children
            .contains(&knotq_model::NodeRef::Scheme(scheme_id)));
    }
}
