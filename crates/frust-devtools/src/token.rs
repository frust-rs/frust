//! The per-process handshake token: how one is minted, and how one is
//! compared.
//!
//! # Why a token at all
//!
//! The listener is loopback-only, but on a device "loopback" is not a trust
//! boundary: any co-resident app can `connect("127.0.0.1", port)` and, without
//! auth, dump the widget tree or inject input. The token restores the boundary
//! the same way the Dart VM service's auth code does — it is printed on the
//! discovery line, which flows to `logcat`/`oslog`/the parent process's stderr,
//! and *only* to a reader privileged enough to see that stream (reading another
//! app's logcat needs `READ_LOGS`, which is a privileged permission; a desktop
//! app's stderr goes to whoever launched it). A peer that can already read the
//! app's log output could attach a debugger anyway.
//!
//! # Entropy, stated honestly
//!
//! There is **no dependency budget for a CSPRNG crate** here (`frust-devtools`
//! is protocol + tokio + log, and version pins are law), so this module uses
//! what std and the platform already provide, in this order:
//!
//! 1. `/dev/urandom`, when it can be read — a real kernel CSPRNG. This path
//!    holds on every unix target (Linux, Android, macOS, iOS). **Windows has
//!    no `/dev/urandom` and always takes path 2** — see the
//!    `devtools-token-entropy-windows-fallback` entry in `docs/LIMITATIONS.md`;
//!    a `BCryptGenRandom`-based Windows source is the known follow-up.
//! 2. Otherwise a composition of [`RandomState`] hashes (whose keys std seeds
//!    from OS entropy on first use), the wall clock, a monotonic instant, the
//!    process id, and a stack address.
//!
//! Path 2 is **not** a CSPRNG and is not claimed to be one: SipHash-1-3 is not
//! a cryptographic PRF, and an attacker who could observe several tokens from
//! one process might learn something about the hasher keys. What it does give
//! is 128 output bits an *unprivileged co-resident peer cannot enumerate*,
//! which is the property this secret needs — it guards a loopback debug port on
//! a debug/profile build, for the lifetime of one process, against guessing.
//!
//! [`generate`] reports which path it took ([`TokenSource`]). That is enough
//! for inspecting an app, not for loading code into it: the service offers the
//! `HotPatch` capability only behind an [`TokenSource::Os`] token.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Token length in bytes (hex-encoded to twice this many characters). 128 bits
/// is the same order as a UUIDv4 and far past brute-forcing a loopback port
/// that answers one connection at a time.
const TOKEN_BYTES: usize = 16;

/// Where a token's bytes came from. Reading a widget tree is fine behind
/// either; loading code is not, so the `HotPatch` capability is offered only
/// behind an [`TokenSource::Os`] token (`crate::service`'s hot-patch gate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenSource {
    /// `/dev/urandom` was read: a kernel CSPRNG.
    Os,
    /// The non-cryptographic [`fallback_random_bytes`] composition (Windows
    /// always; a unix sandbox that cannot open `/dev/urandom`).
    Fallback,
}

/// One minted token and where its entropy came from.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Token {
    pub(crate) value: String,
    pub(crate) source: TokenSource,
}

