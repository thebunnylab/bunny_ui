//! The system's own secret store, through JNI.
//!
//! A secret is not settings. An app writes its configuration to a file
//! the reader opens in a tab and commits to a repository, and a token
//! that signs a person in must never live there. The other four shells
//! open the platform's own store for exactly this. Android has none an
//! app can open by that name — and it has the two halves one is made
//! of.
//!
//! **The key belongs to the platform, and the app never holds it.**
//! `AndroidKeyStore` mints an AES key under an alias, keeps it in the
//! secure element where the phone has one, and lends the app a handle
//! that can encrypt and decrypt but cannot be read. The ciphertext then
//! goes anywhere, because ciphertext is not a secret: here it goes to
//! `SharedPreferences`, a file under the app's own uid that no other app
//! can open. The pair is what `EncryptedSharedPreferences` is under its
//! own cover, minus the AndroidX dependency this crate does not take.
//!
//! An item is named by a PAIR: the service it belongs to and the account
//! inside it, the same pair every other shell's store uses. A write over
//! a pair that already has a secret replaces it.
//!
//! **These calls block**, like every other shell's: they reach a
//! keystore, run a cipher and touch the disk. They also need the UI
//! thread, because the JNI env is that thread's. A call from anywhere
//! else answers "nothing" rather than reaching for an env that is not
//! there.
//!
//! ## Production gotchas
//!
//! **A refusal is never a plaintext fallback.** If the keystore will not
//! mint a key, or a cipher will not run, [`write`] answers `false` and
//! nothing is stored. The alternative — a token in a file that reads
//! like a keychain — is the failure shape that looks like success, and
//! the caller can say the truth only if it is told the truth.
//!
//! **The key dies with the app's data, and that is the contract.**
//! Clearing the app's storage, or uninstalling it, erases the alias.
//! Every stored secret is then unreadable, [`read`] answers `None`, and
//! the app asks the person to sign in again. A backup that restores the
//! preferences file to another install restores ciphertext nobody can
//! open, which is the point.

/// The preferences file, and the alias of the key that guards it. One
/// key for the whole store: a key for each pair would multiply the
/// keystore's work for no secret the others do not already have.
const STORE: &str = "bunny_ui.credentials";

/// `Context.MODE_PRIVATE` — the file belongs to this app's uid alone.
const MODE_PRIVATE: i32 = 0;

/// `Cipher.ENCRYPT_MODE` and `Cipher.DECRYPT_MODE`.
const ENCRYPT: i32 = 1;
const DECRYPT: i32 = 2;

/// The authentication tag's length in bits — GCM's own, and the value
/// the decrypting side must name for the tag to be checked at all.
const TAG_BITS: i32 = 128;

// MARK: - The door

/// Everything that reaches Java, which is everything a phone runs and
/// nothing the build machine can. The half below this — the slot a pair
/// names, the sealed form and its codec — is pure, so it compiles and is
/// tested everywhere.
#[cfg(target_os = "android")]
mod door {
    use std::ffi::CStr;

    use super::{ENCRYPT, DECRYPT, MODE_PRIVATE, STORE, TAG_BITS, base64, join, slot, split, unbase64};
    use crate::jni::{self, Env, Frame, JClass, JObject};

    // MARK: - The door

    /// Reads the secret filed under `(service, account)`.
    ///
    /// `None` when the pair has none, when the key is gone, and when the
    /// ciphertext does not open — a secret that fails its tag is a secret
    /// somebody changed, and this answers the same way it answers a pair
    /// nobody ever wrote.
    #[must_use]
    pub fn read(service: &str, account: &str) -> Option<String> {
        let env = Env::current()?;
        let _frame = Frame::new(env, 32)?;
        let stored = pref_get(env, &slot(service, account))?;
        let sealed = unbase64(&stored)?;
        let (iv, body) = split(&sealed)?;
        let key = key_of(env)?;
        let cipher = cipher_class(env)?;
        let spec = gcm_spec(env, iv)?;
        let handle = new_cipher(env, cipher)?;
        let init = env.method(
            cipher,
            c"init",
            c"(ILjava/security/Key;Ljava/security/spec/AlgorithmParameterSpec;)V",
        )?;
        env.call_void(handle, init, &[jni::int(DECRYPT), jni::object(key), jni::object(spec)])
            .then_some(())?;
        let plain = do_final(env, cipher, handle, body)?;
        String::from_utf8(plain).ok()
    }

