//! The system's password AutoFill on every Apple platform: what the
//! focused box holds, said in the kit's own words, and the
//! saved-password sheet behind `bunny_ui::autofill`.
//!
//! **What a box holds.** AppKit and UIKit ask the focused input view
//! for a content type (`NSTextContent.contentType`,
//! `UITextInputTraits.textContentType`), and that answer is what lets
//! Passwords offer the saved address to the address box and the code to
//! the code box. The kits name the same values with two prefixes, so
//! [`content_type`] maps a field's [`Autocomplete`] to the constant's
//! NAME and resolves it in the running process. Resolving by name is
//! deliberate: the address, name and new-password constants arrived in
//! macOS 14 while bundles still launch on 10.15, and a symbol linked by
//! address that the system does not export fails the launch. A name the
//! system lacks answers `nil` — the box says nothing, which is what it
//! said before.
//!
//! **The sheet.** [`request_saved_password`] asks AuthenticationServices
//! for the passwords saved for the bundle's associated domains
//! (`webcredentials:` in its entitlements, confirmed by the domain's
//! `apple-app-site-association`). It prefers credentials already on the
//! device (macOS 13, iOS 16), so a device with nothing saved shows no
//! sheet at all and answers a cancel. The delegate is a class born here
//! with no ivars: each request's state waits in this thread's table,
//! keyed by the delegate, until the system calls back on the main
//! thread.
//!
//! The controller and the delegate are AUTORELEASED when the answer
//! arrives, never released: the callback runs inside the controller's
//! own call, and the run loop's pool frees both once it returns.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::null_mut;

use bunny_ui::autofill::{Answer, SavedPassword, SavedPasswordError};
use bunny_ui::text_input::Autocomplete;

use crate::ffi::{
    Id, Sel, class, class_addMethod, class_addProtocol, error_message, objc_allocateClassPair,
    objc_getClass, objc_getProtocol, objc_registerClassPair, sel, text_of,
};

// The framework rides along — its classes resolve by name at run time,
// and the link is what loads them.
#[link(name = "AuthenticationServices", kind = "framework")]
unsafe extern "C" {}

#[link(name = "System", kind = "dylib")]
unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

/// `RTLD_DEFAULT` on Darwin: search every image the process loaded.
const RTLD_DEFAULT: *mut c_void = -2_isize as *mut c_void;

// Re-declaring `objc_msgSend` with the concrete signature of each message
// is the runtime's designed usage (the symbol is a trampoline that
// preserves the call ABI) — the clashing-declarations lint does not apply.
#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    #[link_name = "objc_msgSend"]
    fn msg_id(obj: Id, sel: Sel) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_id(obj: Id, sel: Sel, a: Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_void(obj: Id, sel: Sel);
    #[link_name = "objc_msgSend"]
    fn msg_void_id(obj: Id, sel: Sel, a: Id);
    #[link_name = "objc_msgSend"]
    fn msg_void_u64(obj: Id, sel: Sel, a: u64);
    #[link_name = "objc_msgSend"]
    fn msg_i64(obj: Id, sel: Sel) -> i64;
    #[link_name = "objc_msgSend"]
    fn msg_bool_id(obj: Id, sel: Sel, a: Id) -> i8;
    #[link_name = "objc_msgSend"]
    fn msg_bool_sel(obj: Id, sel: Sel, a: Sel) -> i8;
}

// =============================================================================
// What the focused box holds
// =============================================================================

/// Whose names a shell speaks: the two kits prefix the same constants
/// differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kit {
    /// The Mac: `NSTextContentType…`.
    AppKit,
    /// The phone: `UITextContentType…`.
    UIKit,
}

/// The name of the kit's constant for what a box holds — `None` for
/// [`Autocomplete::Off`], which neither kit can say (an input view has no
/// "suggest nothing" content type), and for a value this build does not
/// know yet.
#[must_use]
pub fn content_type_name(holds: Autocomplete, kit: Kit) -> Option<&'static CStr> {
    let (app_kit, ui_kit): (&'static CStr, &'static CStr) = match holds {
        Autocomplete::Username => (c"NSTextContentTypeUsername", c"UITextContentTypeUsername"),
        Autocomplete::Email => (c"NSTextContentTypeEmailAddress", c"UITextContentTypeEmailAddress"),
        Autocomplete::CurrentPassword => (c"NSTextContentTypePassword", c"UITextContentTypePassword"),
        Autocomplete::NewPassword => (c"NSTextContentTypeNewPassword", c"UITextContentTypeNewPassword"),
        Autocomplete::OneTimeCode => (c"NSTextContentTypeOneTimeCode", c"UITextContentTypeOneTimeCode"),
        Autocomplete::Name => (c"NSTextContentTypeName", c"UITextContentTypeName"),
        Autocomplete::GivenName => (c"NSTextContentTypeGivenName", c"UITextContentTypeGivenName"),
        Autocomplete::FamilyName => (c"NSTextContentTypeFamilyName", c"UITextContentTypeFamilyName"),
        Autocomplete::Organization => {
            (c"NSTextContentTypeOrganizationName", c"UITextContentTypeOrganizationName")
        }
        Autocomplete::Telephone => {
            (c"NSTextContentTypeTelephoneNumber", c"UITextContentTypeTelephoneNumber")
        }
        Autocomplete::Url => (c"NSTextContentTypeURL", c"UITextContentTypeURL"),
        // `Off`, and a value added to the #[non_exhaustive] enum after
        // this build: neither kit has a word for them, so the box says
        // nothing
        _ => return None,
    };
    Some(match kit {
        Kit::AppKit => app_kit,
        Kit::UIKit => ui_kit,
    })
}

