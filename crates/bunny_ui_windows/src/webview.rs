//! The webview tenant: WebView2 behind the native host.
//!
//! The OS already ships a browser engine; this module mounts it in
//! the hole the layout keeps (`docs/webview.md`). The engine draws,
//! scrolls and reads input itself — the shell finds the runtime,
//! stands the environment, parents a controller into the host's
//! container, points it at a url and listens. Zero crates and zero
//! bundled bytes: the Evergreen runtime is discovered by registry and
//! spoken to through hand-written COM vtables, verified against SDK
//! 1.0.3650.58 — the slot-count tests below are the guard.
//!
//! Three shapes differ from the mac twin and each is named where it
//! lands: creation is ASYNC (an environment and a controller each
//! answer on a later pump turn — commands queue in arrival order and
//! drain at land), the four named message channels become ONE
//! `WebMessageReceived` wire with a tab-prefix envelope, and console
//! and requests are served NATIVELY (the DevTools Protocol and the
//! response-received event) instead of by injected hooks — richer
//! than the wrap: subresources included, capture with no injection
//! race.
//!
//! Nothing here runs off the shell's one thread: WebView2 is
//! apartment-threaded and answers on the thread that created the
//! environment, through the message pump the shell already runs.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;

use bunny_ui::action::Modifiers;
use bunny_ui::host::{
    Document, EDITOR_SCRIPT, EditorAction, EditorReport, HostSpec, MouseButton, WebviewCapability,
    WebviewInput, editor_report,
};

use crate::ffi::{Guid, Hresult, Hwnd, Rect, UnknownVtbl, com_init, com_ok, com_query, wide};

/// What this backend serves, as the value the app reads
/// (`docs/webview.md` — the capability table's WebView2 column):
/// console and requests NATIVELY, synthetic input by the browser's
/// own input pipeline — and no response bodies yet, because core has
/// no door a body could travel through (the engine-side road exists;
/// declaring the cell with no door would be an empty answer).
pub fn capabilities() -> &'static [WebviewCapability] {
    &[
        WebviewCapability::ConsoleMessages,
        WebviewCapability::NetworkRequests,
        WebviewCapability::SyntheticInput,
        WebviewCapability::MediaEmulation,
        WebviewCapability::HtmlEditor,
    ]
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LoadLibraryExW(name: *const u16, file: isize, flags: u32) -> isize;
    fn GetProcAddress(module: isize, name: *const u8) -> *const c_void;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegGetValueW(
        key: isize,
        subkey: *const u16,
        value: *const u16,
        flags: u32,
        kind: *mut u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> i32;
}

#[link(name = "ole32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CoTaskMemFree(pointer: *mut c_void);
}

// MARK: - What the page sends back

/// What the engine reported — delivered to the shell's dispatch, on
/// the main thread, from the pump's own turns. The mac twin carries
/// the view; here every handler is authored by us and carries its
/// placement path instead, so no pointer scan exists.
/// Clone because an app with more than one window offers a page's news
/// to each of them — the one that owns the page answers.
#[derive(Clone)]
pub(crate) enum WebviewEvent {
    /// The engine committed a navigation — link clicks included.
    Navigated { path: String, url: String },
    /// A link in a DOCUMENT was activated. The engine did not follow
    /// it: the document stays, and the app hears the url.
    Linked { path: String, url: String },
    /// An editable document's body changed under the person's hand.
    Changed { path: String, html: String },
    /// A paste the app owns: the clipboard's html and text, nothing
    /// inserted.
    Pasted { path: String, html: String, text: String },
    /// The engine REFUSED one: the url it tried, and why — the other
    /// leg of the same pair, so no load ends in silence.
    NavigationFailed { path: String, url: String, why: String },
    /// The page called `window.bunny.post(…)`.
    Posted { path: String, body: String },
    /// The page's console spoke — `"level: what it said"`.
    Console { path: String, line: String },
    /// A request of the page's completed — `"METHOD url status"`.
    Requested { path: String, line: String },
    /// An eval answered, by token — `Ok` is JSON, `Err` the thrown
    /// error's name.
    EvalDone { token: u64, result: Result<String, String> },
    /// A snapshot answered, by token — straight RGBA, tightly packed.
    SnapshotDone { token: u64, result: Result<(usize, usize, Vec<u8>), String> },
    /// The page took the keyboard — a click landed in the island. The
    /// shell dismisses what a click beside a popover would dismiss.
    FocusTaken,
}

thread_local! {
    /// Where the engine's callbacks land — the shell installs it at
    /// window start. Taken out while it runs, the way the app handler
    /// is: a re-entrant callback finds the slot empty instead of a
    /// borrow panic.
    static DISPATCH: RefCell<Option<Box<dyn Fn(WebviewEvent)>>> = const { RefCell::new(None) };
}

/// The shell installs the landing spot for everything a page reports.
pub(crate) fn set_dispatch(dispatch: impl Fn(WebviewEvent) + 'static) {
    DISPATCH.with(|slot| *slot.borrow_mut() = Some(Box::new(dispatch)));
}

fn dispatch(event: WebviewEvent) {
    let Some(handler) = DISPATCH.with(|slot| slot.borrow_mut().take()) else {
        return;
    };
    handler(event);
    DISPATCH.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(handler);
        }
    });
}

// MARK: - Finding the runtime (no bundled DLL, no build step)

/// `LOAD_WITH_ALTERED_SEARCH_PATH` — the client DLL's own-directory
/// imports resolve beside it.
const ALTERED_SEARCH: u32 = 0x8;
/// The registry pseudo-handles, sign-extended the way the API reads
/// them.
const HKLM: isize = 0x8000_0002u32 as i32 as isize;
const HKCU: isize = 0x8000_0001u32 as i32 as isize;
/// `RRF_RT_REG_SZ`.
const REG_SZ_ONLY: u32 = 0x2;

/// The EdgeUpdate client id of the WebView2 Evergreen runtime.
const RUNTIME_CLIENT: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

/// One REG_SZ, read twice (size, then bytes) — `None` is a miss, and
/// a miss walks on to the next rung.
fn reg_string(root: isize, subkey: &str, value: &str) -> Option<String> {
    let subkey = wide(subkey);
    let value = wide(value);
    let mut size = 0u32;
    let hit = unsafe {
        RegGetValueW(
            root,
            subkey.as_ptr(),
            value.as_ptr(),
            REG_SZ_ONLY,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if hit != 0 || size < 2 {
        return None;
    }
    let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
    let hit = unsafe {
        RegGetValueW(
            root,
            subkey.as_ptr(),
            value.as_ptr(),
            REG_SZ_ONLY,
            std::ptr::null_mut(),
            buffer.as_mut_ptr() as *mut c_void,
            &mut size,
        )
    };
    if hit != 0 {
        return None;
    }
    let text: String =
        String::from_utf16_lossy(&buffer[..buffer.iter().position(|&c| c == 0).unwrap_or(0)]);
    if text.is_empty() { None } else { Some(text) }
}

/// The architecture folder the runtime keeps its client DLL under.
const fn arch_dir() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86"
    }
}

/// The client DLL under a runtime folder — pure, so the ladder's
/// joinery is testable with no runtime installed.
fn client_dll_under(folder: &str, arch: &str) -> String {
    let base = folder.trim_end_matches('\\');
    format!("{base}\\EBWebView\\{arch}\\EmbeddedBrowserWebView.dll")
}

/// Every place the Evergreen client DLL could be, in trust order:
/// the documented env override first, then the EdgeUpdate registry —
/// `ClientState`'s `EBWebView` names the version folder outright, and
/// `Clients`' `location` + `pv` join to the same place.
fn client_dll_candidates() -> Vec<String> {
    let arch = arch_dir();
    let mut candidates = Vec::new();
    if let Ok(folder) = std::env::var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER")
        && !folder.is_empty()
    {
        // a fixed-version folder holds the DLL beside the browser or
        // under the same EBWebView tree the Evergreen install uses
        candidates.push(client_dll_under(&folder, arch));
        candidates
            .push(format!("{}\\EmbeddedBrowserWebView.dll", folder.trim_end_matches('\\')));
        return candidates;
    }
    const ROOTS: [(isize, &str); 3] = [
        (HKLM, "SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate"),
        (HKLM, "SOFTWARE\\Microsoft\\EdgeUpdate"),
        (HKCU, "SOFTWARE\\Microsoft\\EdgeUpdate"),
    ];
    for (root, base) in ROOTS {
        if let Some(version_dir) =
            reg_string(root, &format!("{base}\\ClientState\\{RUNTIME_CLIENT}"), "EBWebView")
        {
            candidates.push(client_dll_under(&version_dir, arch));
        }
        if let (Some(location), Some(version)) = (
            reg_string(root, &format!("{base}\\Clients\\{RUNTIME_CLIENT}"), "location"),
            reg_string(root, &format!("{base}\\Clients\\{RUNTIME_CLIENT}"), "pv"),
        ) {
            candidates
                .push(client_dll_under(&format!("{}\\{version}", location.trim_end_matches('\\')), arch));
        }
    }
    candidates
}

/// Where the engine keeps its profile. The default (beside the exe)
/// fails under any read-only install, so: the documented env override
/// verbatim, else `%LOCALAPPDATA%\BunnyUi\WebView2\<exe stem>`.
fn user_data_folder() -> String {
    if let Ok(folder) = std::env::var("WEBVIEW2_USER_DATA_FOLDER")
        && !folder.is_empty()
    {
        return folder;
    }
    let stem = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.file_stem().map(|stem| stem.to_string_lossy().into_owned()))
        .unwrap_or_else(|| String::from("app"));
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| String::from("."));
    let folder = format!("{base}\\BunnyUi\\WebView2\\{stem}");
    let _ = std::fs::create_dir_all(&folder);
    folder
}

/// The documented loader entry, taken only when an app CHOSE to drop
/// `WebView2Loader.dll` beside its exe.
type CreateEnvFn =
    unsafe extern "system" fn(*const u16, *const u16, *mut c_void, *mut c_void) -> Hresult;
/// The internal entry the official loader itself calls — undocumented
/// but load-bearing across the ecosystem (reimplemented by every
/// loader-free binding), unchanged in shape since the 2020 runtimes.
/// Verified exported by runtime 151.0.4129.107 on this machine.
type CreateEnvInternalFn =
    unsafe extern "system" fn(i32, i32, *const u16, *mut c_void, *mut c_void) -> Hresult;

/// Stands the ONE environment: the belt (an app-local official
/// loader, documented ABI) first, then the loader-free road — find
/// the client DLL, call its internal export. `Err` is a sentence.
fn create_environment(completed: *mut c_void) -> Result<(), String> {
    let user_data = wide(&user_data_folder());
    unsafe {
        // the belt is EXE-LOCAL by intent: an app author who CHOSE to
        // drop the official loader beside the exe gets the documented
        // ABI — a stray loader on the PATH (a toolkit's, a game's) is
        // nobody's choice and never rides
        let local_loader = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("WebView2Loader.dll")))
            .filter(|path| path.exists());
        if let Some(path) = local_loader {
            let loader =
                LoadLibraryExW(wide(&path.to_string_lossy()).as_ptr(), 0, ALTERED_SEARCH);
            let entry = if loader != 0 {
                GetProcAddress(
                    loader,
                    c"CreateCoreWebView2EnvironmentWithOptions".as_ptr().cast(),
                )
            } else {
                std::ptr::null()
            };
            if !entry.is_null() {
                let create: CreateEnvFn = std::mem::transmute(entry);
                let hr =
                    create(std::ptr::null(), user_data.as_ptr(), std::ptr::null_mut(), completed);
                if com_ok(hr) {
                    return Ok(());
                }
                return Err(format!("the loader refused an environment (0x{:08X})", hr as u32));
            }
        }
        let candidates = client_dll_candidates();
        if candidates.is_empty() {
            return Err(String::from(
                "no WebView2 runtime installed (checked WEBVIEW2_BROWSER_EXECUTABLE_FOLDER \
                 and the EdgeUpdate registry)",
            ));
        }
        for candidate in &candidates {
            let module = LoadLibraryExW(wide(candidate).as_ptr(), 0, ALTERED_SEARCH);
            if module == 0 {
                continue;
            }
            let entry =
                GetProcAddress(module, c"CreateWebViewEnvironmentWithOptionsInternal".as_ptr().cast());
            if entry.is_null() {
                return Err(format!(
                    "the runtime at {candidate} exports no loader entry — the ABI moved"
                ));
            }
            let create: CreateEnvInternalFn = std::mem::transmute(entry);
            // check-running-instance TRUE, runtime kind 0 = installed
            let hr = create(1, 0, user_data.as_ptr(), std::ptr::null_mut(), completed);
            if com_ok(hr) {
                return Ok(());
            }
            return Err(format!("the runtime refused an environment (0x{:08X})", hr as u32));
        }
        Err(String::from("the WebView2 runtime's client DLL did not load"))
    }
}

// MARK: - COM identities (WebView2.h, SDK 1.0.3650.58)

/// `IID_IUnknown` {00000000-0000-0000-C000-000000000046}.
const IID_IUNKNOWN: Guid =
    Guid { d1: 0, d2: 0, d3: 0, d4: [0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46] };
