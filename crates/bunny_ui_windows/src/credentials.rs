//! The system's own secret store, through the house FFI.
//!
//! A secret is not settings. An app writes its configuration to a file
//! the reader opens in a tab and commits to a repository, and a key
//! that reaches a paid service must never live there. Windows already
//! keeps a store for exactly this — the Credential Manager, guarded by
//! the reader's own login — and this module is the door to it.
//!
//! An item is named by a PAIR: the service it belongs to and the
//! account inside it. Windows looks a credential up by ONE name, so the
//! pair becomes `service/account` — the convention every store on this
//! platform uses, and the name the reader sees in Credential Manager.
//!
//! **These calls block.** They are the platform's own and they touch
//! the disk, so they belong on a thread, not in a body — see the
//! macOS twin for the shape.

use std::ffi::c_void;

use crate::ffi::wide;

/// `CRED_TYPE_GENERIC` — an app's own secret, not a domain login.
const CRED_TYPE_GENERIC: u32 = 1;
/// `CRED_PERSIST_LOCAL_MACHINE` — it survives a logout, like the login
/// keychain does on the Mac.
const CRED_PERSIST_LOCAL_MACHINE: u32 = 2;

/// `CREDENTIALW`. The layout is the platform's; `repr(C)` lays the
/// padding out the same way the header does.
#[repr(C)]
struct CredentialW {
    flags: u32,
    kind: u32,
    target_name: *mut u16,
    comment: *mut u16,
    /// `FILETIME` — two DWORDs, never read here.
    last_written: [u32; 2],
    blob_size: u32,
    blob: *mut u8,
    persist: u32,
    attribute_count: u32,
    attributes: *mut c_void,
    target_alias: *mut u16,
    user_name: *mut u16,
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CredReadW(
        target: *const u16,
        kind: u32,
        flags: u32,
        credential: *mut *mut CredentialW,
    ) -> i32;
    fn CredWriteW(credential: *const CredentialW, flags: u32) -> i32;
    fn CredDeleteW(target: *const u16, kind: u32, flags: u32) -> i32;
    fn CredFree(buffer: *mut c_void);
}

/// The one name Windows looks a credential up by.
fn target_of(service: &str, account: &str) -> Vec<u16> {
    wide(&format!("{service}/{account}"))
}

/// The secret stored for this service and account, or `None` when the
/// pair carries none — which is the ordinary answer for a key the
/// reader has not entered yet.
///
/// A secret that is not valid UTF-8 also answers `None`: this door
/// carries the text a settings page types, and bytes that are not text
/// were not written through it.
pub fn read(service: &str, account: &str) -> Option<String> {
    let target = target_of(service, account);
    let mut found: *mut CredentialW = std::ptr::null_mut();
    let ok = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut found) };
    if ok == 0 || found.is_null() {
        return None;
    }
    let secret = unsafe {
        let size = (*found).blob_size as usize;
        let blob = (*found).blob;
        let text = (!blob.is_null() && size > 0)
            .then(|| std::slice::from_raw_parts(blob, size).to_vec())
            .unwrap_or_default();
        CredFree(found.cast());
        text
    };
    String::from_utf8(secret).ok()
}

/// Stores the secret for this service and account, replacing whatever
/// the pair held — `CredWriteW` overwrites by name, which is what a
/// settings page means by saving. `true` = the store took it.
pub fn write(service: &str, account: &str, secret: &str) -> bool {
    let mut target = target_of(service, account);
    let mut user = wide(account);
    let mut blob = secret.as_bytes().to_vec();
    let credential = CredentialW {
        flags: 0,
        kind: CRED_TYPE_GENERIC,
        target_name: target.as_mut_ptr(),
        comment: std::ptr::null_mut(),
        last_written: [0, 0],
        blob_size: blob.len() as u32,
        blob: blob.as_mut_ptr(),
        persist: CRED_PERSIST_LOCAL_MACHINE,
        attribute_count: 0,
        attributes: std::ptr::null_mut(),
        target_alias: std::ptr::null_mut(),
        user_name: user.as_mut_ptr(),
    };
    unsafe { CredWriteW(&credential, 0) != 0 }
}

/// Removes the secret for this service and account. `true` = the pair
/// carries none NOW, which a pair that never carried one already
/// satisfied — deleting is idempotent, the way a settings page needs.
pub fn delete(service: &str, account: &str) -> bool {
    let target = target_of(service, account);
    if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) != 0 } {
        return true;
    }
    // it was already gone: the only failure that is not one
    read(service, account).is_none()
}
