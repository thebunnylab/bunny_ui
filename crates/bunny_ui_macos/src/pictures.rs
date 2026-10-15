//! The headless desk for the preview host's platform views — the macOS
//! door behind `bunny_ui_preview::host::Pictures`.
//!
//! Under `bunny preview` and Studio the app runs with no window: the
//! host lays its scenes out headlessly and sends display lists, and a
//! box the platform would composite a view over — a webview — rides
//! the frame as a placeholder. A `WKWebView` renders only on a window,
//! so the desk opens ONE nobody sees: borderless, off every screen,
//! transient to Mission Control, deaf to the mouse, under an
//! `NSApplication` with the accessory policy (no Dock tile, no menu
//! bar, never activated). Each hosted page mounts on it at its box's
//! size. The page's picture is the engine's own snapshot
//! ([`webview::snapshot`]): taken when the load finishes, then once a
//! second while the box lives, and kept only when the bytes moved. It
//! reaches the host as an [`ImageSource::Rgba`], spliced into the
//! frame where the box stood.
//!
//! ## Wiring
//!
//! The facade's preview host opens a [`Desk`] on the Mac and hands it to
//! `bunny_ui_preview::host::serve_with` behind the `Pictures` trait:
//! [`Desk::picture`] mounts on the first ask and answers per placement,
//! [`Desk::turn`] runs the main run loop one short pass so WebKit's
//! callbacks land and says whether a picture moved, [`Desk::busy`]
//! keeps the host's turns short while a page loads or a shot is in
//! flight, and [`Desk::sweep`] releases the pages whose boxes left.
//!
//! ## Production gotchas
//!
//! - The desk lives on the MAIN thread, like every AppKit object: the
//!   preview host serves there (`bunny_ui::app!` calls it from `main`).
//! - A window off every screen is not "visible" to the window server,
//!   and WebKit reads that as a hidden page: its timers and animations
//!   throttle as a background tab's do. The snapshot does not mind — it
//!   is the web process painting its own frame on request — so a page
//!   pictures; one that only draws on `requestAnimationFrame` pictures
//!   its first frame.
//! - The picture's pixels are at the window's backing scale (two per
//!   point on a Retina Mac); the host draws them into the box's points,
//!   so the tool sees the page sharp at any zoom.
//! - A video box stays the placeholder: this shell refuses a `Video`
//!   host in its own window too, and there is nothing to picture until
//!   a video backend exists on the Mac.

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use bunny_ui::host::HostSpec;
use bunny_ui::image_engine::ImageSource;
use bunny_ui::layout::HostPlacement;

use crate::ffi::{CGPoint, CGRect, CGSize, Id, Sel, class, sel};
use crate::webview::{self, WebviewEvent};

// The msgSend casts in the house pattern, local to the messages this
// module sends.
#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    #[link_name = "objc_msgSend"]
    fn msg_id(obj: Id, sel: Sel) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_void(obj: Id, sel: Sel);
    #[link_name = "objc_msgSend"]
    fn msg_void_id(obj: Id, sel: Sel, a: Id);
    #[link_name = "objc_msgSend"]
    fn msg_void_bool(obj: Id, sel: Sel, a: i8);
    #[link_name = "objc_msgSend"]
    fn msg_void_u64(obj: Id, sel: Sel, a: u64);
    #[link_name = "objc_msgSend"]
    fn msg_void_rect(obj: Id, sel: Sel, rect: CGRect);
    #[link_name = "objc_msgSend"]
    fn msg_void_size(obj: Id, sel: Sel, size: CGSize);
    #[link_name = "objc_msgSend"]
    fn msg_bool_i64(obj: Id, sel: Sel, a: i64) -> i8;
    #[link_name = "objc_msgSend"]
    fn msg_init_window(obj: Id, sel: Sel, rect: CGRect, style: u64, backing: u64, defer: i8)
    -> Id;
}

// The run loop the desk turns by hand: WebKit answers on it, and the
// preview host has no AppKit loop running for it.
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRunLoopRunInMode(mode: Id, seconds: f64, return_after_source_handled: i8) -> i32;
    static kCFRunLoopDefaultMode: Id;
}

