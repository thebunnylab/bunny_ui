//! The Mac's layer presenter: native opaque bands until a scene needs
//! Metal, and the existing GPU backend after that one-way promotion.
//! This module owns where each window's presenter lives and how its
//! native root layer reaches an `NSView`.
//!
//! The presenter itself — the stack, the shaders, the atlas, the frame
//! — is the shared Apple half ([`bunny_ui_apple::metal`]). This module
//! is the part only AppKit knows: a view has no ivars, so the presenter
//! sits in a thread-local next to the run loop, keyed by the view; the
//! layer is grafted with `setLayer:` BEFORE `setWantsLayer:`, so the
//! view becomes layer-HOSTING and `drawRect:` never runs; and the
//! window's delegate is the one who says a live resize started.

use std::cell::RefCell;
use std::collections::HashMap;

use bunny_ui::image_engine::ImageEngine;
use bunny_ui::layout::{Color, DisplayList, Size};
use bunny_ui::text_engine::TextEngine;
pub use bunny_ui_apple::metal::OffscreenGpu;
use bunny_ui_apple::metal::WindowPresenter as MetalPresenter;

use crate::ffi::{Id, Sel, class, sel};

#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    #[link_name = "objc_msgSend"]
    fn msg_id(obj: Id, sel: Sel) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_void(obj: Id, sel: Sel);
    #[link_name = "objc_msgSend"]
    fn msg_void_id(obj: Id, sel: Sel, a: Id);
}

/// Rendering and pacing belong to the same view. A second top-level window
/// must never replace the first window's layer or consume its back-pressure.
struct Presenter {
    renderer: MetalPresenter,
    congested: bool,
}

thread_local! {
    /// Every top-level and pooled dialog view owns one presenter.
    static VIEW_PRESENTERS: RefCell<HashMap<usize, Presenter>> =
        RefCell::new(HashMap::new());
}

/// A present that waited this long for a drawable, in milliseconds, found
/// the line of frames in front of the display full. A drawable that is
/// free is handed over in well under a tenth of this; one that is not is
/// freed by the display's own refresh, milliseconds away.
const CONGESTED_MS: f64 = 1.5;

/// Grafts the native root layer onto the view — called by `create_window`
/// BEFORE `setWantsLayer:`, so the view becomes layer-HOSTING and
/// `drawRect:` never runs. Returns false (and touches nothing) when the
/// layer path is explicitly refused or cannot come up; the caller
/// proceeds with the CPU path. Metal itself initializes on demand.
pub(crate) fn try_install(view: Id, scale: f64, width: f64, height: f64) -> bool {
    match graft(view, scale, width, height) {
        Some(renderer) => {
            VIEW_PRESENTERS.with(|slot| {
                slot.borrow_mut().insert(
                    view as usize,
                    Presenter {
                        renderer,
                        congested: false,
                    },
                );
            });
            true
        }
        None => false,
    }
}

/// A closed top-level view releases only its own presenter. Pooled dialog
/// views remain registered while reusable, just as their native windows do.
pub fn forget_view(view: Id) {
    VIEW_PRESENTERS.with(|slot| {
        slot.borrow_mut().remove(&(view as usize));
    });
}

/// Builds a presenter over a fresh CALayer on `view`, or answers
/// `None` when the GPU road is refused or cannot come up. Touches the
/// view only on `Some`: the layer is configured first, grafted second,
/// and primed (the anti-flash clear) once it hangs from the view.
fn graft(view: Id, scale: f64, width: f64, height: f64) -> Option<MetalPresenter> {
    unsafe {
        let layer = msg_id(msg_id(class("CALayer"), sel("alloc")), sel("init"));
        if layer.is_null() {
            return None;
        }
        let Some(mut presenter) = MetalPresenter::attach(layer, scale) else {
            msg_void(layer, sel("release"));
            return None;
        };
        msg_void_id(view, sel("setLayer:"), layer);
        presenter.prime(width, height, scale.round().max(1.0) as usize);
        Some(presenter)
    }
}

/// Diagnostics: this view's atlas counts and the total live presenter count.
/// `None` when this particular view uses the CPU road.
pub fn retained_counts(view: Id) -> Option<(bunny_ui_apple::metal::AtlasCounts, usize)> {
    VIEW_PRESENTERS.with(|slot| {
        let presenters = slot.borrow();
        presenters
            .get(&(view as usize))
            .map(|presenter| (presenter.renderer.atlas_counts(), presenters.len()))
    })
}

