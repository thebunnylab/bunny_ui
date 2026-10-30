use super::*;
use crate::metal::{MTLCreateSystemDefaultDevice, objc_autoreleasePoolPop, objc_autoreleasePoolPush};

#[link(name = "IOSurface", kind = "framework")]
unsafe extern "C" {
    fn IOSurfaceIncrementUseCount(surface: Id);
    fn IOSurfaceDecrementUseCount(surface: Id);
    fn IOSurfaceGetWidth(surface: Id) -> usize;
}

/// The compositor reading a surface, as Core Animation's commit marks it.
struct Reading(Id);

impl Reading {
    fn new(surface: Id) -> Self {
        unsafe { IOSurfaceIncrementUseCount(surface) };
        Self(surface)
    }
}

impl Drop for Reading {
    fn drop(&mut self) {
        unsafe { IOSurfaceDecrementUseCount(self.0) };
    }
}

/// A free canvas, shown: what a patch's present does with one.
unsafe fn paint(canvases: &mut Canvases, device: Id, size: (usize, usize)) -> Option<Id> {
    let free = unsafe { canvases.free(device, size) }?;
    Some(canvases.show(free))
}

#[test]
fn a_canvas_is_never_painted_while_shown_or_read() {
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let device = MTLCreateSystemDefaultDevice();
        if device.is_null() {
            eprintln!("no metal device — skipping");
            return;
        }
        let mut canvases = Canvases::default();
        let first = canvases.free(device, (64, 32)).expect("a first canvas");
        assert!(!first.texture().is_null());
        let first = canvases.show(first);
        let first_read = Reading::new(first);
        let second = paint(&mut canvases, device, (64, 32)).expect("a second canvas");
        assert_ne!(second, first, "the shown canvas is never handed out");
        let second_read = Reading::new(second);
        let third = paint(&mut canvases, device, (64, 32)).expect("a third canvas");
        assert!(third != first && third != second);
        // shown by index, the other two read by the compositor
        assert!(canvases.free(device, (64, 32)).is_none(), "three at most, all busy");
        drop(second_read);
        let again = paint(&mut canvases, device, (64, 32)).expect("the canvas read no more");
        assert_eq!(again, second);
        drop(first_read);
        msg_void(device, sel("release"));
        objc_autoreleasePoolPop(pool);
    }
}

#[test]
fn a_new_box_size_remakes_only_the_canvas_it_paints() {
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let device = MTLCreateSystemDefaultDevice();
        if device.is_null() {
            eprintln!("no metal device — skipping");
            return;
        }
        let mut canvases = Canvases::default();
        let shown = paint(&mut canvases, device, (64, 32)).expect("a canvas");
        let grown = paint(&mut canvases, device, (192, 128)).expect("a larger canvas");
        assert_eq!(IOSurfaceGetWidth(grown), 192);
        assert_eq!(IOSurfaceGetWidth(shown), 64, "the canvas shown before keeps its pixels");
        let back = paint(&mut canvases, device, (64, 32)).expect("the small size again");
        assert_eq!(back, shown, "a free canvas of the right size is reused, not remade");
        msg_void(device, sel("release"));
        objc_autoreleasePoolPop(pool);
    }
}

#[test]
fn a_resting_patch_keeps_only_the_canvas_it_shows() {
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let device = MTLCreateSystemDefaultDevice();
        if device.is_null() {
            eprintln!("no metal device — skipping");
            return;
        }
        let mut canvases = Canvases::default();
        paint(&mut canvases, device, (64, 32)).expect("a canvas");
        let shown = paint(&mut canvases, device, (64, 32)).expect("another");
        assert_eq!(canvases.kept.len(), 2);
        canvases.rest();
        assert_eq!(canvases.kept.len(), 1, "the canvas off screen is let go");
        let next = paint(&mut canvases, device, (64, 32)).expect("a canvas after rest");
        assert_ne!(next, shown, "the shown one is still never painted");
        let mut idle = Canvases::default();
        idle.rest();
        assert!(idle.kept.is_empty());
        msg_void(device, sel("release"));
        objc_autoreleasePoolPop(pool);
    }
}
