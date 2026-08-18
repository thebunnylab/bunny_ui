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
    fn GetKeyState(vk: i32) -> i16;
    fn GetKeyboardState(state: *mut u8) -> i32;
    fn ToUnicode(
        vk: u32,
        scan_code: u32,
        state: *const u8,
        buffer: *mut u16,
        buffer_len: i32,
        flags: u32,
    ) -> i32;
    fn OpenClipboard(hwnd: Hwnd) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn GetClipboardData(format: u32) -> Handle;
    fn SetClipboardData(format: u32, data: Handle) -> Handle;
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
    fn ScreenToClient(hwnd: Hwnd, point: *mut Point) -> i32;
    fn ClientToScreen(hwnd: Hwnd, point: *mut Point) -> i32;
    fn MonitorFromWindow(hwnd: Hwnd, flags: u32) -> Handle;
    fn GetMonitorInfoW(monitor: Handle, info: *mut MonitorInfo) -> i32;
    fn SystemParametersInfoW(action: u32, param: u32, out: *mut c_void, update: u32) -> i32;
    fn UpdateLayeredWindow(
        hwnd: Hwnd,
        dest_dc: Hdc,
        dest_point: *const Point,
        size: *const SizePx,
        source_dc: Hdc,
        source_point: *const Point,
        color_key: u32,
        blend: *const BlendFunction,
        flags: u32,
    ) -> i32;
    fn IsWindowVisible(hwnd: Hwnd) -> i32;
}

#[repr(C)]
struct MonitorInfo {
    size: u32,
    monitor: Rect,
    work: Rect,
    flags: u32,
}

#[repr(C)]
struct SizePx {
    cx: i32,
    cy: i32,
}

#[repr(C)]
struct BlendFunction {
    op: u8,
    flags: u8,
    source_constant_alpha: u8,
    alpha_format: u8,
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
    fn GlobalAlloc(flags: u32, bytes: usize) -> Handle;
    fn GlobalLock(handle: Handle) -> *mut c_void;
    fn GlobalUnlock(handle: Handle) -> i32;
    fn GlobalFree(handle: Handle) -> Handle;
    fn Sleep(milliseconds: u32);
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
const WM_KEYDOWN: u32 = 0x0100;
const WM_CHAR: u32 = 0x0102;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_SYSCHAR: u32 = 0x0106;
const WM_UNICHAR: u32 = 0x0109;
const WM_MOVE: u32 = 0x0003;
const WM_MOUSEACTIVATE: u32 = 0x0021;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_MOUSEHWHEEL: u32 = 0x020E;
const WM_MOUSELEAVE: u32 = 0x02A3;
/// The never-activate answer to a click on a panel.
const MA_NOACTIVATE: isize = 3;
/// `SPI_GETWHEELSCROLLLINES`.
const SPI_WHEEL_LINES: u32 = 0x68;
// panel window styles
const WS_POPUP: u32 = 0x8000_0000;
const WS_EX_LAYERED: u32 = 0x0008_0000;
const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
const SW_SHOWNOACTIVATE: i32 = 4;
const SW_HIDE: i32 = 0;
const ULW_ALPHA: u32 = 2;
const AC_SRC_OVER: u8 = 0;
const AC_SRC_ALPHA: u8 = 1;
const MONITOR_DEFAULT_TO_NEAREST: u32 = 2;
const WM_ENTERSIZEMOVE: u32 = 0x0231;
const WM_EXITSIZEMOVE: u32 = 0x0232;
const WM_DPICHANGED: u32 = 0x02E0;
/// One frame-driver tick landed (posted by the driver thread).
const WM_APP_FRAME: u32 = 0x8000 + 1;
/// A task woke from another thread (posted by the wake hook).
const WM_APP_WAKE: u32 = 0x8000 + 2;
// virtual keys the shell reads directly
const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;
const VK_MENU: i32 = 0x12;
// clipboard
const CF_UNICODETEXT: u32 = 13;
const GMEM_MOVEABLE: u32 = 2;
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

pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

// MARK: - COM plumbing (shared by every COM consumer in the shell)
//
// The house pattern, translated from the mac's objc_msgSend discipline:
// per interface a `#[repr(C)]` vtable struct in header order with the
// slot indexes cited, unused runs compressed to `_pad` arrays; calls
// are plain `((*(*p).vtbl).method)(p, …)` inside small safe wrappers.
// One prohibition, learned from the platform's ABI: NEVER call a COM
// method that returns a struct by value — every method used here
// answers HRESULT or void through out-pointers.

pub(crate) type Hresult = i32;

pub(crate) fn com_ok(hr: Hresult) -> bool {
    hr >= 0
}

/// A COM identity, written literally with a comment naming it.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Guid {
    pub d1: u32,
    pub d2: u16,
    pub d3: u16,
    pub d4: [u8; 8],
}