    /// Files `secret` under `(service, account)`, replacing what was there.
    ///
    /// `false` means nothing was stored — see the module's gotchas: there is
    /// no path here that stores a secret unencrypted.
    pub fn write(service: &str, account: &str, secret: &str) -> bool {
        let Some(env) = Env::current() else { return false };
        let Some(_frame) = Frame::new(env, 32) else { return false };
        let sealed = || -> Option<String> {
            let key = key_of(env)?;
            let cipher = cipher_class(env)?;
            let handle = new_cipher(env, cipher)?;
            let init = env.method(cipher, c"init", c"(ILjava/security/Key;)V")?;
            env.call_void(handle, init, &[jni::int(ENCRYPT), jni::object(key)]).then_some(())?;
            // The IV is the cipher's own, drawn fresh for every write: a GCM
            // key that encrypts twice under one IV loses the key, not only
            // the message.
            let get_iv = env.method(cipher, c"getIV", c"()[B")?;
            let iv = env.byte_array(env.call_object(handle, get_iv, &[])?)?;
            let body = do_final(env, cipher, handle, secret.as_bytes())?;
            Some(base64(&join(&iv, &body)?))
        };
        let Some(sealed) = sealed() else { return false };
        pref_put(env, &slot(service, account), Some(&sealed))
    }

    /// Erases the secret filed under `(service, account)`.
    ///
    /// `true` when the pair is empty afterwards, which a pair that never had
    /// a secret already is.
    pub fn delete(service: &str, account: &str) -> bool {
        let Some(env) = Env::current() else { return false };
        let Some(_frame) = Frame::new(env, 16) else { return false };
        pref_put(env, &slot(service, account), None)
    }

    // MARK: - The two halves

    /// The preferences file, through the activity, which is a `Context`.
    fn prefs(env: Env) -> Option<JObject> {
        let context = env.class(c"android/content/Context")?;
        let get = env.method(
            context,
            c"getSharedPreferences",
            c"(Ljava/lang/String;I)Landroid/content/SharedPreferences;",
        )?;
        let name = env.string(STORE)?;
        env.call_object(env.activity(), get, &[jni::object(name), jni::int(MODE_PRIVATE)])
    }

    fn pref_get(env: Env, key: &str) -> Option<String> {
        let prefs = prefs(env)?;
        let class = env.class(c"android/content/SharedPreferences")?;
        let get = env.method(
            class,
            c"getString",
            c"(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
        )?;
        let name = env.string(key)?;
        let found = env.call_object(prefs, get, &[jni::object(name), jni::object(std::ptr::null_mut())])?;
        env.to_string(found)
    }

    /// Writes `value`, or removes the key when it is `None`.
    ///
    /// `commit` and not `apply`: the caller is told whether the secret
    /// landed, and an answer that arrives after the method returns is not an
    /// answer.
    fn pref_put(env: Env, key: &str, value: Option<&str>) -> bool {
        let put = || -> Option<bool> {
            let prefs = prefs(env)?;
            let class = env.class(c"android/content/SharedPreferences")?;
            let edit = env.method(class, c"edit", c"()Landroid/content/SharedPreferences$Editor;")?;
            let editor = env.call_object(prefs, edit, &[])?;
            let editor_class = env.class(c"android/content/SharedPreferences$Editor")?;
            let name = env.string(key)?;
            match value {
                Some(value) => {
                    let method = env.method(
                        editor_class,
                        c"putString",
                        c"(Ljava/lang/String;Ljava/lang/String;)Landroid/content/SharedPreferences$Editor;",
                    )?;
                    let text = env.string(value)?;
                    env.call_object(editor, method, &[jni::object(name), jni::object(text)])?;
                }
                None => {
                    let method = env.method(
                        editor_class,
                        c"remove",
                        c"(Ljava/lang/String;)Landroid/content/SharedPreferences$Editor;",
                    )?;
                    env.call_object(editor, method, &[jni::object(name)])?;
                }
            }
            let commit = env.method(editor_class, c"commit", c"()Z")?;
            env.call_bool(editor, commit, &[])
        };
        put().unwrap_or(false)
    }

