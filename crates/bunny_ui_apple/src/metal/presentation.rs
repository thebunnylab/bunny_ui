//! The only native receipt source here is `MTLDrawable`'s presented handler.
//! Blocks own weak scalar tickets, never windows, drawables, or image storage.

use super::{EncodedFrame, Id, MetalGround, Sel, c_void, msg_f64, msg_u64, msg_void_id, sel};
use bunny_ui::gpu::walk::{FrameBatches, RunAtlas};
use bunny_ui::image_engine::FeedKey;
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
    encoded: EncodedFrame,
    attempt: FrameObservation,
    atlas: &RunAtlas,
    batches: &FrameBatches,
    ground: &MetalGround,
) {
    let command = encoded.command;
    if command.is_null() {
        attempt.unavailable(Unavailable::CommandUnavailable);
        return;
    }
    if !batches.glass.is_empty() {
        attempt.unavailable(Unavailable::NativeCompositing);
        return;
    }
    if !encoded.feed_uploads_encoded
        || !attempt
            .feed()
            .is_some_and(|feed| uploaded_in_frame(feed, atlas, ground))
    {
        attempt.unavailable(Unavailable::NoCurrentUpload);
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

fn uploaded_in_frame(feed: FeedKey, atlas: &RunAtlas, ground: &MetalGround) -> bool {
    let Some(entry) = atlas.live.get(&feed.raw()) else {
        return false;
    };
    let Some(texture) = ground.textures.get(&entry.id) else {
        return false;
    };
    ground.pending.iter().any(|copy| {
        copy.texture == *texture
            && copy.width == u64::from(entry.width)
            && copy.height == u64::from(entry.height)
    })
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
    #[test]
    fn cached_generation_without_a_current_upload_is_not_a_receipt_source() {
        use crate::metal::OffscreenGpu;
        use bunny_ui::image_engine::RawImages;
        use bunny_ui::layout::{Color, DisplayList, DrawCommand, Point, Rect, Size};
        use bunny_ui::text_engine::PixelFont;
        let Some(mut gpu) = OffscreenGpu::new(8, 8) else {
            return;
        };
        let feed = ImageFeed::new();
        feed.push((1, 1), vec![255, 0, 0, 255]);
        let display: DisplayList = vec![DrawCommand::Image {
            rect: Rect {
                origin: Point { x: 0.0, y: 0.0 },
                size: Size {
                    width: 8.0,
                    height: 8.0,
                },
            },
            source: feed.source(),
        }]
        .into();
        gpu.present_wait(&display, 1, Color::WHITE, &PixelFont, &RawImages::default());
        assert!(uploaded_in_frame(feed.key(), &gpu.atlas, &gpu.ground));
        // A new staging slot has no bytes for this generation, even though
        // the cache still names it. A prior upload may have been abandoned.
        gpu.ground.begin_slot(1);
        assert!(!uploaded_in_frame(feed.key(), &gpu.atlas, &gpu.ground));
        assert_eq!(gpu.atlas.live[&feed.key().raw()].generation, 1);
        feed.push((1, 1), vec![0, 255, 0, 255]);
        let next: DisplayList = vec![DrawCommand::Image {
            rect: Rect {
                origin: Point { x: 0.0, y: 0.0 },
                size: Size {
                    width: 8.0,
                    height: 8.0,
                },
            },
            source: feed.source(),
        }]
        .into();
        gpu.walk(&next, 1, &PixelFont, &RawImages::default());
        assert!(uploaded_in_frame(feed.key(), &gpu.atlas, &gpu.ground));
        assert_eq!(gpu.atlas.live[&feed.key().raw()].generation, 2);
    }
}
