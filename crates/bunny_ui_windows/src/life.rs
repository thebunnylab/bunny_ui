//! The app's life outside its window, on this platform: the power
//! broadcast (sleep, resume — `ffi` reports it) and the desktop's
//! toasts, by hand-written COM like the webview's — no crate, no
//! bundled bytes, every interface id and slot read off the SDK's own
//! bindings.
//!
//! A toast wants an AppUserModelID. The shell names this process by
//! its executable at boot (`bunnylab.<stem>`) and shows toasts under
//! that name; a Start Menu shortcut carrying the same id gives the
//! toast the app's own name and icon, and is the app's to install. A
//! button reaches the app while it RUNS — the `Activated` event on the
//! toast object, answered on a thread-pool thread and handed to the
//! app's channel from there; a click on a toast of an app that already
//! exited needs a COM server the registry knows, which is the app's
//! own setup too.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use bunny_ui::app::{AppEvent, Notification, emit};

use crate::ffi::{Guid, Hresult, UnknownVtbl, com_init, com_ok, com_query, wide};

#[link(name = "combase")]
unsafe extern "system" {
    fn RoInitialize(kind: u32) -> Hresult;
    fn RoGetActivationFactory(
        class: *mut c_void,
        iid: *const Guid,
        out: *mut *mut c_void,
    ) -> Hresult;
    fn RoActivateInstance(class: *mut c_void, out: *mut *mut c_void) -> Hresult;
    fn WindowsCreateString(text: *const u16, length: u32, out: *mut *mut c_void) -> Hresult;
    fn WindowsDeleteString(text: *mut c_void) -> Hresult;
    fn WindowsGetStringRawBuffer(text: *mut c_void, length: *mut u32) -> *const u16;
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn SetCurrentProcessExplicitAppUserModelID(id: *const u16) -> Hresult;
}

// MARK: - The identities (windows 0.62 bindings, Windows.UI.Notifications)

/// `IToastNotificationManagerStatics` {50AC103F-D235-4598-BBEF-98FE4D1A3AD4}.
const IID_MANAGER_STATICS: Guid = Guid {
    d1: 0x50ac103f,
    d2: 0xd235,
    d3: 0x4598,
    d4: [0xbb, 0xef, 0x98, 0xfe, 0x4d, 0x1a, 0x3a, 0xd4],
};
/// `IToastNotificationFactory` {04124B20-82C6-4229-B109-FD9ED4662B53}.
const IID_TOAST_FACTORY: Guid = Guid {
    d1: 0x04124b20,
    d2: 0x82c6,
    d3: 0x4229,
    d4: [0xb1, 0x09, 0xfd, 0x9e, 0xd4, 0x66, 0x2b, 0x53],
};
/// `IToastNotification2` {9DFB9FD1-143A-490E-90BF-B9FBA7132DE7} — the
/// tag, which is what makes a second toast REPLACE the first.
const IID_TOAST2: Guid = Guid {
    d1: 0x9dfb9fd1,
    d2: 0x143a,
    d3: 0x490e,
    d4: [0x90, 0xbf, 0xb9, 0xfb, 0xa7, 0x13, 0x2d, 0xe7],
};
/// `IToastActivatedEventArgs` {E3BF92F3-C197-436F-8265-0625824F8DAC}.
const IID_ACTIVATED_ARGS: Guid = Guid {
    d1: 0xe3bf92f3,
    d2: 0xc197,
    d3: 0x436f,
    d4: [0x82, 0x65, 0x06, 0x25, 0x82, 0x4f, 0x8d, 0xac],
};
/// `IXmlDocumentIO` {6CD0E74E-EE65-4489-9EBF-CA43E87BA637}.
const IID_XML_IO: Guid = Guid {
    d1: 0x6cd0e74e,
    d2: 0xee65,
    d3: 0x4489,
    d4: [0x9e, 0xbf, 0xca, 0x43, 0xe8, 0x7b, 0xa6, 0x37],
};
/// `IXmlDocument` {F7F3A506-1E87-42D6-BCFB-B8C809FA5494}.
const IID_XML_DOCUMENT: Guid = Guid {
    d1: 0xf7f3a506,
    d2: 0x1e87,
    d3: 0x42d6,
    d4: [0xbc, 0xfb, 0xb8, 0xc8, 0x09, 0xfa, 0x54, 0x94],
};
/// `TypedEventHandler<ToastNotification, IInspectable>`
/// {AB54DE2D-97D9-5528-B6AD-105AFE156530} — a parameterized interface,
/// its id the runtime's own hash of the signature.
const IID_ACTIVATED_HANDLER: Guid = Guid {
    d1: 0xab54de2d,
    d2: 0x97d9,
    d3: 0x5528,
    d4: [0xb6, 0xad, 0x10, 0x5a, 0xfe, 0x15, 0x65, 0x30],
};
/// `IID_IUnknown` {00000000-0000-0000-C000-000000000046}.
const IID_IUNKNOWN: Guid =
    Guid { d1: 0, d2: 0, d3: 0, d4: [0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46] };

