//! PKCE (RFC 7636) code verifier and S256 challenge, plus the OAuth `state`
//! value.
//!
//! Only the `S256` challenge method exists here — there is deliberately no
//! `plain` method (RFC 7636 §4.2 allows it only when S256 is impossible,
//! which never applies to a client with SHA-256 available). Every secret
//! type redacts itself in `Debug` and deliberately has no `PartialEq`, so a
//! non-constant-time comparison cannot be written by accident.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest as _, Sha256};

use crate::encode::is_unreserved;

/// The shortest code verifier RFC 7636 §4.1 allows.
pub const MIN_VERIFIER_LEN: usize = 43;
/// The longest code verifier RFC 7636 §4.1 allows.
pub const MAX_VERIFIER_LEN: usize = 128;

/// Random bytes behind [`PkceVerifier::generate`]: 48 bytes encode to a
/// 64-character base64url verifier (384 bits of entropy).
const DEFAULT_VERIFIER_BYTES: usize = 48;
/// Random bytes behind [`State::generate`]: 32 bytes (256 bits, above the
/// 128 bits RFC 6749 §10.10 asks of a `state` value) encode to 43
/// characters.
const STATE_BYTES: usize = 32;

/// The operating system's random source failed. Never carries random
/// output.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RandomError {
    /// `getrandom` reported an OS-level failure; the payload is its
    /// description.
    #[error("the operating-system random source failed: {0}")]
    Os(String),
}

/// A PKCE verifier or `state` value was rejected. Messages name only the
/// rule broken, never the value.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PkceError {
    /// A code verifier length outside `43..=128` (RFC 7636 §4.1).
    #[error("code verifier length {0} is outside 43..=128")]
    InvalidLength(usize),
    /// A character outside the RFC 3986 unreserved set `[A-Za-z0-9-._~]`.
    #[error("value contains a character outside [A-Za-z0-9-._~]")]
    InvalidCharacter,
    /// An empty `state` value.
    #[error("state value is empty")]
    EmptyState,
    /// The random source failed while generating a value.
    #[error(transparent)]
    Random(#[from] RandomError),
}

fn random_bytes(len: usize) -> Result<Vec<u8>, RandomError> {
    let mut bytes = vec![0u8; len];
    getrandom::fill(&mut bytes).map_err(|err| RandomError::Os(err.to_string()))?;
    Ok(bytes)
}

/// A PKCE code verifier (RFC 7636 §4.1): 43–128 characters from
/// `[A-Za-z0-9-._~]`. Keep it in memory for the duration of one
/// authorization; send only its [`challenge`](Self::challenge) in the
/// authorization request, and the verifier itself only to the token
/// endpoint.
pub struct PkceVerifier(String);

impl PkceVerifier {
    /// Generates a 64-character verifier from 48 bytes of OS randomness,
    /// base64url-encoded without padding.
    pub fn generate() -> Result<Self, RandomError> {
        let bytes = random_bytes(DEFAULT_VERIFIER_BYTES)?;
        Ok(Self(URL_SAFE_NO_PAD.encode(bytes)))
    }

    /// Generates a verifier of exactly `len` characters (`43..=128`): enough
    /// OS randomness for `len` base64url characters, truncated to `len`.
    /// Every base64url character is in the unreserved set.
    pub fn generate_with_len(len: usize) -> Result<Self, PkceError> {
        if !(MIN_VERIFIER_LEN..=MAX_VERIFIER_LEN).contains(&len) {
            return Err(PkceError::InvalidLength(len));
        }
        // Each byte carries 8 bits, each base64 character 6: 3 bytes per 4
        // characters, rounded up.
        let bytes = random_bytes((len * 3).div_ceil(4))?;
        let mut encoded = URL_SAFE_NO_PAD.encode(bytes);
        encoded.truncate(len);
        Ok(Self(encoded))
    }

    /// Wraps an existing verifier (e.g. one restored after a process
    /// restart), validating RFC 7636 §4.1's length and charset.
    pub fn from_string(verifier: String) -> Result<Self, PkceError> {
        let len = verifier.chars().count();
        if !(MIN_VERIFIER_LEN..=MAX_VERIFIER_LEN).contains(&len) {
            return Err(PkceError::InvalidLength(len));
        }
        if !is_unreserved(&verifier) {
            return Err(PkceError::InvalidCharacter);
        }
        Ok(Self(verifier))
    }

    /// The verifier text, for the token request's `code_verifier`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The S256 challenge: `BASE64URL-NOPAD(SHA256(ASCII(verifier)))`
    /// (RFC 7636 §4.2).
    pub fn challenge(&self) -> PkceChallenge {
        let digest = Sha256::digest(self.0.as_bytes());
        PkceChallenge(URL_SAFE_NO_PAD.encode(digest))
    }
}

impl fmt::Debug for PkceVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PkceVerifier(<redacted>)")
    }
}

/// An S256 code challenge derived from a [`PkceVerifier`]. Not a secret:
/// it travels in the authorization URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkceChallenge(String);

