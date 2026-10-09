//! Bounded debug hints for eager text snapshots.
//!
//! ## Wiring
//! A hint is emitted once per text callsite after a component repeats a state
//! read and changes fixed text on the same source line. This is a candidate,
//! not data-flow analysis: intentional snapshots are valid, multiline reads
//! may be missed, and proximity cannot prove that rebuilding was avoidable.
//! Release builds retain no observations and print no hints.

use std::panic::Location;

/// A possible eager text binding found during a component rebuild.
#[derive(Clone, Debug)]
pub struct EagerText {
    /// Component type whose body constructed the text.
    pub component: &'static str,
    /// The state or binding read on the text's source line.
    pub read: &'static Location<'static>,
    /// Fixed text construction site; use `text(state)` or `text!` if reactive.
    pub text: &'static Location<'static>,
}

/// Takes the pending hints on this thread (at most 128); empty in release.
pub fn take_eager_text() -> Vec<EagerText> {
    #[cfg(debug_assertions)]
    {
        debug::take()
    }
    #[cfg(not(debug_assertions))]
    {
        Vec::new()
    }
}

pub(crate) fn body<R>(component: &'static str, run: impl FnOnce() -> R) -> R {
    #[cfg(debug_assertions)]
    let _guard = debug::enter(component);
    #[cfg(not(debug_assertions))]
    let _ = component;
    run()
}

#[track_caller]
pub(crate) fn read() {
    #[cfg(debug_assertions)]
    debug::read(Location::caller());
}

#[track_caller]
pub(crate) fn text(content: &crate::bind::TextSource) {
    #[cfg(debug_assertions)]
    debug::text(Location::caller(), content);
    #[cfg(not(debug_assertions))]
    let _ = content;
}

#[cfg(debug_assertions)]
mod debug {
    use super::EagerText;
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};
    use std::panic::Location;

    const LIMIT: usize = 128;
    type Site = &'static Location<'static>;
    #[derive(Default)]
    struct State {
        body: Option<Body>,
        // Hashing the words keeps bounded metadata; no user text is retained.
        seen: HashMap<(String, Site), u64>,
        warned: HashSet<Site>,
        pending: Vec<EagerText>,
    }
    struct Body {
        component: &'static str,
        read: Option<Site>,
    }
    thread_local! {
        static STATE: RefCell<State> = RefCell::new(State::default());
    }
    pub(super) struct Guard(Option<Body>);
    impl Drop for Guard {
        fn drop(&mut self) {
            STATE.with(|state| state.borrow_mut().body = self.0.take());
        }
    }
    pub(super) fn enter(component: &'static str) -> Guard {
        Guard(STATE.with(|state| {
            state.borrow_mut().body.replace(Body {
                component,
                read: None,
            })
        }))
    }
    pub(super) fn read(site: Site) {
        STATE.with(|state| {
            if let Some(body) = &mut state.borrow_mut().body {
                body.read = Some(site);
            }
        });
    }
    pub(super) fn text(site: Site, source: &crate::bind::TextSource) {
        use std::hash::{Hash, Hasher};
        let hint = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let body = state.body.as_mut()?;
            let read = body.read.take()?;
            let component = body.component;
            if read.file() != site.file() || read.line() != site.line() {
                return None;
            }
            let crate::bind::TextSource::Fixed(words) = source else {
                return None;
            };
            if state.warned.contains(&site) {
                return None;
            }
            let scope = motor::identity::current_view_path()?;
            let key = (scope, site);
            if state.seen.len() >= LIMIT && !state.seen.contains_key(&key) {
                return None;
            }
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            words.hash(&mut hasher);
            let hash = hasher.finish();
            let before = state.seen.insert(key, hash)?;
            if before == hash || state.warned.len() >= LIMIT {
                return None;
            }
            state.warned.insert(site);
            let hint = EagerText {
                component,
                read,
                text: site,
            };
            state.pending.push(hint.clone());
            Some(hint)
        });
        if let Some(hint) = hint {
            eprintln!(
                "bunny_ui: possible eager text in {} at {} (state read at {}); if this should bind directly, use text(state), text(binding), or text! instead of text(state.get()). Intentional snapshots are valid.",
                hint.component, hint.text, hint.read
            );
        }
    }
    pub(super) fn take() -> Vec<EagerText> {
        STATE.with(|state| std::mem::take(&mut state.borrow_mut().pending))
    }
}
