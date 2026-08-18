//! Hand-written Win32 FFI: the window class, the message pump, DPI
//! metrics, the DIB presentation backing and the frame driver. Every
//! declaration is written here, against the platform headers — no
//! import libraries, no bindings crate.
//!
//! Positions handed to the shell are LAYOUT coordinates: top-left
//! origin, logical points. Win32 client coordinates already start at
//! the top-left, so the boundary only divides by the DPI factor —
//! there is no y-flip on this platform.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};

// MARK: - Win32 ABI surface (the platform headers, transcribed)

pub type Hwnd = isize;
type Hdc = isize;
type Handle = isize;
type WndProc = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize;

#[repr(C)]
struct WndClassW {
    style: u32,
    wnd_proc: WndProc,
    cls_extra: i32,
    wnd_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: Point,
}

#[repr(C)]
struct PaintStruct {
    hdc: Hdc,
    erase: i32,
    paint: Rect,
    restore: i32,
    inc_update: i32,
    reserved: [u8; 32],
}

#[repr(C)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_pels_per_meter: i32,
    y_pels_per_meter: i32,
    clr_used: u32,
    clr_important: u32,
}

#[repr(C)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [u32; 1],
}

#[repr(C)]
struct TrackMouseEventArgs {
    size: u32,
    flags: u32,
    hwnd: Hwnd,
    hover_time: u32,
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegisterClassW(class: *const WndClassW) -> u16;
    fn CreateWindowExW(
        ex_style: u32,
        class_name: *const u16,
        window_name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Hwnd,
        menu: Handle,
        instance: Handle,
        param: *const c_void,
    ) -> Hwnd;
    fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn PostQuitMessage(code: i32);
    fn PostMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn DestroyWindow(hwnd: Hwnd) -> i32;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    fn UpdateWindow(hwnd: Hwnd) -> i32;
    fn InvalidateRect(hwnd: Hwnd, rect: *const Rect, erase: i32) -> i32;
    fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn GetDpiForWindow(hwnd: Hwnd) -> u32;
    fn AdjustWindowRectExForDpi(
        rect: *mut Rect,
        style: u32,
        menu: i32,
        ex_style: u32,
        dpi: u32,
    ) -> i32;
    fn SetWindowPos(
        hwnd: Hwnd,
        after: Hwnd,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> i32;
    fn BeginPaint(hwnd: Hwnd, paint: *mut PaintStruct) -> Hdc;
    fn EndPaint(hwnd: Hwnd, paint: *const PaintStruct) -> i32;
    fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
    fn SetCursor(cursor: Handle) -> Handle;
    fn SetTimer(hwnd: Hwnd, id: usize, elapse: u32, callback: *const c_void) -> usize;
    fn KillTimer(hwnd: Hwnd, id: usize) -> i32;
    fn TrackMouseEvent(track: *mut TrackMouseEventArgs) -> i32;
    fn SetCapture(hwnd: Hwnd) -> Hwnd;
    fn ReleaseCapture() -> i32;
    fn GetDoubleClickTime() -> u32;
    fn SetProcessDPIAware() -> i32;
}

#[link(name = "gdi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateDIBSection(
        hdc: Hdc,
        info: *const BitmapInfo,
        usage: u32,
        bits: *mut *mut u8,
        section: Handle,
        offset: u32,
    ) -> Handle;
    fn CreateCompatibleDC(hdc: Hdc) -> Hdc;
    fn SelectObject(hdc: Hdc, object: Handle) -> Handle;
    fn DeleteObject(object: Handle) -> i32;
    fn DeleteDC(hdc: Hdc) -> i32;
    fn BitBlt(
        dest: Hdc,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        src: Hdc,
        src_x: i32,
        src_y: i32,
        rop: u32,
    ) -> i32;
    fn StretchBlt(
        dest: Hdc,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        src: Hdc,
        src_x: i32,
        src_y: i32,
        src_width: i32,
        src_height: i32,
        rop: u32,
    ) -> i32;
    fn SetStretchBltMode(hdc: Hdc, mode: i32) -> i32;
    fn SetBrushOrgEx(hdc: Hdc, x: i32, y: i32, old: *mut Point) -> i32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *const c_void;
    fn QueryPerformanceCounter(count: *mut i64) -> i32;
    fn QueryPerformanceFrequency(frequency: *mut i64) -> i32;
}

