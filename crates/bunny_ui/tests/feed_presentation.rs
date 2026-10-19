use bunny_ui_core::image_engine::ImageFeed;
use bunny_ui_core::presentation::{FeedPresentation, PresentationEvent, Unavailable, backend};

fn committed(
    sender: &backend::PresentationSender,
    generation: u64,
) -> Option<backend::PresentationTicket> {
    let ticket = sender.commit(generation)?;
    ticket.rendered(true);
    Some(ticket)
}

fn channel(capacity: usize) -> (FeedPresentation, backend::PresentationSource) {
    backend::channel(ImageFeed::new().key(), capacity)
}

#[test]
fn submission_and_commit_are_not_presentation() {
    let (observer, source) = channel(16);
    let sender = source.sender();
    assert!(sender.submitted(1));
    let _ticket = committed(&sender, 1).unwrap();
    assert_eq!(observer.snapshot().presented, 0);
    assert_eq!(observer.snapshot().pending, 1);
    assert!(
        observer
            .drain()
            .iter()
            .any(|event| matches!(event, PresentationEvent::Committed { generation: 1 }))
    );
}

#[test]
fn repeated_repaints_never_credit_a_generation_twice() {
    let (observer, source) = channel(16);
    let sender = source.sender();
    assert!(sender.submitted(1));
    let ticket = committed(&sender, 1).unwrap();
    assert!(committed(&sender, 1).is_none());
    ticket.presented(1.25, 7);
    ticket.presented(1.5, 7);
    assert!(!sender.submitted(1));
    assert!(committed(&sender, 1).is_none());
    assert_eq!(observer.snapshot().presented, 1);
    assert!(matches!(
        observer.drain().last(),
        Some(PresentationEvent::Presented {
            generation: 1,
            drawable_id: 7,
            host_time_seconds: 1.25
        })
    ));
}

#[test]
fn replacing_a_snapshot_before_commit_does_not_count_it() {
    let (observer, source) = channel(16);
    let sender = source.sender();
    let _ = sender.submitted(1);
    let _ = sender.submitted(2);
    assert!(committed(&sender, 1).is_none());
    committed(&sender, 2).unwrap().presented(2.0, 9);
    assert_eq!(observer.snapshot().presented, 1);
    assert!(
        observer
            .drain()
            .iter()
            .all(|event| !matches!(event, PresentationEvent::Presented { generation: 1, .. }))
    );
}

#[test]
fn callbacks_may_arrive_out_of_order() {
    let (observer, source) = channel(16);
    let sender = source.sender();
    let _ = sender.submitted(1);
    let one = committed(&sender, 1).unwrap();
    let _ = sender.submitted(2);
    let two = committed(&sender, 2).unwrap();
    two.presented(2.0, 2);
    one.presented(1.0, 1);
    one.presented(1.0, 1);
    assert_eq!(observer.snapshot().presented, 2);
    assert_eq!(observer.snapshot().pending, 0);
}

#[test]
fn zero_and_nonfinite_timestamps_never_prove_presentation() {
    for time in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let (observer, source) = channel(16);
        let sender = source.sender();
        let _ = sender.submitted(1);
        committed(&sender, 1).unwrap().presented(time, 1);
        assert_eq!(observer.snapshot().presented, 0);
        assert_eq!(observer.snapshot().pending, 0);
        assert!(
            observer
                .drain()
                .iter()
                .any(|event| matches!(event, PresentationEvent::Unconfirmed { generation: 1 }))
        );
        committed(&sender, 1).unwrap().presented(2.0, 2);
        assert_eq!(observer.snapshot().presented, 1);
    }
}

#[test]
fn inflight_backpressure_is_bounded_and_recoverable() {
    let (observer, source) = channel(16);
    let sender = source.sender();
    let tickets: Vec<_> = (1..=3)
        .map(|generation| {
            let _ = sender.submitted(generation);
            committed(&sender, generation).unwrap()
        })
        .collect();
    let _ = sender.submitted(4);
    assert!(committed(&sender, 4).is_none());
    assert_eq!(observer.snapshot().pending, 3);
    assert_eq!(
        observer.snapshot().unavailable,
        Some(Unavailable::Backpressure)
    );
    tickets[0].presented(1.0, 1);
    committed(&sender, 4).unwrap().presented(4.0, 4);
    assert_eq!(observer.snapshot().presented, 2);
}

