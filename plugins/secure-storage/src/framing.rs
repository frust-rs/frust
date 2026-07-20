//! Pure-Rust, FFI-free helpers for the Android Keystore backend
//! ([`crate::android`]): Base64, IV framing, and store-id/alias derivation.
//!
//! # Why a separate module
//!
//! The [`crate::android`] backend is `#[cfg(target_os = "android")]`-gated and
//! imports the Android-only `jni` crate, so it cannot compile — and its unit
//! tests cannot run — on the CI host (macOS/Linux). These three helper
//! families carry no JNI dependency, so they live here where they compile and
//! are **host-tested** (task S03 requirement 6: "Pure-Rust helpers unit tested
//! on the host; on-device round-trip is a recorded manual gate"). The module
//! is compiled only where it is actually used or tested
//! (`#[cfg(any(target_os = "android", test))]` on its `lib.rs` declaration),
//! so it never shows up as dead code in a host non-test build.
//!
//! # The stored-value wire format
//!
//! Android's `AndroidKeyStore` AES/GCM cipher generates a fresh random IV per
//! encryption; the backend must persist that IV alongside the ciphertext so a
//! later read can reconstruct the `GCMParameterSpec`. The bytes coming back
//! from JNI (`Cipher.getIV()` and `Cipher.doFinal(...)`) are [`frame`]d as
//! `iv_len(1 byte) || iv || ciphertext`, then [`b64_encode`]d into the ASCII
//! `String` that `SharedPreferences.putString` stores. A read reverses it:
//! [`b64_decode`] then [`unframe`]. Doing the Base64 and framing in Rust
//! (rather than `android.util.Base64`) is deliberate — it is the part of the
//! round-trip that is exactly, cheaply host-testable without a device.