// MARK: - Consumed vtables (header order, indexes cited, runs padded)

/// `IInspectable` — IUnknown and three more.
#[repr(C)]
struct InspectableVtbl {
    unknown: UnknownVtbl, // 0-2
    /// 3 GetIids; 4 GetRuntimeClassName; 5 GetTrustLevel.
    _pad_3_5: [usize; 3],
}

/// `IToastNotificationManagerStatics` — 9 slots.
#[repr(C)]
struct ManagerStaticsVtbl {
    inspectable: InspectableVtbl, // 0-5
    /// 6 CreateToastNotifier.
    _pad_6: [usize; 1],
    /// 7 `CreateToastNotifierWithId(HSTRING, **notifier)`.
    create_toast_notifier_with_id:
        unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> Hresult,
    /// 8 GetTemplateContent.
    _pad_8: [usize; 1],
}

/// `IToastNotificationFactory` — 7 slots.
#[repr(C)]
struct ToastFactoryVtbl {
    inspectable: InspectableVtbl, // 0-5
    /// 6 `CreateToastNotification(IXmlDocument*, **toast)`.
    create_toast_notification:
        unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> Hresult,
}

/// `IToastNotification` — 15 slots.
#[repr(C)]
struct ToastVtbl {
    inspectable: InspectableVtbl, // 0-5
    /// 6 get_Content; 7-8 put/get_ExpirationTime;
    /// 9-10 add/remove_Dismissed.
    _pad_6_10: [usize; 5],
    /// 11 `add_Activated(handler, *token)`.
    add_activated: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut i64) -> Hresult,
    /// 12 remove_Activated; 13-14 add/remove_Failed.
    _pad_12_14: [usize; 3],
}

/// `IToastNotification2` — 12 slots.
#[repr(C)]
struct Toast2Vtbl {
    inspectable: InspectableVtbl, // 0-5
    /// 6 `put_Tag(HSTRING)`.
    put_tag: unsafe extern "system" fn(*mut c_void, *mut c_void) -> Hresult,
    /// 7 get_Tag; 8-9 put/get_Group; 10-11 put/get_SuppressPopup.
    _pad_7_11: [usize; 5],
}

/// `IToastNotifier` — 12 slots.
#[repr(C)]
struct NotifierVtbl {
    inspectable: InspectableVtbl, // 0-5
    /// 6 `Show(toast)`.
    show: unsafe extern "system" fn(*mut c_void, *mut c_void) -> Hresult,
    /// 7 Hide; 8 get_Setting; 9 AddToSchedule; 10 RemoveFromSchedule;
    /// 11 GetScheduledToastNotifications.
    _pad_7_11: [usize; 5],
}

/// `IToastActivatedEventArgs` — 7 slots.
#[repr(C)]
struct ActivatedArgsVtbl {
    inspectable: InspectableVtbl, // 0-5
    /// 6 `get_Arguments(*HSTRING)`.
    get_arguments: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
}

/// `IXmlDocumentIO` — 9 slots.
#[repr(C)]
struct XmlIoVtbl {
    inspectable: InspectableVtbl, // 0-5
    /// 6 `LoadXml(HSTRING)`.
    load_xml: unsafe extern "system" fn(*mut c_void, *mut c_void) -> Hresult,
    /// 7 LoadXmlWithSettings; 8 SaveToFileAsync.
    _pad_7_8: [usize; 2],
}

