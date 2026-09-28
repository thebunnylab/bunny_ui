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
//! own item (the HEAD) holds the first 2560 bytes, and items named after
//! it hold the rest, in order.
//!
//! Three rules keep the parts honest:
//!
//! - **A part's name is reserved.** It is the pair's name, the C0 unit
//!   separator, and the part's number. A pair whose own name holds that
//!   separator is refused, so no pair's item can ever share a name with
//!   another pair's part.
//! - **The head is sealed.** A credential attribute on the head says how
//!   many parts the secret was cut into and carries a digest of all of
//!   its bytes. The tails are written before the head, so the head is
//!   the commit, and a read joins exactly the parts the seal names and
//!   checks the digest. A write interrupted halfway, or two processes
//!   writing the pair at once, therefore reads as NOTHING stored — never
//!   as a secret spliced from two writes, which for a plain-text key
//!   would be a wrong key that looks like a right one.
//! - **The sweep finds every part.** After a write, every part past the
//!   new end is deleted — enumerated by the prefix their names share, so
//!   a gap a failed write left cannot hide the parts beyond it. After a
//!   write the sweep is housekeeping: the seal already makes a leftover
//!   unreachable, so a write that saved says so even when its sweep could
//!   not finish. A DELETE promises the secret's bytes are gone, so there a
//!   part the sweep could not remove is a failure.
//!
//! A head with no seal is an item from before the parts, and it reads
//! exactly as it always did: its own bytes, nothing joined.
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
#[cfg(not(test))]
const PERSIST: u32 = 2;
/// `CRED_PERSIST_SESSION` for the tests: they write the developer's real
/// store, and a run killed before its cleanup must leave nothing behind
/// past the next logoff — and nothing on the disk at all.
#[cfg(test)]
const PERSIST: u32 = 1;
/// `ERROR_NOT_FOUND` — what an enumeration that matched nothing reports.
const ERROR_NOT_FOUND: i32 = 1168;

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
    attributes: *mut CredentialAttributeW,
    target_alias: *mut u16,
    user_name: *mut u16,
}

/// `CREDENTIAL_ATTRIBUTEW`: a keyword and up to 256 bytes of value — the
/// platform's own place for what an item says about itself.
#[repr(C)]
struct CredentialAttributeW {
    keyword: *mut u16,
    flags: u32,
    value_size: u32,
    value: *mut u8,
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
    fn CredEnumerateW(
        filter: *const u16,
        flags: u32,
        count: *mut u32,
        credentials: *mut *mut *mut CredentialW,
    ) -> i32;
    fn CredFree(buffer: *mut c_void);
}

/// `CRED_MAX_CREDENTIAL_BLOB_SIZE`: the most one item holds.
const PART_MAX: usize = 5 * 512;

/// What stands between the pair's name and a part's number: the C0 unit
/// separator, which no settings page types. Reserved — see [`reserved`].
const PART_MARK: char = '\u{1f}';

/// The keyword a head's [`Seal`] rides under.
const SEAL_KEYWORD: &str = "bunny_ui/seal";

/// The one name Windows looks the pair up by.
fn pair_name(service: &str, account: &str) -> String {
    format!("{service}/{account}")
}

/// The name part `index` of a secret is kept under: the pair's own name
/// for the head — the name an older build wrote, and the one the reader
/// sees in Credential Manager — then the pair's name, [`PART_MARK`] and
/// the part's number, from 2.
fn part_target(service: &str, account: &str, index: usize) -> Vec<u16> {
    match index {
        0 => wide(&pair_name(service, account)),
        _ => wide(&format!(
            "{}{PART_MARK}{}",
            pair_name(service, account),
            index + 1
        )),
    }
}

/// A pair whose name holds [`PART_MARK`] could name another pair's part,
/// so this door refuses it: nothing is written under it, and nothing is
/// read from it.
fn reserved(service: &str, account: &str) -> bool {
    service.contains(PART_MARK) || account.contains(PART_MARK)
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

/// What a head certifies about the whole secret: how many parts it was
/// cut into, and a digest of all of its bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Seal {
    parts: u32,
    digest: u64,
}

impl Seal {
    /// The seal of a whole secret.
    fn of(secret: &[u8]) -> Self {
        Self {
            parts: u32::try_from(parts(secret).len()).unwrap_or(u32::MAX),
            digest: fnv1a(secret),
        }
    }

    /// Twelve bytes: the part count, then the digest, little-endian.
    fn to_bytes(self) -> [u8; 12] {
        let mut bytes = [0; 12];
        bytes[..4].copy_from_slice(&self.parts.to_le_bytes());
        bytes[4..].copy_from_slice(&self.digest.to_le_bytes());
        bytes
    }

