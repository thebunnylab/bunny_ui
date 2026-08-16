//! Fake Combine — synchronous, single-threaded, pull-based.
//!
//! - `CurrentValueSubject<State, Never>` → [`Store<State>`]
//! - `AnyPublisher<Value, Failure>`       → [`AnyPublisher<Value>`] (polled)
//! - `PassthroughSubject<Value, Never>`   → [`PassthroughSubject<Value>`]

use std::cell::RefCell;
use std::rc::Rc;

struct StoreInner<T> {
    /// Identidade de dependência: uma view que leu `value()` durante o
    /// body depende do store INTEIRO (granularidade de objeto, como um
    /// ObservableObject) — `send`/`update` sujam quem leu.
    dep_id: u64,
    value: RefCell<T>,
    watchers: RefCell<Vec<Rc<dyn Fn(&T)>>>,
}

/// `typealias Store<State> = CurrentValueSubject<State, Never>`
#[derive(Clone)]
pub struct Store<T> {
    inner: Rc<StoreInner<T>>,
}

impl<T: Clone + 'static> Store<T> {
    pub fn new(value: T) -> Self {
        Store {
            inner: Rc::new(StoreInner {
                dep_id: crate::identity::next_store_id(),
                value: RefCell::new(value),
                watchers: RefCell::new(Vec::new()),
            }),
        }
    }

    /// `subject.value`
    pub fn value(&self) -> T {
        crate::identity::record_read(crate::identity::DepKey::Store(self.inner.dep_id));
        self.inner.value.borrow().clone()
    }

    /// `subject.value = newValue` (notifies subscribers)
    pub fn send(&self, value: T) {
        *self.inner.value.borrow_mut() = value;
        crate::identity::record_write(crate::identity::DepKey::Store(self.inner.dep_id));
        self.notify();
    }

    /// `store.bulkUpdate { … }` — also the `store[\.keyPath] = value` fake.
    pub fn update(&self, f: impl FnOnce(&mut T)) {
        f(&mut self.inner.value.borrow_mut());
        crate::identity::record_write(crate::identity::DepKey::Store(self.inner.dep_id));
        self.notify();
    }

    /// `store.sink { … }` registration (retain the returned handle is not needed —
    /// watchers live as long as the store, like `.store(in:)` into a bag).
    pub fn subscribe(&self, f: impl Fn(&T) + 'static) {
        self.inner.watchers.borrow_mut().push(Rc::new(f));
    }

    /// `store.updates(for: \.keyPath)` → `map(keyPath).removeDuplicates()`
    pub fn updates<K: Clone + PartialEq + 'static>(
        &self,
        key: impl Fn(&T) -> K + 'static,
    ) -> AnyPublisher<K> {
        let inner = self.inner.clone();
        AnyPublisher::from_source(move || Some(key(&inner.value.borrow())))
    }

    fn notify(&self) {
        let watchers = self.inner.watchers.borrow().clone();
        let value = self.value();
        for watcher in watchers {
            watcher(&value);
        }
    }
}

/// `AnyPublisher<Output, Failure>` — a pull-based value source with
/// change detection (`removeDuplicates` happens at poll time).
/// `None` from the source means "no current value" (PassthroughSubject
/// that has not sent anything yet).
pub struct AnyPublisher<V> {
    source: Rc<dyn Fn() -> Option<V>>,
    last: Rc<RefCell<Option<V>>>,
}

impl<V: Clone + PartialEq + 'static> AnyPublisher<V> {
    pub fn from_source(source: impl Fn() -> Option<V> + 'static) -> Self {
        AnyPublisher { source: Rc::new(source), last: Rc::new(RefCell::new(None)) }
    }

    /// `publisher.map { … }`
    pub fn map<W: Clone + PartialEq + 'static>(&self, f: impl Fn(V) -> W + 'static) -> AnyPublisher<W> {
        let source = self.source.clone();
        AnyPublisher::from_source(move || source().map(&f))
    }

    /// `.eraseToAnyPublisher()`
    pub fn eraseToAnyPublisher(self) -> Self {
        self
    }

    /// Returns the value if it changed since the previous poll
    /// (the `onReceive` delivery). `pub` so the typed `bunny_ui` layer can
    /// build its own onReceive effects.
    pub fn poll(&self) -> Option<V> {
        let Some(value) = (self.source)() else { return None };
        let mut last = self.last.borrow_mut();
        if last.as_ref() == Some(&value) {
            None
        } else {
            *last = Some(value.clone());
            Some(value)
        }
    }
}

