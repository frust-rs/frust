//! Theme ladder L3's platform-neutral half:
//! the [`Typeface`] a text-bearing control's `Props` carries, its wire codec,
//! and the pure cache-key/registration-plan/degrade-path logic
//! [`crate::android::fonts`] (the JNI-only half) builds its actual
//! cache-file write + `Typeface.createFromFile` call over.
//!
//! Living here — not under `android/` — is deliberate, mirroring
//! [`crate::api::theme`] vs [`crate::android::theme`]'s own L2/L1 split
//! (that module's doc: "the two live in different modules because this one
//! is host-testable ... and that one is JNI-only"): `android/fonts.rs` sits
//! inside `#[cfg(target_os = "android")] mod android;` (`crate`'s module
//! doc), so nothing inside it ever compiles — let alone runs a `#[cfg(test)]`
//! — on this host's `cargo test -p frust-native-widgets`. Cache-key
//! stability, registration-plan idempotency, the degrade path, and `Props`
//! gating for [`Typeface`] all have to live somewhere that compiles
//! everywhere to be host-testable at all, which is exactly what this module
//! is (declared from `crate::controls`, itself compiled on every target —
//! `crate::controls`'s own module doc).
//!
//! # OFL-1.1 redistribution note
//!
//! [`Typeface::GlyphMono`]/[`Typeface::GlyphPlex`] resolve to Space Mono /
//! IBM Plex Mono — the same bytes `frust-theme`'s `glyph-fonts` feature
//! bundles (Google Fonts / IBM, SIL Open Font License 1.1; see
//! `crates/frust-theme/src/glyph/fonts.rs`'s own module doc and
//! `crates/frust-theme/fonts/README.md` for provenance and the full license
//! text). `crate::android::fonts` writes those upstream bytes, unmodified, to
//! a private app-cache file purely because `Typeface.createFromFile` needs a
//! filesystem path (there is no create-from-bytes overload at this crate's
//! `minSdk` 24 floor) — it never redistributes the fonts anywhere a user or
//! another app can reach.

use crate::runtime::Params;

// --- the `Typeface` field and its wire codec --------------------------------

/// Which Glyph face (or the platform's own default) a text-bearing control's
/// `Typeface` should be set to — theme ladder L3. [`Self::System`] is
/// both the platform's own default AND the target of
/// [`crate::android::fonts`]'s registration-failure degrade path — never a
/// distinct third case a caller has to handle separately.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum Typeface {
    /// Space Mono — Glyph's display/heading face.
    GlyphMono,
    /// IBM Plex Mono — Glyph's UI face (titles/body/labels).
    GlyphPlex,
    /// The platform's own default `Typeface` — no registration and no real
    /// `setTypeface` call beyond restoring it (`setTypeface(null)`).
    #[default]
    System,
}

impl Typeface {
    /// The wire spelling [`crate::api::theme`] writes and [`decode`] reads
    /// back.
    pub(crate) fn wire(self) -> &'static str {
        match self {
            Self::GlyphMono => "glyphMono",
            Self::GlyphPlex => "glyphPlex",
            Self::System => "system",
        }
    }

    /// An unrecognized (or, via [`decode`], absent) spelling degrades to
    /// [`Self::System`] rather than a decode error — this crate's
    /// degrade-don't-fail rule (`crate::controls`' module doc).
    fn from_wire(raw: &str) -> Self {
        match raw {
            "glyphMono" => Self::GlyphMono,
            "glyphPlex" => Self::GlyphPlex,
            _ => Self::System,
        }
    }
}

/// Decode the [`Typeface`] field at `key`; an absent key decodes to
/// [`Typeface::System`], the platform default — see [`Typeface::from_wire`].
pub(crate) fn decode(params: &Params<'_>, key: &str) -> Typeface {
    params
        .string(key)
        .map(|raw| Typeface::from_wire(&raw))
        .unwrap_or_default()
}

// --- cache-key derivation (pure; `crate::android::fonts::register` uses it) -

/// A content-hash-keyed cache filename for `bytes` under `face_id` — the
/// registration spec's own wording: "one-time, content-hash-keyed cache-file
/// write, then `Typeface.createFromFile`". Stable for identical bytes (so a
/// later launch computes the exact same path and [`plan_registration`]
/// reuses the existing file instead of rewriting it), and differs whenever
/// the bytes differ (so a future font update never collides with, or is
/// masked by, a stale cached file). FNV-1a is a plain, dependency-free hash;
/// collision-resistance isn't the concern (this is a cache key, not a
/// security boundary), only stability and distribution.
pub(crate) fn cache_key(face_id: &str, bytes: &[u8]) -> String {
    format!("frust-native-widgets-{face_id}-{:016x}.ttf", fnv1a64(bytes))
}

/// 64-bit FNV-1a — see [`cache_key`].
fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET_BASIS, |hash, &b| {
        (hash ^ b as u64).wrapping_mul(PRIME)
    })
}

// --- registration-plan idempotency (pure) -----------------------------------