/// `ICoreWebView2_2` {9E8F0CF8-E670-4B5E-B2BC-73E061E3184C} — the one
/// `_N` this backend asks for (the requests door; runtime 88+).
const IID_WEBVIEW2_2: Guid = Guid {
    d1: 0x9e8f0cf8,
    d2: 0xe670,
    d3: 0x4b5e,
    d4: [0xb2, 0xbc, 0x73, 0xe0, 0x61, 0xe3, 0x18, 0x4c],
};
/// `ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler`
/// {4E8A3389-C9D8-4BD2-B6B5-124FEE6CC14D}.
const IID_ENV_COMPLETED: Guid = Guid {
    d1: 0x4e8a3389,
    d2: 0xc9d8,
    d3: 0x4bd2,
    d4: [0xb6, 0xb5, 0x12, 0x4f, 0xee, 0x6c, 0xc1, 0x4d],
};
/// `ICoreWebView2CreateCoreWebView2ControllerCompletedHandler`
/// {6C4819F3-C9B7-4260-8127-C9F5BDE7F68C}.
const IID_CONTROLLER_COMPLETED: Guid = Guid {
    d1: 0x6c4819f3,
    d2: 0xc9b7,
    d3: 0x4260,
    d4: [0x81, 0x27, 0xc9, 0xf5, 0xbd, 0xe7, 0xf6, 0x8c],
};
/// `ICoreWebView2WebMessageReceivedEventHandler`
/// {57213F19-00E6-49FA-8E07-898EA01ECBD2}.
const IID_MESSAGE_RECEIVED: Guid = Guid {
    d1: 0x57213f19,
    d2: 0x00e6,
    d3: 0x49fa,
    d4: [0x8e, 0x07, 0x89, 0x8e, 0xa0, 0x1e, 0xcb, 0xd2],
};
/// `ICoreWebView2NavigationStartingEventHandler`
/// {9ADBE429-F36D-432B-9DDC-F8881FBD76E3}.
const IID_NAV_STARTING: Guid = Guid {
    d1: 0x9adbe429,
    d2: 0xf36d,
    d3: 0x432b,
    d4: [0x9d, 0xdc, 0xf8, 0x88, 0x1f, 0xbd, 0x76, 0xe3],
};
/// `ICoreWebView2ContentLoadingEventHandler`
/// {364471E7-F2BE-4910-BDBA-D72077D51C4B}.
const IID_CONTENT_LOADING: Guid = Guid {
    d1: 0x364471e7,
    d2: 0xf2be,
    d3: 0x4910,
    d4: [0xbd, 0xba, 0xd7, 0x20, 0x77, 0xd5, 0x1c, 0x4b],
};
/// `ICoreWebView2NavigationCompletedEventHandler`
/// {D33A35BF-1C49-4F98-93AB-006E0533FE1C}.
const IID_NAV_COMPLETED: Guid = Guid {
    d1: 0xd33a35bf,
    d2: 0x1c49,
    d3: 0x4f98,
    d4: [0x93, 0xab, 0x00, 0x6e, 0x05, 0x33, 0xfe, 0x1c],
};
/// `ICoreWebView2ExecuteScriptCompletedHandler`
/// {49511172-CC67-4BCA-9923-137112F4C4CC}.
const IID_EXECUTE_SCRIPT: Guid = Guid {
    d1: 0x49511172,
    d2: 0xcc67,
    d3: 0x4bca,
    d4: [0x99, 0x23, 0x13, 0x71, 0x12, 0xf4, 0xc4, 0xcc],
};
/// `ICoreWebView2CapturePreviewCompletedHandler`
/// {697E05E9-3D8F-45FA-96F4-8FFE1EDEDAF5} — the ONE-ARG family.
const IID_CAPTURE_PREVIEW: Guid = Guid {
    d1: 0x697e05e9,
    d2: 0x3d8f,
    d3: 0x45fa,
    d4: [0x96, 0xf4, 0x8f, 0xfe, 0x1e, 0xde, 0xda, 0xf5],
};
/// `ICoreWebView2DevToolsProtocolEventReceivedEventHandler`
/// {E2FDA4BE-5456-406C-A261-3D452138362C}.
const IID_DEVTOOLS_EVENT: Guid = Guid {
    d1: 0xe2fda4be,
    d2: 0x5456,
    d3: 0x406c,
    d4: [0xa2, 0x61, 0x3d, 0x45, 0x21, 0x38, 0x36, 0x2c],
};
/// `ICoreWebView2CallDevToolsProtocolMethodCompletedHandler`
/// {5C4889F0-5EF6-4C5A-952C-D8F1B92D0574}.
const IID_DEVTOOLS_CALL: Guid = Guid {
    d1: 0x5c4889f0,
    d2: 0x5ef6,
    d3: 0x4c5a,
    d4: [0x95, 0x2c, 0xd8, 0xf1, 0xb9, 0x2d, 0x05, 0x74],
};
/// `ICoreWebView2AddScriptToExecuteOnDocumentCreatedCompletedHandler`
/// {B99369F3-9B11-47B5-BC6F-8E7895FCEA17}.
const IID_ADD_SCRIPT: Guid = Guid {
    d1: 0xb99369f3,
    d2: 0x9b11,
    d3: 0x47b5,
    d4: [0xbc, 0x6f, 0x8e, 0x78, 0x95, 0xfc, 0xea, 0x17],
};
/// `ICoreWebView2FocusChangedEventHandler`
/// {05EA24BD-6452-4926-9014-4B82B498135D}.
const IID_FOCUS_CHANGED: Guid = Guid {
    d1: 0x05ea24bd,
    d2: 0x6452,
    d3: 0x4926,
    d4: [0x90, 0x14, 0x4b, 0x82, 0xb4, 0x98, 0x13, 0x5d],
};
/// `ICoreWebView2WebResourceResponseReceivedEventHandler`
/// {7DE9898A-24F5-40C3-A2DE-D4F458E69828}.
const IID_RESPONSE_RECEIVED: Guid = Guid {
    d1: 0x7de9898a,
    d2: 0x24f5,
    d3: 0x40c3,
    d4: [0xa2, 0xde, 0xd4, 0xf4, 0x58, 0xe6, 0x98, 0x28],
};
/// `ICoreWebView2AcceleratorKeyPressedEventHandler`
/// {B29C7E28-FA79-41A8-8E44-65811C76DCB2}.
const IID_ACCELERATOR: Guid = Guid {
    d1: 0xb29c7e28,
    d2: 0xfa79,
    d3: 0x41a8,
    d4: [0x8e, 0x44, 0x65, 0x81, 0x1c, 0x76, 0xdc, 0xb2],
};
/// `ICoreWebView2_11` {0BE78E56-C193-4051-B943-23B460C08BDB} — the
/// context-menu door (runtime 101+, 2022); an older runtime keeps the
/// engine's own menu, whole.
const IID_WEBVIEW2_11: Guid = Guid {
    d1: 0x0be78e56,
    d2: 0xc193,
    d3: 0x4051,
    d4: [0xb9, 0x43, 0x23, 0xb4, 0x60, 0xc0, 0x8b, 0xdb],
};
/// `ICoreWebView2ContextMenuRequestedEventHandler`
/// {04D3FE1D-AB87-42FB-A898-DA241D35B63C}.
const IID_CONTEXT_MENU: Guid = Guid {
    d1: 0x04d3fe1d,
    d2: 0xab87,
    d3: 0x42fb,
    d4: [0xa8, 0x98, 0xda, 0x24, 0x1d, 0x35, 0xb6, 0x3c],
};
/// `ICoreWebView2NewWindowRequestedEventHandler`
/// {D4C185FE-C81C-4989-97AF-2D3FA7AB5651} — a `target="_blank"` link.
const IID_NEW_WINDOW: Guid = Guid {
    d1: 0xd4c185fe,
    d2: 0xc81c,
    d3: 0x4989,
    d4: [0x97, 0xaf, 0x2d, 0x3f, 0xa7, 0xab, 0x56, 0x51],
};

// MARK: - Consumed vtables (header order, indexes cited, runs padded)

/// `ICoreWebView2Environment` — 8 slots.
#[repr(C)]
struct EnvironmentVtbl {
    unknown: UnknownVtbl, // 0-2
    /// 3 `CreateCoreWebView2Controller(parentWindow, handler)`.
    create_controller: unsafe extern "system" fn(*mut Environment, Hwnd, *mut c_void) -> Hresult,
    /// 4 CreateWebResourceResponse; 5 get_BrowserVersionString;
    /// 6-7 add/remove_NewBrowserVersionAvailable.
    _pad_4_7: [usize; 4],
}
#[repr(C)]
struct Environment {
    vtbl: *const EnvironmentVtbl,
}

/// `ICoreWebView2Controller` — 26 slots.
#[repr(C)]
struct ControllerVtbl {
    unknown: UnknownVtbl, // 0-2
    /// 3 get_IsVisible.
    _pad_3: [usize; 1],
    /// 4 `put_IsVisible(BOOL)`.
    put_is_visible: unsafe extern "system" fn(*mut Controller, i32) -> Hresult,
    /// 5 get_Bounds.
    _pad_5: [usize; 1],
    /// 6 `put_Bounds(RECT)` — a by-VALUE POD argument is allowed; the
    /// prohibition is struct RETURNS.
    put_bounds: unsafe extern "system" fn(*mut Controller, Rect) -> Hresult,
    /// 7-8 get/put_ZoomFactor; 9-10 add/remove_ZoomFactorChanged;
    /// 11 SetBoundsAndZoomFactor.
    _pad_7_11: [usize; 5],
    /// 12 `MoveFocus(reason)` — the keyboard is the page's.
    move_focus: unsafe extern "system" fn(*mut Controller, i32) -> Hresult,
    /// 13-14 add/remove_MoveFocusRequested.
    _pad_13_14: [usize; 2],
    /// 15 `add_GotFocus(handler, *token)`.
    add_got_focus:
        unsafe extern "system" fn(*mut Controller, *mut c_void, *mut i64) -> Hresult,
    /// 16 remove_GotFocus; 17-18 add/remove_LostFocus.
    _pad_16_18: [usize; 3],
    /// 19 `add_AcceleratorKeyPressed(handler, *token)`.
    add_accelerator_key_pressed:
        unsafe extern "system" fn(*mut Controller, *mut c_void, *mut i64) -> Hresult,
    /// 20 remove_AcceleratorKeyPressed; 21-22 get/put_ParentWindow.
    _pad_20_22: [usize; 3],
    /// 23 `NotifyParentWindowPositionChanged()`.
    notify_parent_window_position_changed:
        unsafe extern "system" fn(*mut Controller) -> Hresult,
    /// 24 `Close()` — the one call that breaks the host↔browser
    /// reference cycle.
    close: unsafe extern "system" fn(*mut Controller) -> Hresult,
    /// 25 `get_CoreWebView2(**core)`.
    get_core_web_view2:
        unsafe extern "system" fn(*mut Controller, *mut *mut WebView2) -> Hresult,
}
#[repr(C)]
struct Controller {
    vtbl: *const ControllerVtbl,
}

/// `ICoreWebView2` (v1) — 61 slots.
#[repr(C)]
struct WebView2Vtbl {
    unknown: UnknownVtbl, // 0-2
    /// 3 `get_Settings(**settings)`.
    get_settings: unsafe extern "system" fn(*mut WebView2, *mut *mut Settings) -> Hresult,
    /// 4 `get_Source(**uri)` — the committed url, CoTaskMem.
    get_source: unsafe extern "system" fn(*mut WebView2, *mut *mut u16) -> Hresult,
    /// 5 `Navigate(uri)`.
    navigate: unsafe extern "system" fn(*mut WebView2, *const u16) -> Hresult,
    /// 6 `NavigateToString(html)` — a document from memory; the
    /// engine's door takes two megabytes at most.
    navigate_to_string: unsafe extern "system" fn(*mut WebView2, *const u16) -> Hresult,
    /// 7 `add_NavigationStarting(handler, *token)`.
    add_navigation_starting:
        unsafe extern "system" fn(*mut WebView2, *mut c_void, *mut i64) -> Hresult,
    /// 8 remove_NavigationStarting.
    _pad_8: [usize; 1],
    /// 9 `add_ContentLoading(handler, *token)` — the commit leg.
    add_content_loading:
        unsafe extern "system" fn(*mut WebView2, *mut c_void, *mut i64) -> Hresult,
    /// 10 remove_ContentLoading; 11-12 add/remove_SourceChanged;
    /// 13-14 add/remove_HistoryChanged.
    _pad_10_14: [usize; 5],
    /// 15 `add_NavigationCompleted(handler, *token)` — the refusal leg.
    add_navigation_completed:
        unsafe extern "system" fn(*mut WebView2, *mut c_void, *mut i64) -> Hresult,
    /// 16 remove_NavigationCompleted; 17-20 frame navigation pairs;
    /// 21-22 add/remove_ScriptDialogOpening;
    /// 23-24 add/remove_PermissionRequested;
    /// 25-26 add/remove_ProcessFailed.
    _pad_16_26: [usize; 11],
    /// 27 `AddScriptToExecuteOnDocumentCreated(script, handler)`.
    add_script_on_created:
        unsafe extern "system" fn(*mut WebView2, *const u16, *mut c_void) -> Hresult,
    /// 28 `RemoveScriptToExecuteOnDocumentCreated(id)`.
    remove_script_on_created:
        unsafe extern "system" fn(*mut WebView2, *const u16) -> Hresult,
    /// 29 `ExecuteScript(script, handler)`.
    execute_script:
        unsafe extern "system" fn(*mut WebView2, *const u16, *mut c_void) -> Hresult,
    /// 30 `CapturePreview(format, stream, handler)`.
    capture_preview:
        unsafe extern "system" fn(*mut WebView2, i32, *mut c_void, *mut c_void) -> Hresult,
    /// 31 Reload; 32 PostWebMessageAsJson; 33 PostWebMessageAsString.
    _pad_31_33: [usize; 3],
    /// 34 `add_WebMessageReceived(handler, *token)` — the one wire.
    add_web_message_received:
        unsafe extern "system" fn(*mut WebView2, *mut c_void, *mut i64) -> Hresult,
    /// 35 remove_WebMessageReceived.
    _pad_35: [usize; 1],
    /// 36 `CallDevToolsProtocolMethod(method, paramsJson, handler)`.
    call_devtools_protocol_method:
        unsafe extern "system" fn(*mut WebView2, *const u16, *const u16, *mut c_void) -> Hresult,
    /// 37 get_BrowserProcessId; 38 get_CanGoBack; 39 get_CanGoForward.
    _pad_37_39: [usize; 3],
    /// 40 `GoBack()`.
    go_back: unsafe extern "system" fn(*mut WebView2) -> Hresult,
    /// 41 `GoForward()`.
    go_forward: unsafe extern "system" fn(*mut WebView2) -> Hresult,
    /// 42 `GetDevToolsProtocolEventReceiver(eventName, **receiver)`.
    get_devtools_receiver:
        unsafe extern "system" fn(*mut WebView2, *const u16, *mut *mut DevToolsReceiver) -> Hresult,
    /// 43 Stop.
    _pad_43: [usize; 1],
    /// 44 `add_NewWindowRequested(handler, *token)` — the ask a
    /// `target="_blank"` link makes.
    add_new_window_requested:
        unsafe extern "system" fn(*mut WebView2, *mut c_void, *mut i64) -> Hresult,
    /// 45 remove_NewWindowRequested;
    /// 46-47 add/remove_DocumentTitleChanged; 48 get_DocumentTitle;
    /// 49 AddHostObjectToScript; 50 RemoveHostObjectFromScript;
    /// 51 OpenDevToolsWindow;
    /// 52-53 add/remove_ContainsFullScreenElementChanged;
    /// 54 get_ContainsFullScreenElement;
    /// 55-56 add/remove_WebResourceRequested;
    /// 57 AddWebResourceRequestedFilter;
    /// 58 RemoveWebResourceRequestedFilter;
    /// 59-60 add/remove_WindowCloseRequested.
    _pad_45_60: [usize; 16],
}
#[repr(C)]
struct WebView2 {
    vtbl: *const WebView2Vtbl,
}

/// `ICoreWebView2_2` — the base's 61 slots and 7 more.
#[repr(C)]
struct WebView2_2Vtbl {
    base: WebView2Vtbl, // 0-60
    /// 61 `add_WebResourceResponseReceived(handler, *token)`.
    add_response_received:
        unsafe extern "system" fn(*mut WebView2_2, *mut c_void, *mut i64) -> Hresult,
    /// 62 `remove_WebResourceResponseReceived(token)`.
    remove_response_received: unsafe extern "system" fn(*mut WebView2_2, i64) -> Hresult,
    /// 63 NavigateWithWebResourceRequest;
    /// 64-65 add/remove_DOMContentLoaded; 66 get_CookieManager;
    /// 67 get_Environment.
    _pad_63_67: [usize; 5],
}
#[repr(C)]
struct WebView2_2 {
    vtbl: *const WebView2_2Vtbl,
}

/// `ICoreWebView2Settings` — 21 slots.
#[repr(C)]
struct SettingsVtbl {
    unknown: UnknownVtbl, // 0-2
    /// 3-10 script/webmessage/dialogs/statusbar get-put pairs;
    /// 11 get_AreDevToolsEnabled.
    _pad_3_11: [usize; 9],
    /// 12 put_AreDevToolsEnabled — declared to cite the devtools cell;
    /// TRUE is the default, nothing is written.
    _pad_12: [usize; 1],
    /// 13-18 context-menu/host-object/zoom get-put pairs;
    /// 19 get_IsBuiltInErrorPageEnabled.
    _pad_13_19: [usize; 7],
    /// 20 `put_IsBuiltInErrorPageEnabled(BOOL)` — OFF, so a dead host
    /// leaves the old page on screen and answers ONLY on the failure
    /// leg (the pair contract; an error page would commit).
    put_built_in_error_page: unsafe extern "system" fn(*mut Settings, i32) -> Hresult,
}
#[repr(C)]
struct Settings {
    vtbl: *const SettingsVtbl,
}

/// `ICoreWebView2WebMessageReceivedEventArgs` — 6 slots.
#[repr(C)]
struct MessageArgsVtbl {
    unknown: UnknownVtbl, // 0-2
    /// 3 get_Source; 4 get_WebMessageAsJson.
    _pad_3_4: [usize; 2],
    /// 5 `TryGetWebMessageAsString(**string)` — fails for a non-string
    /// post, which drops exactly as the mac drops a non-NSString body.
    try_get_string: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> Hresult,
}

/// `ICoreWebView2NavigationStartingEventArgs` — 10 slots.
#[repr(C)]
struct NavStartingArgsVtbl {
    unknown: UnknownVtbl, // 0-2
    /// 3 `get_Uri(**uri)` — where this navigation is AIMING.
    get_uri: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> Hresult,
    /// 4 `get_IsUserInitiated(*bool)` — a gesture, not a script.
    get_is_user_initiated: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    /// 5 get_IsRedirected; 6 get_RequestHeaders; 7 get_Cancel.
    _pad_5_7: [usize; 3],
    /// 8 `put_Cancel(bool)` — the navigation does not happen.
    put_cancel: unsafe extern "system" fn(*mut c_void, i32) -> Hresult,
    /// 9 `get_NavigationId(*id)`.
    get_navigation_id: unsafe extern "system" fn(*mut c_void, *mut u64) -> Hresult,
}

/// `ICoreWebView2NewWindowRequestedEventArgs` — 11 slots.
#[repr(C)]
struct NewWindowArgsVtbl {
    unknown: UnknownVtbl, // 0-2
    /// 3 `get_Uri(**uri)` — where the new window would go.
    get_uri: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> Hresult,
    /// 4 put_NewWindow; 5 get_NewWindow.
    _pad_4_5: [usize; 2],
    /// 6 `put_Handled(bool)` — handled means NO window opens.
    put_handled: unsafe extern "system" fn(*mut c_void, i32) -> Hresult,
    /// 7 get_Handled; 8 get_IsUserInitiated; 9 GetDeferral;
    /// 10 get_WindowFeatures.
    _pad_7_10: [usize; 4],
}

/// `ICoreWebView2ContentLoadingEventArgs` — 5 slots.
#[repr(C)]
struct ContentLoadingArgsVtbl {
    unknown: UnknownVtbl, // 0-2
    /// 3 `get_IsErrorPage(*bool)`.
    get_is_error_page: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    /// 4 get_NavigationId.
    _pad_4: [usize; 1],
}

/// `ICoreWebView2NavigationCompletedEventArgs` — 6 slots.
#[repr(C)]
struct NavCompletedArgsVtbl {
    unknown: UnknownVtbl, // 0-2
    /// 3 `get_IsSuccess(*bool)`.
    get_is_success: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    /// 4 `get_WebErrorStatus(*status)`.
    get_web_error_status: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    /// 5 `get_NavigationId(*id)`.
    get_navigation_id: unsafe extern "system" fn(*mut c_void, *mut u64) -> Hresult,
}