#[test]
fn slow_consumers_report_event_loss_without_unbounded_storage() {
    let (observer, source) = channel(2);
    let sender = source.sender();
    for generation in 1..=100 {
        let _ = sender.submitted(generation);
        committed(&sender, generation)
            .unwrap()
            .presented(1.0, generation);
    }
    assert_eq!(observer.snapshot().presented, 100);
    assert_eq!(observer.snapshot().queued, 2);
    assert!(observer.snapshot().lost_events > 0);
    assert_eq!(observer.drain().len(), 2);
    assert_eq!(observer.snapshot().queued, 0);
}

#[test]
fn capacities_are_clamped_to_one_through_256() {
    assert_eq!(channel(0).0.snapshot().capacity, 1);
    assert_eq!(channel(usize::MAX).0.snapshot().capacity, 256);
}

#[test]
fn source_close_cancels_late_callbacks_and_preserves_a_closed_receipt() {
    let (observer, source) = channel(16);
    let sender = source.sender();
    let _ = sender.submitted(1);
    let ticket = committed(&sender, 1).unwrap();
    drop(source);
    ticket.presented(1.0, 1);
    assert!(observer.snapshot().closed);
    assert_eq!(observer.snapshot().pending, 0);
    assert_eq!(observer.snapshot().presented, 0);
    assert!(!sender.submitted(2));
    assert!(matches!(
        observer.drain().last(),
        Some(PresentationEvent::Closed)
    ));
}

#[test]
fn dropping_the_listener_disables_producers_and_tickets() {
    let (observer, source) = channel(16);
    let sender = source.sender();
    let _ = sender.submitted(1);
    let ticket = committed(&sender, 1).unwrap();
    drop(observer);
    assert!(sender.feed().is_none());
    assert!(!sender.submitted(2));
    assert!(committed(&sender, 2).is_none());
    ticket.presented(1.0, 1);
}

#[test]
fn same_feed_in_two_windows_has_independent_sessions() {
    let feed = ImageFeed::new();
    let (a, sa) = backend::channel(feed.key(), 16);
    let (b, sb) = backend::channel(feed.key(), 16);
    assert_ne!(a.session(), b.session());
    assert_eq!(a.feed(), b.feed());
    let _ = sa.sender().submitted(1);
    let _ = sb.sender().submitted(1);
    committed(&sa.sender(), 1).unwrap().presented(1.0, 1);
    assert_eq!(a.snapshot().presented, 1);
    assert_eq!(b.snapshot().presented, 0);
}

#[test]
fn unavailable_backends_and_old_generations_fail_closed() {
    let (observer, source) = channel(16);
    let sender = source.sender();
    sender.unavailable(Unavailable::UnsupportedPlatform);
    assert_eq!(
        observer.snapshot().unavailable,
        Some(Unavailable::UnsupportedPlatform)
    );
    let _ = sender.submitted(5);
    assert!(!sender.submitted(4));
    assert!(committed(&sender, 4).is_none());
    assert_eq!(observer.snapshot().presented, 0);
}

#[test]
fn callbacks_can_publish_from_other_threads() {
    let (observer, source) = channel(16);
    let sender = source.sender();
    let _ = sender.submitted(1);
    let ticket = committed(&sender, 1).unwrap();
    std::thread::spawn(move || ticket.presented(1.0, 3))
        .join()
        .unwrap();
    assert_eq!(observer.snapshot().presented, 1);
}

#[test]
fn suspension_invalidates_tickets_but_a_visible_retry_can_succeed() {
    let (observer, source) = channel(16);
    let sender = source.sender();
    let _ = sender.submitted(1);
    let hidden = committed(&sender, 1).unwrap();
    sender.suspend();
    hidden.presented(1.0, 1);
    assert_eq!(observer.snapshot().presented, 0);
    assert_eq!(
        observer.snapshot().unavailable,
        Some(Unavailable::WindowNotVisible)
    );
    assert!(sender.commit(1).is_none());
    sender.resume();
    committed(&sender, 1).unwrap().presented(2.0, 2);
    assert_eq!(observer.snapshot().presented, 1);
}

#[cfg(feature = "gpu")]
mod gpu {
    use super::*;
    use bunny_ui_core::gpu::walk::{FrameBatches, RecordingGround, RunAtlas, build_frame};
    use bunny_ui_core::image_engine::{ImageSource, RawImages};
    use bunny_ui_core::layout::{DisplayList, DrawCommand, Point, Rect, Size};
    use bunny_ui_core::text_engine::PixelFont;

