use bunny_ui::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone)]
struct DirectText {
    count: State<i32>,
    runs: Rc<Cell<usize>>,
}

impl Component for DirectText {
    fn body(self) -> impl View {
        self.runs.set(self.runs.get() + 1);
        vstack((text(self.count), text(self.count.binding())))
    }
}

#[test]
fn direct_text_updates_without_rebuilding_its_component() {
    let runtime = Runtime::new();
    let count = State::new(7);
    let runs = Rc::new(Cell::new(0));
    let view = DirectText {
        count,
        runs: runs.clone(),
    };
    assert!(runtime.render(&view).contains("Text(\"7\")"));
    count.set(11);
    assert!(runtime.render(&view).contains("Text(\"11\")"));
    assert_eq!(runs.get(), 1);
}

#[derive(Clone, Copy)]
struct LocaleLabel;

impl Component for LocaleLabel {
    fn body(self) -> impl View {
        text(environment::<Locale>().identifier())
    }
}

#[test]
fn a_component_reads_its_environment_without_a_context_parameter() {
    let runtime = Runtime::new();
    let view = LocaleLabel.environment(|values| values.locale = Locale::new("pt-BR"));
    assert!(runtime.render(&view).contains("Text(\"pt-BR\")"));
}

#[test]
fn retained_environment_reads_stay_with_their_scene_and_nested_override() {
    #[derive(Clone)]
    struct Reader {
        revision: State<u32>,
        observed: Rc<std::cell::RefCell<String>>,
    }
    impl Component for Reader {
        fn body(self) -> impl View {
            let revision = self.revision.get();
            let locale = environment::<Locale>();
            *self.observed.borrow_mut() = format!("{}:{revision}", locale.identifier());
            text(locale.identifier())
        }
    }
    let first = Runtime::scene("first_environment");
    let second = Runtime::scene("second_environment");
    first.set_environment(|values| values.locale = Locale::new("pt-BR"));
    second.set_environment(|values| values.locale = Locale::new("en-US"));
    let first_reader = Reader {
        revision: State::new(0),
        observed: Default::default(),
    };
    let second_reader = Reader {
        revision: State::new(0),
        observed: Default::default(),
    };
    let first_tree = first_reader
        .clone()
        .environment(|values| values.locale = Locale::new("fr-FR"));
    first.render(&first_tree);
    second.render(&second_reader);
    assert_eq!(&*first_reader.observed.borrow(), "fr-FR:0");
    assert_eq!(&*second_reader.observed.borrow(), "en-US:0");
    first_reader.revision.set(1);
    second_reader.revision.set(2);
    first.render(&first_tree);
    second.render(&second_reader);
    assert_eq!(&*first_reader.observed.borrow(), "fr-FR:1");
    assert_eq!(&*second_reader.observed.borrow(), "en-US:2");
    assert!(std::panic::catch_unwind(environment::<Locale>).is_err());
}

#[test]
fn environment_outside_a_body_fails_explicitly() {
    assert!(std::panic::catch_unwind(environment::<Locale>).is_err());
}

#[test]
fn fixed_text_accepts_borrowed_owned_and_shared_strings() {
    let runtime = Runtime::new();
    let owned = String::from("owned");
    let shared: std::sync::Arc<str> = "shared".into();
    let tree = vstack((
        text("literal"),
        text(&owned),
        text(owned.clone()),
        text(shared),
    ));
    let rendered = runtime.render(&tree);
    for word in ["literal", "owned", "shared"] {
        assert!(rendered.contains(word));
    }
}

#[derive(Clone, Copy)]
struct CounterModel {
    count: State<i32>,
    name: State<String>,
}

impl CounterModel {
    fn new() -> Self {
        Self {
            count: State::new(0),
            name: State::new("Ada".to_owned()),
        }
    }

    fn increment(self) {
        self.count.add(1);
    }

    fn greeting(self) -> Derived<String> {
        derived(move || format!("Hello, {}", self.name.get()))
    }
}

#[derive(Clone)]
struct ModelOwner {
    rebuild: State<i32>,
    visible: State<bool>,
    mounted: Rc<Cell<Option<CounterModel>>>,
    initializations: Rc<Cell<usize>>,
}

#[derive(Clone)]
struct ModelBody(ModelOwner);