    /// The seal in an attribute's value, or `None` when the value is not
    /// one — which the read treats as a torn head.
    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let bytes: &[u8; 12] = bytes.try_into().ok()?;
        let (parts, digest) = bytes.split_at(4);
        Some(Self {
            parts: u32::from_le_bytes(parts.try_into().ok()?),
            digest: u64::from_le_bytes(digest.try_into().ok()?),
        })
    }
}

/// FNV-1a, 64-bit: a digest fixed by its definition rather than by a
/// toolchain — `DefaultHasher` may change between Rust releases, and a
/// seal one build wrote must verify under the next. It guards against a
/// torn write, not an attacker: whoever can write the store can write a
/// seal too.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// A head's attribute, as the read found it.
enum Attribute {
    /// No seal: an item from before the parts, or a tail.
    Absent,
    /// A seal that parsed.
    Sealed(Seal),
    /// A seal keyword whose value is not a seal.
    Torn,
}

/// One item as the store holds it: its bytes and what its attribute says.
struct Item {
    bytes: Vec<u8>,
    attribute: Attribute,
}

/// The characters of a nul-terminated wide string the store handed back.
///
/// # Safety
///
/// `text` points at a nul-terminated UTF-16 string that outlives the call.
unsafe fn wide_text(text: *const u16) -> Vec<u16> {
    let mut length = 0;
    // SAFETY: the caller promises a terminator before the end of the
    // allocation, and each read stops at it.
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    unsafe { std::slice::from_raw_parts(text, length) }.to_vec()
}

/// One item, or `None` when the store holds no item by that name.
fn read_item(target: &[u16]) -> Option<Item> {
    let mut found: *mut CredentialW = std::ptr::null_mut();
    let ok = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut found) };
    if ok == 0 || found.is_null() {
        return None;
    }
    let keyword: Vec<u16> = SEAL_KEYWORD.encode_utf16().collect();
    let item = unsafe {
        let credential = &*found;
        let bytes = if !credential.blob.is_null() && credential.blob_size > 0 {
            std::slice::from_raw_parts(credential.blob, credential.blob_size as usize).to_vec()
        } else {
            Vec::new()
        };
        let mut attribute = Attribute::Absent;
        if !credential.attributes.is_null() {
            let all = std::slice::from_raw_parts(
                credential.attributes,
                credential.attribute_count as usize,
            );
            for entry in all {
                if entry.keyword.is_null() || wide_text(entry.keyword) != keyword {
                    continue;
                }
                let value = if entry.value.is_null() {
                    &[][..]
                } else {
                    std::slice::from_raw_parts(entry.value, entry.value_size as usize)
                };
                attribute = Seal::from_bytes(value).map_or(Attribute::Torn, Attribute::Sealed);
            }
        }
        Item { bytes, attribute }
    };
    unsafe { CredFree(found.cast()) };
    Some(item)
}

/// Writes one item, replacing whatever held that name, sealed when it is
/// a head. `true` = taken.
fn write_item(target: &[u16], account: &str, part: &[u8], seal: Option<Seal>) -> bool {
    let mut target = target.to_vec();
    let mut user = wide(account);
    let mut blob = part.to_vec();
    let mut keyword = wide(SEAL_KEYWORD);
    let mut value = seal.map(Seal::to_bytes);
    let mut attribute = value.as_mut().map(|value| CredentialAttributeW {
        keyword: keyword.as_mut_ptr(),
        flags: 0,
        value_size: u32::try_from(value.len()).unwrap_or(u32::MAX),
        value: value.as_mut_ptr(),
    });
    let credential = CredentialW {
        flags: 0,
        kind: CRED_TYPE_GENERIC,
        target_name: target.as_mut_ptr(),
        comment: std::ptr::null_mut(),
        last_written: [0, 0],
        // a part is at most `PART_MAX`, far inside a u32
        blob_size: u32::try_from(blob.len()).unwrap_or(u32::MAX),
        blob: blob.as_mut_ptr(),
        persist: PERSIST,
        attribute_count: u32::from(attribute.is_some()),
        attributes: attribute
            .as_mut()
            .map_or(std::ptr::null_mut(), std::ptr::from_mut),
        target_alias: std::ptr::null_mut(),
        user_name: user.as_mut_ptr(),
    };
    unsafe { CredWriteW(&credential, 0) != 0 }
}

/// Removes one item. `true` = no item holds that name NOW — one that
/// was already gone is the only failure that is not one.
fn delete_item(target: &[u16]) -> bool {
    let deleted = unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) != 0 };
    deleted || read_item(target).is_none()
}