/// `ICoreWebView2DevToolsProtocolEventReceiver` — 5 slots.
#[repr(C)]
struct DevToolsReceiverVtbl {
    unknown: UnknownVtbl, // 0-2
    /// 3 `add_DevToolsProtocolEventReceived(handler, *token)`.
    add_event_received:
        unsafe extern "system" fn(*mut DevToolsReceiver, *mut c_void, *mut i64) -> Hresult,
    /// 4 remove_DevToolsProtocolEventReceived.
    _pad_4: [usize; 1],
}
#[repr(C)]
struct DevToolsReceiver {
    vtbl: *const DevToolsReceiverVtbl,
}

/// `ICoreWebView2DevToolsProtocolEventReceivedEventArgs` — 4 slots.
#[repr(C)]
struct DevToolsArgsVtbl {
    unknown: UnknownVtbl, // 0-2
    /// 3 `get_ParameterObjectAsJson(**json)`.
    get_parameter_json: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> Hresult,
}

/// `ICoreWebView2WebResourceResponseReceivedEventArgs` — 5 slots.
#[repr(C)]
struct ResponseReceivedArgsVtbl {
    unknown: UnknownVtbl, // 0-2
    /// 3 `get_Request(**request)`.
    get_request: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
    /// 4 `get_Response(**view)`.
    get_response: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
}

/// `ICoreWebView2WebResourceRequest` — 10 slots.
#[repr(C)]
struct ResourceRequestVtbl {
    unknown: UnknownVtbl, // 0-2
    /// 3 `get_Uri(**uri)`.
    get_uri: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> Hresult,
    /// 4 put_Uri.
    _pad_4: [usize; 1],
    /// 5 `get_Method(**method)`.
    get_method: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> Hresult,
    /// 6 put_Method; 7 get_Content; 8 put_Content; 9 get_Headers.
    _pad_6_9: [usize; 4],
}

/// `ICoreWebView2WebResourceResponseView` — 7 slots.
#[repr(C)]
struct ResponseViewVtbl {
    unknown: UnknownVtbl, // 0-2
    /// 3 get_Headers.
    _pad_3: [usize; 1],
    /// 4 `get_StatusCode(*status)`.
    get_status_code: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    /// 5 get_ReasonPhrase; 6 GetContent.
    _pad_5_6: [usize; 2],
}

/// `ICoreWebView2_11` — 102 slots; reached by QI for the menu door
/// alone, so everything before it rides as one pad.
#[repr(C)]
struct WebView2_11Vtbl {
    /// 0-2 IUnknown; 3-60 the base; 61-98 the `_2`..`_10` chain;
    /// 99 CallDevToolsProtocolMethodForSession.
    _pad_0_99: [usize; 100],
    /// 100 `add_ContextMenuRequested(handler, *token)`.
    add_context_menu_requested:
        unsafe extern "system" fn(*mut WebView2_11, *mut c_void, *mut i64) -> Hresult,
    /// 101 remove_ContextMenuRequested.
    _pad_101: [usize; 1],
}
#[repr(C)]
struct WebView2_11 {
    vtbl: *const WebView2_11Vtbl,
}

/// `ICoreWebView2ContextMenuRequestedEventArgs` — 11 slots.
#[repr(C)]
struct ContextMenuArgsVtbl {
    unknown: UnknownVtbl, // 0-2
    /// 3 `get_MenuItems(**collection)`.
    get_menu_items: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
    /// 4 get_ContextMenuTarget; 5 get_Location;
    /// 6 put_SelectedCommandId; 7 get_SelectedCommandId;
    /// 8 put_Handled; 9 get_Handled; 10 GetDeferral. `Handled` stays
    /// untouched — the ENGINE shows the trimmed menu.
    _pad_4_10: [usize; 7],
}

/// `ICoreWebView2ContextMenuItemCollection` — 7 slots.
#[repr(C)]
struct MenuItemsVtbl {
    unknown: UnknownVtbl, // 0-2
    /// 3 `get_Count(*count)`.
    get_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> Hresult,
    /// 4 `GetValueAtIndex(index, **item)`.
    get_value_at_index:
        unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> Hresult,
    /// 5 `RemoveValueAtIndex(index)`.
    remove_value_at_index: unsafe extern "system" fn(*mut c_void, u32) -> Hresult,
    /// 6 InsertValueAtIndex.
    _pad_6: [usize; 1],
}

/// `ICoreWebView2ContextMenuItem` — 16 slots.
#[repr(C)]
struct MenuItemVtbl {
    unknown: UnknownVtbl, // 0-2
    /// 3 `get_Name(**name)` — the STABLE identifier ("back",
    /// "saveAs", "inspectElement"), never the localized label.
    get_name: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> Hresult,
    /// 4 get_Label; 5 get_CommandId; 6 get_ShortcutKeyDescription;
    /// 7 get_Icon.
    _pad_4_7: [usize; 4],
    /// 8 `get_Kind(*kind)` — 0 command, 1 checkbox, 2 radio,
    /// 3 separator, 4 submenu.
    get_kind: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    /// 9-12 is-enabled/is-checked pairs; 13 get_Children;
    /// 14-15 add/remove_CustomItemSelected.
    _pad_9_15: [usize; 7],
}

/// `ICoreWebView2AcceleratorKeyPressedEventArgs` — 9 slots.
#[repr(C)]
struct AcceleratorArgsVtbl {
    unknown: UnknownVtbl, // 0-2
    /// 3 `get_KeyEventKind(*kind)` — 0 KEY_DOWN, 1 KEY_UP,
    /// 2 SYSTEM_KEY_DOWN, 3 SYSTEM_KEY_UP.
    get_key_event_kind: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    /// 4 `get_VirtualKey(*vk)`.
    get_virtual_key: unsafe extern "system" fn(*mut c_void, *mut u32) -> Hresult,
    /// 5 `get_KeyEventLParam(*lparam)`.
    get_key_event_lparam: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    /// 6 get_PhysicalKeyStatus; 7 get_Handled.
    _pad_6_7: [usize; 2],
    /// 8 `put_Handled(BOOL)` — TRUE suppresses both the browser's
    /// default action and the page's sight of the stroke.
    put_handled: unsafe extern "system" fn(*mut c_void, i32) -> Hresult,
}

/// `IStream` — 14 slots; the snapshot needs a rewind and a read.
#[repr(C)]
struct StreamVtbl {
    unknown: UnknownVtbl, // 0-2
    /// 3 `Read(buffer, ask, *got)`.
    read: unsafe extern "system" fn(*mut c_void, *mut u8, u32, *mut u32) -> Hresult,
    /// 4 Write.
    _pad_4: [usize; 1],
    /// 5 `Seek(move, origin, *landed)`.
    seek: unsafe extern "system" fn(*mut c_void, i64, u32, *mut u64) -> Hresult,
    /// 6 SetSize; 7 CopyTo; 8 Commit; 9 Revert; 10 LockRegion;
    /// 11 UnlockRegion; 12 Stat; 13 Clone.
    _pad_6_13: [usize; 8],
}

// MARK: - The one Rust-authored COM shape

/// Every WebView2 handler is IUnknown plus one `Invoke`, in two
/// arities — `(this, a, b)` for completions and events, `(this, a)`
/// for `CapturePreviewCompleted` alone. Two static vtables (an arity
/// mismatch corrupts the stack on x86 stdcall), one heap layout.
#[repr(C)]
struct Handler {
    /// `&HANDLER2_VTBL` or `&HANDLER1_VTBL`, erased — one pointer
    /// either way, so the shared IUnknown code reads past it blind.
    vtbl: *const c_void,
    /// The ONE interface this instance answers besides IUnknown.
    iid: Guid,
    /// STA-only: WebView2 invokes on the thread that created the
    /// environment — this pump's. A `Cell` is the honest spelling.
    refs: Cell<u32>,
    /// `Invoke`'s two raw args; the closure casts what it knows.
    land: RefCell<Box<dyn FnMut(usize, usize) -> Hresult>>,
}

#[repr(C)]
struct Handler2Vtbl {
    unknown: UnknownVtbl, // 0-2
    invoke: unsafe extern "system" fn(*mut Handler, usize, usize) -> Hresult, // 3
}

#[repr(C)]
struct Handler1Vtbl {
    unknown: UnknownVtbl, // 0-2
    invoke: unsafe extern "system" fn(*mut Handler, usize) -> Hresult, // 3
}

/// `E_NOINTERFACE`.
const NO_INTERFACE: Hresult = 0x8000_4002u32 as i32;

