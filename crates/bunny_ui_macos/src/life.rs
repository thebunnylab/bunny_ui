//! The app's life outside its window, on this platform: the application
//! delegate (a reopen, a url handed over), the workspace's sleep and
//! wake, and the desktop's notifications (UserNotifications) — each one
//! answering a door of `bunny_ui::app`.
//!
//! Notifications need a BUNDLE. The system's own notification center
//! raises for a process with no bundle, so a bare binary is refused
//! by name instead: a dev binary is shown by wrapping it in an `.app`
//! whose `Info.plist` names a `CFBundleIdentifier`. A bundled app is
//! also ONE process by the system's own rule — a second launch and a
//! url both reach the running one through the delegate, and land as
//! the same event the spool delivers everywhere else.

use std::cell::Cell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::null_mut;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

use bunny_ui::app::{AppEvent, Notification, emit};

use crate::ffi::{Id, Sel, class, sel};

// The framework rides along — the classes resolve by name at runtime,
// and the link is what loads them.
#[link(name = "UserNotifications", kind = "framework")]
unsafe extern "C" {}

#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    #[link_name = "objc_msgSend"]
    fn msg_id(obj: Id, sel: Sel) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_id(obj: Id, sel: Sel, a: Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_id_id_id(obj: Id, sel: Sel, a: Id, b: Id, c: Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_id_id_u64(obj: Id, sel: Sel, a: Id, b: Id, c: u64) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_id_id_id_u64(obj: Id, sel: Sel, a: Id, b: Id, c: Id, d: u64) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_cstr(obj: Id, sel: Sel, a: *const c_char) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_u64(obj: Id, sel: Sel, a: u64) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_void(obj: Id, sel: Sel);
    #[link_name = "objc_msgSend"]
    fn msg_void_id(obj: Id, sel: Sel, a: Id);
    #[link_name = "objc_msgSend"]
    fn msg_void_id_id(obj: Id, sel: Sel, a: Id, b: Id);
    #[link_name = "objc_msgSend"]
    fn msg_void_id_sel_id_id(obj: Id, sel: Sel, a: Id, b: Sel, c: Id, d: Id);
    #[link_name = "objc_msgSend"]
    fn msg_void_u64_id(obj: Id, sel: Sel, a: u64, b: Id);
    #[link_name = "objc_msgSend"]
    fn msg_i64(obj: Id, sel: Sel) -> i64;

    fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra: usize) -> Id;
    fn objc_registerClassPair(class: Id);
    fn class_addMethod(class: Id, sel: Sel, imp: *const c_void, types: *const c_char) -> i8;
    fn objc_getProtocol(name: *const c_char) -> Id;
    fn class_addProtocol(class: Id, protocol: Id) -> i8;
}

#[link(name = "System", kind = "dylib")]
unsafe extern "C" {
    static _NSConcreteStackBlock: [*const c_void; 32];
}

thread_local! {
    /// The ONE delegate — the application's, the workspace's
    /// observer and the notification center's, in one object that
    /// lives as long as the process.
    static DELEGATE: Cell<Id> = const { Cell::new(null_mut()) };
}

/// Whether the person allowed notifications — asked once, answered
/// on a later turn. 0 not asked, 1 asking, 2 granted, 3 denied.
static AUTHORIZED: AtomicU8 = AtomicU8::new(0);
/// Why they were denied, in the system's words.
static DENIAL: Mutex<Option<String>> = Mutex::new(None);

/// `UNNotificationPresentationOptions` for a notification that arrives
/// while the app is frontmost: the banner, the list, the sound — a
/// person who is looking at the mail still hears the letter land.
const PRESENT_BANNER: u64 = 1 << 4;
const PRESENT_LIST: u64 = 1 << 3;
const PRESENT_SOUND: u64 = 1 << 1;
/// `UNAuthorizationOptions`: badge, sound, alert.
const AUTHORIZATION_ASK: u64 = 1 | 2 | 4;
/// `UNNotificationActionOptionForeground` — a button brings the app
/// to the front, which is what a "Reply" button means.
const ACTION_FOREGROUND: u64 = 1 << 2;

