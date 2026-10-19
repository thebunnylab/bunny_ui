//! Recovery must preserve the pixels, not merely the atlas generation stamp.

use super::{
    Color, DisplayList, FrameSlot, MetalPresenter, OffscreenGpu, RunAtlas, Size, acquire_slot,
    class, msg_id, msg_void, msg_void_id, null_mut, objc_autoreleasePoolPop,
    objc_autoreleasePoolPush, sel,
};
use bunny_ui::ext::ViewExt;
use bunny_ui::image_engine::{ImageFeed, RawImages};
use bunny_ui::runtime::Runtime;
use bunny_ui::text_engine::PixelFont;
use bunny_ui::views::image;

fn frame(feed: &ImageFeed, color: [u8; 4]) -> DisplayList {
    feed.push((8, 8), color.repeat(64));
    Runtime::new().display_frame(
        &image(feed).resizable().frame(8.0, 8.0),
        Size {
            width: 8.0,
            height: 8.0,
        },
    )
}

#[test]
fn abandoned_live_upload_retries_the_same_generation_pixels() {
    let mut gpu = OffscreenGpu::new(8, 8).expect("this regression requires actual Metal");
    let feed = ImageFeed::new();
    let images = RawImages::default();
    let first = frame(&feed, [255, 0, 0, 255]);
    gpu.present_wait(&first, 1, Color::BLACK, &PixelFont, &images);
    assert_eq!(gpu.read_rgba(), [255, 0, 0, 255].repeat(64));
    let second = frame(&feed, [0, 255, 0, 255]);
    // The window walks/stages before nextDrawable. A nil drawable returns
    // here without encoding or committing; the next frame takes another slot.
    let index = acquire_slot(
        &mut gpu.slots,
        &mut gpu.cursor,
        &gpu.stack.sels,
        &mut gpu.atlas,
    )
    .index;
    gpu.ground.begin_slot(index, &mut gpu.atlas);
    gpu.walk(&second, 1, &PixelFont, &images);
    assert_eq!(gpu.ground.pending.len(), 1);
    assert_eq!(gpu.atlas.live.values().next().unwrap().generation, 2);
    gpu.present_wait(&second, 1, Color::BLACK, &PixelFont, &images);
    assert_eq!(gpu.read_rgba(), [0, 255, 0, 255].repeat(64));
}

#[test]
fn failed_upload_status_retries_real_pixels_without_a_new_generation() {
    let mut gpu = OffscreenGpu::new(8, 8).expect("this regression requires actual Metal");
    let feed = ImageFeed::new();
    let images = RawImages::default();
    let first = frame(&feed, [255, 0, 0, 255]);
    gpu.present_wait(&first, 1, Color::BLACK, &PixelFont, &images);
    let second = frame(&feed, [0, 255, 0, 255]);
    let index = acquire_slot(
        &mut gpu.slots,
        &mut gpu.cursor,
        &gpu.stack.sels,
        &mut gpu.atlas,
    )
    .index;
    gpu.ground.begin_slot(index, &mut gpu.atlas);
    gpu.walk(&second, 1, &PixelFont, &images);
    // Inject the terminal command result at the slot's outcome seam. The
    // staged bytes deliberately never reach the GPU; this is not a real GPU fault.
    let mut failed = FrameSlot::empty();
    failed.submitted_uploads(&mut gpu.ground, &gpu.atlas);
    assert!(gpu.ground.pending.is_empty());
    assert!(
        !failed.settle_uploads(3, &mut gpu.atlas),
        "scheduled is not completed"
    );
    assert!(!failed.uploads.is_empty());
    assert!(failed.settle_uploads(5, &mut gpu.atlas));
    assert!(
        !failed.settle_uploads(5, &mut gpu.atlas),
        "consume a result once"
    );
    gpu.present_wait(&second, 1, Color::BLACK, &PixelFont, &images);
    assert_eq!(gpu.read_rgba(), [0, 255, 0, 255].repeat(64));
}

