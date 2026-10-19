use super::ModalScopes;

#[test]
fn independent_modal_scopes_do_not_gate_each_other() {
    let mut scopes = ModalScopes::default();
    scopes.set(1, vec![11]);
    assert!(scopes.is_blocked(1));
    assert!(!scopes.is_blocked(2));
    assert!(!scopes.is_blocked(11));
    scopes.set(2, vec![21]);
    scopes.end(11);
    assert!(!scopes.is_blocked(1));
    assert!(scopes.is_blocked(2));
    assert!(!scopes.is_blocked(21));
}

#[test]
fn only_the_top_dialog_is_eligible_in_a_nested_scope() {
    let mut scopes = ModalScopes::default();
    scopes.set(1, vec![11, 12]);
    assert!(scopes.is_blocked(1));
    assert!(scopes.is_blocked(11));
    assert!(!scopes.is_blocked(12));
    scopes.end(12);
    assert!(scopes.is_blocked(1));
    assert!(!scopes.is_blocked(11));
    scopes.end(11);
    assert_eq!(scopes.blocked().count(), 0);
}

#[test]
fn repeated_frames_do_not_accumulate_modal_holds() {
    let mut scopes = ModalScopes::default();
    for _ in 0..100 {
        scopes.set(1, vec![11, 12]);
    }
    assert!(scopes.is_blocked(11));
    assert!(!scopes.is_blocked(12));
    scopes.end(12);
    scopes.end(11);
    scopes.end(11);
    assert_eq!(scopes.blocked().count(), 0);
}

#[test]
fn layout_order_controls_eligibility_after_reopening_a_lower_dialog() {
    let mut scopes = ModalScopes::default();
    scopes.set(1, vec![11, 12]);
    scopes.end(11);
    assert!(scopes.is_blocked(1));
    assert!(!scopes.is_blocked(11));
    assert!(!scopes.is_blocked(12));
    scopes.set(1, vec![11, 12]);
    assert!(scopes.is_blocked(11));
    assert!(!scopes.is_blocked(12));
    scopes.set(1, vec![12, 11]);
    assert!(scopes.is_blocked(12));
    assert!(!scopes.is_blocked(11));
}

#[test]
fn teardown_forgets_all_old_identities_without_releasing_another_scope() {
    let mut scopes = ModalScopes::default();
    scopes.set(1, vec![11, 12]);
    scopes.set(2, vec![21]);
    scopes.forget(1);
    for old in [1, 11, 12] {
        assert!(!scopes.is_blocked(old));
    }
    assert!(scopes.is_blocked(2));
    scopes.set(1, vec![11]);
    assert!(scopes.is_blocked(1));
    assert!(!scopes.is_blocked(11));
    scopes.set(1, vec![]);
    assert!(scopes.is_blocked(2));
    assert!(!scopes.is_blocked(1));
}
