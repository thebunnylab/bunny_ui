use bunny_ui_core::action::Modifiers;
use bunny_ui_core::layout::{Proposal, Size};
use bunny_ui_core::prelude::*;
use std::{cell::RefCell, rc::Rc};

#[test]
fn clicks_keep_the_initiating_modifiers_and_cancel_does_not_leak_them() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&seen);
    let view = text("Row")
        .frame(120.0, 24.0)
        .on_click_with_modifiers(move |clicks, modifiers| {
            sink.borrow_mut().push((clicks, modifiers))
        })
        .id("row");
    let runtime = Runtime::new();
    runtime.settled_layout(
        &view,
        Proposal::exact(Size {
            width: 120.0,
            height: 24.0,
        }),
    );
    runtime.pointer_clicked(30.0, 12.0, 2, Modifiers::SHIFT);
    runtime.pointer_moved(32.0, 12.0, Modifiers::NONE);
    runtime.pointer_released(32.0, 12.0);
    assert_eq!(&*seen.borrow(), &[(2, Modifiers::SHIFT)]);
    runtime.pointer_clicked(
        30.0,
        12.0,
        1,
        Modifiers {
            command: true,
            ..Modifiers::NONE
        },
    );
    runtime.pointer_cancelled();
    runtime.pointer_released(30.0, 12.0);
    runtime.pointer_pressed(30.0, 12.0);
    runtime.pointer_released(30.0, 12.0);
    assert_eq!(
        &*seen.borrow(),
        &[(2, Modifiers::SHIFT), (1, Modifiers::NONE)]
    );
}

#[test]
fn a_modified_click_target_still_lifts_its_normal_drag_source() {
    let clicked = Rc::new(RefCell::new(Vec::new()));
    let dropped = Rc::new(RefCell::new(Vec::new()));
    let click = Rc::clone(&clicked);
    let drop = Rc::clone(&dropped);
    let view = vstack!(
        text("Source")
            .frame(160.0, 30.0)
            .on_click_with_modifiers(move |count, modifiers| click
                .borrow_mut()
                .push((count, modifiers)))
            .on_drag(|| drag(17_u32, "Source")),
        text("Target")
            .frame(160.0, 30.0)
            .on_drop::<u32>(move |value| drop.borrow_mut().push(*value)),
    )
    .spacing(10.0);
    let runtime = Runtime::new();
    runtime.settled_layout(
        &view,
        Proposal::exact(Size {
            width: 160.0,
            height: 70.0,
        }),
    );
    runtime.pointer_clicked(30.0, 15.0, 1, Modifiers::SHIFT);
    runtime.pointer_moved(30.0, 55.0, Modifiers::SHIFT);
    runtime.pointer_released(30.0, 55.0);
    assert_eq!(&*dropped.borrow(), &[17]);
    assert!(clicked.borrow().is_empty());
}
