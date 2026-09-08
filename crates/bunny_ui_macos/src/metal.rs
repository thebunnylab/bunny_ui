//! The Mac's side of the Metal road: where each window's presenter
//! lives, and how a `CAMetalLayer` reaches an `NSView`.
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
use bunny_ui_apple::metal::MetalPresenter;
pub use bunny_ui_apple::metal::OffscreenGpu;

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

thread_local! {
    /// The main window's presenter.
    static PRESENTER: RefCell<Option<MetalPresenter>> = const { RefCell::new(None) };
    /// One presenter per grafted DIALOG view (D127: a dialog is a real
    /// window, and a real window resizes on the GPU road like the main
    /// one — a CPU raster of a whole 1220×820 dialog on every step of a
    /// drag was the difference between a fluid workbench and a dialog
    /// that lagged its own corner).
    static VIEW_PRESENTERS: RefCell<HashMap<usize, MetalPresenter>> =
        RefCell::new(HashMap::new());
}

/// Grafts the CAMetalLayer onto the view — called by `create_window`
/// BEFORE `setWantsLayer:`, so the view becomes layer-HOSTING and
/// `drawRect:` never runs. Returns false (and touches nothing) when the
/// GPU path is refused or cannot come up; the caller proceeds with the
/// CPU path.
pub(crate) fn try_install(view: Id, scale: f64, width: f64, height: f64) -> bool {
    match graft(view, scale, width, height) {
        Some(presenter) => {
            PRESENTER.with(|slot| *slot.borrow_mut() = Some(presenter));
            true
        }
        None => false,
    }
}

/// The same graft on a DIALOG's view — a window of its own, presented
/// by its own layer and its own presenter, keyed by the view (a dialog
/// is pooled reusable-dead and never re-grafted). False leaves the view
/// on the CPU road, exactly as before.
pub(crate) fn try_install_view(view: Id, scale: f64, width: f64, height: f64) -> bool {
    match graft(view, scale, width, height) {
        Some(presenter) => {
            VIEW_PRESENTERS.with(|slot| {
                slot.borrow_mut().insert(view as usize, presenter);
            });
            true
        }
        None => false,
    }
}

/// Builds a presenter over a fresh CAMetalLayer on `view`, or answers
/// `None` when the GPU road is refused or cannot come up. Touches the
/// view only on `Some`: the layer is configured first, grafted second,
/// and primed (the anti-flash clear) once it hangs from the view.
fn graft(view: Id, scale: f64, width: f64, height: f64) -> Option<MetalPresenter> {
    unsafe {
        let layer = msg_id(msg_id(class("CAMetalLayer"), sel("alloc")), sel("init"));
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

/// True when this window presents by GPU — the shell branches ONCE per
/// frame on this, never mid-flight.
pub(crate) fn active() -> bool {
    PRESENTER.with(|slot| slot.borrow().is_some())
}

/// Presents one frame on a grafted DIALOG view. False when the view was
/// never grafted, and the caller takes the CPU road for it. `live` is
/// the dialog's own word on its resize.
#[allow(clippy::too_many_arguments)]
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
        presenter.present(display, size, scale, canvas, text, images, live);
        true
    })
}

/// [`arm_transaction`] for a grafted dialog view — the dialog's own
/// delegate speaks for its own drag. A view on the CPU road is a no-op.
pub(crate) fn arm_transaction_view(view: Id, live: bool) {
    VIEW_PRESENTERS.with(|slot| {
        if let Some(presenter) = slot.borrow_mut().get_mut(&(view as usize)) {
            presenter.set_transactional(live);
        }
    });
}

/// Arms (or disarms) the layer's transactional present, from AppKit's
/// own word that a drag is starting. It arrives BEFORE the first
/// resized frame, which is the only moment early enough: by the time a
/// frame observes `inLiveResize` the window has already grown, and a
/// drawable of the old size stretched to the new bounds is what the
/// eye reads as the whole UI drawn twice.
pub(crate) fn arm_transaction(live: bool) {
    PRESENTER.with(|slot| {
        if let Some(presenter) = slot.borrow_mut().as_mut() {
            presenter.set_transactional(live);
        }
    });
}

/// The GPU twin of the Surface + blit path: same display list in, one
/// presented frame out. `text` is the frame's engine — the atlas
/// rasterizes through it, exactly like the CPU compositor. `live` is
/// the window's word on whether a resize drag is under way.
pub(crate) fn present_window(
    display: &DisplayList,
    size: Size,
    scale: usize,
    canvas: Color,
    text: &dyn TextEngine,
    images: &dyn ImageEngine,
    live: bool,
) {
    PRESENTER.with(|slot| {
        if let Some(presenter) = slot.borrow_mut().as_mut() {
            presenter.present(display, size, scale, canvas, text, images, live);
        }
    });
}
