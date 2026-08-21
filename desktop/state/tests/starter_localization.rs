//! The starter workspace is the first thing a new install shows, and it is the
//! one piece of user data this app writes on the user's behalf. It has to come
//! out in their language, and it has to come out *complete* — a missing catalog
//! key does not fail loudly here, it seeds a scheme whose lines read
//! `starter.daily.coffee`.
//!
//! Everything runs inside one test: `set_locale` is process-global, so separate
//! `#[test]`s in this binary would race each other's locale.

use chrono::NaiveDate;
use knotq_model::{ItemContent, Workspace};
use knotq_state::make_default_workspace_for_date;

fn all_lines(workspace: &Workspace) -> Vec<String> {
    let mut lines: Vec<String> = workspace
        .schemes
        .values()
        .flat_map(|scheme| {
            std::iter::once(scheme.name.clone()).chain(scheme.items.iter().map(|item| {
                match &item.content {
                    ItemContent::Text { text } => text.clone(),
                    _ => String::new(),
                }
            }))
        })
        .collect();
    lines.retain(|line| !line.is_empty());
    lines
}

#[test]
fn the_starter_workspace_is_translated_everywhere_and_complete() {
    let today = NaiveDate::from_ymd_opt(2026, 8, 20).unwrap();

    let english = all_lines(&make_default_workspace_for_date(today));
    assert!(
        english.iter().any(|line| line.contains("Make coffee")),
        "the English starter workspace lost its content: {english:?}"
    );
    // The author's own note about how they use KnotQ. It is the reason the daily
    // page reads like a person wrote it; a rewrite of the seed must not drop it.
    assert!(
        english
            .iter()
            .any(|line| line.contains("paradoxical state") && line.contains("actionable tasks")),
        "the daily page lost the note about how KnotQ is used"
    );

    for locale in knotq_l10n::available_locales() {
        let code = knotq_l10n::set_locale(locale.code);
        assert_eq!(code, locale.code, "locale {} did not activate", locale.code);
        let lines = all_lines(&make_default_workspace_for_date(today));

        // A key with no catalog entry falls through to the key itself, which is
        // what a half-translated starter workspace looks like on disk.
        for line in &lines {
            assert!(
                !line.starts_with("starter."),
                "{}: unresolved catalog key {line:?}",
                locale.code
            );
        }
        assert_eq!(
            lines.len(),
            english.len(),
            "{}: the starter workspace has a different number of lines",
            locale.code
        );

        if locale.code == "en" {
            continue;
        }
        // Not a single English line left: every locale is fully translated, so a
        // catalog that silently falls back to English shows up here.
        let untranslated: Vec<&String> = lines
            .iter()
            .filter(|line| english.contains(line))
            .filter(|line| !line.contains("KnotQ"))
            // `Daily <date>` is deliberately not localized. It is a stored
            // canonical value, not display text: both shells compare a daily
            // scheme's name against it to decide whether the user renamed the
            // day, and they render a localized label instead. Translating it
            // would make that comparison fail for every day created under a
            // different language — including the user's own, after a switch.
            .filter(|line| {
                **line != knotq_state::daily_queue_scheme_name(today)
                    && **line
                        != knotq_state::daily_queue_scheme_name(today - chrono::Duration::days(1))
            })
            .collect();
        assert!(
            untranslated.is_empty(),
            "{}: still in English: {untranslated:?}",
            locale.code
        );
    }
    knotq_l10n::set_locale("en");
}

/// The editor only treats *two-character* markers as emphasis (`**`, `__`, `==`,
/// `~~`); a single `*` is literal text. Seed content that uses one renders the
/// asterisks to the user, in every language at once.
#[test]
fn starter_content_uses_only_markers_the_editor_renders() {
    let today = NaiveDate::from_ymd_opt(2026, 8, 20).unwrap();
    for line in all_lines(&make_default_workspace_for_date(today)) {
        let single_asterisks = line.replace("**", "").chars().filter(|c| *c == '*').count();
        assert_eq!(
            single_asterisks, 0,
            "{line:?} uses a single `*`, which the editor renders literally — use `__` for italic"
        );
        for marker in ["**", "__", "=="] {
            assert_eq!(
                line.matches(marker).count() % 2,
                0,
                "{line:?} has an unbalanced {marker}"
            );
        }
    }
}
