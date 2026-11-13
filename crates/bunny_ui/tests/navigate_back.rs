//! `Runtime::navigate_back`: what "back" closes, and in what order.

use bunny_ui::layout::AlertSpec;
use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

const SIZE: Size = Size {
    width: 480.0,
    height: 480.0,
};

#[derive(Clone, Copy)]
struct Page {
    sheet: State<bool>,
    popover: State<bool>,
    inner: State<bool>,
    alert: State<bool>,
}

impl Page {
    fn new() -> Self {
        Self {
            sheet: State::new(false),
            popover: State::new(false),
            inner: State::new(false),
            alert: State::new(false),
        }
    }

    fn open(&self) -> (bool, bool, bool, bool) {
        (self.sheet.get(), self.popover.get(), self.inner.get(), self.alert.get())
    }
}

impl Component for Page {
    fn body(self) -> impl View {
        vstack!(
            button(text("Details"), move || self.popover.set(true)).popover(
                self.popover.binding(),
                Side::Bottom,
                |_| erased(text("A popover").padding()),
            ),
            text("The page"),
        )
        .sheet(self.sheet.binding(), move |_| {
            erased(
                button(text("More"), move || self.inner.set(true))
                    .popover(self.inner.binding(), Side::Bottom, |_| erased(text("Inside the sheet").padding()))
                    .padding(),
            )
        })
        .alert(self.alert.binding(), AlertSpec::new("Delete", 280.0), |_| erased(text("Delete it?").padding()))
    }
}

/// "Back", and the frame that follows it.
fn back(runtime: &Runtime, page: &Page) -> bool {
    let went = runtime.navigate_back();
    runtime.display_frame(page, SIZE);
    went
}

#[test]
fn with_nothing_open_back_is_the_platforms() {
    let runtime = Runtime::new();
    let page = Page::new();
    runtime.display_frame(&page, SIZE);
    assert!(!back(&runtime, &page), "the shell lets the platform do its default");
    assert_eq!(page.open(), (false, false, false, false));
}

#[test]
fn back_closes_a_popover() {
    let runtime = Runtime::new();
    let page = Page::new();
    page.popover.set(true);
    runtime.display_frame(&page, SIZE);
    assert!(back(&runtime, &page));
    assert_eq!(page.open(), (false, false, false, false));
    assert!(!back(&runtime, &page), "one back, one thing");
}

#[test]
fn back_closes_a_sheet_and_an_app_switch_does_not() {
    let runtime = Runtime::new();
    let page = Page::new();
    page.sheet.set(true);
    runtime.display_frame(&page, SIZE);
    // the road an app switch takes closes popovers; a sheet stays
    runtime.dismiss_all_overlays();
    runtime.display_frame(&page, SIZE);
    assert!(page.sheet.get(), "a sheet survives the window that resigns key");
    // Escape is not back either
    runtime.offer_key(KeyPattern::key(Key::Escape));
    runtime.display_frame(&page, SIZE);
    assert!(page.sheet.get(), "Escape closes no sheet");

    assert!(back(&runtime, &page));
    assert!(!page.sheet.get());
}

#[test]
fn back_closes_the_nearest_thing_first() {
    let runtime = Runtime::new();
    let page = Page::new();
    page.sheet.set(true);
    runtime.display_frame(&page, SIZE);
    page.inner.set(true);
    runtime.display_frame(&page, SIZE);
    assert_eq!(page.open(), (true, false, true, false));

    assert!(back(&runtime, &page));
    assert_eq!(page.open(), (true, false, false, false), "the popover inside the sheet goes first");
    assert!(back(&runtime, &page));
    assert_eq!(page.open(), (false, false, false, false), "then the sheet");
    assert!(!back(&runtime, &page));
}

#[test]
fn back_is_the_cancel_answer_of_an_alert() {
    let runtime = Runtime::new();
    let page = Page::new();
    page.alert.set(true);
    runtime.display_frame(&page, SIZE);
    assert!(back(&runtime, &page));
    assert!(!page.alert.get(), "the binding is written false, which is the app's cancel");
}
