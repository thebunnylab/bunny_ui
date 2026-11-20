use super::edwards::Point;
use super::*;

fn unhex<const N: usize>(text: &str) -> [u8; N] {
    let bytes: Vec<u8> = (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
        .collect();
    bytes.try_into().unwrap()
}

/// A stream of bytes the same on every run: SHA-512 of a counter.
struct Stream(u64);

impl Stream {
    fn bytes<const N: usize>(&mut self) -> [u8; N] {
        let mut out = [0u8; N];
        for chunk in out.chunks_mut(64) {
            self.0 += 1;
            let block = crate::sha512::sha512(&self.0.to_le_bytes());
            chunk.copy_from_slice(&block[..chunk.len()]);
        }
        out
    }

    fn message(&mut self) -> Vec<u8> {
        let len = usize::from(self.bytes::<1>()[0]) * 3;
        (0..len).map(|_| self.bytes::<1>()[0]).collect()
    }
}

fn dalek_verify(key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    use ed25519_dalek::Verifier as _;
    ed25519_dalek::VerifyingKey::from_bytes(key)
        .is_ok_and(|key| key.verify(message, &ed25519_dalek::Signature::from_bytes(signature)).is_ok())
}

fn own_verify(key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    VerifyingKey::from_bytes(key).is_some_and(|key| own::verify(&key, message, signature))
}

// RFC 8032, section 7.1: TEST 1, TEST 2, TEST 3.
const RFC: [(&str, &str, &str, &str); 3] = [
    (
        "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
        "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
        "",
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
    ),
    (
        "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
        "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
        "72",
        "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
    ),
    (
        "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
        "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
        "af82",
        "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
    ),
];

#[test]
fn the_rfc_vectors_sign_and_verify() {
    for (seed, public, message, signature) in RFC {
        let message: Vec<u8> = (0..message.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&message[at..at + 2], 16).unwrap())
            .collect();
        let key = SigningKey::from_seed(&unhex(seed));
        assert_eq!(key.verifying_key().as_bytes(), &unhex::<32>(public));
        assert_eq!(own::sign(&key, &message), unhex::<64>(signature));
        assert!(own::verify(&key.verifying_key(), &message, &unhex(signature)));
        assert!(dalek_verify(&unhex(public), &message, &unhex(signature)), "dalek reads the vector the same");
    }
}

#[test]
fn the_constants_are_the_curves() {
    use super::field::{Fe, constants};
    // d and the square root of -1, as RFC 8032 and every implementation print them
    assert_eq!(
        constants().d.to_bytes(),
        unhex::<32>("a3785913ca4deb75abd841414d0a700098e879777940c78c73fe6f2bee6c0352")
    );
    assert_eq!(
        constants().sqrt_m1.to_bytes(),
        unhex::<32>("b0a00e4a271beec478e42fad0618432fa7d7fb3d99004d2b0bdfc14f8024832b")
    );
    assert!(constants().sqrt_m1.square().equals(Fe::ONE.neg()));
    // the base point comes back to its own bytes
    let mut base = [0x66; 32];
    base[0] = 0x58;
    assert_eq!(Point::base().compress(), base);
}

#[test]
fn the_field_reduces_to_one_form() {
    use super::field::Fe;
    // p itself, p + 1 and 2^255 - 1 are 0, 1 and 18
    let mut p = [0xFF; 32];
    p[0] = 0xED;
    p[31] = 0x7F;
    assert!(Fe::from_bytes(&p).is_zero());
    let mut p1 = p;
    p1[0] = 0xEE;
    assert_eq!(Fe::from_bytes(&p1).to_bytes()[0], 1);
    let mut top = [0xFF; 32];
    top[31] = 0x7F;
    let mut eighteen = [0u8; 32];
    eighteen[0] = 18;
    assert_eq!(Fe::from_bytes(&top).to_bytes(), eighteen);
    // x * (1/x) = 1, and 0 has no inverse but zero
    let x = Fe::from_bytes(&unhex("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"));
    assert!(x.mul(x.invert()).equals(Fe::ONE));
    assert!(Fe::ZERO.invert().is_zero());
}

#[test]
fn the_backends_give_the_same_keys_and_signatures() {
    let mut stream = Stream(0);
    for _ in 0..200 {
        let seed = stream.bytes::<32>();
        let message = stream.message();
        let own = SigningKey::from_seed(&seed);
        let dalek = ed25519_dalek::SigningKey::from_bytes(&seed);
        assert_eq!(own.verifying_key().as_bytes(), dalek.verifying_key().as_bytes());
        let signature = own::sign(&own, &message);
        {
            use ed25519_dalek::Signer as _;
            assert_eq!(signature, dalek.sign(&message).to_bytes());
        }
        assert!(own::verify(&own.verifying_key(), &message, &signature));
        let mut reader = &message[..];
        assert!(own::verify_reader(&own.verifying_key(), &mut reader, &signature).unwrap());
    }
}

