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
                reusable: true,
            },
        );
    });
    assert!(!current_navigation(path, old));
    assert!(current_navigation(path, current));
    LETTERS.with(|letters| letters.borrow_mut().remove(path));
}

#[test]
fn recycled_mounts_cannot_accept_an_old_bridge_channel() {
    let view = 3usize as Id;
    let first = register_mount(view);
    assert!(current_channel(view, &format!("bunny_{first}")).is_some());
    let second = register_mount(view);
    assert_ne!(first, second);
    assert!(current_channel(view, &format!("bunny_{first}")).is_none());
    assert_eq!(
        current_channel(view, &format!("bunny_{second}")),
        Some("bunny")
    );
    assert!(current_channel(view, "bunny").is_none());
    MOUNTS.with(|mounts| mounts.borrow_mut().remove(&(view as usize)));
}
#[test]
fn native_scripts_scope_every_bridge_channel_to_the_mount() {
    let script = scoped_script(WEBKIT_BOOT, 17);
    assert!(script.contains("messageHandlers.bunny_17.postMessage"));
    let script = scoped_script(&webkit_eval_wrap(1, "document.body.innerText", true), 18);
    assert!(script.contains("messageHandlers.bunnyEval_18.postMessage"));
    assert!(!script.contains("messageHandlers.bunnyEval.postMessage"));
}

#[test]
fn a_blank_navigation_in_progress_is_not_a_ready_engine() {
    let requested = HtmlPreparation::request();
    requested.pool.borrow_mut().cleaning.push(CleaningEngine {
        engine: SpareEngine(null_mut()),
        navigation: 1usize as Id,
        origin: EngineOrigin::Fresh,
    });
    assert_eq!(requested.available(), 0);
    assert_eq!(requested.pool.borrow().len(), 1);
}

#[test]
fn retire_only_owns_the_matching_native_host() {
    let path = "independent-native-host";
    let actual = 10usize as Id;
    let unrelated = 11usize as Id;
    PAGES.with(|pages| pages.borrow_mut().insert(path.into(), actual));
    let generation = register_mount(actual);
    assert!(!reuse::owns_host(path, unrelated));
    assert_eq!(key_of(actual).as_deref(), Some(path));
    assert!(current_channel(actual, &format!("bunny_{generation}")).is_some());
    PAGES.with(|pages| pages.borrow_mut().remove(path));
    MOUNTS.with(|mounts| mounts.borrow_mut().remove(&(actual as usize)));
}


#[test]
fn a_claimed_engine_keeps_its_reserve_slot_until_retirement() {
    let requested = HtmlPreparation::request();
    requested.pool.borrow_mut().ready.extend([
        SpareEngine(null_mut()), SpareEngine(null_mut()),
    ]);
    let local = Document::new("<p>Selected message</p>", "", NetworkPolicy::Deny);
    assert!(take_prepared(&local).is_some());
    assert_eq!(requested.available(), 1);
    assert_eq!(requested.pool.borrow().occupied(), 2,
        "displaying a prepared message must not trigger fresh WebKit construction");
}


#[test]
fn releasing_a_claim_preserves_the_other_pool_and_is_idempotent() {
    let first = HtmlPreparation::request();
    let second = HtmlPreparation::request();
    first.pool.borrow_mut().borrowed.insert(101);
    second.pool.borrow_mut().borrowed.insert(102);
    let home = release_prepared(102usize as Id).unwrap();
    assert!(Rc::ptr_eq(&home, &second.pool));
    assert_eq!(first.pool.borrow().occupied(), 1);
    assert_eq!(second.pool.borrow().occupied(), 0);
    assert!(release_prepared(102usize as Id).is_none());
    assert!(release_prepared(103usize as Id).is_none());
}

#[test]
fn swept_hosts_release_only_their_own_reservation() {
    let requested = HtmlPreparation::request();
    requested.pool.borrow_mut().borrowed.extend([201, 202]);
    PAGES.with(|pages| {
        pages.borrow_mut().insert("retired".into(), 201usize as Id);
        pages.borrow_mut().insert("still-mounted".into(), 202usize as Id);
    });
    sweep(&["still-mounted".into()]);
    assert_eq!(requested.pool.borrow().occupied(), 1);
    assert!(requested.pool.borrow().borrowed.contains(&202));
    sweep(&[]);
    assert_eq!(requested.pool.borrow().occupied(), 0);
}

#[test]
fn claims_and_cleaning_share_the_same_preparation_budget() {
    let requested = HtmlPreparation::request();
    requested.pool.borrow_mut().borrowed.insert(301);
    requested.pool.borrow_mut().cleaning.push(CleaningEngine {
        engine: SpareEngine(null_mut()),
        navigation: 302usize as Id,
        origin: EngineOrigin::Retired,
    });
    assert_eq!(requested.pool.borrow().occupied(), 2);
    assert_eq!(requested.available(), 0);
    release_prepared(301usize as Id);
    assert_eq!(requested.pool.borrow().occupied(), 1);
}

#[test]
fn fresh_preparation_places_an_empty_engine_before_starting_its_blank_barrier() {
    let requested = HtmlPreparation::request();
    let steps = RefCell::new(Vec::new());
    refill_preparation(
        &requested.pool,
        || {
            steps.borrow_mut().push("allocate");
            SpareEngine(null_mut())
        },
        |view| {
            assert!(view.is_null());
            steps.borrow_mut().push("place");
        },
        |pool, engine| {
            steps.borrow_mut().push("blank");
            pool.borrow_mut().cleaning.push(CleaningEngine {
                engine,
                navigation: null_mut(),
                origin: EngineOrigin::Fresh,
            });
        },
    );
    assert_eq!(
        *steps.borrow(),
        ["allocate", "place", "blank", "allocate", "place", "blank"]
    );
    assert_eq!(
        requested.available(),
        0,
        "association cannot bypass the blank barrier"
    );
    assert_eq!(requested.pool.borrow().occupied(), 2);
}

#[test]
fn fresh_preparation_never_repositions_a_retired_or_borrowed_engine() {
    let requested = HtmlPreparation::request();
    requested.pool.borrow_mut().borrowed.insert(401);
    requested.pool.borrow_mut().cleaning.push(CleaningEngine {
        engine: SpareEngine(null_mut()),
        navigation: 402usize as Id,
        origin: EngineOrigin::Retired,
    });
    refill_preparation(
        &requested.pool,
        || panic!("the two existing engines already occupy the reserve"),
        |_| panic!("a private or borrowed view cannot enter fresh placement"),
        |_, _| panic!("an existing blank barrier cannot restart"),
    );
    assert_eq!(requested.pool.borrow().occupied(), 2);
    assert_eq!(requested.available(), 0);
}