    /// The app's key, minted on the first call and found on every one
    /// after.
    fn key_of(env: Env) -> Option<JObject> {
        let store = env.class(c"java/security/KeyStore")?;
        let get_instance =
            env.static_method(store, c"getInstance", c"(Ljava/lang/String;)Ljava/security/KeyStore;")?;
        let name = env.string("AndroidKeyStore")?;
        let keystore = env.call_static_object(store, get_instance, &[jni::object(name)])?;
        // `load(null)`: the keystore is the platform's and has no stream to
        // read and no password to take.
        let load = env.method(store, c"load", c"(Ljava/security/KeyStore$LoadStoreParameter;)V")?;
        env.call_void(keystore, load, &[jni::object(std::ptr::null_mut())]).then_some(())?;
        let alias = env.string(STORE)?;
        let get_key = env.method(store, c"getKey", c"(Ljava/lang/String;[C)Ljava/security/Key;")?;
        // `call_object` answers `None` for a null as well as for a throw, and
        // both mean the same thing here: this app has no key yet.
        if let Some(key) =
            env.call_object(keystore, get_key, &[jni::object(alias), jni::object(std::ptr::null_mut())])
        {
            return Some(key);
        }
        mint(env)
    }

    /// Mints the app's key: AES-GCM, in the keystore, for this app alone.
    ///
    /// No user authentication is asked for. The secret this guards is the
    /// one that decides whether the app must ask for a password at all, so a
    /// biometric prompt to read it would ask twice for the same thing.
    fn mint(env: Env) -> Option<JObject> {
        let generator = env.class(c"javax/crypto/KeyGenerator")?;
        let get_instance = env.static_method(
            generator,
            c"getInstance",
            c"(Ljava/lang/String;Ljava/lang/String;)Ljavax/crypto/KeyGenerator;",
        )?;
        let algorithm = env.string("AES")?;
        let provider = env.string("AndroidKeyStore")?;
        let keygen = env.call_static_object(
            generator,
            get_instance,
            &[jni::object(algorithm), jni::object(provider)],
        )?;

        let properties = env.class(c"android/security/keystore/KeyProperties")?;
        let purpose = |name: &CStr| env.static_int_field(properties, env.static_field(properties, name, c"I")?);
        let purposes = purpose(c"PURPOSE_ENCRYPT")? | purpose(c"PURPOSE_DECRYPT")?;

        let builder_class = env.class(c"android/security/keystore/KeyGenParameterSpec$Builder")?;
        let new_builder = env.method(builder_class, c"<init>", c"(Ljava/lang/String;I)V")?;
        let alias = env.string(STORE)?;
        let builder =
            env.new_object(builder_class, new_builder, &[jni::object(alias), jni::int(purposes)])?;

        let string_class = env.class(c"java/lang/String")?;
        let one = |value: &str| -> Option<JObject> {
            let value = env.string(value)?;
            env.new_object_array(string_class, &[value])
        };
        for (name, value) in [
            (c"setBlockModes", one("GCM")?),
            (c"setEncryptionPaddings", one("NoPadding")?),
        ] {
            let method = env.method(
                builder_class,
                name,
                c"([Ljava/lang/String;)Landroid/security/keystore/KeyGenParameterSpec$Builder;",
            )?;
            env.call_object(builder, method, &[jni::object(value)])?;
        }
        let build = env.method(
            builder_class,
            c"build",
            c"()Landroid/security/keystore/KeyGenParameterSpec;",
        )?;
        let spec = env.call_object(builder, build, &[])?;

        let init = env.method(generator, c"init", c"(Ljava/security/spec/AlgorithmParameterSpec;)V")?;
        env.call_void(keygen, init, &[jni::object(spec)]).then_some(())?;
        let generate = env.method(generator, c"generateKey", c"()Ljavax/crypto/SecretKey;")?;
        env.call_object(keygen, generate, &[])
    }

    fn cipher_class(env: Env) -> Option<JClass> {
        env.class(c"javax/crypto/Cipher")
    }

    fn new_cipher(env: Env, cipher: JClass) -> Option<JObject> {
        let get_instance =
            env.static_method(cipher, c"getInstance", c"(Ljava/lang/String;)Ljavax/crypto/Cipher;")?;
        let transformation = env.string("AES/GCM/NoPadding")?;
        env.call_static_object(cipher, get_instance, &[jni::object(transformation)])
    }

    fn gcm_spec(env: Env, iv: &[u8]) -> Option<JObject> {
        let class = env.class(c"javax/crypto/spec/GCMParameterSpec")?;
        let constructor = env.method(class, c"<init>", c"(I[B)V")?;
        let bytes = env.new_byte_array(iv)?;
        env.new_object(class, constructor, &[jni::int(TAG_BITS), jni::object(bytes)])
    }

    fn do_final(env: Env, cipher: JClass, handle: JObject, input: &[u8]) -> Option<Vec<u8>> {
        let method = env.method(cipher, c"doFinal", c"([B)[B")?;
        let bytes = env.new_byte_array(input)?;
        let answer = env.call_object(handle, method, &[jni::object(bytes)])?;
        env.byte_array(answer)
    }
}

