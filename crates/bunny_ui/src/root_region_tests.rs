use super::*;
use crate::{action::ActionId, prelude::*, view::NodeList};

const ACTION: ActionId = ActionId("root-region-action");
const SIZE: Size = Size {
    width: 320.0,
    height: 120.0,
};

#[derive(Clone)]
struct RootView {
    context: &'static str,
    calls: Rc<Cell<usize>>,
    effects: Rc<Cell<usize>>,
    enabled: bool,
}
impl View for RootView {
    type Arity = crate::view::Single;
    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let root = motor::identity::current_pass_root().unwrap();
        if self.enabled {
            attribute_context(root.clone(), self.context, false);
            let calls = self.calls.clone();
            attribute_handler(root, ACTION, Rc::new(move || calls.set(calls.get() + 1)));
            let effects = self.effects.clone();
            attribute_effect(Rc::new(move |_| {
                effects.set(effects.get() + 1);
                false
            }));
        }
        text("Root").render_into(ctx, out);
    }
}

#[test]
fn root_handlers_and_contexts_survive_input_activation_without_rearming_effects() {
    let first = Runtime::scene("root-handlers-a");
    let second = Runtime::scene("root-handlers-b");
    let mut a = RootView {
        context: "first-context",
        calls: Rc::new(Cell::new(0)),
        effects: Rc::new(Cell::new(0)),
        enabled: true,
    };
    let b = RootView {
        context: "second-context",
        calls: Rc::new(Cell::new(0)),
        effects: Rc::new(Cell::new(0)),
        enabled: true,
    };
    first.display_frame(&a, SIZE);
    first.pump();
    second.display_frame(&b, SIZE);
    second.pump();
    let rendered_effects = (a.effects.get(), b.effects.get());
    assert!(rendered_effects.0 > 0 && rendered_effects.1 > 0);
    for _ in 0..3 {
        assert_eq!(first.active_contexts(), ["first-context"]);
        assert!(first.dispatch_action(ACTION));
        first.pump();
        assert_eq!(second.active_contexts(), ["second-context"]);
        assert!(second.dispatch_action(ACTION));
        second.pump();
        assert!(live_tables_match_retention());
    }
    assert_eq!((a.calls.get(), b.calls.get()), (3, 3));
    assert_eq!((a.effects.get(), b.effects.get()), rendered_effects);
    // Activate A before rendering an entirely empty registration region. Even
    // with no new root inputs/effects, its previous derived tables must leave.
    assert_eq!(first.active_contexts(), ["first-context"]);
    a.enabled = false;
    first.display_frame(&a, SIZE);
    first.pump();
    assert!(first.active_contexts().is_empty());
    assert!(!first.dispatch_action(ACTION));
    assert_eq!(second.active_contexts(), ["second-context"]);
    assert!(second.dispatch_action(ACTION));
    second.pump();
    assert_eq!((a.effects.get(), b.effects.get()), rendered_effects);
    // Invalidation of the inactive scene cannot remove its neighbor's roots.
    first.set_theme(Some(Theme::dark()));
    first.display_frame(&a, SIZE);
    assert_eq!(second.active_contexts(), ["second-context"]);
    assert!(second.dispatch_action(ACTION));
    assert!(live_tables_match_retention());
}

#[test]
fn closing_the_current_root_releases_derived_handlers_and_contexts() {
    let runtime = Runtime::scene("root-current-closed");
    let calls = Rc::new(Cell::new(0));
    let weak = Rc::downgrade(&calls);
    let view = RootView {
        context: "closed-context",
        calls,
        effects: Rc::new(Cell::new(0)),
        enabled: true,
    };
    runtime.display_frame(&view, SIZE);
    assert!(runtime.dispatch_action(ACTION));
    drop(view);
    drop(runtime);
    assert!(weak.upgrade().is_none());
    assert!(!has_handler(ACTION));
    assert!(!context_active("closed-context", None));
    assert!(assembled_root().is_none());
}