/// The three slots every vtable starts with.
#[repr(C)]
pub(crate) struct UnknownVtbl {
    pub query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
    pub add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    pub release: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
struct Unknown {
    vtbl: *const UnknownVtbl,
}

/// A retained COM interface — released on Drop through the IUnknown
/// prefix (the owner pattern, the mac's `OwnedFont` translated).
pub(crate) struct Com<T>(std::ptr::NonNull<T>);

impl<T> Com<T> {
    pub fn from_raw(pointer: *mut T) -> Option<Com<T>> {
        std::ptr::NonNull::new(pointer).map(Com)
    }

    pub fn as_ptr(&self) -> *mut T {
        self.0.as_ptr()
    }
}

impl<T> Drop for Com<T> {
    fn drop(&mut self) {
        unsafe {
            let unknown = self.0.as_ptr() as *mut Unknown;
            (((*unknown).vtbl).read().release)(unknown as *mut c_void);
        }
    }
}

#[link(name = "ole32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *const c_void, model: u32) -> Hresult;
    pub(crate) fn CoCreateInstance(
        clsid: *const Guid,
        aggregate: *mut c_void,
        context: u32,
        iid: *const Guid,
        out: *mut *mut c_void,
    ) -> Hresult;
}

/// `CLSCTX_INPROC_SERVER`.
pub(crate) const CLSCTX_INPROC_SERVER: u32 = 1;

/// Joins the apartment — once per THREAD, the unit CoInitializeEx
/// works in (the shell is one thread; the tests are many). `S_FALSE`
/// (already in) and `RPC_E_CHANGED_MODE` (someone chose the other
/// model first — in-proc servers still work) both count as joined.
pub(crate) fn com_init() {
    thread_local! {
        static JOINED: Cell<bool> = const { Cell::new(false) };
    }
    JOINED.with(|joined| {
        if !joined.replace(true) {
            const COINIT_APARTMENTTHREADED: u32 = 0x2;
            unsafe {
                CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED);
            }
        }
    });
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
    /// Scrolling: deltas in logical points, the engine's sign
    /// (positive reveals content above). Notches convert at arrival.
    Wheel { x: f64, y: f64, dx: f64, dy: f64 },
    /// RAW editing key — only arrives when the keymap gate declined:
    /// movement, deletion, and the Ctrl chords over a focused field.
    /// `command` carries Ctrl, the platform's accelerator.
    Key { vk: u32, shift: bool, command: bool },
    /// The text road: typing, a paste of characters, the IME's final
    /// commit — surrogate halves already joined at the boundary.
    Text(String),
    /// Live composition: the marked text + the caret INSIDE it (UTF-16).
    ImeMark { text: String, caret: usize },
    /// The composition ended with what was marked still standing.
    ImeUnmark,
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
    /// A task woke from somewhere else — a worker thread finished a
    /// step. The frame the shell already knows how to draw drains the
    /// queue on its way.
    Wake,
}

// MARK: - Keyboard

/// One key press, read at the boundary: the virtual key, the live
/// modifiers, and the BASE character the key would type with a clean
/// keyboard state — the `charactersIgnoringModifiers` twin, so `Char`
/// patterns match independent of shift. `types_text` is the AltGr
/// verdict: Ctrl+Alt that produces a character IS text on this
/// platform, and the gate must let it through to the character road.
pub struct KeyStroke {
    pub vk: u32,
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub chars_ignoring: String,
    pub types_text: bool,
}

thread_local! {
    static KEY_GATE: RefCell<Option<Box<dyn FnMut(&KeyStroke) -> bool>>> =
        const { RefCell::new(None) };
}

/// Installs the keymap's first refusal. Returning `true` consumes the
/// stroke whole — no character is ever born from it, because the pump
/// only translates what the gate declined.
pub fn set_key_gate(gate: Box<dyn FnMut(&KeyStroke) -> bool>) {
    KEY_GATE.with(|slot| *slot.borrow_mut() = Some(gate));
}

/// The base character a key would type with NO modifiers held. The
/// no-mutation flag keeps the kernel's dead-key state untouched; a
/// dead key answers nothing and can never be a `Char` binding — its
/// composed text arrives later through the character road.
fn base_char(vk: u32, scan_code: u32) -> String {
    const DONT_CHANGE_STATE: u32 = 0x4;
    let clean = [0u8; 256];
    let mut buffer = [0u16; 8];
    let count = unsafe {
        ToUnicode(vk, scan_code, clean.as_ptr(), buffer.as_mut_ptr(), 8, DONT_CHANGE_STATE)
    };
    if count <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..count as usize])
}