    const fn rect(x: f64) -> Rect {
        Rect {
            origin: Point { x, y: 0.0 },
            size: Size {
                width: 20.0,
                height: 20.0,
            },
        }
    }

    fn display(source: ImageSource, x: f64) -> DisplayList {
        vec![DrawCommand::Image {
            rect: rect(x),
            source,
        }]
        .into()
    }

    #[test]
    fn actual_live_draw_and_matching_texture_are_required() {
        let feed = ImageFeed::new();
        feed.push((2, 2), vec![255; 16]);
        let (observer, source) = backend::channel(feed.key(), 16);
        let mut atlas = RunAtlas::new();
        let mut batches = FrameBatches::default();
        let mut ground = RecordingGround::default();
        let list = display(feed.source(), 0.0);
        build_frame(
            &mut ground,
            &list,
            1,
            (40, 40),
            &PixelFont,
            &RawImages::default(),
            &mut atlas,
            &mut batches,
        )
        .unwrap();
        let attempt = backend::FrameObservation::begin(&source.sender(), &list).unwrap();
        let ticket = attempt.commit(&atlas, &batches).unwrap();
        ticket.rendered(true);
        ticket.presented(1.0, 1);
        assert_eq!(observer.snapshot().presented, 1);
        feed.push((2, 2), vec![128; 16]);
        let newer = display(feed.source(), 0.0);
        let attempt = backend::FrameObservation::begin(&source.sender(), &newer).unwrap();
        assert!(attempt.commit(&atlas, &batches).is_none());
        assert_eq!(
            observer.snapshot().unavailable,
            Some(Unavailable::UploadFailed)
        );
    }

    #[test]
    fn clipped_feeds_and_stale_uploads_never_get_tickets() {
        let feed = ImageFeed::new();
        feed.push((2, 2), vec![255; 16]);
        let (observer, source) = backend::channel(feed.key(), 16);
        let mut atlas = RunAtlas::new();
        let mut batches = FrameBatches::default();
        let mut ground = RecordingGround::default();
        let list = display(feed.source(), 0.0);
        build_frame(
            &mut ground,
            &list,
            1,
            (40, 40),
            &PixelFont,
            &RawImages::default(),
            &mut atlas,
            &mut batches,
        )
        .unwrap();
        atlas.live.get_mut(&feed.key().raw()).unwrap().stale = true;
        assert!(
            backend::FrameObservation::begin(&source.sender(), &list)
                .unwrap()
                .commit(&atlas, &batches)
                .is_none()
        );
        let offscreen = display(feed.source(), 100.0);
        build_frame(
            &mut ground,
            &offscreen,
            1,
            (40, 40),
            &PixelFont,
            &RawImages::default(),
            &mut atlas,
            &mut batches,
        )
        .unwrap();
        atlas.live.get_mut(&feed.key().raw()).unwrap().stale = false;
        assert!(
            backend::FrameObservation::begin(&source.sender(), &offscreen)
                .unwrap()
                .commit(&atlas, &batches)
                .is_none()
        );
        assert_eq!(observer.snapshot().presented, 0);
        assert_eq!(observer.snapshot().unavailable, Some(Unavailable::NotDrawn));
    }

    #[derive(Default)]
    struct FailingUpload {
        inner: RecordingGround,
        fail: bool,
    }
    impl bunny_ui_core::gpu::walk::AtlasGround for FailingUpload {
        fn ensure_shared(&mut self, size: u32) -> bool {
            self.inner.ensure_shared(size)
        }
        fn upload_shared(&mut self, x: u32, y: u32, w: u32, h: u32, bytes: &[u8], pitch: u32) {
            self.inner.upload_shared(x, y, w, h, bytes, pitch);
        }
        fn drop_shared(&mut self) {
            self.inner.drop_shared();
        }
        fn make_dedicated(&mut self, w: u32, h: u32, bytes: &[u8], pitch: u32) -> Option<u64> {
            self.inner.make_dedicated(w, h, bytes, pitch)
        }
        fn drop_dedicated(&mut self, id: u64) {
            self.inner.drop_dedicated(id);
        }
        fn make_live(
            &mut self,
            w: u32,
            h: u32,
            format: bunny_ui_core::image_engine::PixelFormat,
        ) -> Option<u64> {
            self.inner.make_live(w, h, format)
        }
        fn update_live(&mut self, id: u64, w: u32, h: u32, bytes: &[u8], pitch: u32) -> bool {
            !self.fail && self.inner.update_live(id, w, h, bytes, pitch)
        }
        fn drop_live(&mut self, id: u64) {
            self.inner.drop_live(id);
        }
    }

