use chrono::{DateTime, Duration, TimeZone, Utc};
use knotq_index::search::{search_hits_at, SearchHit, SearchOptions, SearchTarget};
use knotq_index::IndexedWorkspace;
use knotq_model::{Item, NodeRef, Scheme, TimeFormat, Workspace};

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
fn a_scheme_name_match_does_not_drag_in_every_line_of_the_scheme() {
    let mut workspace = Workspace::new();
    add_scheme(&mut workspace, "Alpha Project", 1, &["Book venue"]);
    add_scheme(&mut workspace, "Operations", 2, &["Alpha launch"]);

    let hits = run(&workspace, "alpha");

    assert_eq!(
        titles(&hits),
        ["Alpha Project", "Alpha launch"],
        "{hits:#?}"
    );
}

#[test]
fn letters_scattered_across_a_line_are_not_a_match() {
    let mut workspace = Workspace::new();
    add_scheme(
        &mut workspace,
        "General",
        1,
        &["Personal launch archive note", "Plan launch"],
    );

    let hits = run(&workspace, "plan");

    assert_eq!(titles(&hits), ["Plan launch"], "{hits:#?}");
}

#[test]
fn words_match_in_any_order_and_a_phrase_in_order_ranks_first() {
    let mut workspace = Workspace::new();
    add_scheme(
        &mut workspace,
        "General",
        1,
        &[
            "Report on the new tax",
            "File tax report",
            "Tax season",
            "Report card",
        ],
    );

    let hits = run(&workspace, "tax report");

    assert_eq!(
        titles(&hits),
        ["File tax report", "Report on the new tax"],
        "{hits:#?}"
    );
}

#[test]
fn a_scheme_name_narrows_a_search_for_its_lines() {
    let mut workspace = Workspace::new();
    add_scheme(&mut workspace, "Work", 1, &["Quarterly report", "Lunch"]);
    add_scheme(&mut workspace, "School", 2, &["Book report"]);

    let hits = run(&workspace, "work report");

    assert_eq!(titles(&hits), ["Quarterly report"], "{hits:#?}");
}

#[test]
fn whole_word_beats_word_start_beats_inside_a_word() {
    let mut workspace = Workspace::new();
    add_scheme(
        &mut workspace,
        "General",
        1,
        &["Scatter seeds", "Catalog the books", "Feed the cat"],
    );

    let hits = run(&workspace, "cat");

    assert_eq!(
        titles(&hits),
        ["Feed the cat", "Catalog the books", "Scatter seeds"],
        "{hits:#?}"
    );
}

#[test]
fn a_typo_still_finds_the_line() {
    let mut workspace = Workspace::new();
    add_scheme(
        &mut workspace,
        "General",
        1,
        &["Dentist appointment", "Buy groceries"],
    );

    assert_eq!(titles(&run(&workspace, "dentsit")), ["Dentist appointment"]);
    assert_eq!(titles(&run(&workspace, "grocries")), ["Buy groceries"]);
    assert_eq!(titles(&run(&workspace, "apointm")), ["Dentist appointment"]);
}

#[test]
fn guesses_are_dropped_once_enough_lines_match_as_typed() {
    let mut workspace = Workspace::new();
    add_scheme(
        &mut workspace,
        "General",
        1,
        &[
            "Plan a",
            "Plan b",
            "Plan c",
            "Plan d",
            "Plan e",
            "Plane tickets",
            "Play outside",
        ],
    );

    let hits = run(&workspace, "plan");

    assert!(!titles(&hits).contains(&"Play outside"), "{hits:#?}");
    assert_eq!(hits.len(), 6, "{hits:#?}");
}

#[test]
fn accents_and_case_do_not_matter() {
    let mut workspace = Workspace::new();
    add_scheme(
        &mut workspace,
        "General",
        1,
        &["Café with Zoë", "Über ride"],
    );

    assert_eq!(titles(&run(&workspace, "cafe zoe")), ["Café with Zoë"]);
    assert_eq!(titles(&run(&workspace, "UBER")), ["Über ride"]);
    assert_eq!(titles(&run(&workspace, "café")), ["Café with Zoë"]);
}