/// Installs the delegate and the observers, and the notifier — at
/// boot, before the application runs, so a launch that comes from a
/// notification click or a url finds the doors already open.
pub(crate) fn install() {
    let delegate = delegate();
    unsafe {
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        msg_void_id(app, sel("setDelegate:"), delegate);
        let workspace = msg_id(class("NSWorkspace"), sel("sharedWorkspace"));
        let center = msg_id(workspace, sel("notificationCenter"));
        for (name, door) in [
            ("NSWorkspaceWillSleepNotification", "bunnyWillSleep:"),
            ("NSWorkspaceDidWakeNotification", "bunnyDidWake:"),
        ] {
            msg_void_id_sel_id_id(
                center,
                sel("addObserver:selector:name:object:"),
                delegate,
                sel(door),
                ns(name),
                null_mut(),
            );
        }
        // a bundled app's notification center learns its delegate
        // now: a click on a notification can be the very thing that
        // launched the process, and the response arrives early
        if bundled() {
            let center =
                msg_id(class("UNUserNotificationCenter"), sel("currentNotificationCenter"));
            if !center.is_null() {
                msg_void_id(center, sel("setDelegate:"), delegate);
            }
        }
    }
    bunny_ui::app::install_notifier(notify);
}

/// Does this process have a bundle identifier — the one thing the
/// notification center will not do without?
fn bundled() -> bool {
    unsafe {
        let bundle = msg_id(class("NSBundle"), sel("mainBundle"));
        !bundle.is_null() && !msg_id(bundle, sel("bundleIdentifier")).is_null()
    }
}

/// Shows a notification, or says why it cannot: no bundle, a person
/// who said no. The first ask requests the person's permission and
/// still posts — the system holds the notification until they answer.
fn notify(notification: &Notification) -> Result<(), String> {
    if !bundled() {
        return Err(String::from(
            "notifications need an app bundle: this process has no bundle identifier",
        ));
    }
    unsafe {
        let center = msg_id(class("UNUserNotificationCenter"), sel("currentNotificationCenter"));
        if center.is_null() {
            return Err(String::from("the notification center is not available"));
        }
        match AUTHORIZED.load(Ordering::SeqCst) {
            0 => request_authorization(center),
            3 => {
                let why = DENIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
                return Err(why.unwrap_or_else(|| {
                    String::from("the person did not allow notifications for this app")
                }));
            }
            _ => {}
        }
        msg_void_id(center, sel("setDelegate:"), delegate());

        // the buttons: a category per SET of keys, remembered, so a
        // later notification with other buttons does not unmake this one
        let keys: Vec<&str> =
            notification.actions.iter().map(|action| action.key.as_str()).collect();
        let category_id = format!("bunny:{}", keys.join(","));
        let actions = msg_id(class("NSMutableArray"), sel("array"));
        for action in &notification.actions {
            let button = msg_id_id_id_u64(
                class("UNNotificationAction"),
                sel("actionWithIdentifier:title:options:"),
                ns(&action.key),
                ns(&action.label),
                ACTION_FOREGROUND,
            );
            msg_void_id(actions, sel("addObject:"), button);
        }
        let category = msg_id_id_id_id_u64(
            class("UNNotificationCategory"),
            sel("categoryWithIdentifier:actions:intentIdentifiers:options:"),
            ns(&category_id),
            actions,
            msg_id(class("NSArray"), sel("array")),
            0,
        );
        let categories = known_categories();
        msg_void_id(categories, sel("addObject:"), category);
        msg_void_id(center, sel("setNotificationCategories:"), categories);

        let content =
            msg_id(msg_id(class("UNMutableNotificationContent"), sel("alloc")), sel("init"));
        msg_void_id(content, sel("setTitle:"), ns(&notification.title));
        msg_void_id(content, sel("setBody:"), ns(&notification.body));
        msg_void_id(content, sel("setCategoryIdentifier:"), ns(&category_id));
        let sound = msg_id(class("UNNotificationSound"), sel("defaultSound"));
        msg_void_id(content, sel("setSound:"), sound);
        let request = msg_id_id_id_id(
            class("UNNotificationRequest"),
            sel("requestWithIdentifier:content:trigger:"),
            ns(&notification.id),
            content,
            null_mut(),
        );
        // the same identifier REPLACES: the center keeps one per id
        msg_void_id_id(
            center,
            sel("addNotificationRequest:withCompletionHandler:"),
            request,
            null_mut(),
        );
        msg_void(content, sel("release"));
    }
    Ok(())
}