#[test]
fn an_older_failed_generation_does_not_invalidate_a_later_upload() {
    let mut gpu = OffscreenGpu::new(8, 8).expect("this regression requires actual Metal");
    let feed = ImageFeed::new();
    let images = RawImages::default();
    let first = frame(&feed, [255, 0, 0, 255]);
    gpu.ground.begin_slot(0, &mut gpu.atlas);
    gpu.walk(&first, 1, &PixelFont, &images);
    let mut failed = FrameSlot::empty();
    failed.submitted_uploads(&mut gpu.ground, &gpu.atlas);
    let newer = frame(&feed, [0, 0, 255, 255]);
    gpu.present_wait(&newer, 1, Color::BLACK, &PixelFont, &images);
    assert!(!failed.settle_uploads(5, &mut gpu.atlas));
    assert!(!gpu.atlas.live.values().next().unwrap().stale);
    assert_eq!(gpu.read_rgba(), [0, 0, 255, 255].repeat(64));
    // A successful cached frame must still reuse the existing bytes.
    let index = acquire_slot(
        &mut gpu.slots,
        &mut gpu.cursor,
        &gpu.stack.sels,
        &mut gpu.atlas,
    )
    .index;
    gpu.ground.begin_slot(index, &mut gpu.atlas);
    gpu.walk(&newer, 1, &PixelFont, &images);
    assert!(gpu.ground.pending.is_empty());
}

#[test]
fn completed_uploads_preserve_the_texture_and_skip_unchanged_copies() {
    let mut gpu = OffscreenGpu::new(8, 8).expect("this regression requires actual Metal");
    let feed = ImageFeed::new();
    let display = frame(&feed, [0, 255, 0, 255]);
    let images = RawImages::default();
    gpu.present_nowait(&display, 1, Color::BLACK, &PixelFont, &images);
    gpu.drain();
    let id = gpu.atlas.live.values().next().unwrap().id;
    gpu.present_wait(&display, 1, Color::BLACK, &PixelFont, &images);
    assert_eq!(gpu.atlas.live.values().next().unwrap().id, id);
    assert!(gpu.ground.pending.is_empty());
    assert!(gpu.slots.iter().all(|slot| slot.uploads.is_empty()));
    assert_eq!(gpu.read_rgba(), [0, 255, 0, 255].repeat(64));
}

#[test]
fn abandoned_upload_also_recovers_in_a_patch() {
    let mut gpu = OffscreenGpu::new(8, 8).expect("this regression requires actual Metal");
    let feed = ImageFeed::new();
    let images = RawImages::default();
    let first = frame(&feed, [255, 0, 0, 255]);
    gpu.present_wait(&first, 1, Color::BLACK, &PixelFont, &images);
    let second = frame(&feed, [0, 255, 0, 255]);
    let index = acquire_slot(
        &mut gpu.slots,
        &mut gpu.cursor,
        &gpu.stack.sels,
        &mut gpu.atlas,
    )
    .index;
    gpu.ground.begin_slot(index, &mut gpu.atlas);
    gpu.walk(&second, 1, &PixelFont, &images);
    let pixels = gpu.patch_rgba(&second, 1, Color::BLACK, &PixelFont, &images, (2, 2, 6, 6));
    assert_eq!(pixels, [0, 255, 0, 255].repeat(16));
}

#[test]
fn an_actual_nil_drawable_does_not_lose_staged_feed_bytes() {
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let layer = msg_id(msg_id(class("CAMetalLayer"), sel("alloc")), sel("init"));
        let mut presenter = MetalPresenter::attach(layer, 1.0).expect("actual Metal presenter");
        let mut gpu = OffscreenGpu::new(8, 8).expect("actual Metal readback");
        let feed = ImageFeed::new();
        let images = RawImages::default();
        let first = frame(&feed, [255, 0, 0, 255]);
        gpu.present_wait(&first, 1, Color::BLACK, &PixelFont, &images);
        gpu.drain();
        std::mem::swap(&mut presenter.ground, &mut gpu.ground);
        std::mem::swap(&mut presenter.atlas, &mut gpu.atlas);
        // An unconfigured layer returns nil from nextDrawable. The real
        // present method still walks and stages before it reaches that result.
        msg_void_id(layer, sel("setDevice:"), null_mut());
        let second = frame(&feed, [0, 255, 0, 255]);
        presenter.present(
            &second,
            Size {
                width: 8.0,
                height: 8.0,
            },
            1,
            Color::BLACK,
            &PixelFont,
            &images,
            true,
        );
        assert!(presenter.retained.is_none(), "no frame was committed");
        assert_eq!(
            presenter.ground.pending.len(),
            1,
            "the actual abort left staged bytes"
        );
        std::mem::swap(&mut presenter.ground, &mut gpu.ground);
        std::mem::swap(&mut presenter.atlas, &mut gpu.atlas);
        gpu.present_wait(&second, 1, Color::BLACK, &PixelFont, &images);
        assert_eq!(gpu.read_rgba(), [0, 255, 0, 255].repeat(64));
        drop(presenter);
        drop(gpu);
        msg_void(layer, sel("release"));
        objc_autoreleasePoolPop(pool);
    }
}