/// Deletes every part of the pair from index `keep` on. The parts are
/// found by the prefix their names share rather than walked from the
/// head, so a gap a failed write left cannot hide the parts beyond it.
fn sweep(service: &str, account: &str, keep: usize) -> bool {
    let filter = wide(&format!("{}{PART_MARK}*", pair_name(service, account)));
    let mut count = 0_u32;
    let mut found: *mut *mut CredentialW = std::ptr::null_mut();
    let ok = unsafe { CredEnumerateW(filter.as_ptr(), 0, &mut count, &mut found) };
    if ok == 0 || found.is_null() {
        // matching nothing is the one failure that means "already swept"
        return std::io::Error::last_os_error().raw_os_error() == Some(ERROR_NOT_FOUND);
    }
    let names: Vec<Vec<u16>> = unsafe {
        std::slice::from_raw_parts(found, count as usize)
            .iter()
            .filter(|credential| !credential.is_null() && !(***credential).target_name.is_null())
            .map(|credential| wide_text((**credential).target_name))
            .collect()
    };
    unsafe { CredFree(found.cast()) };
    names
        .into_iter()
        .filter(|name| {
            // the number after the LAST mark: the pair's own name holds
            // none (`reserved`), so that mark is the part's
            String::from_utf16(name)
                .ok()
                .and_then(|name| {
                    name.rsplit_once(PART_MARK)
                        .and_then(|(_, number)| number.parse::<usize>().ok())
                })
                .is_some_and(|number| number > keep)
        })
        .all(|mut name| {
            name.push(0);
            delete_item(&name)
        })
}

/// The secret stored for this service and account, or `None` when the
/// pair carries none — which is the ordinary answer for a key the
/// reader has not entered yet.
///
/// A secret whose parts do not match its head's seal — a write that was
/// interrupted, or two that crossed — also answers `None`, and so does a
/// secret that is not valid UTF-8: this door carries the text a settings
/// page types, and bytes that are not that text were not written through
/// it whole.
pub fn read(service: &str, account: &str) -> Option<String> {
    if reserved(service, account) {
        return None;
    }
    let head = read_item(&part_target(service, account, 0))?;
    let secret = match head.attribute {
        // an item from before the parts: exactly what it holds
        Attribute::Absent => head.bytes,
        Attribute::Torn => return None,
        Attribute::Sealed(seal) => {
            let mut secret = head.bytes;
            for index in 1..seal.parts as usize {
                secret.extend(read_item(&part_target(service, account, index))?.bytes);
            }
            (Seal::of(&secret) == seal).then_some(secret)?
        }
    };
    String::from_utf8(secret).ok()
}

/// Stores the secret for this service and account, replacing whatever
/// the pair held — `CredWriteW` overwrites by name, which is what a
/// settings page means by saving. `true` = the secret is saved: every
/// tail, then the head that seals them.
///
/// The sweep of an earlier, longer secret's parts runs after, and its
/// outcome does not change the answer: the head's seal already names
/// exactly this secret's parts, so a part the sweep could not reach is
/// unreachable to every read, and the next write or delete sweeps it
/// again. Answering `false` there would report a saved secret as lost.
///
/// A pair whose name holds the reserved part mark is refused (`false`).
pub fn write(service: &str, account: &str, secret: &str) -> bool {
    if reserved(service, account) {
        return false;
    }
    let bytes = secret.as_bytes();
    let parts = parts(bytes);
    let Some((head, tails)) = parts.split_first() else {
        return false;
    };
    // the tails before the head: the head is the commit, and it never
    // seals a tail that is not there yet
    let tails_written = tails.iter().enumerate().rev().all(|(offset, tail)| {
        write_item(
            &part_target(service, account, offset + 1),
            account,
            tail,
            None,
        )
    });
    let saved = tails_written
        && write_item(
            &part_target(service, account, 0),
            account,
            head,
            Some(Seal::of(bytes)),
        );
    if saved {
        // housekeeping, not the save: see above
        sweep(service, account, parts.len());
    }
    saved
}