#[link(name = "dwmapi", kind = "raw-dylib")]
unsafe extern "system" {
    fn DwmFlush() -> i32;
}

// window styles
const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
// SetWindowPos flags
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_NOMOVE: u32 = 0x0002;
// ShowWindow
const SW_SHOW: i32 = 5;
// messages
const WM_CREATE: u32 = 0x0001;
const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_ACTIVATE: u32 = 0x0006;
const WM_PAINT: u32 = 0x000F;
const WM_CLOSE: u32 = 0x0010;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_SETCURSOR: u32 = 0x0020;
const WM_TIMER: u32 = 0x0113;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_ENTERSIZEMOVE: u32 = 0x0231;
const WM_EXITSIZEMOVE: u32 = 0x0232;
const WM_DPICHANGED: u32 = 0x02E0;
/// One frame-driver tick landed (posted by the driver thread).
const WM_APP_FRAME: u32 = 0x8000 + 1;
// WM_SIZE minimized
const SIZE_MINIMIZED: usize = 1;
// WM_ACTIVATE inactive
const WA_INACTIVE: usize = 0;
// hit-test: the client area (WM_SETCURSOR's low word)
const HTCLIENT: isize = 1;
// timers
const TIMER_BLINK: usize = 1;
const TIMER_RESIZE: usize = 2;
// TrackMouseEvent
const TME_LEAVE: u32 = 0x0002;
// raster op
const SRCCOPY: u32 = 0x00CC_0020;
// StretchBlt mode: block-average shrink (needs SetBrushOrgEx by contract)
const HALFTONE: i32 = 4;
// stock cursors
const IDC_ARROW: usize = 32512;
const IDC_HAND: usize = 32649;
const IDC_SIZEWE: usize = 32644;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

// MARK: - Events

/// The shell's event vocabulary — the Windows twin of the mac AppEvent.
/// Positions are LAYOUT coordinates (top-left origin, logical points).
pub enum AppEvent {
    MouseDown { x: f64, y: f64, clicks: u8 },
    /// The right button: the context-menu press.
    RightMouseDown { x: f64, y: f64 },
    MouseUp { x: f64, y: f64 },
    MouseMoved { x: f64, y: f64 },
    /// The pointer left the window — without this event the hover would
    /// stay stuck at the edge (the reason for TrackMouseEvent).
    MouseExited,
    /// Half-period of the caret blink (the shell's timer).
    Blink,
    /// One frame-driver tick: compose the next animated frame. `dt` is
    /// the interval this frame covers, in seconds, already clamped.
    Frame { dt: f64 },
    /// The window changed size (or needs the first frame).
    Redraw,
    /// The window deactivated (the user switched apps or windows) —
    /// open popovers close, the platform's own manner.
    ResignKey,
}

thread_local! {
    static HANDLER: RefCell<Option<Box<dyn FnMut(AppEvent)>>> = const { RefCell::new(None) };
}

/// Registers who receives the events (the shell's loop).
pub fn set_handler(handler: Box<dyn FnMut(AppEvent)>) {
    HANDLER.with(|slot| *slot.borrow_mut() = Some(handler));
}

/// Delivers an event to the handler — used by the window procedure and
/// by the first frame. Never called from a paint: `WM_PAINT` reads the
/// backing directly, so a synchronous present inside a dispatch cannot
/// re-enter this borrow.
pub fn dispatch(event: AppEvent) {
    HANDLER.with(|slot| {
        if let Some(handler) = slot.borrow_mut().as_mut() {
            handler(event);
        }
    });
}

// MARK: - DPI metrics (one funnel)

/// Everything the shell knows about a window's geometry, refreshed in
/// ONE place. `factor` is the true DPI ratio (1.25, 1.5); `int_scale`
/// is the raster scale the engine sees (`ceil(factor)`, the integer
/// contract); the present path maps raster pixels onto client pixels.
#[derive(Clone, Copy, Default)]
struct Metrics {
    factor: f64,
    int_scale: usize,
    logical: (f64, f64),
    client_px: (u32, u32),
}

thread_local! {
    static METRICS: RefCell<HashMap<Hwnd, Metrics>> = RefCell::new(HashMap::new());
}

