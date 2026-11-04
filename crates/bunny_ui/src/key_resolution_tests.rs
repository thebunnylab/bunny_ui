//! The key resolver port (D435): the live stack, innermost first; a
//! Strategy that reads frames and can disable a key; sequences matched by
//! the character a stroke typed; the wait ending in a replay instead of a
//! loss; and a lone modifier made a stroke.
use super::Runtime;
use crate::action::{
    ActionId, ContextFrame, ContextStack, Fallback, Key, KeyMatch, KeyPattern, KeyResolution,
    Modifiers, Stroke,
};
use crate::custom::{ElementEvent, EventCtx, PaintCtx, Painter, Response};
use crate::layout::{Proposal, Size};
use crate::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

fn viewport() -> Proposal {
    Proposal::exact(Size { width: 400.0, height: 300.0 })
}

/// A box that declines text, as a modal editor in its command mode does:
/// every bare stroke reaches the keymap, and what reaches it as text is
/// written down.
struct Modal {
    heard: Rc<RefCell<String>>,
}

impl CustomElement for Modal {
    fn accepts_keys(&self) -> bool {
        true
    }

    fn takes_text(&self) -> bool {
        false
    }

    fn paint(&self, _ctx: &PaintCtx, _painter: &mut Painter) {}

    fn event(&self, event: &ElementEvent, _ctx: &EventCtx) -> Response {
        if let ElementEvent::Text(text) = event {
            self.heard.borrow_mut().push_str(text);
            return Response::handled();
        }
        Response::ignored()
    }
}

/// Clicks the first box the layout placed, so it holds the keyboard.
fn focus_the_box(runtime: &Runtime, view: &impl Component) {
    runtime.render_stable(view);
    let placed = runtime.layout(view, viewport());
    let frame = placed.customs.first().expect("the box is placed").frame;
    runtime.pointer_pressed(frame.origin.x + 5.0, frame.origin.y + 5.0);
    runtime.pointer_released(frame.origin.x + 5.0, frame.origin.y + 5.0);
    assert!(runtime.focused().is_some(), "the click focused the box");
}

/// A bare character and the stroke that types it.
fn typed(character: char) -> Stroke {
    Stroke::new(KeyPattern::key(Key::Char(character)), Some(character))
}

/// Counts how often an action runs.
fn counter(runtime: &Runtime, action: ActionId) -> Rc<Cell<usize>> {
    let count = Rc::new(Cell::new(0));
    let heard = Rc::clone(&count);
    runtime.on_action(action, move || heard.set(heard.get() + 1));
    count
}

#[derive(Clone)]
struct Workbench {
    heard: Rc<RefCell<String>>,
    mode: State<&'static str>,
}

impl Component for Workbench {
    fn body(self) -> impl View {
        let editor = custom(Modal { heard: Rc::clone(&self.heard) })
            .id("editor")
            .key_context_focused_with(ContextFrame::new("Editor").fact("mode", self.mode.get()));
        vstack!(
            hstack!(text("tabs"), editor).key_context("pane"),
            text("terminal").key_context("terminal"),
        )
        .key_context("workspace")
    }
}

fn workbench() -> Workbench {
    Workbench { heard: Rc::default(), mode: State::new("normal") }
}