/// `NSApplicationActivationPolicyAccessory`: no Dock tile, no menu bar,
/// windows allowed — never activated, since nothing here asks.
const ACCESSORY: i64 = 1;
/// Where the window stands: far outside any screen's coordinates.
const OFFSTAGE: f64 = -20_000.0;
/// `NSWindowCollectionBehavior`: joins every space, transient (hidden
/// from Mission Control and Exposé), stationary, out of the window cycle.
const UNSEEN: u64 = (1 << 0) | (1 << 3) | (1 << 4) | (1 << 6);
/// `NSBackingStoreBuffered`.
const BUFFERED: u64 = 2;
/// How long the run loop is turned per [`Desk::turn`] — long enough to
/// hand over every callback that is ready, short enough to vanish in a
/// frame.
const PASS: Duration = Duration::from_millis(1);
/// A page committed: a picture a beat later, in case the load event
/// never comes (a page that keeps a request open never finishes).
const SETTLE: Duration = Duration::from_millis(150);
/// A pictured page pictures again this much later, while its box lives.
const CADENCE: Duration = Duration::from_secs(1);
/// A load that neither committed nor failed in this long stops holding
/// the host's snapshots: the box stays the placeholder it is.
const LOAD_PATIENCE: Duration = Duration::from_secs(10);
/// A shot the engine never answered is given up on, and asked again.
const SHOT_PATIENCE: Duration = Duration::from_secs(5);

/// The offscreen window and the pages on it. One per preview host,
/// opened on the main thread.
pub struct Desk {
    /// The window nobody sees — its content view holds every page.
    content: Id,
    pages: HashMap<String, Page>,
    /// What the pages reported since the last turn, through the shell's
    /// one dispatch slot.
    inbox: Rc<RefCell<Vec<WebviewEvent>>>,
    /// The next shot's token.
    tokens: u64,
}

/// One mounted page.
struct Page {
    view: Id,
    /// The spec's fingerprint the page was last instructed with.
    stamp: String,
    size: (f64, f64),
    /// The load in flight since: cleared when it finished or failed,
    /// or when patience ran out.
    loading: Option<Instant>,
    /// When the next shot is wanted.
    due: Option<Instant>,
    /// The shot in flight: its token and when it was asked.
    shot: Option<(u64, Instant)>,
    /// How many shots this page kept — the picture's key counts them.
    kept: u64,
    picture: Option<ImageSource>,
}

/// The page's key: the scene and the box's path in it, joined by a
/// character no path spells. It is also the path the shell's webview
/// files the page under, so a report holding the view finds it.
fn key(scene: &str, path: &str) -> String {
    format!("{scene}\u{1}{path}")
}

fn rect(size: (f64, f64)) -> CGRect {
    CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: size.0, height: size.1 } }
}

impl Desk {
    /// Opens the desk: the shared application under the accessory
    /// policy, the window off every screen, the dispatch pointed at
    /// the desk's inbox.
    pub fn open() -> Desk {
        let inbox: Rc<RefCell<Vec<WebviewEvent>>> = Rc::new(RefCell::new(Vec::new()));
        let content = unsafe {
            let app = msg_id(class("NSApplication"), sel("sharedApplication"));
            let _ = msg_bool_i64(app, sel("setActivationPolicy:"), ACCESSORY);
            let window = msg_id(class("NSWindow"), sel("alloc"));
            let window = msg_init_window(
                window,
                sel("initWithContentRect:styleMask:backing:defer:"),
                CGRect { origin: CGPoint { x: OFFSTAGE, y: OFFSTAGE }, size: CGSize { width: 1.0, height: 1.0 } },
                0, // borderless
                BUFFERED,
                0,
            );
            // the desk holds the window for the process's life; the
            // window never closes, and would not free itself if it did
            msg_void_bool(window, sel("setReleasedWhenClosed:"), 0);
            msg_void_bool(window, sel("setIgnoresMouseEvents:"), 1);
            msg_void_bool(window, sel("setHasShadow:"), 0);
            msg_void_bool(window, sel("setExcludedFromWindowsMenu:"), 1);
            msg_void_u64(window, sel("setCollectionBehavior:"), UNSEEN);
            // ordered in without activating anything: a webview on a
            // window that is not ordered in has no backing to paint
            msg_void(window, sel("orderFrontRegardless"));
            msg_id(window, sel("contentView"))
        };
        let landing = Rc::clone(&inbox);
        webview::set_dispatch(move |event| landing.borrow_mut().push(event));
        Desk { content, pages: HashMap::new(), inbox, tokens: 0 }
    }