/// The scale policy: dpi → (integer raster scale, true factor).
/// 96 → (1, 1.0); 120 → (2, 1.25); 144 → (2, 1.5); 192 → (2, 2.0).
fn scale_of(dpi: u32) -> (usize, f64) {
    let factor = dpi.max(1) as f64 / 96.0;
    (factor.ceil().max(1.0) as usize, factor)
}

/// The ONLY writer of [`Metrics`] — called from window creation,
/// `WM_SIZE` and `WM_DPICHANGED` (the three entries, one function).
fn refresh_metrics(hwnd: Hwnd) {
    let mut rect = Rect::default();
    let dpi = unsafe {
        GetClientRect(hwnd, &mut rect);
        GetDpiForWindow(hwnd)
    };
    let (int_scale, factor) = scale_of(dpi);
    let client = ((rect.right - rect.left).max(0) as u32, (rect.bottom - rect.top).max(0) as u32);
    let logical = (client.0 as f64 / factor, client.1 as f64 / factor);
    METRICS.with(|slot| {
        slot.borrow_mut().insert(hwnd, Metrics { factor, int_scale, logical, client_px: client });
    });
}

fn metrics_of(hwnd: Hwnd) -> Metrics {
    METRICS.with(|slot| slot.borrow().get(&hwnd).copied().unwrap_or_default())
}

// MARK: - The presentation backing (a DIB section per window)

/// The ffi-owned presentation backing: `WM_PAINT` always reads from
/// here (it arrives uninvited — a restore, an occlusion revalidation —
/// and must repaint from retained bytes). The DIB is top-down BGRA;
/// [`WindowHandle::blit_partial`] syncs it with damage-only row copies,
/// swizzling from the engine's RGBA in the same pass.
struct Backing {
    dc: Hdc,
    bitmap: Handle,
    old_bitmap: Handle,
    bits: *mut u8,
    width: usize,
    height: usize,
}

impl Backing {
    fn release(&mut self) {
        unsafe {
            if self.dc != 0 {
                SelectObject(self.dc, self.old_bitmap);
                DeleteObject(self.bitmap);
                DeleteDC(self.dc);
            }
        }
        self.dc = 0;
        self.bitmap = 0;
        self.bits = std::ptr::null_mut();
        self.width = 0;
        self.height = 0;
    }
}

thread_local! {
    /// One backing per WINDOW — the main window and every future panel
    /// present through the same `WM_PAINT`, each from its own store.
    static BACKING: RefCell<HashMap<Hwnd, Backing>> = RefCell::new(HashMap::new());
}

/// Makes (or remakes) the DIB at the given raster size. Returns false
/// when the platform refuses (zero size, headless quirk) — the present
/// simply skips.
fn ensure_backing(hwnd: Hwnd, width: usize, height: usize) -> bool {
    BACKING.with(|stores| {
        let mut stores = stores.borrow_mut();
        let backing = stores.entry(hwnd).or_insert(Backing {
            dc: 0,
            bitmap: 0,
            old_bitmap: 0,
            bits: std::ptr::null_mut(),
            width: 0,
            height: 0,
        });
        if backing.width == width && backing.height == height && backing.dc != 0 {
            return true;
        }
        backing.release();
        if width == 0 || height == 0 {
            return false;
        }
        let info = BitmapInfo {
            header: BitmapInfoHeader {
                size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                width: width as i32,
                // negative height = top-down rows, the engine's order
                height: -(height as i32),
                planes: 1,
                bit_count: 32,
                compression: 0, // BI_RGB
                size_image: 0,
                x_pels_per_meter: 0,
                y_pels_per_meter: 0,
                clr_used: 0,
                clr_important: 0,
            },
            colors: [0],
        };
        unsafe {
            let mut bits: *mut u8 = std::ptr::null_mut();
            let bitmap = CreateDIBSection(0, &info, 0, &mut bits, 0, 0);
            if bitmap == 0 || bits.is_null() {
                return false;
            }
            let dc = CreateCompatibleDC(0);
            let old_bitmap = SelectObject(dc, bitmap);
            *backing = Backing { dc, bitmap, old_bitmap, bits, width, height };
        }
        true
    })
}

/// One damage row, engine RGBA → DIB BGRA, alpha forced opaque (the
/// main window is opaque; panels premultiply on their own road).
fn swizzle_row(rgba: &[u8], bgra: &mut [u8]) {
    for (source, dest) in rgba.chunks_exact(4).zip(bgra.chunks_exact_mut(4)) {
        dest[0] = source[2];
        dest[1] = source[1];
        dest[2] = source[0];
        dest[3] = 0xFF;
    }
}