unsafe extern "system" fn handler_query(
    this: *mut c_void,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> Hresult {
    unsafe {
        let handler = this as *mut Handler;
        if *riid == IID_IUNKNOWN || *riid == (*handler).iid {
            (*handler).refs.set((*handler).refs.get() + 1);
            *out = this;
            0
        } else {
            *out = std::ptr::null_mut();
            NO_INTERFACE
        }
    }
}

unsafe extern "system" fn handler_add_ref(this: *mut c_void) -> u32 {
    unsafe {
        let handler = this as *mut Handler;
        let refs = (*handler).refs.get() + 1;
        (*handler).refs.set(refs);
        refs
    }
}

unsafe extern "system" fn handler_release(this: *mut c_void) -> u32 {
    unsafe {
        let handler = this as *mut Handler;
        let refs = (*handler).refs.get() - 1;
        (*handler).refs.set(refs);
        if refs == 0 {
            drop(Box::from_raw(handler));
        }
        refs
    }
}

unsafe extern "system" fn handler_invoke2(this: *mut Handler, a: usize, b: usize) -> Hresult {
    unsafe { ((*this).land.borrow_mut())(a, b) }
}

unsafe extern "system" fn handler_invoke1(this: *mut Handler, a: usize) -> Hresult {
    unsafe { ((*this).land.borrow_mut())(a, 0) }
}

static HANDLER2_VTBL: Handler2Vtbl = Handler2Vtbl {
    unknown: UnknownVtbl {
        query_interface: handler_query,
        add_ref: handler_add_ref,
        release: handler_release,
    },
    invoke: handler_invoke2,
};

static HANDLER1_VTBL: Handler1Vtbl = Handler1Vtbl {
    unknown: UnknownVtbl {
        query_interface: handler_query,
        add_ref: handler_add_ref,
        release: handler_release,
    },
    invoke: handler_invoke1,
};

/// Boxes a two-arg handler with ONE reference — the caller passes it
/// to the COM call and then [`com_release`]s it; the callee AddRef'd
/// if it kept it.
fn handler2(iid: Guid, land: impl FnMut(usize, usize) -> Hresult + 'static) -> *mut c_void {
    Box::into_raw(Box::new(Handler {
        vtbl: (&raw const HANDLER2_VTBL) as *const c_void,
        iid,
        refs: Cell::new(1),
        land: RefCell::new(Box::new(land)),
    })) as *mut c_void
}

/// The one-arg family — `CapturePreviewCompleted`.
fn handler1(iid: Guid, mut land: impl FnMut(usize) + 'static) -> *mut c_void {
    let boxed = Box::new(Handler {
        vtbl: (&raw const HANDLER1_VTBL) as *const c_void,
        iid,
        refs: Cell::new(1),
        land: RefCell::new(Box::new(move |a, _| {
            land(a);
            0
        })),
    });
    Box::into_raw(boxed) as *mut c_void
}

/// One release through the IUnknown prefix every interface starts with.
unsafe fn com_release(pointer: *mut c_void) {
    unsafe {
        let vtbl = *(pointer as *mut *const UnknownVtbl);
        ((*vtbl).release)(pointer);
    }
}

unsafe fn com_add_ref(pointer: *mut c_void) {
    unsafe {
        let vtbl = *(pointer as *mut *const UnknownVtbl);
        ((*vtbl).add_ref)(pointer);
    }
}

/// A CoTaskMem string OUT-PARAM, taken whole: walked to the NUL,
/// copied out, freed. Only a `get_*`/`TryGet*` answer transfers
/// ownership — a string ARGUMENT of a completed handler is the
/// engine's own and takes [`borrow_ws`] instead (freeing it corrupts
/// the heap and the process dies without a word).
unsafe fn take_ws(pointer: *mut u16) -> String {
    let text = unsafe { borrow_ws(pointer) };
    if !pointer.is_null() {
        unsafe {
            CoTaskMemFree(pointer as *mut c_void);
        }
    }
    text
}

/// A borrowed wide string, copied out and left alone.
unsafe fn borrow_ws(pointer: *const u16) -> String {
    if pointer.is_null() {
        return String::new();
    }
    unsafe {
        let mut length = 0;
        while *pointer.add(length) != 0 {
            length += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(pointer, length))
    }
}

// MARK: - The page registry and the async mount

/// One environment per process — WebView2's own rule (one user-data
/// folder, one environment), and the mac's one-bridge discipline.
enum EnvState {
    Unresolved,
    /// The completed handler is in flight; these paths wait for it.
    Creating { waiters: Vec<String> },
    /// One AddRef held for the process, released at teardown.
    Ready(*mut Environment),
    Failed(Rc<str>),
}

/// What the spec instructed, copied out of the node.
#[derive(Clone)]
struct SpecCopy {
    url: Rc<str>,
    /// A page from memory, sealed — loaded instead of `url`.
    document: Option<Document>,
    scripts: Rc<[Rc<str>]>,
    console: bool,
    requests: bool,
    full_motion: bool,
}

impl SpecCopy {
    fn of(spec: &HostSpec) -> SpecCopy {
        let HostSpec::Webview { url, document, scripts, console, requests, full_motion } = spec;
        SpecCopy {
            url: Rc::clone(url),
            document: document.clone(),
            scripts: Rc::clone(scripts),
            console: *console,
            requests: *requests,
            full_motion: *full_motion,
        }
    }
}

/// A command asked of a page still assembling — replayed at land, in
/// arrival order, so `navigate(a); back()` means what it says.
enum Queued {
    Navigate(Rc<str>),
    Back,
    Forward,
    Eval { token: u64, js: Rc<str>, raw: bool },
    Snapshot { token: u64 },
    Input(WebviewInput),
    Edit(EditorAction),
}

/// The engine half of a Live mount — raw pointers, each holding ONE
/// reference this module releases at sweep.
struct Live {
    controller: *mut Controller,
    core: *mut WebView2,
    /// `ICoreWebView2_2` where the runtime answers it, or the refusal
    /// already spoken (the requests door, 88+).
    core2: Option<*mut WebView2_2>,
    /// The url each in-flight navigation is AIMING at, by id — the
    /// failure leg's answer (the view's own url is the page that did
    /// not move).
    nav_targets: HashMap<u64, String>,
    /// Whether the CDP console door was opened.
    console_wired: bool,
    /// The requests registration — `Some` once wired (or once refused
    /// by name, so the refusal speaks exactly once).
    requests_token: Option<i64>,
    /// Whether the standing engine currently emulates the visitor's
    /// motion (`Emulation.setEmulatedMedia`) — the flag `update`
    /// compares against, so a mid-session toggle re-instructs.
    full_motion: bool,
    /// The DOCUMENT loaded, if one is: its fingerprint (what `update`
    /// compares) and whether the app's own load is still the one
    /// navigation the starting leg lets through.
    letter: Option<Letter>,
}

/// A loaded document's standing — the mac's, verbatim.
struct Letter {
    digest: u64,
    expected: bool,
    /// The editor takes the keyboard at the commit — once.
    focus: bool,
}

enum Mount {
    /// Registered; the environment is not ready yet.
    Waiting,
    /// The controller-completed handler is in flight.
    Creating,
    Live(Live),
    /// The engine will never come — every later ask answers by this
    /// name, never silence.
    Refused(Rc<str>),
}

struct Slot {
    container: Hwnd,
    /// Bumped by every `update` — async arrivals from an older
    /// instruct die on sight.
    generation: u64,
    spec: SpecCopy,
    /// The latest tenant rect, container-local physical px — applied
    /// at land, and on every `place` while Live.
    bounds: (i32, i32, i32, i32),
    shown: bool,
    queued: Vec<Queued>,
    /// Installed document-created scripts with the generation that
    /// asked for them — held on the SLOT because the ids arrive
    /// async, possibly before the mount turns Live.
    script_ids: Vec<(u64, String)>,
    state: Mount,
}

thread_local! {
    static ENVIRONMENT: RefCell<EnvState> = const { RefCell::new(EnvState::Unresolved) };
    static VIEWS: RefCell<HashMap<String, Slot>> = RefCell::new(HashMap::new());
}

// MARK: - Mounting

/// Registers the page and starts the chain: environment (once per
/// process), then this host's controller. Everything asked before the
/// controller lands queues — the host IS mounted; the engine is still
/// assembling, and an `Err` here would be a lie about a webview that
/// exists.
pub(crate) fn create(path: &str, container: Hwnd, spec: &HostSpec) {
    com_init();
    VIEWS.with(|views| {
        views.borrow_mut().insert(
            path.to_string(),
            Slot {
                container,
                generation: 0,
                spec: SpecCopy::of(spec),
                bounds: (0, 0, 0, 0),
                shown: false,
                queued: Vec::new(),
                script_ids: Vec::new(),
                state: Mount::Waiting,
            },
        );
    });
    enum Next {
        Stand,
        Wait,
        Start(*mut Environment),
        Refuse(Rc<str>),
    }
    let next = ENVIRONMENT.with(|state| {
        let mut state = state.borrow_mut();
        match &mut *state {
            EnvState::Unresolved => {
                *state = EnvState::Creating { waiters: vec![path.to_string()] };
                Next::Stand
            }
            EnvState::Creating { waiters } => {
                waiters.push(path.to_string());
                Next::Wait
            }
            EnvState::Ready(environment) => Next::Start(*environment),
            EnvState::Failed(why) => Next::Refuse(Rc::clone(why)),
        }
    });
    match next {
        Next::Stand => stand_environment(),
        Next::Wait => {}
        Next::Start(environment) => start_controller(environment, path),
        Next::Refuse(why) => refuse_mount(path, &why),
    }
}

/// Kicks the environment; a synchronous refusal fails every waiter.
fn stand_environment() {
    let completed = handler2(IID_ENV_COMPLETED, |hr, environment| {
        environment_landed(hr as Hresult, environment as *mut Environment);
        0
    });
    let stood = create_environment(completed);
    unsafe {
        com_release(completed);
    }
    if let Err(why) = stood {
        environment_failed(&why);
    }
}

fn environment_landed(hr: Hresult, environment: *mut Environment) {
    if !com_ok(hr) || environment.is_null() {
        environment_failed(&format!("the runtime refused an environment (0x{:08X})", hr as u32));
        return;
    }
    unsafe {
        com_add_ref(environment as *mut c_void);
    }
    let waiters = ENVIRONMENT.with(|state| {
        let mut state = state.borrow_mut();
        let waiters = match &mut *state {
            EnvState::Creating { waiters } => std::mem::take(waiters),
            _ => Vec::new(),
        };
        *state = EnvState::Ready(environment);
        waiters
    });
    for path in waiters {
        start_controller(environment, &path);
    }
}

fn environment_failed(why: &str) {
    let why: Rc<str> = Rc::from(why);
    let waiters = ENVIRONMENT.with(|state| {
        let mut state = state.borrow_mut();
        let waiters = match &mut *state {
            EnvState::Creating { waiters } => std::mem::take(waiters),
            _ => Vec::new(),
        };
        *state = EnvState::Failed(Rc::clone(&why));
        waiters
    });
    for path in waiters {
        refuse_mount(&path, &why);
    }
}

/// A host that will never get an engine: the pair contract still
/// holds — the asked load answers on the refusal leg, every parked
/// eval and snapshot answers `Err` now, and the slot remembers the
/// refusal so every LATER ask answers by the same name.
fn refuse_mount(path: &str, why: &str) {
    let why: Rc<str> = Rc::from(why);
    let parked = VIEWS.with(|views| {
        let mut views = views.borrow_mut();
        let Some(slot) = views.get_mut(path) else {
            return None;
        };
        slot.state = Mount::Refused(Rc::clone(&why));
        Some((Rc::clone(&slot.spec.url), std::mem::take(&mut slot.queued)))
    });
    let Some((url, queued)) = parked else {
        return;
    };
    dispatch(WebviewEvent::NavigationFailed {
        path: path.to_string(),
        url: url.to_string(),
        why: why.to_string(),
    });
    for command in queued {
        match command {
            Queued::Eval { token, .. } => {
                dispatch(WebviewEvent::EvalDone { token, result: Err(why.to_string()) });
            }
            Queued::Snapshot { token } => {
                dispatch(WebviewEvent::SnapshotDone { token, result: Err(why.to_string()) });
            }
            _ => {}
        }
    }
}

fn start_controller(environment: *mut Environment, path: &str) {
    let (container, generation) = match VIEWS.with(|views| {
        let mut views = views.borrow_mut();
        views.get_mut(path).map(|slot| {
            slot.state = Mount::Creating;
            (slot.container, slot.generation)
        })
    }) {
        Some(found) => found,
        None => return, // swept before the environment stood
    };
    let owned = path.to_string();
    let completed = handler2(IID_CONTROLLER_COMPLETED, move |hr, controller| {
        controller_landed(&owned, generation, hr as Hresult, controller as *mut Controller);
        0
    });
    let hr = unsafe {
        let hr = ((*(*environment).vtbl).create_controller)(environment, container, completed);
        com_release(completed);
        hr
    };
    if !com_ok(hr) {
        // the handler will not fire — the refusal speaks now
        refuse_mount(path, &format!("the controller refused (0x{:08X})", hr as u32));
    }
}

/// The heart: the controller landed, and the whole page stands up in
/// one sequence — wiring, settings, scripts, instrumentation,
/// geometry, the first navigation, and the queued commands in order.
fn controller_landed(path: &str, generation: u64, hr: Hresult, controller: *mut Controller) {
    let current =
        VIEWS.with(|views| views.borrow().get(path).map(|slot| slot.generation));
    let stale = match current {
        // swept before land — the parked answers were spoken at sweep
        None => true,
        // an older instruct's arrival dies on sight
        Some(live_generation) => live_generation != generation,
    };
    if stale {
        // the tombstone rule: close what arrived, and done
        if !controller.is_null() {
            unsafe {
                let _ = ((*(*controller).vtbl).close)(controller);
            }
        }
        return;
    }
    if !com_ok(hr) || controller.is_null() {
        refuse_mount(path, &format!("the controller refused (0x{:08X})", hr as u32));
        return;
    }

    unsafe {
        com_add_ref(controller as *mut c_void);
    }
    let mut core: *mut WebView2 = std::ptr::null_mut();
    unsafe {
        if !com_ok(((*(*controller).vtbl).get_core_web_view2)(controller, &mut core))
            || core.is_null()
        {
            let _ = ((*(*controller).vtbl).close)(controller);
            com_release(controller as *mut c_void);
            refuse_mount(path, "the controller carries no core");
            return;
        }
    }
    let core2 = unsafe {
        com_query(core as *mut c_void, &IID_WEBVIEW2_2).map(|found| found as *mut WebView2_2)
    };

    // the spec and geometry, current AT LAND TIME (an update while
    // pending replaced the parked spec)
    let (spec, bounds, shown, queued) = VIEWS.with(|views| {
        let mut views = views.borrow_mut();
        let slot = views.get_mut(path).expect("checked above");
        (slot.spec.clone(), slot.bounds, slot.shown, std::mem::take(&mut slot.queued))
    });

    unsafe {
        // the always-on wires; the tokens drop — a registration lives
        // as long as the view, and Close severs them all
        let mut token = 0i64;
        let on_message = {
            let path = path.to_string();
            handler2(IID_MESSAGE_RECEIVED, move |_sender, args| {
                message_received(&path, args as *mut c_void);
                0
            })
        };
        let _ = ((*(*core).vtbl).add_web_message_received)(core, on_message, &mut token);
        com_release(on_message);

        let on_starting = {
            let path = path.to_string();
            handler2(IID_NAV_STARTING, move |_sender, args| {
                navigation_starting(&path, args as *mut c_void);
                0
            })
        };
        let _ = ((*(*core).vtbl).add_navigation_starting)(core, on_starting, &mut token);
        com_release(on_starting);

        let on_loading = {
            let path = path.to_string();
            handler2(IID_CONTENT_LOADING, move |sender, args| {
                content_loading(&path, sender as *mut WebView2, args as *mut c_void);
                0
            })
        };
        let _ = ((*(*core).vtbl).add_content_loading)(core, on_loading, &mut token);
        com_release(on_loading);

        let on_completed = {
            let path = path.to_string();
            handler2(IID_NAV_COMPLETED, move |sender, args| {
                navigation_completed(&path, sender as *mut WebView2, args as *mut c_void);
                0
            })
        };
        let _ = ((*(*core).vtbl).add_navigation_completed)(core, on_completed, &mut token);
        com_release(on_completed);

        let on_new_window = {
            let path = path.to_string();
            handler2(IID_NEW_WINDOW, move |_sender, args| {
                new_window_requested(&path, args as *mut c_void);
                0
            })
        };
        let _ = ((*(*core).vtbl).add_new_window_requested)(core, on_new_window, &mut token);
        com_release(on_new_window);

        let on_focus = handler2(IID_FOCUS_CHANGED, move |_sender, _args| {
            dispatch(WebviewEvent::FocusTaken);
            0
        });
        let _ = ((*(*controller).vtbl).add_got_focus)(controller, on_focus, &mut token);
        com_release(on_focus);

        // the chords outrank the island: while the page holds the
        // keyboard this event is the one road an app chord has back —
        // the pump's own gate runs behind it, so one stroke can never
        // meet both gates
        let on_accelerator = handler2(IID_ACCELERATOR, move |_sender, args| {
            accelerator_pressed(args as *mut c_void);
            0
        });
        let _ = ((*(*controller).vtbl).add_accelerator_key_pressed)(
            controller,
            on_accelerator,
            &mut token,
        );
        com_release(on_accelerator);

        // the menu over the page wears the mac's cut: the engine still
        // owns it, but the browser-chrome rows (Save as, Print, Send
        // tab…) leave and the WebKit set stays. An older runtime has
        // no door and keeps the whole menu, honestly.
        if let Some(core11) = com_query(core as *mut c_void, &IID_WEBVIEW2_11) {
            let core11 = core11 as *mut WebView2_11;
            let on_menu = handler2(IID_CONTEXT_MENU, move |_sender, args| {
                trim_context_menu(args as *mut c_void);
                0
            });
            let _ = ((*(*core11).vtbl).add_context_menu_requested)(core11, on_menu, &mut token);
            com_release(on_menu);
            com_release(core11 as *mut c_void);
        }

        // the pair contract needs the refusal leg pure: with the
        // built-in error page a dead host would COMMIT an error
        // document and answer on both legs
        let mut settings: *mut Settings = std::ptr::null_mut();
        if com_ok(((*(*core).vtbl).get_settings)(core, &mut settings)) && !settings.is_null() {
            let _ = ((*(*settings).vtbl).put_built_in_error_page)(settings, 0);
            com_release(settings as *mut c_void);
        }

        apply_scripts(core, path, generation, &spec);

        // geometry stored while pending lands now, then the spec's url
        let _ = ((*(*controller).vtbl).put_bounds)(
            controller,
            Rect { left: bounds.0, top: bounds.1, right: bounds.0 + bounds.2, bottom: bounds.1 + bounds.3 },
        );
        let _ = ((*(*controller).vtbl).put_is_visible)(controller, shown as i32);
        // the visitor's motion stands BEFORE the first navigation
        // departs — the page never consults the tester's OS first —
        // and is re-armed on every commit (the emulation is the
        // session's, and a renderer swap must not shed it)
        if spec.full_motion {
            cdp(core, "Emulation.setEmulatedMedia", &emulated_media_params(true));
        }
    }
    // the letter is filed BEFORE its load departs: the starting leg is
    // asked about that load, and must find the letter expecting it
    let letter = spec.document.as_ref().map(|document| Letter {
        digest: document.digest,
        expected: true,
        focus: document.focus,
    });
    VIEWS.with(|views| {
        let mut views = views.borrow_mut();
        if let Some(slot) = views.get_mut(path) {
            slot.state = Mount::Live(Live {
                controller,
                core,
                core2,
                nav_targets: HashMap::new(),
                console_wired: false,
                requests_token: None,
                full_motion: spec.full_motion,
                letter,
            });
        }
    });
    unsafe {
        match &spec.document {
            Some(document) => load_document(core, path, document),
            None => navigate_core(core, &spec.url),
        }
    }

    // the declared ears open on the standing engine
    let console_wired = spec.console && wire_console(core, path);
    let requests_token = if spec.requests { Some(wire_requests(core2, path)) } else { None };

    for command in queued {
        match command {
            Queued::Navigate(url) => unsafe { navigate_core(core, &url) },
            Queued::Back => unsafe {
                let _ = ((*(*core).vtbl).go_back)(core);
            },
            Queued::Forward => unsafe {
                let _ = ((*(*core).vtbl).go_forward)(core);
            },
            Queued::Eval { token, js, raw } => unsafe { eval_core(core, token, &js, raw) },
            Queued::Snapshot { token } => unsafe { snapshot_core(core, token) },
            Queued::Input(event) => unsafe { send_input(core, &event) },
            Queued::Edit(action) => unsafe { edit_core(controller, core, &action) },
        }
    }

    VIEWS.with(|views| {
        let mut views = views.borrow_mut();
        if let Some(Mount::Live(live)) = views.get_mut(path).map(|slot| &mut slot.state) {
            live.console_wired = console_wired;
            live.requests_token = requests_token;
        }
    });
}

// MARK: - The scripts and the wires

/// The page's side of the bus, injected at document start on every
/// navigation. ONE wire exists on this platform, so the mac's four
/// named channels become a tag envelope: `bunny\t…` is the bus and
/// `eval\t…` the answers; `console`/`net` are reserved for a backend
/// that ever serves those by injection.
const BOOT: &str = "window.bunny = { post: function(m) { \
    window.chrome.webview.postMessage('bunny\\t' + String(m)); } };";

/// Wraps a document-created script to the mac's main-frame-only law —
/// a BLOCK, not an IIFE, so a user script's top-level declarations
/// still hoist as `WKUserScript` hoists them. Reading `top`'s
/// identity is permitted cross-origin; the guard never throws.
fn main_frame_only(script: &str) -> String {
    format!("if (self === top) {{ {script} }}")
}

/// The document-start set, in the mac's fixed order: the bus first,
/// then the hooks a backend serves by injection (none here — console
/// and requests are native), then the app's own scripts in
/// declaration order. Ids arrive async and are kept with the
/// generation that asked; a stale arrival removes itself.
fn apply_scripts(core: *mut WebView2, path: &str, generation: u64, spec: &SpecCopy) {
    let mut sources = vec![BOOT.to_string()];
    // the editor for an editable document: its transport first (the
    // one wire, tagged `edit`), the framework's script after
    if let Some(document) = spec.document.as_ref().filter(|document| document.editable) {
        sources.push(format!(
            "window.__bunnyEditor = {{ paste: {}, focus: {}, send: function(line) {{ \
             window.chrome.webview.postMessage('edit\\t' + line); }} }};",
            document.paste, document.focus
        ));
        sources.push(EDITOR_SCRIPT.to_string());
    }
    sources.extend(spec.scripts.iter().map(|script| script.to_string()));
    for source in sources {
        let wrapped = wide(&main_frame_only(&source));
        let completed = {
            let path = path.to_string();
            handler2(IID_ADD_SCRIPT, move |_hr, id| {
                script_added(&path, generation, id as *const u16);
                0
            })
        };
        unsafe {
            let _ = ((*(*core).vtbl).add_script_on_created)(core, wrapped.as_ptr(), completed);
            com_release(completed);
        }
    }
}

/// An id landed. The generation that asked still stands ⇒ keep it for
/// the next re-instruct; a stale or orphaned arrival removes itself —
/// no script from a dead instruct survives.
fn script_added(path: &str, generation: u64, id: *const u16) {
    // a handler ARGUMENT is the engine's string, borrowed
    let id = unsafe { borrow_ws(id) };
    if id.is_empty() {
        return;
    }
    let remove_on = VIEWS.with(|views| {
        let mut views = views.borrow_mut();
        let Some(slot) = views.get_mut(path) else {
            return None;
        };
        if slot.generation == generation {
            slot.script_ids.push((generation, id.clone()));
            return None;
        }
        match &slot.state {
            Mount::Live(live) => Some(live.core),
            _ => None,
        }
    });
    if let Some(core) = remove_on {
        let id = wide(&id);
        unsafe {
            let _ = ((*(*core).vtbl).remove_script_on_created)(core, id.as_ptr());
        }
    }
}

/// The console door: the DevTools Protocol, which sees what an
/// injected hook cannot — messages before any script runs, every
/// frame's context, the page's own `console.error` habits. Answers
/// whether the door opened.
fn wire_console(core: *mut WebView2, path: &str) -> bool {
    unsafe {
        let enabled = {
            let path = path.to_string();
            handler2(IID_DEVTOOLS_CALL, move |hr, _json| {
                let hr = hr as Hresult;
                if !com_ok(hr) {
                    // the refusal lands on the ear that asked
                    dispatch(WebviewEvent::Console {
                        path: path.clone(),
                        line: format!("error: the console door refused (0x{:08X})", hr as u32),
                    });
                }
                0
            })
        };
        let method = wide("Runtime.enable");
        let empty = wide("{}");
        let hr = ((*(*core).vtbl).call_devtools_protocol_method)(
            core,
            method.as_ptr(),
            empty.as_ptr(),
            enabled,
        );
        com_release(enabled);
        if !com_ok(hr) {
            dispatch(WebviewEvent::Console {
                path: path.to_string(),
                line: format!("error: the console door refused (0x{:08X})", hr as u32),
            });
            return false;
        }
        for (event, kind) in [
            ("Runtime.consoleAPICalled", ConsoleChannel::ApiCalled),
            ("Runtime.exceptionThrown", ConsoleChannel::Exception),
        ] {
            let name = wide(event);
            let mut receiver: *mut DevToolsReceiver = std::ptr::null_mut();
            if !com_ok(((*(*core).vtbl).get_devtools_receiver)(core, name.as_ptr(), &mut receiver))
                || receiver.is_null()
            {
                continue;
            }
            let on_event = {
                let path = path.to_string();
                handler2(IID_DEVTOOLS_EVENT, move |_sender, args| {
                    devtools_event(&path, kind, args as *mut c_void);
                    0
                })
            };
            let mut token = 0i64;
            let _ = ((*(*receiver).vtbl).add_event_received)(receiver, on_event, &mut token);
            com_release(on_event);
            com_release(receiver as *mut c_void);
        }
    }
    true
}

#[derive(Clone, Copy)]
enum ConsoleChannel {
    ApiCalled,
    Exception,
}

fn devtools_event(path: &str, kind: ConsoleChannel, args: *mut c_void) {
    let json = unsafe {
        let vtbl = *(args as *mut *const DevToolsArgsVtbl);
        let mut text: *mut u16 = std::ptr::null_mut();
        if !com_ok(((*vtbl).get_parameter_json)(args, &mut text)) {
            return;
        }
        take_ws(text)
    };
    let Some(parsed) = json::parse(&json) else {
        return;
    };
    let line = match kind {
        ConsoleChannel::ApiCalled => console_line(&parsed),
        ConsoleChannel::Exception => exception_line(&parsed),
    };
    let Some(line) = line else {
        return;
    };
    dispatch(WebviewEvent::Console { path: path.to_string(), line });
}

/// `Runtime.consoleAPICalled` → the mac's `"level: what it said"`.
/// Four levels ride (the mac carried exactly four); the exotic types
/// (`dir`, `table`, `trace`, groups) are dropped, so the two
/// platforms stay comparable.
fn console_line(parsed: &json::Json) -> Option<String> {
    let level = match parsed.get("type")?.as_str()? {
        "log" | "debug" => "log",
        "info" => "info",
        "warning" => "warn",
        "error" | "assert" => "error",
        _ => return None,
    };
    let mut parts = Vec::new();
    if let Some(json::Json::Array(items)) = parsed.get("args") {
        for item in items {
            parts.push(remote_object_text(item));
        }
    }
    Some(format!("{level}: {}", parts.join(" ")))
}

/// One CDP RemoteObject, rendered: an unserializable name (`NaN`),
/// a string's raw text (the mac passes string args raw), any other
/// value as its JSON, else the description (`"Object"` — the one
/// honest difference from a stringifying hook), else the type.
fn remote_object_text(object: &json::Json) -> String {
    if let Some(text) = object.get("unserializableValue").and_then(json::Json::as_str) {
        return text.to_string();
    }
    if let Some(value) = object.get("value") {
        if let Some(text) = value.as_str() {
            return text.to_string();
        }
        return value.source();
    }
    if let Some(description) = object.get("description").and_then(json::Json::as_str) {
        return description.lines().next().unwrap_or_default().to_string();
    }
    object
        .get("type")
        .and_then(json::Json::as_str)
        .unwrap_or("?")
        .to_string()
}

/// `Runtime.exceptionThrown` → the mac's uncaught-error leg:
/// `"error: <first line of the description>"`.
fn exception_line(parsed: &json::Json) -> Option<String> {
    let details = parsed.get("exceptionDetails")?;
    let text = details
        .get("exception")
        .and_then(|exception| exception.get("description"))
        .and_then(json::Json::as_str)
        .map(|description| description.lines().next().unwrap_or_default().to_string())
        .or_else(|| details.get("text").and_then(json::Json::as_str).map(str::to_string))?;
    Some(format!("error: {text}"))
}

/// The requests door: `WebResourceResponseReceived`, EVERY resource —
/// documents, images, stylesheets, fetch — where the mac's injected
/// wrap saw fetch and XHR alone. Needs `ICoreWebView2_2` (runtime
/// 88.0.705.50, 2021); an older runtime refuses by name on the ear
/// that asked. One honest difference, named: a request that never
/// gets a response is silent here (the navigation-level failure still
/// reports on the failure leg).
fn wire_requests(core2: Option<*mut WebView2_2>, path: &str) -> i64 {
    let Some(core2) = core2 else {
        dispatch(WebviewEvent::Requested {
            path: path.to_string(),
            line: String::from(
                "requests: refused — this WebView2 runtime predates response observation \
                 (needs 88.0.705.50)",
            ),
        });
        // remembered as spoken: a token of 0 is never granted by the
        // engine, so the refusal speaks once
        return 0;
    };
    let on_response = {
        let path = path.to_string();
        handler2(IID_RESPONSE_RECEIVED, move |_sender, args| {
            response_received(&path, args as *mut c_void);
            0
        })
    };
    let mut token = 0i64;
    unsafe {
        let _ = ((*(*core2).vtbl).add_response_received)(core2, on_response, &mut token);
        com_release(on_response);
    }
    token
}

fn response_received(path: &str, args: *mut c_void) {
    let line = unsafe {
        let vtbl = *(args as *mut *const ResponseReceivedArgsVtbl);
        let mut request: *mut c_void = std::ptr::null_mut();
        if !com_ok(((*vtbl).get_request)(args, &mut request)) || request.is_null() {
            return;
        }
        let request_vtbl = *(request as *mut *const ResourceRequestVtbl);
        let mut method: *mut u16 = std::ptr::null_mut();
        let mut uri: *mut u16 = std::ptr::null_mut();
        let _ = ((*request_vtbl).get_method)(request, &mut method);
        let _ = ((*request_vtbl).get_uri)(request, &mut uri);
        let method = take_ws(method);
        let uri = take_ws(uri);
        com_release(request);
        let mut response: *mut c_void = std::ptr::null_mut();
        let mut status = 0i32;
        if com_ok(((*vtbl).get_response)(args, &mut response)) && !response.is_null() {
            let response_vtbl = *(response as *mut *const ResponseViewVtbl);
            let _ = ((*response_vtbl).get_status_code)(response, &mut status);
            com_release(response);
        }
        format!("{method} {uri} {status}")
    };
    dispatch(WebviewEvent::Requested { path: path.to_string(), line });
}

// MARK: - The wires' landings

/// The one message wire. Only the LEADING fields split, so a payload
/// keeps its tabs: `bunny\t<body>` and
/// `eval\t<token>\t<ok|err>\t<payload>`.
fn parse_message(text: &str) -> Option<Parsed> {
    let (tag, rest) = text.split_once('\t')?;
    match tag {
        "bunny" => Some(Parsed::Posted(rest.to_string())),
        "edit" => Some(Parsed::Edited(rest.to_string())),
        "eval" => {
            let mut parts = rest.splitn(3, '\t');
            let token = parts.next()?.parse::<u64>().ok()?;
            let verdict = parts.next()?;
            let payload = parts.next()?;
            let result = match verdict {
                "ok" => Ok(payload.to_string()),
                "err" => Err(payload.to_string()),
                _ => return None,
            };
            Some(Parsed::EvalDone { token, result })
        }
        _ => None,
    }
}

enum Parsed {
    Posted(String),
    /// The editor's line, still to decode (`bunny_ui::host::editor_report`).
    Edited(String),
    EvalDone { token: u64, result: Result<String, String> },
}

fn message_received(path: &str, args: *mut c_void) {
    let text = unsafe {
        let vtbl = *(args as *mut *const MessageArgsVtbl);
        let mut text: *mut u16 = std::ptr::null_mut();
        if !com_ok(((*vtbl).try_get_string)(args, &mut text)) {
            // a non-string post is a page poking the private channel —
            // the boot script and the eval wrapper send strings only
            return;
        }
        take_ws(text)
    };
    match parse_message(&text) {
        Some(Parsed::Posted(body)) => {
            dispatch(WebviewEvent::Posted { path: path.to_string(), body });
        }
        Some(Parsed::EvalDone { token, result }) => {
            dispatch(WebviewEvent::EvalDone { token, result });
        }
        Some(Parsed::Edited(line)) => match editor_report(&line) {
            Some(EditorReport::Changed(html)) => {
                dispatch(WebviewEvent::Changed { path: path.to_string(), html });
            }
            Some(EditorReport::Pasted { html, text }) => {
                dispatch(WebviewEvent::Pasted { path: path.to_string(), html, text });
            }
            None => {}
        },
        None => {}
    }
}

/// The starting leg. A page by url records where each navigation
/// aims (the failure leg's answer) and lets it go. A DOCUMENT is
/// answered by its one rule, the mac's verbatim: the app's own load
/// goes through, a navigation the PERSON started — a link — is
/// cancelled and reported to the app (the document never follows
/// it), and every other ask — a refresh the document wrote, a form,
/// the engine's own reload (which would fetch the base) — is
/// cancelled without a word.
fn navigation_starting(path: &str, args: *mut c_void) {
    let (id, uri, by_hand) = unsafe {
        let vtbl = *(args as *mut *const NavStartingArgsVtbl);
        let mut uri: *mut u16 = std::ptr::null_mut();
        let mut id = 0u64;
        let mut by_hand = 0i32;
        let _ = ((*vtbl).get_uri)(args, &mut uri);
        let _ = ((*vtbl).get_navigation_id)(args, &mut id);
        let _ = ((*vtbl).get_is_user_initiated)(args, &mut by_hand);
        (id, take_ws(uri), by_hand != 0)
    };
    enum Verdict {
        Page,
        Load,
        Link,
        Shut,
    }
    let verdict = VIEWS.with(|views| {
        let mut views = views.borrow_mut();
        let Some(Mount::Live(live)) = views.get_mut(path).map(|slot| &mut slot.state) else {
            return Verdict::Page;
        };
        let Some(letter) = live.letter.as_mut() else {
            // a redirect re-fires the same id with the new aim; the
            // insert overwrites, which is the truth
            if live.nav_targets.len() >= 32 {
                live.nav_targets.clear();
            }
            live.nav_targets.insert(id, uri.clone());
            return Verdict::Page;
        };
        let expected = std::mem::replace(&mut letter.expected, false);
        if by_hand {
            Verdict::Link
        } else if expected {
            Verdict::Load
        } else {
            Verdict::Shut
        }
    });
    match verdict {
        Verdict::Page | Verdict::Load => {}
        Verdict::Link | Verdict::Shut => {
            unsafe {
                let vtbl = *(args as *mut *const NavStartingArgsVtbl);
                let _ = ((*vtbl).put_cancel)(args, 1);
            }
            if matches!(verdict, Verdict::Link) {
                report_link(path, uri);
            }
        }
    }
}

/// A `target="_blank"` link asked for a window. A document's link
/// reports to the app and the ask is HANDLED — no window opens; a
/// page by url keeps what it had: the engine's own popup.
fn new_window_requested(path: &str, args: *mut c_void) {
    let sealed = VIEWS.with(|views| {
        matches!(
            views.borrow().get(path).map(|slot| &slot.state),
            Some(Mount::Live(live)) if live.letter.is_some()
        )
    });
    if !sealed {
        return;
    }
    let uri = unsafe {
        let vtbl = *(args as *mut *const NewWindowArgsVtbl);
        let mut uri: *mut u16 = std::ptr::null_mut();
        let _ = ((*vtbl).get_uri)(args, &mut uri);
        let _ = ((*vtbl).put_handled)(args, 1);
        take_ws(uri)
    };
    report_link(path, uri);
}

/// A document's link, to the app — unless it is a `javascript:` link,
/// which is not a place and runs nowhere.
fn report_link(path: &str, url: String) {
    let scheme = url.split(':').next().unwrap_or_default();
    if url.is_empty() || scheme.eq_ignore_ascii_case("javascript") {
        return;
    }
    dispatch(WebviewEvent::Linked { path: path.to_string(), url });
}

/// The commit leg — the engine's own committed url, redirects
/// included, the very string `update` compares. `about:blank` is the
/// engine's empty stage, never a page the app asked for; an error
/// page cannot arrive (disabled), but the belt stays.
fn content_loading(path: &str, sender: *mut WebView2, args: *mut c_void) {
    unsafe {
        let vtbl = *(args as *mut *const ContentLoadingArgsVtbl);
        let mut error_page = 0i32;
        let _ = ((*vtbl).get_is_error_page)(args, &mut error_page);
        if error_page != 0 {
            return;
        }
    }
    // the visitor's motion is re-armed on every commit — the
    // emulation is the session's, and a renderer swap must not shed it
    let armed = VIEWS.with(|views| {
        let views = views.borrow();
        matches!(
            views.get(path).map(|slot| &slot.state),
            Some(Mount::Live(live)) if live.full_motion
        )
    });
    if armed {
        unsafe { cdp(sender, "Emulation.setEmulatedMedia", &emulated_media_params(true)) };
    }
    // a document's commit also shuts the door its own load came
    // through: from here on nothing the document asks for moves it
    let base = VIEWS.with(|views| {
        let mut views = views.borrow_mut();
        let slot = views.get_mut(path)?;
        let Mount::Live(live) = &mut slot.state else {
            return None;
        };
        let letter = live.letter.as_mut()?;
        letter.expected = false;
        if std::mem::replace(&mut letter.focus, false) {
            unsafe { move_focus(live.controller) };
        }
        slot.spec.document.as_ref().map(|document| document.base.to_string())
    });
    let url = unsafe { source_of(sender) };
    if url.is_empty() || url == "about:blank" {
        // a document from memory stands on the engine's empty stage:
        // its commit reports the base it resolves by, the mac's very
        // string, so the two shells say the same thing
        if let Some(base) = base.filter(|base| !base.is_empty()) {
            dispatch(WebviewEvent::Navigated { path: path.to_string(), url: base });
        }
        return;
    }
    dispatch(WebviewEvent::Navigated { path: path.to_string(), url });
}

/// The refusal leg. A load ANOTHER navigation replaced answers
/// `OPERATION_CANCELED` and never reports — the one that replaced it
/// answers for both (the mac's `-999` rule). Everything else answers
/// with the url it was AIMING at and the status's name.
fn navigation_completed(path: &str, sender: *mut WebView2, args: *mut c_void) {
    const OPERATION_CANCELED: i32 = 14;
    let (success, status, id) = unsafe {
        let vtbl = *(args as *mut *const NavCompletedArgsVtbl);
        let mut success = 0i32;
        let mut status = 0i32;
        let mut id = 0u64;
        let _ = ((*vtbl).get_is_success)(args, &mut success);
        let _ = ((*vtbl).get_web_error_status)(args, &mut status);
        let _ = ((*vtbl).get_navigation_id)(args, &mut id);
        (success != 0, status, id)
    };
    let aimed = VIEWS.with(|views| {
        let mut views = views.borrow_mut();
        match views.get_mut(path).map(|slot| &mut slot.state) {
            Some(Mount::Live(live)) => live.nav_targets.remove(&id),
            _ => None,
        }
    });
    if success || status == OPERATION_CANCELED {
        return;
    }
    let url = match aimed {
        Some(url) if !url.is_empty() => url,
        _ => unsafe { source_of(sender) },
    };
    dispatch(WebviewEvent::NavigationFailed {
        path: path.to_string(),
        url,
        why: error_name(status),
    });
}

/// The status, as a sentence a person can read — a NAME, never a
/// number (`COREWEBVIEW2_WEB_ERROR_STATUS`, all 19 values).
fn error_name(status: i32) -> String {
    match status {
        1 => String::from("the certificate's name does not match the site"),
        2 => String::from("the certificate expired"),
        3 => String::from("the client certificate contains errors"),
        4 => String::from("the certificate was revoked"),
        5 => String::from("the certificate is invalid"),
        6 => String::from("the server is unreachable"),
        7 => String::from("the connection timed out"),
        8 => String::from("the server answered nonsense"),
        9 => String::from("the connection was aborted"),
        10 => String::from("the connection was reset"),
        11 => String::from("the network disconnected"),
        12 => String::from("the connection could not be made"),
        13 => String::from("the host name did not resolve"),
        14 => String::from("the load was canceled"),
        15 => String::from("a redirect failed"),
        16 => String::from("the engine met an unexpected error"),
        17 => String::from("the site wants credentials"),
        18 => String::from("the proxy wants credentials"),
        _ => String::from("the engine refused unnamed"),
    }
}

/// Where the engine is right now — the committed url, CoTaskMem taken.
unsafe fn source_of(core: *mut WebView2) -> String {
    unsafe {
        let mut uri: *mut u16 = std::ptr::null_mut();
        if !com_ok(((*(*core).vtbl).get_source)(core, &mut uri)) {
            return String::new();
        }
        take_ws(uri)
    }
}

// MARK: - Commands

/// Points the engine at `url` — the load is the engine's own affair,
/// asynchronous and cancellable by the next call.
unsafe fn navigate_core(core: *mut WebView2, url: &str) {
    let url = wide(url);
    unsafe {
        let _ = ((*(*core).vtbl).navigate)(core, url.as_ptr());
    }
}

/// Loads a document from MEMORY — `NavigateToString`, the sealed html
/// the spec holds (the base rides in its head: this engine has no
/// door of its own for one). The engine's door takes two megabytes;
/// a larger letter is refused BY NAME on the failure leg, never
/// truncated into a quiet half-page.
unsafe fn load_document(core: *mut WebView2, path: &str, document: &Document) {
    const DOOR: usize = 2 * 1024 * 1024;
    let sealed = document.sealed();
    if sealed.len() > DOOR {
        VIEWS.with(|views| {
            if let Some(Mount::Live(live)) =
                views.borrow_mut().get_mut(path).map(|slot| &mut slot.state)
            {
                if let Some(letter) = live.letter.as_mut() {
                    letter.expected = false;
                }
            }
        });
        dispatch(WebviewEvent::NavigationFailed {
            path: path.to_string(),
            url: document.base.to_string(),
            why: String::from("the document is larger than the engine's two-megabyte door"),
        });
        return;
    }
    let html = wide(&sealed);
    unsafe {
        let _ = ((*(*core).vtbl).navigate_to_string)(core, html.as_ptr());
    }
}

/// The keyboard becomes the page's — `MoveFocus`, programmatic.
unsafe fn move_focus(controller: *mut Controller) {
    const PROGRAMMATIC: i32 = 0;
    unsafe {
        let _ = ((*(*controller).vtbl).move_focus)(controller, PROGRAMMATIC);
    }
}

/// One editing action on the document — the allowlist's script, run
/// on the engine. The editor takes the keyboard back first (a toolbar
/// click took it), except for the app's own write of the whole body.
unsafe fn edit_core(controller: *mut Controller, core: *mut WebView2, action: &EditorAction) {
    let script = action.script();
    if script.is_empty() {
        return;
    }
    unsafe {
        if !matches!(action, EditorAction::SetHtml(_)) {
            move_focus(controller);
        }
        run_script(core, &script);
    }
}

/// Runs `js` on the page, answer discarded — the shared no-op handler
/// takes the completion.
unsafe fn run_script(core: *mut WebView2, js: &str) {
    let js = wide(js);
    let completed = handler2(IID_EXECUTE_SCRIPT, |_hr, _json| 0);
    unsafe {
        let _ = ((*(*core).vtbl).execute_script)(core, js.as_ptr(), completed);
        com_release(completed);
    }
}

/// Evaluates `js` as an EXPRESSION in the page; the answer rides the
/// bus by token — the mac wrapper verbatim with the door swapped.
/// `ExecuteScript`'s own callback cannot serve the contract (a thrown
/// script answers `null` with a success code), so it gets a shared
/// no-op handler and the wrapper does the reporting.
unsafe fn eval_core(core: *mut WebView2, token: u64, js: &str, raw: bool) {
    let serialize = if raw {
        "(__v === undefined || __v === null) ? \"\" : String(__v)"
    } else {
        "JSON.stringify(__v)"
    };
    let wrapped = format!(
        "(function() {{ try {{ \
           var __v = (function() {{ return ( {js} ); }})(); \
           var __s = {serialize}; \
           window.chrome.webview.postMessage(\
             \"eval\\t{token}\\tok\\t\" + (__s === undefined ? \"null\" : __s)); \
         }} catch (e) {{ \
           window.chrome.webview.postMessage(\
             \"eval\\t{token}\\terr\\t\" + String(e)); \
         }} }})();"
    );
    unsafe { run_script(core, &wrapped) }
}

/// `COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG`.
const CAPTURE_PNG: i32 = 0;
/// `STREAM_SEEK_SET`.
const SEEK_SET: u32 = 0;

/// The page as an image: `CapturePreview` renders the visible
/// viewport as PNG into a memory stream; the completion decodes it to
/// straight RGBA through the platform's own decoder. The answer rides
/// the dispatch by token, like an eval's — or a refusal by name.
unsafe fn snapshot_core(core: *mut WebView2, token: u64) {
    let stream = crate::image::empty_stream();
    if stream.is_null() {
        dispatch(WebviewEvent::SnapshotDone {
            token,
            result: Err(String::from("no memory stream for the snapshot")),
        });
        return;
    }
    let completed = handler1(IID_CAPTURE_PREVIEW, move |hr| {
        let hr = hr as Hresult;
        let result = if com_ok(hr) {
            unsafe { read_snapshot(stream) }
        } else {
            Err(format!("the engine refused the capture (0x{:08X})", hr as u32))
        };
        unsafe {
            com_release(stream);
        }
        dispatch(WebviewEvent::SnapshotDone { token, result });
    });
    unsafe {
        let hr = ((*(*core).vtbl).capture_preview)(core, CAPTURE_PNG, stream, completed);
        com_release(completed);
        if !com_ok(hr) {
            // the handler will not fire — release the stream's one
            // reference here and answer now
            com_release(stream);
            dispatch(WebviewEvent::SnapshotDone {
                token,
                result: Err(format!("the engine refused the capture (0x{:08X})", hr as u32)),
            });
        }
    }
}

/// The PNG bytes back out of the stream, decoded by WIC to the tight
/// straight RGBA the contract promises.
unsafe fn read_snapshot(stream: *mut c_void) -> Result<(usize, usize, Vec<u8>), String> {
    unsafe {
        let vtbl = *(stream as *mut *const StreamVtbl);
        let mut landed = 0u64;
        if !com_ok(((*vtbl).seek)(stream, 0, SEEK_SET, &mut landed)) {
            return Err(String::from("the snapshot stream would not rewind"));
        }
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 64 * 1024];
        loop {
            let mut got = 0u32;
            let hr = ((*vtbl).read)(stream, chunk.as_mut_ptr(), chunk.len() as u32, &mut got);
            if got > 0 {
                bytes.extend_from_slice(&chunk[..got as usize]);
            }
            // S_FALSE (1) is the end; a failure is a refusal
            if !com_ok(hr) {
                return Err(String::from("the snapshot stream would not read"));
            }
            if got == 0 {
                break;
            }
        }
        if bytes.is_empty() {
            return Err(String::from("the engine answered no pixels"));
        }
        crate::image::decode_rgba(&bytes)
    }
}