impl std::fmt::Debug for Token {
    /// Hand-written so a `{:?}` can never print the secret itself.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Token")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

/// Mints this process's devtools token: `2 * TOKEN_BYTES` lowercase hex
/// characters, containing no whitespace (the discovery line is
/// whitespace-delimited — see `frust_devtools_protocol::format_discovery_line`),
/// together with the [`TokenSource`] its bytes came from.
pub(crate) fn generate() -> Token {
    let (bytes, source) = match os_random_bytes() {
        Some(bytes) => (bytes, TokenSource::Os),
        None => (fallback_random_bytes(), TokenSource::Fallback),
    };
    let mut value = String::with_capacity(TOKEN_BYTES * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        // Writing into a String is infallible; the result is discarded rather
        // than unwrapped so a token can never be the reason a service dies.
        let _ = write!(value, "{byte:02x}");
    }
    Token { value, source }
}

/// Whether `presented` is the expected token, compared in a length-checked,
/// data-independent fold.
///
/// Constant-time comparison is arguably overkill for a loopback secret — but it
/// costs nothing here, and `==` on strings short-circuits at the first byte,
/// which is exactly the signal a timing oracle needs. No `subtle`-style
/// dependency: the fold below is the whole mechanism (`Ordering::Relaxed`-free,
/// no early return, no branch on the compared bytes).
pub(crate) fn matches(expected: &str, presented: &str) -> bool {
    let expected = expected.as_bytes();
    let presented = presented.as_bytes();
    // A length mismatch is unavoidably observable (there is no comparison to
    // make), and the token's length is not secret — only its bytes are.
    if expected.len() != presented.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in expected.iter().zip(presented.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// Kernel CSPRNG bytes, when the platform has the classic unix interface.
/// `None` on any platform or sandbox where it cannot be read, which routes the
/// caller to [`fallback_random_bytes`].
///
/// `std::fs` rather than `getrandom(2)`: no new dependency, and no `unsafe`
/// (`frust-devtools` is outside the sanctioned-unsafe zones in
/// `docs/CODE_STANDARDS.md`).
fn os_random_bytes() -> Option<[u8; TOKEN_BYTES]> {
    #[cfg(unix)]
    {
        use std::io::Read as _;
        let mut file = std::fs::File::open("/dev/urandom").ok()?;
        let mut bytes = [0u8; TOKEN_BYTES];
        file.read_exact(&mut bytes).ok()?;
        Some(bytes)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// The no-CSPRNG path (see the module doc): two independently seeded
/// [`RandomState`] hashers over per-process/per-moment inputs, giving 64 bits
/// each. Unguessable by an unprivileged peer; not a cryptographic guarantee.
fn fallback_random_bytes() -> [u8; TOKEN_BYTES] {
    let stack_marker = 0u8;
    let stack_addr = std::ptr::addr_of!(stack_marker) as usize as u64;
    let now_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let pid = u64::from(std::process::id());

    let high = hash_of(&(0xf1_u64, pid, now_nanos, stack_addr));
    // A second hasher (freshly built, so a different key) over a *later* clock
    // read: the two halves must not be recoverable from one another.
    let elapsed = Instant::now().elapsed().as_nanos() as u64;
    let low = hash_of(&(0x5c_u64, stack_addr, now_nanos ^ elapsed, high));

    let mut bytes = [0u8; TOKEN_BYTES];
    bytes[..8].copy_from_slice(&high.to_le_bytes());
    bytes[8..].copy_from_slice(&low.to_le_bytes());
    bytes
}

/// One 64-bit hash under a freshly built [`RandomState`] — std seeds its keys
/// from OS entropy on first use in the process, and it is that key, not the
/// hashed value, that carries the entropy here.
fn hash_of<T: Hash>(value: &T) -> u64 {
    RandomState::new().hash_one(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn a_token_is_hex_of_the_declared_width_and_carries_no_whitespace() {
        let token = generate().value;
        assert_eq!(token.len(), TOKEN_BYTES * 2);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!token.chars().any(char::is_whitespace));
    }

    #[test]
    fn tokens_do_not_repeat() {
        let tokens: HashSet<String> = (0..64).map(|_| generate().value).collect();
        assert_eq!(tokens.len(), 64, "a repeated token means no entropy at all");
    }

    #[test]
    fn the_token_reports_its_source() {
        // Unix reads /dev/urandom (the hosts this suite runs on); anything else
        // takes the fallback, Windows always.
        let expected = if cfg!(unix) && std::path::Path::new("/dev/urandom").exists() {
            TokenSource::Os
        } else {
            TokenSource::Fallback
        };
        assert_eq!(generate().source, expected);
    }

    #[test]
    fn a_token_debug_print_never_carries_the_secret() {
        let token = generate();
        assert!(!format!("{token:?}").contains(&token.value));
    }

    #[test]
    fn the_fallback_path_is_itself_non_repeating() {
        // Covers the non-unix/sandboxed branch on a host where /dev/urandom
        // exists and would otherwise mask it.
        let a = fallback_random_bytes();
        let b = fallback_random_bytes();
        assert_ne!(a, b);
        assert_ne!(a, [0u8; TOKEN_BYTES]);
    }

    #[test]
    fn matches_only_the_exact_token() {
        let token = generate().value;
        assert!(matches(&token, &token.clone()));
        assert!(!matches(&token, ""));
        assert!(!matches(&token, &token[..token.len() - 1]));
        assert!(!matches(&token, &format!("{token}x")));

        let mut flipped: Vec<char> = token.chars().collect();
        // Flip the *last* character: a short-circuiting comparison would have
        // read the whole token before noticing, which is what the fold avoids.
        flipped[TOKEN_BYTES * 2 - 1] = if flipped[TOKEN_BYTES * 2 - 1] == 'a' {
            'b'
        } else {
            'a'
        };
        assert!(!matches(&token, &flipped.into_iter().collect::<String>()));
    }
}
