use std::cell::RefCell;
use std::future::Future;
use std::pin::pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use super::{Answer, SavedPassword, SavedPasswordError, install_requester, saved_password};

/// Polls a request once — every requester here answers in place, or
/// keeps the answer for the test to give.
fn poll_once<F: Future>(future: std::pin::Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn a_thread_without_a_shell_is_told_the_service_is_not_there() {
    let request = pin!(saved_password());
    match poll_once(request) {
        Poll::Ready(Err(error)) => assert_eq!(error, SavedPasswordError::Unsupported),
        other => panic!("expected Unsupported at once, got {:?}", other.map(|r| r.map(|_| ()))),
    }
}

#[test]
fn the_system_answer_reaches_the_window() {
    install_requester(|answer: Answer| {
        answer.give(Ok(SavedPassword::new("ana@example.com", "correct horse")));
    });
    let request = pin!(saved_password());
    let Poll::Ready(Ok(saved)) = poll_once(request) else {
        panic!("the requester answered in place");
    };
    assert_eq!(saved.user(), "ana@example.com");
    assert_eq!(saved.password(), "correct horse");
}

#[test]
fn the_system_is_asked_on_the_first_poll_and_waits_for_the_choice() {
    let kept: Rc<RefCell<Option<Answer>>> = Rc::default();
    let asked = Rc::new(std::cell::Cell::new(0));
    install_requester({
        let kept = Rc::clone(&kept);
        let asked = Rc::clone(&asked);
        move |answer| {
            asked.set(asked.get() + 1);
            *kept.borrow_mut() = Some(answer);
        }
    });
    let mut request = pin!(saved_password());
    assert_eq!(asked.get(), 0, "making the future asks nobody");
    assert!(poll_once(request.as_mut()).is_pending(), "the sheet is up");
    assert_eq!(asked.get(), 1);
    kept.borrow_mut().take().expect("the shell kept the answer").give(Err(SavedPasswordError::Declined));
    match poll_once(request.as_mut()) {
        Poll::Ready(Err(error)) => assert_eq!(error, SavedPasswordError::Declined),
        _ => panic!("the cancel ends the request"),
    }
    assert_eq!(asked.get(), 1, "the system is asked once");
}

#[test]
fn a_shell_that_drops_the_request_ends_it_as_a_failure() {
    install_requester(drop::<Answer>);
    let request = pin!(saved_password());
    match poll_once(request) {
        Poll::Ready(Err(SavedPasswordError::Failed { code, reason })) => {
            assert_eq!(code, 0);
            assert!(reason.contains("without an answer"), "{reason}");
        }
        _ => panic!("an unanswered request must not wait forever"),
    }
}

#[test]
fn a_password_never_prints() {
    let saved = SavedPassword::new("ana@example.com", "correct horse");
    let shown = format!("{saved:?}");
    assert!(shown.contains("ana@example.com"), "{shown}");
    assert!(!shown.contains("correct horse"), "{shown}");
}

#[test]
fn every_refusal_says_what_happened() {
    let failed = SavedPasswordError::Failed { code: 1004, reason: String::from("not associated") };
    assert!(failed.to_string().contains("1004"));
    assert!(failed.to_string().contains("not associated"));
    assert!(!SavedPasswordError::Unsupported.to_string().is_empty());
    assert!(!SavedPasswordError::Declined.to_string().is_empty());
}
