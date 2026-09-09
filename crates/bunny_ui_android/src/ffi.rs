//! The Android shell's FFI border: `android.app.NativeActivity` spoken
//! by hand — the activity's callback table, the main looper our clocks
//! and the input queue ride, the window a frame presents into (Vulkan
//! through the shared tier, or the CPU floor through
//! `ANativeWindow_lock`), the configuration and the soft keyboard. Not
//! a single dependency: `libandroid` and bionic through `extern "C"`.
//!
//! Everything runs on the UI thread. The system calls the activity's
//! callbacks there, the main looper lives there, and the runtime is
//! single-threaded by design — so there is no glue thread, no command
//! pipe and no acknowledgement to wait on. A window that is going away
//! is let go INSIDE its callback, and a frame the system asks for is
//! drawn before the callback returns, by construction.

use std::cell::{Cell, RefCell};
use std::ffi::{c_char, c_int, c_void};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Instant;

use bunny_ui::image_engine::ImageEngine;
use bunny_ui::layout::{Color, DisplayList, Size};
use bunny_ui::text_engine::TextEngine;
use bunny_ui_vulkan::{Presented, SurfaceSource, VkPresenter};

use crate::keys::{self, KeyStroke, TapCounter};

// MARK: - The ABI (native_activity.h, looper.h, input.h, native_window.h,
// configuration.h, choreographer.h)

macro_rules! opaque {
    ($($name:ident),*) => {
        $(
            #[repr(C)]
            pub struct $name {
                _private: [u8; 0],
            }
        )*
    };
}

opaque!(ANativeWindow, AInputQueue, AInputEvent, ALooper, AChoreographer, AConfiguration);

/// The activity the system hands the entry point — field for field
/// the header's struct. `clazz` is the ACTIVITY INSTANCE, whatever its
/// name says; `env` is the UI thread's `JNIEnv`.
#[repr(C)]
pub struct ANativeActivity {
    pub callbacks: *mut ANativeActivityCallbacks,
    pub vm: *mut c_void,
    pub env: *mut c_void,
    pub clazz: *mut c_void,
    pub internal_data_path: *const c_char,
    pub external_data_path: *const c_char,
    pub sdk_version: i32,
    pub instance: *mut c_void,
    pub asset_manager: *mut c_void,
    pub obb_path: *const c_char,
}

type ActivityFn = unsafe extern "C" fn(*mut ANativeActivity);
type WindowFn = unsafe extern "C" fn(*mut ANativeActivity, *mut ANativeWindow);
type QueueFn = unsafe extern "C" fn(*mut ANativeActivity, *mut AInputQueue);

/// The sixteen callbacks, in the header's order.
#[repr(C)]
pub struct ANativeActivityCallbacks {
    on_start: Option<ActivityFn>,
    on_resume: Option<ActivityFn>,
    on_save_instance_state: Option<unsafe extern "C" fn(*mut ANativeActivity, *mut usize) -> *mut c_void>,
    on_pause: Option<ActivityFn>,
    on_stop: Option<ActivityFn>,
    on_destroy: Option<ActivityFn>,
    on_window_focus_changed: Option<unsafe extern "C" fn(*mut ANativeActivity, c_int)>,
    on_native_window_created: Option<WindowFn>,
    on_native_window_resized: Option<WindowFn>,
    on_native_window_redraw_needed: Option<WindowFn>,
    on_native_window_destroyed: Option<WindowFn>,
    on_input_queue_created: Option<QueueFn>,
    on_input_queue_destroyed: Option<QueueFn>,
    on_content_rect_changed: Option<unsafe extern "C" fn(*mut ANativeActivity, *const ARect)>,
    on_configuration_changed: Option<ActivityFn>,
    on_low_memory: Option<ActivityFn>,
}

