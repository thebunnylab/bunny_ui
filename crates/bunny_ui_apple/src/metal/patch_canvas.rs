//! A Metal patch paints into IOSurfaces of its own and hands one to its
//! layer as `contents`. That is an ordinary layer property: the pixels,
//! the box and the layer's visibility reach the screen in ONE commit.
//!
//! A `CAMetalLayer` drawable does not, even presented inside the
//! transaction with `presentsWithTransaction`: the window server shows
//! the drawable a refresh after the geometry committed with it. A patch
//! that moved, grew or came back from hiding showed the pixels of its
//! previous box stretched over the new one for that refresh — a
//! magnified rectangle flashing through every set-up transition
//! (T2-BUNNY-385). Waiting for the GPU to complete, or flushing the
//! implicit transaction first, changes nothing: the drawable takes a
//! different road to the screen than the layer tree does.
//!
//! ## What this ships
//!
//! [`Canvases`] keeps up to three surfaces per patch layer, each with the
//! Metal texture that renders into it. [`Canvases::free`] hands out one
//! the compositor is done with as a [`Free`] canvas; only a `Free` canvas
//! can be [`Canvases::show`]n, so the surface the layer shows is never
//! painted over.
//!
//! ## Production gotchas
//!
//! - The layer reads the surface as soon as the transaction commits, so
//!   the command buffer that paints it must have COMPLETED first — a
//!   scheduled buffer is not enough for a surface the compositor samples
//!   directly. A patch is a small box: the wait is a fraction of a frame.
//! - `IOSurfaceIsInUse` turns true only once Core Animation's commit took
//!   its use count, so the canvas just shown is protected by index, as
//!   the software patch's backing does.
//! - The surface is tagged Device RGB, as the software patch's is: the
//!   window's own layer is uncolormatched, and an untagged surface takes
//!   CA's default profile and visibly lightens the box.

use super::software_patch::Surface;
use super::{
    Id, PIXEL_FORMAT_BGRA8, STORAGE_MODE_SHARED, TEXTURE_USAGE_RENDER_TARGET, class,
    msg_id_u64_u64_u64_bool, msg_void, msg_void_u64, sel,
};

/// How many canvases a patch keeps at most: the one its layer shows, one
/// the compositor may still be reading, and one to paint.
const CANVASES: usize = 3;

#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    #[link_name = "objc_msgSend"]
    fn msg_id_id_id_u64(obj: Id, sel: super::Sel, a: Id, b: Id, c: u64) -> Id;
}

/// One IOSurface and the render target that paints it.
struct Canvas {
    surface: Surface,
    texture: Id,
}

impl Canvas {
    /// A canvas of `size` pixels on `device`; `None` when either half
    /// cannot be had.
    unsafe fn new(device: Id, size: (usize, usize)) -> Option<Canvas> {
        unsafe {
            let surface = Surface::new(size)?;
            let descriptor = msg_id_u64_u64_u64_bool(
                class("MTLTextureDescriptor"),
                sel("texture2DDescriptorWithPixelFormat:width:height:mipmapped:"),
                PIXEL_FORMAT_BGRA8,
                size.0 as u64,
                size.1 as u64,
                0,
            );
            if descriptor.is_null() {
                return None;
            }
            msg_void_u64(descriptor, sel("setUsage:"), TEXTURE_USAGE_RENDER_TARGET);
            // shared memory: the module's premise (Apple Silicon)
            msg_void_u64(descriptor, sel("setStorageMode:"), STORAGE_MODE_SHARED);
            let texture = msg_id_id_id_u64(
                device,
                sel("newTextureWithDescriptor:iosurface:plane:"),
                descriptor,
                surface.raw(),
                0,
            );
            (!texture.is_null()).then_some(Canvas { surface, texture })
        }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe { msg_void(self.texture, sel("release")) };
    }
}

/// A canvas the compositor is done with, ready to paint — the only kind
/// [`Canvases::show`] takes.
pub(super) struct Free {
    index: usize,
    texture: Id,
}

impl Free {
    /// The render target this frame's box paints into.
    pub(super) fn texture(&self) -> Id {
        self.texture
    }
}

/// A patch layer's canvases. Made on its first paint, remade at a new
/// size one at a time as each comes free.
#[derive(Default)]
pub(super) struct Canvases {
    kept: Vec<Canvas>,
    /// The canvas the layer shows.
    shown: Option<usize>,
}

impl Canvases {
    /// A canvas of `size` to paint: never the one the layer shows, never
    /// one the compositor still reads. `None` when all are busy or one
    /// cannot be made — the frame then paints whole.
    pub(super) unsafe fn free(&mut self, device: Id, size: (usize, usize)) -> Option<Free> {
        unsafe {
            let free = self
                .kept
                .iter()
                .enumerate()
                .position(|(index, canvas)| Some(index) != self.shown && !canvas.surface.busy());
            let index = match free {
                Some(index) => {
                    if self.kept[index].surface.size() != size {
                        self.kept[index] = Canvas::new(device, size)?;
                    }
                    index
                }
                None if self.kept.len() < CANVASES => {
                    self.kept.push(Canvas::new(device, size)?);
                    self.kept.len() - 1
                }
                None => return None,
            };
            Some(Free { index, texture: self.kept[index].texture })
        }
    }

    /// The surface the layer takes as its contents, in the transaction
    /// the caller has open. Its painting must have completed.
    pub(super) fn show(&mut self, free: Free) -> Id {
        self.shown = Some(free.index);
        self.kept[free.index].surface.raw()
    }

    /// Lets go of every canvas but the one the layer shows, while the
    /// window rests — the next stroke makes what it needs. A canvas the
    /// compositor still reads lives on in its hands.
    pub(super) fn rest(&mut self) {
        let shown = self.shown.take().map(|index| self.kept.swap_remove(index));
        self.kept.clear();
        if let Some(shown) = shown {
            self.kept.push(shown);
            self.shown = Some(0);
        }
    }
}

#[cfg(test)]
#[path = "patch_canvas_tests.rs"]
mod tests;