impl<V: Clone + 'static> Clone for AnyPublisher<V> {
    fn clone(&self) -> Self {
        // Shares the `last` cell: clones of one publisher are one subscription.
        AnyPublisher { source: self.source.clone(), last: self.last.clone() }
    }
}

/// `PassthroughSubject<Output, Never>`
#[derive(Clone)]
pub struct PassthroughSubject<V> {
    last: Rc<RefCell<Option<V>>>,
    watchers: Rc<RefCell<Vec<Rc<dyn Fn(&V)>>>>,
}

impl<V: Clone + PartialEq + 'static> PassthroughSubject<V> {
    pub fn new() -> Self {
        PassthroughSubject {
            last: Rc::new(RefCell::new(None)),
            watchers: Rc::new(RefCell::new(Vec::new())),
        }
    }

    pub fn send(&self, value: V) {
        *self.last.borrow_mut() = Some(value);
        let watchers = self.watchers.borrow().clone();
        for watcher in watchers {
            if let Some(value) = self.last.borrow().as_ref() {
                watcher(value);
            }
        }
    }

    /// `subject.sink { … }` — registra o watcher; `.store(in:)` guarda o
    /// cancellable num `CancelBag` (o handle descartado continua inscrito,
    /// como uma subscription retida do Combine).
    pub fn sink(&self, f: impl Fn(&V) + 'static) -> Subscription {
        let watcher: Rc<dyn Fn(&V)> = Rc::new(f);
        self.watchers.borrow_mut().push(watcher.clone());
        let watchers = self.watchers.clone();
        Subscription {
            remove: Rc::new(move || {
                watchers.borrow_mut().retain(|w| !Rc::ptr_eq(w, &watcher));
            }),
        }
    }

    pub fn eraseToAnyPublisher(&self) -> AnyPublisher<V> {
        let last = self.last.clone();
        AnyPublisher::from_source(move || last.borrow().clone())
    }

}

/// Combine's `AnyCancellable` (o retorno de `sink`).
pub struct Subscription {
    remove: Rc<dyn Fn()>,
}

impl Subscription {
    /// `cancellable.store(in: bag)`
    pub fn store_in(self, bag: &crate::cancel_bag::CancelBag) {
        bag.store(Rc::new(self));
    }
}

impl crate::cancel_bag::Cancellable for Subscription {
    fn cancel(&self) {
        (self.remove)();
    }
}

impl<V: Clone + PartialEq + 'static> Default for PassthroughSubject<V> {
    fn default() -> Self {
        Self::new()
    }
}

/// Lets `.onReceive` accept either a publisher or a subject directly.
pub trait IntoPublisher<V: Clone + PartialEq + 'static> {
    fn into_publisher(self) -> AnyPublisher<V>;
}

impl<V: Clone + PartialEq + 'static> IntoPublisher<V> for AnyPublisher<V> {
    fn into_publisher(self) -> AnyPublisher<V> {
        self
    }
}

impl<V: Clone + PartialEq + 'static> IntoPublisher<V> for &PassthroughSubject<V> {
    fn into_publisher(self) -> AnyPublisher<V> {
        self.eraseToAnyPublisher()
    }
}

impl<V: Clone + PartialEq + 'static> IntoPublisher<V> for PassthroughSubject<V> {
    fn into_publisher(self) -> AnyPublisher<V> {
        self.eraseToAnyPublisher()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_notifies_and_updates_filters() {
        let store = Store::new((1, 100));
        let seen = Rc::new(RefCell::new(Vec::<i32>::new()));
        let seen2 = seen.clone();
        store.subscribe(move |(a, _)| seen2.borrow_mut().push(*a));
        store.send((2, 100));
        assert_eq!(*seen.borrow(), vec![2]);

        let publisher = store.updates(|(_, b)| *b);
        let publisher = publisher.clone();
        assert_eq!(publisher.poll(), Some(100)); // initial value delivered once
        assert_eq!(publisher.poll(), None); // removeDuplicates
        store.update(|(_, b)| *b = 200);
        assert_eq!(publisher.poll(), Some(200));
    }

    #[test]
    fn map_composes() {
        let store = Store::new(1);
        let publisher = store.updates(|v| *v).map(|v| v % 2 == 0).eraseToAnyPublisher();
        assert_eq!(publisher.poll(), Some(false));
        store.send(4);
        assert_eq!(publisher.poll(), Some(true));
    }
}