#[test]
fn the_backends_refuse_the_same_broken_signatures() {
    let mut stream = Stream(1 << 40);
    for round in 0..150 {
        let key = SigningKey::from_seed(&stream.bytes());
        let public = *key.verifying_key().as_bytes();
        let message = stream.message();
        let good = own::sign(&key, &message);
        // one bit flipped: in R, in s, in the key or in the message
        let bit = usize::from(stream.bytes::<1>()[0]) + round;
        let mut signature = good;
        signature[bit % 64] ^= 1 << (bit % 8);
        assert_eq!(own_verify(&public, &message, &signature), dalek_verify(&public, &message, &signature));
        assert!(!own_verify(&public, &message, &signature));
        let mut other = public;
        other[bit % 32] ^= 1 << (bit % 8);
        assert_eq!(own_verify(&other, &message, &good), dalek_verify(&other, &message, &good));
        let mut longer = message.clone();
        longer.push(0);
        assert!(!own_verify(&public, &longer, &good));
        assert!(!dalek_verify(&public, &longer, &good));
    }
}

#[test]
fn s_at_or_above_the_order_is_refused_by_both() {
    let key = SigningKey::from_seed(&[7; 32]);
    let public = *key.verifying_key().as_bytes();
    let good = own::sign(&key, b"release");
    // s + l is the same scalar, in a form no signature has
    let l = unhex::<32>("edd3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010");
    let mut s = [0u8; 32];
    let mut carry = 0u16;
    for i in 0..32 {
        let sum = u16::from(good[32 + i]) + u16::from(l[i]) + carry;
        s[i] = sum as u8;
        carry = sum >> 8;
    }
    let mut signature = good;
    signature[32..].copy_from_slice(&s);
    assert!(!own_verify(&public, b"release", &signature));
    assert!(!dalek_verify(&public, b"release", &signature));
    let mut at_l = good;
    at_l[32..].copy_from_slice(&l);
    assert_eq!(own_verify(&public, b"release", &at_l), dalek_verify(&public, b"release", &at_l));
}

#[test]
fn small_order_and_odd_keys_get_the_same_answer_from_both() {
    let mut stream = Stream(1 << 50);
    let mut p = [0xFF; 32];
    p[0] = 0xED;
    p[31] = 0x7F;
    let identity = {
        let mut bytes = [0u8; 32];
        bytes[0] = 1;
        bytes
    };
    let identity_with_sign = {
        let mut bytes = identity;
        bytes[31] |= 0x80;
        bytes
    };
    let identity_past_p = {
        let mut bytes = p;
        bytes[0] = 0xEE;
        bytes
    };
    let order_two = {
        // y = -1 = p - 1
        let mut bytes = p;
        bytes[0] = 0xEC;
        bytes
    };
    let mut accepted = 0;
    for key in [identity, identity_with_sign, identity_past_p, order_two] {
        assert_eq!(VerifyingKey::from_bytes(&key).is_some(), ed25519_dalek::VerifyingKey::from_bytes(&key).is_ok());
        for _ in 0..8 {
            // with a small-order key, R = [s]B passes whenever [k]A is the identity
            let mut s = stream.bytes::<32>();
            s[31] &= 0x0F;
            let mut signature = [0u8; 64];
            signature[..32].copy_from_slice(&Point::base().mul(&s).compress());
            signature[32..].copy_from_slice(&s);
            let message = stream.message();
            let own = own_verify(&key, &message, &signature);
            assert_eq!(own, dalek_verify(&key, &message, &signature), "key {key:02x?}");
            accepted += usize::from(own);
        }
    }
    // the identity keys pass every time and y = -1 about half the time:
    // the rule is the same, not only the refusals
    assert!(accepted >= 24, "{accepted} accepted");
}

#[test]
fn bytes_off_the_curve_are_refused_by_both() {
    let mut stream = Stream(1 << 60);
    let mut refused = 0;
    for _ in 0..400 {
        let bytes = stream.bytes::<32>();
        let own = VerifyingKey::from_bytes(&bytes).is_some();
        assert_eq!(own, ed25519_dalek::VerifyingKey::from_bytes(&bytes).is_ok(), "{bytes:02x?}");
        refused += usize::from(!own);
    }
    // about half of all y have no x
    assert!(refused > 100, "{refused} refused");
}

#[test]
fn the_public_api_signs_and_verifies() {
    let key = SigningKey::generate().unwrap();
    let signature = key.sign(b"bunny");
    assert!(key.verifying_key().verify(b"bunny", &signature));
    assert!(!key.verifying_key().verify(b"bunnY", &signature));
    assert!(key.verifying_key().verify_reader(&b"bunny"[..], &signature).unwrap());
    assert_eq!(SigningKey::from_seed(key.seed()).verifying_key(), key.verifying_key());
    assert!(!format!("{key:?}").contains(&crate::sha::hex(key.seed())), "the seed stays out of a log");
}
