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
//! **An item holds 2560 bytes** (`CRED_MAX_CREDENTIAL_BLOB_SIZE`), and a
//! longer blob is refused outright — where the Mac's keychain takes a
//! secret of any length. A signed-in session with its lease is already
//! within sight of that line, so a secret is kept in PARTS: the pair's
//! own item holds the first 2560 bytes, and `service/account#2`, `#3`…
//! hold the rest, in order. A read joins the parts until one is missing;
//! a write lays the tail down before the head, so a head never points at
//! a tail that is not there yet, and then sweeps whatever a longer secret
//! left past the new end. A store interrupted mid-write reads as bytes
//! that do not parse, which every caller already treats as "nothing
//! stored" — never as someone else's secret.
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

/// `CRED_MAX_CREDENTIAL_BLOB_SIZE`: the most one item holds.
const PART_MAX: usize = 5 * 512;

/// The name Windows looks part `index` of a secret up by: the pair's own
/// `service/account` for the first — the name an older build wrote and
/// the one the reader sees in Credential Manager — then `#2`, `#3`….
fn part_target(service: &str, account: &str, index: usize) -> Vec<u16> {
    match index {
        0 => wide(&format!("{service}/{account}")),
        _ => wide(&format!("{service}/{account}#{}", index + 1)),
    }
}

/// A secret cut where the store stops. The empty secret is still one
/// part: "saved an empty key" is not "saved nothing".
fn parts(secret: &[u8]) -> Vec<&[u8]> {
    if secret.is_empty() {
        vec![secret]
    } else {
        secret.chunks(PART_MAX).collect()
    }
}

/// One item's bytes, or `None` when the store holds no item by that name.
fn read_part(target: &[u16]) -> Option<Vec<u8>> {
    let mut found: *mut CredentialW = std::ptr::null_mut();
    let ok = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut found) };
    if ok == 0 || found.is_null() {
        return None;
    }
    Some(unsafe {
        let size = (*found).blob_size as usize;
        let blob = (*found).blob;
        let bytes = if !blob.is_null() && size > 0 {
            std::slice::from_raw_parts(blob, size).to_vec()
        } else {
            Vec::new()
        };
        CredFree(found.cast());
        bytes
    })
}

/// Writes one item, replacing whatever held that name. `true` = taken.
fn write_part(target: &[u16], account: &str, part: &[u8]) -> bool {
    let mut target = target.to_vec();
    let mut user = wide(account);
    let mut blob = part.to_vec();
    let credential = CredentialW {
        flags: 0,
        kind: CRED_TYPE_GENERIC,
        target_name: target.as_mut_ptr(),
        comment: std::ptr::null_mut(),
        last_written: [0, 0],
        // a part is at most `PART_MAX`, far inside a u32
        blob_size: u32::try_from(blob.len()).unwrap_or(u32::MAX),
        blob: blob.as_mut_ptr(),
        persist: CRED_PERSIST_LOCAL_MACHINE,
        attribute_count: 0,
        attributes: std::ptr::null_mut(),
        target_alias: std::ptr::null_mut(),
        user_name: user.as_mut_ptr(),
    };
    unsafe { CredWriteW(&credential, 0) != 0 }
}

/// Removes one item. `true` = no item holds that name NOW — one that
/// was already gone is the only failure that is not one.
fn delete_part(target: &[u16]) -> bool {
    let deleted = unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) != 0 };
    deleted || read_part(target).is_none()
}

/// Deletes the parts from `first` on, stopping at the first that is not
/// there — the parts are contiguous, so nothing lies past a gap.
fn sweep(service: &str, account: &str, first: usize) -> bool {
    for index in first.. {
        let target = part_target(service, account, index);
        if read_part(&target).is_none() {
            return true;
        }
        if !delete_part(&target) {
            return false;
        }
    }
    true
}

/// The secret stored for this service and account, or `None` when the
/// pair carries none — which is the ordinary answer for a key the
/// reader has not entered yet.
///
/// A secret that is not valid UTF-8 also answers `None`: this door
/// carries the text a settings page types, and bytes that are not text
/// were not written through it.
pub fn read(service: &str, account: &str) -> Option<String> {
    let mut secret = read_part(&part_target(service, account, 0))?;
    for index in 1.. {
        match read_part(&part_target(service, account, index)) {
            Some(part) => secret.extend(part),
            None => break,
        }
    }
    String::from_utf8(secret).ok()
}

