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

use std::ffi::{CStr, CString, c_char, c_int, c_void};

/// `SECRET_SCHEMA_NONE` — the schema name is part of the match, so an
/// item written through this door is found through this door and not
/// confused with one some other tool wrote under the same attributes.
const SECRET_SCHEMA_NONE: c_int = 0;
/// `SECRET_SCHEMA_ATTRIBUTE_STRING`.
const ATTRIBUTE_STRING: c_int = 0;
/// libsecret's own count — the array is fixed and the tail is zeroed.
const SCHEMA_ATTRIBUTE_SLOTS: usize = 32;

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
        error: *mut *mut c_void,
        ...
    ) -> *mut c_char;
    fn secret_password_store_sync(
        schema: *const SecretSchema,
        collection: *const c_char,
        label: *const c_char,
        password: *const c_char,
        cancellable: *mut c_void,
        error: *mut *mut c_void,
        ...
    ) -> c_int;
    fn secret_password_clear_sync(
        schema: *const SecretSchema,
        cancellable: *mut c_void,
        error: *mut *mut c_void,
        ...
    ) -> c_int;
    fn secret_password_free(password: *mut c_char);
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