// MARK: - The public(crate) surface (the seam lib.rs drives)

/// Re-instructs a MOUNTED page after its spec changed: the scripts
/// are replaced (they take effect on the next navigation, exactly as
/// `WKUserContentController` re-instruction does), the declared ears
/// re-apply, and the page re-points — AFTER comparing with where the
/// engine already is (the mac's d52c1b8 lesson: an app that folds the
/// committed url back into its spec must not reload the page the
/// engine just arrived at). While pending, the newest spec simply
/// replaces the parked one — the land sequence reads the latest.
pub(crate) fn update(path: &str, spec: &HostSpec) {
    let copied = SpecCopy::of(spec);
    struct ReInstruct {
        core: *mut WebView2,
        core2: Option<*mut WebView2_2>,
        generation: u64,
        ids: Vec<(u64, String)>,
        want_console: bool,
        want_requests: bool,
        /// `Some(now)` when the motion emulation must flip — unlike
        /// the ears, this door swings BOTH ways (an empty value hands
        /// the media feature back to the OS).
        retune_motion: Option<bool>,
    }
    let step = VIEWS.with(|views| {
        let mut views = views.borrow_mut();
        let slot = views.get_mut(path)?;
        slot.spec = copied.clone();
        match &slot.state {
            Mount::Live(live) => {
                // the generation bump is the STAMP script arrivals
                // check — bumped only here, so an update while the
                // controller is still in flight never tombstones it
                // (the land sequence reads the latest spec anyway)
                slot.generation += 1;
                Some(ReInstruct {
                    core: live.core,
                    core2: live.core2,
                    generation: slot.generation,
                    ids: std::mem::take(&mut slot.script_ids),
                    want_console: copied.console && !live.console_wired,
                    want_requests: copied.requests && live.requests_token.is_none(),
                    retune_motion: (copied.full_motion != live.full_motion)
                        .then_some(copied.full_motion),
                })
            }
            _ => None,
        }
    });
    let Some(step) = step else {
        return;
    };
    unsafe {
        for (_, id) in step.ids {
            let id = wide(&id);
            let _ = ((*(*step.core).vtbl).remove_script_on_created)(step.core, id.as_ptr());
        }
    }
    apply_scripts(step.core, path, step.generation, &copied);
    let console_wired = step.want_console && wire_console(step.core, path);
    let requests_token = if step.want_requests { Some(wire_requests(step.core2, path)) } else { None };
    if let Some(now) = step.retune_motion {
        unsafe { cdp(step.core, "Emulation.setEmulatedMedia", &emulated_media_params(now)) };
    }
    if console_wired || requests_token.is_some() || step.retune_motion.is_some() {
        VIEWS.with(|views| {
            let mut views = views.borrow_mut();
            if let Some(Mount::Live(live)) = views.get_mut(path).map(|slot| &mut slot.state) {
                live.console_wired |= console_wired;
                if requests_token.is_some() {
                    live.requests_token = requests_token;
                }
                if let Some(now) = step.retune_motion {
                    live.full_motion = now;
                }
            }
        });
    }
    match &copied.document {
        Some(document) => {
            // the same letter never reloads; a changed one always does
            let stale = VIEWS.with(|views| {
                let mut views = views.borrow_mut();
                let Some(Mount::Live(live)) = views.get_mut(path).map(|slot| &mut slot.state)
                else {
                    return false;
                };
                if live.letter.as_ref().is_some_and(|letter| letter.digest == document.digest) {
                    return false;
                }
                live.letter = Some(Letter {
                    digest: document.digest,
                    expected: true,
                    focus: document.focus,
                });
                true
            });
            if stale {
                unsafe { load_document(step.core, path, document) };
            }
        }
        None => {
            // a view that goes from a document back to a url closes
            // the letter: the page follows its own links again
            VIEWS.with(|views| {
                if let Some(Mount::Live(live)) =
                    views.borrow_mut().get_mut(path).map(|slot| &mut slot.state)
                {
                    live.letter = None;
                }
            });
            unsafe {
                if source_of(step.core) != *copied.url {
                    navigate_core(step.core, &copied.url);
                }
            }
        }
    }
}

