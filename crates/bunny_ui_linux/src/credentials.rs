//! The system's own secret store, through the house FFI.
//!
//! A secret is not settings. An app writes its configuration to a file
//! the reader opens in a tab and commits to a repository, and a key
//! that reaches a paid service must never live there. The desktop
//! already keeps a store for exactly this, guarded by the reader's own
//! login, and this module is the door to it.
//!
//! An item is named by a PAIR: the service it belongs to and the
//! account inside it. Both travel as attributes, so the reader finds
//! the item under a readable label in whatever the desktop shows its
//! keyring with.
//!
//! **Why libsecret and not the wire.** The store here is a D-Bus
//! service rather than a call, and libsecret is the desktop's own
//! client for it — the same standing that Security.framework has on the
//! Mac and `advapi32` on Windows, and the same standing that fontconfig
//! and FreeType already have in this shell. The house writes its
//! bindings by hand, which is what this file is; it does not
//! reimplement the service a platform already ships a client for. What
//! the client carries is exactly the part that fails QUIETLY when it is
//! written twice: the session negotiation, the prompt that unlocks a
//! locked collection, and the difference between one desktop's keyring
//! daemon and another's.
//!
//! **These calls block.** They are the platform's own, they cross a bus
//! and the desktop may ask the reader to unlock the keyring — so they
//! belong on a thread, not in a body. The macOS twin carries the shape.
//!
//! ## Production gotchas
//!
//! **A desktop may have no store at all.** The Mac always has a keychain
//! and Windows a Credential Manager; here the store is a SERVICE, and a
//! compositor session that starts no keyring daemon — niri, sway or
//! Hyprland without gnome-keyring, KeePassXC or oo7 — has nobody on the
//! bus to answer. [`read`] then answers `None`, exactly as for a pair
//! that holds nothing, and [`write`] answers `false`, exactly as for a
//! refusal: the answers a settings page needs, and the wrong ones for a
//! caller that promised to keep something. That caller asks
//! [`availability`], which tells the absence apart, and says the truth.
//! This door never keeps the secret anywhere else — a token in a file
//! that reads like a keyring is the failure that looks like success, and
//! the Android twin carries the same rule.

use std::ffi::{CStr, CString, c_char, c_int, c_void};

use crate::wpe::GError;

/// `SECRET_SCHEMA_NONE` — the schema name is part of the match, so an
/// item written through this door is found through this door and not
/// confused with one some other tool wrote under the same attributes.
const SECRET_SCHEMA_NONE: c_int = 0;
/// `SECRET_SCHEMA_ATTRIBUTE_STRING`.
const ATTRIBUTE_STRING: c_int = 0;
/// libsecret's own count — the array is fixed and the tail is zeroed.
const SCHEMA_ATTRIBUTE_SLOTS: usize = 32;
/// `G_DBUS_ERROR_SERVICE_UNKNOWN` — nothing holds the name and nothing
/// can start it: what the bus says when no provider is installed.
const SERVICE_UNKNOWN: c_int = 2;
/// `G_DBUS_ERROR_NAME_HAS_NO_OWNER` — the name is known, but nothing
/// holds it now.
const NAME_HAS_NO_OWNER: c_int = 3;
/// The pair [`availability`] asks for. Nothing in the house writes it, so
/// the lookup can only find nothing or fail — and finding nothing
/// unlocks nothing, so the question never raises a prompt.
const PROBE_SERVICE: &CStr = c"com.thebunnylab.bunny_ui.availability";
const PROBE_ACCOUNT: &CStr = c"probe";

#[repr(C)]
#[derive(Clone, Copy)]
struct SecretSchemaAttribute {
    name: *const c_char,
    kind: c_int,
}

/// `SecretSchema`. The private tail is reserved and stays zero — the
/// struct is passed by pointer and libsecret only reads what it owns.
#[repr(C)]
struct SecretSchema {
    name: *const c_char,
    flags: c_int,
    attributes: [SecretSchemaAttribute; SCHEMA_ATTRIBUTE_SLOTS],
    reserved: c_int,
    reserved1: *mut c_void,
    reserved2: *mut c_void,
    reserved3: *mut c_void,
    reserved4: *mut c_void,
    reserved5: *mut c_void,
    reserved6: *mut c_void,
    reserved7: *mut c_void,
}

// the schema is read-only for libsecret's whole call and never leaves
// this module — the raw pointers inside it all point at `'static` text
unsafe impl Sync for SecretSchema {}

#[link(name = "secret-1")]
unsafe extern "C" {
    /// The stored password, or NULL. The caller frees it with
    /// [`secret_password_free`]. Attributes come as NULL-terminated
    /// name/value pairs.
    fn secret_password_lookup_sync(
        schema: *const SecretSchema,
        cancellable: *mut c_void,
        error: *mut *mut GError,
        ...
    ) -> *mut c_char;
    fn secret_password_store_sync(
        schema: *const SecretSchema,
        collection: *const c_char,
        label: *const c_char,
        password: *const c_char,
        cancellable: *mut c_void,
        error: *mut *mut GError,
        ...
    ) -> c_int;
    fn secret_password_clear_sync(
        schema: *const SecretSchema,
        cancellable: *mut c_void,
        error: *mut *mut GError,
        ...
    ) -> c_int;
    fn secret_password_free(password: *mut c_char);
}

