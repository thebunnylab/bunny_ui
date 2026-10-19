use bunny_ui_core::preview::{ExecutionContext, ExecutionMode as Mode, ExecutionScope};
use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

#[test]
fn nested_contexts_restore_after_return_and_unwind() {
    assert!(ExecutionContext::current().is_none());
    let design = ExecutionScope::new(Mode::Design);
    let play = ExecutionScope::new(Mode::Play);
    design.with(|| {
        assert_eq!(
            ExecutionContext::current().unwrap().mode(),
            Some(Mode::Design)
        );
        let result = catch_unwind(AssertUnwindSafe(|| {
            play.with(|| {
                assert_eq!(
                    ExecutionContext::current().unwrap().mode(),
                    Some(Mode::Play)
                );
                panic!("leave the nested scope");
            });
        }));
        assert!(result.is_err());
        assert_eq!(
            ExecutionContext::current().unwrap().mode(),
            Some(Mode::Design)
        );
        play.with(|| {
            assert_eq!(
                ExecutionContext::current().unwrap().mode(),
                Some(Mode::Play)
            )
        });
        assert_eq!(
            ExecutionContext::current().unwrap().mode(),
            Some(Mode::Design)
        );
    });
    assert!(ExecutionContext::current().is_none());
}

#[test]
fn handles_do_not_keep_the_owner_alive_and_subscriptions_detach() {
    let owner = ExecutionScope::new(Mode::Design);
    let context = owner.with(|| ExecutionContext::current().unwrap());
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);
    let subscription = context.subscribe(move |mode| sink.borrow_mut().push(mode));
    assert!(
        events.borrow().is_empty(),
        "subscriptions observe subsequent changes"
    );
    assert!(owner.set_mode(Mode::Play));
    assert!(!owner.set_mode(Mode::Play));
    assert_eq!(*events.borrow(), vec![Some(Mode::Play)]);
    drop(subscription);
    owner.set_mode(Mode::Design);
    assert_eq!(events.borrow().len(), 1);
    let sink = Rc::clone(&events);
    let _subscription = context.subscribe(move |mode| {
        assert_eq!(ExecutionContext::current().unwrap().mode(), mode);
        sink.borrow_mut().push(mode);
    });
    drop(owner);
    assert_eq!(context.mode(), None);
    assert_eq!(*events.borrow(), vec![Some(Mode::Play), None]);
    context.with(|| assert_eq!(ExecutionContext::current().unwrap().mode(), None));
    assert!(ExecutionContext::current().is_none());
}

#[test]
fn reentrant_change_never_delivers_an_obsolete_mode_to_later_listeners() {
    let owner = Rc::new(ExecutionScope::new(Mode::Design));
    let weak = Rc::downgrade(&owner);
    let _first = owner.context().subscribe(move |mode| {
        if mode == Some(Mode::Play) {
            weak.upgrade().unwrap().set_mode(Mode::Design);
        }
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);
    let context = owner.context();
    let _second = context.clone().subscribe(move |mode| {
        assert_eq!(context.mode(), mode);
        sink.borrow_mut().push(mode);
    });
    owner.set_mode(Mode::Play);
    assert!(
        events.borrow().is_empty(),
        "the second listener has never left Design"
    );
    assert_eq!(owner.context().mode(), Some(Mode::Design));
}

#[test]
fn detached_listener_is_skipped_even_in_an_existing_notification_snapshot() {
    let owner = ExecutionScope::new(Mode::Design);
    let later = Rc::new(RefCell::new(None));
    let drop_later = Rc::clone(&later);
    let _first = owner.context().subscribe(move |_| {
        drop_later.borrow_mut().take();
    });
    *later.borrow_mut() = Some(owner.context().subscribe(|_| panic!("already detached")));
    owner.set_mode(Mode::Play);
    assert!(later.borrow().is_none());
}

#[test]
fn independent_owners_do_not_share_transitions_or_lifetime() {
    let first = ExecutionScope::new(Mode::Design);
    let second = ExecutionScope::new(Mode::Design);
    let context = second.context();
    let _subscription = context.subscribe(|_| panic!("another scene changed"));
    first.set_mode(Mode::Play);
    drop(first);
    assert_eq!(context.mode(), Some(Mode::Design));
    drop(_subscription);
}

#[test]
fn panicking_listener_does_not_hide_closure_from_other_services() {
    let owner = ExecutionScope::new(Mode::Design);
    let context = owner.context();
    let _first = context.subscribe(|_| panic!("one adapter failed"));
    let closed = Rc::new(std::cell::Cell::new(false));
    let sink = Rc::clone(&closed);
    let _second = context.subscribe(move |mode| sink.set(mode.is_none()));
    assert!(catch_unwind(AssertUnwindSafe(|| drop(owner))).is_err());
    assert!(closed.get());
    assert_eq!(context.mode(), None);
    assert!(ExecutionContext::current().is_none());
}

#[test]
fn closure_during_unwind_preserves_the_original_failure() {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let owner = ExecutionScope::new(Mode::Design);
        let context = owner.context();
        // Keep the registration alive through the scope's drop.
        let _subscription = context.subscribe(|_| panic!("secondary adapter failure"));
        let _owner = owner;
        panic!("original failure");
    }));
    assert_eq!(
        result.unwrap_err().downcast_ref::<&str>(),
        Some(&"original failure")
    );
    assert!(ExecutionContext::current().is_none());
}
