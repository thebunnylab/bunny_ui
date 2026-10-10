//! UIAutomationCore.h and OleAuto.h declarations used by the native adapter.
use super::super::{Guid, Hresult, Hwnd, UnknownVtbl};
use std::ffi::c_void;
pub(super) type Object = *mut c_void;
pub(super) type SafeArray = c_void;

pub(super) const OK: Hresult = 0;
pub(super) const NO_INTERFACE: Hresult = 0x80004002u32 as i32;
pub(super) const POINTER: Hresult = 0x80004003u32 as i32;
pub(super) const FAILED: Hresult = 0x80004005u32 as i32;
pub(super) const INVALID: Hresult = 0x80070057u32 as i32;
pub(super) const NO_MEMORY: Hresult = 0x8007000eu32 as i32;
pub(super) const DENIED: Hresult = 0x80070005u32 as i32;
pub(super) const UNAVAILABLE: Hresult = 0x80040201u32 as i32;
pub(super) const UNSUPPORTED: Hresult = 0x80040204u32 as i32;

const fn guid(value: u128) -> Guid {
    Guid {
        d1: (value >> 96) as u32,
        d2: (value >> 80) as u16,
        d3: (value >> 64) as u16,
        d4: (value as u64).to_be_bytes(),
    }
}
pub(super) const UNKNOWN: Guid = guid(0x00000000_0000_0000_c000_000000000046);
pub(super) const SIMPLE: Guid = guid(0xd6dd68d1_86fd_4332_8666_9abedea2d24c);
pub(super) const FRAGMENT: Guid = guid(0xf7063da8_8359_439c_9297_bbc5299a7d87);
pub(super) const ROOT: Guid = guid(0x620ce2a5_ab8f_40a9_86cb_de3c75599b58);
pub(super) const INVOKE: Guid = guid(0x54fcb24b_e18e_47a2_b4d3_eccbe77599a2);
pub(super) const VALUE: Guid = guid(0xc7935180_6fb3_4201_b174_7df73adbf64a);