/// The tenant's rect (container-local physical px) and visibility —
/// every visible frame from `host_place`, stored while pending so the
/// land applies the latest.
pub(crate) fn place(path: &str, bounds: (i32, i32, i32, i32), shown: bool) {
    let live = VIEWS.with(|views| {
        let mut views = views.borrow_mut();
        let slot = views.get_mut(path)?;
        let moved = slot.bounds != bounds || slot.shown != shown;
        slot.bounds = bounds;
        slot.shown = shown;
        match &slot.state {
            Mount::Live(live) if moved => Some(live.controller),
            _ => None,
        }
    });
    if let Some(controller) = live {
        unsafe {
            let _ = ((*(*controller).vtbl).put_bounds)(
                controller,
                Rect {
                    left: bounds.0,
                    top: bounds.1,
                    right: bounds.0 + bounds.2,
                    bottom: bounds.1 + bounds.3,
                },
            );
            let _ = ((*(*controller).vtbl).put_is_visible)(controller, shown as i32);
        }
    }
}

/// The host left the scene: `Close()` first (the one call that breaks
/// the host↔browser cycle — without it the browser processes linger),
/// then every held reference. Parked evals answer `Err` — the
/// tombstone rule; a controller that lands later finds no slot and
/// closes itself.
pub(crate) fn sweep(path: &str) {
    let removed = VIEWS.with(|views| views.borrow_mut().remove(path));
    let Some(slot) = removed else {
        return;
    };
    for command in slot.queued {
        match command {
            Queued::Eval { token, .. } => {
                dispatch(WebviewEvent::EvalDone {
                    token,
                    result: Err(String::from("the webview is not mounted")),
                });
            }
            Queued::Snapshot { token } => {
                dispatch(WebviewEvent::SnapshotDone {
                    token,
                    result: Err(String::from("the webview is not mounted")),
                });
            }
            _ => {}
        }
    }
    if let Mount::Live(live) = slot.state {
        unsafe {
            let _ = ((*(*live.controller).vtbl).close)(live.controller);
            com_release(live.controller as *mut c_void);
            com_release(live.core as *mut c_void);
            if let Some(core2) = live.core2 {
                com_release(core2 as *mut c_void);
            }
        }
    }
}

/// The window moved: the engine repositions its own popups — the
/// `<select>` dropdown, autofill — after the parent's move lands.
pub(crate) fn nudge_all() {
    let controllers: Vec<*mut Controller> = VIEWS.with(|views| {
        views
            .borrow()
            .values()
            .filter_map(|slot| match &slot.state {
                Mount::Live(live) => Some(live.controller),
                _ => None,
            })
            .collect()
    });
    for controller in controllers {
        unsafe {
            let _ = ((*(*controller).vtbl).notify_parent_window_position_changed)(controller);
        }
    }
}

/// The window is dying: every controller closes BEFORE the HWNDs go
/// (the d3d law — the swapchain must not outlive its window —
/// extended to the tenants), and the environment's process reference
/// releases. The client DLL is never unloaded: the browser stack
/// unwinds asynchronously, and unloading a COM server mid-teardown is
/// a classic hang — process exit reclaims it.
pub(crate) fn teardown_all() {
    let paths: Vec<String> = VIEWS.with(|views| views.borrow().keys().cloned().collect());
    for path in paths {
        sweep(&path);
    }
    let environment = ENVIRONMENT.with(|state| {
        let mut state = state.borrow_mut();
        match std::mem::replace(&mut *state, EnvState::Unresolved) {
            EnvState::Ready(environment) => Some(environment),
            other => {
                *state = other;
                None
            }
        }
    });
    if let Some(environment) = environment {
        unsafe {
            com_release(environment as *mut c_void);
        }
    }
}

/// The imperative doors, path-addressed — queued while the engine
/// assembles, spent on it once Live. A handle bound to nothing is the
/// DRAIN's refusal (`lib.rs` answers those tokens on the spot).
pub(crate) fn navigate(path: &str, url: &str) {
    let url: Rc<str> = Rc::from(url);
    match ask(path, Queued::Navigate(Rc::clone(&url))) {
        Asked::Live(core) => unsafe { navigate_core(core, &url) },
        // every ASKED load answers: pointing a refused mount somewhere
        // reports on the failure leg again
        Asked::Refused(why) => dispatch(WebviewEvent::NavigationFailed {
            path: path.to_string(),
            url: url.to_string(),
            why: why.to_string(),
        }),
        Asked::Queued | Asked::Unknown => {}
    }
}

pub(crate) fn back(path: &str) {
    if let Asked::Live(core) = ask(path, Queued::Back) {
        unsafe {
            let _ = ((*(*core).vtbl).go_back)(core);
        }
    }
}

pub(crate) fn forward(path: &str) {
    if let Asked::Live(core) = ask(path, Queued::Forward) {
        unsafe {
            let _ = ((*(*core).vtbl).go_forward)(core);
        }
    }
}

/// `Ok` = asked (or queued); `Err` = answer the token NOW with this
/// sentence — the drain speaks it so no token is ever silenced.
pub(crate) fn eval(path: &str, token: u64, js: &str, raw: bool) -> Result<(), String> {
    let js: Rc<str> = Rc::from(js);
    match ask(path, Queued::Eval { token, js: Rc::clone(&js), raw }) {
        Asked::Live(core) => unsafe { eval_core(core, token, &js, raw) },
        Asked::Refused(why) => return Err(why.to_string()),
        Asked::Unknown => return Err(String::from("the webview is not mounted")),
        Asked::Queued => {}
    }
    Ok(())
}

pub(crate) fn snapshot(path: &str, token: u64) -> Result<(), String> {
    match ask(path, Queued::Snapshot { token }) {
        Asked::Live(core) => unsafe { snapshot_core(core, token) },
        Asked::Refused(why) => return Err(why.to_string()),
        Asked::Unknown => return Err(String::from("the webview is not mounted")),
        Asked::Queued => {}
    }
    Ok(())
}

/// One synthetic event into the page — the capability the table calls
/// `SyntheticInput`, served here by the DevTools Protocol's `Input`
/// domain: the event enters the browser's own input pipeline, above
/// the renderer, so the page reads `isTrusted` as true — the same
/// road every browser-automation hand rides, and the whole reason the
/// door exists (a synthetic DOM event, real sites refuse).
///
/// Coordinates are CSS px from the viewport's top-left — exactly the
/// contract's own words, so there is no flip and no DPI math (the
/// mac's window-point machinery has no twin). The scroll deltas pass
/// through UNCHANGED: `WebviewInput`'s signs are the page's, and so
/// are the wheel event's (the mac negated because CoreGraphics counts
/// the other way; nothing to do here). No closed-ears guard either —
/// a protocol event can never walk back into the shell's own window
/// procedure, and a key the page declines stays declined (a child of
/// another process does not bubble; the one road back is the
/// accelerator, which a synthetic event never rides).
/// One editing action on the document — spent now on a standing
/// engine (with its controller, for the keyboard), parked while the
/// mount assembles, dropped for a mount that never came: the door is
/// fire-and-forget, and there is no answer to refuse in.
pub(crate) fn edit(path: &str, action: &EditorAction) {
    let controller = VIEWS.with(|views| match views.borrow().get(path).map(|slot| &slot.state) {
        Some(Mount::Live(live)) => Some(live.controller),
        _ => None,
    });
    let asked = ask(path, Queued::Edit(action.clone()));
    if let (Asked::Live(core), Some(controller)) = (asked, controller) {
        unsafe { edit_core(controller, core, action) };
    }
}