    /// The picture of the box at `placement` in `scene`, if the page
    /// has one yet. The first ask mounts the page; a changed spec
    /// re-instructs it; a changed frame resizes it — each with a fresh
    /// shot soon after. A box that is not a webview has no picture.
    pub fn picture(&mut self, scene: &str, placement: &HostPlacement) -> Option<ImageSource> {
        let HostSpec::Webview { .. } = &placement.spec else { return None };
        let key = key(scene, &placement.path);
        let size = (placement.frame.size.width.max(1.0), placement.frame.size.height.max(1.0));
        let stamp = webview::stamp(&placement.spec);
        let mut refit = false;
        let page = match self.pages.entry(key.clone()) {
            Entry::Occupied(slot) => slot.into_mut(),
            Entry::Vacant(slot) => {
                let view = webview::create(&key, &placement.spec);
                if view.is_null() {
                    return None;
                }
                unsafe {
                    msg_void_rect(view, sel("setFrame:"), rect(size));
                    msg_void_id(self.content, sel("addSubview:"), view);
                }
                refit = true;
                slot.insert(Page {
                    view,
                    stamp: stamp.clone(),
                    size,
                    loading: Some(Instant::now()),
                    due: None,
                    shot: None,
                    kept: 0,
                    picture: None,
                })
            }
        };
        if page.stamp != stamp {
            page.stamp = stamp;
            webview::update(&key, page.view, &placement.spec);
            // a new page may be loading; the commit and the finish say
            // so, and a settle covers an instruction that loads nothing
            page.due = Some(Instant::now() + SETTLE);
        }
        if page.size != size {
            page.size = size;
            unsafe { msg_void_rect(page.view, sel("setFrame:"), rect(size)) };
            refit = true;
            // the page relays out to its new box: a picture of that
            page.due = Some(page.due.map_or(Instant::now() + SETTLE, |due| due.min(Instant::now() + SETTLE)));
        }
        let picture = page.picture.clone();
        if refit {
            self.fit();
        }
        picture
    }

    /// The window grows to hold every page: each sits at the origin,
    /// and the snapshot is the page's own, so they may overlap.
    fn fit(&self) {
        let (width, height) = self.pages.values().fold((1.0f64, 1.0f64), |(w, h), page| (w.max(page.size.0), h.max(page.size.1)));
        unsafe {
            let window = msg_id(self.content, sel("window"));
            if !window.is_null() {
                msg_void_size(window, sel("setContentSize:"), CGSize { width, height });
            }
        }
    }