impl Component for ModelBody {
    fn body(self) -> impl View {
        let owner = self.0;
        let model = view_model(|| {
            owner.initializations.set(owner.initializations.get() + 1);
            CounterModel::new()
        });
        owner.mounted.set(Some(model));
        // This eager read deliberately exercises a parent rebuild.
        let rebuild = owner.rebuild.get();
        vstack((
            text(format!("Revision {rebuild}")),
            text(model.count),
            text(model.greeting()),
        ))
    }
}

impl Component for ModelOwner {
    fn body(self) -> impl View {
        self.visible.get().then_some(ModelBody(self))
    }
}

#[test]
fn a_view_model_keeps_its_properties_on_rerender_and_releases_them_on_unmount() {
    let runtime = Runtime::new();
    let owner = ModelOwner {
        rebuild: State::new(0),
        visible: State::new(true),
        mounted: Rc::new(Cell::new(None)),
        initializations: Rc::new(Cell::new(0)),
    };
    assert!(runtime.render(&owner).contains("Hello, Ada"));
    let model = owner.mounted.get().unwrap();
    model.increment();
    model.name.binding().set("Grace".to_owned());
    assert_eq!(model.count.get(), 1);
    assert!(runtime.render(&owner).contains("Hello, Grace"));
    owner.rebuild.set(1);
    let text = runtime.render(&owner);
    assert!(text.contains("Hello, Grace") && text.contains("Text(\"1\")"));
    assert_eq!(owner.initializations.get(), 1);
    owner.visible.set(false);
    runtime.render(&owner);
    assert!(std::panic::catch_unwind(|| model.count.get()).is_err());
    owner.visible.set(true);
    let text = runtime.render(&owner);
    assert!(text.contains("Hello, Ada") && text.contains("Text(\"0\")"));
    assert_eq!(owner.initializations.get(), 2);
}

#[test]
fn view_model_properties_have_independent_subscriptions() {
    #[derive(Clone)]
    struct Properties {
        model: CounterModel,
        count_reads: Rc<Cell<usize>>,
        name_reads: Rc<Cell<usize>>,
    }
    impl Component for Properties {
        fn body(self) -> impl View {
            let count = derived(move || {
                self.count_reads.set(self.count_reads.get() + 1);
                self.model.count.get()
            });
            let name = derived(move || {
                self.name_reads.set(self.name_reads.get() + 1);
                self.model.name.get()
            });
            vstack((text(count), text(name)))
        }
    }
    let model = CounterModel::new();
    let count_reads = Rc::new(Cell::new(0));
    let name_reads = Rc::new(Cell::new(0));
    let view = Properties {
        model,
        count_reads: count_reads.clone(),
        name_reads: name_reads.clone(),
    };
    let runtime = Runtime::new();
    runtime.render(&view);
    let before = (count_reads.get(), name_reads.get());
    model.increment();
    assert!(runtime.render(&view).contains("Text(\"1\")"));
    assert_eq!(name_reads.get(), before.1);
    assert!(count_reads.get() > before.0);
}

#[cfg(debug_assertions)]
#[test]
fn an_eager_text_rebuild_has_one_actionable_debug_hint() {
    #[derive(Clone, Copy)]
    struct Eager(State<String>);
    impl Component for Eager {
        fn body(self) -> impl View {
            text(self.0.get())
        }
    }
    bunny_ui::diagnostics::take_eager_text();
    let view = Eager(State::new("one".to_owned()));
    let runtime = Runtime::new();
    runtime.render(&view);
    assert!(bunny_ui::diagnostics::take_eager_text().is_empty());
    view.0.set("two".to_owned());
    runtime.render(&view);
    let hints = bunny_ui::diagnostics::take_eager_text();
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].read.line(), hints[0].text.line());
    assert!(hints[0].text.file().ends_with("authoring.rs"));
    view.0.set("three".to_owned());
    runtime.render(&view);
    assert!(bunny_ui::diagnostics::take_eager_text().is_empty());
}