/// Clamps a damage rect to the surface and answers it as usize bounds;
/// `None` when the intersection is empty.
fn clamp_damage(
    damage: (i64, i64, i64, i64),
    width: usize,
    height: usize,
) -> Option<(usize, usize, usize, usize)> {
    let (x0, y0, x1, y1) = damage;
    let x0 = x0.clamp(0, width as i64) as usize;
    let x1 = x1.clamp(0, width as i64) as usize;
    let y0 = y0.clamp(0, height as i64) as usize;
    let y1 = y1.clamp(0, height as i64) as usize;
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
}

// MARK: - Cursor

/// What the pointer wears: the hand over an interactive target, the
/// horizontal resizer over a split's grip, the arrow elsewhere.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    Arrow,
    Pointing,
    ResizeLeftRight,
}

thread_local! {
    /// The cursor the scene wants right now — `WM_SETCURSOR` re-applies
    /// it every time the system would reset to the class cursor.
    static CURRENT_CURSOR: Cell<Cursor> = const { Cell::new(Cursor::Arrow) };
}

fn apply_cursor(cursor: Cursor) {
    let id = match cursor {
        Cursor::Arrow => IDC_ARROW,
        Cursor::Pointing => IDC_HAND,
        Cursor::ResizeLeftRight => IDC_SIZEWE,
    };
    unsafe {
        SetCursor(LoadCursorW(0, id as *const u16));
    }
}

// MARK: - Frame driver (born paused)

/// The driver thread parks on this shared state; the composition clock
/// (`DwmFlush`) paces the resumed loop and a posted message delivers
/// the tick, folded by `in_flight` so a busy queue never floods.
struct Driver {
    running: Mutex<bool>,
    resume: Condvar,
    hwnd: AtomicIsize,
    in_flight: AtomicBool,
}

static DRIVER: OnceLock<&'static Driver> = OnceLock::new();

fn driver() -> &'static Driver {
    DRIVER.get_or_init(|| {
        let shared: &'static Driver = Box::leak(Box::new(Driver {
            running: Mutex::new(false),
            resume: Condvar::new(),
            hwnd: AtomicIsize::new(0),
            in_flight: AtomicBool::new(false),
        }));
        std::thread::spawn(move || {
            loop {
                {
                    let mut running = shared.running.lock().expect("driver lock");
                    while !*running {
                        running = shared.resume.wait(running).expect("driver wait");
                    }
                }
                // the composition clock; some sessions (remote desktop)
                // refuse it — degrade to a plain metronome
                if unsafe { DwmFlush() } < 0 {
                    std::thread::sleep(std::time::Duration::from_millis(16));
                }
                let hwnd = shared.hwnd.load(Ordering::Acquire);
                if hwnd != 0 && !shared.in_flight.swap(true, Ordering::AcqRel) {
                    unsafe {
                        PostMessageW(hwnd, WM_APP_FRAME, 0, 0);
                    }
                }
            }
        });
        shared
    })
}

/// Parks or resumes the frame driver. Born paused; the shell resumes it
/// only while an animation (or a sleeping task) needs the clock.
pub fn set_frame_driver_paused(paused: bool) {
    let driver = driver();
    let mut running = driver.running.lock().expect("driver lock");
    *running = !paused;
    if !paused {
        driver.resume.notify_one();
    }
}

thread_local! {
    /// The previous tick's clock, for dt.
    static LAST_TICK: Cell<i64> = const { Cell::new(0) };
}

fn ticks_per_second() -> f64 {
    static FREQUENCY: OnceLock<i64> = OnceLock::new();
    *FREQUENCY.get_or_init(|| {
        let mut frequency = 0i64;
        unsafe {
            QueryPerformanceFrequency(&mut frequency);
        }
        frequency.max(1)
    }) as f64
}

/// dt for one driver tick, in seconds. Clamped: the first tick after a
/// resume reports the whole pause as the gap, and the clamp keeps the
/// springs continuous instead of teleporting them.
fn frame_dt() -> f64 {
    let mut now = 0i64;
    unsafe {
        QueryPerformanceCounter(&mut now);
    }
    let last = LAST_TICK.with(|cell| cell.replace(now));
    if last == 0 {
        return 0.0;
    }
    ((now - last) as f64 / ticks_per_second()).clamp(0.0, 1.0 / 30.0)
}