    /// One turn: the main run loop runs a short pass so WebKit's
    /// callbacks land, the reports are folded, the shots that are due
    /// are asked. Answers whether a picture moved — a page pictured for
    /// the first time, or pictured differently.
    pub fn turn(&mut self) -> bool {
        unsafe {
            let _ = CFRunLoopRunInMode(kCFRunLoopDefaultMode, PASS.as_secs_f64(), 0);
        }
        let now = Instant::now();
        let mut moved = false;
        let reports: Vec<WebviewEvent> = self.inbox.borrow_mut().drain(..).collect();
        for report in reports {
            match report {
                WebviewEvent::Navigated { path, .. } => {
                    if let Some(page) = self.pages.get_mut(&path) {
                        page.due = Some(page.due.map_or(now + SETTLE, |due| due.min(now + SETTLE)));
                    }
                }
                WebviewEvent::Loaded { path, .. } => {
                    if let Some(page) = self.pages.get_mut(&path) {
                        page.loading = None;
                        page.due = Some(now);
                    }
                }
                WebviewEvent::NavigationFailed { path, .. } => {
                    if let Some(page) = self.pages.get_mut(&path) {
                        page.loading = None;
                    }
                }
                WebviewEvent::SnapshotDone { token, result } => {
                    let Some(page) = self.pages.values_mut().find(|page| page.shot.is_some_and(|(asked, _)| asked == token)) else {
                        continue;
                    };
                    page.shot = None;
                    if let Ok((width, height, rgba)) = result
                        && width > 0
                        && height > 0
                        && !page.pictures(&rgba)
                    {
                        page.kept += 1;
                        let mut hasher = std::hash::DefaultHasher::new();
                        // the key names the page (its view) and the shot: a new
                        // picture is a new resource on the wire, an old one stays cached
                        ("bunny-ui-macos pictures", page.view as usize, page.kept).hash(&mut hasher);
                        page.picture = Some(ImageSource::Rgba { key: hasher.finish(), size: (width as u32, height as u32), rgba: Rc::from(rgba) });
                        moved = true;
                    }
                    // the same bytes, or an engine that refused: the
                    // picture that stands stays, and the cadence goes on
                    page.due = Some(page.due.map_or(now + CADENCE, |due| due.min(now + CADENCE)));
                }
                WebviewEvent::Linked { .. }
                | WebviewEvent::Changed { .. }
                | WebviewEvent::Pasted { .. }
                | WebviewEvent::Posted { .. }
                | WebviewEvent::Console { .. }
                | WebviewEvent::Requested { .. }
                | WebviewEvent::EvalDone { .. } => {}
            }
        }
        for page in self.pages.values_mut() {
            if page.loading.is_some_and(|since| now.duration_since(since) > LOAD_PATIENCE) {
                page.loading = None;
            }
            if page.shot.is_some_and(|(_, asked)| now.duration_since(asked) > SHOT_PATIENCE) {
                page.shot = None;
                page.due = Some(now + CADENCE);
            }
            if page.shot.is_none() && page.due.is_some_and(|due| due <= now) {
                self.tokens += 1;
                page.due = None;
                page.shot = Some((self.tokens, now));
                webview::snapshot(page.view, self.tokens);
            }
        }
        moved
    }

    /// Whether a page is loading, a shot is in flight, or one is due now
    /// — the host keeps its turns short, and holds a snapshot of the
    /// frame for the picture.
    pub fn busy(&self) -> bool {
        let now = Instant::now();
        self.pages.values().any(|page| page.loading.is_some() || page.shot.is_some() || page.due.is_some_and(|due| due <= now))
    }

    /// Releases every page `alive` does not name by its scene and path —
    /// the view leaves the window, the two holds go (the shell's
    /// `create` and the window's own), the shell's tables forget it.
    pub fn sweep(&mut self, alive: impl Fn(&str, &str) -> bool) {
        let mut refit = false;
        self.pages.retain(|key, page| {
            let (scene, path) = key.split_once('\u{1}').unwrap_or((key, ""));
            if alive(scene, path) {
                return true;
            }
            unsafe {
                msg_void(page.view, sel("removeFromSuperview"));
                webview::retire(key, page.view);
                msg_void(page.view, sel("release"));
            }
            refit = true;
            false
        });
        if refit {
            let kept: Vec<String> = self.pages.keys().cloned().collect();
            webview::sweep(&kept);
            self.fit();
        }
    }

    /// How many pages the desk holds.
    pub fn pages(&self) -> usize {
        self.pages.len()
    }
}

impl Page {
    /// Whether the page already pictures these bytes.
    fn pictures(&self, rgba: &[u8]) -> bool {
        match &self.picture {
            Some(ImageSource::Rgba { rgba: standing, .. }) => standing.as_ref() == rgba,
            _ => false,
        }
    }
}
