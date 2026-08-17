//! Fake port of SwiftUI/Combine for the CountriesSwiftUI Rust port.
//!
//! This is a *syntax* port — there is no real UI. Views are description trees
//! (`RenderNode`) that get printed. Semantics are faked single-threaded and
//! synchronous, which is honest to the "headless demo" goal:
//!
//! - `@State`         → [`State<T>`] (`Rc<RefCell<T>>`; views are `Clone` and cheap,
//!                       so value-type-view + heap-state, like real SwiftUI)
//! - `CurrentValueSubject` → [`Store<T>`] (synchronous watchers, no async)
//! - `AnyPublisher`   → pull-based [`AnyPublisher<V>`] polled by [`Runtime::pump`]
//! - `@Environment`   → [`Environment<T>`] resolved from [`Context`] / [`EnvironmentValues`]

#![forbid(unsafe_code)]
#![allow(non_snake_case)]
#![allow(non_camel_case_types)]
#![allow(dead_code)]

pub mod cancel_bag;
pub mod combine;
pub mod hash;
pub mod identity;
pub mod inspection;
pub mod loadable;
pub mod modifiers;
pub mod runtime;
pub mod state;
pub mod view;
pub mod views;

pub mod prelude;

/// Minimal `block_on` for futures that never really park (the mock URL session
/// resolves immediately). A real executor takes over when the engine grows one.
pub fn block_on<F: Future>(fut: F) -> F::Output {
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    let mut cx = Context::from_waker(Waker::noop());
    let mut fut = pin!(fut);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(out) => return out,
            Poll::Pending => std::hint::spin_loop(),
        }
    }
}