#[cfg(target_os = "android")]
pub use door::{delete, read, write};

// MARK: - The pair, as one key of the preferences file

/// The two names as one preferences key.
///
/// Base64 on each half, and not `service/account`: a preferences file is
/// XML, the two names come from the app, and a name carrying a slash —
/// or a character the XML writer cannot hold — would either collide with
/// another pair or lose the file. The alphabet below has neither
/// problem.
fn slot(service: &str, account: &str) -> String {
    format!("{}.{}", base64(service.as_bytes()), base64(account.as_bytes()))
}

// MARK: - The sealed form

/// `[length of the iv][the iv][the ciphertext]`.
///
/// The length rides along because GCM's own is twelve bytes on every
/// provider that matters and is not promised anywhere: a reader that
/// assumed twelve would open nothing the day a provider drew sixteen,
/// and it would fail as a wrong tag rather than as a wrong shape.
fn join(iv: &[u8], body: &[u8]) -> Option<Vec<u8>> {
    let length = u8::try_from(iv.len()).ok()?;
    let mut out = Vec::with_capacity(1 + iv.len() + body.len());
    out.push(length);
    out.extend_from_slice(iv);
    out.extend_from_slice(body);
    Some(out)
}

fn split(sealed: &[u8]) -> Option<(&[u8], &[u8])> {
    let (length, rest) = sealed.split_first()?;
    let length = usize::from(*length);
    (rest.len() > length).then(|| rest.split_at(length))
}

/// The standard alphabet, with padding. Written here because the crate
/// takes no dependency, and in Rust rather than through
/// `android.util.Base64` because a codec that a test can run on the
/// build machine is a codec a phone never has to prove.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let block = u32::from(chunk[0]) << 16
            | u32::from(chunk.get(1).copied().unwrap_or(0)) << 8
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        for slot in 0..4 {
            if slot <= chunk.len() {
                let index = (block >> (18 - 6 * slot)) & 0x3F;
                out.push(ALPHABET[index as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn unbase64(text: &str) -> Option<Vec<u8>> {
    let mut block = 0u32;
    let mut held = 0;
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    for byte in text.bytes() {
        if byte == b'=' {
            break;
        }
        let index = ALPHABET.iter().position(|slot| *slot == byte)? as u32;
        block = block << 6 | index;
        held += 6;
        if held >= 8 {
            held -= 8;
            out.push((block >> held) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The codec answers itself, at every remainder the padding has to
    /// cover — a store whose keys are base64 cannot afford a round trip
    /// that loses the last byte of a name.
    #[test]
    fn the_codec_answers_its_own_encoding() {
        for length in 0..64usize {
            let bytes: Vec<u8> = (0..length).map(|index| (index * 37 + 11) as u8).collect();
            let text = base64(&bytes);
            assert_eq!(text.len() % 4, 0, "padded to a whole block: {text}");
            assert_eq!(unbase64(&text).as_deref(), Some(bytes.as_slice()), "round trip of {length}");
        }
        // the shapes a name actually takes
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(unbase64("Zm9vYg==").as_deref(), Some(&b"foob"[..]));
    }

    /// Two pairs are two keys, and a name carrying the separator does
    /// not become another pair's.
    #[test]
    fn a_pair_names_one_slot_and_only_its_own() {
        assert_ne!(slot("a.b", "c"), slot("a", "b.c"));
        assert_ne!(slot("service", "one"), slot("service", "two"));
        assert_eq!(slot("service", "one"), slot("service", "one"));
        assert!(
            slot("api.example.com", "default")
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"+/=.".contains(&byte)),
            "a preferences key is XML, so the slot stays in the alphabet",
        );
    }

    /// The iv travels with its own length, so a provider that draws a
    /// longer one is opened by the same reader.
    #[test]
    fn the_sealed_form_carries_the_length_of_its_iv() {
        for length in [12usize, 16] {
            let iv: Vec<u8> = (0..length).map(|index| index as u8).collect();
            let body = b"the ciphertext".to_vec();
            let sealed = join(&iv, &body).expect("an iv of a legible length");
            let (read_iv, read_body) = split(&sealed).expect("the halves come back");
            assert_eq!(read_iv, iv.as_slice());
            assert_eq!(read_body, body.as_slice());
        }
        // nothing to split is nothing, not a panic
        assert!(split(&[]).is_none());
        assert!(split(&[12]).is_none(), "an iv with no body is not a secret");
        assert!(split(&[4, 1, 2]).is_none(), "a length past the end answers nothing");
    }
}