#[repr(C)]
pub struct ARect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// The buffer `ANativeWindow_lock` hands out; `stride` is in PIXELS.
#[repr(C)]
struct ANativeWindowBuffer {
    width: i32,
    height: i32,
    stride: i32,
    format: i32,
    bits: *mut c_void,
    reserved: [u32; 6],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Timespec {
    sec: i64,
    nsec: i64,
}

#[repr(C)]
struct Itimerspec {
    interval: Timespec,
    value: Timespec,
}

type LooperCallback = unsafe extern "C" fn(c_int, c_int, *mut c_void) -> c_int;

#[link(name = "android")]
unsafe extern "C" {
    fn ANativeActivity_showSoftInput(activity: *mut ANativeActivity, flags: u32);
    fn ANativeActivity_hideSoftInput(activity: *mut ANativeActivity, flags: u32);
    fn ANativeWindow_acquire(window: *mut ANativeWindow);
    fn ANativeWindow_release(window: *mut ANativeWindow);
    fn ANativeWindow_getWidth(window: *mut ANativeWindow) -> i32;
    fn ANativeWindow_getHeight(window: *mut ANativeWindow) -> i32;
    fn ANativeWindow_setBuffersGeometry(
        window: *mut ANativeWindow,
        width: i32,
        height: i32,
        format: i32,
    ) -> i32;
    fn ANativeWindow_lock(
        window: *mut ANativeWindow,
        buffer: *mut ANativeWindowBuffer,
        dirty: *mut ARect,
    ) -> i32;
    fn ANativeWindow_unlockAndPost(window: *mut ANativeWindow) -> i32;
    fn ALooper_forThread() -> *mut ALooper;
    fn ALooper_acquire(looper: *mut ALooper);
    fn ALooper_release(looper: *mut ALooper);
    fn ALooper_addFd(
        looper: *mut ALooper,
        fd: c_int,
        ident: c_int,
        events: c_int,
        callback: Option<LooperCallback>,
        data: *mut c_void,
    ) -> c_int;
    fn ALooper_removeFd(looper: *mut ALooper, fd: c_int) -> c_int;
    fn AInputQueue_attachLooper(
        queue: *mut AInputQueue,
        looper: *mut ALooper,
        ident: c_int,
        callback: Option<LooperCallback>,
        data: *mut c_void,
    );
    fn AInputQueue_detachLooper(queue: *mut AInputQueue);
    fn AInputQueue_getEvent(queue: *mut AInputQueue, event: *mut *mut AInputEvent) -> i32;
    fn AInputQueue_preDispatchEvent(queue: *mut AInputQueue, event: *mut AInputEvent) -> i32;
    fn AInputQueue_finishEvent(queue: *mut AInputQueue, event: *mut AInputEvent, handled: c_int);
    fn AInputEvent_getType(event: *const AInputEvent) -> i32;
    fn AInputEvent_getSource(event: *const AInputEvent) -> i32;
    fn AMotionEvent_getAction(event: *const AInputEvent) -> i32;
    fn AMotionEvent_getPointerCount(event: *const AInputEvent) -> usize;
    fn AMotionEvent_getPointerId(event: *const AInputEvent, index: usize) -> i32;
    fn AMotionEvent_getX(event: *const AInputEvent, index: usize) -> f32;
    fn AMotionEvent_getY(event: *const AInputEvent, index: usize) -> f32;
    fn AKeyEvent_getAction(event: *const AInputEvent) -> i32;
    fn AKeyEvent_getKeyCode(event: *const AInputEvent) -> i32;
    fn AKeyEvent_getMetaState(event: *const AInputEvent) -> i32;
    fn AChoreographer_getInstance() -> *mut AChoreographer;
    fn AChoreographer_postFrameCallback64(
        choreographer: *mut AChoreographer,
        callback: unsafe extern "C" fn(i64, *mut c_void),
        data: *mut c_void,
    );
    fn AConfiguration_new() -> *mut AConfiguration;
    fn AConfiguration_delete(config: *mut AConfiguration);
    fn AConfiguration_fromAssetManager(config: *mut AConfiguration, assets: *mut c_void);
    fn AConfiguration_getDensity(config: *mut AConfiguration) -> i32;
    fn AConfiguration_getUiModeNight(config: *mut AConfiguration) -> i32;
    fn AConfiguration_getScreenWidthDp(config: *mut AConfiguration) -> i32;
}

// bionic
unsafe extern "C" {
    fn pipe2(fds: *mut c_int, flags: c_int) -> c_int;
    fn read(fd: c_int, buffer: *mut c_void, count: usize) -> isize;
    fn write(fd: c_int, buffer: *const c_void, count: usize) -> isize;
    fn close(fd: c_int) -> c_int;
    fn timerfd_create(clock: c_int, flags: c_int) -> c_int;
    fn timerfd_settime(
        fd: c_int,
        flags: c_int,
        new: *const Itimerspec,
        old: *mut Itimerspec,
    ) -> c_int;
}

const O_CLOEXEC: c_int = 0o2000000;
const O_NONBLOCK: c_int = 0o4000;
const CLOCK_MONOTONIC: c_int = 1;
const ALOOPER_EVENT_INPUT: c_int = 1;
const IDENT_INPUT: c_int = 1;
const IDENT_CLOCK: c_int = 2;

// android/input.h
const AINPUT_EVENT_TYPE_KEY: i32 = 1;
const AINPUT_EVENT_TYPE_MOTION: i32 = 2;
const AINPUT_SOURCE_CLASS_POINTER: i32 = 0x2;
const AMOTION_EVENT_ACTION_MASK: i32 = 0xff;
const AMOTION_EVENT_ACTION_POINTER_INDEX_MASK: i32 = 0xff00;
const AMOTION_EVENT_ACTION_POINTER_INDEX_SHIFT: i32 = 8;
const AMOTION_EVENT_ACTION_DOWN: i32 = 0;
const AMOTION_EVENT_ACTION_UP: i32 = 1;
const AMOTION_EVENT_ACTION_MOVE: i32 = 2;
const AMOTION_EVENT_ACTION_CANCEL: i32 = 3;
const AMOTION_EVENT_ACTION_POINTER_DOWN: i32 = 5;
const AMOTION_EVENT_ACTION_POINTER_UP: i32 = 6;
const AKEY_EVENT_ACTION_DOWN: i32 = 0;
const AKEY_EVENT_ACTION_UP: i32 = 1;

// android/native_activity.h
const ANATIVEACTIVITY_SHOW_SOFT_INPUT_FORCED: u32 = 0x2;

// android/native_window.h
const WINDOW_FORMAT_RGBA_8888: i32 = 1;
const WINDOW_FORMAT_RGBX_8888: i32 = 2;

// android/configuration.h
const ACONFIGURATION_DENSITY_ANY: i32 = 0xfffe;
const ACONFIGURATION_DENSITY_NONE: i32 = 0xffff;
const ACONFIGURATION_UI_MODE_NIGHT_NO: i32 = 1;
const ACONFIGURATION_UI_MODE_NIGHT_YES: i32 = 2;

// MARK: - Events

/// What the platform delivers to the Rust world. Positions in LAYOUT
/// coordinates (origin at top-left, logical points): the window's
/// physical pixels divided by the scale, once, here.
#[derive(Clone, Debug)]
pub enum AppEvent {
    /// The window wants a frame — a resize, a redraw the system asked
    /// for, a rotation.
    Redraw,
    /// A task woke from somewhere else — a worker thread finished a
    /// step. The frame the shell already knows how to draw drains the
    /// queue on its way.
    Wake,
    /// The activity paused: nothing presents until it resumes.
    Background,
    /// The activity resumed.
    Foreground,
    /// The system wants memory back — drop what can be re-made.
    LowMemory,
    /// The configuration moved: dark or light (`None` = unspecified),
    /// whether the width is compact, and the scale.
    Config { dark: Option<bool>, compact: bool, scale: usize },
    /// The safe area moved — a rotation, a bar that came or went.
    SafeArea { top: f64, left: f64, bottom: f64, right: f64 },
    /// The soft keyboard covers this many points of the window's
    /// bottom (0 when hidden).
    Keyboard { overlap: f64 },
    /// The person sent the keyboard away (the back key, which the IME
    /// keeps): the field that wanted it lets go.
    KeyboardDismissed,
    TouchBegan { id: u64, x: f64, y: f64, taps: u8 },
    TouchMoved { id: u64, x: f64, y: f64 },
    TouchEnded { id: u64, x: f64, y: f64 },
    TouchCancelled { id: u64 },
    /// Text a key typed — the soft keyboard's and the hardware one's,
    /// one road. A return arrives as `"\n"`.
    Text(String),
    /// The keyboard's backspace.
    DeleteBackward,
    /// A key the keymap gate did not take, that types nothing — an
    /// arrow, a forward delete, the back key, a chord with control.
    Key(KeyStroke),
    /// The caret's blink half-period.
    Blink,
    /// One frame tick; `dt` seconds since the last, clamped.
    Frame { dt: f64 },
    /// The window went away (the background, a rotation): nothing
    /// presents until one comes back.
    WindowLost,
    /// A window stands again — present a frame.
    WindowGained,
}

/// Which road a frame takes to the window, decided once per mount.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Road {
    /// No window has been mounted yet.
    None,
    /// The shared Vulkan tier.
    Vulkan,
    /// The CPU floor: the raster copied through `ANativeWindow_lock`.
    Cpu,
}

/// How fast the shell drives frames — the ffi twin of the runtime's
/// pace, chosen after every present.
#[derive(Clone, Copy, PartialEq)]
pub enum DriverPace {
    /// The choreographer paces — springs, flights and fingers are moving.
    Full,
    /// Only loop clocks live: a timer beats once per step.
    Slow(f64),
    /// Nothing moves.
    Off,
}