#[link(name = "glib-2.0")]
unsafe extern "C" {
    /// Frees an error libsecret handed back, message and all.
    fn g_error_free(error: *mut GError);
}

#[link(name = "gio-2.0")]
unsafe extern "C" {
    /// `G_DBUS_ERROR` — the domain of an error the bus itself answered.
    fn g_dbus_error_quark() -> u32;
}

/// The pair, as libsecret sees it. The name namespaces the attribute
/// set; it is not shown to the reader.
static SCHEMA: SecretSchema = SecretSchema {
    name: c"com.thebunnylab.bunny_ui.Credential".as_ptr(),
    flags: SECRET_SCHEMA_NONE,
    attributes: {
        let mut slots = [SecretSchemaAttribute { name: std::ptr::null(), kind: 0 };
            SCHEMA_ATTRIBUTE_SLOTS];
        slots[0] = SecretSchemaAttribute { name: c"service".as_ptr(), kind: ATTRIBUTE_STRING };
        slots[1] = SecretSchemaAttribute { name: c"account".as_ptr(), kind: ATTRIBUTE_STRING };
        slots
    },
    reserved: 0,
    reserved1: std::ptr::null_mut(),
    reserved2: std::ptr::null_mut(),
    reserved3: std::ptr::null_mut(),
    reserved4: std::ptr::null_mut(),
    reserved5: std::ptr::null_mut(),
    reserved6: std::ptr::null_mut(),
    reserved7: std::ptr::null_mut(),
};

/// The two attribute values, as C text. A pair carrying an interior NUL
/// is not a pair this store can name.
fn pair(service: &str, account: &str) -> Option<(CString, CString)> {
    Some((CString::new(service).ok()?, CString::new(account).ok()?))
}

/// The secret stored for this service and account, or `None` when the
/// pair carries none — which is the ordinary answer for a key the
/// reader has not entered yet.
///
/// A secret that is not valid UTF-8 also answers `None`: this door
/// carries the text a settings page types, and bytes that are not text
/// were not written through it.
pub fn read(service: &str, account: &str) -> Option<String> {
    let (service, account) = pair(service, account)?;
    let found = unsafe {
        secret_password_lookup_sync(
            &raw const SCHEMA,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            c"service".as_ptr(),
            service.as_ptr(),
            c"account".as_ptr(),
            account.as_ptr(),
            std::ptr::null::<c_char>(),
        )
    };
    if found.is_null() {
        return None;
    }
    let secret = unsafe { CStr::from_ptr(found) }.to_str().ok().map(str::to_owned);
    // the store's memory, freed by the store — and always, even when
    // the bytes turned out not to be text
    unsafe { secret_password_free(found) };
    secret
}

/// Stores the secret for this service and account, replacing whatever
/// the pair held — libsecret writes over an item with the same
/// attributes, which is what a settings page means by saving. `true` =
/// the store took it.
pub fn write(service: &str, account: &str, secret: &str) -> bool {
    let Some((service_c, account_c)) = pair(service, account) else {
        return false;
    };
    let Ok(secret_c) = CString::new(secret) else {
        return false;
    };
    // what the reader sees in the desktop's keyring window; the same
    // shape Windows uses for its one lookup name
    let Ok(label) = CString::new(format!("{service}/{account}")) else {
        return false;
    };
    unsafe {
        secret_password_store_sync(
            &raw const SCHEMA,
            // NULL is the default collection: the reader's login keyring
            std::ptr::null(),
            label.as_ptr(),
            secret_c.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            c"service".as_ptr(),
            service_c.as_ptr(),
            c"account".as_ptr(),
            account_c.as_ptr(),
            std::ptr::null::<c_char>(),
        ) != 0
    }
}

/// Removes the secret for this service and account. `true` = the pair
/// carries none NOW, which a pair that never carried one already
/// satisfied — deleting is idempotent, the way a settings page needs.
pub fn delete(service: &str, account: &str) -> bool {
    let Some((service_c, account_c)) = pair(service, account) else {
        return false;
    };
    let cleared = unsafe {
        secret_password_clear_sync(
            &raw const SCHEMA,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            c"service".as_ptr(),
            service_c.as_ptr(),
            c"account".as_ptr(),
            account_c.as_ptr(),
            std::ptr::null::<c_char>(),
        )
    };
    // libsecret answers false for "there was nothing to remove" as well
    // as for a real failure, and only one of those is a failure
    cleared != 0 || read(service, account).is_none()
}

