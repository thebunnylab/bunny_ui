//! The system's own secret store, through the house FFI.
//!
//! A secret is not settings. An app writes its configuration to a file
//! the reader opens in a tab and commits to a repository, and a key
//! that reaches a paid service must never live there. The platform
//! already keeps a store for exactly this, guarded by the reader's own
//! login, and this module is the door to the Mac's.
//!
//! An item is named by a PAIR: the service it belongs to and the
//! account inside it — the same pair Keychain Access shows in its two
//! columns. Writing over a pair that already has a secret replaces it,
//! which is what a settings page means by saving.
//!
//! **These calls block.** They are the platform's own, they touch the
//! disk, and the system may ask the reader to allow the access — so
//! they belong on a thread, not in a body:
//!
//! ```ignore
//! let key = State::new(String::new());
//! view.task(move || {
//!     let found = credentials::read("api.example.com", "default");
//!     key.set(found.unwrap_or_default());
//! })
//! ```

use std::ffi::c_void;

use crate::ffi::CFRelease;

type CFStringRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFDataRef = *const c_void;
type CFTypeRef = *const c_void;
type OSStatus = i32;

/// `errSecSuccess`.
const SEC_SUCCESS: OSStatus = 0;
/// `errSecItemNotFound` — the pair carries no secret, which is an
/// answer and not a failure.
const SEC_ITEM_NOT_FOUND: OSStatus = -25300;
/// `errSecDuplicateItem` — the pair already has one, so the write is
/// an update.
const SEC_DUPLICATE_ITEM: OSStatus = -25299;

const KCF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithBytes(
        allocator: *const c_void,
        bytes: *const u8,
        length: isize,
        encoding: u32,
        external: u8,
    ) -> CFStringRef;
    fn CFDataCreate(allocator: *const c_void, bytes: *const u8, length: isize) -> CFDataRef;
    fn CFDataGetBytePtr(data: CFDataRef) -> *const u8;
    fn CFDataGetLength(data: CFDataRef) -> isize;
    fn CFDictionaryCreate(
        allocator: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        count: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> CFDictionaryRef;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    static kCFBooleanTrue: CFTypeRef;
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecItemCopyMatching(query: CFDictionaryRef, result: *mut CFTypeRef) -> OSStatus;
    fn SecItemAdd(attributes: CFDictionaryRef, result: *mut CFTypeRef) -> OSStatus;
    fn SecItemUpdate(query: CFDictionaryRef, changes: CFDictionaryRef) -> OSStatus;
    fn SecItemDelete(query: CFDictionaryRef) -> OSStatus;
    static kSecClass: CFStringRef;
    static kSecClassGenericPassword: CFStringRef;
    static kSecAttrService: CFStringRef;
    static kSecAttrAccount: CFStringRef;
    static kSecValueData: CFStringRef;
    static kSecReturnData: CFStringRef;
    static kSecMatchLimit: CFStringRef;
    static kSecMatchLimitOne: CFStringRef;
}

/// A retained CoreFoundation value — released on drop, so no road out
/// of these functions can leak one.
struct Owned(*const c_void);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

/// A CFString the caller owns. Empty text is a real string, not null.
fn cf_string(text: &str) -> Option<Owned> {
    let string = unsafe {
        CFStringCreateWithBytes(
            std::ptr::null(),
            text.as_ptr(),
            text.len() as isize,
            KCF_STRING_ENCODING_UTF8,
            0,
        )
    };
    (!string.is_null()).then_some(Owned(string))
}

/// The dictionary every call starts from: a generic password named by
/// the service/account pair. The extra entries are appended by the
/// caller, which is why this hands back the parts instead of the
/// dictionary itself.
fn pair(service: &str, account: &str) -> Option<(Owned, Owned)> {
    Some((cf_string(service)?, cf_string(account)?))
}