// MARK: - The Rust-authored handler (thread-safe: a toast answers on the pool)

/// The `Activated` handler — IUnknown plus `Invoke(sender, args)`.
/// Unlike the webview's handlers this one is called on a thread-pool
/// thread, so its count is atomic and its closure sits behind a lock.
#[repr(C)]
struct ToastHandler {
    vtbl: *const ToastHandlerVtbl,
    refs: AtomicU32,
    land: Mutex<Box<dyn FnMut(*mut c_void) + Send>>,
}

#[repr(C)]
struct ToastHandlerVtbl {
    unknown: UnknownVtbl, // 0-2
    invoke: unsafe extern "system" fn(*mut ToastHandler, *mut c_void, *mut c_void) -> Hresult, // 3
}

/// `E_NOINTERFACE`.
const NO_INTERFACE: Hresult = 0x8000_4002u32 as i32;

unsafe extern "system" fn toast_query(
    this: *mut c_void,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> Hresult {
    unsafe {
        let handler = this as *mut ToastHandler;
        if *riid == IID_IUNKNOWN || *riid == IID_ACTIVATED_HANDLER {
            (*handler).refs.fetch_add(1, Ordering::SeqCst);
            *out = this;
            0
        } else {
            *out = std::ptr::null_mut();
            NO_INTERFACE
        }
    }
}

unsafe extern "system" fn toast_add_ref(this: *mut c_void) -> u32 {
    unsafe { (*(this as *mut ToastHandler)).refs.fetch_add(1, Ordering::SeqCst) + 1 }
}

unsafe extern "system" fn toast_release(this: *mut c_void) -> u32 {
    unsafe {
        let handler = this as *mut ToastHandler;
        let refs = (*handler).refs.fetch_sub(1, Ordering::SeqCst) - 1;
        if refs == 0 {
            drop(Box::from_raw(handler));
        }
        refs
    }
}

unsafe extern "system" fn toast_invoke(
    this: *mut ToastHandler,
    _sender: *mut c_void,
    args: *mut c_void,
) -> Hresult {
    unsafe {
        if let Ok(mut land) = (*this).land.lock() {
            land(args);
        }
    }
    0
}

static TOAST_HANDLER_VTBL: ToastHandlerVtbl = ToastHandlerVtbl {
    unknown: UnknownVtbl {
        query_interface: toast_query,
        add_ref: toast_add_ref,
        release: toast_release,
    },
    invoke: toast_invoke,
};

fn toast_handler(land: impl FnMut(*mut c_void) + Send + 'static) -> *mut c_void {
    Box::into_raw(Box::new(ToastHandler {
        vtbl: &raw const TOAST_HANDLER_VTBL,
        refs: AtomicU32::new(1),
        land: Mutex::new(Box::new(land)),
    })) as *mut c_void
}

// MARK: - Strings and refusals

/// An HSTRING with its own release.
struct Hstring(*mut c_void);

impl Hstring {
    fn new(text: &str) -> Hstring {
        let wide = wide(text);
        let mut out = std::ptr::null_mut();
        unsafe {
            // `wide` ends in a NUL the runtime does not count
            let _ = WindowsCreateString(wide.as_ptr(), (wide.len() - 1) as u32, &mut out);
        }
        Hstring(out)
    }
}

impl Drop for Hstring {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WindowsDeleteString(self.0);
            }
        }
    }
}

/// An HSTRING's text, borrowed out and copied.
unsafe fn hstring_text(text: *mut c_void) -> String {
    if text.is_null() {
        return String::new();
    }
    unsafe {
        let mut length = 0u32;
        let buffer = WindowsGetStringRawBuffer(text, &mut length);
        if buffer.is_null() {
            return String::new();
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(buffer, length as usize))
    }
}

/// A COM pointer released on the way out.
struct Owned(*mut c_void);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let vtbl = *(self.0 as *mut *const UnknownVtbl);
                ((*vtbl).release)(self.0);
            }
        }
    }
}

fn refused(what: &str, hr: Hresult) -> String {
    format!("{what} (0x{:08X})", hr as u32)
}