// MARK: - Click counting

thread_local! {
    /// The running click count: Win32 hands double-clicks as a message
    /// kind, not a count — the shell counts like AppKit does, within
    /// the system's double-click time and a small travel box.
    static CLICK_STATE: Cell<(i64, i32, i32, u8)> = const { Cell::new((0, 0, 0, 0)) };
}

fn count_click(x: i32, y: i32) -> u8 {
    let mut now = 0i64;
    unsafe {
        QueryPerformanceCounter(&mut now);
    }
    let window_ms = unsafe { GetDoubleClickTime() } as f64;
    let (last, last_x, last_y, count) = CLICK_STATE.with(|cell| cell.get());
    let elapsed_ms = (now - last) as f64 / ticks_per_second() * 1000.0;
    let near = (x - last_x).abs() <= 4 && (y - last_y).abs() <= 4;
    let clicks = if last != 0 && elapsed_ms <= window_ms && near {
        count.saturating_add(1)
    } else {
        1
    };
    CLICK_STATE.with(|cell| cell.set((now, x, y, clicks)));
    clicks
}

// MARK: - The window procedure

thread_local! {
    /// Whether the leave message is armed — TrackMouseEvent is one-shot
    /// and re-arms on the first move after each leave.
    static LEAVE_ARMED: Cell<bool> = const { Cell::new(false) };
}

fn layout_point(hwnd: Hwnd, lparam: isize) -> (f64, f64) {
    let x = (lparam & 0xFFFF) as u16 as i16 as i32;
    let y = ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32;
    let metrics = metrics_of(hwnd);
    let factor = if metrics.factor > 0.0 { metrics.factor } else { 1.0 };
    (x as f64 / factor, y as f64 / factor)
}

