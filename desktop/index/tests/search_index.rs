use knotq_index::search::{tokenize, SearchHit, SearchOptions, SearchTarget};
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
    let options = search_options();

    let professor = indexed
        .search_query(TimeFormat::TwelveHour, options)
        .run("prof");
    let research = indexed
        .search_query(TimeFormat::TwelveHour, options)
        .run("rsrch");

    assert!(professor.iter().any(|hit| hit.title == "Meet Professor"));
    assert!(research.iter().any(|hit| hit.scheme_name == "Research"));
}

#[test]
fn search_query_returns_direct_scheme_hits() {
    let indexed = IndexedWorkspace::build(workspace_with_item("Research", "Meet Professor"));
    let hits = indexed
        .search_query(TimeFormat::TwelveHour, search_options())
        .run("rsrch");

    assert!(hits.iter().any(|hit| {
        hit.title == "Research" && matches!(&hit.target, SearchTarget::Scheme { item_id: None, .. })
    }));
}

#[test]
fn search_query_ranks_item_title_matches_above_scheme_context_matches() {
    let mut workspace = Workspace::new();
    add_scheme(&mut workspace, "Alpha Project", 1, &["Book venue"]);
    add_scheme(&mut workspace, "Operations", 2, &["Alpha launch"]);
    let indexed = IndexedWorkspace::build(workspace);

    let hits = indexed
        .search_query(TimeFormat::TwelveHour, search_options())
        .run("alpha");
    let title_match = hit_position(&hits, "Alpha launch").unwrap();
    let scheme_context_match = hit_position(&hits, "Book venue").unwrap();

    assert!(title_match < scheme_context_match, "{hits:#?}");
}

#[test]
fn search_query_ranks_phrase_matches_above_loose_subsequences() {
    let mut workspace = Workspace::new();
    add_scheme(
        &mut workspace,
        "General",
        1,
        &["Personal launch archive note", "Plan launch"],
    );
    let indexed = IndexedWorkspace::build(workspace);

    let hits = indexed
        .search_query(TimeFormat::TwelveHour, search_options())
        .run("plan");
    let phrase_match = hit_position(&hits, "Plan launch").unwrap();
    let loose_match = hit_position(&hits, "Personal launch archive note").unwrap();

    assert!(phrase_match < loose_match, "{hits:#?}");
}

fn search_options() -> SearchOptions<'static> {
    SearchOptions {
        daily_queue_title: "Nut List",
        daily_queue_marker_color: 0,
    }
}

fn hit_position(hits: &[SearchHit], title: &str) -> Option<usize> {
    hits.iter().position(|hit| hit.title == title)
}

fn workspace_with_item(scheme_name: &str, item_text: &str) -> Workspace {
    let mut workspace = Workspace::new();
    add_scheme(&mut workspace, scheme_name, 1, &[item_text]);
    workspace
}

fn add_scheme(workspace: &mut Workspace, scheme_name: &str, color_index: u8, item_texts: &[&str]) {
    let mut scheme = Scheme::new(scheme_name, color_index);
    for text in item_texts {
        scheme.items.push(Item::new(*text));
    }
    let scheme_id = scheme.id;
    workspace.schemes.insert(scheme_id, scheme);
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme_id));
}