/// Stores the secret for this service and account, replacing whatever
/// the pair held — `CredWriteW` overwrites by name, which is what a
/// settings page means by saving. `true` = the store took all of it.
pub fn write(service: &str, account: &str, secret: &str) -> bool {
    let parts = parts(secret.as_bytes());
    // the tail before the head: the head never names a tail that is not
    // there yet
    let written = parts
        .iter()
        .enumerate()
        .rev()
        .all(|(index, part)| write_part(&part_target(service, account, index), account, part));
    written && sweep(service, account, parts.len())
}

/// Removes the secret for this service and account, every part of it.
/// `true` = the pair carries none NOW, which a pair that never carried
/// one already satisfied — deleting is idempotent, the way a settings
/// page needs.
pub fn delete(service: &str, account: &str) -> bool {
    delete_part(&part_target(service, account, 0)) && sweep(service, account, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The store takes 2560 bytes an item, so a secret is cut at exactly
    /// that — and the cuts rebuild it. The empty secret is still one item:
    /// "saved an empty key" is not "saved nothing".
    #[test]
    fn a_secret_is_cut_where_the_store_stops_and_the_cuts_rebuild_it() {
        for length in [0, 1, PART_MAX, PART_MAX + 1, 3 * PART_MAX - 7] {
            let secret = vec![b'k'; length];
            let cut = parts(&secret);
            assert!(cut.iter().all(|part| part.len() <= PART_MAX), "{length}");
            assert_eq!(cut.len(), length.div_ceil(PART_MAX).max(1), "{length}");
            assert_eq!(cut.concat(), secret, "{length}");
        }
    }

    /// The first part keeps the pair's own name — what an older build wrote
    /// and what the reader sees in Credential Manager — and the rest follow
    /// it as `#2`, `#3`.
    #[test]
    fn the_parts_after_the_first_are_named_after_it() {
        assert_eq!(part_target("svc", "acct", 0), wide("svc/acct"));
        assert_eq!(part_target("svc", "acct", 1), wide("svc/acct#2"));
        assert_eq!(part_target("svc", "acct", 2), wide("svc/acct#3"));
    }

    /// The limit is the platform's, measured rather than assumed: an item
    /// takes exactly `PART_MAX` bytes and refuses one more — which is what
    /// a single-item `write` did to every secret past that line.
    #[test]
    fn the_store_refuses_one_byte_past_an_item() {
        let service = format!("bunny-ui-test-{}-limit", std::process::id());
        let target = part_target(&service, "item", 0);
        let taken = write_part(&target, "item", &[b'k'; PART_MAX]);
        let refused = !write_part(&target, "item", &[b'k'; PART_MAX + 1]);
        delete_part(&target);
        assert!(taken && refused, "taken: {taken}, refused: {refused}");
    }

    /// Against the real Credential Manager: a signed-in session is longer
    /// than one item holds, and it must come back whole; a shorter one
    /// written over it must not drag the old tail back in; and a delete
    /// leaves no part behind. The service name is this run's own, and the
    /// guard deletes it however the test ends.
    #[test]
    fn a_long_secret_round_trips_and_a_shorter_one_leaves_no_tail() {
        struct Forget(String);
        impl Drop for Forget {
            fn drop(&mut self) {
                delete(&self.0, "session");
            }
        }
        let service = format!("bunny-ui-test-{}-parts", std::process::id());
        let _forget = Forget(service.clone());

        let long = "{\"refresh\":\"0123456789abcdef\"}".repeat(250);
        assert!(long.len() > 2 * PART_MAX, "three parts at least");
        assert!(
            write(&service, "session", &long),
            "the store takes a long secret"
        );
        assert_eq!(read(&service, "session").as_deref(), Some(long.as_str()));

        assert!(write(&service, "session", "short"));
        assert_eq!(read(&service, "session").as_deref(), Some("short"));
        assert!(
            read_part(&part_target(&service, "session", 1)).is_none(),
            "the old tail is swept"
        );

        assert!(delete(&service, "session"));
        assert_eq!(read(&service, "session"), None);
        assert!(read_part(&part_target(&service, "session", 0)).is_none());
    }
}