/// Why this desktop has no store the door can open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// The session bus answered, and nothing on it holds or can start
    /// `org.freedesktop.secrets`: a session that runs no keyring daemon.
    NoProvider,
    /// libsecret failed some other way — no session bus to ask, or a
    /// provider that answered with an error. GLib's own words, verbatim.
    Failed(String),
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoProvider => f.write_str("no Secret Service provider on the session bus"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Unavailable {}

/// Whether this desktop has a store to open at all — the one question
/// [`read`] and [`write`] cannot answer (see the module's gotchas).
///
/// It asks libsecret itself, not the bus's list of names, so the answer
/// follows whichever backend libsecret would pick for the three calls
/// above — its own file store inside a sandbox included. It blocks like
/// they do: one round trip on the session bus, plus the start of a
/// provider that is installed but not running yet.
///
/// # Errors
///
/// [`Unavailable::NoProvider`] when the bus has nobody to answer for the
/// store; [`Unavailable::Failed`] for every other way the question failed.
pub fn availability() -> Result<(), Unavailable> {
    let mut error: *mut GError = std::ptr::null_mut();
    let found = unsafe {
        secret_password_lookup_sync(
            &raw const SCHEMA,
            std::ptr::null_mut(),
            &raw mut error,
            c"service".as_ptr(),
            PROBE_SERVICE.as_ptr(),
            c"account".as_ptr(),
            PROBE_ACCOUNT.as_ptr(),
            std::ptr::null::<c_char>(),
        )
    };
    if !found.is_null() {
        // somebody wrote the reserved pair; a store answered all the same
        unsafe { secret_password_free(found) };
    }
    if error.is_null() {
        return Ok(());
    }
    // GLib's memory: copied out, then handed back to GLib — always
    let raised = unsafe {
        let copied = Raised {
            domain: (*error).domain,
            code: (*error).code,
            message: if (*error).message.is_null() {
                String::new()
            } else {
                CStr::from_ptr((*error).message).to_string_lossy().into_owned()
            },
        };
        g_error_free(error);
        copied
    };
    Err(raised.into_unavailable(unsafe { g_dbus_error_quark() }))
}

/// A `GError`, copied out of GLib's memory.
struct Raised {
    domain: u32,
    code: c_int,
    message: String,
}

impl Raised {
    /// The bus's own "nobody is there" is the absence; every other
    /// failure travels in GLib's words. A code means something only inside
    /// its domain, so the domain is checked first — `2` from GIO's own
    /// domain is not the bus's `ServiceUnknown`.
    fn into_unavailable(self, dbus: u32) -> Unavailable {
        match self.code {
            SERVICE_UNKNOWN | NAME_HAS_NO_OWNER if self.domain == dbus => Unavailable::NoProvider,
            _ => Unavailable::Failed(self.message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two quarks as the process might have registered them: which one is
    /// the bus's is the point, not the numbers.
    const DBUS: u32 = 7;
    const IO: u32 = 9;

    fn raised(domain: u32, code: c_int) -> Raised {
        Raised {
            domain,
            code,
            message: "GDBus.Error:org.freedesktop.DBus.Error.ServiceUnknown: \
                      The name is not activatable"
                .to_owned(),
        }
    }

    /// Nothing can start the name, or nothing holds it now: either way
    /// the desktop has no store, and that is the one answer named.
    #[test]
    fn the_bus_saying_nobody_is_there_is_no_provider() {
        assert_eq!(raised(DBUS, SERVICE_UNKNOWN).into_unavailable(DBUS), Unavailable::NoProvider);
        assert_eq!(raised(DBUS, NAME_HAS_NO_OWNER).into_unavailable(DBUS), Unavailable::NoProvider);
    }

    /// A code is read inside its domain: the same number from another
    /// domain, or another code from the bus, is carried in GLib's words
    /// instead of guessed into an absence.
    #[test]
    fn every_other_failure_is_carried_in_glibs_words() {
        let words = raised(IO, SERVICE_UNKNOWN).into_unavailable(DBUS);
        assert!(
            matches!(&words, Unavailable::Failed(message) if message.ends_with("not activatable")),
            "{words:?}",
        );
        // G_DBUS_ERROR_FAILED: the bus answered, and it was not about who is there
        assert!(matches!(raised(DBUS, 0).into_unavailable(DBUS), Unavailable::Failed(_)));
    }

    /// The absence reads as a sentence, and a failure as exactly GLib's.
    #[test]
    fn a_reason_reads_as_its_own_sentence() {
        assert_eq!(
            Unavailable::NoProvider.to_string(),
            "no Secret Service provider on the session bus",
        );
        assert_eq!(Unavailable::Failed("verbatim".to_owned()).to_string(), "verbatim");
    }

    /// On a session with no keyring daemon — the bare compositor this
    /// module's gotchas describe — the store is named absent, while the
    /// doors still answer what they answer for an empty pair.
    #[test]
    #[ignore = "requires a session bus with no Secret Service provider"]
    fn no_secret_service_is_named_and_not_taken_for_an_empty_store() {
        assert_eq!(availability(), Err(Unavailable::NoProvider));
        assert_eq!(read("bunny-ui-test", "absent"), None);
        assert!(!write("bunny-ui-test", "absent", "never kept"));
    }

    /// With a provider on the bus — running, or started by the asking —
    /// the store is there.
    #[test]
    #[ignore = "requires a running Secret Service provider"]
    fn a_secret_service_is_available() {
        assert_eq!(availability(), Ok(()));
    }
}