/// Builds the stroke for one `WM_KEYDOWN`/`WM_SYSKEYDOWN`.
fn key_stroke_of(wparam: usize, lparam: isize) -> KeyStroke {
    let vk = wparam as u32;
    let scan_code = ((lparam >> 16) & 0x1FF) as u32;
    let shift = unsafe { GetKeyState(VK_SHIFT) } as u16 & 0x8000 != 0;
    let control = unsafe { GetKeyState(VK_CONTROL) } as u16 & 0x8000 != 0;
    let alt = unsafe { GetKeyState(VK_MENU) } as u16 & 0x8000 != 0;
    // the AltGr verdict needs the REAL state: does this chord type?
    let types_text = control && alt && {
        const DONT_CHANGE_STATE: u32 = 0x4;
        let mut state = [0u8; 256];
        let mut buffer = [0u16; 8];
        unsafe {
            GetKeyboardState(state.as_mut_ptr());
            ToUnicode(vk, scan_code, state.as_ptr(), buffer.as_mut_ptr(), 8, DONT_CHANGE_STATE)
                > 0
                && buffer[0] >= 0x20
        }
    };
    KeyStroke { vk, shift, control, alt, chars_ignoring: base_char(vk, scan_code), types_text }
}

// MARK: - Clipboard

/// The clipboard contends with managers on real machines: a short
/// bounded retry, then give up in silence — an unbounded wait could
/// hang the interface for a copy.
fn open_clipboard_patiently() -> bool {
    for attempt in 0..4 {
        if unsafe { OpenClipboard(0) } != 0 {
            return true;
        }
        if attempt < 3 {
            unsafe {
                Sleep(3);
            }
        }
    }
    false
}

/// Writes text to the system clipboard. On success the system owns
/// the handle; it is freed only when the hand-off failed.
pub fn clipboard_write(text: &str) {
    if !open_clipboard_patiently() {
        return;
    }
    unsafe {
        EmptyClipboard();
        let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = utf16.len() * 2;
        let handle = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if handle != 0 {
            let memory = GlobalLock(handle);
            if !memory.is_null() {
                std::ptr::copy_nonoverlapping(utf16.as_ptr(), memory as *mut u16, utf16.len());
                GlobalUnlock(handle);
                if SetClipboardData(CF_UNICODETEXT, handle) == 0 {
                    GlobalFree(handle);
                }
            } else {
                GlobalFree(handle);
            }
        }
        CloseClipboard();
    }
}

/// Reads text from the system clipboard, to the first NUL.
pub fn clipboard_read() -> Option<String> {
    if !open_clipboard_patiently() {
        return None;
    }
    let text = unsafe {
        let handle = GetClipboardData(CF_UNICODETEXT);
        if handle == 0 {
            None
        } else {
            let memory = GlobalLock(handle) as *const u16;
            if memory.is_null() {
                None
            } else {
                let mut length = 0usize;
                while *memory.add(length) != 0 {
                    length += 1;
                }
                let text =
                    String::from_utf16_lossy(std::slice::from_raw_parts(memory, length));
                GlobalUnlock(handle);
                Some(text)
            }
        }
    };
    unsafe {
        CloseClipboard();
    }
    text
}

// MARK: - IME (IMM32 — the three doors and the mirror)

#[link(name = "imm32", kind = "raw-dylib")]
unsafe extern "system" {
    fn ImmGetContext(hwnd: Hwnd) -> isize;
    fn ImmReleaseContext(hwnd: Hwnd, himc: isize) -> i32;
    fn ImmGetCompositionStringW(himc: isize, index: u32, buffer: *mut c_void, length: u32)
        -> i32;
    fn ImmSetCandidateWindow(himc: isize, form: *const CandidateForm) -> i32;
    fn ImmAssociateContext(hwnd: Hwnd, himc: isize) -> isize;
    fn ImmNotifyIME(himc: isize, action: u32, index: u32, value: u32) -> i32;
}

#[repr(C)]
struct CandidateForm {
    index: u32,
    style: u32,
    point: Point,
    area: Rect,
}

const WM_IME_STARTCOMPOSITION: u32 = 0x010D;
const WM_IME_ENDCOMPOSITION: u32 = 0x010E;
const WM_IME_COMPOSITION: u32 = 0x010F;
const WM_IME_SETCONTEXT: u32 = 0x0281;
const WM_KILLFOCUS: u32 = 0x0008;
const GCS_COMPSTR: u32 = 0x0008;
const GCS_CURSORPOS: u32 = 0x0080;
const GCS_RESULTSTR: u32 = 0x0800;
/// "Never cover this rect" — the correct semantic for a caret.
const CFS_EXCLUDE: u32 = 0x0080;
const ISC_SHOWUICOMPOSITIONWINDOW: isize = 0x8000_0000u32 as i32 as isize;
const NI_COMPOSITIONSTR: u32 = 0x0015;
const CPS_COMPLETE: u32 = 0x0001;
/// The IME consumed this keystroke — never the keymap's business.
const VK_PROCESSKEY: u32 = 0xE5;

/// What the shell knows about the focused field, synced per blit —
/// the mirror the doors read without asking the runtime mid-message.
#[derive(Default, Clone, Copy)]
struct ImeMirror {
    /// A field (or an escape hatch with ime) is focused.
    enabled: bool,
    /// A composition is live in the runtime.
    marked: bool,
    /// Where the composition starts, in UTF-16 — the candidate anchor.
    marked_start: usize,
    /// The caret rect in LAYOUT points, the fallback anchor.
    caret: (f64, f64, f64, f64),
}

