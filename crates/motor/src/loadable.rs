//! `Loadable<T>` + `LoadableSubject<T>` — direct port of Utilities/Loadable.swift.

use crate::cancel_bag::CancelBag;
use crate::state::Binding;
use std::future::Future;

/// `struct ValueIsMissingError: Error { … }`-style errors: all the Swift code
/// ever reads back is `localizedDescription`.
#[derive(Clone, Debug)]
pub struct LoadError {
    pub localizedDescription: String,
}

impl LoadError {
    pub fn new(localizedDescription: impl Into<String>) -> Self {
        LoadError { localizedDescription: localizedDescription.into() }
    }

    /// The snake_case side of `localizedDescription` — it is what the typed
    /// layer exposes; the camelCase field stays for the mirrored port.
    pub fn message(&self) -> &str {
        &self.localizedDescription
    }
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.localizedDescription)
    }
}

impl std::error::Error for LoadError {}

impl PartialEq for LoadError {
    fn eq(&self, other: &Self) -> bool {
        self.localizedDescription == other.localizedDescription
    }
}

/// `enum Loadable<T>`
#[derive(Clone, Debug)]
pub enum Loadable<T> {
    NotRequested,
    /// `.isLoading(last:cancelBag:)`
    IsLoading(Option<T>, CancelBag),
    Loaded(T),
    Failed(LoadError),
}

impl<T: Clone> Loadable<T> {
    /// `var value: T?`
    pub fn value(&self) -> Option<T> {
        match self {
            Loadable::Loaded(value) => Some(value.clone()),
            Loadable::IsLoading(last, _) => last.clone(),
            _ => None,
        }
    }

    /// `var error: Error?`
    pub fn error(&self) -> Option<LoadError> {
        match self {
            Loadable::Failed(error) => Some(error.clone()),
            _ => None,
        }
    }

    /// `mutating func setIsLoading(cancelBag:)`
    pub fn setIsLoading(&mut self, cancelBag: CancelBag) {
        *self = Loadable::IsLoading(self.value(), cancelBag);
    }

    /// `mutating func cancelLoading()`
    pub fn cancelLoading(&mut self) {
        if let Loadable::IsLoading(last, cancelBag) = self {
            cancelBag.cancel();
            if let Some(last) = last.clone() {
                *self = Loadable::Loaded(last);
            } else {
                *self = Loadable::Failed(LoadError::new("Canceled by user"));
            }
        }
    }
}

impl<T: Clone + PartialEq> PartialEq for Loadable<T> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Loadable::NotRequested, Loadable::NotRequested) => true,
            (Loadable::IsLoading(a, bagA), Loadable::IsLoading(b, bagB)) => {
                a == b && bagA.isEqual(bagB)
            }
            (Loadable::Loaded(a), Loadable::Loaded(b)) => a == b,
            (Loadable::Failed(a), Loadable::Failed(b)) => a.localizedDescription == b.localizedDescription,
            _ => false,
        }
    }
}

/// `typealias LoadableSubject<T> = Binding<Loadable<T>>`
pub type LoadableSubject<T> = Binding<Loadable<T>>;

impl<T: Clone + PartialEq + 'static> Binding<Loadable<T>> {
    /// `subject.load { try await resource() }`
    ///
    /// The resource goes on the engine's queue: the subject turns
    /// `.isLoading` NOW and takes its value (or its error) on the turn
    /// the future resolves. The `CancelBag` owns the running task, so
    /// `cancelLoading()` — and the bag dying with its view — ends the
    /// work where it stands.
    pub fn load<F, E>(&self, resource: F)
    where
        F: Future<Output = Result<T, E>> + 'static,
        E: Into<LoadError> + 'static,
    {
        let mut value = self.wrappedValue();
        let cancelBag = CancelBag::new();
        value.setIsLoading(cancelBag.clone());
        self.set(value);

        let subject = self.clone();
        let task = crate::task::spawn(async move {
            match resource.await {
                Ok(value) => subject.set(Loadable::Loaded(value)),
                Err(error) => subject.set(Loadable::Failed(error.into())),
            }
        });
        // task.store(in: cancelBag)
        cancelBag.store(crate::cancel_bag::RunningTask::new(task));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;

    #[test]
    fn load_roundtrip_through_a_binding() {
        let state = State::new(Loadable::<u32>::NotRequested);
        state.binding().load(async { Ok::<_, LoadError>(42) });
        assert!(
            matches!(state.wrappedValue(), Loadable::IsLoading(..)),
            "the subject says loading until the turn runs the task"
        );
        crate::task::poll_ready();
        assert_eq!(state.wrappedValue(), Loadable::Loaded(42));

        state.binding().load(async { Err::<u32, _>(LoadError::new("boom")) });
        crate::task::poll_ready();
        assert_eq!(
            state.wrappedValue(),
            Loadable::Failed(LoadError::new("boom"))
        );
    }

    #[test]
    fn cancel_loading_ends_the_task_it_owns() {
        let state = State::new(Loadable::<u32>::NotRequested);
        state.binding().load(std::future::pending::<Result<u32, LoadError>>());
        crate::task::poll_ready();
        assert_eq!(crate::task::pending(), 1, "a resource still in flight");

        // `cancelLoading` clears the bag; the bag held the task
        state.update(|value| value.cancelLoading());
        assert_eq!(crate::task::pending(), 0, "the work stopped with it");
    }

    #[test]
    fn cancel_loading_restores_last_or_fails() {
        let mut loading: Loadable<i32> = Loadable::IsLoading(Some(7), CancelBag::new());
        loading.cancelLoading();
        assert_eq!(loading, Loadable::Loaded(7));

        let mut loading: Loadable<i32> = Loadable::IsLoading(None, CancelBag::new());
        loading.cancelLoading();
        assert_eq!(
            loading,
            Loadable::Failed(LoadError::new("Canceled by user"))
        );
    }
}