#[test]
fn a_failed_slot_invalidates_only_the_matching_texture_and_generation() {
    use bunny_ui::gpu::walk::LiveEntry;
    use bunny_ui::image_engine::PixelFormat;
    let mut atlas = RunAtlas::new();
    for id in 1..=3 {
        atlas.live.insert(
            id,
            LiveEntry {
                id,
                width: 8,
                height: 8,
                format: PixelFormat::Rgba8,
                generation: id,
                walk: 1,
                stale: false,
            },
        );
    }
    let mut slot = FrameSlot::empty();
    slot.uploads = vec![
        super::uploads::LiveUpload::for_test(1, 1),
        super::uploads::LiveUpload::for_test(2, 1),
    ];
    assert!(slot.settle_uploads(5, &mut atlas));
    assert!(atlas.live[&1].stale);
    assert!(!atlas.live[&2].stale, "a newer generation stays valid");
    assert!(!atlas.live[&3].stale, "an unrelated feed stays valid");
}

#[test]
fn a_failed_upload_bypasses_the_identical_display_shortcut() {
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let layer = msg_id(msg_id(class("CAMetalLayer"), sel("alloc")), sel("init"));
        let mut presenter = MetalPresenter::attach(layer, 1.0).expect("actual Metal presenter");
        let mut gpu = OffscreenGpu::new(8, 8).expect("actual Metal readback");
        let feed = ImageFeed::new();
        let images = RawImages::default();
        let first = frame(&feed, [255, 0, 0, 255]);
        gpu.present_wait(&first, 1, Color::BLACK, &PixelFont, &images);
        gpu.drain();
        std::mem::swap(&mut presenter.ground, &mut gpu.ground);
        std::mem::swap(&mut presenter.atlas, &mut gpu.atlas);
        let second = frame(&feed, [0, 255, 0, 255]);
        presenter.ground.begin_slot(0, &mut presenter.atlas);
        presenter.build_with_retries(&second, 1, (8, 8), &PixelFont, &images);
        // A real empty command supplies native lifetime/queue behavior. The
        // injected failure leaves the live texture at its prior red pixels.
        let command = msg_id(presenter.stack.queue, presenter.stack.sels.command_buffer);
        msg_void(command, presenter.stack.sels.commit);
        msg_void(command, presenter.stack.sels.wait_completed);
        presenter.slots[0].command = msg_id(command, presenter.stack.sels.retain);
        presenter.slots[0].submitted_uploads(&mut presenter.ground, &presenter.atlas);
        presenter.slots[0].failed_upload_for_test = true;
        let kept = (std::rc::Rc::new(second.clone()), (8, 8), 1, Color::BLACK);
        presenter.retained = Some(kept.clone());
        presenter.base = Some(kept);
        presenter.present(
            &second,
            Size {
                width: 8.0,
                height: 8.0,
            },
            1,
            Color::BLACK,
            &PixelFont,
            &images,
            true,
        );
        assert!(
            !presenter.atlas.live.values().next().unwrap().stale,
            "the window itself must retry before the offscreen witness reads it"
        );
        presenter.drain_slots();
        std::mem::swap(&mut presenter.ground, &mut gpu.ground);
        std::mem::swap(&mut presenter.atlas, &mut gpu.atlas);
        gpu.present_wait(&second, 1, Color::BLACK, &PixelFont, &images);
        assert_eq!(gpu.read_rgba(), [0, 255, 0, 255].repeat(64));
        drop(presenter);
        drop(gpu);
        msg_void(layer, sel("release"));
        objc_autoreleasePoolPop(pool);
    }
}