/// Builds a CFDictionary from parts the caller keeps alive. The
/// dictionary retains what it holds, so the parts may fall after it.
fn dictionary(entries: &[(CFStringRef, *const c_void)]) -> Option<Owned> {
    let keys: Vec<*const c_void> = entries.iter().map(|(key, _)| *key).collect();
    let values: Vec<*const c_void> = entries.iter().map(|(_, value)| *value).collect();
    let dictionary = unsafe {
        CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            entries.len() as isize,
            &raw const kCFTypeDictionaryKeyCallBacks,
            &raw const kCFTypeDictionaryValueCallBacks,
        )
    };
    (!dictionary.is_null()).then_some(Owned(dictionary))
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
    let query = dictionary(&[
        (unsafe { kSecClass }, unsafe { kSecClassGenericPassword }),
        (unsafe { kSecAttrService }, service.0),
        (unsafe { kSecAttrAccount }, account.0),
        (unsafe { kSecReturnData }, unsafe { kCFBooleanTrue }),
        (unsafe { kSecMatchLimit }, unsafe { kSecMatchLimitOne }),
    ])?;
    let mut found: CFTypeRef = std::ptr::null();
    let status = unsafe { SecItemCopyMatching(query.0, &mut found) };
    if status != SEC_SUCCESS || found.is_null() {
        return None;
    }
    let data = Owned(found);
    unsafe {
        let bytes = CFDataGetBytePtr(data.0);
        let length = CFDataGetLength(data.0);
        if bytes.is_null() || length < 0 {
            return None;
        }
        let slice = std::slice::from_raw_parts(bytes, length as usize);
        String::from_utf8(slice.to_vec()).ok()
    }
}

/// Stores the secret for this service and account, replacing whatever
/// the pair held. `true` = the store took it.
pub fn write(service: &str, account: &str, secret: &str) -> bool {
    let Some((service, account)) = pair(service, account) else {
        return false;
    };
    let bytes = unsafe {
        CFDataCreate(std::ptr::null(), secret.as_ptr(), secret.len() as isize)
    };
    if bytes.is_null() {
        return false;
    }
    let bytes = Owned(bytes);
    let Some(attributes) = dictionary(&[
        (unsafe { kSecClass }, unsafe { kSecClassGenericPassword }),
        (unsafe { kSecAttrService }, service.0),
        (unsafe { kSecAttrAccount }, account.0),
        (unsafe { kSecValueData }, bytes.0),
    ]) else {
        return false;
    };
    match unsafe { SecItemAdd(attributes.0, std::ptr::null_mut()) } {
        SEC_SUCCESS => true,
        // the pair already carries a secret: the add is an UPDATE, and
        // the query that finds it must not carry the new value
        SEC_DUPLICATE_ITEM => {
            let Some(query) = dictionary(&[
                (unsafe { kSecClass }, unsafe { kSecClassGenericPassword }),
                (unsafe { kSecAttrService }, service.0),
                (unsafe { kSecAttrAccount }, account.0),
            ]) else {
                return false;
            };
            let Some(changes) = dictionary(&[(unsafe { kSecValueData }, bytes.0)]) else {
                return false;
            };
            unsafe { SecItemUpdate(query.0, changes.0) == SEC_SUCCESS }
        }
        _ => false,
    }
}

/// Removes the secret for this service and account. `true` = the pair
/// carries none NOW, which a pair that never carried one already
/// satisfied — deleting is idempotent, the way a settings page needs.
pub fn delete(service: &str, account: &str) -> bool {
    let Some((service, account)) = pair(service, account) else {
        return false;
    };
    let Some(query) = dictionary(&[
        (unsafe { kSecClass }, unsafe { kSecClassGenericPassword }),
        (unsafe { kSecAttrService }, service.0),
        (unsafe { kSecAttrAccount }, account.0),
    ]) else {
        return false;
    };
    matches!(
        unsafe { SecItemDelete(query.0) },
        SEC_SUCCESS | SEC_ITEM_NOT_FOUND
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_goes_in_comes_back_and_can_be_taken_out() {
        // a service of our own, so the test never touches a real one
        const SERVICE: &str = "com.thebunnylab.bunny_ui.test";
        const ACCOUNT: &str = "roundtrip";

        // whatever a previous run left behind
        delete(SERVICE, ACCOUNT);
        assert_eq!(read(SERVICE, ACCOUNT), None, "an empty pair reads as nothing");

        assert!(write(SERVICE, ACCOUNT, "first"), "the store took it");
        assert_eq!(read(SERVICE, ACCOUNT).as_deref(), Some("first"));

        // writing over the pair REPLACES it — that is what saving means
        assert!(write(SERVICE, ACCOUNT, "second"));
        assert_eq!(read(SERVICE, ACCOUNT).as_deref(), Some("second"));

        assert!(delete(SERVICE, ACCOUNT));
        assert_eq!(read(SERVICE, ACCOUNT), None, "and it is gone");
        assert!(delete(SERVICE, ACCOUNT), "deleting nothing is not a failure");
    }
}
