//! The only native receipt source here is `MTLDrawable`'s presented handler.
//! Blocks own weak scalar tickets, never windows, drawables, or image storage.

use super::{Id, Sel, c_void, msg_f64, msg_u64, msg_void_id, sel};
use bunny_ui::gpu::walk::{FrameBatches, RunAtlas};
use bunny_ui::presentation::Unavailable;
use bunny_ui::presentation::backend::{FrameObservation, PresentationTicket};

#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    #[link_name = "objc_msgSend"]
    fn responds(object: Id, selector: Sel, argument: Sel) -> i8;
}

#[link(name = "System", kind = "dylib")]
unsafe extern "C" {
    static _NSConcreteStackBlock: [*const c_void; 32];
}

#[repr(C)]
struct PresentedBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: extern "C" fn(*mut Self, Id),
    descriptor: *const Descriptor,
    ticket: PresentationTicket,
}

#[repr(C)]
struct Descriptor {
    reserved: usize,
    size: usize,
    copy: extern "C" fn(*mut PresentedBlock, *const PresentedBlock),
    dispose: extern "C" fn(*mut PresentedBlock),
}

static DESCRIPTOR: Descriptor = Descriptor {
    reserved: 0,
    size: std::mem::size_of::<PresentedBlock>(),
    copy: copy_ticket,
    dispose: dispose_ticket,
};

extern "C" fn copy_ticket(destination: *mut PresentedBlock, source: *const PresentedBlock) {
    // Block_copy first copies the literal bytes. Replace the borrowed bitwise
    // Weak copy with an independently owned clone, without dropping those bytes.
    unsafe { (&raw mut (*destination).ticket).write((*source).ticket.clone()) };
}

extern "C" fn dispose_ticket(block: *mut PresentedBlock) {
    // Exactly the clone made by copy_ticket. The stack literal drops normally.
    unsafe { std::ptr::drop_in_place(&raw mut (*block).ticket) };
}

extern "C" fn presented(block: *mut PresentedBlock, drawable: Id) {
    if drawable.is_null() {
        return;
    }
    // Metal may invoke on a background thread. Only scalar Objective-C getters
    // and the thread-safe weak ticket are touched; no AppKit or renderer state.
    unsafe {
        (*block).ticket.presented(
            msg_f64(drawable, sel("presentedTime")),
            msg_u64(drawable, sel("drawableID")),
        );
    }
}

extern "C" fn rendered(block: *mut PresentedBlock, command: Id) {
    // MTLCommandBufferStatusCompleted = 4. Error (5) is not success.
    let success = !command.is_null() && unsafe { msg_u64(command, sel("status")) } == 4;
    unsafe { (*block).ticket.rendered(success) };
}

fn block(ticket: PresentationTicket) -> PresentedBlock {
    PresentedBlock {
        isa: (&raw const _NSConcreteStackBlock).cast(),
        flags: 1 << 25, // BLOCK_HAS_COPY_DISPOSE
        reserved: 0,
        invoke: presented,
        descriptor: &raw const DESCRIPTOR,
        ticket,
    }
}

/// Register before presenting/committing the command. `drawable` is a live
/// `MTLDrawable` borrowed for this call; Metal copies the block before returning.
pub(super) unsafe fn observe(
    drawable: Id,
    command: Id,
    attempt: FrameObservation,
    atlas: &RunAtlas,
    batches: &FrameBatches,
) {
    if command.is_null() {
        attempt.unavailable(Unavailable::CommandUnavailable);
        return;
    }
    for selector in ["addPresentedHandler:", "presentedTime", "drawableID"] {
        if unsafe { responds(drawable, sel("respondsToSelector:"), sel(selector)) } == 0 {
            attempt.unavailable(Unavailable::TimestampUnsupported);
            return;
        }
    }
    let Some(ticket) = attempt.commit(atlas, batches) else {
        return;
    };
    let mut completion = block(ticket.clone());
    completion.invoke = rendered;
    unsafe {
        msg_void_id(
            command,
            sel("addCompletedHandler:"),
            (&raw const completion).cast_mut().cast(),
        );
    };
    let block = block(ticket);
    unsafe {
        msg_void_id(
            drawable,
            sel("addPresentedHandler:"),
            (&raw const block).cast_mut().cast(),
        );
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use bunny_ui::image_engine::ImageFeed;
    use bunny_ui::presentation::backend;

    unsafe extern "C" {
        fn _Block_copy(block: *const c_void) -> *mut c_void;
        fn _Block_release(block: *const c_void);
    }

    #[test]
    fn native_block_copies_own_independent_weak_tickets() {
        let (observer, source) = backend::channel(ImageFeed::new().key(), 16);
        let sender = source.sender();
        let _ = sender.submitted(1);
        let literal = block(sender.commit(1).unwrap());
        let copied = unsafe { _Block_copy((&raw const literal).cast()) }.cast::<PresentedBlock>();
        assert!(!copied.is_null());
        drop(literal);
        // Exercise the copied Rust capture after its original stack is gone.
        unsafe {
            (*copied).ticket.rendered(true);
            (*copied).ticket.presented(1.0, 3);
        }
        assert_eq!(observer.snapshot().presented, 1);
        drop(observer);
        assert!(
            sender.feed().is_none(),
            "the copied block does not own the queue"
        );
        unsafe { _Block_release(copied.cast()) };
        drop(source);
    }
}
