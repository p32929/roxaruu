//! The remote-access password and logged-in sessions.
//!
//! Held only in memory, by the window serving remote access, for as long as
//! it serves: a salted PBKDF2 hash of the password (never the password
//! itself) and a random key that signs session cookies. Nothing here is ever
//! written to disk, so turning remote access off (or closing the window)
//! forgets the password and every session with it.

use std::num::NonZeroU32;

use ring::rand::{SecureRandom, SystemRandom};
use ring::{hmac, pbkdf2};

/// PBKDF2-HMAC-SHA256 rounds: slow enough to make guessing a stolen hash
/// expensive, quick enough that a login takes a fraction of a second.
const ITERATIONS: u32 = 300_000;
/// How long a login lasts.
pub const SESSION_SECS: u64 = 30 * 24 * 3600;

pub struct Secrets {
    salt: String,
    hash: String,
    iterations: u32,
    /// Signs session cookies.
    key: String,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn random<const N: usize>() -> Option<[u8; N]> {
    let mut b = [0u8; N];
    SystemRandom::new().fill(&mut b).ok()?;
    Some(b)
}

fn derive(password: &str, salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut out = [0u8; 32];
    let rounds = NonZeroU32::new(iterations.max(1)).unwrap_or(NonZeroU32::MIN);
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        rounds,
        salt,
        password.as_bytes(),
        &mut out,
    );
    out
}

impl Secrets {
    /// New secrets for `password` (any length, any characters).
    pub fn new(password: &str) -> Option<Secrets> {
        Secrets::with_iterations(password, ITERATIONS)
    }

    fn with_iterations(password: &str, iterations: u32) -> Option<Secrets> {
        let salt = random::<16>()?;
        let key = random::<32>()?;
        Some(Secrets {
            hash: hex(&derive(password, &salt, iterations)),
            salt: hex(&salt),
            iterations,
            key: hex(&key),
        })
    }

    /// Whether `password` is the one these secrets were made from
    /// (compared in constant time).
    pub fn verify(&self, password: &str) -> bool {
        let (Some(salt), Some(hash)) = (unhex(&self.salt), unhex(&self.hash)) else {
            return false;
        };
        let rounds = NonZeroU32::new(self.iterations.max(1)).unwrap_or(NonZeroU32::MIN);
        pbkdf2::verify(
            pbkdf2::PBKDF2_HMAC_SHA256,
            rounds,
            &salt,
            password.as_bytes(),
            &hash,
        )
        .is_ok()
    }

    fn signing_key(&self) -> hmac::Key {
        hmac::Key::new(hmac::HMAC_SHA256, &unhex(&self.key).unwrap_or_default())
    }

    /// A session token valid until `now + SESSION_SECS`: `expiry.signature`.
    pub fn issue(&self, now: u64) -> String {
        let expiry = (now + SESSION_SECS).to_string();
        let tag = hmac::sign(&self.signing_key(), expiry.as_bytes());
        format!("{expiry}.{}", hex(tag.as_ref()))
    }

    /// Whether `token` was issued with these secrets and hasn't expired.
    pub fn check(&self, token: &str, now: u64) -> bool {
        let Some((expiry, sig)) = token.split_once('.') else {
            return false;
        };
        let (Ok(until), Some(sig)) = (expiry.parse::<u64>(), unhex(sig)) else {
            return false;
        };
        now < until && hmac::verify(&self.signing_key(), expiry.as_bytes(), &sig).is_ok()
    }
}

/// A random hex token (for a window's private link to the server).
pub fn random_token() -> String {
    random::<24>().map(|b| hex(&b)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_only_the_right_password() {
        let s = Secrets::with_iterations("correct horse 🐎", 1000).unwrap();
        assert!(s.verify("correct horse 🐎"));
        assert!(!s.verify("correct horse"));
        assert!(!s.verify(""));
        // Any length works, including one character.
        assert!(Secrets::with_iterations("x", 1000).unwrap().verify("x"));
        // What's kept isn't the password.
        assert!(!s.hash.contains("horse") && !s.salt.contains("horse"));
    }

    #[test]
    fn sessions_are_signed_and_expire() {
        let s = Secrets::with_iterations("pw", 1000).unwrap();
        let t = s.issue(1_000);
        assert!(s.check(&t, 1_000));
        assert!(s.check(&t, 1_000 + SESSION_SECS - 1));
        assert!(!s.check(&t, 1_000 + SESSION_SECS), "expired");
        // Tampered expiry or signature, garbage, another key: all refused.
        let (exp, sig) = t.split_once('.').unwrap();
        let later = format!("{}.{sig}", exp.parse::<u64>().unwrap() + 999);
        assert!(!s.check(&later, 1_000));
        assert!(!s.check(&format!("{exp}.00"), 1_000));
        assert!(!s.check("nonsense", 1_000));
        let other = Secrets::with_iterations("pw", 1000).unwrap();
        assert!(!other.check(&t, 1_000), "a new password logs everyone out");
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(unhex(&hex(&[0, 1, 254, 255])), Some(vec![0, 1, 254, 255]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
    }
}