#[test]
fn text_without_spaces_matches_inside_a_run() {
    let mut workspace = Workspace::new();
    add_scheme(
        &mut workspace,
        "General",
        1,
        &["明日の会議の準備", "買い物"],
    );

    assert_eq!(titles(&run(&workspace, "会議")), ["明日の会議の準備"]);
}

#[test]
fn a_query_with_no_words_matches_literally() {
    let mut workspace = Workspace::new();
    add_scheme(&mut workspace, "General", 1, &["a -> b", "a and b"]);

    assert_eq!(titles(&run(&workspace, "->")), ["a -> b"]);
}

#[test]
fn open_lines_rank_above_finished_ones() {
    let mut workspace = Workspace::new();
    let mut scheme = Scheme::new("General", 1);
    scheme.items.push(Item::new("Call the bank").done());
    scheme.items.push(Item::new("Call the plumber"));
    insert_scheme(&mut workspace, scheme);

    let hits = run(&workspace, "call");

    assert_eq!(
        titles(&hits),
        ["Call the plumber", "Call the bank"],
        "{hits:#?}"
    );
}

#[test]
fn a_line_due_around_now_ranks_above_one_long_past() {
    let mut workspace = Workspace::new();
    let mut scheme = Scheme::new("General", 1);
    scheme
        .items
        .push(Item::new("Team sync").with_start(now() - Duration::days(200)));
    scheme
        .items
        .push(Item::new("Team sync").with_start(now() + Duration::days(2)));
    let upcoming = scheme.items[1].id;
    insert_scheme(&mut workspace, scheme);

    let hits = run(&workspace, "team sync");

    assert!(
        matches!(&hits[0].target, SearchTarget::Scheme { item_id: Some(id), .. } if *id == upcoming),
        "{hits:#?}"
    );
}

#[test]
fn recent_daily_pages_rank_above_old_ones() {
    let mut workspace = Workspace::new();
    let today = now().with_timezone(&chrono::Local).date_naive();
    let mut old = Scheme::new("old", 0);
    old.items.push(Item::new("Water plants last year"));
    let mut recent = Scheme::new("recent", 0);
    recent.items.push(Item::new("Water plants yesterday"));
    for (date, scheme) in [
        (today - Duration::days(300), old),
        (today - Duration::days(1), recent),
    ] {
        workspace.daily_queue.insert(date, scheme.id);
        workspace.schemes.insert(scheme.id, scheme);
    }

    let hits = run(&workspace, "water plants");

    assert_eq!(
        titles(&hits),
        ["Water plants yesterday", "Water plants last year"],
        "{hits:#?}"
    );
}

#[test]
fn an_empty_query_lists_the_workspace_in_order() {
    let mut workspace = Workspace::new();
    add_scheme(&mut workspace, "General", 1, &["First", "Second"]);

    let hits = run(&workspace, "  ");

    assert_eq!(
        titles(&hits),
        ["Calendar", "Nut List", "General", "First", "Second"]
    );
}

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 9, 12, 0, 0).unwrap()
}

fn run(workspace: &Workspace, query: &str) -> Vec<SearchHit> {
    search_hits_at(
        workspace,
        TimeFormat::TwelveHour,
        query,
        search_options(),
        now(),
    )
}

fn titles(hits: &[SearchHit]) -> Vec<&str> {
    hits.iter().map(|hit| hit.title.as_str()).collect()
}

fn search_options() -> SearchOptions<'static> {
    SearchOptions {
        daily_queue_title: "Nut List",
        daily_queue_marker_color: 0,
    }
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
    insert_scheme(workspace, scheme);
}

fn insert_scheme(workspace: &mut Workspace, scheme: Scheme) {
    let scheme_id = scheme.id;
    workspace.schemes.insert(scheme_id, scheme);
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme_id));
}