#[test]
fn the_declaration_nearest_the_keyboard_wins_a_key_several_bind() {
    const EDITOR: ActionId = ActionId("editor.escape");
    const PANE: ActionId = ActionId("pane.escape");
    const TERMINAL: ActionId = ActionId("terminal.escape");
    const WORKSPACE: ActionId = ActionId("workspace.escape");
    let bench = workbench();
    let runtime = Runtime::new();
    let escape = KeyPattern::key(Key::Escape);
    runtime.bind_in("workspace", escape, WORKSPACE);
    runtime.bind_in("terminal", escape, TERMINAL);
    runtime.bind_in("pane", escape, PANE);
    runtime.bind_in("Editor", escape, EDITOR);
    focus_the_box(&runtime, &bench);

    // the stack reads innermost first: the path, deepest first, then the
    // mounted view the keyboard is not in
    let stack = runtime.context_stack();
    let names: Vec<&str> = stack.levels().iter().map(|level| level.frame().idents()[0]).collect();
    assert_eq!(names, ["Editor", "pane", "workspace", "terminal"]);
    assert!(stack.levels()[..3].iter().all(|level| level.on_focus_path()));
    assert!(!stack.levels()[3].on_focus_path(), "the terminal is mounted, not focused");
    let chain: Vec<&str> = stack.ancestry(0).map(|level| level.frame().idents()[0]).collect();
    assert_eq!(chain, ["Editor", "pane", "workspace"], "each level knows the declaration holding it");
    assert_eq!(stack.levels()[3].parent(), Some(2), "the terminal hangs on the workspace too");

    // innermost wins, every time, whatever order the tables were filled in
    for _ in 0..8 {
        assert_eq!(runtime.match_key(&escape), Some(EDITOR));
        assert_eq!(runtime.chord(escape), KeyMatch::Action(EDITOR));
    }
    // without the editor's own binding the pane is next — still on the
    // path, so it beats the deeper-than-the-workspace terminal
    runtime.clear_bindings_in("Editor");
    assert_eq!(runtime.chord(escape), KeyMatch::Action(PANE));
    runtime.clear_bindings_in("pane");
    assert_eq!(runtime.chord(escape), KeyMatch::Action(WORKSPACE), "the path beats a view off it");
    runtime.clear_bindings_in("workspace");
    assert_eq!(runtime.chord(escape), KeyMatch::Action(TERMINAL), "and a mounted view still answers");

    // with nobody holding the keyboard the focused frame is gone, and
    // every declaration is off the path: deeper first
    runtime.bind_in("Editor", escape, EDITOR);
    runtime.bind_in("workspace", escape, WORKSPACE);
    runtime.blur();
    assert!(!runtime.context_stack().has_ident("Editor"), "a focused frame needs the keyboard");
    assert_eq!(runtime.chord(escape), KeyMatch::Action(TERMINAL), "the deeper mounted view first");
}

#[test]
fn a_resolver_reads_the_live_frames_and_a_disabled_key_is_spent() {
    const DELETE: ActionId = ActionId("vim.delete");
    const FLOOR: ActionId = ActionId("app.floor");
    let bench = workbench();
    let runtime = Runtime::new();
    let read: Rc<RefCell<Vec<String>>> = Rc::default();
    {
        let read = Rc::clone(&read);
        runtime.set_key_resolver(move |strokes: &[Stroke], contexts: &ContextStack| {
            let innermost = contexts.levels().first().map(|level| level.frame());
            read.borrow_mut().push(innermost.map(ToString::to_string).unwrap_or_default());
            let mode = innermost.and_then(|frame| frame.fact_value("mode"));
            match (strokes, mode) {
                ([stroke], Some("normal")) if stroke.pattern == KeyPattern::key(Key::F(5)) => {
                    KeyResolution::Action(DELETE)
                }
                // the keymap's `null`: bound to nothing, on purpose
                ([stroke], Some("insert")) if stroke.pattern == KeyPattern::key(Key::F(5)) => {
                    KeyResolution::Disabled
                }
                _ => KeyResolution::None,
            }
        });
    }
    let deleted = counter(&runtime, DELETE);
    let floor = counter(&runtime, FLOOR);
    runtime.bind(KeyPattern::key(Key::F(5)), FLOOR);
    runtime.bind(KeyPattern::key(Key::F(6)), FLOOR);
    focus_the_box(&runtime, &bench);

    let f5 = Stroke::from(KeyPattern::key(Key::F(5)));
    assert!(runtime.offer_key(f5), "the resolver's action ran");
    assert_eq!((deleted.get(), floor.get()), (1, 0), "the resolver answers before the tables");
    assert_eq!(read.borrow().last().map(String::as_str), Some("Editor mode=normal"));

    // the fact moves with the view: the next stroke reads the next render
    bench.mode.set("insert");
    runtime.render_stable(&bench);
    assert_eq!(runtime.chord(f5), KeyMatch::Disabled);
    assert!(runtime.offer_key(f5), "a disabled key is consumed");
    assert_eq!((deleted.get(), floor.get()), (1, 0), "and nothing ran — not even the table's binding");

    // a key the resolver leaves unbound falls to the tables, the floor
    assert!(runtime.offer_key(Stroke::from(KeyPattern::key(Key::F(6)))));
    assert_eq!(floor.get(), 1);

    // the pure lookup reads the resolver too: a disabled key means nothing,
    // and what the resolver binds is what `match_key` names
    assert_eq!(runtime.match_key(&KeyPattern::key(Key::F(5))), None, "disabled in insert");
    assert_eq!(runtime.match_key(&KeyPattern::key(Key::F(6))), Some(FLOOR), "the floor still answers");
    bench.mode.set("normal");
    runtime.render_stable(&bench);
    assert_eq!(runtime.match_key(&KeyPattern::key(Key::F(5))), Some(DELETE), "the resolver's action");

    // taken down, the tables answer alone again
    runtime.clear_key_resolver();
    assert!(runtime.offer_key(f5));
    assert_eq!(floor.get(), 2);
}

