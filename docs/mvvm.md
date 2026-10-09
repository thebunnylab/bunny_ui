# Presentation models

A ViewModel is a Rust struct containing presentation properties and command
methods. Each `State<T>` is an independent reactive property; domain services
remain constructor arguments and need not depend on Bunny UI.

Prefer a `vm` field on the view: the struct declares the dependency, and its
body composes the interface from the model's properties and commands. Both
field-based models and body-local `view_model` declarations are supported.

## A model supplied to the view

```rust
#[derive(Clone, Copy)]
struct CounterModel {
    count: State<i32>,
}

impl CounterModel {
    fn new() -> Self {
        Self { count: State::new(0) }
    }

    fn increment(self) {
        self.count.add(1);
    }

    fn caption(self) -> Derived<String> {
        derived(move || format!("Count: {}", self.count))
    }
}

#[derive(Clone, Copy)]
struct Counter {
    vm: CounterModel,
}

impl Component for Counter {
    fn body(self) -> impl View {
        vstack!(
            text(self.vm.caption()),
            button(text("Increment"), move || self.vm.increment()),
        )
    }
}

// Application assembly, outside rendering:
let counter = Counter { vm: CounterModel::new() };
```

The model above uses `State`'s application lifetime. The view can be recreated
or temporarily hidden without disposing of that externally owned state. A
`vm` field stores the supplied handles; it does not implicitly initialize,
reinitialize or adopt them into the receiving component's lifetime.

## A model owned by a component

Declaring a model in a body remains valid when that component should manage
its lifetime. It can still pass the result into a child view's `vm` field:

```rust
#[derive(Clone, Copy)]
struct CounterScreen;

impl Component for CounterScreen {
    fn body(self) -> impl View {
        let vm = view_model(CounterModel::new);
        Counter { vm }
    }
}
```

Here `CounterScreen` owns the model; `Counter` declares its dependency and
composes the controls. A component may also use the local `vm` directly in its
body. The initialization scope determines ownership in either form, not the
presence of a field, and each property keeps the same granular subscriptions.

`view_model` initializes once at that callsite for the mounted component.
Parent rebuilds preserve it. Removing the owning view releases the model and
state constructed inside its initializer; a retained state handle fails loudly
if used after that. Child views may receive the model by value. Keyed rows keep
their models with their keys. Repeated declarations in one loop need separate
keyed views; calling the same declaration site twice does not create two models.

Initializer arguments seed the first mount. Use a new view identity to recreate
a model for a different document or account within the same root or window
scene (`Runtime::scene`). Work attached with `.task` still
belongs to the view and is cancelled by unmounting; the ViewModel introduces no
executor. A model created explicitly outside rendering follows `State`'s
application lifetime, so that construction is for app-owned state, not rows.

`derived` declares a read-only computation. Reads subscribe wherever the derived
property is used; `text(derived_property)` caches through the text node's existing
binding. It is not a global memo: calling `.get()` evaluates the computation.
Use `property.binding()` for two-way fields and normal methods for commands.
`counter_mvvm` demonstrates both and tests the commands without a window.
`profile_mvvm` adds a two-way editable form, a derived status, and a pure domain
smart constructor that rejects invalid names before saving.

# Context and text migration

Change `fn body(self, _ctx: &Context)` to `fn body(self)`. Read ambient values
with `environment::<Locale>()`, `environment::<Viewport>()`, or another
`FromEnvironment` implementation, inside the body or a synchronous helper.
Capture the resulting value for callbacks. Low-level render, effect, sheet,
paint and event callbacks keep their explicit context parameters.

`text(state)` and `text(binding)` read at the node. `text!` still formats
reactively; `text_with` accepts a custom reader. Fixed strings remain fixed.
A generic helper accepting `Into<Arc<str>>` can explicitly convert to `Arc<str>`
before calling `text`, or accept `IntoText` to preserve reactive inputs too.

`text(state.get())` is an intentional-or-accidental snapshot: Rust evaluates
`get()` before calling `text`. Debug builds print a bounded, once-per-callsite
hint when fixed text changes on a later body execution and its source line also
reads state with `get()`. `diagnostics::take_eager_text()` exposes the same hints.
This is not data-flow analysis: multiline reads may be missed, and intentional
snapshots may be reported. It never changes behavior. Release builds collect
nothing and print no hints.
