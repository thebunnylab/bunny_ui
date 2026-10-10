//! Active field input holds the caret; a quiet beat starts idle blinking.
extern crate bunny_ui_core as bunny_ui; // the core, by the name an application uses

use bunny_ui::layout::{DrawCommand, Proposal, Size};
use bunny_ui::prelude::*;
use bunny_ui::text_input::EditCommand;

#[test]
fn typing_stays_solid_and_idle_blinking_resumes_without_a_new_clock() {
    let content = State::new(String::new());
    let view = text_field("", content.binding()).id("field");
    let runtime = Runtime::new();
    let proposal = Proposal::exact(Size {
        width: 240.0,
        height: 40.0,
    });
    runtime.render_stable(&view);
    let field = runtime.layout(&view, proposal).fields[0].path.clone();
    runtime.focus(&field);
    let visible = || {
        runtime.render_stable(&view);
        runtime
            .layout(&view, proposal)
            .display
            .iter()
            .any(|command| {
                matches!(command, DrawCommand::FillRect { rect, color, .. }
            if *color == Color::BLACK && rect.size.width < 2.0 && rect.size.height > 8.0)
            })
    };
    assert!(visible());
    for _ in 0..8 {
        assert!(runtime.key(EditCommand::Insert("x".into())).applied);
        assert!(
            !runtime.blink(),
            "a beat immediately after field input must not hide its caret"
        );
        assert!(visible());
    }
    assert!(runtime.blink(), "the next quiet beat starts the off phase");
    assert!(!visible());
    assert!(runtime.key(EditCommand::Read).applied);
    assert!(runtime.key(EditCommand::Copy).applied);
    assert!(!visible(), "observational reads must not restart the caret");
    assert!(runtime.blink());
    assert!(visible());
    assert!(runtime.blink());
    assert!(!visible());
    assert!(runtime.key(EditCommand::Insert("!".into())).applied);
    assert!(visible(), "input immediately restores a hidden caret");
    assert!(!runtime.blink());
    assert!(visible());
    assert!(runtime.blink());
    assert!(!visible());
    assert_eq!(
        &*runtime.ime_snapshot().expect("focused field").text,
        "xxxxxxxx!"
    );
    runtime.blur();
    assert!(!runtime.blink());
    runtime.collect_garbage();
    assert!(!runtime.slow_tick_needed(), "blur leaves no new clock work");
    assert!(runtime.render_stable(&view).contains("xxxxxxxx!"));
}