#[test]
fn a_sequence_completes_through_the_character_its_stroke_typed() {
    const CASE: ActionId = ActionId("vim.toggle_case");
    const BRACE: ActionId = ActionId("vim.unmatched_brace");
    const PHYSICAL: ActionId = ActionId("app.physical");
    let runtime = Runtime::new();
    let g = KeyPattern::key(Key::Char('g'));
    // spelled by what the key TYPES: `~`, `}` — the layout's business
    runtime.bind_sequence(&[g, KeyPattern::key(Key::Char('~'))], CASE);
    runtime.bind_sequence(&[KeyPattern::key(Key::Char(']')), KeyPattern::key(Key::Char('}'))], BRACE);

    // a US keyboard: `~` is shift and the grave key
    let tilde = Stroke::new(KeyPattern::shift(Key::Char('`')), Some('~'));
    assert_eq!(runtime.chord(typed('g')), KeyMatch::Pending);
    assert_eq!(runtime.chord(tilde), KeyMatch::Action(CASE), "the second stroke matched by its character");
    // and `}` is shift and the bracket key
    let brace = Stroke::new(KeyPattern::shift(Key::Char(']')), Some('}'));
    assert_eq!(runtime.chord(typed(']')), KeyMatch::Pending);
    assert_eq!(runtime.chord(brace), KeyMatch::Action(BRACE));

    // the key's own spelling wins a tie
    runtime.bind_sequence(&[g, KeyPattern::shift(Key::Char('`'))], PHYSICAL);
    assert_eq!(runtime.chord(typed('g')), KeyMatch::Pending);
    assert_eq!(runtime.chord(tilde), KeyMatch::Action(PHYSICAL));
}

#[test]
fn a_lone_prefix_reaches_the_text_door_when_the_wait_ends() {
    const TOP: ActionId = ActionId("vim.top");
    let bench = workbench();
    let runtime = Runtime::new();
    let g = KeyPattern::key(Key::Char('g'));
    runtime.bind_sequence(&[g, g], TOP);
    let top = counter(&runtime, TOP);
    focus_the_box(&runtime, &bench);

    // the keymap holds `g` for `g g` …
    assert!(runtime.offer_key(typed('g')), "the prefix is spent");
    assert_eq!(bench.heard.borrow().as_str(), "", "and nothing reached the box yet");
    // … and when the slow clock runs out, the box hears the `g` it held
    assert!(!runtime.chord_tick());
    assert!(runtime.chord_tick());
    assert_eq!(bench.heard.borrow().as_str(), "g", "the timed-out `g` was replayed to the focused box");
    assert!(runtime.pending_chord().is_empty());

    // a stroke that leads nowhere replays the prefix, then offers itself
    // again — unbound, it types too
    bench.heard.borrow_mut().clear();
    assert!(runtime.offer_key(typed('g')));
    assert!(runtime.offer_key(typed('x')), "the dead end is spent");
    assert_eq!(bench.heard.borrow().as_str(), "gx", "both reached the box, in order");

    // and a completed sequence never reaches it at all
    bench.heard.borrow_mut().clear();
    assert!(runtime.offer_key(typed('g')));
    assert!(runtime.offer_key(typed('g')));
    assert_eq!((top.get(), bench.heard.borrow().as_str()), (1, ""));
}

#[test]
fn the_longest_prefix_bound_alone_fires_when_the_wait_ends() {
    const DOWN: ActionId = ActionId("app.down");
    const ESCAPE: ActionId = ActionId("app.escape_by_jk");
    let bench = workbench();
    let runtime = Runtime::new();
    let j = KeyPattern::key(Key::Char('j'));
    runtime.bind(j, DOWN);
    runtime.bind_sequence(&[j, KeyPattern::key(Key::Char('k'))], ESCAPE);
    let down = counter(&runtime, DOWN);
    let escaped = counter(&runtime, ESCAPE);
    focus_the_box(&runtime, &bench);

    assert!(runtime.offer_key(typed('j')));
    assert_eq!(runtime.pending_chord(), vec![j], "`j` waits: `j k` may follow");
    assert!(runtime.offer_key(typed('k')));
    assert_eq!((down.get(), escaped.get()), (0, 1), "the sequence completed, and `j` alone never ran");

    assert!(runtime.offer_key(typed('j')));
    assert!(!runtime.chord_tick());
    assert!(runtime.chord_tick());
    assert_eq!(down.get(), 1, "the wait ended and `j` meant what it means alone");

    assert!(runtime.offer_key(typed('j')));
    assert!(runtime.offer_key(typed('x')));
    assert_eq!(down.get(), 2, "a dead end too");
    assert_eq!(bench.heard.borrow().as_str(), "x", "and the stroke after it went on its way");
}