thread_local! {
    /// Every category registered so far — the set the center is
    /// handed each time, whole.
    static CATEGORIES: Cell<Id> = const { Cell::new(null_mut()) };
}

unsafe fn known_categories() -> Id {
    CATEGORIES.with(|slot| {
        if slot.get().is_null() {
            unsafe {
                let set = msg_id(msg_id(class("NSMutableSet"), sel("alloc")), sel("init"));
                slot.set(set);
            }
        }
        slot.get()
    })
}

/// The ONE authored block of this module — the authorization's
/// completion, `(BOOL granted, NSError *error)`. The layout the
/// runtime documents, no captures, so a byte copy is the whole move.
#[repr(C)]
struct AuthorizationBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: extern "C" fn(*mut AuthorizationBlock, i8, Id),
    descriptor: *const BlockDescriptor,
}

#[repr(C)]
struct BlockDescriptor {
    reserved: u64,
    size: u64,
}

static AUTHORIZATION_DESCRIPTOR: BlockDescriptor =
    BlockDescriptor { reserved: 0, size: std::mem::size_of::<AuthorizationBlock>() as u64 };

extern "C" fn authorization_answered(_block: *mut AuthorizationBlock, granted: i8, error: Id) {
    if granted != 0 {
        AUTHORIZED.store(2, Ordering::SeqCst);
        return;
    }
    let why = unsafe { error_name(error) };
    *DENIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(why);
    AUTHORIZED.store(3, Ordering::SeqCst);
}

unsafe fn request_authorization(center: Id) {
    AUTHORIZED.store(1, Ordering::SeqCst);
    let block = AuthorizationBlock {
        isa: (&raw const _NSConcreteStackBlock) as *const c_void,
        flags: 0,
        reserved: 0,
        invoke: authorization_answered,
        descriptor: &AUTHORIZATION_DESCRIPTOR,
    };
    unsafe {
        // the center copies the block before this returns — the stack
        // literal only has to live through the send
        msg_void_u64_id(
            center,
            sel("requestAuthorizationWithOptions:completionHandler:"),
            AUTHORIZATION_ASK,
            (&raw const block) as Id,
        );
    }
}

/// The block the engine hands a delegate: called once, with the
/// answer — read far enough to find `invoke`, never authored.
#[repr(C)]
struct AnswerBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: unsafe extern "C" fn(*mut AnswerBlock, u64),
}

/// `applicationShouldHandleReopen:hasVisibleWindows:` — the Dock icon,
/// or a second launch of a bundled app: a reopen with no arguments.
/// YES lets AppKit do its own part (unminiaturize, show).
extern "C" fn bridge_reopen(_this: Id, _sel: Sel, _app: Id, _visible: i8) -> i8 {
    emit(AppEvent::Reopened { arguments: Vec::new() });
    1
}

/// `application:openURLs:` — a url the system handed this app: each
/// one an argument of a reopen, the same event the spool delivers.
extern "C" fn bridge_open_urls(_this: Id, _sel: Sel, _app: Id, urls: Id) {
    let mut arguments = Vec::new();
    unsafe {
        let count = msg_i64(urls, sel("count")).max(0) as u64;
        for index in 0..count {
            let url = msg_id_u64(urls, sel("objectAtIndex:"), index);
            let text = to_string(msg_id(url, sel("absoluteString")));
            if !text.is_empty() {
                arguments.push(text);
            }
        }
    }
    if !arguments.is_empty() {
        emit(AppEvent::Reopened { arguments });
    }
}

extern "C" fn bridge_will_sleep(_this: Id, _sel: Sel, _notification: Id) {
    emit(AppEvent::WillSleep);
}

extern "C" fn bridge_did_wake(_this: Id, _sel: Sel, _notification: Id) {
    emit(AppEvent::DidWake);
}