#[test]
fn keyed_models_keep_their_values_when_reordered_and_drop_when_removed() {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    type Models = Rc<RefCell<BTreeMap<String, CounterModel>>>;
    #[derive(Clone)]
    struct Row {
        key: String,
        models: Models,
        initializations: Rc<Cell<usize>>,
    }
    impl Component for Row {
        fn body(self) -> impl View {
            let model = view_model(|| {
                self.initializations.set(self.initializations.get() + 1);
                CounterModel::new()
            });
            self.models.borrow_mut().insert(self.key.clone(), model);
            hstack((text(self.key), text(model.count)))
        }
    }
    #[derive(Clone)]
    struct Rows {
        keys: State<Vec<String>>,
        models: Models,
        initializations: Rc<Cell<usize>>,
    }
    impl Component for Rows {
        fn body(self) -> impl View {
            for_each(self.keys, String::clone, move |key| Row {
                key: key.clone(),
                models: self.models.clone(),
                initializations: self.initializations.clone(),
            })
        }
    }
    let models = Rc::new(RefCell::new(BTreeMap::new()));
    let initializations = Rc::new(Cell::new(0));
    let view = Rows {
        keys: State::new(vec!["a".into(), "b".into()]),
        models: models.clone(),
        initializations: initializations.clone(),
    };
    let runtime = Runtime::new();
    runtime.render(&view);
    let a = models.borrow()["a"];
    let b = models.borrow()["b"];
    a.count.set(12);
    b.count.set(34);
    view.keys.set(vec!["b".into(), "a".into()]);
    runtime.render(&view);
    assert_eq!(initializations.get(), 2);
    assert_eq!(models.borrow()["a"].count.version(), a.count.version());
    assert_eq!(models.borrow()["b"].count.version(), b.count.version());
    assert_eq!((a.count.get(), b.count.get()), (12, 34));
    view.keys.set(vec!["b".into()]);
    runtime.render(&view);
    assert!(std::panic::catch_unwind(|| a.count.get()).is_err());
    assert_eq!(b.count.get(), 34);
    view.keys.set(vec!["b".into(), "a".into()]);
    runtime.render(&view);
    assert_eq!(models.borrow()["a"].count.get(), 0);
    assert_eq!(initializations.get(), 3);
}

#[test]
fn model_declarations_in_one_body_have_separate_storage() {
    #[derive(Clone)]
    struct Pair {
        rebuild: State<bool>,
        models: Rc<Cell<Option<(CounterModel, CounterModel)>>>,
    }
    impl Component for Pair {
        fn body(self) -> impl View {
            let first = view_model(CounterModel::new);
            let second = view_model(CounterModel::new);
            self.models.set(Some((first, second)));
            text(format!("{}", self.rebuild.get()))
        }
    }
    let view = Pair {
        rebuild: State::new(false),
        models: Rc::new(Cell::new(None)),
    };
    let runtime = Runtime::new();
    runtime.render(&view);
    let (a, b) = view.models.get().unwrap();
    a.count.set(10);
    b.count.set(20);
    view.rebuild.set(true);
    runtime.render(&view);
    let (again_a, again_b) = view.models.get().unwrap();
    assert_eq!((again_a.count.get(), again_b.count.get()), (10, 20));
}

#[test]
fn changing_the_owner_identity_recreates_its_model() {
    let runtime = Runtime::scene("model_identity");
    let owner = ModelOwner {
        rebuild: State::new(0),
        visible: State::new(true),
        mounted: Rc::new(Cell::new(None)),
        initializations: Rc::new(Cell::new(0)),
    };
    runtime.render(&ModelBody(owner.clone()).id("first"));
    let first = owner.mounted.get().unwrap();
    first.count.set(42);
    runtime.render(&ModelBody(owner.clone()).id("second"));
    assert_eq!(owner.mounted.get().unwrap().count.get(), 0);
    assert_eq!(owner.initializations.get(), 2);
    assert!(std::panic::catch_unwind(|| first.count.get()).is_err());
}

#[test]
fn reactive_text_does_not_report_an_eager_snapshot() {
    let runtime = Runtime::new();
    let view = DirectText {
        count: State::new(7),
        runs: Rc::new(Cell::new(0)),
    };
    bunny_ui::diagnostics::take_eager_text();
    runtime.render(&view);
    view.count.set(8);
    runtime.render(&view);
    assert!(bunny_ui::diagnostics::take_eager_text().is_empty());
}

#[cfg(not(debug_assertions))]
#[test]
fn release_collects_no_eager_text_diagnostics() {
    #[derive(Clone, Copy)]
    struct Eager(State<String>);
    impl Component for Eager {
        fn body(self) -> impl View {
            text(self.0.get())
        }
    }
    let runtime = Runtime::new();
    let view = Eager(State::new("before".into()));
    runtime.render(&view);
    view.0.set("after".into());
    assert!(runtime.render(&view).contains("after"));
    assert!(bunny_ui::diagnostics::take_eager_text().is_empty());
}

#[test]
fn view_models_require_a_runtime_scope() {
    assert!(std::panic::catch_unwind(|| view_model(CounterModel::new)).is_err());
}