pub(crate) fn input(path: &str, event: &WebviewInput) {
    if let Asked::Live(core) = ask(path, Queued::Input(event.clone())) {
        unsafe { send_input(core, event) }
    }
}

/// The buttons a page reads by mask: 1 left, 2 right, 4 middle.
const fn button_mask(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Right => 2,
        MouseButton::Middle => 4,
    }
}

const fn button_name(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
    }
}

/// What the keyboard was holding, in the protocol's own bits
/// (Alt 1, Ctrl 2, Meta 4, Shift 8) — mapped by the shell's key law:
/// `option` is Alt and `command` is Ctrl, the platform's accelerator.
const fn cdp_modifiers(modifiers: Modifiers) -> u32 {
    (modifiers.option as u32)
        | ((modifiers.command as u32) << 1)
        | ((modifiers.control as u32) << 2)
        | ((modifiers.shift as u32) << 3)
}

/// One mouse event, spelled for the protocol.
unsafe fn mouse(
    core: *mut WebView2,
    kind: &str,
    x: f64,
    y: f64,
    button: &str,
    buttons: u32,
    clicks: u32,
    modifiers: u32,
    wheel: Option<(f64, f64)>,
) {
    let mut params = format!(
        "{{\"type\":\"{kind}\",\"x\":{x},\"y\":{y},\"button\":\"{button}\",\
         \"buttons\":{buttons},\"clickCount\":{clicks},\"modifiers\":{modifiers},\
         \"pointerType\":\"mouse\""
    );
    if let Some((dx, dy)) = wheel {
        params.push_str(&format!(",\"deltaX\":{dx},\"deltaY\":{dy}"));
    }
    params.push('}');
    unsafe { cdp(core, "Input.dispatchMouseEvent", &params) }
}

unsafe fn send_input(core: *mut WebView2, event: &WebviewInput) {
    unsafe {
        match event {
            WebviewInput::Click { x, y, clicks, button } => {
                // a double click is two PAIRS, counted 1 then 2 — the
                // page reads the count off the second press, and one
                // press carrying a 2 is a lie a hand never tells
                let (name, mask) = (button_name(*button), button_mask(*button));
                for count in 1..=(*clicks).clamp(1, 3) {
                    mouse(core, "mousePressed", *x, *y, name, mask, count, 0, None);
                    mouse(core, "mouseReleased", *x, *y, name, 0, count, 0, None);
                }
            }
            WebviewInput::Hover { x, y } => {
                mouse(core, "mouseMoved", *x, *y, "none", 0, 0, 0, None);
            }
            WebviewInput::Down { x, y, button, clicks, modifiers } => {
                mouse(
                    core,
                    "mousePressed",
                    *x,
                    *y,
                    button_name(*button),
                    button_mask(*button),
                    (*clicks).clamp(1, 3),
                    cdp_modifiers(*modifiers),
                    None,
                );
            }
            WebviewInput::Drag { x, y, button, modifiers } => {
                // a move with the mask still held is what the page
                // reads a drag by
                mouse(
                    core,
                    "mouseMoved",
                    *x,
                    *y,
                    "none",
                    button_mask(*button),
                    0,
                    cdp_modifiers(*modifiers),
                    None,
                );
            }
            WebviewInput::Up { x, y, button, clicks, modifiers } => {
                mouse(
                    core,
                    "mouseReleased",
                    *x,
                    *y,
                    button_name(*button),
                    0,
                    (*clicks).clamp(1, 3),
                    cdp_modifiers(*modifiers),
                    None,
                );
            }
            WebviewInput::Scroll { x, y, dx, dy } => {
                mouse(core, "mouseWheel", *x, *y, "none", 0, 0, 0, Some((*dx, *dy)));
            }
            WebviewInput::Type { text } => {
                // the commit door — the protocol's own IME/paste
                // insertion, the `insertText:replacementRange:` twin:
                // it lands in a field, in a contenteditable, under a
                // keyboard layout nobody guessed
                let quoted = json::Json::Str(text.to_string()).source();
                cdp(core, "Input.insertText", &format!("{{\"text\":{quoted}}}"));
            }
            WebviewInput::Key { key } => send_key(core, key),
        }
    }
}

/// One named key: `(vk, code, text)` in the page's own vocabulary — a
/// name nobody knows presses nothing. Pure, so the table is testable.
fn key_spec(key: &str) -> Option<(u32, &'static str, &'static str, Option<&'static str>)> {
    Some(match key {
        "Enter" | "Return" => (0x0D, "Enter", "Enter", Some("\r")),
        "Tab" => (0x09, "Tab", "Tab", None),
        "Escape" | "Esc" => (0x1B, "Escape", "Escape", None),
        "Backspace" => (0x08, "Backspace", "Backspace", None),
        "Delete" => (0x2E, "Delete", "Delete", None),
        "ArrowUp" => (0x26, "ArrowUp", "ArrowUp", None),
        "ArrowDown" => (0x28, "ArrowDown", "ArrowDown", None),
        "ArrowLeft" => (0x25, "ArrowLeft", "ArrowLeft", None),
        "ArrowRight" => (0x27, "ArrowRight", "ArrowRight", None),
        "Home" => (0x24, "Home", "Home", None),
        "End" => (0x23, "End", "End", None),
        "PageUp" => (0x21, "PageUp", "PageUp", None),
        "PageDown" => (0x22, "PageDown", "PageDown", None),
        "Space" => (0x20, " ", "Space", Some(" ")),
        _ => return None,
    })
}

/// One press and release of a key, delivered to the page. A key that
/// TYPES goes down as `keyDown` with its text (the page hears the
/// character too); one that does not goes down raw.
unsafe fn send_key(core: *mut WebView2, key: &str) {
    unsafe {
        if let Some((vk, name, code, text)) = key_spec(key) {
            let name = json::Json::Str(name.to_string()).source();
            let down_kind = if text.is_some() { "keyDown" } else { "rawKeyDown" };
            let mut down = format!(
                "{{\"type\":\"{down_kind}\",\"key\":{name},\"code\":\"{code}\",\
                 \"windowsVirtualKeyCode\":{vk},\"nativeVirtualKeyCode\":{vk}"
            );
            if let Some(text) = text {
                let text = json::Json::Str(text.to_string()).source();
                down.push_str(&format!(",\"text\":{text},\"unmodifiedText\":{text}"));
            }
            down.push('}');
            cdp(core, "Input.dispatchKeyEvent", &down);
            cdp(
                core,
                "Input.dispatchKeyEvent",
                &format!(
                    "{{\"type\":\"keyUp\",\"key\":{name},\"code\":\"{code}\",\
                     \"windowsVirtualKeyCode\":{vk},\"nativeVirtualKeyCode\":{vk}}}"
                ),
            );
            return;
        }
        // a single character IS its own key name, typed; anything
        // else presses nothing
        if key.chars().count() == 1 {
            let quoted = json::Json::Str(key.to_string()).source();
            cdp(
                core,
                "Input.dispatchKeyEvent",
                &format!(
                    "{{\"type\":\"keyDown\",\"key\":{quoted},\"text\":{quoted},\
                     \"unmodifiedText\":{quoted}}}"
                ),
            );
            cdp(
                core,
                "Input.dispatchKeyEvent",
                &format!("{{\"type\":\"keyUp\",\"key\":{quoted}}}"),
            );
        }
    }
}

/// The media emulation's one sentence to the protocol: on, the page
/// sees `prefers-reduced-motion: no-preference`; off, the EMPTY value
/// hands the feature back to the OS — the protocol's own way to say
/// "no override", never a guessed preference.
fn emulated_media_params(full: bool) -> String {
    let value = if full { "no-preference" } else { "" };
    format!("{{\"features\":[{{\"name\":\"prefers-reduced-motion\",\"value\":\"{value}\"}}]}}")
}

/// One protocol call whose answer nobody reads — the input door is
/// fire-and-forget, like the hand it stands for.
unsafe fn cdp(core: *mut WebView2, method: &str, params: &str) {
    let method = wide(method);
    let params = wide(params);
    let completed = handler2(IID_DEVTOOLS_CALL, |_hr, _json| 0);
    unsafe {
        let _ = ((*(*core).vtbl).call_devtools_protocol_method)(
            core,
            method.as_ptr(),
            params.as_ptr(),
            completed,
        );
        com_release(completed);
    }
}

/// What survives the trim: the WebKit menu's own vocabulary — the
/// navigation row, the editing set a field needs, the copies a link
/// or an image offers, and Inspect (the devtools door). Everything
/// the trim does not NAME leaves, so a new Edge row never sneaks in.
const MENU_KEEP: [&str; 14] = [
    "back",
    "forward",
    "reload",
    "inspectElement",
    "undo",
    "redo",
    "cut",
    "copy",
    "paste",
    "pasteAndMatchStyle",
    "selectAll",
    "copyLinkLocation",
    "copyImage",
    "copyImageLocation",
];

/// `COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR`.
const MENU_SEPARATOR: i32 = 3;

/// One item's name and kind, read and released.
unsafe fn menu_item_facts(items: *mut c_void, index: u32) -> Option<(String, i32)> {
    unsafe {
        let items_vtbl = *(items as *mut *const MenuItemsVtbl);
        let mut item: *mut c_void = std::ptr::null_mut();
        if !com_ok(((*items_vtbl).get_value_at_index)(items, index, &mut item))
            || item.is_null()
        {
            return None;
        }
        let item_vtbl = *(item as *mut *const MenuItemVtbl);
        let mut name: *mut u16 = std::ptr::null_mut();
        let _ = ((*item_vtbl).get_name)(item, &mut name);
        let mut kind = 0i32;
        let _ = ((*item_vtbl).get_kind)(item, &mut kind);
        let name = take_ws(name);
        com_release(item);
        Some((name, kind))
    }
}

/// The engine's menu, wearing the mac's cut: rows the keep-list does
/// not name leave (back to front, so the indexes hold), and then the
/// separators settle — never first, never last, never doubled. The
/// engine still draws and runs what remains; `Handled` is never set.
fn trim_context_menu(args: *mut c_void) {
    unsafe {
        let vtbl = *(args as *mut *const ContextMenuArgsVtbl);
        let mut items: *mut c_void = std::ptr::null_mut();
        if !com_ok(((*vtbl).get_menu_items)(args, &mut items)) || items.is_null() {
            return;
        }
        let items_vtbl = *(items as *mut *const MenuItemsVtbl);
        let mut count = 0u32;
        let _ = ((*items_vtbl).get_count)(items, &mut count);
        for index in (0..count).rev() {
            let Some((name, kind)) = menu_item_facts(items, index) else {
                continue;
            };
            if kind != MENU_SEPARATOR && !MENU_KEEP.contains(&name.as_str()) {
                let _ = ((*items_vtbl).remove_value_at_index)(items, index);
            }
        }
        // the separators: a break can only stand between two rows
        let mut count = 0u32;
        let _ = ((*items_vtbl).get_count)(items, &mut count);
        let mut index = 0u32;
        let mut at_break = true; // the top edge counts as one
        while index < count {
            let Some((_, kind)) = menu_item_facts(items, index) else {
                index += 1;
                continue;
            };
            if kind == MENU_SEPARATOR && at_break {
                let _ = ((*items_vtbl).remove_value_at_index)(items, index);
                count -= 1;
            } else {
                at_break = kind == MENU_SEPARATOR;
                index += 1;
            }
        }
        while count > 0 {
            match menu_item_facts(items, count - 1) {
                Some((_, kind)) if kind == MENU_SEPARATOR => {
                    let _ = ((*items_vtbl).remove_value_at_index)(items, count - 1);
                    count -= 1;
                }
                _ => break,
            }
        }
        com_release(items);
    }
}

/// The chords outrank the island (the mac's a8086b7, on this door):
/// command chords only — Ctrl is this platform's `command` — and the
/// same ONE gate body the pump runs; consumed means the page never
/// sees the stroke. Everything else is the page's to type, and a
/// chord the app declines falls through to the engine's own
/// accelerators (Ctrl+F's find bar, F12's devtools).
fn accelerator_pressed(args: *mut c_void) {
    const KEY_DOWN: i32 = 0;
    const SYSTEM_KEY_DOWN: i32 = 2;
    /// `VK_PROCESSKEY` — the IME owns this stroke.
    const VK_PROCESSKEY: u32 = 0xE5;
    let (kind, vk, lparam) = unsafe {
        let vtbl = *(args as *mut *const AcceleratorArgsVtbl);
        let mut kind = 0i32;
        let mut vk = 0u32;
        let mut lparam = 0i32;
        let _ = ((*vtbl).get_key_event_kind)(args, &mut kind);
        let _ = ((*vtbl).get_virtual_key)(args, &mut vk);
        let _ = ((*vtbl).get_key_event_lparam)(args, &mut lparam);
        (kind, vk, lparam)
    };
    if kind != KEY_DOWN && kind != SYSTEM_KEY_DOWN {
        return;
    }
    if vk == VK_PROCESSKEY || crate::ffi::ime_composing() || !crate::ffi::control_held() {
        return;
    }
    let stroke = crate::ffi::key_stroke_of(vk as usize, lparam as isize);
    if crate::ffi::gate_consumes(&stroke) {
        unsafe {
            let vtbl = *(args as *mut *const AcceleratorArgsVtbl);
            let _ = ((*vtbl).put_handled)(args, 1);
        }
    }
}

/// One table read, one verdict — acted on OUTSIDE the borrow, so a
/// dispatch that re-enters a handle finds the table at rest.
enum Asked {
    /// Speak to the engine now.
    Live(*mut WebView2),
    /// Parked until the land drains it.
    Queued,
    /// The engine will never come — the remembered sentence.
    Refused(Rc<str>),
    /// No such mount.
    Unknown,
}

fn ask(path: &str, command: Queued) -> Asked {
    VIEWS.with(|views| {
        let mut views = views.borrow_mut();
        let Some(slot) = views.get_mut(path) else {
            return Asked::Unknown;
        };
        match &slot.state {
            Mount::Live(live) => Asked::Live(live.core),
            Mount::Refused(why) => Asked::Refused(Rc::clone(why)),
            Mount::Waiting | Mount::Creating => {
                slot.queued.push(command);
                Asked::Queued
            }
        }
    })
}

// MARK: - A JSON reader (the console door's only need)

/// A minimal recursive-descent JSON reader — the CDP payloads are the
/// one place the shell parses JSON, and a dependency is not an answer.
/// Numbers keep their SOURCE text (no float rounding lies).
pub(crate) mod json {
    #[derive(Debug, PartialEq)]
    pub enum Json {
        Null,
        Bool(bool),
        Number(String),
        Str(String),
        Array(Vec<Json>),
        Object(Vec<(String, Json)>),
    }

    impl Json {
        pub fn get(&self, key: &str) -> Option<&Json> {
            match self {
                Json::Object(fields) => {
                    fields.iter().find(|(name, _)| name == key).map(|(_, value)| value)
                }
                _ => None,
            }
        }

        pub fn as_str(&self) -> Option<&str> {
            match self {
                Json::Str(text) => Some(text),
                _ => None,
            }
        }

        /// The value back as JSON text — what a non-string console
        /// argument renders as.
        pub fn source(&self) -> String {
            match self {
                Json::Null => String::from("null"),
                Json::Bool(true) => String::from("true"),
                Json::Bool(false) => String::from("false"),
                Json::Number(text) => text.clone(),
                Json::Str(text) => {
                    let mut out = String::with_capacity(text.len() + 2);
                    out.push('"');
                    for character in text.chars() {
                        match character {
                            '"' => out.push_str("\\\""),
                            '\\' => out.push_str("\\\\"),
                            '\n' => out.push_str("\\n"),
                            '\r' => out.push_str("\\r"),
                            '\t' => out.push_str("\\t"),
                            c if (c as u32) < 0x20 => {
                                out.push_str(&format!("\\u{:04x}", c as u32));
                            }
                            c => out.push(c),
                        }
                    }
                    out.push('"');
                    out
                }
                Json::Array(items) => {
                    let inner: Vec<String> = items.iter().map(Json::source).collect();
                    format!("[{}]", inner.join(","))
                }
                Json::Object(fields) => {
                    let inner: Vec<String> = fields
                        .iter()
                        .map(|(key, value)| {
                            format!("{}:{}", Json::Str(key.clone()).source(), value.source())
                        })
                        .collect();
                    format!("{{{}}}", inner.join(","))
                }
            }
        }
    }