thread_local! {
    static IME: Cell<ImeMirror> = Cell::new(ImeMirror::default());
    /// The context the window was born with, kept while disabled.
    static ORIGINAL_HIMC: Cell<Option<isize>> = const { Cell::new(None) };
    /// The composition-start rect resolver — answered live by the
    /// runtime, in layout points.
    static IME_RECT: RefCell<Option<Box<dyn Fn(usize) -> Option<(f64, f64, f64, f64)>>>> =
        const { RefCell::new(None) };
}

/// Installs the runtime's rect-at-index resolver (candidate placement).
pub fn set_ime_rect_resolver(
    resolver: Box<dyn Fn(usize) -> Option<(f64, f64, f64, f64)>>,
) {
    IME_RECT.with(|slot| *slot.borrow_mut() = Some(resolver));
}

/// Syncs the mirror after a blit, and keeps the window's input
/// context association honest: no field focused = the IME detached,
/// keys arrive as clean strokes for the keymap.
pub fn sync_ime(state: Option<(bool, usize, (f64, f64, f64, f64))>) {
    let hwnd = MAIN_HWND.load(Ordering::Acquire);
    let mirror = match state {
        Some((marked, marked_start, caret)) => {
            ImeMirror { enabled: true, marked, marked_start, caret }
        }
        None => ImeMirror::default(),
    };
    let was = IME.with(|cell| cell.replace(mirror));
    if hwnd == 0 || was.enabled == mirror.enabled {
        return;
    }
    if mirror.enabled {
        // give the context back
        if let Some(original) = ORIGINAL_HIMC.with(|cell| cell.take()) {
            unsafe {
                ImmAssociateContext(hwnd, original);
            }
        }
    } else {
        // detach, remembering what the window was born with
        let original = unsafe { ImmAssociateContext(hwnd, 0) };
        ORIGINAL_HIMC.with(|cell| cell.set(Some(original)));
    }
}

/// Whether a composition is live — the gate's first question.
fn ime_composing() -> bool {
    IME.with(|cell| cell.get().marked)
}

/// Places the candidate window at the composition's start (or the
/// caret), excluding the rect so the list never covers what it spells.
fn place_candidate_window(himc: isize) {
    let mirror = IME.with(|cell| cell.get());
    let rect = IME_RECT
        .with(|slot| slot.borrow().as_ref().and_then(|resolve| resolve(mirror.marked_start)))
        .unwrap_or(mirror.caret);
    let factor = shared_factor();
    let (x, y, w, h) = rect;
    let area = Rect {
        left: (x * factor).round() as i32,
        top: (y * factor).round() as i32,
        right: ((x + w) * factor).round() as i32,
        bottom: ((y + h) * factor).round() as i32,
    };
    let form = CandidateForm {
        index: 0,
        style: CFS_EXCLUDE,
        point: Point { x: area.left, y: area.bottom },
        area,
    };
    unsafe {
        ImmSetCandidateWindow(himc, &form);
    }
}

/// Reads one composition string (`GCS_COMPSTR`/`GCS_RESULTSTR`).
fn composition_string(himc: isize, kind: u32) -> Option<String> {
    unsafe {
        let bytes = ImmGetCompositionStringW(himc, kind, std::ptr::null_mut(), 0);
        if bytes < 0 {
            return None;
        }
        let units = bytes as usize / 2;
        let mut buffer = vec![0u16; units];
        if units > 0 {
            ImmGetCompositionStringW(himc, kind, buffer.as_mut_ptr() as *mut c_void, bytes as u32);
        }
        Some(String::from_utf16_lossy(&buffer))
    }
}

// MARK: - Cross-thread wake

/// The main window every cross-thread knock lands on. An atomic (not
/// a thread-local) because the signal comes from ANY thread —
/// `PostMessageW` is the thread-safe half of the pump.
static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);

/// The wake hook for `Runtime::set_wake_hook`: a worker thread asks
/// the pump for one more turn. Posted messages coalesce naturally in
/// the queue; a signal raised during a frame lands on the next turn.
pub fn wake_from_any_thread() {
    post_wake_to(MAIN_HWND.load(Ordering::Acquire));
}

fn post_wake_to(hwnd: Hwnd) {
    if hwnd != 0 {
        unsafe {
            PostMessageW(hwnd, WM_APP_WAKE, 0, 0);
        }
    }
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
    /// The windows whose leave message is armed — TrackMouseEvent is
    /// one-shot per window and re-arms on the first move after a leave.
    static LEAVE_ARMED: RefCell<std::collections::HashSet<Hwnd>> =
        RefCell::new(std::collections::HashSet::new());
    /// Where each panel's slice sits in SCENE coordinates — the
    /// translation that lets a panel's pointer events look identical
    /// to the window's, so the runtime never learns which surface the
    /// pointer touched.
    static PANEL_ORIGINS: RefCell<HashMap<Hwnd, (f64, f64)>> = RefCell::new(HashMap::new());
}

