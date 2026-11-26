//! The test host: what a shell does with a block's answers, painted with
//! the framework's own painter — and the native render it must equal.
//!
//! A Trinity shell draws the scene with its own renderer; this host
//! draws it with the CPU raster every bunny-ui shell keeps as its
//! oracle, so a test can hold the scene that crossed the protocol to the
//! pixels the same view paints natively ([`native`]).

use std::collections::HashMap;
use std::sync::mpsc;

use bunny_ui_core::image_engine::{ImageSource, RawImages};
use bunny_ui_core::layout::{DisplayList, Size};
use bunny_ui_core::prelude::Runtime;
use bunny_ui_core::text_engine::PixelFont;
use bunny_ui_core::view::View;
use bunny_ui_preview::convert::{self, Missing};
use bunny_ui_preview::wire::Frame;

use crate::guest::{apply, background, pixel_scale, pointer};
use crate::protocol::{
    Capabilities, Fetch, Malformed, Message, Reader, Received, Requested, SemanticNode, ToGuest, ToHost, Versions,
    Viewport,
};

/// What a handshake agreed, as the host heard it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub version: u32,
    pub capabilities: Capabilities,
    pub engine: String,
}

/// A host that holds what a block told it.
#[derive(Default)]
pub struct TestHost {
    session: Option<Session>,
    refusal: Option<(Versions, String)>,
    scene: Option<Frame>,
    images: HashMap<u64, ImageSource>,
    semantics: Vec<SemanticNode>,
    animating: bool,
    faults: Vec<String>,
    requests: Vec<Requested>,
    fetches: Vec<Fetch>,
}

impl TestHost {
    pub fn new() -> Self {
        TestHost::default()
    }

    /// A block's answer, read whole.
    pub fn receive(&mut self, bytes: &[u8]) -> Result<(), Malformed> {
        let mut reader = Reader::new();
        reader.feed(bytes);
        while let Some(received) = reader.pull()? {
            let Received::Message(Message::ToHost(message)) = received else { continue };
            match message {
                ToHost::Accept { version, capabilities, engine } => {
                    self.session = Some(Session { version, capabilities, engine });
                    self.refusal = None;
                }
                ToHost::Refuse { versions, reason } => {
                    self.session = None;
                    self.refusal = Some((versions, reason));
                }
                ToHost::Scene(frame) => self.scene = Some(frame),
                ToHost::Resource(resource) => {
                    if let Some(source) = convert::image_source(&resource) {
                        self.images.insert(resource.key, source);
                    }
                }
                ToHost::Semantics { nodes, .. } => self.semantics = nodes,
                ToHost::Pace { animating } => self.animating = animating,
                ToHost::Fault { reason } => self.faults.push(reason),
                ToHost::Request(requested) => self.requests.push(requested),
                ToHost::Fetch(fetch) => self.fetches.push(fetch),
            }
        }
        if reader.pending() > 0 {
            return Err(Malformed::Truncated { kind: 0 });
        }
        Ok(())
    }

    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    /// The block's own range and its reason, when it refused the offer.
    pub fn refusal(&self) -> Option<&(Versions, String)> {
        self.refusal.as_ref()
    }

    /// The last scene.
    pub fn scene(&self) -> Option<&Frame> {
        self.scene.as_ref()
    }

    pub fn semantics(&self) -> &[SemanticNode] {
        &self.semantics
    }

    pub fn animating(&self) -> bool {
        self.animating
    }

    pub fn faults(&self) -> &[String] {
        &self.faults
    }

    /// Every action the block asked for, in the order it asked.
    pub fn requests(&self) -> &[Requested] {
        &self.requests
    }

    /// Every fetch the block asked for, in the order it asked — performed
    /// by nobody: a test host opens no network.
    pub fn fetches(&self) -> &[Fetch] {
        &self.fetches
    }

    /// The last scene as the framework's display list.
    pub fn display_list(&self) -> Result<DisplayList, Missing> {
        let Some(frame) = &self.scene else { return Ok(DisplayList::default()) };
        convert::raise(&frame.commands, &frame.strings, &self.images)
    }

    /// The last scene's pixels, straight RGBA at its scale — painted with
    /// the face the block measured with.
    pub fn paint(&self) -> Result<Vec<u8>, Missing> {
        let Some(frame) = &self.scene else { return Ok(Vec::new()) };
        let display = self.display_list()?;
        let scale = pixel_scale(frame.scale);
        let bitmap = bunny_ui_core::raster::rasterize_with(
            &display,
            (frame.size.width.round() as usize) * scale,
            (frame.size.height.round() as usize) * scale,
            scale,
            frame.background.into(),
            &PixelFont,
            &RawImages::default(),
        );
        Ok(bitmap.to_rgba_bytes())
    }
}

/// The view `make` builds, rendered the way a native shell renders it —
/// one runtime, its own frame, no protocol — under `viewport`: the
/// pixels before `events`, then after them. Straight RGBA at the
/// viewport's scale.
///
/// It runs on a thread of its own, because a runtime owns the thread's
/// world. It replays pointer, wheel, resize and tick events, the way a
/// shell hands them over; any other event panics, so a test cannot
/// believe it replayed what it did not.
pub fn native<V: View + 'static>(make: fn() -> V, viewport: Viewport, events: &[ToGuest]) -> [Vec<u8>; 2] {
    let events = events.to_vec();
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let runtime = Runtime::new();
        let root = make();
        let mut viewport = viewport;
        apply(&runtime, viewport);
        let paint = |runtime: &Runtime, viewport: Viewport| {
            let size = Size { width: viewport.width, height: viewport.height };
            runtime.frame(&root, size, pixel_scale(viewport.scale), background(runtime)).to_rgba_bytes()
        };
        let before = paint(&runtime, viewport);
        for event in events {
            match event {
                ToGuest::Pointer { phase, x, y, button, clicks, modifiers } => {
                    let _ = pointer(&runtime, phase, x, y, button, clicks, modifiers);
                }
                ToGuest::Wheel { x, y, dx, dy, .. } => {
                    let _ = runtime.wheel(x, y, dx, dy);
                }
                ToGuest::Resize(next) => {
                    viewport = next;
                    apply(&runtime, viewport);
                }
                ToGuest::Tick { dt_ms } => {
                    let _ = runtime.tick(dt_ms / 1000.0);
                }
                other => panic!("the native render does not replay {other:?}"),
            }
        }
        let after = paint(&runtime, viewport);
        let _ = send.send([before, after]);
    });
    receive.recv().expect("the native render panicked")
}
