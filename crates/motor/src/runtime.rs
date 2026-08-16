//! `Runtime` — the fake main-thread: renders the tree, pumps effects,
//! and stabilizes (re-renders until the printed tree stops changing —
//! our stand-in for SwiftUI's diffing).

use crate::state::{Context, EffectFn, EnvironmentValues};
use crate::view::View;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub struct Runtime {
    ctx: Context,
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

impl Runtime {
    pub fn new() -> Self {
        Runtime { ctx: Context::default() }
    }

    pub fn with_environment(values: EnvironmentValues) -> Self {
        Runtime { ctx: Context { values, effects: Rc::new(RefCell::new(Vec::new())) } }
    }

    pub fn context(&self) -> Context {
        self.ctx.clone()
    }

    /// Builds the tree once; effects registered along the way are collected
    /// (and `onAppear` actions have already fired during rendering).
    pub fn render(&self, root: &impl View) -> String {
        self.ctx.effects.borrow_mut().clear();
        root.render(&self.ctx).print()
    }

    /// Drains registered effects (`onReceive`, `onChange`, `query`).
    /// Returns whether any of them observed a change.
    pub fn pump(&self) -> bool {
        let effects: Vec<EffectFn> = self.ctx.effects.borrow().clone();
        effects.iter().any(|effect| effect(&self.ctx))
    }

    /// Render → pump → re-render until the tree is stable (max 8 cycles).
    pub fn renderStable(&self, root: &impl View) -> String {
        let mut previous = String::new();
        for _ in 0..8 {
            let printed = self.render(root);
            // pump first: side effects fired by THIS render's onAppear
            // nodes must be observed before declaring the tree stable
            let observedChange = self.pump();
            if printed == previous && !observedChange {
                return printed;
            }
            previous = printed;
        }
        previous
    }
}

/// A identidade de um efeito entre renders — ou um nome escolhido à mão
/// (o `concat!(file!(), …)` do motor), ou o callsite capturado por
/// `#[track_caller]` (o caminho sem cerimônia da camada tipada).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Site {
    Named(&'static str),
    Caller(&'static std::panic::Location<'static>),
}

impl From<&'static str> for Site {
    fn from(name: &'static str) -> Self {
        Site::Named(name)
    }
}

impl From<&'static std::panic::Location<'static>> for Site {
    fn from(location: &'static std::panic::Location<'static>) -> Self {
        Site::Caller(location)
    }
}

impl std::fmt::Display for Site {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Site::Named(name) => f.write_str(name),
            Site::Caller(location) => write!(f, "{location}"),
        }
    }
}

/// Persistent per-site storage for change detection across renders.
/// (Single-threaded by design — the whole fake main actor.)
pub fn effect_slot<V: 'static>(site: impl Into<Site>) -> Rc<RefCell<Option<V>>> {
    thread_local! {
        static SLOTS: RefCell<HashMap<Site, Rc<dyn std::any::Any>>> =
            RefCell::new(HashMap::new());
    }
    let key = site.into();
    SLOTS.with(|slots| {
        if let Some(any) = slots.borrow().get(&key).cloned()
            && let Ok(cell) = any.downcast::<RefCell<Option<V>>>()
        {
            return cell;
        }
        let cell: Rc<RefCell<Option<V>>> = Rc::new(RefCell::new(None));
        slots.borrow_mut().insert(key, cell.clone());
        cell
    })
}