/// Standard Base64 alphabet (RFC 4648 §4), with `+`/`/` and `=` padding — the
/// same alphabet `android.util.Base64.DEFAULT` uses, so a value written by this
/// backend is also decodable by ordinary Android/Java tooling if ever needed.
const B64_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode `input` as padded standard Base64.
pub(crate) fn b64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64_ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(B64_ALPHABET[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            B64_ALPHABET[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64_ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Decode padded standard Base64, returning `None` for any malformed input
/// (bad length, an illegal character, or padding that isn't a contiguous run
/// at the very end). A corrupted stored entry maps to a typed
/// [`crate::SecureStorageError::Storage`] at the call site rather than a panic.
pub(crate) fn b64_decode(input: &str) -> Option<Vec<u8>> {
    let bytes = input.as_bytes();
    if bytes.is_empty() {
        return Some(Vec::new());
    }
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let n_chunks = bytes.len() / 4;
    let mut out = Vec::with_capacity(n_chunks * 3);
    for (ci, chunk) in bytes.chunks(4).enumerate() {
        let is_last = ci + 1 == n_chunks;
        let mut acc = 0u32;
        let mut pad = 0usize;
        for (i, &c) in chunk.iter().enumerate() {
            let v = match c {
                b'A'..=b'Z' => u32::from(c - b'A'),
                b'a'..=b'z' => u32::from(c - b'a') + 26,
                b'0'..=b'9' => u32::from(c - b'0') + 52,
                b'+' => 62,
                b'/' => 63,
                // Padding is only ever valid in the last one or two positions
                // of the final quartet.
                b'=' if is_last && i >= 2 => {
                    pad += 1;
                    0
                }
                _ => return None,
            };
            // Once a pad byte has appeared, only more pad bytes may follow.
            if pad > 0 && c != b'=' {
                return None;
            }
            acc = (acc << 6) | v;
        }
        out.push((acc >> 16) as u8);
        if pad < 2 {
            out.push((acc >> 8) as u8);
        }
        if pad < 1 {
            out.push(acc as u8);
        }
    }
    Some(out)
}

/// Frame an `iv` and `ciphertext` into the single `iv_len || iv || ciphertext`
/// byte string this backend persists (see the module doc). `iv` must be at
/// most 255 bytes — always true for the 12-byte GCM IV `AndroidKeyStore`
/// generates.
pub(crate) fn frame(iv: &[u8], ciphertext: &[u8]) -> Vec<u8> {
    debug_assert!(
        iv.len() <= u8::MAX as usize,
        "GCM IV must fit in the single length byte"
    );
    let mut out = Vec::with_capacity(1 + iv.len() + ciphertext.len());
    out.push(iv.len() as u8);
    out.extend_from_slice(iv);
    out.extend_from_slice(ciphertext);
    out
}

/// Split framed bytes back into `(iv, ciphertext)`, or `None` if the buffer is
/// truncated (an empty buffer, or one shorter than its own declared IV length).
pub(crate) fn unframe(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let (&iv_len, rest) = bytes.split_first()?;
    let iv_len = iv_len as usize;
    if rest.len() < iv_len {
        return None;
    }
    Some(rest.split_at(iv_len))
}

/// The per-store identifier `frust.ss.<sanitized-name>` used for **both** the
/// `AndroidKeyStore` key alias and the `SharedPreferences` file name (task
/// S03 requirements 1 and 2). Baking the store name into the prefs file name
/// gives store-name isolation the same way the [`crate::file`] backend's
/// per-store filename does, and the `frust.ss.` namespace
/// ([`crate::KEY_NAMESPACE_PREFIX`]) keeps a store from ever colliding with
/// another library's Keystore alias or prefs file.
pub(crate) fn store_id(name: &str) -> String {
    format!("{}{}", crate::KEY_NAMESPACE_PREFIX, sanitize(name))
}

/// Make a store name safe to use as a single `SharedPreferences` file-name
/// component (and a tidy Keystore alias): replace any character that isn't
/// ASCII alphanumeric, `-`, or `_` with `_`. Mirrors the [`crate::file`]
/// backend's `sanitize_store` so the two backends namespace identically. The
/// `frust.ss.` prefix's own `.`s are added by [`store_id`] *after* this and
/// are intentionally preserved (valid in both a file name and an alias).
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Base64 round-trips every byte-length residue class (0,1,2 mod 3), which
    /// exercises both padding shapes and the no-padding case.
    #[test]
    fn b64_round_trips_all_padding_shapes() {
        for len in 0..=64usize {
            let input: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
            let encoded = b64_encode(&input);
            assert_eq!(encoded.len() % 4, 0, "encoded len must be a multiple of 4");
            let decoded = b64_decode(&encoded).expect("round-trip must decode");
            assert_eq!(decoded, input, "round-trip mismatch at len {len}");
        }
    }

    /// Known-answer vectors from RFC 4648 §10 pin the exact encoding (not just
    /// self-consistency).
    #[test]
    fn b64_known_answers() {
        assert_eq!(b64_encode(b""), "");
        assert_eq!(b64_encode(b"f"), "Zg==");
        assert_eq!(b64_encode(b"fo"), "Zm8=");
        assert_eq!(b64_encode(b"foo"), "Zm9v");
        assert_eq!(b64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(b64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(b64_decode("Zm9vYmFy").unwrap(), b"foobar");
        assert_eq!(b64_decode("").unwrap(), b"");
    }

    /// A full byte range survives, catching any signed/unsigned or high-bit bug.
    #[test]
    fn b64_covers_full_byte_range() {
        let input: Vec<u8> = (0..=255u8).collect();
        assert_eq!(b64_decode(&b64_encode(&input)).unwrap(), input);
    }

    /// Malformed Base64 is rejected (typed error at the call site), never
    /// decoded into garbage or a panic.
    #[test]
    fn b64_rejects_malformed() {
        assert_eq!(b64_decode("Zm9"), None, "length not a multiple of 4");
        assert_eq!(b64_decode("Zg=="), Some(b"f".to_vec()), "control: valid");
        assert_eq!(b64_decode("Z==="), None, "pad before position 2");
        assert_eq!(b64_decode("Zm=v"), None, "pad followed by non-pad");
        assert!(
            b64_decode("Zm9v=g==").is_none(),
            "pad in a non-final quartet"
        );
        assert_eq!(b64_decode("Zg=@"), None, "illegal character");
        assert_eq!(b64_decode("Z g="), None, "embedded space");
    }

    /// Framing prepends the IV length and round-trips exactly.
    #[test]
    fn frame_unframe_round_trips() {
        let iv = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let ct = [0xAAu8; 40];
        let framed = frame(&iv, &ct);
        assert_eq!(framed.len(), 1 + iv.len() + ct.len());
        assert_eq!(framed[0] as usize, iv.len());
        let (got_iv, got_ct) = unframe(&framed).expect("must unframe");
        assert_eq!(got_iv, iv);
        assert_eq!(got_ct, ct);
    }

    /// An empty IV and empty ciphertext are still framed unambiguously.
    #[test]
    fn frame_unframe_empty_parts() {
        let framed = frame(&[], &[]);
        assert_eq!(framed, vec![0u8]);
        let (iv, ct) = unframe(&framed).unwrap();
        assert!(iv.is_empty() && ct.is_empty());
    }

    /// Truncated framed buffers are rejected rather than panicking on a slice.
    #[test]
    fn unframe_rejects_truncated() {
        assert_eq!(unframe(&[]), None, "no length byte");
        // Declares a 12-byte IV but only 3 bytes follow.
        assert_eq!(unframe(&[12, 1, 2, 3]), None);
    }

    /// The whole stored-value pipeline (frame → Base64 → Base64 → unframe)
    /// reconstructs the exact IV and ciphertext.
    #[test]
    fn full_wire_pipeline_round_trips() {
        let iv = [9u8, 8, 7, 6, 5, 4, 3, 2, 1, 0, 255, 128];
        let ct: Vec<u8> = (0..37).map(|i| (i * 13 + 1) as u8).collect();
        let stored = b64_encode(&frame(&iv, &ct));
        let bytes = b64_decode(&stored).unwrap();
        let (got_iv, got_ct) = unframe(&bytes).unwrap();
        assert_eq!(got_iv, iv);
        assert_eq!(got_ct, ct.as_slice());
    }

    /// `store_id` applies the `frust.ss.` namespace and preserves its dots
    /// while sanitizing the store-name segment.
    #[test]
    fn store_id_namespaces_and_sanitizes() {
        assert_eq!(store_id("credentials"), "frust.ss.credentials");
        assert_eq!(store_id("ok-name_1"), "frust.ss.ok-name_1");
        // Path-hostile / separator characters in the name become `_`; the
        // prefix's own dots are preserved.
        assert_eq!(store_id("a/b"), "frust.ss.a_b");
        assert_eq!(store_id("../etc"), "frust.ss.___etc");
        assert_eq!(store_id("my.store"), "frust.ss.my_store");
    }
}
