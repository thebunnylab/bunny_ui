use bunny_ui_core::window::{Lifecycle, Relationships, WindowError, WindowParentKind};

#[test]
fn inherited_visibility_does_not_erase_an_explicit_hide() {
    let lifecycle = Lifecycle::default();
    lifecycle.set_owner_hidden(true);
    assert!(lifecycle.is_hidden());
    lifecycle.set_hidden(true);
    lifecycle.set_owner_hidden(false);
    assert!(lifecycle.is_hidden());
    lifecycle.set_hidden(false);
    assert!(!lifecycle.is_hidden());
}

#[test]
fn relationships_reject_foreign_stale_and_cyclic_lifetimes() {
    let graph = Relationships::default();
    let a = Lifecycle::default().id();
    let b = Lifecycle::default().id();
    let foreign = Lifecycle::default().id();
    graph.register(a);
    graph.register(b);
    assert!(matches!(
        graph.change(a, Some(foreign)),
        Err(WindowError::StaleWindow)
    ));
    assert!(matches!(
        graph.change(a, Some(a)),
        Err(WindowError::RelationshipCycle)
    ));
    graph
        .change(b, Some(a))
        .unwrap()
        .commit(WindowParentKind::AppKitChild)
        .unwrap();
    assert_eq!(graph.parent(b).unwrap().unwrap().window(), a);
    assert!(matches!(
        graph.change(a, Some(b)),
        Err(WindowError::RelationshipCycle)
    ));
    graph.retire(b);
    assert_eq!(graph.parent(b), Err(WindowError::StaleWindow));
}

#[test]
fn a_failed_native_change_does_not_publish_a_relationship() {
    let graph = Relationships::default();
    let a = Lifecycle::default().id();
    let b = Lifecycle::default().id();
    graph.register(a);
    graph.register(b);
    drop(graph.change(b, Some(a)).unwrap());
    assert_eq!(graph.parent(b), Ok(None));
    graph
        .change(b, Some(a))
        .unwrap()
        .commit(WindowParentKind::Win32Owner)
        .unwrap();
    assert_eq!(
        graph.parent(b).unwrap().unwrap().kind(),
        WindowParentKind::Win32Owner
    );
}

#[test]
fn reentrancy_is_rejected_without_holding_a_registry_borrow() {
    let graph = Relationships::default();
    let a = Lifecycle::default().id();
    let b = Lifecycle::default().id();
    graph.register(a);
    graph.register(b);
    let pending = graph.change(b, Some(a)).unwrap();
    assert_eq!(graph.parent(b), Ok(None));
    assert!(matches!(
        graph.change(a, Some(b)),
        Err(WindowError::RelationshipInProgress)
    ));
    graph.retire(a);
    assert_eq!(
        pending.commit(WindowParentKind::WaylandParent),
        Err(WindowError::StaleWindow)
    );
    assert_eq!(graph.parent(b), Ok(None));
    graph
        .change(b, None)
        .unwrap()
        .commit(WindowParentKind::WaylandParent)
        .unwrap();
}

#[test]
fn a_retired_proposed_parent_preserves_the_previous_live_owner() {
    let graph = Relationships::default();
    let previous = Lifecycle::default().id();
    let proposed = Lifecycle::default().id();
    let child = Lifecycle::default().id();
    for id in [previous, proposed, child] {
        graph.register(id);
    }
    graph
        .change(child, Some(previous))
        .unwrap()
        .commit(WindowParentKind::AppKitChild)
        .unwrap();
    let pending = graph.change(child, Some(proposed)).unwrap();
    graph.retire(proposed);
    assert_eq!(
        pending.commit(WindowParentKind::AppKitChild),
        Err(WindowError::StaleWindow)
    );
    assert_eq!(graph.parent(child).unwrap().unwrap().window(), previous);
}

#[test]
fn detachment_and_retirement_preserve_independent_lifetimes() {
    let graph = Relationships::default();
    let root = Lifecycle::default().id();
    let child = Lifecycle::default().id();
    let grandchild = Lifecycle::default().id();
    for id in [root, child, grandchild] {
        graph.register(id);
    }
    graph
        .change(child, Some(root))
        .unwrap()
        .commit(WindowParentKind::X11Transient)
        .unwrap();
    graph
        .change(grandchild, Some(child))
        .unwrap()
        .commit(WindowParentKind::X11Transient)
        .unwrap();
    assert_eq!(graph.children(root), vec![child]);
    graph
        .change(grandchild, None)
        .unwrap()
        .commit(WindowParentKind::X11Transient)
        .unwrap();
    assert!(graph.children(child).is_empty());
    graph.retire(root);
    assert_eq!(graph.parent(child), Ok(None));
    assert_eq!(graph.parent(grandchild), Ok(None));
}