/// Every window's presenter rests — its frames in flight let go, its atlas
/// and its drawables off screen offered back to the system — when the
/// shell's frame driver parks (see [`MetalPresenter::rest`]).
pub(crate) fn rest() {
    if !each_presenter(MetalPresenter::rest) {
        offer_later(0);
    }
}

/// Runs `f` on every window's presenter; true when every one answered true.
fn each_presenter(f: impl Fn(&mut MetalPresenter) -> bool) -> bool {
    let mut all = true;
    VIEW_PRESENTERS.with(|slot| {
        if let Ok(mut presenters) = slot.try_borrow_mut() {
            for presenter in presenters.values_mut() {
                all &= f(&mut presenter.renderer);
            }
        }
    });
    all
}

#[allow(non_upper_case_globals)]
unsafe extern "C" {
    static _dispatch_main_q: std::ffi::c_void;
    fn dispatch_time(when: u64, delta: i64) -> u64;
    fn dispatch_after_f(
        when: u64,
        queue: *const std::ffi::c_void,
        context: *mut std::ffi::c_void,
        work: extern "C" fn(*mut std::ffi::c_void),
    );
}

/// How long after a park the drawables are offered again when the last
/// present had not landed: past the time a present takes to land.
const OFFER_AGAIN_NS: i64 = 120_000_000;

/// Asks the presenters to offer their drawables a moment from now, on the
/// main queue — the window never waits for its last present to land.
fn offer_later(attempt: usize) {
    unsafe {
        dispatch_after_f(
            dispatch_time(0, OFFER_AGAIN_NS),
            &raw const _dispatch_main_q,
            attempt as *mut std::ffi::c_void,
            offer_again,
        );
    }
}

extern "C" fn offer_again(context: *mut std::ffi::c_void) {
    let attempt = context as usize;
    // a presenter that painted since it rested has nothing to offer
    if !each_presenter(MetalPresenter::offer_drawables) && attempt < 4 {
        offer_later(attempt + 1);
    }
}

/// True when this window presents by GPU — the shell branches ONCE per
/// frame on this, never mid-flight.
pub fn active(view: Id) -> bool {
    VIEW_PRESENTERS.with(|slot| slot.borrow().contains_key(&(view as usize)))
}

/// Whether any view owns a presenter, for the application-wide rest timer.
pub fn any_active() -> bool {
    VIEW_PRESENTERS.with(|slot| !slot.borrow().is_empty())
}

/// Presents one frame on a grafted top-level or dialog view. False when it was
/// never grafted, and the caller takes the CPU road for it. `live` is
/// the addressed window's own word on its resize.
#[allow(
    clippy::too_many_arguments,
    reason = "one native frame carries its view, geometry, engines and resize state"
)]
pub(crate) fn present_view(
    view: Id,
    display: &DisplayList,
    size: Size,
    scale: usize,
    canvas: Color,
    text: &dyn TextEngine,
    images: &dyn ImageEngine,
    live: bool,
) -> bool {
    VIEW_PRESENTERS.with(|slot| {
        let mut presenters = slot.borrow_mut();
        let Some(presenter) = presenters.get_mut(&(view as usize)) else {
            return false;
        };
        presenter
            .renderer
            .present(display, size, scale, canvas, text, images, live);
        let waited = presenter.renderer.drawable_wait_ms();
        if !live && waited > CONGESTED_MS {
            presenter.congested = true;
            bunny_ui_apple::trace::mark("X", format_args!("what=line-full wait={waited:.1}"));
        }
        true
    })
}

/// Arms the addressed view's transaction before its first resize frame.
/// A CPU view is a no-op; a dialog or another top-level view stays untouched.
pub(crate) fn arm_transaction_view(view: Id, live: bool) {
    VIEW_PRESENTERS.with(|slot| {
        if let Some(presenter) = slot.borrow_mut().get_mut(&(view as usize)) {
            presenter.renderer.set_transactional(live);
        }
    });
}

/// Did this view wait for a drawable since its pacer last asked? A sibling's
/// traffic must not consume or introduce back-pressure for this window.
pub fn take_congested(view: Id) -> bool {
    VIEW_PRESENTERS.with(|slot| {
        slot.borrow_mut()
            .get_mut(&(view as usize))
            .is_some_and(|presenter| std::mem::take(&mut presenter.congested))
    })
}
