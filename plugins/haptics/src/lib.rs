//! `frust-haptics`: a minimal, general-purpose haptic-feedback plugin —
//! Android `Vibrator`/`VibrationEffect` via plain JNI, iOS
//! `UISelectionFeedbackGenerator`/`UIImpactFeedbackGenerator`/
//! `UINotificationFeedbackGenerator` via `objc2-ui-kit`, desktop
//! (macOS/Linux/Windows) unavailable by design.
//!
//! # Charter: a platform plugin
//!
//! Like [`frust-clipboard`](../frust_clipboard/index.html) and
//! `frust-secure-storage`/`frust-shared-preferences`, this is a **platform
//! plugin** (see `docs/ARCHITECTURE.md`'s Module Structure): it depends on
//! `frust-plugin` plus FFI crates only, and carries **no other `frust-*`
//! framework dependency**. An app adds this crate to its own `Cargo.toml`
//! alongside `frust`, the Flutter-pubspec model; the `frust` facade does not
//! depend on or re-export it.
//!
//! # Fire-and-forget, best-effort, silent no-op where unsupported
//!
//! [`Haptics::perform`] is a one-shot imperative action, not a state query —
//! there is nothing to read back, unlike `Clipboard`/`SharedPreferences`'
//! get/set pairs. A caller is expected to ignore the `Result` in the common
//! case (a haptic tick is cosmetic feedback, never load-bearing UI state);
//! the `Result` exists for the caller that *does* want to know why nothing
//! buzzed (see [`HapticsError`]). Every backend degrades gracefully rather
//! than panicking: no vibration hardware ([`android`]'s `hasVibrator()`
//! check) is a silent `Ok(())` no-op, and desktop is
//! [`HapticsError::NotAvailable`] unconditionally (see [`desktop`]'s module
//! doc).
//!
//! # `HapticEffect` is the Android/iOS lowest-common vocabulary
//!
//! [`HapticEffect`] is deliberately small — seven variants covering the
//! feedback categories both mobile platforms ship a first-class API for:
//! a selection tick, three impact intensities, and three notification-style
//! outcomes. It is not `#[non_exhaustive]`: this is a closed, considered
//! vocabulary (widening it is a deliberate, future decision each backend's
//! `match` would have to answer for, not an accidental compile pass — see
//! each backend module's own `perform` implementation, every one of which
//! matches every variant with no wildcard arm).
//!
//! # Backends
//!
//! [`Haptics::perform`] routes by `#[cfg(target_os = ...)]` to one of three
//! real backends: [`android`] (`Vibrator`/`VibrationEffect`, plain JNI — no
//! Kotlin/Gradle module, see that module's doc for why one isn't needed),
//! [`apple`] (iOS only — the three `UI*FeedbackGenerator` classes, dispatched
//! onto the main queue), and [`desktop`] (macOS, Linux, Windows —
//! unconditionally unavailable). Every backend's `perform` exhaustively
//! matches [`HapticEffect`] with no wildcard arm — a compile-level guarantee
//! that every effect routes on every cfg arm, checked by this crate's own
//! `cargo test` (desktop), and by the mobile compile gates (`cargo check
//! --target aarch64-linux-android`/`aarch64-apple-ios -p frust-haptics` — see
//! `docs/DEVELOPMENT.md`'s Test section).

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod apple;
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
mod desktop;

/// The lowest-common haptic-feedback vocabulary Android and iOS both ship a
/// first-class API for.
///
/// A closed vocabulary (see the crate doc's *`HapticEffect` is the
/// Android/iOS lowest-common vocabulary*) — deliberately not
/// `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HapticEffect {
    /// A UI selection changed (e.g. a picker wheel tick, a segmented control
    /// moving to a new segment) — the lightest, most frequent effect in this
    /// vocabulary. Android: `VibrationEffect.EFFECT_TICK`. iOS:
    /// `UISelectionFeedbackGenerator`.
    SelectionClick,
    /// A light-weight impact (e.g. a small element docking into place).
    /// Android: `VibrationEffect.EFFECT_CLICK`. iOS:
    /// `UIImpactFeedbackGenerator` with `.light`.
    ImpactLight,
    /// A medium-weight impact. Android has no built-in "medium" predefined
    /// effect, so this backend uses a custom amplitude one-shot instead (see
    /// [`android`]'s module doc). iOS: `UIImpactFeedbackGenerator` with
    /// `.medium`.
    ImpactMedium,
    /// A heavy-weight impact (e.g. a large element docking, a forceful
    /// collision). Android: `VibrationEffect.EFFECT_HEAVY_CLICK`. iOS:
    /// `UIImpactFeedbackGenerator` with `.heavy`.
    ImpactHeavy,
    /// A task completed successfully. Android: a short composed waveform.
    /// iOS: `UINotificationFeedbackGenerator` with `.success`.
    Success,
    /// A task produced a warning. Android: a short composed waveform. iOS:
    /// `UINotificationFeedbackGenerator` with `.warning`.
    Warning,
    /// A task failed. Android: a short composed waveform. iOS:
    /// `UINotificationFeedbackGenerator` with `.error`.
    Error,
}