thread_local! {
    static HANDLER: RefCell<Option<Box<dyn FnMut(AppEvent)>>> = const { RefCell::new(None) };
    /// The keymap gate — asked first about every key.
    static KEY_GATE: RefCell<Option<Box<dyn FnMut(&KeyStroke) -> bool>>> =
        const { RefCell::new(None) };
    /// A handler is running: anything raised from inside it queues.
    static DISPATCHING: Cell<bool> = const { Cell::new(false) };
    /// What was raised while a handler ran, in the order it was raised.
    static PENDING: RefCell<Vec<AppEvent>> = const { RefCell::new(Vec::new()) };
    /// The mount, waiting for the first window.
    static MOUNT: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    static ACTIVITY: Cell<*mut ANativeActivity> = const { Cell::new(null_mut()) };
    static LOOPER: Cell<*mut ALooper> = const { Cell::new(null_mut()) };
    static WINDOW: Cell<*mut ANativeWindow> = const { Cell::new(null_mut()) };
    static QUEUE: Cell<*mut AInputQueue> = const { Cell::new(null_mut()) };
    /// Device pixels per point — the density rounded to a whole number.
    static SCALE: Cell<usize> = const { Cell::new(1) };
    static ROAD: Cell<Road> = const { Cell::new(Road::None) };
    /// The window's presenter, while the Vulkan road is up.
    static PRESENTER: RefCell<Option<VkPresenter>> = const { RefCell::new(None) };
    /// The device is lost once already: the next loss steps down.
    static RECREATE_SPENT: Cell<bool> = const { Cell::new(false) };
    /// The CPU floor's retained surface, with the scale and canvas it
    /// was made for.
    static CPU: RefCell<Option<(bunny_ui::raster::Surface, usize, Color)>> =
        const { RefCell::new(None) };
    /// The activity is paused: presenting would draw into a window
    /// the system is taking away.
    static BACKGROUNDED: Cell<bool> = const { Cell::new(false) };
    /// Whether the soft keyboard is wanted — asked of the system only
    /// when this flips.
    static KEYBOARD_WANTED: Cell<bool> = const { Cell::new(false) };
    static CHOREOGRAPHER: Cell<*mut AChoreographer> = const { Cell::new(null_mut()) };
    /// A frame callback is posted and not yet fired.
    static FRAME_POSTED: Cell<bool> = const { Cell::new(false) };
    static LAST_FRAME_NANOS: Cell<Option<i64>> = const { Cell::new(None) };
    static PACE: Cell<DriverPace> = const { Cell::new(DriverPace::Off) };
    /// The caret's clock, the slow beat and the wake pipe's read end.
    static BLINK_FD: Cell<c_int> = const { Cell::new(-1) };
    static SLOW_FD: Cell<c_int> = const { Cell::new(-1) };
    static SLOW_INTERVAL: Cell<f64> = const { Cell::new(0.0) };
    static WAKE_READ_FD: Cell<c_int> = const { Cell::new(-1) };
    /// The insets settle late after the keyboard moves: a one-shot
    /// clock asks again, three times.
    static INSETS_FD: Cell<c_int> = const { Cell::new(-1) };
    static INSETS_RETRY: Cell<u8> = const { Cell::new(0) };
    /// The last good insets, in points: the safe area and the keyboard.
    static SAFE_LAST: Cell<(f64, f64, f64, f64)> = const { Cell::new((0.0, 0.0, 0.0, 0.0)) };
    static KEYBOARD_LAST: Cell<f64> = const { Cell::new(0.0) };
    /// The keyboard was seen up: a later "not visible" is a dismissal.
    static IME_SEEN: Cell<bool> = const { Cell::new(false) };
    static TAPS: RefCell<TapCounter> = const { RefCell::new(TapCounter::new()) };
    /// The handler took the key it was just handed.
    static KEY_TAKEN: Cell<bool> = const { Cell::new(false) };
    /// The back key's press was taken: its release is ours too, or the
    /// system would leave the activity on the release.
    static BACK_TAKEN: Cell<bool> = const { Cell::new(false) };
    /// The in-process clipboard — what was copied last, when the
    /// system's cannot be read (a window without the focus).
    static CLIPBOARD: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The wake pipe's write end — the one thing another thread touches.
static WAKE_WRITE_FD: AtomicI32 = AtomicI32::new(-1);

/// Registers who receives the events (the shell's loop).
pub fn set_handler(handler: Box<dyn FnMut(AppEvent)>) {
    HANDLER.with(|slot| *slot.borrow_mut() = Some(handler));
}

/// Registers the keymap gate: `true` = the stroke was spent.
pub fn set_key_gate(gate: Box<dyn FnMut(&KeyStroke) -> bool>) {
    KEY_GATE.with(|slot| *slot.borrow_mut() = Some(gate));
}

/// Delivers an event to the handler. An event raised from INSIDE a
/// handler waits its turn instead of re-entering one: the handler is
/// borrowed for as long as it runs, and this is an `extern "C"` frame
/// — a borrow panic here cannot unwind, so it would abort the process.
/// Queued, not dropped: the second event still arrives, after the
/// first finishes, in the order it was raised.
pub fn dispatch(event: AppEvent) {
    if DISPATCHING.with(Cell::get) {
        PENDING.with(|queue| queue.borrow_mut().push(event));
        return;
    }
    DISPATCHING.with(|flag| flag.set(true));
    HANDLER.with(|slot| {
        if let Some(handler) = slot.borrow_mut().as_mut() {
            handler(event);
        }
    });
    DISPATCHING.with(|flag| flag.set(false));
    loop {
        let next = PENDING.with(|queue| {
            let mut queue = queue.borrow_mut();
            if queue.is_empty() { None } else { Some(queue.remove(0)) }
        });
        let Some(event) = next else { break };
        dispatch(event);
    }
}

/// The handler says it consumed the key it was handed — the platform
/// hears `handled`, and a back key it took does not leave the activity.
pub fn take_key() {
    KEY_TAKEN.with(|slot| slot.set(true));
}

// MARK: - The entry: the activity is created

/// `ANativeActivity_onCreate`, on the UI thread: the callback table,
/// the looper, the clocks, then `boot` — which builds the app and
/// registers its window. The mount waits for the first window.
pub unsafe fn on_create(activity: *mut ANativeActivity, boot: fn()) {
    crate::log::install();
    unsafe {
        let callbacks = &mut *(*activity).callbacks;
        callbacks.on_start = Some(on_start);
        callbacks.on_resume = Some(on_resume);
        callbacks.on_save_instance_state = Some(on_save_instance_state);
        callbacks.on_pause = Some(on_pause);
        callbacks.on_stop = Some(on_stop);
        callbacks.on_destroy = Some(on_destroy);
        callbacks.on_window_focus_changed = Some(on_window_focus_changed);
        callbacks.on_native_window_created = Some(on_native_window_created);
        callbacks.on_native_window_resized = Some(on_native_window_resized);
        callbacks.on_native_window_redraw_needed = Some(on_native_window_redraw_needed);
        callbacks.on_native_window_destroyed = Some(on_native_window_destroyed);
        callbacks.on_input_queue_created = Some(on_input_queue_created);
        callbacks.on_input_queue_destroyed = Some(on_input_queue_destroyed);
        callbacks.on_content_rect_changed = Some(on_content_rect_changed);
        callbacks.on_configuration_changed = Some(on_configuration_changed);
        callbacks.on_low_memory = Some(on_low_memory);
    }
    // the process may host one activity after another: a fresh start
    ACTIVITY.with(|slot| slot.set(activity));
    unsafe { crate::jni::install((*activity).env, (*activity).clazz) };
    crate::jni::edge_to_edge();
    BACKGROUNDED.with(|slot| slot.set(false));
    KEYBOARD_WANTED.with(|slot| slot.set(false));
    RECREATE_SPENT.with(|slot| slot.set(false));
    ROAD.with(|slot| slot.set(Road::None));
    let looper = unsafe { ALooper_forThread() };
    if looper.is_null() {
        aerr!("no looper on the activity's thread — clocks and wakes are silent");
    } else {
        unsafe { ALooper_acquire(looper) };
        LOOPER.with(|slot| slot.set(looper));
        install_wake_pipe(looper);
        install_clocks(looper);
    }
    CHOREOGRAPHER.with(|slot| slot.set(unsafe { AChoreographer_getInstance() }));
    let (_, _, scale) = config();
    SCALE.with(|slot| slot.set(scale));
    if crate::log::trace() {
        let sdk = unsafe { (*activity).sdk_version };
        alog!("create: sdk {sdk}, scale {scale}");
        // the pump's own proof: this line must come back as a WARN
        eprintln!("bunny_ui android: stderr reaches logcat");
    }
    boot();
}

/// Keeps the mount for the first window — or runs it now, if a window
/// already stands.
pub fn set_mount(mount: Box<dyn FnOnce()>) {
    if WINDOW.with(Cell::get).is_null() {
        MOUNT.with(|slot| *slot.borrow_mut() = Some(mount));
        return;
    }
    mount();
    dispatch(AppEvent::WindowGained);
}

fn trace_line(what: &str) {
    if crate::log::trace() {
        alog!("{what}");
    }
}

// MARK: - The activity's callbacks (all on the UI thread)

unsafe extern "C" fn on_start(_activity: *mut ANativeActivity) {
    trace_line("start");
}

unsafe extern "C" fn on_resume(_activity: *mut ANativeActivity) {
    trace_line("resume");
    if BACKGROUNDED.with(|slot| slot.replace(false)) {
        bunny_ui::app::emit(bunny_ui::app::AppEvent::DidWake);
        dispatch(AppEvent::Foreground);
    }
}

unsafe extern "C" fn on_save_instance_state(
    _activity: *mut ANativeActivity,
    out_size: *mut usize,
) -> *mut c_void {
    // nothing is saved: the scene is rebuilt from the app's own state
    unsafe { *out_size = 0 };
    null_mut()
}

unsafe extern "C" fn on_pause(_activity: *mut ANativeActivity) {
    trace_line("pause");
    if !BACKGROUNDED.with(|slot| slot.replace(true)) {
        // parked before the callback returns: no frame may be posted
        // into a window the system is about to take away
        set_frame_driver(DriverPace::Off);
        dispatch(AppEvent::Background);
        bunny_ui::app::emit(bunny_ui::app::AppEvent::WillSleep);
    }
}

unsafe extern "C" fn on_stop(_activity: *mut ANativeActivity) {
    trace_line("stop");
}

unsafe extern "C" fn on_destroy(_activity: *mut ANativeActivity) {
    trace_line("destroy");
    set_frame_driver(DriverPace::Off);
    PRESENTER.with(|slot| drop(slot.borrow_mut().take()));
    CPU.with(|slot| drop(slot.borrow_mut().take()));
    let looper = LOOPER.with(|slot| slot.replace(null_mut()));
    let queue = QUEUE.with(|slot| slot.replace(null_mut()));
    if !queue.is_null() {
        unsafe { AInputQueue_detachLooper(queue) };
    }
    for slot in [&BLINK_FD, &SLOW_FD, &INSETS_FD, &WAKE_READ_FD] {
        let fd = slot.with(|slot| slot.replace(-1));
        if fd >= 0 {
            unsafe {
                if !looper.is_null() {
                    ALooper_removeFd(looper, fd);
                }
                close(fd);
            }
        }
    }
    let wake = WAKE_WRITE_FD.swap(-1, Ordering::AcqRel);
    if wake >= 0 {
        unsafe { close(wake) };
    }
    if !looper.is_null() {
        unsafe { ALooper_release(looper) };
    }
    HANDLER.with(|slot| drop(slot.borrow_mut().take()));
    KEY_GATE.with(|slot| drop(slot.borrow_mut().take()));
    ACTIVITY.with(|slot| slot.set(null_mut()));
}

unsafe extern "C" fn on_window_focus_changed(_activity: *mut ANativeActivity, has_focus: c_int) {
    if crate::log::trace() {
        alog!("focus {}", has_focus != 0);
    }
    refresh_insets();
}

unsafe extern "C" fn on_native_window_created(
    _activity: *mut ANativeActivity,
    window: *mut ANativeWindow,
) {
    unsafe { ANativeWindow_acquire(window) };
    WINDOW.with(|slot| slot.set(window));
    let (_, _, scale) = config();
    SCALE.with(|slot| slot.set(scale));
    let physical = window_physical();
    if crate::log::trace() {
        alog!("window created: {}×{} px, scale {scale}", physical.0, physical.1);
    }
    let mount = MOUNT.with(|slot| slot.borrow_mut().take());
    match mount {
        // the first window: the mount picks the road
        Some(mount) => mount(),
        // a window after a window: the road stands, the surface renews
        None => match ROAD.with(Cell::get) {
            Road::Vulkan => {
                let attached = PRESENTER.with(|slot| {
                    slot.borrow_mut().as_mut().is_some_and(|presenter| {
                        presenter.attach_surface(SurfaceSource::Android { window: window.cast() }, physical)
                    })
                });
                if !attached {
                    aerr!("the new window refused a Vulkan surface — the CPU floor takes over");
                    PRESENTER.with(|slot| drop(slot.borrow_mut().take()));
                    install_cpu();
                }
            }
            Road::Cpu => install_cpu(),
            Road::None => {}
        },
    }
    dispatch(AppEvent::WindowGained);
    refresh_insets();
}

unsafe extern "C" fn on_native_window_resized(
    _activity: *mut ANativeActivity,
    _window: *mut ANativeWindow,
) {
    let physical = window_physical();
    if crate::log::trace() {
        alog!("window resized: {}×{} px", physical.0, physical.1);
    }
    // the platform may never say OUT_OF_DATE for a window that changed
    // size under its swapchain — the shell says it
    PRESENTER.with(|slot| {
        if let Some(presenter) = slot.borrow_mut().as_mut() {
            presenter.resize(physical);
        }
    });
    CPU.with(|slot| drop(slot.borrow_mut().take()));
    dispatch(AppEvent::Redraw);
    refresh_insets();
}

unsafe extern "C" fn on_native_window_redraw_needed(
    _activity: *mut ANativeActivity,
    _window: *mut ANativeWindow,
) {
    // the system waits for a frame before it returns — and gets one,
    // because the handler runs here and now
    dispatch(AppEvent::Redraw);
}

unsafe extern "C" fn on_native_window_destroyed(
    _activity: *mut ANativeActivity,
    window: *mut ANativeWindow,
) {
    trace_line("window destroyed");
    // the surface and the swapchain die INSIDE the callback: after it
    // returns the window is no more
    set_frame_driver(DriverPace::Off);
    PRESENTER.with(|slot| {
        if let Some(presenter) = slot.borrow_mut().as_mut() {
            presenter.detach_surface();
        }
    });
    CPU.with(|slot| drop(slot.borrow_mut().take()));
    WINDOW.with(|slot| slot.set(null_mut()));
    unsafe { ANativeWindow_release(window) };
    dispatch(AppEvent::WindowLost);
}

unsafe extern "C" fn on_input_queue_created(
    _activity: *mut ANativeActivity,
    queue: *mut AInputQueue,
) {
    let looper = LOOPER.with(Cell::get);
    if looper.is_null() {
        return;
    }
    QUEUE.with(|slot| slot.set(queue));
    unsafe { AInputQueue_attachLooper(queue, looper, IDENT_INPUT, Some(on_input), null_mut()) };
}

unsafe extern "C" fn on_input_queue_destroyed(
    _activity: *mut ANativeActivity,
    queue: *mut AInputQueue,
) {
    unsafe { AInputQueue_detachLooper(queue) };
    QUEUE.with(|slot| slot.set(null_mut()));
}

unsafe extern "C" fn on_content_rect_changed(
    _activity: *mut ANativeActivity,
    rect: *const ARect,
) {
    if crate::log::trace() && !rect.is_null() {
        let rect = unsafe { &*rect };
        alog!("content rect: {} {} {} {}", rect.left, rect.top, rect.right, rect.bottom);
    }
    refresh_insets();
}

unsafe extern "C" fn on_configuration_changed(_activity: *mut ANativeActivity) {
    let (dark, compact, scale) = config();
    SCALE.with(|slot| slot.set(scale));
    dispatch(AppEvent::Config { dark, compact, scale });
    refresh_insets();
}

unsafe extern "C" fn on_low_memory(_activity: *mut ANativeActivity) {
    dispatch(AppEvent::LowMemory);
}

// MARK: - The clocks and the wake, on the main looper

/// A knock from another thread lands here, on the UI thread, as one
/// more beat.
unsafe extern "C" fn on_wake(fd: c_int, _events: c_int, _data: *mut c_void) -> c_int {
    let mut sink = [0u8; 64];
    while unsafe { read(fd, sink.as_mut_ptr().cast(), sink.len()) } > 0 {}
    dispatch(AppEvent::Wake);
    1
}

unsafe extern "C" fn on_blink(fd: c_int, _events: c_int, _data: *mut c_void) -> c_int {
    let mut count = 0u64;
    unsafe { read(fd, (&raw mut count).cast(), 8) };
    // the keyboard can leave on its own (the back key, which the input
    // method keeps) and no callback says so: while it is up, or was,
    // the slow clock asks the window
    if KEYBOARD_WANTED.with(Cell::get) || IME_SEEN.with(Cell::get) {
        refresh_insets();
    }
    dispatch(AppEvent::Blink);
    1
}

unsafe extern "C" fn on_slow(fd: c_int, _events: c_int, _data: *mut c_void) -> c_int {
    let mut count = 0u64;
    unsafe { read(fd, (&raw mut count).cast(), 8) };
    dispatch(AppEvent::Frame { dt: SLOW_INTERVAL.with(Cell::get) });
    1
}

/// The choreographer's beat: one frame at the display's pace, re-posted
/// while the pace stays full.
unsafe extern "C" fn on_frame(frame_time_nanos: i64, _data: *mut c_void) {
    FRAME_POSTED.with(|slot| slot.set(false));
    let dt = match LAST_FRAME_NANOS.with(|slot| slot.replace(Some(frame_time_nanos))) {
        // the first tick after a pause would report the whole pause as
        // the gap — a clamped step keeps springs continuous
        Some(last) => ((frame_time_nanos - last).max(0) as f64 / 1e9).clamp(0.0, 1.0 / 30.0),
        None => 1.0 / 60.0,
    };
    dispatch(AppEvent::Frame { dt });
    if PACE.with(Cell::get) == DriverPace::Full {
        post_frame();
    }
}

fn post_frame() {
    if FRAME_POSTED.with(Cell::get)
        || BACKGROUNDED.with(Cell::get)
        || WINDOW.with(Cell::get).is_null()
    {
        return;
    }
    let choreographer = CHOREOGRAPHER.with(Cell::get);
    if choreographer.is_null() {
        return;
    }
    unsafe { AChoreographer_postFrameCallback64(choreographer, on_frame, null_mut()) };
    FRAME_POSTED.with(|slot| slot.set(true));
}

fn install_wake_pipe(looper: *mut ALooper) {
    let mut fds = [0 as c_int; 2];
    if unsafe { pipe2(fds.as_mut_ptr(), O_CLOEXEC | O_NONBLOCK) } != 0 {
        aerr!("no wake pipe — worker threads cannot knock");
        return;
    }
    WAKE_WRITE_FD.store(fds[1], Ordering::Release);
    WAKE_READ_FD.with(|slot| slot.set(fds[0]));
    unsafe { ALooper_addFd(looper, fds[0], IDENT_CLOCK, ALOOPER_EVENT_INPUT, Some(on_wake), null_mut()) };
}

/// A knock from any thread: one byte down the wake pipe.
pub fn wake_from_any_thread() {
    let fd = WAKE_WRITE_FD.load(Ordering::Acquire);
    if fd >= 0 {
        let byte = 1u8;
        unsafe { write(fd, (&raw const byte).cast(), 1) };
    }
}

fn timer_fd(looper: *mut ALooper, callback: LooperCallback) -> c_int {
    let fd = unsafe { timerfd_create(CLOCK_MONOTONIC, O_CLOEXEC | O_NONBLOCK) };
    if fd < 0 {
        return -1;
    }
    unsafe { ALooper_addFd(looper, fd, IDENT_CLOCK, ALOOPER_EVENT_INPUT, Some(callback), null_mut()) };
    fd
}

fn arm_timer(fd: c_int, interval: f64) {
    if fd < 0 {
        return;
    }
    let spec = Timespec {
        sec: interval.trunc() as i64,
        nsec: (interval.fract() * 1e9).round() as i64,
    };
    let armed = Itimerspec { interval: spec, value: spec };
    unsafe { timerfd_settime(fd, 0, &armed, null_mut()) };
}

/// The caret's blink half-period beats always; the slow clock waits to
/// be asked.
fn install_clocks(looper: *mut ALooper) {
    let blink = timer_fd(looper, on_blink);
    BLINK_FD.with(|slot| slot.set(blink));
    arm_timer(blink, 0.5);
    SLOW_FD.with(|slot| slot.set(timer_fd(looper, on_slow)));
    INSETS_FD.with(|slot| slot.set(timer_fd(looper, on_insets_retry)));
}

/// Points the frame driver at the pace the moment deserves.
pub fn set_frame_driver(pace: DriverPace) {
    PACE.with(|slot| slot.set(pace));
    match pace {
        DriverPace::Full => {
            arm_slow(0.0);
            post_frame();
        }
        DriverPace::Slow(interval) => arm_slow(interval),
        DriverPace::Off => arm_slow(0.0),
    }
}

fn arm_slow(interval: f64) {
    if (SLOW_INTERVAL.with(|slot| slot.replace(interval)) - interval).abs() < f64::EPSILON {
        return;
    }
    arm_timer(SLOW_FD.with(Cell::get), interval);
}

// MARK: - Input

/// The queue has events: every one is taken, given to the IME first,
/// then handled or not, and FINISHED — a finished event is the
/// platform's own bookkeeping, and one left unfinished stalls the pipe.
unsafe extern "C" fn on_input(_fd: c_int, _events: c_int, _data: *mut c_void) -> c_int {
    let queue = QUEUE.with(Cell::get);
    if queue.is_null() {
        return 1;
    }
    loop {
        let mut event: *mut AInputEvent = null_mut();
        if unsafe { AInputQueue_getEvent(queue, &mut event) } < 0 {
            break;
        }
        if unsafe { AInputQueue_preDispatchEvent(queue, event) } != 0 {
            // the IME took it; it comes back through the queue if not
            continue;
        }
        let handled = match unsafe { AInputEvent_getType(event) } {
            AINPUT_EVENT_TYPE_MOTION => motion(event),
            AINPUT_EVENT_TYPE_KEY => key(event),
            _ => false,
        };
        unsafe { AInputQueue_finishEvent(queue, event, handled as c_int) };
    }
    1
}

/// A motion event from a pointer source — a finger, a stylus, a mouse —
/// spoken as the touch events, one per pointer, in points.
fn motion(event: *mut AInputEvent) -> bool {
    if unsafe { AInputEvent_getSource(event) } & AINPUT_SOURCE_CLASS_POINTER == 0 {
        return false;
    }
    let action = unsafe { AMotionEvent_getAction(event) };
    let kind = action & AMOTION_EVENT_ACTION_MASK;
    let index = ((action & AMOTION_EVENT_ACTION_POINTER_INDEX_MASK)
        >> AMOTION_EVENT_ACTION_POINTER_INDEX_SHIFT) as usize;
    let scale = SCALE.with(Cell::get).max(1) as f64;
    let at = |index: usize| unsafe {
        (
            AMotionEvent_getX(event, index) as f64 / scale,
            AMotionEvent_getY(event, index) as f64 / scale,
        )
    };
    let id = |index: usize| unsafe { AMotionEvent_getPointerId(event, index) } as u64;
    let count = unsafe { AMotionEvent_getPointerCount(event) };
    match kind {
        AMOTION_EVENT_ACTION_DOWN => {
            let (x, y) = at(0);
            let taps = TAPS.with(|taps| taps.borrow_mut().down(Instant::now(), x, y));
            dispatch(AppEvent::TouchBegan { id: id(0), x, y, taps });
        }
        AMOTION_EVENT_ACTION_POINTER_DOWN => {
            let (x, y) = at(index);
            dispatch(AppEvent::TouchBegan { id: id(index), x, y, taps: 1 });
        }
        // a move carries EVERY pointer, the still ones included
        AMOTION_EVENT_ACTION_MOVE => {
            for index in 0..count {
                let (x, y) = at(index);
                dispatch(AppEvent::TouchMoved { id: id(index), x, y });
            }
        }
        AMOTION_EVENT_ACTION_POINTER_UP => {
            let (x, y) = at(index);
            dispatch(AppEvent::TouchEnded { id: id(index), x, y });
        }
        AMOTION_EVENT_ACTION_UP => {
            let (x, y) = at(0);
            TAPS.with(|taps| taps.borrow_mut().up(Instant::now(), x, y));
            dispatch(AppEvent::TouchEnded { id: id(0), x, y });
        }
        AMOTION_EVENT_ACTION_CANCEL => {
            for index in 0..count {
                dispatch(AppEvent::TouchCancelled { id: id(index) });
            }
            TAPS.with(|taps| taps.borrow_mut().cancel());
        }
        _ => return false,
    }
    true
}

/// A key event: the keymap gate first, then the keys that type, then
/// the named keys. The back key is the one the platform watches — a
/// press and a release both unhandled leave the activity, so a press
/// the app took keeps its release too.
fn key(event: *mut AInputEvent) -> bool {
    let keycode = unsafe { AKeyEvent_getKeyCode(event) };
    match unsafe { AKeyEvent_getAction(event) } {
        AKEY_EVENT_ACTION_DOWN => {}
        AKEY_EVENT_ACTION_UP => {
            return keycode == keys::KEYCODE_BACK && BACK_TAKEN.with(|slot| slot.replace(false));
        }
        _ => return false,
    }
    let stroke = keys::stroke_of(keycode, unsafe { AKeyEvent_getMetaState(event) });
    let handled = key_down(stroke);
    if keycode == keys::KEYCODE_BACK {
        BACK_TAKEN.with(|slot| slot.set(handled));
    }
    handled
}

fn key_down(stroke: KeyStroke) -> bool {
    let gated = KEY_GATE.with(|slot| {
        slot.borrow_mut().as_mut().is_some_and(|gate| gate(&stroke))
    });
    if gated {
        return true;
    }
    let chord = stroke.control || stroke.alt || stroke.meta;
    match stroke.keycode {
        keys::KEYCODE_ENTER | keys::KEYCODE_NUMPAD_ENTER | keys::KEYCODE_DPAD_CENTER
            if !chord =>
        {
            dispatch(AppEvent::Text("\n".to_string()));
            return true;
        }
        keys::KEYCODE_DEL if !chord => {
            dispatch(AppEvent::DeleteBackward);
            return true;
        }
        _ => {}
    }
    if let Some(typed) = stroke.typed {
        dispatch(AppEvent::Text(typed.to_string()));
        return true;
    }
    // a named key, or a chord: the handler says whether it was taken
    let named = keys::key_pattern(&stroke).is_some();
    if !named {
        return false;
    }
    KEY_TAKEN.with(|slot| slot.set(false));
    let back = stroke.keycode == keys::KEYCODE_BACK;
    dispatch(AppEvent::Key(stroke));
    // the back key is the platform's unless the app took it; every
    // other named key is the app's whatever it did with it
    !back || KEY_TAKEN.with(Cell::get)
}

// MARK: - What the shell reads

fn window_physical() -> (u32, u32) {
    let window = WINDOW.with(Cell::get);
    if window.is_null() {
        return (0, 0);
    }
    unsafe {
        (
            ANativeWindow_getWidth(window).max(0) as u32,
            ANativeWindow_getHeight(window).max(0) as u32,
        )
    }
}

/// The window's size in points.
pub fn view_size() -> (f64, f64) {
    let (width, height) = window_physical();
    let scale = SCALE.with(Cell::get).max(1) as f64;
    (width as f64 / scale, height as f64 / scale)
}

/// Device pixels per point — the density over 160, rounded to a whole
/// number (the tiers count in whole pixels per point).
pub fn view_scale() -> usize {
    SCALE.with(Cell::get).max(1)
}

/// The configuration: dark or light (`None` when the system says
/// neither), whether the width is compact, and the scale.
pub fn config() -> (Option<bool>, bool, usize) {
    // the resources first: they hold what the framework updated before
    // it called; the native configuration answers a rotation or a night
    // switch late
    if let Some((ui_mode, width_dp, density_dpi)) = crate::jni::configuration() {
        // Configuration.UI_MODE_NIGHT_MASK 0x30: NO 0x10, YES 0x20
        let dark = match ui_mode & 0x30 {
            0x20 => Some(true),
            0x10 => Some(false),
            _ => None,
        };
        let scale = ((density_dpi.max(1) as f64) / 160.0).round().max(1.0) as usize;
        return (dark, width_dp < 600, scale);
    }
    let activity = ACTIVITY.with(Cell::get);
    if activity.is_null() {
        return (None, false, 1);
    }
    unsafe {
        let config = AConfiguration_new();
        if config.is_null() {
            return (None, false, 1);
        }
        AConfiguration_fromAssetManager(config, (*activity).asset_manager);
        let density = AConfiguration_getDensity(config);
        let night = AConfiguration_getUiModeNight(config);
        let width_dp = AConfiguration_getScreenWidthDp(config);
        AConfiguration_delete(config);
        let density = match density {
            0 | ACONFIGURATION_DENSITY_ANY | ACONFIGURATION_DENSITY_NONE => 160,
            other => other,
        };
        let scale = ((density as f64) / 160.0).round().max(1.0) as usize;
        let dark = match night {
            ACONFIGURATION_UI_MODE_NIGHT_YES => Some(true),
            ACONFIGURATION_UI_MODE_NIGHT_NO => Some(false),
            _ => None,
        };
        (dark, width_dp < 600, scale)
    }
}

/// The safe area, in points: the system bars and the cutout, read
/// from the window (the last good answer while the decor view is not
/// attached yet).
pub fn safe_area() -> (f64, f64, f64, f64) {
    read_insets();
    SAFE_LAST.with(Cell::get)
}

/// Reads the window's insets into the last-good cells; answers
/// whether the safe area or the keyboard moved.
fn read_insets() -> (bool, bool, bool) {
    let Some(insets) = crate::jni::window_insets() else {
        trace_line("insets: none yet");
        return (false, false, false);
    };
    if crate::log::trace() {
        alog!("insets: bars {:?}, ime {:?} visible {}", insets.bars, insets.ime, insets.ime_visible);
    }
    let scale = SCALE.with(Cell::get).max(1) as f64;
    let safe = (
        insets.bars.top as f64 / scale,
        insets.bars.left as f64 / scale,
        insets.bars.bottom as f64 / scale,
        insets.bars.right as f64 / scale,
    );
    let keyboard = if insets.ime_visible { insets.ime.bottom as f64 / scale } else { 0.0 };
    let safe_moved = SAFE_LAST.with(|slot| slot.replace(safe)) != safe;
    let keyboard_moved = KEYBOARD_LAST.with(|slot| slot.replace(keyboard)) != keyboard;
    // a keyboard that was up and is up no more was dismissed by the
    // person (the back key, which the IME keeps) — the field lets go,
    // or the next frame would raise it again
    let dismissed = if insets.ime_visible {
        IME_SEEN.with(|slot| slot.set(true));
        false
    } else {
        IME_SEEN.with(|slot| slot.replace(false)) && KEYBOARD_WANTED.with(Cell::get)
    };
    (safe_moved, keyboard_moved, dismissed)
}

/// Reads the insets and reports what moved.
fn refresh_insets() {
    let (safe_moved, keyboard_moved, dismissed) = read_insets();
    if safe_moved {
        let (top, left, bottom, right) = SAFE_LAST.with(Cell::get);
        dispatch(AppEvent::SafeArea { top, left, bottom, right });
    }
    if keyboard_moved {
        dispatch(AppEvent::Keyboard { overlap: KEYBOARD_LAST.with(Cell::get) });
    }
    if dismissed {
        KEYBOARD_WANTED.with(|slot| slot.set(false));
        dispatch(AppEvent::KeyboardDismissed);
    }
}

unsafe extern "C" fn on_insets_retry(fd: c_int, _events: c_int, _data: *mut c_void) -> c_int {
    let mut count = 0u64;
    unsafe { read(fd, (&raw mut count).cast(), 8) };
    refresh_insets();
    let left = INSETS_RETRY.with(|slot| slot.get().saturating_sub(1));
    INSETS_RETRY.with(|slot| slot.set(left));
    if left > 0 {
        arm_once(fd, if left == 2 { 0.2 } else { 0.3 });
    }
    1
}

/// The keyboard was asked to move: the insets say so a little later,
/// so the clock asks at 100, 300 and 600 ms.
fn arm_insets_retry() {
    INSETS_RETRY.with(|slot| slot.set(3));
    arm_once(INSETS_FD.with(Cell::get), 0.1);
}

fn arm_once(fd: c_int, delay: f64) {
    if fd < 0 {
        return;
    }
    let armed = Itimerspec {
        interval: Timespec { sec: 0, nsec: 0 },
        value: Timespec { sec: delay.trunc() as i64, nsec: (delay.fract() * 1e9).round() as i64 },
    };
    unsafe { timerfd_settime(fd, 0, &armed, null_mut()) };
}

/// The system's reduce-motion setting.
pub fn reduce_motion() -> bool {
    crate::jni::reduce_motion()
}

/// Asks the system for the soft keyboard, or to take it back — only
/// when the wish flips.
pub fn want_keyboard(wanted: bool) {
    if KEYBOARD_WANTED.with(|slot| slot.replace(wanted)) == wanted {
        return;
    }
    // the input method is asked in the name of the view it serves; the
    // activity's own door is the fallback, and a refusal is not
    // remembered — the next frame asks again
    if !crate::jni::keyboard(wanted) {
        let activity = ACTIVITY.with(Cell::get);
        if activity.is_null() {
            KEYBOARD_WANTED.with(|slot| slot.set(!wanted));
            return;
        }
        unsafe {
            if wanted {
                ANativeActivity_showSoftInput(activity, ANATIVEACTIVITY_SHOW_SOFT_INPUT_FORCED);
            } else {
                ANativeActivity_hideSoftInput(activity, 0);
            }
        }
    }
    arm_insets_retry();
}

pub fn clipboard_write(text: &str) {
    CLIPBOARD.with(|slot| *slot.borrow_mut() = Some(text.to_string()));
    crate::jni::clipboard_write(text);
}

pub fn clipboard_read() -> Option<String> {
    crate::jni::clipboard_read().or_else(|| CLIPBOARD.with(|slot| slot.borrow().clone()))
}

// MARK: - The roads to the window

/// Picks the road for the window that stands: Vulkan through the shared
/// tier, or the CPU floor. `debug.bunny.present=cpu` skips the tier.
/// False = the floor took the window.
pub fn install_gpu() -> bool {
    let window = WINDOW.with(Cell::get);
    if window.is_null() {
        return false;
    }
    let skip = crate::log::property(c"debug.bunny.present").is_some_and(|value| value == "cpu");
    if !skip {
        let physical = window_physical();
        if let Some(presenter) =
            VkPresenter::install(SurfaceSource::Android { window: window.cast() }, physical, false)
        {
            PRESENTER.with(|slot| *slot.borrow_mut() = Some(presenter));
            ROAD.with(|slot| slot.set(Road::Vulkan));
            alog!("presenting by vulkan");
            return true;
        }
    }
    install_cpu();
    false
}

/// The CPU floor: the window's buffers in RGBA, filled by the raster.
fn install_cpu() {
    let window = WINDOW.with(Cell::get);
    ROAD.with(|slot| slot.set(Road::Cpu));
    CPU.with(|slot| drop(slot.borrow_mut().take()));
    if window.is_null() {
        return;
    }
    unsafe { ANativeWindow_setBuffersGeometry(window, 0, 0, WINDOW_FORMAT_RGBA_8888) };
    alog!("presenting by the CPU floor");
}

/// Presents one frame — nothing while the activity is paused or has no
/// window.
pub fn present(
    display: &DisplayList,
    size: Size,
    scale: usize,
    canvas: Color,
    text: &dyn TextEngine,
    images: &dyn ImageEngine,
) {
    if BACKGROUNDED.with(Cell::get) || WINDOW.with(Cell::get).is_null() {
        return;
    }
    match ROAD.with(Cell::get) {
        Road::None => {}
        Road::Vulkan => present_vulkan(display, size, scale, canvas, text, images),
        Road::Cpu => present_cpu(display, size, scale, canvas, text, images),
    }
}

fn present_vulkan(
    display: &DisplayList,
    size: Size,
    scale: usize,
    canvas: Color,
    text: &dyn TextEngine,
    images: &dyn ImageEngine,
) {
    let present_once = || {
        PRESENTER.with(|slot| {
            slot.borrow_mut().as_mut().map(|presenter| {
                presenter.present(display, size, scale, canvas, text, images, &mut |_| true, &mut || {})
            })
        })
    };
    match present_once() {
        None | Some(Presented::Ok) => {}
        Some(Presented::SurfaceLost) => {
            // the surface went with a window; the one standing now
            // takes its place on the same device
            let window = WINDOW.with(Cell::get);
            let physical = window_physical();
            let attached = PRESENTER.with(|slot| {
                slot.borrow_mut().as_mut().is_some_and(|presenter| {
                    presenter.detach_surface();
                    presenter.attach_surface(SurfaceSource::Android { window: window.cast() }, physical)
                })
            });
            if attached {
                let _ = present_once();
            } else {
                aerr!("the surface is lost and the window refuses another — the CPU floor takes over");
                PRESENTER.with(|slot| drop(slot.borrow_mut().take()));
                install_cpu();
                present_cpu(display, size, scale, canvas, text, images);
            }
        }
        Some(Presented::DeviceLost) => {
            PRESENTER.with(|slot| drop(slot.borrow_mut().take()));
            if !RECREATE_SPENT.with(|spent| spent.replace(true)) && install_gpu() {
                let _ = present_once();
                return;
            }
            aerr!("the vulkan device is lost — the CPU floor takes over");
            install_cpu();
            present_cpu(display, size, scale, canvas, text, images);
        }
    }
}

/// The floor: the retained surface rasterizes what changed, and the
/// whole picture is copied into the window's next buffer — a locked
/// buffer may be any of the window's, so a partial copy would leave
/// holes.
fn present_cpu(
    display: &DisplayList,
    size: Size,
    scale: usize,
    canvas: Color,
    text: &dyn TextEngine,
    images: &dyn ImageEngine,
) {
    let physical = (
        (size.width * scale as f64).round().max(0.0) as usize,
        (size.height * scale as f64).round().max(0.0) as usize,
    );
    if physical.0 == 0 || physical.1 == 0 {
        return;
    }
    CPU.with(|slot| {
        let mut slot = slot.borrow_mut();
        let stale = match &*slot {
            Some((retained, retained_scale, retained_canvas)) => {
                retained.bitmap().width() != physical.0
                    || retained.bitmap().height() != physical.1
                    || *retained_scale != scale
                    || *retained_canvas != canvas
            }
            None => true,
        };
        let fresh = stale;
        if stale {
            *slot = Some((
                bunny_ui::raster::Surface::new(physical.0, physical.1, scale, canvas),
                scale,
                canvas,
            ));
        }
        let (retained, _, _) = slot.as_mut().expect("surface for the frame");
        let damage = retained.frame(display.clone(), text, images);
        if damage.is_empty() && !fresh {
            return;
        }
        blit(retained.bitmap().width(), retained.bitmap().height(), retained.rgba());
    });
}

/// Copies an RGBA picture into the window's next buffer and posts it.
fn blit(width: usize, height: usize, rgba: &[u8]) {
    let window = WINDOW.with(Cell::get);
    if window.is_null() {
        return;
    }
    let mut buffer = ANativeWindowBuffer {
        width: 0,
        height: 0,
        stride: 0,
        format: 0,
        bits: null_mut(),
        reserved: [0; 6],
    };
    if unsafe { ANativeWindow_lock(window, &mut buffer, null_mut()) } != 0 {
        return;
    }
    let format_ok =
        buffer.format == WINDOW_FORMAT_RGBA_8888 || buffer.format == WINDOW_FORMAT_RGBX_8888;
    if format_ok && !buffer.bits.is_null() {
        let rows = height.min(buffer.height.max(0) as usize);
        let columns = width.min(buffer.width.max(0) as usize);
        let stride = buffer.stride.max(0) as usize * 4;
        for row in 0..rows {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    rgba.as_ptr().add(row * width * 4),
                    buffer.bits.cast::<u8>().add(row * stride),
                    columns * 4,
                );
            }
        }
    } else if !format_ok {
        aerr!("the window's buffer is not RGBA ({}) — the floor cannot paint", buffer.format);
    }
    unsafe { ANativeWindow_unlockAndPost(window) };
    if buffer.width.max(0) as usize != width || buffer.height.max(0) as usize != height {
        // the window moved under the surface: the next frame remakes it
        CPU.with(|slot| drop(slot.borrow_mut().take()));
        dispatch(AppEvent::Redraw);
    }
}

/// The activity's private files directory — where a registered face is
/// written for the platform to read back.
pub fn internal_data_path() -> Option<String> {
    let activity = ACTIVITY.with(Cell::get);
    if activity.is_null() {
        return None;
    }
    let path = unsafe { (*activity).internal_data_path };
    if path.is_null() {
        return None;
    }
    Some(unsafe { std::ffi::CStr::from_ptr(path) }.to_string_lossy().into_owned())
}