/// The scale every surface shares — panels ride the OWNER's metrics.
fn shared_factor() -> f64 {
    let main = MAIN_HWND.load(Ordering::Acquire);
    let factor = metrics_of(main).factor;
    if factor > 0.0 { factor } else { 1.0 }
}

fn scene_origin(hwnd: Hwnd) -> (f64, f64) {
    PANEL_ORIGINS.with(|origins| origins.borrow().get(&hwnd).copied().unwrap_or((0.0, 0.0)))
}

fn layout_point(hwnd: Hwnd, lparam: isize) -> (f64, f64) {
    let x = (lparam & 0xFFFF) as u16 as i16 as i32;
    let y = ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32;
    let factor = shared_factor();
    let (dx, dy) = scene_origin(hwnd);
    (x as f64 / factor + dx, y as f64 / factor + dy)
}

/// Wheel notches → logical points. `delta` is the raw wheel value
/// (±120 per notch, fractional on precision touchpads); a notch moves
/// the system's scroll-lines setting worth of ~16-point lines — the
/// same conversion the mac applies to its legacy line-tick wheels.
fn wheel_px(delta: f64, lines: f64) -> f64 {
    delta / 120.0 * lines * 16.0
}

fn wheel_lines() -> f64 {
    let mut lines: u32 = 3;
    unsafe {
        SystemParametersInfoW(SPI_WHEEL_LINES, 0, &mut lines as *mut u32 as *mut c_void, 0);
    }
    // WHEEL_PAGESCROLL (u32::MAX) means "a page": keep the default
    if lines == 0 || lines == u32::MAX { 3.0 } else { lines as f64 }
}