unsafe extern "system" fn window_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_CREATE => 0,
        WM_SIZE => {
            if wparam == SIZE_MINIMIZED {
                return 0;
            }
            refresh_metrics(hwnd);
            // present synchronously before returning: content and size
            // land in the same composition — the resize never shows a
            // stretched stale frame
            dispatch(AppEvent::Redraw);
            0
        }
        WM_DPICHANGED => {
            // honor the suggested rect verbatim — it is what makes a
            // mixed-DPI monitor drag land at the right physical size
            let suggested = lparam as *const Rect;
            if !suggested.is_null() {
                let rect = unsafe { *suggested };
                unsafe {
                    SetWindowPos(
                        hwnd,
                        0,
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
            }
            refresh_metrics(hwnd);
            dispatch(AppEvent::Redraw);
            0
        }
        WM_PAINT => {
            let mut paint = PaintStruct {
                hdc: 0,
                erase: 0,
                paint: Rect::default(),
                restore: 0,
                inc_update: 0,
                reserved: [0; 32],
            };
            let hdc = unsafe { BeginPaint(hwnd, &mut paint) };
            BACKING.with(|stores| {
                let stores = stores.borrow();
                let Some(backing) = stores.get(&hwnd) else { return };
                if backing.dc == 0 {
                    return;
                }
                let metrics = metrics_of(hwnd);
                let raster = (backing.width as i32, backing.height as i32);
                let client = (metrics.client_px.0 as i32, metrics.client_px.1 as i32);
                unsafe {
                    if raster == client {
                        // integer DPI: raster pixels ARE client pixels
                        let rect = paint.paint;
                        BitBlt(
                            hdc,
                            rect.left,
                            rect.top,
                            rect.right - rect.left,
                            rect.bottom - rect.top,
                            backing.dc,
                            rect.left,
                            rect.top,
                            SRCCOPY,
                        );
                    } else {
                        // fractional DPI: block-average the whole DIB
                        // onto the client; GDI clips to the dirty rect
                        SetStretchBltMode(hdc, HALFTONE);
                        SetBrushOrgEx(hdc, 0, 0, std::ptr::null_mut());
                        StretchBlt(
                            hdc, 0, 0, client.0, client.1, backing.dc, 0, 0, raster.0, raster.1,
                            SRCCOPY,
                        );
                    }
                }
            });
            unsafe {
                EndPaint(hwnd, &paint);
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_SETCURSOR => {
            if (lparam & 0xFFFF) == HTCLIENT {
                apply_cursor(CURRENT_CURSOR.with(|cell| cell.get()));
                1
            } else {
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
        }
        WM_MOUSEMOVE => {
            if !LEAVE_ARMED.with(|cell| cell.get()) {
                let mut track = TrackMouseEventArgs {
                    size: std::mem::size_of::<TrackMouseEventArgs>() as u32,
                    flags: TME_LEAVE,
                    hwnd,
                    hover_time: 0,
                };
                unsafe {
                    TrackMouseEvent(&mut track);
                }
                LEAVE_ARMED.with(|cell| cell.set(true));
            }
            let (x, y) = layout_point(hwnd, lparam);
            dispatch(AppEvent::MouseMoved { x, y });
            0
        }
        WM_MOUSELEAVE => {
            LEAVE_ARMED.with(|cell| cell.set(false));
            dispatch(AppEvent::MouseExited);
            0
        }
        WM_LBUTTONDOWN => {
            // the grab: a drag that leaves the window keeps reporting
            unsafe {
                SetCapture(hwnd);
            }
            let raw_x = (lparam & 0xFFFF) as u16 as i16 as i32;
            let raw_y = ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32;
            let clicks = count_click(raw_x, raw_y);
            let (x, y) = layout_point(hwnd, lparam);
            dispatch(AppEvent::MouseDown { x, y, clicks });
            0
        }
        WM_LBUTTONUP => {
            unsafe {
                ReleaseCapture();
            }
            let (x, y) = layout_point(hwnd, lparam);
            dispatch(AppEvent::MouseUp { x, y });
            0
        }
        WM_RBUTTONDOWN => {
            let (x, y) = layout_point(hwnd, lparam);
            dispatch(AppEvent::RightMouseDown { x, y });
            0
        }
        WM_TIMER => {
            if wparam == TIMER_BLINK {
                dispatch(AppEvent::Blink);
                return 0;
            }
            if wparam == TIMER_RESIZE {
                // the belt inside the modal resize loop: same tick road
                let dt = frame_dt();
                driver().in_flight.store(false, Ordering::Release);
                dispatch(AppEvent::Frame { dt });
                return 0;
            }
            0
        }
        WM_APP_FRAME => {
            driver().in_flight.store(false, Ordering::Release);
            let dt = frame_dt();
            dispatch(AppEvent::Frame { dt });
            0
        }
        WM_ENTERSIZEMOVE => {
            unsafe {
                SetTimer(hwnd, TIMER_RESIZE, 15, std::ptr::null());
            }
            0
        }
        WM_EXITSIZEMOVE => {
            unsafe {
                KillTimer(hwnd, TIMER_RESIZE);
            }
            0
        }
        WM_ACTIVATE => {
            if (wparam & 0xFFFF) == WA_INACTIVE {
                dispatch(AppEvent::ResignKey);
            }
            0
        }
        WM_CLOSE => {
            unsafe {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => {
            BACKING.with(|stores| {
                if let Some(mut backing) = stores.borrow_mut().remove(&hwnd) {
                    backing.release();
                }
            });
            // closing the window quits — the mac shell's manner
            unsafe {
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

// MARK: - Window creation and the pump

/// Opts the process into per-monitor DPI awareness, once, before any
/// window exists. Resolved at runtime: an old system degrades to
/// system-DPI-aware with one line on stderr, never a refusal to open.
fn install_dpi_awareness() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 is the pseudo
        // handle -4; the setter exists from Windows 10 1703
        let user32 = wide("user32.dll");
        unsafe {
            let module = GetModuleHandleW(user32.as_ptr());
            let name = b"SetProcessDpiAwarenessContext\0";
            let address = GetProcAddress(module, name.as_ptr());
            if address.is_null() {
                eprintln!("bunny_ui: per-monitor DPI unavailable; system DPI only");
                SetProcessDPIAware();
            } else {
                let set: unsafe extern "system" fn(isize) -> i32 =
                    std::mem::transmute(address);
                set(-4);
            }
        }
    });
}

fn register_class() -> Vec<u16> {
    static ONCE: OnceLock<Vec<u16>> = OnceLock::new();
    ONCE.get_or_init(|| {
        let name = wide("BunnyWindow");
        let class = WndClassW {
            // no CS_HREDRAW/CS_VREDRAW: a resize repaints through the
            // synchronous present, never through a forced erase
            style: 0,
            wnd_proc: window_proc,
            cls_extra: 0,
            wnd_extra: 0,
            instance: unsafe { GetModuleHandleW(std::ptr::null()) },
            icon: 0,
            // the class cursor stays empty — WM_SETCURSOR applies the
            // scene's choice instead
            cursor: 0,
            background: 0,
            menu_name: std::ptr::null(),
            class_name: name.as_ptr(),
        };
        unsafe {
            RegisterClassW(&class);
        }
        name
    })
    .clone()
}

/// Creates the window HIDDEN at the requested logical content size.
/// The caller presents the first frame and then shows it — the window
/// never flashes unpainted. `_scene_chrome` is accepted for signature
/// parity with the mac shell; the scene-drawn chrome is a later phase.
pub fn create_window(title: &str, width: f64, height: f64, _scene_chrome: bool) -> WindowHandle {
    install_dpi_awareness();
    let class_name = register_class();
    let title = wide(title);
    let style = WS_OVERLAPPEDWINDOW;
    const CW_USEDEFAULT: i32 = i32::MIN; // 0x80000000
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            640,
            480,
            0,
            0,
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        )
    };
    assert!(hwnd != 0, "the platform refused the window");
    // now the window knows its monitor: size the CLIENT area to the
    // requested logical points at the true DPI
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let (_, factor) = scale_of(dpi);
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: (width * factor).round() as i32,
        bottom: (height * factor).round() as i32,
    };
    unsafe {
        AdjustWindowRectExForDpi(&mut rect, style, 0, 0, dpi);
        SetWindowPos(
            hwnd,
            0,
            0,
            0,
            rect.right - rect.left,
            rect.bottom - rect.top,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOMOVE,
        );
        // the slow clock: caret blink and the tooltip's wait
        SetTimer(hwnd, TIMER_BLINK, 500, std::ptr::null());
    }
    refresh_metrics(hwnd);
    driver().hwnd.store(hwnd, Ordering::Release);
    WindowHandle { hwnd }
}

/// Shows the window after the first present (the anti-flash order).
pub fn show_window(window: WindowHandle) {
    unsafe {
        ShowWindow(window.hwnd, SW_SHOW);
        UpdateWindow(window.hwnd);
    }
}

/// Runs the message pump until the last window closes.
pub fn run() {
    let mut msg = Msg {
        hwnd: 0,
        message: 0,
        wparam: 0,
        lparam: 0,
        time: 0,
        pt: Point::default(),
    };
    unsafe {
        while GetMessageW(&mut msg, 0, 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

// MARK: - WindowHandle

/// Raw window handle — `Copy`, same thread, wrapped by the safe
/// operations below.
#[derive(Clone, Copy)]
pub struct WindowHandle {
    hwnd: Hwnd,
}

impl WindowHandle {
    /// Logical size of the content area (the layout viewport). Under a
    /// fractional DPI the size is fractional — layout takes f64.
    pub fn content_size(&self) -> (f64, f64) {
        metrics_of(self.hwnd).logical
    }

    /// The integer raster scale the engine sees (`ceil(dpi / 96)`).
    pub fn scale(&self) -> usize {
        metrics_of(self.hwnd).int_scale.max(1)
    }

    /// Presents damaged rects only: syncs the DIB backing with
    /// damage-only row copies (RGBA → BGRA in the same pass), marks
    /// each rect dirty in client pixels, and flushes synchronously —
    /// without the flush every event would show the PREVIOUS frame.
    /// `damage` is in raster pixels, top-left origin.
    pub fn blit_partial(
        &self,
        width: usize,
        height: usize,
        rgba: &[u8],
        damage: &[(i64, i64, i64, i64)],
    ) {
        if damage.is_empty() || !ensure_backing(self.hwnd, width, height) {
            return;
        }
        let full = BACKING.with(|stores| {
            let mut stores = stores.borrow_mut();
            let backing = stores.get_mut(&self.hwnd).expect("backing for the frame");
            // a fresh DIB starts blank: take everything once
            let fresh = backing.bits.is_null();
            debug_assert!(!fresh, "ensure_backing built the section");
            let bytes = unsafe {
                std::slice::from_raw_parts_mut(backing.bits, width * height * 4)
            };
            let mut whole = false;
            for &rect in damage {
                let Some((x0, y0, x1, y1)) = clamp_damage(rect, width, height) else {
                    continue;
                };
                whole |= (x1 - x0) == width && (y1 - y0) == height;
                for y in y0..y1 {
                    let from = (y * width + x0) * 4;
                    let to = (y * width + x1) * 4;
                    swizzle_row(&rgba[from..to], &mut bytes[from..to]);
                }
            }
            whole
        });
        let metrics = metrics_of(self.hwnd);
        let raster_client_match =
            (metrics.client_px.0 as usize, metrics.client_px.1 as usize) == (width, height);
        unsafe {
            if full || !raster_client_match {
                // fractional DPI (or a whole-frame wound): one stretch
                // pass repaints the union — seams cannot exist
                InvalidateRect(self.hwnd, std::ptr::null(), 0);
            } else {
                for &rect in damage {
                    let Some((x0, y0, x1, y1)) = clamp_damage(rect, width, height) else {
                        continue;
                    };
                    let client = Rect {
                        left: x0 as i32,
                        top: y0 as i32,
                        right: x1 as i32,
                        bottom: y1 as i32,
                    };
                    InvalidateRect(self.hwnd, &client, 0);
                }
            }
            UpdateWindow(self.hwnd);
        }
    }

    /// The pointer's outfit over the scene, applied now and re-applied
    /// on every `WM_SETCURSOR`.
    pub fn set_cursor(&self, cursor: Cursor) {
        let changed = CURRENT_CURSOR.with(|cell| {
            let previous = cell.replace(cursor);
            previous != cursor
        });
        if changed {
            apply_cursor(cursor);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scale_policy_rounds_up_and_keeps_the_factor() {
        assert_eq!(scale_of(96), (1, 1.0));
        assert_eq!(scale_of(120), (2, 1.25));
        assert_eq!(scale_of(144), (2, 1.5));
        assert_eq!(scale_of(168), (2, 1.75));
        assert_eq!(scale_of(192), (2, 2.0));
        assert_eq!(scale_of(288), (3, 3.0));
        // degenerate input still answers a usable scale
        assert_eq!(scale_of(0).0, 1);
    }

    #[test]
    fn the_swizzle_swaps_red_and_blue_and_forces_opaque() {
        let rgba = [10u8, 20, 30, 128, 200, 150, 100, 0];
        let mut bgra = [0u8; 8];
        swizzle_row(&rgba, &mut bgra);
        assert_eq!(bgra, [30, 20, 10, 255, 100, 150, 200, 255]);
    }

    #[test]
    fn damage_clamps_to_the_surface_and_drops_empties() {
        assert_eq!(clamp_damage((-5, -5, 4, 4), 10, 10), Some((0, 0, 4, 4)));
        assert_eq!(clamp_damage((8, 8, 200, 200), 10, 10), Some((8, 8, 10, 10)));
        assert_eq!(clamp_damage((3, 3, 3, 9), 10, 10), None);
        assert_eq!(clamp_damage((20, 0, 30, 5), 10, 10), None);
    }

    #[test]
    fn a_window_registers_creates_and_dies() {
        // headless smoke: the class registers and a real window is
        // born and destroyed without a pump
        let window = create_window("bunny test", 120.0, 90.0, false);
        let (width, height) = window.content_size();
        assert!(width > 0.0 && height > 0.0);
        assert!(window.scale() >= 1);
        unsafe {
            DestroyWindow(window.hwnd);
        }
    }

    #[test]
    fn the_backing_holds_the_swizzled_rows()
    {
        let window = create_window("bunny backing", 64.0, 64.0, false);
        let width = 8usize;
        let height = 4usize;
        let mut rgba = vec![0u8; width * height * 4];
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[1, 2, 3, 255]);
        }
        window.blit_partial(width, height, &rgba, &[(0, 0, width as i64, height as i64)]);
        BACKING.with(|stores| {
            let stores = stores.borrow();
            let backing = stores.get(&window.hwnd).expect("backing exists");
            let bytes = unsafe {
                std::slice::from_raw_parts(backing.bits, width * height * 4)
            };
            assert_eq!(&bytes[0..4], &[3, 2, 1, 255]);
        });
        unsafe {
            DestroyWindow(window.hwnd);
        }
    }
}