/// `userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:`
/// — the person acted on a notification: the button they pressed, or
/// the notification itself. A dismissal is not an activation and is
/// not reported. The handler is called on every road out.
extern "C" fn bridge_response(_this: Id, _sel: Sel, _center: Id, response: Id, handler: Id) {
    const DEFAULT: &str = "com.apple.UNNotificationDefaultActionIdentifier";
    const DISMISS: &str = "com.apple.UNNotificationDismissActionIdentifier";
    unsafe {
        let action = to_string(msg_id(response, sel("actionIdentifier")));
        let request = msg_id(msg_id(response, sel("notification")), sel("request"));
        let id = to_string(msg_id(request, sel("identifier")));
        if action != DISMISS {
            let action = (action != DEFAULT).then_some(action);
            emit(AppEvent::NotificationActivated { id, action });
        }
        let block = handler as *mut AnswerBlock;
        if !block.is_null() {
            // a void block: the argument is unread
            ((*block).invoke)(block, 0);
        }
    }
}

/// `userNotificationCenter:willPresentNotification:withCompletionHandler:`
/// — a notification arrived while the app is in front: show it anyway.
extern "C" fn bridge_present(_this: Id, _sel: Sel, _center: Id, _notification: Id, handler: Id) {
    unsafe {
        let block = handler as *mut AnswerBlock;
        if !block.is_null() {
            ((*block).invoke)(block, PRESENT_BANNER | PRESENT_LIST | PRESENT_SOUND);
        }
    }
}

/// The one delegate instance, built on first use.
fn delegate() -> Id {
    DELEGATE.with(|slot| {
        let existing = slot.get();
        if !existing.is_null() {
            return existing;
        }
        let instance = unsafe {
            let name = CString::new("BunnyAppDelegate").expect("class name");
            let bridge = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
            let methods: [(&str, *const c_void, &str); 6] = [
                (
                    "applicationShouldHandleReopen:hasVisibleWindows:",
                    bridge_reopen as *const c_void,
                    "B@:@B",
                ),
                ("application:openURLs:", bridge_open_urls as *const c_void, "v@:@@"),
                ("bunnyWillSleep:", bridge_will_sleep as *const c_void, "v@:@"),
                ("bunnyDidWake:", bridge_did_wake as *const c_void, "v@:@"),
                (
                    "userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:",
                    bridge_response as *const c_void,
                    "v@:@@@?",
                ),
                (
                    "userNotificationCenter:willPresentNotification:withCompletionHandler:",
                    bridge_present as *const c_void,
                    "v@:@@@?",
                ),
            ];
            for (selector, imp, types) in methods {
                let types = CString::new(types).expect("type encoding");
                class_addMethod(bridge, sel(selector), imp, types.as_ptr());
            }
            for protocol in ["NSApplicationDelegate", "UNUserNotificationCenterDelegate"] {
                let protocol = CString::new(protocol).expect("protocol name");
                let protocol = objc_getProtocol(protocol.as_ptr());
                if !protocol.is_null() {
                    class_addProtocol(bridge, protocol);
                }
            }
            objc_registerClassPair(bridge);
            msg_id(msg_id(bridge, sel("alloc")), sel("init"))
        };
        slot.set(instance);
        instance
    })
}

/// The error's own words, or a name for silence.
unsafe fn error_name(error: Id) -> String {
    if error.is_null() {
        return String::from("the person did not allow notifications for this app");
    }
    unsafe {
        let text = to_string(msg_id(error, sel("localizedDescription")));
        if text.is_empty() { String::from("notifications were refused unnamed") } else { text }
    }
}

/// A borrowed NSString for the message being sent (autoreleased).
unsafe fn ns(text: &str) -> Id {
    let text = CString::new(text).unwrap_or_default();
    unsafe { msg_id_cstr(class("NSString"), sel("stringWithUTF8String:"), text.as_ptr()) }
}

/// The NSString's bytes, copied out.
unsafe fn to_string(ns: Id) -> String {
    if ns.is_null() {
        return String::new();
    }
    unsafe {
        let utf8 = msg_id(ns, sel("UTF8String")) as *const c_char;
        if utf8.is_null() {
            return String::new();
        }
        CStr::from_ptr(utf8).to_string_lossy().into_owned()
    }
}
