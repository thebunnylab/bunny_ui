//! View modifiers — `.font(.title)`, `.padding()`, `.onReceive(…)`, …
//! Each one wraps the view in a `ModifiedView` that records the modifier
//! (for the printed tree) and optionally carries a behavior.

use crate::combine::IntoPublisher;
use crate::state::{Binding, Context, EffectFn, EnvironmentValues, ProvidesQueries};
use crate::view::{short_type_name, AnyView, RenderNode, View};
use crate::views::{
    Alignment, ContentMode, Edge, Font, ListStyle, ProgressViewStyle, Query, TextAlignment,
};
use std::rc::Rc;

/// What a modifier does beyond describing itself.
pub enum ModifierBehavior {
    /// Fires when the node is rendered (`onAppear`).
    OnAppear(Rc<dyn Fn()>),
    /// Polled by `Runtime::pump` (`onReceive`, `onChange`, `query`).
    Effect(EffectFn),
    /// `.sheet2(isPresented:content:)`
    Sheet { isPresented: Binding<bool>, content: Rc<dyn Fn(&Context) -> AnyView> },
    /// Mutates the environment seen by the subtree (`.inject`, `.modelContainer`).
    EnvSet(Rc<dyn Fn(&mut EnvironmentValues)>),
    /// `.modifier(LocaleReader(…))` and friends.
    Custom(Rc<dyn CustomModifier>),
}

/// `ViewModifier` / `EnvironmentalModifier` — wraps the content the way
/// `func body(content: Content) -> some View` does in Swift.
pub trait CustomModifier {
    fn name(&self) -> String;
    fn apply(&self, ctx: &Context, content: AnyView) -> AnyView;
}

pub struct Modifier {
    pub name: String,
    pub detail: String,
    pub behavior: Option<ModifierBehavior>,
}

#[derive(Clone)]
pub struct ModifiedView {
    pub base: AnyView,
    pub modifier: Rc<Modifier>,
}

impl View for ModifiedView {
    fn render(&self, ctx: &Context) -> RenderNode {
        // `.modifier(…)` re-renders through the custom modifier's own
        // `body(content:)` chain, then marks the node.
        if let Some(ModifierBehavior::Custom(custom)) = &self.modifier.behavior {
            let mut node = custom.apply(ctx, self.base.clone()).render(ctx);
            node.line.push_str(&format!(" [.modifier({})]", custom.name()));
            return node;
        }

        let base_ctx = match &self.modifier.behavior {
            Some(ModifierBehavior::EnvSet(set)) => {
                let mut values = ctx.values.clone();
                set(&mut values);
                Context { values, effects: ctx.effects.clone() }
            }
            _ => ctx.clone(),
        };
        let mut node = self.base.render(&base_ctx);

        match &self.modifier.behavior {
            Some(ModifierBehavior::OnAppear(action)) => action(),
            Some(ModifierBehavior::Effect(effect)) => {
                // The effect sees the subtree's environment — `.inject()` /
                // `.modelContainer()` flow down to descendants, and the
                // Runtime pump only has the root ctx at hand.
                let effect = effect.clone();
                let subtree_ctx = base_ctx.clone();
                ctx.effects.borrow_mut().push(Rc::new(move |_: &Context| effect(&subtree_ctx)));
            }
            Some(ModifierBehavior::Sheet { isPresented, content }) => {
                if isPresented.wrappedValue() {
                    node.children.push(RenderNode::branch(
                        "Sheet",
                        vec![content(ctx).render(ctx)],
                    ));
                }
            }
            _ => {}
        }

        node.line.push_str(&format!(" [.{}{}]", self.modifier.name, self.modifier.detail));
        node
    }
}

/// Every modifier in the UI layer, as a chained method (`Text("x").font(.title)`).
pub trait ViewExt: View + Sized {
    fn into_any(self) -> AnyView {
        AnyView::new(self)
    }

    fn with_modifier(
        self,
        name: &str,
        detail: impl Into<String>,
        behavior: Option<ModifierBehavior>,
    ) -> AnyView {
        AnyView::new(ModifiedView {
            base: self.into_any(),
            modifier: Rc::new(Modifier { name: name.into(), detail: detail.into(), behavior }),
        })
    }

    fn with_behavior(self, name: &str, detail: impl Into<String>, behavior: ModifierBehavior) -> AnyView {
        self.with_modifier(name, detail, Some(behavior))
    }

    fn inert(self, name: &str, detail: &str) -> AnyView {
        self.with_modifier(name, detail.to_string(), None)
    }

    fn with_effect(self, name: &str, detail: impl Into<String>, effect: EffectFn) -> AnyView {
        self.with_behavior(name, detail, ModifierBehavior::Effect(effect))
    }

    // MARK: - Formatting

    fn font(self, font: Font) -> AnyView {
        self.inert("font", &format!("({font})"))
    }