/// This process's AppUserModelID — its executable's name, under the
/// house's own prefix.
fn app_user_model_id() -> String {
    let stem = std::env::current_exe()
        .ok()
        .and_then(|path| path.file_stem().map(|stem| stem.to_string_lossy().into_owned()))
        .unwrap_or_else(|| String::from("app"));
    format!("bunnylab.{stem}")
}

/// Names the process for the desktop, and installs the notifier — at
/// boot. The power broadcast needs no installing: the window
/// procedure reports it.
pub(crate) fn install() {
    let id = wide(&app_user_model_id());
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(id.as_ptr());
    }
    bunny_ui::app::install_notifier(notify);
}

/// `text` inside an XML attribute or element.
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The toast, as the XML the runtime reads: the generic binding with
/// two lines, one button per action.
fn toast_xml(notification: &Notification) -> String {
    let mut out = String::from(
        "<toast launch=\"bunny:open\"><visual><binding template=\"ToastGeneric\">",
    );
    out.push_str(&format!(
        "<text>{}</text><text>{}</text>",
        xml(&notification.title),
        xml(&notification.body)
    ));
    out.push_str("</binding></visual>");
    if !notification.actions.is_empty() {
        out.push_str("<actions>");
        for action in &notification.actions {
            out.push_str(&format!(
                "<action content=\"{}\" arguments=\"bunny:action:{}\"/>",
                xml(&action.label),
                xml(&action.key)
            ));
        }
        out.push_str("</actions>");
    }
    out.push_str("</toast>");
    out
}

/// What an activation's arguments mean: the toast itself, or a button.
fn activated_action(arguments: &str) -> Option<String> {
    arguments.strip_prefix("bunny:action:").map(str::to_string)
}