impl PkceChallenge {
    /// The challenge text, for the `code_challenge` parameter.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The challenge method, always `"S256"` — the only one this crate
    /// supports.
    pub fn method(&self) -> &'static str {
        "S256"
    }
}

/// The OAuth `state` value (RFC 6749 §4.1.1): an unguessable per-request
/// value the callback must echo back, binding the redirect to the request
/// that started it.
#[derive(Clone)]
pub struct State(String);

impl State {
    /// Generates 32 bytes of OS randomness (256 bits) as 43 base64url
    /// characters.
    pub fn generate() -> Result<Self, RandomError> {
        let bytes = random_bytes(STATE_BYTES)?;
        Ok(Self(URL_SAFE_NO_PAD.encode(bytes)))
    }

    /// Wraps an existing `state` value, which must be non-empty and drawn
    /// from `[A-Za-z0-9-._~]`.
    pub fn from_string(state: String) -> Result<Self, PkceError> {
        if state.is_empty() {
            return Err(PkceError::EmptyState);
        }
        if !is_unreserved(&state) {
            return Err(PkceError::InvalidCharacter);
        }
        Ok(Self(state))
    }

    /// The state text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("State(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7636 Appendix B: the base64url encoding of the appendix's 32
    /// verifier octets, and its S256 challenge.
    const RFC_VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    const RFC_CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

    #[test]
    fn rfc_7636_appendix_b_vector() {
        let verifier = PkceVerifier::from_string(RFC_VERIFIER.to_string()).unwrap();
        let challenge = verifier.challenge();
        assert_eq!(challenge.as_str(), RFC_CHALLENGE);
        assert_eq!(challenge.method(), "S256");
    }

    #[test]
    fn generated_verifier_is_64_unreserved_chars_and_unique() {
        let a = PkceVerifier::generate().unwrap();
        let b = PkceVerifier::generate().unwrap();
        assert_eq!(a.as_str().len(), 64);
        assert!(is_unreserved(a.as_str()));
        assert_ne!(a.as_str(), b.as_str());
        // A generated verifier round-trips through the validator.
        assert!(PkceVerifier::from_string(a.as_str().to_string()).is_ok());
    }

    #[test]
    fn generate_with_len_bounds() {
        assert_eq!(
            PkceVerifier::generate_with_len(42).unwrap_err(),
            PkceError::InvalidLength(42)
        );
        assert_eq!(
            PkceVerifier::generate_with_len(129).unwrap_err(),
            PkceError::InvalidLength(129)
        );
        for len in [43, 44, 45, 46, 100, 127, 128] {
            let v = PkceVerifier::generate_with_len(len).unwrap();
            assert_eq!(v.as_str().len(), len);
            assert!(is_unreserved(v.as_str()));
        }
    }

    #[test]
    fn from_string_validates_length_and_charset() {
        assert_eq!(
            PkceVerifier::from_string("a".repeat(42)).unwrap_err(),
            PkceError::InvalidLength(42)
        );
        assert_eq!(
            PkceVerifier::from_string("a".repeat(129)).unwrap_err(),
            PkceError::InvalidLength(129)
        );
        assert!(PkceVerifier::from_string("a".repeat(43)).is_ok());
        assert!(PkceVerifier::from_string("-._~".repeat(32)).is_ok());
        let mut bad = "a".repeat(42);
        bad.push('+');
        assert_eq!(
            PkceVerifier::from_string(bad).unwrap_err(),
            PkceError::InvalidCharacter
        );
        let mut non_ascii = "a".repeat(42);
        non_ascii.push('é');
        assert_eq!(
            PkceVerifier::from_string(non_ascii).unwrap_err(),
            PkceError::InvalidCharacter
        );
    }

    #[test]
    fn state_is_43_unreserved_chars_and_unique() {
        let a = State::generate().unwrap();
        let b = State::generate().unwrap();
        assert_eq!(a.as_str().len(), 43);
        assert!(is_unreserved(a.as_str()));
        assert_ne!(a.as_str(), b.as_str());
    }

    #[test]
    fn state_from_string_rejects_empty_and_reserved() {
        assert_eq!(
            State::from_string(String::new()).unwrap_err(),
            PkceError::EmptyState
        );
        assert_eq!(
            State::from_string("a b".to_string()).unwrap_err(),
            PkceError::InvalidCharacter
        );
        assert_eq!(
            State::from_string("a&b".to_string()).unwrap_err(),
            PkceError::InvalidCharacter
        );
        assert_eq!(
            State::from_string("xyz".to_string()).unwrap().as_str(),
            "xyz"
        );
    }

    #[test]
    fn debug_redacts_verifier_and_state() {
        let verifier = PkceVerifier::from_string(RFC_VERIFIER.to_string()).unwrap();
        let shown = format!("{verifier:?}");
        assert_eq!(shown, "PkceVerifier(<redacted>)");
        assert!(!shown.contains(RFC_VERIFIER));

        let state = State::from_string("secret-state-value".to_string()).unwrap();
        let shown = format!("{state:?}");
        assert_eq!(shown, "State(<redacted>)");
        assert!(!shown.contains("secret-state-value"));
    }
}