    #[test]
    fn a_failed_real_upload_keeps_old_pixels_without_new_generation_credit() {
        let feed = ImageFeed::new();
        feed.push((2, 2), vec![255; 16]);
        let (observer, source) = backend::channel(feed.key(), 16);
        let mut atlas = RunAtlas::new();
        let mut batches = FrameBatches::default();
        let mut ground = FailingUpload::default();
        let list = display(feed.source(), 0.0);
        build_frame(
            &mut ground,
            &list,
            1,
            (40, 40),
            &PixelFont,
            &RawImages::default(),
            &mut atlas,
            &mut batches,
        )
        .unwrap();
        feed.push((2, 2), vec![128; 16]);
        let newer = display(feed.source(), 0.0);
        ground.fail = true;
        build_frame(
            &mut ground,
            &newer,
            1,
            (40, 40),
            &PixelFont,
            &RawImages::default(),
            &mut atlas,
            &mut batches,
        )
        .unwrap();
        assert_eq!(atlas.live[&feed.key().raw()].generation, 1);
        assert!(
            backend::FrameObservation::begin(&source.sender(), &newer)
                .unwrap()
                .commit(&atlas, &batches)
                .is_none()
        );
        assert_eq!(observer.snapshot().presented, 0);
        ground.fail = false;
        build_frame(
            &mut ground,
            &newer,
            1,
            (40, 40),
            &PixelFont,
            &RawImages::default(),
            &mut atlas,
            &mut batches,
        )
        .unwrap();
        let ticket = backend::FrameObservation::begin(&source.sender(), &newer)
            .unwrap()
            .commit(&atlas, &batches)
            .unwrap();
        ticket.rendered(true);
        ticket.presented(2.0, 2);
        assert_eq!(observer.snapshot().presented, 1);
    }

    #[test]
    fn transformed_feeds_are_unavailable_without_retaining_image_bytes() {
        use std::rc::Rc;
        let feed = ImageFeed::new();
        let bytes: Rc<[u8]> = vec![255; 16].into();
        feed.push((2, 2), Rc::clone(&bytes));
        let baseline = Rc::strong_count(&bytes);
        let (observer, source) = backend::channel(feed.key(), 16);
        for image in [
            ImageSource::Faded {
                key: 4,
                inner: Rc::new(feed.source()),
                alpha: 128,
            },
            ImageSource::Mirrored {
                key: 5,
                inner: Rc::new(feed.source()),
            },
        ] {
            let list = display(image, 0.0);
            assert!(backend::FrameObservation::begin(&source.sender(), &list).is_none());
        }
        assert_eq!(Rc::strong_count(&bytes), baseline);
        assert_eq!(
            observer.snapshot().unavailable,
            Some(Unavailable::UnsupportedSnapshot)
        );
    }

    #[test]
    fn competing_snapshots_of_a_texture_are_explicitly_unsupported() {
        let feed = ImageFeed::new();
        feed.push((2, 2), vec![255; 16]);
        let old = feed.source();
        feed.push((2, 2), vec![128; 16]);
        let list: DisplayList = vec![
            DrawCommand::Image {
                rect: rect(0.0),
                source: old,
            },
            DrawCommand::Image {
                rect: rect(20.0),
                source: feed.source(),
            },
        ]
        .into();
        let (observer, source) = backend::channel(feed.key(), 16);
        assert!(backend::FrameObservation::begin(&source.sender(), &list).is_none());
        assert_eq!(
            observer.snapshot().unavailable,
            Some(Unavailable::UnsupportedSnapshot)
        );
    }
}

#[test]
fn presentation_waits_for_successful_render_completion_in_either_order() {
    for success in [false, true] {
        let (observer, source) = channel(16);
        let sender = source.sender();
        let _ = sender.submitted(1);
        let ticket = sender.commit(1).unwrap();
        ticket.presented(1.0, 1);
        assert_eq!(observer.snapshot().presented, 0);
        ticket.rendered(success);
        assert_eq!(observer.snapshot().presented, u64::from(success));
        assert_eq!(observer.snapshot().pending, 0);
    }
}

#[test]
fn superseded_event_distinguishes_uncommitted_replacement() {
    let (observer, source) = channel(16);
    let sender = source.sender();
    let _ = sender.submitted(1);
    let _ = sender.submitted(2);
    assert!(
        observer
            .drain()
            .contains(&PresentationEvent::Superseded { generation: 1 })
    );
}