/// Removes the secret for this service and account, every part of it.
/// `true` = the pair carries none NOW, which a pair that never carried
/// one already satisfied — deleting is idempotent, the way a settings
/// page needs. A reserved pair carries none by construction.
///
/// Unlike a write's, this sweep decides the answer: a part it could not
/// remove still holds some of the secret's bytes, and a sign-out that
/// leaves them behind has not signed out.
pub fn delete(service: &str, account: &str) -> bool {
    if reserved(service, account) {
        return true;
    }
    delete_item(&part_target(service, account, 0)) && sweep(service, account, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deletes `service/account` however the test ends.
    struct Forget(String, &'static str);

    impl Drop for Forget {
        fn drop(&mut self) {
            delete(&self.0, self.1);
        }
    }

    /// This run's own service name for one test.
    fn service(test: &str) -> String {
        format!("bunny-ui-test-{}-{test}", std::process::id())
    }

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

    /// The head keeps the pair's own name — what an older build wrote and
    /// what the reader sees in Credential Manager — and the parts after it
    /// carry the reserved mark and their number.
    #[test]
    fn the_parts_after_the_head_are_named_with_the_reserved_mark() {
        assert_eq!(part_target("svc", "acct", 0), wide("svc/acct"));
        assert_eq!(part_target("svc", "acct", 1), wide("svc/acct\u{1f}2"));
        assert_eq!(part_target("svc", "acct", 2), wide("svc/acct\u{1f}3"));
        assert!(reserved("svc", "acct\u{1f}2"));
        assert!(reserved("svc\u{1f}", "acct"));
        assert!(
            !reserved("svc", "acct#2"),
            "an ordinary `#` is an ordinary name"
        );
    }

    /// The digest is FNV-1a by its published vectors — fixed by definition,
    /// so a seal one build wrote verifies under the next — and a seal
    /// survives its twelve bytes while anything else is not one.
    #[test]
    fn a_seal_is_fixed_by_definition_and_survives_its_bytes() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        let seal = Seal::of(&[b'k'; PART_MAX + 1]);
        assert_eq!(seal.parts, 2);
        assert_eq!(Seal::from_bytes(&seal.to_bytes()), Some(seal));
        assert_eq!(Seal::from_bytes(&seal.to_bytes()[..11]), None);
    }

    /// The limit is the platform's, measured rather than assumed: an item
    /// takes exactly `PART_MAX` bytes and refuses one more — which is what
    /// a single-item write did to every secret past that line.
    #[test]
    fn the_store_refuses_one_byte_past_an_item() {
        let target = part_target(&service("limit"), "item", 0);
        let taken = write_item(&target, "item", &[b'k'; PART_MAX], None);
        let refused = !write_item(&target, "item", &[b'k'; PART_MAX + 1], None);
        delete_item(&target);
        assert!(taken && refused, "taken: {taken}, refused: {refused}");
    }

    /// Against the real Credential Manager: a signed-in session is longer
    /// than one item holds, and it must come back whole; a shorter one
    /// written over it must not drag the old tail back in; and a delete
    /// leaves no part behind.
    #[test]
    fn a_long_secret_round_trips_and_a_shorter_one_leaves_no_tail() {
        let service = service("parts");
        let _forget = Forget(service.clone(), "session");

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
            read_item(&part_target(&service, "session", 1)).is_none(),
            "the old tail is swept"
        );

        assert!(delete(&service, "session"));
        assert_eq!(read(&service, "session"), None);
        assert!(read_item(&part_target(&service, "session", 0)).is_none());
    }

    /// A secret whose parts are not the ones its head sealed reads as
    /// NOTHING — a tail from another write, or a missing one — never as a
    /// spliced secret that looks like a real one.
    #[test]
    fn a_torn_secret_reads_as_nothing_stored() {
        let service = service("torn");
        let _forget = Forget(service.clone(), "key");
        let secret = "sk-".repeat(2 * PART_MAX);
        assert!(write(&service, "key", &secret));

        let middle = part_target(&service, "key", 1);
        assert!(write_item(&middle, "key", &[b'x'; PART_MAX], None));
        assert_eq!(read(&service, "key"), None, "a tail from another write");

        assert!(delete_item(&middle));
        assert_eq!(read(&service, "key"), None, "a missing tail");
    }

    /// An item from before the parts has no seal, and it reads exactly as
    /// it always did — its own bytes, with nothing joined, even when a
    /// stray part sits beside it.
    #[test]
    fn an_item_from_before_the_parts_reads_as_it_always_did() {
        let service = service("legacy");
        let _forget = Forget(service.clone(), "key");
        assert!(write_item(
            &part_target(&service, "key", 0),
            "key",
            b"legacy",
            None
        ));
        assert!(write_item(
            &part_target(&service, "key", 1),
            "key",
            b"stray",
            None
        ));
        assert_eq!(read(&service, "key").as_deref(), Some("legacy"));
    }

    /// A pair whose name holds the reserved mark is refused whole: it
    /// could otherwise name another pair's part.
    #[test]
    fn a_pair_holding_the_reserved_mark_is_refused() {
        let service = service("reserved");
        assert!(!write(&service, "a\u{1f}2", "secret"));
        assert_eq!(read(&service, "a\u{1f}2"), None);
        assert!(delete(&service, "a\u{1f}2"));
    }

    /// A gap a failed write left does not hide the parts past it: the
    /// sweep finds every part by the prefix their names share.
    #[test]
    fn a_gap_does_not_hide_the_parts_past_it_from_the_sweep() {
        let service = service("gap");
        let _forget = Forget(service.clone(), "session");
        assert!(write(&service, "session", &"g".repeat(3 * PART_MAX + 1)));
        assert!(delete_item(&part_target(&service, "session", 1)));

        assert!(write(&service, "session", "short"));
        for index in 1..=3 {
            assert!(
                read_item(&part_target(&service, "session", index)).is_none(),
                "part {index} is swept"
            );
        }
    }
}