/// The kit's `NSString` constant for what a box holds, resolved in the
/// running process — null when the box says nothing, or when this
/// system does not export the name (an older one). The constants live
/// as long as the process: nothing to retain or release.
#[must_use]
pub fn content_type(holds: Option<Autocomplete>, kit: Kit) -> Id {
    let Some(name) = holds.and_then(|holds| content_type_name(holds, kit)) else {
        return null_mut();
    };
    // SAFETY: `dlsym` with RTLD_DEFAULT and a NUL-terminated name reads
    // no memory of ours; a non-null answer is the address of an
    // exported `NSString *const`, which the read below dereferences once.
    unsafe {
        let symbol = dlsym(RTLD_DEFAULT, name.as_ptr()).cast::<Id>();
        if symbol.is_null() { null_mut() } else { *symbol }
    }
}

// =============================================================================
// The saved-password sheet
// =============================================================================

/// `ASAuthorizationErrorCanceled`: the person closed the sheet — or,
/// with credentials preferred from the device, there were none.
const CANCELED: i64 = 1001;
/// `ASAuthorizationErrorNotInteractive`: the request needed a person and
/// was not allowed to ask one.
const NOT_INTERACTIVE: i64 = 1005;
/// `ASAuthorizationControllerRequestOptionPreferImmediatelyAvailableCredentials`.
const PREFER_IMMEDIATELY_AVAILABLE: u64 = 1;

/// The system's error code, in the vocabulary of `bunny_ui::autofill`.
#[must_use]
pub fn outcome_of_error(code: i64, reason: String) -> SavedPasswordError {
    match code {
        CANCELED | NOT_INTERACTIVE => SavedPasswordError::Declined,
        code => SavedPasswordError::Failed { code, reason },
    }
}

/// One request waiting for the system.
struct Pending {
    answer: Answer,
    /// The window the sheet hangs from, retained for the request's life.
    anchor: Id,
    /// The controller, owned here (+1) until the answer.
    controller: Id,
}

thread_local! {
    /// The requests in flight, by the address of their delegate.
    static PENDING: RefCell<HashMap<usize, Pending>> = RefCell::new(HashMap::new());
}

/// Shows the system's saved-password sheet over `anchor` and gives the
/// outcome to `answer` when the system replies — on this thread, from
/// the run loop. A system without AuthenticationServices' password
/// provider answers [`SavedPasswordError::Unsupported`] at once, and a
/// missing window [`SavedPasswordError::Failed`].
///
/// # Safety
///
/// Called on the main thread. `anchor` is null or a live window of the
/// kit (`NSWindow`, `UIWindow`).
pub unsafe fn request_saved_password(anchor: Id, answer: Answer) {
    unsafe {
        let provider_class = class("ASAuthorizationPasswordProvider");
        let controller_class = class("ASAuthorizationController");
        if provider_class.is_null() || controller_class.is_null() {
            answer.give(Err(SavedPasswordError::Unsupported));
            return;
        }
        if anchor.is_null() {
            answer.give(Err(SavedPasswordError::Failed {
                code: 0,
                reason: String::from("no window to show the saved-password sheet over"),
            }));
            return;
        }
        let delegate_class = delegate_class();
        if delegate_class.is_null() {
            answer.give(Err(SavedPasswordError::Failed {
                code: 0,
                reason: String::from("the request's delegate class could not be registered"),
            }));
            return;
        }

        let provider = msg_id(msg_id(provider_class, sel("alloc")), sel("init"));
        let request = msg_id(provider, sel("createRequest"));
        let requests = msg_id_id(class("NSArray"), sel("arrayWithObject:"), request);
        let controller = msg_id_id(
            msg_id(controller_class, sel("alloc")),
            sel("initWithAuthorizationRequests:"),
            requests,
        );
        // the request holds what it needs of its provider
        msg_void(provider, sel("release"));
        let delegate = msg_id(msg_id(delegate_class, sel("alloc")), sel("init"));
        // both properties are weak: the table below is what keeps the
        // delegate alive, and the controller keeps itself alive until
        // it has called back
        msg_void_id(controller, sel("setDelegate:"), delegate);
        msg_void_id(controller, sel("setPresentationContextProvider:"), delegate);
        msg_id(anchor, sel("retain"));
        PENDING.with(|pending| {
            pending.borrow_mut().insert(delegate as usize, Pending { answer, anchor, controller });
        });
        if msg_bool_sel(controller, sel("respondsToSelector:"), sel("performRequestsWithOptions:")) != 0 {
            msg_void_u64(controller, sel("performRequestsWithOptions:"), PREFER_IMMEDIATELY_AVAILABLE);
        } else {
            msg_void(controller, sel("performRequests"));
        }
    }
}

