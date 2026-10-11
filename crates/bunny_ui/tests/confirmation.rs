use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use bunny_ui_core as bunny_ui;

use bunny_ui::window::{
    Confirmation, ConfirmationError, ConfirmationKind, ConfirmationResponse, ConfirmationSlot,
    ConfirmationSpec, Lifecycle,
};

#[derive(Default)]
struct Wakes(AtomicUsize);
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn poll(
    future: &mut Confirmation,
    wakes: &Arc<Wakes>,
) -> Poll<Result<ConfirmationResponse, ConfirmationError>> {
    let waker = Waker::from(Arc::clone(wakes));
    Pin::new(future).poll(&mut Context::from_waker(&waker))
}

fn spec() -> ConfirmationSpec {
    ConfirmationSpec::new("Delete \"Cedar\"?", "This removes the selected project.").unwrap()
}

#[test]
fn confirmation_answers_are_catalog_words_in_every_language() {
    use bunny_ui::prelude::Locale;
    use bunny_ui::words::{TABLES, Word, Words};

    for table in &TABLES {
        let words = Words::for_locale(&Locale::new(table.tag));
        assert!(!words.get(Word::Yes).is_empty(), "{}", table.tag);
        assert!(!words.get(Word::No).is_empty(), "{}", table.tag);
        assert_ne!(words.get(Word::Yes), words.get(Word::No), "{}", table.tag);
    }
    let words = Words::for_locale(&Locale::new("pt-BR"));
    assert_eq!(words.get(Word::Yes), "Sim");
    assert_eq!(words.get(Word::No), "Não");
}

fn slot() -> ConfirmationSlot {
    ConfirmationSlot::new(Lifecycle::default().id(), ConfirmationKind::FrameworkAlert)
}

#[test]
fn text_rejects_empty_questions_and_embedded_nul_without_altering_unicode() {
    for question in ["", " \n\t", "Delete\0hidden"] {
        assert!(matches!(
            ConfirmationSpec::new(question, ""),
            Err(ConfirmationError::InvalidText(_))
        ));
    }
    assert!(ConfirmationSpec::new("Delete?", "hidden\0suffix").is_err());
    let value = ConfirmationSpec::new("Excluir \"Árvore 🌳\"?", "一行\nثان").unwrap();
    assert_eq!(value.question(), "Excluir \"Árvore 🌳\"?");
    assert_eq!(value.detail(), "一行\nثان");
}

#[test]
fn native_completion_is_single_and_wakes_only_the_current_waiter() {
    let slot = slot();
    let (mut future, request) = slot.start(spec()).unwrap();
    let first = Arc::new(Wakes::default());
    let last = Arc::new(Wakes::default());
    assert_eq!(poll(&mut future, &first), Poll::Pending);
    assert_eq!(poll(&mut future, &last), Poll::Pending);
    assert!(request.complete(Ok(ConfirmationResponse::Yes)));
    assert!(!request.complete(Ok(ConfirmationResponse::No)));
    assert_eq!(first.0.load(Ordering::SeqCst), 0);
    assert_eq!(last.0.load(Ordering::SeqCst), 1);
    assert_eq!(
        poll(&mut future, &last),
        Poll::Ready(Ok(ConfirmationResponse::Yes))
    );
    assert_eq!(future.kind(), ConfirmationKind::FrameworkAlert);
    assert!(slot.current().is_none());
}

#[test]
fn dropping_the_single_consumer_cancels_and_late_yes_cannot_escape() {
    let slot = slot();
    let (future, request) = slot.start(spec()).unwrap();
    let cancelled = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&cancelled);
    let reply = request.clone();
    request.on_cancel(move || {
        seen.fetch_add(1, Ordering::SeqCst);
        let _ = reply.complete(Ok(ConfirmationResponse::Yes));
    });
    drop(future);
    assert_eq!(cancelled.load(Ordering::SeqCst), 1);
    assert!(slot.current().is_none());
    assert!(!request.complete(Ok(ConfirmationResponse::Yes)));
    assert!(slot.start(spec()).is_ok());
}

#[test]
fn cancellation_waits_for_native_teardown_and_overrides_a_racing_answer() {
    let slot = slot();
    let (mut future, request) = slot.start(spec()).unwrap();
    request.on_cancel(|| {});
    let wakes = Arc::new(Wakes::default());
    assert_eq!(poll(&mut future, &wakes), Poll::Pending);
    assert!(future.cancel());
    assert!(!future.cancel());
    assert!(!request.on_cancel(|| panic!("native cancellation cannot be installed twice")));
    assert_eq!(poll(&mut future, &wakes), Poll::Pending);
    assert!(matches!(
        slot.start(spec()),
        Err(ConfirmationError::AlreadyOpen)
    ));
    assert!(request.complete(Ok(ConfirmationResponse::Yes)));
    assert_eq!(
        poll(&mut future, &wakes),
        Poll::Ready(Err(ConfirmationError::Cancelled))
    );
    assert_eq!(wakes.0.load(Ordering::SeqCst), 1);
}

#[test]
fn owner_retirement_resolves_the_pending_future_without_reusing_its_request() {
    let slot = slot();
    let (mut future, request) = slot.start(spec()).unwrap();
    let old = request.clone();
    let reply = request.clone();
    request.on_cancel(move || {
        let _ = reply.complete(Ok(ConfirmationResponse::Dismissed));
    });
    slot.owner_closed();
    assert_eq!(
        poll(&mut future, &Arc::default()),
        Poll::Ready(Err(ConfirmationError::OwnerClosed))
    );
    assert!(matches!(
        slot.start(spec()),
        Err(ConfirmationError::OwnerClosed)
    ));
    let next_owner =
        ConfirmationSlot::new(Lifecycle::default().id(), ConfirmationKind::FrameworkAlert);
    let (mut next, current) = next_owner.start(spec()).unwrap();
    assert!(!old.complete(Ok(ConfirmationResponse::Yes)));
    assert!(current.complete(Ok(ConfirmationResponse::No)));
    assert_eq!(
        poll(&mut next, &Arc::default()),
        Poll::Ready(Ok(ConfirmationResponse::No))
    );
}

#[test]
fn cancellation_and_completion_do_not_borrow_the_registry_during_callbacks() {
    let slot = slot();
    let (mut future, request) = slot.start(spec()).unwrap();
    let reentrant = slot.clone();
    let reply = request.clone();
    request.on_cancel(move || {
        assert!(reentrant.current().is_some());
        assert!(reply.complete(Ok(ConfirmationResponse::No)));
        let (next, request) = reentrant.start(spec()).unwrap();
        assert!(request.complete(Ok(ConfirmationResponse::Yes)));
        drop(next);
    });
    assert!(future.cancel());
    assert_eq!(
        poll(&mut future, &Arc::default()),
        Poll::Ready(Err(ConfirmationError::Cancelled))
    );
    assert!(slot.current().is_none());
}

#[test]
fn pre_presentation_drop_releases_the_slot_and_native_errors_remain_errors() {
    let slot = slot();
    let (future, request) = slot.start(spec()).unwrap();
    drop(future);
    assert!(!request.is_pending());
    assert!(slot.current().is_none());
    let (mut future, request) = slot.start(spec()).unwrap();
    let failure = ConfirmationError::Window(bunny_ui::window::WindowError::NativeFailure {
        operation: "native confirmation fixture",
        code: 17,
    });
    assert!(request.complete(Err(failure.clone())));
    assert_eq!(
        poll(&mut future, &Arc::default()),
        Poll::Ready(Err(failure))
    );
}
