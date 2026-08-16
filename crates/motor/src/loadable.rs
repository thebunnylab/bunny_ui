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

    /// O lado snake_case de `localizedDescription` — é o que a camada
    /// tipada expõe; o campo camelCase fica para o port espelhado.
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
    /// The future is driven to completion synchronously with the crate-local
    /// `block_on` — the mocked web repository resolves instantly.
    pub fn load<F, E>(&self, resource: F)
    where
        F: Future<Output = Result<T, E>> + 'static,
        E: Into<LoadError> + 'static,
    {
        let mut value = self.wrappedValue();
        let cancelBag = CancelBag::new();
        value.setIsLoading(cancelBag.clone());
        self.set(value);

        let (task, _cancelled) = crate::cancel_bag::TaskHandle::new();
        // task.store(in: cancelBag)
        cancelBag.store(task);

        match crate::block_on(resource) {
            Ok(value) => self.set(Loadable::Loaded(value)),
            Err(error) => self.set(Loadable::Failed(error.into())),
        }
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
        assert_eq!(state.wrappedValue(), Loadable::Loaded(42));

        state.binding().load(async { Err::<u32, _>(LoadError::new("boom")) });
        assert_eq!(
            state.wrappedValue(),
            Loadable::Failed(LoadError::new("boom"))
        );
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