#[test]
fn a_resolver_s_fallback_is_what_the_held_strokes_mean() {
    const SAVE: ActionId = ActionId("app.save");
    let runtime = Runtime::new();
    let ctrl_k = KeyPattern::control(Key::Char('k'));
    runtime.set_key_resolver(move |strokes: &[Stroke], _: &ContextStack| match strokes {
        [first] if first.pattern == ctrl_k => KeyResolution::Pending(Fallback::Fire(SAVE)),
        [first, _] if first.pattern == ctrl_k => KeyResolution::None,
        _ => KeyResolution::None,
    });
    let saved = counter(&runtime, SAVE);
    assert_eq!(runtime.chord(ctrl_k), KeyMatch::Pending);
    assert!(!runtime.chord_tick());
    assert!(runtime.chord_tick());
    assert_eq!(saved.get(), 1, "the resolver's fallback fired");

    // a swallowed prefix is spent, and the next stroke is offered again
    runtime.set_key_resolver(move |strokes: &[Stroke], _: &ContextStack| match strokes {
        [first] if first.pattern == ctrl_k => KeyResolution::Pending(Fallback::Swallow),
        _ => KeyResolution::None,
    });
    assert_eq!(runtime.chord(ctrl_k), KeyMatch::Pending);
    assert_eq!(runtime.chord(KeyPattern::key(Key::F(7))), KeyMatch::Pending);
    assert_eq!(saved.get(), 1);
}

#[test]
fn a_lone_modifier_let_go_alone_is_a_stroke() {
    const SEARCH: ActionId = ActionId("app.search_everywhere");
    let runtime = Runtime::new();
    let shift = KeyPattern::key(Key::Shift);
    runtime.bind_sequence(&[shift, shift], SEARCH);
    let searched = counter(&runtime, SEARCH);
    let tap = |runtime: &Runtime| {
        assert!(!runtime.modifiers_changed(Modifiers::SHIFT).tap_taken, "a press is no tap yet");
        runtime.modifiers_changed(Modifiers::NONE)
    };

    let first = tap(&runtime);
    assert!(first.tap_taken && first.present, "the first tap opens the sequence");
    assert_eq!(runtime.pending_chord(), vec![shift]);
    let second = tap(&runtime);
    assert!(second.tap_taken);
    assert_eq!(searched.get(), 1, "shift, shift");

    // shift held for a capital is that letter's modifier, never a tap
    assert!(!runtime.modifiers_changed(Modifiers::SHIFT).tap_taken);
    let _ = runtime.offer_key(Stroke::new(KeyPattern::shift(Key::Char('a')), Some('A')));
    assert!(!runtime.modifiers_changed(Modifiers::NONE).tap_taken);
    // a second modifier joining makes a chord of them, and a click is the
    // hand leaving the keyboard
    let _ = runtime.modifiers_changed(Modifiers::SHIFT);
    let _ = runtime.modifiers_changed(Modifiers { command: true, ..Modifiers::SHIFT });
    let _ = runtime.modifiers_changed(Modifiers::SHIFT);
    assert!(!runtime.modifiers_changed(Modifiers::NONE).tap_taken);
    let _ = runtime.modifiers_changed(Modifiers::SHIFT);
    runtime.pointer_pressed(1.0, 1.0);
    assert!(!runtime.modifiers_changed(Modifiers::NONE).tap_taken);
    assert!(runtime.pending_chord().is_empty());
    assert_eq!(searched.get(), 1);

    // unbound, a lone modifier asks for nothing
    runtime.clear_bindings();
    let change = tap(&runtime);
    assert!(!change.tap_taken && !change.present);
}

#[test]
fn a_lone_modifier_never_reaches_the_focused_box() {
    let bench = workbench();
    let runtime = Runtime::new();
    focus_the_box(&runtime, &bench);
    assert!(!runtime.offer_key(KeyPattern::key(Key::Shift)), "unbound, the platform's own road goes on");
    assert_eq!(bench.heard.borrow().as_str(), "", "and the box heard nothing");
}