    fn bold(self) -> AnyView {
        self.inert("bold", "()")
    }

    fn padding(self) -> AnyView {
        self.inert("padding", "()")
    }

    /// `.padding(5)`
    fn paddingLength(self, length: f64) -> AnyView {
        self.inert("padding", &format!("({length})"))
    }

    /// `.padding(.bottom, 40)`
    fn paddingEdge(self, edge: Edge, length: f64) -> AnyView {
        self.inert("padding", &format!("({edge}, {length})"))
    }

    /// `.frame(width: 120, height: 80)`
    fn frameWH(self, width: f64, height: f64) -> AnyView {
        self.inert("frame", &format!("(width: {width}, height: {height})"))
    }

    /// `.frame(maxWidth: .infinity, maxHeight: 60, alignment: .leading)`
    fn frameMax(self, maxWidth: f64, maxHeight: f64, alignment: Alignment) -> AnyView {
        self.inert("frame", &format!("(maxWidth: {maxWidth:?}, maxHeight: {maxHeight}, alignment: {alignment})"))
    }

    fn navigationTitle(self, title: impl Into<String>) -> AnyView {
        self.inert("navigationTitle", &format!("({:?})", title.into()))
    }

    fn navigationBarTitle(self, title: impl Into<String>) -> AnyView {
        self.inert("navigationBarTitle", &format!("({:?})", title.into()))
    }

    fn listStyle(self, style: ListStyle) -> AnyView {
        self.inert("listStyle", &format!("({style})"))
    }

    fn progressViewStyle(self, style: ProgressViewStyle) -> AnyView {
        self.inert("progressViewStyle", &format!("({style})"))
    }

    /// `.navigationViewStyle(StackNavigationViewStyle())` — inert here.
    fn navigationViewStyle(self) -> AnyView {
        self.inert("navigationViewStyle", "(stack)")
    }

    fn multilineTextAlignment(self, alignment: TextAlignment) -> AnyView {
        self.inert("multilineTextAlignment", &format!("({alignment})"))
    }

    fn aspectRatio(self, contentMode: ContentMode) -> AnyView {
        self.inert("aspectRatio", &format!("(contentMode: .{:?})", contentMode))
    }

    fn resizable(self) -> AnyView {
        self.inert("resizable", "()")
    }

    fn blur(self, radius: f64) -> AnyView {
        self.inert("blur", &format!("(radius: {radius})"))
    }

    fn ignoresSafeArea(self) -> AnyView {
        self.inert("ignoresSafeArea", "()")
    }

    /// `.background { … }`
    fn backgroundView(self, content: AnyView) -> AnyView {
        let line = content.render(&Context::default()).line;
        self.inert("background", &format!(" {{ {} }}", line))
    }

    // MARK: - Interaction

    /// `Button`-style tap; also `.onTapGesture { … }`
    fn onTapGesture(self, action: Rc<dyn Fn()>) -> AnyView {
        self.with_behavior("onTapGesture", "()", ModifierBehavior::OnAppear(action))
    }

    /// `.onAppear { … }` / `.onAppear()`
    fn onAppear(self, action: Option<Rc<dyn Fn()>>) -> AnyView {
        match action {
            Some(action) => self.with_behavior("onAppear", "()", ModifierBehavior::OnAppear(action)),
            None => self.inert("onAppear", "()"),
        }
    }