/// What a Glyph face's one-time registration should do, given whether its
/// content-hash-keyed cache file already exists — modelled as pure data,
/// like every [`Setter`](super::Setter) plan elsewhere in this crate, so
/// idempotency across launches is host-testable with no filesystem/JNI
/// dependency at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RegistrationAction {
    /// The cache file from an earlier launch (or an earlier call this same
    /// process) is already there — skip the write and go straight to
    /// `Typeface.createFromFile`.
    ReuseExisting,
    /// No file at this content-hash-keyed path yet — write it, then
    /// `createFromFile`.
    WriteThenCreate,
}

/// See [`RegistrationAction`].
pub(crate) fn plan_registration(cache_file_exists: bool) -> RegistrationAction {
    if cache_file_exists {
        RegistrationAction::ReuseExisting
    } else {
        RegistrationAction::WriteThenCreate
    }
}

// --- the degrade path (pure, generic) ---------------------------------------

/// The degrade-on-failure contract every registration attempt follows: `Ok`
/// keeps the resolved value; `Err` degrades to `None` (the caller's cue to
/// fall back to [`Typeface::System`]) and reports exactly one warning via
/// `warn` — never a crash, never a panic across FFI
/// (`docs/CODE_STANDARDS.md`'s no-unwind rule). Pure and generic over the
/// resolved value/error so a host test can exercise the branch with a fake
/// `Result`, no JNI involved; `crate::android::fonts::resolve` is the one
/// real caller, pairing it with a [`std::sync::Once`]-guarded `warn` so the
/// "exactly one" half of the contract holds across every later failed retry
/// too.
pub(crate) fn degrade_on_failure<T, E>(
    result: Result<T, E>,
    mut warn: impl FnMut(&E),
) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            warn(&error);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::with_identity;

    // --- Typeface wire codec / Props gating ---------------------------------

    #[test]
    fn wire_round_trips_every_variant() {
        for tf in [Typeface::GlyphMono, Typeface::GlyphPlex, Typeface::System] {
            assert_eq!(Typeface::from_wire(tf.wire()), tf);
        }
    }

    #[test]
    fn decode_defaults_to_system_when_absent_or_unrecognized() {
        let absent = with_identity("k", 1, "\"text\":\"hi\"");
        assert_eq!(decode(&Params::new(&absent), "typeface"), Typeface::System);

        let unrecognized = with_identity("k", 1, "\"typeface\":\"nonsense\"");
        assert_eq!(
            decode(&Params::new(&unrecognized), "typeface"),
            Typeface::System
        );
    }

    #[test]
    fn decode_reads_a_recognized_value() {
        let raw = with_identity("k", 1, "\"typeface\":\"glyphMono\"");
        assert_eq!(decode(&Params::new(&raw), "typeface"), Typeface::GlyphMono);

        let raw = with_identity("k", 1, "\"typeface\":\"glyphPlex\"");
        assert_eq!(decode(&Params::new(&raw), "typeface"), Typeface::GlyphPlex);
    }

    // --- cache-key derivation ------------------------------------------------

    #[test]
    fn cache_key_is_stable_for_identical_bytes() {
        let bytes = b"same font bytes".to_vec();
        assert_eq!(
            cache_key("glyphMono", &bytes),
            cache_key("glyphMono", &bytes),
            "identical bytes must derive the same cache key across calls (and \
             therefore across launches)"
        );
    }

    #[test]
    fn cache_key_differs_for_different_bytes() {
        assert_ne!(
            cache_key("glyphMono", b"one"),
            cache_key("glyphMono", b"two"),
        );
    }

    #[test]
    fn cache_key_differs_for_different_faces_given_the_same_bytes() {
        let bytes = b"shared bytes".to_vec();
        assert_ne!(
            cache_key("glyphMono", &bytes),
            cache_key("glyphPlex", &bytes)
        );
    }

    // --- registration-plan idempotency ---------------------------------------

    #[test]
    fn plan_registration_writes_once_then_reuses() {
        assert_eq!(
            plan_registration(false),
            RegistrationAction::WriteThenCreate,
            "no cache file yet: write it"
        );
        // A second launch (or a second call this same process) against the
        // same content-hash-keyed path, now that the file exists — the whole
        // "idempotent across launches" contract collapses to this one bit.
        assert_eq!(
            plan_registration(true),
            RegistrationAction::ReuseExisting,
            "the cache file already exists: never rewrite it"
        );
    }

    // --- the degrade path -----------------------------------------------------

    #[test]
    fn degrade_on_failure_passes_ok_through_without_warning() {
        let mut warned = 0;
        let result: Result<i32, &str> = Ok(42);
        assert_eq!(degrade_on_failure(result, |_| warned += 1), Some(42));
        assert_eq!(warned, 0, "a successful registration never warns");
    }

    #[test]
    fn degrade_on_failure_degrades_to_none_and_warns_exactly_once() {
        let mut warned = 0;
        let result: Result<i32, &str> = Err("boom");
        assert_eq!(
            degrade_on_failure(result, |_| warned += 1),
            None,
            "a failed registration degrades to None (the caller's System fallback)"
        );
        assert_eq!(warned, 1, "exactly one warning for the one failure");
    }
}