pub(super) const BOUNDS: i32 = 30001;
pub(super) const CONTROL_TYPE: i32 = 30003;
pub(super) const NAME: i32 = 30005;
pub(super) const FOCUSED: i32 = 30008;
pub(super) const FOCUSABLE: i32 = 30009;
pub(super) const ENABLED: i32 = 30010;
pub(super) const AUTOMATION_ID: i32 = 30011;
pub(super) const CONTROL: i32 = 30016;
pub(super) const CONTENT: i32 = 30017;
pub(super) const PASSWORD: i32 = 30019;
pub(super) const OFFSCREEN: i32 = 30022;
pub(super) const FRAMEWORK: i32 = 30024;
pub(super) const VALUE_VALUE: i32 = 30045;
pub(super) const VALUE_READ_ONLY: i32 = 30046;
pub(super) const INVOKE_PATTERN: i32 = 10000;
pub(super) const VALUE_PATTERN: i32 = 10002;
pub(super) const FOCUS_EVENT: i32 = 20005;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub(super) struct UiaRect {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) union ValueData {
    pub pointer: Object,
    pub integer: i32,
    pub boolean: i16,
    pub real: f64,
    // VARIANT's BRECORD arm determines its size on both pointer widths.
    pub record: [Object; 2],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Variant {
    pub kind: u16,
    reserved: [u16; 3],
    pub data: ValueData,
}
impl Variant {
    pub const fn empty() -> Self {
        Self {
            kind: 0,
            reserved: [0; 3],
            data: ValueData {
                record: [std::ptr::null_mut(); 2],
            },
        }
    }
    pub fn integer(value: i32) -> Self {
        Self {
            kind: 3,
            data: ValueData { integer: value },
            ..Self::empty()
        }
    }
    pub fn boolean(value: bool) -> Self {
        Self {
            kind: 11,
            data: ValueData {
                boolean: if value { -1 } else { 0 },
            },
            ..Self::empty()
        }
    }
    pub fn string(value: &str) -> Result<Self, Hresult> {
        let words: Vec<_> = value.encode_utf16().collect();
        let length = u32::try_from(words.len()).map_err(|_| NO_MEMORY)?;
        let pointer = unsafe { SysAllocStringLen(words.as_ptr(), length) };
        if pointer.is_null() {
            return Err(NO_MEMORY);
        }
        Ok(Self {
            kind: 8,
            data: ValueData {
                pointer: pointer.cast(),
            },
            ..Self::empty()
        })
    }
    pub fn bounds(rect: UiaRect) -> Result<Self, Hresult> {
        let array = unsafe { SafeArrayCreateVector(5, 0, 4) }; // VT_R8
        if array.is_null() {
            return Err(NO_MEMORY);
        }
        for (index, mut value) in [rect.left, rect.top, rect.width, rect.height]
            .into_iter()
            .enumerate()
        {
            let result = unsafe {
                SafeArrayPutElement(array, &(index as i32), (&mut value as *mut f64).cast())
            };
            if result < 0 {
                unsafe {
                    SafeArrayDestroy(array);
                }
                return Err(result);
            }
        }
        Ok(Self {
            kind: 0x2000 | 5,
            data: ValueData { pointer: array },
            ..Self::empty()
        })
    }
}

#[repr(C)]
pub(super) struct SimpleVtbl {
    pub unknown: UnknownVtbl,
    pub options: unsafe extern "system" fn(Object, *mut i32) -> Hresult,
    pub pattern: unsafe extern "system" fn(Object, i32, *mut Object) -> Hresult,
    pub property: unsafe extern "system" fn(Object, i32, *mut Variant) -> Hresult,
    pub host: unsafe extern "system" fn(Object, *mut Object) -> Hresult,
}
#[repr(C)]
pub(super) struct FragmentVtbl {
    pub unknown: UnknownVtbl,
    pub navigate: unsafe extern "system" fn(Object, i32, *mut Object) -> Hresult,
    pub runtime_id: unsafe extern "system" fn(Object, *mut *mut SafeArray) -> Hresult,
    pub bounds: unsafe extern "system" fn(Object, *mut UiaRect) -> Hresult,
    pub embedded: unsafe extern "system" fn(Object, *mut *mut SafeArray) -> Hresult,
    pub focus: unsafe extern "system" fn(Object) -> Hresult,
    pub root: unsafe extern "system" fn(Object, *mut Object) -> Hresult,
}
#[repr(C)]
pub(super) struct RootVtbl {
    pub unknown: UnknownVtbl,
    pub at_point: unsafe extern "system" fn(Object, f64, f64, *mut Object) -> Hresult,
    pub focused: unsafe extern "system" fn(Object, *mut Object) -> Hresult,
}
#[repr(C)]
pub(super) struct InvokeVtbl {
    pub unknown: UnknownVtbl,
    pub invoke: unsafe extern "system" fn(Object) -> Hresult,
}
#[repr(C)]
pub(super) struct ValueVtbl {
    pub unknown: UnknownVtbl,
    pub set: unsafe extern "system" fn(Object, *const u16) -> Hresult,
    pub get: unsafe extern "system" fn(Object, *mut *mut u16) -> Hresult,
    pub read_only: unsafe extern "system" fn(Object, *mut i32) -> Hresult,
}

#[link(name = "uiautomationcore", kind = "raw-dylib")]
unsafe extern "system" {
    pub(super) fn UiaReturnRawElementProvider(
        window: Hwnd,
        wparam: usize,
        lparam: isize,
        provider: Object,
    ) -> isize;
    pub(super) fn UiaHostProviderFromHwnd(window: Hwnd, out: *mut Object) -> Hresult;
    pub(super) fn UiaDisconnectProvider(provider: Object) -> Hresult;
    pub(super) fn UiaClientsAreListening() -> i32;
    pub(super) fn UiaRaiseAutomationEvent(provider: Object, event: i32) -> Hresult;
    pub(super) fn UiaRaiseAutomationPropertyChangedEvent(
        provider: Object,
        property: i32,
        old: Variant,
        new: Variant,
    ) -> Hresult;
    pub(super) fn UiaRaiseStructureChangedEvent(
        provider: Object,
        change: i32,
        runtime_id: *const i32,
        length: i32,
    ) -> Hresult;
}
#[link(name = "oleaut32", kind = "raw-dylib")]
unsafe extern "system" {
    pub(super) fn SysAllocStringLen(text: *const u16, length: u32) -> *mut u16;
    pub(super) fn VariantClear(value: *mut Variant) -> Hresult;
    pub(super) fn SafeArrayCreateVector(kind: u16, lower: i32, elements: u32) -> *mut SafeArray;
    pub(super) fn SafeArrayPutElement(
        array: *mut SafeArray,
        indices: *const i32,
        value: Object,
    ) -> Hresult;
    pub(super) fn SafeArrayDestroy(array: *mut SafeArray) -> Hresult;
}

#[repr(C)]
struct GuiThreadInfo {
    size: u32,
    flags: u32,
    active: Hwnd,
    focus: Hwnd,
    capture: Hwnd,
    menu_owner: Hwnd,
    move_size: Hwnd,
    caret: Hwnd,
    caret_rect: super::super::Rect,
}
#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetWindowThreadProcessId(window: Hwnd, process: *mut u32) -> u32;
    fn GetGUIThreadInfo(thread: u32, info: *mut GuiThreadInfo) -> i32;
}
/// GetFocus alone reads the CALLER's thread; a COM callback may be on an MTA.
pub(super) fn window_has_focus(window: Hwnd) -> bool {
    let thread = unsafe { GetWindowThreadProcessId(window, std::ptr::null_mut()) };
    if thread == 0 {
        return false;
    }
    let mut info = GuiThreadInfo {
        size: std::mem::size_of::<GuiThreadInfo>() as u32,
        flags: 0,
        active: 0,
        focus: 0,
        capture: 0,
        menu_owner: 0,
        move_size: 0,
        caret: 0,
        caret_rect: super::super::Rect::default(),
    };
    unsafe { GetGUIThreadInfo(thread, &mut info) != 0 && info.focus == window }
}
