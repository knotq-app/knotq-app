use knotq_index::search::{tokenize, SearchOptions};
use knotq_index::IndexedWorkspace;
use knotq_model::{Item, NodeRef, Scheme, TimeFormat, Workspace};

#[test]
fn tokenization_splits_words_and_punctuation() {
    assert_eq!(
        tokenize("Research: ELSAN-v2"),
        vec!["research", "elsan", "v2"]
    );
}

#[test]
fn search_query_matches_item_text_and_scheme_name() {
    let indexed = IndexedWorkspace::build(workspace_with_item("Research", "Meet Professor"));
    let options = SearchOptions {
        daily_queue_title: "Nut List",
        daily_queue_marker_color: 0,
    };

    let professor = indexed
        .search_query(TimeFormat::TwelveHour, options)
        .run("prof");
    let research = indexed
        .search_query(TimeFormat::TwelveHour, options)
        .run("rsrch");

    assert!(professor.iter().any(|hit| hit.title == "Meet Professor"));
    assert!(research.iter().any(|hit| hit.scheme_name == "Research"));
}

fn workspace_with_item(scheme_name: &str, item_text: &str) -> Workspace {
    let mut workspace = Workspace::new();
    let mut scheme = Scheme::new(scheme_name, 1);
    scheme.items.push(Item::new(item_text));
    let scheme_id = scheme.id;
    workspace.schemes.insert(scheme_id, scheme);
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme_id));
    workspace
}
