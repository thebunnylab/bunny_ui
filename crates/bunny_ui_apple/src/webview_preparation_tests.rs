use super::*;
use bunny_ui::host::NetworkPolicy;

#[test]
fn preparation_is_limited_to_local_read_only_network_denied_documents() {
    let local = Document::new("<p>Mail</p>", "", NetworkPolicy::Deny);
    assert!(preparation_allowed(&local));
    assert!(!preparation_allowed(&local.clone().editable()));
    assert!(!preparation_allowed(&Document::new(
        "<p>Mail</p>",
        "https://example.org/",
        NetworkPolicy::Deny
    )));
    assert!(!preparation_allowed(&Document::new(
        "<p>Mail</p>",
        "",
        NetworkPolicy::RemoteImages
    )));
}

#[test]
fn requesting_and_dropping_preparation_does_not_allocate_native_engines() {
    let requested = HtmlPreparation::request();
    assert_eq!(requested.available(), 0);
    let held = Rc::downgrade(&requested.pool);
    drop(requested);
    assert!(held.upgrade().is_none());
}

#[test]
fn old_preparation_navigation_cannot_finish_the_claimed_document() {
    let path = "preparation-navigation-test";
    let old = 1usize as Id;
    let current = 2usize as Id;
    LETTERS.with(|letters| {
        letters.borrow_mut().insert(
            path.to_owned(),
            Letter {
                digest: 1,
                expected: true,
                focus: false,
                navigation: current,
            },
        );
    });
    assert!(!current_navigation(path, old));
    assert!(current_navigation(path, current));
    LETTERS.with(|letters| letters.borrow_mut().remove(path));
}