    pub fn parse(text: &str) -> Option<Json> {
        let bytes = text.as_bytes();
        let mut at = 0;
        let value = value(bytes, &mut at)?;
        skip_space(bytes, &mut at);
        if at == bytes.len() { Some(value) } else { None }
    }

    fn skip_space(bytes: &[u8], at: &mut usize) {
        while *at < bytes.len() && matches!(bytes[*at], b' ' | b'\t' | b'\n' | b'\r') {
            *at += 1;
        }
    }

    fn value(bytes: &[u8], at: &mut usize) -> Option<Json> {
        skip_space(bytes, at);
        match bytes.get(*at)? {
            b'{' => object(bytes, at),
            b'[' => array(bytes, at),
            b'"' => string(bytes, at).map(Json::Str),
            b't' => literal(bytes, at, b"true", Json::Bool(true)),
            b'f' => literal(bytes, at, b"false", Json::Bool(false)),
            b'n' => literal(bytes, at, b"null", Json::Null),
            _ => number(bytes, at),
        }
    }

    fn literal(bytes: &[u8], at: &mut usize, word: &[u8], value: Json) -> Option<Json> {
        if bytes[*at..].starts_with(word) {
            *at += word.len();
            Some(value)
        } else {
            None
        }
    }

    fn number(bytes: &[u8], at: &mut usize) -> Option<Json> {
        let start = *at;
        if bytes.get(*at) == Some(&b'-') {
            *at += 1;
        }
        while *at < bytes.len()
            && matches!(bytes[*at], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
        {
            *at += 1;
        }
        if *at == start {
            return None;
        }
        std::str::from_utf8(&bytes[start..*at])
            .ok()
            .map(|text| Json::Number(text.to_string()))
    }

    fn string(bytes: &[u8], at: &mut usize) -> Option<String> {
        *at += 1; // the opening quote
        let mut out = String::new();
        loop {
            match bytes.get(*at)? {
                b'"' => {
                    *at += 1;
                    return Some(out);
                }
                b'\\' => {
                    *at += 1;
                    match bytes.get(*at)? {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let high = hex4(bytes, *at + 1)?;
                            *at += 4;
                            let code = if (0xD800..0xDC00).contains(&high)
                                && bytes.get(*at + 1) == Some(&b'\\')
                                && bytes.get(*at + 2) == Some(&b'u')
                            {
                                let low = hex4(bytes, *at + 3)?;
                                *at += 6;
                                0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00)
                            } else {
                                high
                            };
                            out.push(char::from_u32(code)?);
                        }
                        _ => return None,
                    }
                    *at += 1;
                }
                _ => {
                    // one UTF-8 scalar, whole
                    let text = std::str::from_utf8(&bytes[*at..]).ok()?;
                    let character = text.chars().next()?;
                    out.push(character);
                    *at += character.len_utf8();
                }
            }
        }
    }

    fn hex4(bytes: &[u8], from: usize) -> Option<u32> {
        let slice = bytes.get(from..from + 4)?;
        u32::from_str_radix(std::str::from_utf8(slice).ok()?, 16).ok()
    }

    fn array(bytes: &[u8], at: &mut usize) -> Option<Json> {
        *at += 1; // '['
        let mut items = Vec::new();
        skip_space(bytes, at);
        if bytes.get(*at) == Some(&b']') {
            *at += 1;
            return Some(Json::Array(items));
        }
        loop {
            items.push(value(bytes, at)?);
            skip_space(bytes, at);
            match bytes.get(*at)? {
                b',' => *at += 1,
                b']' => {
                    *at += 1;
                    return Some(Json::Array(items));
                }
                _ => return None,
            }
        }
    }

    fn object(bytes: &[u8], at: &mut usize) -> Option<Json> {
        *at += 1; // '{'
        let mut fields = Vec::new();
        skip_space(bytes, at);
        if bytes.get(*at) == Some(&b'}') {
            *at += 1;
            return Some(Json::Object(fields));
        }
        loop {
            skip_space(bytes, at);
            if bytes.get(*at)? != &b'"' {
                return None;
            }
            let key = string(bytes, at)?;
            skip_space(bytes, at);
            if bytes.get(*at)? != &b':' {
                return None;
            }
            *at += 1;
            fields.push((key, value(bytes, at)?));
            skip_space(bytes, at);
            match bytes.get(*at)? {
                b',' => *at += 1,
                b'}' => {
                    *at += 1;
                    return Some(Json::Object(fields));
                }
                _ => return None,
            }
        }
    }
}

// MARK: - Tests

#[cfg(test)]
mod tests {
    use super::*;

    /// The house guard against a mis-numbered hand-written vtable —
    /// every struct must hold EXACTLY the slots its header declares.
    #[test]
    fn the_vtables_hold_exactly_the_slots_their_headers_declare() {
        let slot = std::mem::size_of::<usize>();
        assert_eq!(std::mem::size_of::<EnvironmentVtbl>(), 8 * slot);
        assert_eq!(std::mem::size_of::<ControllerVtbl>(), 26 * slot);
        assert_eq!(std::mem::size_of::<WebView2Vtbl>(), 61 * slot);
        assert_eq!(std::mem::size_of::<WebView2_2Vtbl>(), 68 * slot);
        assert_eq!(std::mem::size_of::<SettingsVtbl>(), 21 * slot);
        assert_eq!(std::mem::size_of::<MessageArgsVtbl>(), 6 * slot);
        assert_eq!(std::mem::size_of::<NavStartingArgsVtbl>(), 10 * slot);
        assert_eq!(std::mem::size_of::<NewWindowArgsVtbl>(), 11 * slot);
        assert_eq!(std::mem::size_of::<ContentLoadingArgsVtbl>(), 5 * slot);
        assert_eq!(std::mem::size_of::<NavCompletedArgsVtbl>(), 6 * slot);
        assert_eq!(std::mem::size_of::<DevToolsReceiverVtbl>(), 5 * slot);
        assert_eq!(std::mem::size_of::<DevToolsArgsVtbl>(), 4 * slot);
        assert_eq!(std::mem::size_of::<ResponseReceivedArgsVtbl>(), 5 * slot);
        assert_eq!(std::mem::size_of::<ResourceRequestVtbl>(), 10 * slot);
        assert_eq!(std::mem::size_of::<ResponseViewVtbl>(), 7 * slot);
        assert_eq!(std::mem::size_of::<AcceleratorArgsVtbl>(), 9 * slot);
        assert_eq!(std::mem::size_of::<WebView2_11Vtbl>(), 102 * slot);
        assert_eq!(std::mem::size_of::<ContextMenuArgsVtbl>(), 11 * slot);
        assert_eq!(std::mem::size_of::<MenuItemsVtbl>(), 7 * slot);
        assert_eq!(std::mem::size_of::<MenuItemVtbl>(), 16 * slot);
        assert_eq!(std::mem::size_of::<StreamVtbl>(), 14 * slot);
        assert_eq!(std::mem::size_of::<Handler2Vtbl>(), 4 * slot);
        assert_eq!(std::mem::size_of::<Handler1Vtbl>(), 4 * slot);
    }

    #[test]
    fn the_ladder_joins_a_runtime_folder_to_its_client_dll() {
        assert_eq!(
            client_dll_under("C:\\rt\\151.0.4129.107", "x64"),
            "C:\\rt\\151.0.4129.107\\EBWebView\\x64\\EmbeddedBrowserWebView.dll"
        );
        // separator hygiene: a trailing slash never doubles
        assert_eq!(
            client_dll_under("C:\\rt\\151.0.4129.107\\", "arm64"),
            "C:\\rt\\151.0.4129.107\\EBWebView\\arm64\\EmbeddedBrowserWebView.dll"
        );
        assert!(matches!(arch_dir(), "x64" | "arm64" | "x86"));
    }

    #[test]
    fn the_envelope_splits_leading_fields_and_keeps_payload_tabs() {
        match parse_message("bunny\ta line\twith\ttabs") {
            Some(Parsed::Posted(body)) => assert_eq!(body, "a line\twith\ttabs"),
            _ => panic!("the bus leg"),
        }
        match parse_message("eval\t7\tok\t\"x\"") {
            Some(Parsed::EvalDone { token, result }) => {
                assert_eq!(token, 7);
                assert_eq!(result, Ok(String::from("\"x\"")));
            }
            _ => panic!("the ok leg"),
        }
        match parse_message("eval\t9\terr\tTypeError: no\treally") {
            Some(Parsed::EvalDone { token, result }) => {
                assert_eq!(token, 9);
                assert_eq!(result, Err(String::from("TypeError: no\treally")));
            }
            _ => panic!("the err leg"),
        }
        assert!(parse_message("noise").is_none());
        assert!(parse_message("eval\tnot-a-number\tok\tx").is_none());
        assert!(parse_message("console\treserved but unused").is_none());
    }

    #[test]
    fn every_refusal_is_a_sentence_never_a_number() {
        for status in 0..=18 {
            let name = error_name(status);
            assert!(!name.is_empty());
            assert!(
                !name.contains(char::is_numeric),
                "status {status} leaked a number: {name}"
            );
        }
        assert_eq!(error_name(999), "the engine refused unnamed");
        assert_eq!(error_name(13), "the host name did not resolve");
    }

    #[test]
    fn the_key_table_speaks_the_pages_vocabulary() {
        assert_eq!(key_spec("Enter"), Some((0x0D, "Enter", "Enter", Some("\r"))));
        assert_eq!(key_spec("Return"), key_spec("Enter"));
        assert_eq!(key_spec("Esc"), key_spec("Escape"));
        // the space key NAMES itself as the character it types
        assert_eq!(key_spec("Space"), Some((0x20, " ", "Space", Some(" "))));
        assert_eq!(key_spec("ArrowDown"), Some((0x28, "ArrowDown", "ArrowDown", None)));
        // a name nobody knows presses nothing (single chars take the
        // typed road instead)
        assert_eq!(key_spec("Hyperspace"), None);
        assert_eq!(key_spec("a"), None);
    }

    #[test]
    fn the_modifier_bits_follow_the_shells_own_law() {
        // option→Alt(1), command→Ctrl(2), control→Meta(4), shift→Shift(8)
        let none = Modifiers::NONE;
        assert_eq!(cdp_modifiers(none), 0);
        let mut m = Modifiers::NONE;
        m.shift = true;
        assert_eq!(cdp_modifiers(m), 8);
        m.command = true;
        assert_eq!(cdp_modifiers(m), 10);
        m.option = true;
        assert_eq!(cdp_modifiers(m), 11);
        m.control = true;
        assert_eq!(cdp_modifiers(m), 15);
    }

    #[test]
    fn a_user_script_wraps_as_a_block_not_an_iife() {
        // a block keeps top-level `var` hoisting to the global scope,
        // the way WKUserScript runs it
        assert_eq!(
            main_frame_only("var x = 1;"),
            "if (self === top) { var x = 1; }"
        );
    }

    #[test]
    fn the_motion_emulation_spells_what_the_protocol_expects() {
        // on: the visitor's answer; off: the EMPTY value, the
        // protocol's own "no override" — never a guessed preference
        assert_eq!(
            emulated_media_params(true),
            r#"{"features":[{"name":"prefers-reduced-motion","value":"no-preference"}]}"#
        );
        assert_eq!(
            emulated_media_params(false),
            r#"{"features":[{"name":"prefers-reduced-motion","value":""}]}"#
        );
    }

    #[test]
    fn the_handler_lives_by_its_count_and_answers_its_one_iid() {
        let landed = std::rc::Rc::new(Cell::new((0usize, 0usize, 0u32)));
        let seen = Rc::clone(&landed);
        let handler = handler2(IID_ENV_COMPLETED, move |a, b| {
            seen.set((a, b, seen.get().2 + 1));
            0
        }) as *mut Handler;
        unsafe {
            // QI: its own IID and IUnknown answer; a stranger refuses
            let vtbl = &*((*handler).vtbl as *const Handler2Vtbl);
            let mut out: *mut c_void = std::ptr::null_mut();
            assert_eq!(
                (vtbl.unknown.query_interface)(handler as *mut c_void, &IID_ENV_COMPLETED, &mut out),
                0
            );
            assert_eq!(out, handler as *mut c_void);
            assert_eq!(
                (vtbl.unknown.query_interface)(handler as *mut c_void, &IID_IUNKNOWN, &mut out),
                0
            );
            assert_eq!(
                (vtbl.unknown.query_interface)(handler as *mut c_void, &IID_NAV_STARTING, &mut out),
                NO_INTERFACE
            );
            assert!(out.is_null());
            // the two QI hits took two references: 1 + 2
            assert_eq!((*handler).refs.get(), 3);
            // invoke reaches the closure with both raw args
            (vtbl.invoke)(handler, 41, 42);
            assert_eq!(landed.get(), (41, 42, 1));
            // release to zero frees — miri would scream on a double
            assert_eq!((vtbl.unknown.release)(handler as *mut c_void), 2);
            assert_eq!((vtbl.unknown.release)(handler as *mut c_void), 1);
            assert_eq!((vtbl.unknown.release)(handler as *mut c_void), 0);
        }
    }

    #[test]
    fn the_json_reader_reads_what_the_protocol_sends() {
        use json::Json;
        let parsed = json::parse(
            r#"{"type":"warning","args":[{"type":"string","value":"two\twords"},{"type":"number","value":3.5,"description":"3.5"},{"type":"object","description":"Object"},{"unserializableValue":"NaN"}]}"#,
        )
        .expect("the payload parses");
        assert_eq!(parsed.get("type").and_then(Json::as_str), Some("warning"));
        let line = console_line(&parsed).expect("a line");
        assert_eq!(line, "warn: two\twords 3.5 Object NaN");
        // escapes, nesting, surrogate pairs
        let deep = json::parse(r#"{"a":[1,{"b":"\ud83d\ude00"},null,true]}"#).expect("nesting");
        let emoji = deep
            .get("a")
            .and_then(|a| match a {
                Json::Array(items) => items.get(1),
                _ => None,
            })
            .and_then(|item| item.get("b"))
            .and_then(Json::as_str)
            .expect("the pair decodes");
        assert_eq!(emoji, "😀");
        // numbers keep their source text
        assert_eq!(json::parse("1e-7").map(|n| n.source()), Some(String::from("1e-7")));
        assert!(json::parse("{broken").is_none());
        // an exception renders as the mac's error leg
        let thrown = json::parse(
            r#"{"exceptionDetails":{"text":"Uncaught","exception":{"description":"ReferenceError: nope is not defined\n    at <anonymous>:1:1"}}}"#,
        )
        .expect("the exception parses");
        assert_eq!(
            exception_line(&thrown),
            Some(String::from("error: ReferenceError: nope is not defined"))
        );
    }

    /// The `device_present()` idiom: probes the real runtime and skips
    /// honestly when there is none — everywhere else the environment
    /// stands for real, through the loader-free ladder.
    #[test]
    fn the_environment_stands_when_the_runtime_is_installed() {
        use crate::ffi::{DispatchMessageW, Msg, TranslateMessage};
        #[link(name = "user32", kind = "raw-dylib")]
        unsafe extern "system" {
            fn PeekMessageW(
                message: *mut Msg,
                hwnd: isize,
                min: u32,
                max: u32,
                remove: u32,
            ) -> i32;
        }
        if client_dll_candidates().is_empty() {
            eprintln!("webview smoke: no WebView2 runtime on this machine; skipping");
            return;
        }
        com_init();
        let landed: Rc<Cell<Option<Hresult>>> = Rc::new(Cell::new(None));
        let seen = Rc::clone(&landed);
        let completed = handler2(IID_ENV_COMPLETED, move |hr, environment| {
            assert!(environment != 0, "a landed environment is not null");
            seen.set(Some(hr as Hresult));
            0
        });
        create_environment(completed).expect("the ladder finds the runtime");
        unsafe {
            com_release(completed);
        }
        let start = std::time::Instant::now();
        while landed.get().is_none() {
            assert!(
                start.elapsed() < std::time::Duration::from_secs(10),
                "the environment never landed"
            );
            unsafe {
                let mut message: Msg = std::mem::zeroed();
                const PM_REMOVE: u32 = 1;
                if PeekMessageW(&mut message, 0, 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        }
        let hr = landed.get().expect("landed");
        assert!(com_ok(hr), "the environment refused: 0x{:08X}", hr as u32);
    }

    #[test]
    fn the_registry_ladder_is_pure_until_it_touches_the_hive() {
        // the candidate list is deterministic under the env override
        // SAFETY: tests in this binary run single-threaded per test,
        // and the var is removed before the test ends
        unsafe {
            std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", "C:\\fixed\\rt");
        }
        let candidates = client_dll_candidates();
        unsafe {
            std::env::remove_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER");
        }
        assert_eq!(
            candidates,
            vec![
                format!("C:\\fixed\\rt\\EBWebView\\{}\\EmbeddedBrowserWebView.dll", arch_dir()),
                String::from("C:\\fixed\\rt\\EmbeddedBrowserWebView.dll"),
            ]
        );
    }
}