unsafe extern "system" fn window_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_CREATE => 0,
        WM_SIZE => {
            // ONLY the main window has a size of its own: a panel is
            // sized by the present that created it, and its birth
            // arrives here SYNCHRONOUSLY from inside that present — a
            // dispatch would re-enter the handler mid-frame
            if hwnd != MAIN_HWND.load(Ordering::Acquire) || wparam == SIZE_MINIMIZED {
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
            if hwnd != MAIN_HWND.load(Ordering::Acquire) {
                // layered panels are raw screen pixels — the owner's
                // funnel rescales them on its next present
                return 0;
            }
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
            if LEAVE_ARMED.with(|armed| armed.borrow_mut().insert(hwnd)) {
                let mut track = TrackMouseEventArgs {
                    size: std::mem::size_of::<TrackMouseEventArgs>() as u32,
                    flags: TME_LEAVE,
                    hwnd,
                    hover_time: 0,
                };
                unsafe {
                    TrackMouseEvent(&mut track);
                }
            }
            let (x, y) = layout_point(hwnd, lparam);
            dispatch(AppEvent::MouseMoved { x, y });
            0
        }
        WM_MOUSELEAVE => {
            LEAVE_ARMED.with(|armed| armed.borrow_mut().remove(&hwnd));
            dispatch(AppEvent::MouseExited);
            0
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            // wheel positions arrive in SCREEN pixels, unlike every
            // other mouse message
            let mut point = Point {
                x: (lparam & 0xFFFF) as u16 as i16 as i32,
                y: ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32,
            };
            unsafe {
                ScreenToClient(hwnd, &mut point);
            }
            let factor = shared_factor();
            let (ox, oy) = scene_origin(hwnd);
            let x = point.x as f64 / factor + ox;
            let y = point.y as f64 / factor + oy;
            let delta = ((wparam >> 16) & 0xFFFF) as u16 as i16 as f64;
            let px = wheel_px(delta, wheel_lines());
            // vertical matches the engine's sign (positive reveals
            // content above); the tilt wheel flips, like the web's dx
            let (dx, dy) =
                if msg == WM_MOUSEWHEEL { (0.0, px) } else { (-px, 0.0) };
            dispatch(AppEvent::Wheel { x, y, dx, dy });
            0
        }
        WM_MOUSEACTIVATE => {
            if hwnd != MAIN_HWND.load(Ordering::Acquire) {
                // a panel never takes the keyboard — the second belt
                // beside WS_EX_NOACTIVATE
                return MA_NOACTIVATE;
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_MOVE => {
            if hwnd == MAIN_HWND.load(Ordering::Acquire) {
                // owned panels do not ride the owner on their own —
                // the present repositions them, so a caption drag asks
                // for one
                dispatch(AppEvent::Redraw);
            }
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
        WM_KEYDOWN => {
            if wparam as u32 == VK_PROCESSKEY {
                // the IME owns this stroke — its message machinery
                // needs the default road
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
            // the gate already declined this stroke in the pump — what
            // arrives here is the editing vocabulary and the Ctrl chords
            let shift = unsafe { GetKeyState(VK_SHIFT) } as u16 & 0x8000 != 0;
            let control = unsafe { GetKeyState(VK_CONTROL) } as u16 & 0x8000 != 0;
            dispatch(AppEvent::Key { vk: wparam as u32, shift, command: control });
            0
        }
        WM_IME_SETCONTEXT => {
            // the belt: the IME draws no composition window of its own
            let lparam = lparam & !ISC_SHOWUICOMPOSITIONWINDOW;
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_IME_STARTCOMPOSITION => {
            // the suspenders: Def would create the default composition
            // window — the scene draws marked text inline instead
            let himc = unsafe { ImmGetContext(hwnd) };
            if himc != 0 {
                place_candidate_window(himc);
                unsafe {
                    ImmReleaseContext(hwnd, himc);
                }
            }
            0
        }
        WM_IME_COMPOSITION => {
            let himc = unsafe { ImmGetContext(hwnd) };
            if himc == 0 {
                return 0;
            }
            // the order rule: RESULT first, then the fresh composition —
            // a commit and the next syllable ride one message in
            // Japanese and Korean
            if (lparam as u32) & GCS_RESULTSTR != 0 {
                if let Some(text) = composition_string(himc, GCS_RESULTSTR) {
                    if !text.is_empty() {
                        dispatch(AppEvent::Text(text));
                    }
                }
            }
            if (lparam as u32) & GCS_COMPSTR != 0 {
                if let Some(text) = composition_string(himc, GCS_COMPSTR) {
                    let caret =
                        unsafe { ImmGetCompositionStringW(himc, GCS_CURSORPOS, std::ptr::null_mut(), 0) };
                    dispatch(AppEvent::ImeMark { text, caret: caret.max(0) as usize });
                    place_candidate_window(himc);
                }
            }
            unsafe {
                ImmReleaseContext(hwnd, himc);
            }
            // never Def: it would synthesize WM_IME_CHAR duplicates
            0
        }
        WM_IME_ENDCOMPOSITION => {
            // unmark ONLY if the runtime still holds marked text: after
            // a commit the result already cleared it, and a second
            // clearing would erase real input
            if ime_composing() {
                dispatch(AppEvent::ImeUnmark);
            }
            0
        }
        WM_KILLFOCUS => {
            // the platform's manner: focus leaving mid-composition
            // commits what stands — the commit flows back through the
            // composition door as a result
            if ime_composing() {
                let himc = unsafe { ImmGetContext(hwnd) };
                if himc != 0 {
                    unsafe {
                        ImmNotifyIME(himc, NI_COMPOSITIONSTR, CPS_COMPLETE, 0);
                        ImmReleaseContext(hwnd, himc);
                    }
                }
            }
            0
        }
        WM_CHAR => {
            thread_local! {
                /// The high half of a surrogate pair waits for its twin.
                static PENDING_HIGH: Cell<Option<u16>> = const { Cell::new(None) };
            }
            let unit = wparam as u16;
            match unit {
                0xD800..=0xDBFF => {
                    PENDING_HIGH.with(|cell| cell.set(Some(unit)));
                }
                0xDC00..=0xDFFF => {
                    if let Some(high) = PENDING_HIGH.with(|cell| cell.take()) {
                        let text = String::from_utf16_lossy(&[high, unit]);
                        dispatch(AppEvent::Text(text));
                    }
                    // a lone low half drops — it spells nothing
                }
                _ => {
                    PENDING_HIGH.with(|cell| cell.take());
                    // control characters take the Key road, never this one
                    if unit >= 0x20 && unit != 0x7F {
                        dispatch(AppEvent::Text(
                            char::from_u32(unit as u32).map(String::from).unwrap_or_default(),
                        ));
                    }
                }
            }
            0
        }
        WM_UNICHAR => {
            const UNICODE_NOCHAR: usize = 0xFFFF;
            if wparam == UNICODE_NOCHAR {
                // the probe: answer that the road exists
                return 1;
            }
            if let Some(text) = char::from_u32(wparam as u32) {
                if !text.is_control() {
                    dispatch(AppEvent::Text(text.to_string()));
                }
            }
            0
        }
        WM_SYSCHAR => {
            // no menu bar to ring: a consumed Alt chord stays silent
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
        WM_APP_WAKE => {
            dispatch(AppEvent::Wake);
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
            PANEL_ORIGINS.with(|origins| origins.borrow_mut().remove(&hwnd));
            LEAVE_ARMED.with(|armed| armed.borrow_mut().remove(&hwnd));
            // closing the MAIN window quits — a panel dies in silence
            if hwnd == MAIN_HWND.load(Ordering::Acquire) {
                unsafe {
                    PostQuitMessage(0);
                }
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
    MAIN_HWND.store(hwnd, Ordering::Release);
    WindowHandle { hwnd }
}

/// Shows the window after the first present (the anti-flash order).
pub fn show_window(window: WindowHandle) {
    unsafe {
        ShowWindow(window.hwnd, SW_SHOW);
        UpdateWindow(window.hwnd);
    }
}

/// Runs the message pump until the last window closes. The pump owns
/// the keymap's first refusal: a key the gate consumes is never
/// translated, so no character is ever born from it — the
/// consumed-flag bug cannot exist by construction. `WM_SYSKEYDOWN`
/// passes the gate too (every Alt chord is a system key), and the
/// declined ones still reach `DefWindowProc` for the platform's own
/// chords (Alt+F4 closes).
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
            let gated = (msg.message == WM_KEYDOWN || msg.message == WM_SYSKEYDOWN)
                // a live composition owns its keys (Esc closes the
                // candidates, arrows walk the clauses), and a stroke
                // the IME consumed was never the keymap's to take
                && msg.wparam as u32 != VK_PROCESSKEY
                && !ime_composing()
                && {
                    let stroke = key_stroke_of(msg.wparam, msg.lparam);
                    KEY_GATE.with(|slot| {
                        slot.borrow_mut().as_mut().is_some_and(|gate| gate(&stroke))
                    })
                };
            if !gated {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
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

    /// A rect in LAYOUT coordinates converted to SCREEN pixels — where
    /// a panel (and one day the IME candidate window) lands.
    pub fn layout_rect_to_screen(&self, x: f64, y: f64, width: f64, height: f64) -> Rect {
        let factor = shared_factor();
        let mut origin = Point { x: 0, y: 0 };
        unsafe {
            ClientToScreen(self.hwnd, &mut origin);
        }
        let left = origin.x + (x * factor).round() as i32;
        let top = origin.y + (y * factor).round() as i32;
        Rect {
            left,
            top,
            right: left + (width * factor).round() as i32,
            bottom: top + (height * factor).round() as i32,
        }
    }

    /// The monitor's WORK area (the taskbar excluded) in this window's
    /// layout coordinates — left of or above the window comes out
    /// negative. What popovers clamp against.
    pub fn screen_bounds_in_layout(&self) -> Option<(f64, f64, f64, f64)> {
        let factor = shared_factor();
        unsafe {
            let monitor = MonitorFromWindow(self.hwnd, MONITOR_DEFAULT_TO_NEAREST);
            if monitor == 0 {
                return None;
            }
            let mut info = MonitorInfo {
                size: std::mem::size_of::<MonitorInfo>() as u32,
                monitor: Rect::default(),
                work: Rect::default(),
                flags: 0,
            };
            if GetMonitorInfoW(monitor, &mut info) == 0 {
                return None;
            }
            let mut origin = Point { x: 0, y: 0 };
            ClientToScreen(self.hwnd, &mut origin);
            Some((
                (info.work.left - origin.x) as f64 / factor,
                (info.work.top - origin.y) as f64 / factor,
                (info.work.right - info.work.left) as f64 / factor,
                (info.work.bottom - info.work.top) as f64 / factor,
            ))
        }
    }

    /// Registers where this panel's slice sits in the scene, so its
    /// pointer events translate back on arrival.
    pub fn set_scene_origin(&self, x: f64, y: f64) {
        PANEL_ORIGINS.with(|origins| {
            origins.borrow_mut().insert(self.hwnd, (x, y));
        });
    }

    /// Presents a panel whole: position, size and pixels land in ONE
    /// atomic call. Takes the scene's STRAIGHT rgba and premultiplies
    /// into BGRA during the copy — the platform's per-pixel-alpha
    /// window wants exactly that.
    pub fn present_layered(&self, screen: Rect, width: usize, height: usize, rgba: &[u8]) {
        if width == 0 || height == 0 || !ensure_backing(self.hwnd, width, height) {
            return;
        }
        BACKING.with(|stores| {
            let mut stores = stores.borrow_mut();
            let backing = stores.get_mut(&self.hwnd).expect("backing for the panel");
            let bytes =
                unsafe { std::slice::from_raw_parts_mut(backing.bits, width * height * 4) };
            for (source, dest) in rgba.chunks_exact(4).zip(bytes.chunks_exact_mut(4)) {
                let alpha = source[3] as u32;
                dest[0] = (source[2] as u32 * alpha / 255) as u8;
                dest[1] = (source[1] as u32 * alpha / 255) as u8;
                dest[2] = (source[0] as u32 * alpha / 255) as u8;
                dest[3] = source[3];
            }
            let position = Point { x: screen.left, y: screen.top };
            let size = SizePx { cx: width as i32, cy: height as i32 };
            let source = Point { x: 0, y: 0 };
            let blend = BlendFunction {
                op: AC_SRC_OVER,
                flags: 0,
                source_constant_alpha: 255,
                alpha_format: AC_SRC_ALPHA,
            };
            unsafe {
                UpdateLayeredWindow(
                    self.hwnd,
                    0,
                    &position,
                    &size,
                    backing.dc,
                    &source,
                    0,
                    &blend,
                    ULW_ALPHA,
                );
                if IsWindowVisible(self.hwnd) == 0 {
                    ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                }
            }
        });
    }

    /// Retires a panel: hidden, forgotten, destroyed.
    pub fn close_panel(&self) {
        unsafe {
            ShowWindow(self.hwnd, SW_HIDE);
            DestroyWindow(self.hwnd);
        }
    }
}

/// A borderless never-activate panel owned by the window — the surface
/// a popover, tooltip, menu or drag chip leaves the window on. Shares
/// the window class (and so the pointer road); takes no timer, no
/// driver, no keyboard — the parent drives every frame.
pub fn create_panel(owner: &WindowHandle) -> WindowHandle {
    let class_name = register_class();
    let title = wide("");
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            owner.hwnd,
            0,
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        )
    };
    assert!(hwnd != 0, "the platform refused the panel");
    WindowHandle { hwnd }
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
    fn wheel_notches_become_scroll_lines_in_points() {
        // one notch, three lines of ~16 points — the platform default
        assert_eq!(wheel_px(120.0, 3.0), 48.0);
        assert_eq!(wheel_px(-120.0, 3.0), -48.0);
        // precision touchpads send fractions of a notch
        assert_eq!(wheel_px(30.0, 3.0), 12.0);
        // a taller system setting scrolls farther
        assert_eq!(wheel_px(120.0, 5.0), 80.0);
    }

    #[test]
    fn a_layout_rect_lands_on_screen_and_comes_back() {
        let window = create_window("bunny screen", 200.0, 150.0, false);
        MAIN_HWND.store(window.hwnd, Ordering::Release);
        let factor = shared_factor();
        let rect = window.layout_rect_to_screen(10.0, 20.0, 30.0, 40.0);
        assert_eq!(rect.right - rect.left, (30.0 * factor).round() as i32);
        assert_eq!(rect.bottom - rect.top, (40.0 * factor).round() as i32);
        // the work area exists and holds a positive size
        let (_, _, work_w, work_h) = window.screen_bounds_in_layout().expect("a monitor");
        assert!(work_w > 100.0 && work_h > 100.0);
        unsafe {
            DestroyWindow(window.hwnd);
        }
    }

    #[test]
    fn a_panel_translates_its_events_into_the_scene() {
        let window = create_window("bunny panel", 100.0, 80.0, false);
        MAIN_HWND.store(window.hwnd, Ordering::Release);
        let panel = create_panel(&window);
        panel.set_scene_origin(300.0, -20.0);
        let factor = shared_factor();
        // a client point on the panel reads as scene coordinates
        let lparam = ((10.0 * factor) as isize) | (((8.0 * factor) as isize) << 16);
        let (x, y) = layout_point(panel.hwnd, lparam);
        assert!((x - 310.0).abs() < 1.0, "x lands in the scene: {x}");
        assert!((y - (-12.0)).abs() < 1.0, "y lands in the scene: {y}");
        panel.close_panel();
        unsafe {
            DestroyWindow(window.hwnd);
        }
    }

    #[test]
    fn the_layered_copy_premultiplies_into_bgra() {
        let window = create_window("bunny layered", 60.0, 40.0, false);
        let panel = create_panel(&window);
        // one half-transparent red pixel: premultiplied BGRA
        panel.present_layered(
            Rect { left: 0, top: 0, right: 2, bottom: 1 },
            2,
            1,
            &[255, 0, 0, 128, 0, 255, 0, 255],
        );
        BACKING.with(|stores| {
            let stores = stores.borrow();
            let backing = stores.get(&panel.hwnd).expect("panel backing");
            let bytes = unsafe { std::slice::from_raw_parts(backing.bits, 8) };
            assert_eq!(&bytes[0..4], &[0, 0, 128, 128], "premultiplied red, BGRA order");
            assert_eq!(&bytes[4..8], &[0, 255, 0, 255], "opaque green stays whole");
        });
        panel.close_panel();
        unsafe {
            DestroyWindow(window.hwnd);
        }
    }

    #[test]
    fn the_clipboard_round_trips_and_gives_back() {
        // this is a dev machine: save what the user had, restore after
        let before = clipboard_read();
        clipboard_write("bunny raro \u{1F407}");
        assert_eq!(clipboard_read().as_deref(), Some("bunny raro \u{1F407}"));
        if let Some(previous) = before {
            clipboard_write(&previous);
        }
    }

    #[test]
    fn a_wake_crosses_threads_into_the_pump() {
        let window = create_window("bunny wake", 80.0, 60.0, false);
        let hwnd = window.hwnd;
        let handle = std::thread::spawn(move || {
            // the thread-safe half of the pump, from another thread
            post_wake_to(hwnd);
        });
        handle.join().expect("the poster returns");
        // one blocking read: the posted message IS the next message
        let mut msg = Msg {
            hwnd: 0,
            message: 0,
            wparam: 0,
            lparam: 0,
            time: 0,
            pt: Point::default(),
        };
        let got = unsafe { GetMessageW(&mut msg, hwnd, WM_APP_WAKE, WM_APP_WAKE) };
        assert!(got > 0, "the pump observed a message");
        assert_eq!(msg.message, WM_APP_WAKE);
        unsafe {
            DestroyWindow(hwnd);
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
