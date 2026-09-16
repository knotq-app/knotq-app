//! The first population of an empty scheme document: byte-identical on every
//! replica that populates it from the same content, and edits made before it
//! land as edits.
use super::super::*;

use knotq_model::{Item, ItemId};

/// Fixed ids, like the desktop starter workspace: every install builds this
/// exact scheme on its own.
fn starter() -> Scheme {
    let mut scheme = Scheme::new("Start here", 0);
    scheme.id = "00000000-0000-8000-8000-000000000101".parse().unwrap();
    for (index, text) in ["Thesis", "Argument", "Final draft"]
        .into_iter()
        .enumerate()
    {
        let mut item = Item::new(text);
        item.id = format!("00000000-0000-8000-8000-00000000100{}", index + 3)
            .parse::<ItemId>()
            .unwrap();
        scheme.items.push(item);
    }
    scheme
}

fn texts(doc: &YrsSchemeDocument) -> Vec<String> {
    doc.scheme_items()
        .unwrap()
        .iter()
        .map(|item| item.text())
        .collect()
}

#[test]
fn replicas_populating_the_same_content_write_identical_bytes() {
    let document = DocumentId::new();
    let first = YrsSchemeDocument::new(document);
    let second = YrsSchemeDocument::new(document);
    first.sync_scheme(&starter()).unwrap();
    second.sync_scheme(&starter()).unwrap();
    assert_eq!(first.encode_state_v1(), second.encode_state_v1());

    // Exchanging them changes nothing: each already holds the other's operations.
    let before = first.encode_state_v1();
    assert!(!first.apply_update_v1(&second.encode_state_v1()).unwrap());
    assert_eq!(first.encode_state_v1(), before);
    assert_eq!(texts(&first), ["Thesis", "Argument", "Final draft"]);
}

#[test]
fn edits_made_before_population_land_as_edits() {
    let document = DocumentId::new();
    let base = starter();
    let first = YrsSchemeDocument::new(document);
    let first_update = first.sync_scheme(&base).unwrap().unwrap().update_v1;

    let mut edited = base.clone();
    let deleted = edited.items.remove(0).id;
    edited.items[0].set_text("Retyped");
    let second = YrsSchemeDocument::new(document);
    let second_update = second
        .sync_scheme_from_base(Some(&base), &edited)
        .unwrap()
        .unwrap()
        .update_v1;

    first.apply_update_v1(&second_update).unwrap();
    second.apply_update_v1(&first_update).unwrap();
    for (label, doc) in [("first", &first), ("second", &second)] {
        let items = doc.scheme_items().unwrap();
        assert!(
            items.iter().all(|item| item.id != deleted),
            "{label}: the line deleted before population came back"
        );
        assert_eq!(texts(doc), ["Retyped", "Final draft"], "{label}");
    }
}

#[test]
fn different_content_populates_under_different_operations() {
    let document = DocumentId::new();
    let mut renamed = starter();
    renamed.items[2].set_text("Last draft");
    let first = YrsSchemeDocument::new(document);
    let second = YrsSchemeDocument::new(document);
    first.sync_scheme(&starter()).unwrap();
    second.sync_scheme(&renamed).unwrap();
    assert_ne!(first.state_vector_v1(), second.state_vector_v1());

    // Concurrent populations still merge commutatively into one state.
    let first_state = first.encode_state_v1();
    first.apply_update_v1(&second.encode_state_v1()).unwrap();
    second.apply_update_v1(&first_state).unwrap();
    assert_eq!(texts(&first), texts(&second));
}

#[test]
fn a_document_is_populated_only_once() {
    let document = DocumentId::new();
    let doc = YrsSchemeDocument::new(document);
    assert!(doc.is_unpopulated());
    doc.sync_scheme(&starter()).unwrap();
    assert!(!doc.is_unpopulated());

    // Emptying the scheme leaves tombstones, not an unpopulated document, so a
    // later write is an ordinary edit rather than a second population.
    let mut empty = starter();
    empty.items.clear();
    doc.sync_scheme(&empty).unwrap();
    assert!(!doc.is_unpopulated());
    let base = starter();
    let update = doc.sync_scheme_from_base(Some(&base), &base).unwrap();
    assert!(update.is_some(), "restoring the lines is an edit");
    assert_eq!(texts(&doc), ["Thesis", "Argument", "Final draft"]);
}

/// The population clientID hashes the CONTENT, not the encoding, so these bytes
/// must never change for the same content without bumping
/// `SCHEME_POPULATION_ENCODING_VERSION` — otherwise two builds would write
/// different operations under the same `(clientID, clock)`.
#[test]
fn scheme_population_encoding_is_pinned() {
    const PINNED: &str = "74c2eff839f054532dbfedb6e9ca7c97950d8e516fe79a0477ef403d35571348";
    let document: DocumentId = "00000000-0000-8000-8000-000000000201".parse().unwrap();
    let doc = YrsSchemeDocument::new(document);
    doc.sync_scheme(&starter()).unwrap();
    let digest = Sha256::digest(doc.encode_state_v1());
    let actual: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(
        actual, PINNED,
        "the first-population encoding changed: bump SCHEME_POPULATION_ENCODING_VERSION \
         and re-pin"
    );
}

/// The creation clientID hashes the CONTENT, not the encoding, so these bytes
/// must never change for the same id and content without bumping
/// `ITEM_CREATION_ENCODING_VERSION` — otherwise two builds would write different
/// operations under the same `(clientID, clock)`.
#[test]
fn item_creation_encoding_is_pinned() {
    const PINNED: &str = "62773a23bac0ac564b32b59b04e032ccc043a9dd3d5a9e6d09633198ae3617c7";
    let document: DocumentId = "00000000-0000-8000-8000-000000000201".parse().unwrap();
    // A FIXED item id: `Item::new` mints a random one, which would re-pin itself
    // on every run.
    const ITEM: &str = "00000000-0000-8000-8000-000000000401";
    let item = knotq_model::Item::new("Thesis");
    let content =
        super::super::scheme_content::normalize_inline_content(&item.content.to_inlines());
    let update = super::super::scheme_content::build_item_creation_update(document, ITEM, &content)
        .expect("build creation update");
    let digest = Sha256::digest(update);
    let actual: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(
        actual, PINNED,
        "the item creation encoding changed: bump ITEM_CREATION_ENCODING_VERSION and re-pin"
    );
}
