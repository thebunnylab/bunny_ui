//! Where a node was written (`bunny_ui::provenance`): sites are recorded
//! only when asked, cost nothing when not, and point at the line the app
//! wrote — through `vstack!` and `text!` alike.

use bunny_ui::layout::{Proposal, Size};
use bunny_ui::prelude::*;
use bunny_ui::provenance::{NodeKind, Site, SitePlacement};
use bunny_ui::runtime::Runtime;
use bunny_ui_core as bunny_ui;

fn page() -> impl UnaryView {
    let line = line!();
    let _ = line;
    vstack!(text!("Hello").padding(), button(text("Go"), || {}).padding_length(4.0),).spacing(8.0)
}

fn commands(runtime: &Runtime, view: &impl View) -> Vec<String> {
    runtime
        .layout(view, Proposal::exact(Size { width: 320.0, height: 200.0 }))
        .display
        .as_slice()
        .iter()
        .map(|command| format!("{command:?}"))
        .collect()
}

/// Off, a pass is the pass it always was: no sites, and the same picture.
#[test]
fn recording_off_leaves_no_sites_and_the_same_display_list() {
    let quiet = Runtime::new();
    let before = commands(&quiet, &page());
    assert!(quiet.sites().is_empty());

    let watched = Runtime::new();
    watched.record_sites(true);
    let after = commands(&watched, &page());
    assert!(!watched.sites().is_empty(), "recording on fills the table");
    assert_eq!(before, after, "the wrappers are transparent to paint");
    watched.record_sites(false);
    assert!(watched.sites().is_empty(), "turning it off clears the table");
}

/// On, every builder and modifier of the page is there with the line the
/// app wrote — the macro's invocation for `vstack!` and `text!` — and the
/// parents nest as the code does.
#[test]
fn recording_on_sites_every_builder_and_modifier_at_the_apps_line() {
    let runtime = Runtime::new();
    runtime.record_sites(true);
    let page_line = line!() + 3; // `page()` is defined above; its vstack! sits on one line
    let _ = page_line;
    let _ = runtime.layout(&page(), Proposal::exact(Size { width: 320.0, height: 200.0 }));
    let sites: Vec<SitePlacement> = runtime.sites();
    let names: Vec<(String, Option<u32>)> = sites
        .iter()
        .map(|placement| {
            let name = match placement.kind {
                NodeKind::Modifier => format!(".{}", placement.modifier.unwrap_or("?")),
                other => other.name().to_owned(),
            };
            (name, placement.parent)
        })
        .collect();
    // the stack holds two modified children (`.spacing` is the stack's own
    // method, not a modifier); a text's modifier wraps the text, a button's
    // modifier wraps the button, and the button's label is sited under it
    assert_eq!(
        names,
        [
            ("vstack".to_owned(), None),
            (".padding".to_owned(), Some(0)),
            ("text".to_owned(), Some(1)),
            (".padding".to_owned(), Some(0)),
            ("button".to_owned(), Some(3)),
            ("text".to_owned(), Some(4)),
        ]
    );
    let this_file = file!();
    let vstack_line = sites[0].site;
    let Site::Caller(location) = vstack_line else { panic!("a builder's site is its caller") };
    assert!(location.file().ends_with("provenance.rs"), "{location}");
    assert_eq!(location.file(), this_file);
    // every site of the page is on the one line `page()` writes
    let lines: std::collections::HashSet<u32> = sites
        .iter()
        .filter_map(|placement| match placement.site {
            Site::Caller(location) => Some(location.line()),
            Site::Named(_) => None,
        })
        .collect();
    assert_eq!(lines.len(), 1, "one line wrote the whole page: {lines:?}");
    // frames nest: a child's frame lies inside its parent's
    for placement in &sites {
        if let Some(parent) = placement.parent {
            let outer = &sites[parent as usize].frame;
            let inner = &placement.frame;
            assert!(inner.origin.x >= outer.origin.x - 0.01 && inner.origin.y >= outer.origin.y - 0.01, "{placement:?} outside {outer:?}");
        }
    }
}

/// Two paddings fold into one node when nobody watches, and keep their
/// two sites when somebody does — with the same geometry either way.
#[test]
fn a_folded_modifier_keeps_both_sites_and_the_geometry() {
    let folded = || text("x").padding_length(4.0).padding_length(6.0);
    let quiet = Runtime::new();
    let before = commands(&quiet, &folded());
    let watched = Runtime::new();
    watched.record_sites(true);
    let after = commands(&watched, &folded());
    assert_eq!(before, after);
    let paddings = watched.sites().iter().filter(|p| p.modifier == Some("padding")).count();
    assert_eq!(paddings, 2);
}

/// A component boundary names the sites under it by its path.
#[test]
fn sites_under_a_component_carry_its_path() {
    #[derive(Clone)]
    struct Card;
    impl Component for Card {
        fn body(self) -> impl View {
            text("card")
        }
    }
    let runtime = Runtime::new();
    runtime.record_sites(true);
    let _ = runtime.layout(&vstack!(Card), Proposal::exact(Size { width: 100.0, height: 100.0 }));
    let sites = runtime.sites();
    let inner = sites.iter().find(|p| p.kind == NodeKind::Text).expect("the card's text");
    assert!(inner.path.as_deref().is_some_and(|path| path.contains("Card")), "{:?}", inner.path);
}

#[test]
fn retained_override_sets_restore_the_thread_and_keep_their_own_versions() {
    use bunny_ui::provenance::{OverrideSet, clear_overrides, overrides_version};
    clear_overrides();
    let outside = overrides_version();
    let first = OverrideSet::new(std::iter::empty());
    let second = OverrideSet::new(std::iter::empty());
    let first_version = first.with(overrides_version);
    let second_version = second.with(overrides_version);
    assert_ne!(first_version, second_version);
    first.with(|| {
        assert_eq!(overrides_version(), first_version);
        second.with(|| assert_eq!(overrides_version(), second_version));
        assert_eq!(overrides_version(), first_version);
    });
    assert_eq!(overrides_version(), outside);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        first.with(|| panic!("scene failed"))
    }));
    assert!(result.is_err());
    assert_eq!(overrides_version(), outside);
    assert_eq!(
        first.with(overrides_version),
        first_version,
        "entering another scene did not dirty this one"
    );
}