/// Shows a toast, or says why it cannot — the runtime's refusal, by
/// call and code.
fn notify(notification: &Notification) -> Result<(), String> {
    com_init();
    unsafe {
        // the runtime joins the apartment COM already opened; S_FALSE
        // (joined already) and a changed mode are both "it is on"
        let hr = RoInitialize(0);
        if !com_ok(hr) && hr as u32 != 0x8001_0106 {
            return Err(refused("the Windows Runtime did not initialize", hr));
        }

        let class = Hstring::new("Windows.UI.Notifications.ToastNotificationManager");
        let mut statics = std::ptr::null_mut();
        let hr = RoGetActivationFactory(class.0, &IID_MANAGER_STATICS, &mut statics);
        if !com_ok(hr) || statics.is_null() {
            return Err(refused("the toast manager is not available", hr));
        }
        let statics = Owned(statics);
        let aumid = Hstring::new(&app_user_model_id());
        let mut notifier = std::ptr::null_mut();
        let vtbl = *(statics.0 as *mut *const ManagerStaticsVtbl);
        let hr = ((*vtbl).create_toast_notifier_with_id)(statics.0, aumid.0, &mut notifier);
        if !com_ok(hr) || notifier.is_null() {
            return Err(refused("the toast notifier refused this app's id", hr));
        }
        let notifier = Owned(notifier);

        let class = Hstring::new("Windows.Data.Xml.Dom.XmlDocument");
        let mut document = std::ptr::null_mut();
        let hr = RoActivateInstance(class.0, &mut document);
        if !com_ok(hr) || document.is_null() {
            return Err(refused("the XML document did not activate", hr));
        }
        let document = Owned(document);
        let Some(io) = com_query(document.0, &IID_XML_IO) else {
            return Err(String::from("the XML document has no loading door"));
        };
        let io = Owned(io);
        let text = Hstring::new(&toast_xml(notification));
        let vtbl = *(io.0 as *mut *const XmlIoVtbl);
        let hr = ((*vtbl).load_xml)(io.0, text.0);
        if !com_ok(hr) {
            return Err(refused("the toast's XML was refused", hr));
        }
        let Some(xml_document) = com_query(document.0, &IID_XML_DOCUMENT) else {
            return Err(String::from("the XML document is not one"));
        };
        let xml_document = Owned(xml_document);

        let class = Hstring::new("Windows.UI.Notifications.ToastNotification");
        let mut factory = std::ptr::null_mut();
        let hr = RoGetActivationFactory(class.0, &IID_TOAST_FACTORY, &mut factory);
        if !com_ok(hr) || factory.is_null() {
            return Err(refused("the toast factory is not available", hr));
        }
        let factory = Owned(factory);
        let mut toast = std::ptr::null_mut();
        let vtbl = *(factory.0 as *mut *const ToastFactoryVtbl);
        let hr = ((*vtbl).create_toast_notification)(factory.0, xml_document.0, &mut toast);
        if !com_ok(hr) || toast.is_null() {
            return Err(refused("the toast did not build", hr));
        }
        let toast = Owned(toast);

        // the tag: one toast per id, the newest replacing the last
        if let Some(toast2) = com_query(toast.0, &IID_TOAST2) {
            let toast2 = Owned(toast2);
            let tag: String = notification.id.chars().take(64).collect();
            let tag = Hstring::new(&tag);
            let vtbl = *(toast2.0 as *mut *const Toast2Vtbl);
            let _ = ((*vtbl).put_tag)(toast2.0, tag.0);
        }

        let id = notification.id.clone();
        let handler = toast_handler(move |args| {
            let action = match com_query(args, &IID_ACTIVATED_ARGS) {
                Some(activated) => {
                    let activated = Owned(activated);
                    let mut arguments = std::ptr::null_mut();
                    let vtbl = *(activated.0 as *mut *const ActivatedArgsVtbl);
                    let _ = ((*vtbl).get_arguments)(activated.0, &mut arguments);
                    let text = hstring_text(arguments);
                    let _ = WindowsDeleteString(arguments);
                    activated_action(&text)
                }
                None => None,
            };
            emit(AppEvent::NotificationActivated { id: id.clone(), action });
        });
        let mut token = 0i64;
        let vtbl = *(toast.0 as *mut *const ToastVtbl);
        let hr = ((*vtbl).add_activated)(toast.0, handler, &mut token);
        // the toast AddRef'd if it kept the handler; this reference goes
        drop(Owned(handler));
        if !com_ok(hr) {
            return Err(refused("the toast would not take a handler", hr));
        }

        let vtbl = *(notifier.0 as *mut *const NotifierVtbl);
        let hr = ((*vtbl).show)(notifier.0, toast.0);
        if !com_ok(hr) {
            return Err(refused("the toast was not shown", hr));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The house guard against a mis-numbered hand-written vtable.
    #[test]
    fn the_toast_vtables_hold_exactly_the_slots_their_headers_declare() {
        let slot = std::mem::size_of::<usize>();
        assert_eq!(std::mem::size_of::<InspectableVtbl>(), 6 * slot);
        assert_eq!(std::mem::size_of::<ManagerStaticsVtbl>(), 9 * slot);
        assert_eq!(std::mem::size_of::<ToastFactoryVtbl>(), 7 * slot);
        assert_eq!(std::mem::size_of::<ToastVtbl>(), 15 * slot);
        assert_eq!(std::mem::size_of::<Toast2Vtbl>(), 12 * slot);
        assert_eq!(std::mem::size_of::<NotifierVtbl>(), 12 * slot);
        assert_eq!(std::mem::size_of::<ActivatedArgsVtbl>(), 7 * slot);
        assert_eq!(std::mem::size_of::<XmlIoVtbl>(), 9 * slot);
        assert_eq!(std::mem::size_of::<ToastHandlerVtbl>(), 4 * slot);
    }

    #[test]
    fn the_toast_is_the_xml_the_runtime_reads() {
        let letter = Notification::new("t", "Ada <a>", "\"figures\" & more")
            .action("reply", "Reply")
            .action("archive", "Archive");
        let xml = toast_xml(&letter);
        assert!(xml.starts_with("<toast launch=\"bunny:open\">"));
        assert!(xml.contains(
            "<text>Ada &lt;a&gt;</text><text>&quot;figures&quot; &amp; more</text>"
        ));
        assert!(xml.contains("<action content=\"Reply\" arguments=\"bunny:action:reply\"/>"));
        assert!(xml.ends_with("</actions></toast>"));
        assert!(!toast_xml(&Notification::new("t", "a", "b")).contains("<actions>"));
        assert_eq!(activated_action("bunny:action:reply"), Some(String::from("reply")));
        assert_eq!(activated_action("bunny:open"), None);
        assert_eq!(activated_action(""), None);
    }
}