/// Errors from a [`Haptics::perform`] call.
///
/// `thiserror`-derived per `docs/CODE_STANDARDS.md`: callers match on the
/// variant rather than only displaying it. A caller that only wants
/// best-effort fire-and-forget feedback is expected to ignore the `Result`
/// entirely (the crate doc's *Fire-and-forget* section) — this enum exists
/// for the caller that does want to distinguish "not initialized yet" from
/// "this platform can't do haptics" from "a genuine backend failure".
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum HapticsError {
    /// The Android host shell never installed the `(JavaVM, Context)`
    /// platform handles this crate's Android backend needs
    /// (`frust-plugin`'s pre-init state) — an old scaffold predating
    /// `nativeInitPlatform`. Never a panic; the caller degrades gracefully.
    #[error("haptics platform not initialized")]
    PlatformNotInitialized,

    /// Haptic feedback is unavailable for the given reason (see
    /// [`Unavailability`]) — distinct from a transient per-call failure
    /// ([`Self::Platform`]).
    #[error("haptics unavailable: {0:?}")]
    NotAvailable(Unavailability),

    /// A backend-specific failure that isn't a not-yet-initialized/
    /// unavailable condition — a platform JNI error this crate doesn't
    /// otherwise classify.
    #[error("haptics error: {0}")]
    Platform(String),
}

/// Why haptic feedback is unavailable (the [`HapticsError::NotAvailable`]
/// payload).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Unavailability {
    /// This platform has no haptic-feedback mechanism this crate drives —
    /// every desktop target (macOS, Linux, Windows) unconditionally, in v1
    /// (see [`desktop`]'s module doc) — or a build target this crate has no
    /// backend module for at all (not Android/iOS/macOS/Linux/Windows —
    /// e.g. tvOS, wasm), where [`Haptics::perform`] reports this rather
    /// than failing to compile.
    UnsupportedPlatform,
}

/// One backend implementation — [`android::AndroidHaptics`],
/// [`apple::AppleHaptics`] (iOS only), or [`desktop::DesktopHaptics`]
/// (macOS/Linux/Windows).
///
/// Crate-private and deliberately minimal: [`Haptics`]'s public API is a thin
/// wrapper cfg-dispatching to exactly one of these per target. Unlike
/// `frust-clipboard`'s `Backend`, there is no shared conformance suite here —
/// a fire-and-forget vibration has no observable state to round-trip; each
/// backend's own `perform` exhaustively matching [`HapticEffect`] (no
/// wildcard arm) is the compile-level guarantee every effect routes on every
/// cfg arm (the crate doc's *Backends* section).
pub(crate) trait Backend: Send + Sync {
    /// Perform `effect`, best-effort. Never blocks the caller — see each
    /// backend module's own doc for how (Android: the JNI call itself is
    /// quick, fired inline; iOS: dispatched async to the main queue; desktop:
    /// returns synchronously, always `Err`).
    fn perform(&self, effect: HapticEffect) -> Result<(), HapticsError>;
}

/// The platform haptic-feedback entry point.
///
/// Carries no state — [`Self::perform`] is a plain associated function,
/// matching `frust-clipboard`'s `Clipboard` (there is exactly one haptic
/// actuator this crate addresses per device, no named-handle model).
pub struct Haptics;

impl Haptics {
    /// Perform `effect`, best-effort and fire-and-forget (the crate doc's
    /// *Fire-and-forget, best-effort, silent no-op where unsupported*
    /// section) — a caller with no interest in *why* nothing happened is
    /// expected to ignore the `Result`.
    ///
    /// # Errors
    /// On Android, [`HapticsError::PlatformNotInitialized`] for an old
    /// scaffold predating `nativeInitPlatform`, or [`HapticsError::Platform`]
    /// for a genuine JNI failure — a device with no vibration hardware is
    /// *not* an error (see [`android`]'s module doc). On iOS, always
    /// `Ok(())`: dispatching to the main queue cannot itself fail (see
    /// [`apple`]'s module doc). On desktop,
    /// [`HapticsError::NotAvailable`] unconditionally.
    pub fn perform(effect: HapticEffect) -> Result<(), HapticsError> {
        #[cfg(target_os = "android")]
        {
            android::AndroidHaptics.perform(effect)
        }
        #[cfg(target_os = "ios")]
        {
            apple::AppleHaptics.perform(effect)
        }
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        {
            desktop::DesktopHaptics.perform(effect)
        }
        // Total-cover fallback: any target not one of the four arms above
        // (tvOS, wasm, …) has no backend module compiled in at all — report
        // it as a typed unavailability rather than failing to compile with a
        // confusing "no arm produced a value" error (matches
        // `frust-clipboard`'s own dispatch functions).
        #[cfg(not(any(
            target_os = "android",
            target_os = "ios",
            target_os = "macos",
            target_os = "linux",
            target_os = "windows"
        )))]
        {
            let _ = effect;
            Err(HapticsError::NotAvailable(
                Unavailability::UnsupportedPlatform,
            ))
        }
    }
}
