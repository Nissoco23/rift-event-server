//! The voice packets' encryption: AES-128-GCM with a key of the player's own (their secret). A
//! sealed packet is a fresh 12-byte nonce, then the ciphertext with its 16-byte tag after it, which
//! is what Java's `AES/GCM/NoPadding` writes and reads.

use ring::aead::{AES_128_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};

pub const SECRET_LEN: usize = 16;

pub type Secret = [u8; SECRET_LEN];

/// A fresh random secret.
pub fn new_secret() -> Secret {
    let mut secret = [0u8; SECRET_LEN];
    SystemRandom::new()
        .fill(&mut secret)
        .expect("the system's random source");
    secret
}

/// Whether two secrets are the same, in the time it takes whatever they are.
pub fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

fn key(secret: &Secret) -> LessSafeKey {
    LessSafeKey::new(UnboundKey::new(&AES_128_GCM, secret).expect("a 16-byte key"))
}

/// `plain` sealed for `secret`.
pub fn seal(secret: &Secret, plain: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce)
        .expect("the system's random source");
    seal_with(secret, nonce, plain)
}

fn seal_with(secret: &Secret, nonce: [u8; NONCE_LEN], plain: &[u8]) -> Vec<u8> {
    let mut sealed = plain.to_vec();
    key(secret)
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::empty(),
            &mut sealed,
        )
        .expect("sealing a short packet");
    let mut out = nonce.to_vec();
    out.extend_from_slice(&sealed);
    out
}

/// What `sealed` holds, or none when it is too short or was not sealed with `secret`.
pub fn open(secret: &Secret, sealed: &[u8]) -> Option<Vec<u8>> {
    if sealed.len() < NONCE_LEN + 16 {
        return None;
    }
    let (nonce, cipher) = sealed.split_at(NONCE_LEN);
    let mut buffer = cipher.to_vec();
    let plain = key(secret)
        .open_in_place(
            Nonce::try_assume_unique_for_key(nonce).ok()?,
            Aad::empty(),
            &mut buffer,
        )
        .ok()?;
    Some(plain.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// NIST's AES-128-GCM test case 3 (a 96-bit nonce, no associated data): the bytes Java's
    /// `AES/GCM/NoPadding` gives for it.
    #[test]
    fn it_matches_the_published_gcm_vector() {
        let key: Secret = hex("feffe9928665731c6d6a8f9467308308").try_into().unwrap();
        let nonce: [u8; 12] = hex("cafebabefacedbaddecaf888").try_into().unwrap();
        let plain = hex(concat!(
            "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a72",
            "1c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b391aafd255"
        ));
        let sealed = hex(concat!(
            "cafebabefacedbaddecaf888",
            "42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e",
            "21d514b25466931c7d8f6a5aac84aa051ba30b396a0aac973d58e091473f5985",
            "4d5c2af327cd64a62cf35abd2ba6fab4"
        ));
        assert_eq!(seal_with(&key, nonce, &plain), sealed);
        assert_eq!(open(&key, &sealed), Some(plain));
    }

    #[test]
    fn a_sealed_packet_opens_only_with_its_secret_and_unchanged() {
        let secret = [7u8; SECRET_LEN];
        let sealed = seal(&secret, b"hello voice");
        assert_eq!(sealed.len(), 12 + 11 + 16);
        assert_eq!(open(&secret, &sealed).as_deref(), Some(&b"hello voice"[..]));
        assert_eq!(open(&[8u8; SECRET_LEN], &sealed), None);
        let mut broken = sealed.clone();
        *broken.last_mut().unwrap() ^= 1;
        assert_eq!(open(&secret, &broken), None);
        assert_eq!(open(&secret, &sealed[..20]), None);
        // The nonce is fresh each time.
        assert_ne!(seal(&secret, b"hello voice"), sealed);
    }

    #[test]
    fn secrets_are_random_and_compared_whole() {
        let (a, b) = (new_secret(), new_secret());
        assert_ne!(a, b);
        assert!(same(&a, &a) && !same(&a, &b) && !same(&a, &a[..8]));
    }
}