/// The delegate class, registered once per process. Another copy of this
/// crate in the process (a reloaded dylib) finds it registered and uses
/// it as it stands.
fn delegate_class() -> Id {
    static REGISTERED: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *REGISTERED.get_or_init(|| unsafe {
        let name = c"BunnyPasswordRequest";
        let existing = objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing as usize;
        }
        let born = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
        if born.is_null() {
            return 0;
        }
        let answered = CString::new("v@:@@").expect("type encoding");
        class_addMethod(
            born,
            sel("authorizationController:didCompleteWithAuthorization:"),
            completed_with_authorization as *const c_void,
            answered.as_ptr(),
        );
        class_addMethod(
            born,
            sel("authorizationController:didCompleteWithError:"),
            completed_with_error as *const c_void,
            answered.as_ptr(),
        );
        let anchor = CString::new("@@:@").expect("type encoding");
        class_addMethod(
            born,
            sel("presentationAnchorForAuthorizationController:"),
            presentation_anchor as *const c_void,
            anchor.as_ptr(),
        );
        for protocol in
            [c"ASAuthorizationControllerDelegate", c"ASAuthorizationControllerPresentationContextProviding"]
        {
            let protocol = objc_getProtocol(protocol.as_ptr());
            if !protocol.is_null() {
                class_addProtocol(born, protocol);
            }
        }
        objc_registerClassPair(born);
        born as usize
    }) as Id
}

/// `presentationAnchorForAuthorizationController:` — the window the
/// request was given.
extern "C" fn presentation_anchor(this: Id, _sel: Sel, _controller: Id) -> Id {
    PENDING.with(|pending| pending.borrow().get(&(this as usize)).map_or(null_mut(), |held| held.anchor))
}

/// `authorizationController:didCompleteWithAuthorization:` — the person
/// picked an account.
extern "C" fn completed_with_authorization(this: Id, _sel: Sel, _controller: Id, authorization: Id) {
    let outcome = unsafe {
        let credential = msg_id(authorization, sel("credential"));
        let password_class = class("ASPasswordCredential");
        if credential.is_null()
            || password_class.is_null()
            || msg_bool_id(credential, sel("isKindOfClass:"), password_class) == 0
        {
            Err(SavedPasswordError::Failed {
                code: 0,
                reason: String::from("the system answered with a credential that is not a password"),
            })
        } else {
            let user = text_of(msg_id(credential, sel("user"))).unwrap_or_default();
            let password = text_of(msg_id(credential, sel("password"))).unwrap_or_default();
            Ok(SavedPassword::new(user, password))
        }
    };
    finish(this, outcome);
}

/// `authorizationController:didCompleteWithError:` — a cancel, nothing
/// saved, or a refusal.
extern "C" fn completed_with_error(this: Id, _sel: Sel, _controller: Id, error: Id) {
    let outcome = unsafe {
        let code = if error.is_null() { 0 } else { msg_i64(error, sel("code")) };
        outcome_of_error(code, error_message(error))
    };
    finish(this, Err(outcome));
}

/// Ends the request the delegate `this` stands for: the answer reaches
/// the app, and what the request held goes back to the system.
fn finish(this: Id, outcome: Result<SavedPassword, SavedPasswordError>) {
    let Some(pending) = PENDING.with(|pending| pending.borrow_mut().remove(&(this as usize))) else {
        return;
    };
    pending.answer.give(outcome);
    unsafe {
        msg_void(pending.anchor, sel("release"));
        // both are on the stack of this very call: the pool frees them
        // once the system's callback returns
        msg_id(pending.controller, sel("autorelease"));
        msg_id(this, sel("autorelease"));
    }
}

#[cfg(test)]
#[path = "autofill_tests.rs"]
mod tests;