    /// `.onChange(of:initial:) { old, new in … }`
    ///
    /// `site` is a unique key per call site (`concat!(file!(), ":onChange:3")`)
    /// so the change-detection slot survives re-renders.
    fn onChange<V: Clone + PartialEq + 'static>(
        self,
        site: &'static str,
        of: Rc<dyn Fn() -> V>,
        initial: bool,
        action: Rc<dyn Fn(&V, &V)>,
    ) -> AnyView {
        let effect: EffectFn = Rc::new(move |_ctx| {
            let value = (of)();
            let cell = crate::runtime::effect_slot::<V>(site);
            let mut previous = cell.borrow_mut();
            match previous.take() {
                None => {
                    *previous = Some(value.clone());
                    if initial {
                        let old = value.clone();
                        action(&old, &value);
                        true
                    } else {
                        false
                    }
                }
                Some(old) if old != value => {
                    *previous = Some(value.clone());
                    action(&old, &value);
                    true
                }
                Some(old) => {
                    *previous = Some(old);
                    false
                }
            }
        });
        self.with_effect("onChange", "()", effect)
    }

    /// `.onReceive(publisher) { value in … }`
    fn onReceive<V: Clone + PartialEq + 'static>(
        self,
        publisher: impl IntoPublisher<V>,
        action: Rc<dyn Fn(V)>,
    ) -> AnyView {
        let publisher = publisher.into_publisher();
        let effect: EffectFn = Rc::new(move |_ctx| match publisher.poll() {
            Some(value) => {
                action(value);
                true
            }
            None => false,
        });
        self.with_effect("onReceive", "()", effect)
    }

    /// `.searchable(text: $searchText)`
    fn searchable(self, _text: Binding<String>) -> AnyView {
        self.inert("searchable", "(text: $…)")
    }

    /// `.refreshable { … }` — pull-to-refresh is a user gesture; nothing
    /// fires it headlessly (the initial load goes through `.onAppear`).
    fn refreshable(self, _action: Rc<dyn Fn()>) -> AnyView {
        self.inert("refreshable", " { … }")
    }

    /// `.sheet2(isPresented:content:)`
    fn sheet2(
        self,
        isPresented: Binding<bool>,
        content: Rc<dyn Fn(&Context) -> AnyView>,
    ) -> AnyView {
        self.with_behavior(
            "sheet2",
            "(isPresented: $…)",
            ModifierBehavior::Sheet { isPresented, content },
        )
    }

    /// `.toolbar { … }`
    fn toolbar(self, _items: Vec<AnyView>) -> AnyView {
        self.inert("toolbar", " { … }")
    }

    // MARK: - Data & environment

    /// `.inject(diContainer)`
    fn inject<T: 'static>(self, container: Rc<T>) -> AnyView {
        let detail = format!("({})", short_type_name::<T>());
        self.with_behavior(
            "inject",
            detail,
            ModifierBehavior::EnvSet(Rc::new(move |values: &mut EnvironmentValues| {
                values.injected = Some(container.clone());
            })),
        )
    }

    /// `.modelContainer(container)`
    fn modelContainer<T: ProvidesQueries + 'static>(self, container: Rc<T>) -> AnyView {
        let source = container.querySource();
        self.with_behavior(
            "modelContainer",
            "(…)",
            ModifierBehavior::EnvSet(Rc::new(move |values: &mut EnvironmentValues| {
                values.querySource = Some(source.clone());
            })),
        )
    }

    /// `.modifier(LocaleReader(container: …))`
    fn modifier(self, custom: impl CustomModifier + 'static) -> AnyView {
        let custom: Rc<dyn CustomModifier> = Rc::new(custom);
        let detail = format!("({})", custom.name());
        self.with_behavior("modifier", detail, ModifierBehavior::Custom(custom))
    }

    /// `.query(searchText:results:) { search in Query(…) }` — the @Query fake.
    fn query<T: Clone + PartialEq + 'static>(
        self,
        searchText: String,
        results: Binding<Vec<T>>,
        builder: Rc<dyn Fn(String) -> Query<T>>,
    ) -> AnyView {
        let effect: EffectFn = Rc::new(move |ctx: &Context| {
            let Some(source) = ctx.values.querySource.clone() else { return false };
            let Some(any) = source(std::any::type_name::<T>()) else { return false };
            let Ok(storage) = any.downcast::<std::cell::RefCell<Vec<T>>>() else { return false };

            let query = builder(searchText.clone());
            let items: Vec<T> = storage
                .borrow()
                .iter()
                .filter(|item| (query.filter)(item))
                .cloned()
                .collect();
            let mut keyed: Vec<(String, T)> =
                items.into_iter().map(|item| ((query.sortKey)(&item), item)).collect();
            keyed.sort_by(|a, b| a.0.cmp(&b.0));
            let items: Vec<T> = keyed.into_iter().map(|(_, item)| item).collect();

            if results.wrappedValue() != items {
                results.set(items);
                true
            } else {
                false
            }
        });
        self.with_effect("query", "(searchText: …, results: $…)", effect)
    }

    // MARK: - Inert no-ops kept for port parity

    fn equatable(self) -> AnyView {
        self.inert("equatable", "()")
    }

    fn hidden(self) -> AnyView {
        self.inert("hidden", "()")
    }

    /// `.attachEnvironmentOverrides()`
    fn attachEnvironmentOverrides(self) -> AnyView {
        self.inert("attachEnvironmentOverrides", "()")
    }

    /// `.attachEnvironmentOverrides(onChange: …)`
    fn attachEnvironmentOverridesOnChange(self) -> AnyView {
        self.inert("attachEnvironmentOverrides", "(onChange: …)")
    }

    /// `.flipsForRightToLeftLayoutDirection(true)`
    fn flipsForRightToLeftLayoutDirection(self, flips: bool) -> AnyView {
        self.inert("flipsForRightToLeftLayoutDirection", &format!("({flips})"))
    }

    /// `.navigationDestination(for: …) { … }` — the fake NavigationPath carries
    /// no typed values, so destinations are description-only.
    fn navigationDestination(self) -> AnyView {
        self.inert("navigationDestination", "(for: …)")
    }
}

impl<V: View> ViewExt for V {}
