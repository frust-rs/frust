//! Theme ladder L2: platform-neutral token
//! resolution. [`resolve`] folds the active [`frust::Theme`] into the packed
//! primitives ([`u32`] ARGB, [`f32`] dp/sp) [`crate::api::builders`] writes
//! into each control's `params_json` and Android's `NativeWidget::update`/
//! `create` setters ([`crate::controls::platform::apply`]) apply, unchanged,
//! via the exact same direct-setter path every other property already rides.
//! See [`crate::android::theme`] for L1 (the night-qualified `Context` this
//! module's `dark` flag drives) — the two live in different modules because
//! this one is host-testable (no `target_os` gate) and that one is JNI-only.
//!
//! # Mapping table (pinning, not full token fidelity)
//!
//! This crate's approximation policy: this folds a *representative* subset
//! of `Theme` into each control, not full design-token fidelity (no
//! elevation/motion/glass, no per-state — hover/pressed/disabled — variants
//! the platform's own drawables already provide for free).
//!
//! | Token | `ColorScheme`/`ShapeScale`/`TypeScale` source | Controls |
//! |---|---|---|
//! | `accent_ink` | `scheme().primary` | `Switch`/`Slider` thumb tint |
//! | `accent_fill` | `scheme().primary_container` | `Button` background, `Switch` track tint, `Slider`/`ProgressBar` progress tint |
//! | `on_accent_fill` | `scheme().on_primary_container` | `Button` text colour |
//! | `body_text` | `scheme().on_surface` | `Label` text colour |
//! | `surface_bg` | `scheme().surface` | `Label`/`ProgressBar` background (explicit — see *Explicit backgrounds* below for why `Switch`/`Slider` are deliberately excluded) |
//! | `corner_radius_dp` | `shape.small` | `Button` background (via a `GradientDrawable`) |
//! | `button_text_size_sp` | `type_scale.label_large.size` | `Button` text size |
//! | `body_text_size_sp` | `type_scale.body_large.size` | `Label` text size |
//! | `dark` | `brightness == Brightness::Dark` | every control (L1's `Context` qualification) |
//! | `button_typeface` | `NativeTypefaces::button` ⇒ that face, else `design_language == Glyph` ⇒ Space Mono, else the platform's own | `Button` `Typeface` (theme ladder L3) |
//! | `body_typeface` | `NativeTypefaces::body` ⇒ that face, else `design_language == Glyph` ⇒ IBM Plex Mono, else the platform's own | `Label`/`Switch` `Typeface` (theme ladder L3) |
//!
//! # Theme ladder L3: typography, extension-first
//!
//! Unlike every other row above (folded unconditionally from whichever
//! `Theme` is active), the two typeface rows resolve through a three-step
//! ladder, **independently per slot** ([`resolve_slot`]):
//!
//! 1. **[`frust_theme::NativeTypefaces`]**, the theme extension a third-party
//!    design system attaches to carry its own faces (see that type's module
//!    doc — no built-in baseline attaches it). Its `button`/`body` face bytes
//!    are published straight through this module's existing two-payload
//!    platform seam and selected for that slot.
//! 2. **The Glyph shortcut** — no extension face for this slot, but
//!    [`Theme::design_language`] is [`DesignLanguage::Glyph`]: the bundled
//!    `frust_theme::glyph::font_data()` faces apply, `Button` getting Space
//!    Mono (Glyph's bolder display face) and `Label`/`Switch` IBM Plex Mono
//!    (Glyph's body face).
//! 3. **[`typeface::Typeface::System`]** — the platform's own face. Imposing
//!    Glyph's monospace faces on a Material3/Cupertino theme that never asked
//!    for them would be a worse regression than leaving the platform's own
//!    face alone, so a theme with neither an attached face nor the Glyph tag
//!    lands here. This arm publishes **no bytes** (`&[]`), not the bundled
//!    Glyph bytes — see *System publishes nothing* below.
//!
//! ## System publishes nothing
//!
//! An earlier revision of step 3 carried the bundled Glyph bytes along even
//! when `System` was selected, reasoning that this stopped the *other*
//! slot's publish from "blanking" them. That reasoning was wrong: with the
//! `glyph-fonts` feature on by default, it meant a theme with no
//! `NativeTypefaces` extension at all — a plain, unmodified Material3
//! baseline — published the non-empty pair `(Space Mono, IBM Plex Mono)` on
//! its very first [`resolve`], because the ride-along bytes made the pair
//! look non-empty to [`publish_font_bytes`]'s empty-pair skip. Since the
//! platform halves latch their *first* published pair
//! ([`crate::android::fonts::set_glyph_bytes`], mirrored on iOS), a
//! Material3-only app that happened to construct any native control before
//! installing a design system permanently latched Glyph's own faces — a
//! design system's later, real publish would register correctly host-side
//! (this module's own `ResolvedTheme`/props) but the platform half would
//! never re-register the device-side font object, so the device kept
//! rendering Space Mono / IBM Plex Mono for `GlyphMono`/`GlyphPlex` no
//! matter what the design system published.
//!
//! With step 3 now publishing `&[]`, a Material3/no-extension resolve
//! produces the empty pair `(&[], &[])`, [`publish_font_bytes`] skips it
//! entirely (its own empty-pair short-circuit), and nothing latches — so a
//! design system installed afterward gets the first real publish and
//! registers correctly. Every legitimate need for the bundled Glyph bytes is
//! still met: step 2 supplies them directly, in the same resolve, whenever a
//! Glyph theme actually selects them for a slot — the ride-along in step 3
//! never fired for a case step 2 did not already cover on its own.
//!
//! **Half-filled extension, one real slot + one `System` slot** (e.g. a
//! design system that only overrides the button face): the published pair
//! is one-sided — `(custom_button_bytes, &[])` — which is *not* the
//! all-empty case, so it still crosses the seam and still latches
//! process-wide via the platform halves' `OnceLock`. That is coherent with
//! this resolve: the empty body slot resolves to `Typeface::System` in the
//! very same call, so nothing ever asks the platform half to register
//! `GlyphPlex` off these bytes — `typeface_for`/`descriptor_for` short-circuit
//! `System` before touching the published payload at all. The empty half
//! only becomes observable if a *later, different* resolve wants real bytes
//! for that same slot (e.g. the app later switches to a Glyph theme, or the
//! design system later fills the body slot too); that is exactly the
//! existing first-publish-latch gap (see *Publishing* below and
//! `docs/LIMITATIONS.md`'s `native-typeface-first-publish-latch`), not a new
//! one this change introduces.
//!
//! `Switch` never actually shows text through this plugin today, but it's
//! still a `TextView` subclass under the hood (`android.widget.Switch extends
//! CompoundButton extends Button extends TextView`), so setting it costs
//! nothing and future-proofs against a later on/off-text builder. Same "pin,
//! not full fidelity" policy as every other row above — a future widening to
//! per-slot `TypeScale` family resolution stays additive.
//!
//! ## `Typeface::GlyphMono`/`GlyphPlex` name two slots, not two Glyph faces
//!
//! With step 1 in place both variants mean "custom face slot 0 (button) /
//! slot 1 (body)", whatever bytes were published into them — the Glyph faces
//! are just step 2's occupants. The variants keep their Glyph-era names on
//! purpose: their spellings are the frozen FFI wire strings
//! (`crate::controls::typeface::Typeface::wire`) the Kotlin/ObjC halves
//! decode, and `crate::android::fonts` reuses the same strings as the
//! `face_id` half of its content-hash cache-file name. Renaming the Rust
//! variants alone would leave the two out of step for no functional gain, so
//! the mismatch is documented here instead.
//!
//! ## Publishing: last-pair-wins, not once-per-process
//!
//! [`publish_font_bytes`] crosses the platform seam only when the (button,
//! body) payload pair differs from the pair this process last published
//! ([`PublishGuard`], keyed on each payload's address+length rather than its
//! content — a face is a `&'static [u8]`, so identity is the cheap and exact
//! question). That subsumes the once-per-process publish this module used to
//! do (an unchanging theme re-publishes nothing, every frame) while still
//! letting an app swap in a theme carrying different faces.
//!
//! **Known gap, owed a device gate:** the *platform* halves still latch their
//! first payload (`crate::android::fonts::set_glyph_bytes` is a
//! `OnceLock::set`, and both arms cache each resolved face object
//! process-wide), so a mid-process face swap re-publishes from here but does
//! not re-register on device. Widening that is a platform-half change with
//! its own device gate, not a host-side one.
//!
//! `Image` folds only `dark` — tinting an app-supplied photo from the theme
//! would corrupt its content, and no builder method exposes an explicit tint
//! yet (`crate::api::builders`' own "left for a future task" note).
//!
//! # Explicit backgrounds: closing the light-theme dark-on-dark defect
//! without defeating the ripple
//!
//! L1's night-qualified `Context` ([`crate::android::theme`]) only resolves a
//! control's platform-default background/chrome at CREATE time — Android
//! bakes brightness at construction, unlike iOS which re-pins
//! `overrideUserInterfaceStyle` live on every `update` (a deliberate
//! per-platform asymmetry, `docs/CODE_STANDARDS.md`'s Plugin Conventions).
//! So only an EXPLICIT L2 setter re-applies live on a theme flip (the
//! ordinary Props-diff-then-setter path every property already rides);
//! `Button` already has one (`corner_radius_dp` row above, via
//! `Setter::ThemedBackground`).
//!
//! Folding `surface_bg` into every themed control unconditionally is wrong,
//! though: `android.widget.Switch` (`Widget.Material.CompoundButton.Switch`)
//! and `AbsSeekBar` (`Slider`'s superclass) both carry
//! `?attr/selectableItemBackgroundBorderless` as their platform-default
//! background, which paints the Material touch ripple — a flat
//! `View.setBackgroundColor(int)` (`Setter::BackgroundColor`) REPLACES that
//! drawable outright, silently killing the ripple. So only the controls with
//! nothing to lose get an explicit fold:
//!
//! ## Which controls get an explicit background, and why
//!
//! | Control | Explicit background? | Why |
//! |---|---|---|
//! | `Label` | **Yes** | The only control with visible text, and a `TextView`'s platform-default background is `null` (no ripple, nothing to lose). |
//! | `ProgressBar` | **Yes** | Never clickable, so no ripple to defeat; kept so it reads as part of the page rather than a floating rectangle, matching `examples/glyph-catalog`'s root `AppBackground` fill. |
//! | `Switch` | **No** | `?attr/selectableItemBackgroundBorderless` is its default background — an explicit fill would replace the ripple. Its `thumb_tint`/`track_tint` fold already carries the theme. |
//! | `Slider` | **No** | Same reasoning — `AbsSeekBar` carries the same borderless-ripple background attr. Its `progress_tint`/`thumb_tint` fold already carries the theme. |
//! | `Button` | N/A (already covered) | Gets `Setter::ThemedBackground` (accent-filled) instead — a deliberately opaque fill, not the page's `surface_bg` role. |
//! | `Image` | **No** | A tint or background fill would corrupt app-supplied photo content. |
//!
//! **If a future task wants `Switch`/`Slider` to carry a themed background
//! anyway**, the only acceptable route is a ripple-preserving
//! `RippleDrawable`/`LayerDrawable` setter layering a themed fill UNDER the
//! platform's own ripple foreground — a new [`crate::controls::Setter`]
//! variant, needing its own on-device pass. Do not re-add a bare
//! `Setter::BackgroundColor` fold for either control.
//!
//! `surface_bg` is the same role `examples/glyph-catalog`'s own root
//! `AppBackground` layer paints as the page's base fill, so a native
//! control's background matches the page it sits on rather than floating as
//! a mismatched rectangle; for `Label` specifically it also guarantees
//! contrast with `body_text` (`ColorScheme`'s surface/on_surface pairing is
//! authored to contrast by construction, both brightnesses — see this
//! module's own
//! `text_bearing_controls_pair_a_contrasting_background_and_foreground_in_both_brightnesses`
//! test, covering `Label`/`Button` only). `Image` is excluded for the same
//! tint-corruption reason as above.
//!
//! This folds through the same `Setter::BackgroundColor` (`Tier::Relayout`)
//! every other flat, non-`Button` background already would — no new
//! [`crate::controls::Setter`] variant needed.
//!
//! # Accent-role split and explicit-override precedence
//!
//! This module resolves both `Theme` precedence rules from
//! `docs/CODE_STANDARDS.md`'s Theming conventions. **Accent-role split**:
//! `primary`/`on_primary` is the accent's TEXT/ICON ink; `primary_container`/
//! `on_primary_container` is the bright FILL — a filled control's background
//! is therefore `primary_container` (never bare `primary`), and a small
//! accented part on an otherwise neutral surface (`Switch`/`Slider`'s
//! thumb) is `primary` (never `primary_container`); conflating the two is
//! the catalog's most common accent bug, and this module is the one place
//! this plugin resolves either role. **Precedence**:
//! *explicit builder value > theme > fallback* — no builder method sets a
//! competing explicit colour/radius/size yet, so this module folds theme tokens
//! unconditionally; a future `.text_color()`/`.background_color()` override
//! must thread its value through [`resolve`]'s callers ahead of the theme,
//! not into this module.

use std::sync::{Mutex, PoisonError};

use frust::{Brightness, Color, DesignLanguage, Theme};
use frust_theme::{FontFace, NativeTypefaces};

use crate::controls::typeface::Typeface;

/// Whether `theme`'s active brightness is [`Brightness::Dark`] — L1's input
/// (theme ladder): [`crate::android::theme::night_qualified_context`]
/// wraps a control's construction `Context` off this same bit.
pub(crate) fn is_dark(theme: &Theme) -> bool {
    theme.brightness == Brightness::Dark
}

/// Pack a [`Color`] (`frust::Color`, a straight re-export of `peniko::Color`
/// — used here rather than depending on `peniko` directly, since `frust` is
/// already this feature's required dependency) as the ARGB `u32` every
/// native-widgets colour setter already round-trips (`crate::controls::color`'s
/// doc: "the signed colour int Java uses ... and the unsigned u32 ... differ
/// only above the sign bit").
pub(crate) fn argb_u32(color: Color) -> u32 {
    let [r, g, b, a] = color.to_rgba8().to_u8_array();
    u32::from_be_bytes([a, r, g, b])
}

/// Every token a control's `params_for` may fold in, resolved once per
/// [`Component::build`](frust_core::Component::build) from the active
/// [`Theme`] — see the module doc's mapping table for the *why* of each
/// field. `Copy`: cheap to thread through every builder's `params_for` by
/// value, and the exact shape a host test compares for the "unchanged theme
/// yields `PartialEq`-equal Props" acceptance criterion (identical
/// `ResolvedTheme` values fold into byte-identical `params_json`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ResolvedTheme {
    /// L1's input — see [`is_dark`].
    pub(crate) dark: bool,
    /// `scheme().primary` — accent TEXT/ICON ink (module doc's accent-role
    /// split).
    pub(crate) accent_ink: u32,
    /// `scheme().primary_container` — the bright accent FILL.
    pub(crate) accent_fill: u32,
    /// `scheme().on_primary_container` — ink atop [`Self::accent_fill`].
    pub(crate) on_accent_fill: u32,
    /// `scheme().on_surface` — ordinary body-text ink.
    pub(crate) body_text: u32,
    /// `scheme().surface` — the page-background role a native control's
    /// EXPLICIT background resolves to (module doc's *Explicit backgrounds*
    /// section): only `Label`/
    /// `ProgressBar`'s background, so it re-paints live on a brightness flip
    /// instead of pinning to L1's creation-time `Context` the way an unset
    /// (platform default) background would. Deliberately NOT folded into
    /// `Switch`/`Slider` (module doc's *Which controls get an explicit
    /// background* table) — both carry a ripple-painting platform-default
    /// background (`?attr/selectableItemBackgroundBorderless`) an explicit
    /// fill would replace. Not used by `Button` (already has its own
    /// accent-filled `accent_fill` background) or `Image` (module doc's
    /// no-tint rule).
    pub(crate) surface_bg: u32,
    /// `shape.small`, dp.
    pub(crate) corner_radius_dp: f32,
    /// `type_scale.label_large.size`, sp.
    pub(crate) button_text_size_sp: f32,
    /// `type_scale.body_large.size`, sp.
    pub(crate) body_text_size_sp: f32,
    /// `Button`'s `Typeface` (theme ladder L3) — see the module doc's
    /// *typography, gated on `design_language`* section.
    pub(crate) button_typeface: Typeface,
    /// `Label`/`Switch`'s `Typeface` (theme ladder L3) — see the
    /// module doc's *typography, gated on `design_language`* section.
    pub(crate) body_typeface: Typeface,
}

/// Resolve `theme` into the packed primitives every builder folds into its
/// control's `params_json` — see the module doc's mapping table.
pub(crate) fn resolve(theme: &Theme) -> ResolvedTheme {
    let (button, body) = plan_typefaces(theme);
    publish_font_bytes(button.bytes, body.bytes);
    let scheme = theme.scheme();
    ResolvedTheme {
        dark: is_dark(theme),
        accent_ink: argb_u32(scheme.primary),
        accent_fill: argb_u32(scheme.primary_container),
        on_accent_fill: argb_u32(scheme.on_primary_container),
        body_text: argb_u32(scheme.on_surface),
        surface_bg: argb_u32(scheme.surface),
        corner_radius_dp: theme.shape.small as f32,
        button_text_size_sp: theme.type_scale.label_large.size,
        body_text_size_sp: theme.type_scale.body_large.size,
        button_typeface: button.typeface,
        body_typeface: body.typeface,
    }
}

/// One typeface slot's resolution: which [`Typeface`] the control's props
/// carry, and which face bytes that slot publishes across the platform seam.
///
/// Pure data, like every [`Setter`](crate::controls::Setter) plan elsewhere
/// in this crate, so the whole ladder is host-testable with no FFI involved
/// ([`plan_typefaces`] is the pure producer; [`resolve`] is the one caller
/// that publishes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ResolvedFace {
    /// The slot's selected face — see [`resolve_slot`].
    typeface: Typeface,
    /// The bytes to publish into this slot. Empty when nothing at all is
    /// available for it (no attached face, no bundled Glyph bytes), in which
    /// case [`Self::typeface`] is [`Typeface::System`] and the slot is never
    /// asked to register them.
    bytes: &'static [u8],
}

/// Both typeface slots, resolved through the module doc's extension-first
/// ladder — pure; [`resolve`] publishes the result.
fn plan_typefaces(theme: &Theme) -> (ResolvedFace, ResolvedFace) {
    let faces = theme.extension::<NativeTypefaces>();
    let is_glyph = theme.design_language == DesignLanguage::Glyph;
    let (glyph_mono, glyph_plex) = glyph_font_bytes();
    (
        resolve_slot(
            faces.and_then(|f| f.button),
            glyph_mono,
            is_glyph,
            Typeface::GlyphMono,
        ),
        resolve_slot(
            faces.and_then(|f| f.body),
            glyph_plex,
            is_glyph,
            Typeface::GlyphPlex,
        ),
    )
}

/// One slot of the module doc's ladder: an attached [`FontFace`] wins; else
/// the bundled Glyph face, but only when the active theme actually *is*
/// Glyph; else the platform's own face.
///
/// `slot` is the wire-level slot this face would occupy
/// ([`Typeface::GlyphMono`] for button, [`Typeface::GlyphPlex`] for body —
/// see the module doc on why those names outlived their Glyph-only meaning).
/// The `System` arm publishes **no bytes at all** (`&[]`), whatever bundled
/// Glyph bytes exist — see the module doc's *System publishes nothing*
/// section for why an earlier ride-along here was a real bug, not a
/// harmless belt-and-braces default. With `frust-theme`'s bundled bytes
/// absent (this crate's `glyph-fonts` feature off) step 2 has nothing to
/// apply, so a Glyph theme with no attached face lands on `System` too — the
/// same face the platform half would have degraded to on finding no
/// published bytes.
fn resolve_slot(
    attached: Option<FontFace>,
    glyph: Option<&'static [u8]>,
    is_glyph: bool,
    slot: Typeface,
) -> ResolvedFace {
    match (attached, glyph) {
        (Some(face), _) => ResolvedFace {
            typeface: slot,
            bytes: face.bytes,
        },
        (None, Some(bytes)) if is_glyph => ResolvedFace {
            typeface: slot,
            bytes,
        },
        (None, _) => ResolvedFace {
            typeface: Typeface::System,
            bytes: &[],
        },
    }
}

/// `frust-theme`'s embedded Glyph faces — the ladder's step 2, and the one
/// place this crate names `frust_theme::glyph` (the api→runtime seam
/// `crate::android::fonts`'s module doc describes: this crate's `Cargo.toml`
/// allows a `frust-theme` dependency only behind its own `frust-api`
/// feature, and only this module ever names it, so the platform half stays
/// free of it regardless of platform or feature state).
///
/// `frust_theme::glyph::font_data()` always exists — an empty slice with
/// `frust-theme`'s own `glyph-fonts` feature off
/// (`crates/frust-theme/src/glyph/mod.rs`'s own doc) — so this quietly
/// yields `None`s on that configuration rather than panicking on an
/// out-of-bounds index. Indices 0/3 are a documented coupling to
/// `frust-theme`'s own (private) `font_data()` array literal — Space Mono ×3
/// (Regular/Bold/Italic) then IBM Plex Mono ×4
/// (Regular/Medium/SemiBold/Italic), each family's first entry being its
/// Regular face; no public API names a face by weight, so a future reorder
/// there would silently pick a different (but still valid) face, never a
/// panic or crash.
fn glyph_font_bytes() -> (Option<&'static [u8]>, Option<&'static [u8]>) {
    let faces = frust_theme::glyph::font_data();
    (faces.first().copied(), faces.get(3).copied())
}

/// One published payload's identity: its address and length. A face is a
/// `&'static [u8]`, so identity answers "are these the same bytes?" without
/// hashing a megabyte of font data on every frame's resolve.
type FaceKey = (usize, usize);

/// See [`FaceKey`].
fn face_key(bytes: &'static [u8]) -> FaceKey {
    (bytes.as_ptr() as usize, bytes.len())
}

/// The (button, body) payload pair this process last published across the
/// platform seam — see the module doc's *last-pair-wins* section. Held
/// behind a plain `Mutex` (uncontended, allocation-free, taken once per
/// [`resolve`]) rather than four loose atomics, so the pair is read and
/// replaced as one value and can never tear across a swap.
struct PublishGuard {
    last: Mutex<Option<(FaceKey, FaceKey)>>,
}

impl PublishGuard {
    /// A guard that has published nothing yet.
    const fn new() -> Self {
        Self {
            last: Mutex::new(None),
        }
    }

    /// Record `button`/`body` as this process's current pair, reporting
    /// whether it differs from the previous one — i.e. whether the caller
    /// should actually cross the FFI seam. Swapping back to an
    /// earlier pair republishes (this is "last pair", not "every pair ever
    /// seen").
    ///
    /// Nothing under this lock can panic, so poisoning is unreachable; it is
    /// still recovered rather than unwrapped, per this crate's no-panic-near-
    /// FFI rule.
    fn take_if_changed(&self, button: &'static [u8], body: &'static [u8]) -> bool {
        let pair = (face_key(button), face_key(body));
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        if *last == Some(pair) {
            return false;
        }
        *last = Some(pair);
        true
    }
}

/// See [`PublishGuard`].
static PUBLISHED: PublishGuard = PublishGuard::new();

/// Publish this resolve's face payloads to the platform backend (theme
/// ladder L3), skipping the seam entirely when they match the last published
/// pair. Two payloads with nothing in either (no attached faces, no bundled
/// Glyph bytes) publish nothing at all — there would be nothing to register.
fn publish_font_bytes(button: &'static [u8], body: &'static [u8]) {
    if (button.is_empty() && body.is_empty()) || !PUBLISHED.take_if_changed(button, body) {
        return;
    }
    set_platform_font_bytes(button, body);
}

/// Android's half of [`publish_font_bytes`] — `crate::android::fonts` reads
/// the two payloads back as slot 0 (`glyphMono`) / slot 1 (`glyphPlex`).
#[cfg(target_os = "android")]
fn set_platform_font_bytes(button: &'static [u8], body: &'static [u8]) {
    crate::android::fonts::set_glyph_bytes(button, body);
}

/// iOS half of the same publish (theme ladder L3) —
/// `crate::apple::fonts::set_glyph_bytes` mirrors the Android call above
/// exactly (same two slots, same publish contract).
#[cfg(target_os = "ios")]
fn set_platform_font_bytes(button: &'static [u8], body: &'static [u8]) {
    crate::apple::fonts::set_glyph_bytes(button, body);
}

/// No other platform backend reads the published bytes at all (desktop
/// preview, wasm) — a no-op here rather than a platform-module reference
/// neither configuration can compile.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn set_platform_font_bytes(_button: &'static [u8], _body: &'static [u8]) {}

#[cfg(test)]
mod tests {
    use super::*;

    // --- mapping-table snapshots, both brightnesses (real, source-cited
    // Glyph hex values — not a tautological re-derivation of `resolve`
    // itself) --------------------------------------------------------------

    #[test]
    fn resolve_snapshots_the_glyph_dark_baseline() {
        // Glyph dark (`crates/frust-theme/src/glyph/color.rs`'s
        // `ColorScheme::glyph_dark`): primary == primary_container == the
        // amber fill (dark's accent split collapses text/fill to the same
        // hex), on_primary_container the near-black amber ink, on_surface
        // the warm-white body ink.
        let theme = Theme::glyph_baseline();
        assert_eq!(theme.brightness, Brightness::Dark, "dark-first default");

        let tokens = resolve(&theme);
        assert!(tokens.dark);
        assert_eq!(tokens.accent_ink, 0xFFFF_B627, "primary (accent ink)");
        assert_eq!(tokens.accent_fill, 0xFFFF_B627, "primary_container (fill)");
        assert_eq!(
            tokens.on_accent_fill, 0xFF24_1A04,
            "on_primary_container (ink atop the fill)"
        );
        assert_eq!(tokens.body_text, 0xFFF2_EAD9, "on_surface (body ink)");
        assert_eq!(tokens.surface_bg, 0xFF16_1A23, "surface (bg-surface, dark)");
        assert_eq!(tokens.corner_radius_dp, 6.0, "shape.small (Glyph)");
        assert_eq!(tokens.button_text_size_sp, 12.5, "type_scale.label_large");
        assert_eq!(tokens.body_text_size_sp, 13.0, "type_scale.body_large");
        assert_eq!(tokens.button_typeface, Typeface::GlyphMono);
        assert_eq!(tokens.body_typeface, Typeface::GlyphPlex);
    }

    #[test]
    fn resolve_snapshots_the_glyph_light_variant() {
        // Glyph light (`ColorScheme::glyph_light`): the accent role split
        // pulls apart here — primary (text ink) darkens for AA on paper
        // while primary_container (fill) stays the same bright amber.
        let theme = Theme::glyph_baseline().with_brightness(Brightness::Light);

        let tokens = resolve(&theme);
        assert!(!tokens.dark);
        assert_eq!(
            tokens.accent_ink, 0xFFA3_650A,
            "primary (accent ink, AA-darkened)"
        );
        assert_eq!(
            tokens.accent_fill, 0xFFFF_B627,
            "primary_container (fill, unchanged from dark)"
        );
        assert_eq!(
            tokens.on_accent_fill, 0xFF2A_1C04,
            "on_primary_container (ink atop the fill)"
        );
        assert_eq!(tokens.body_text, 0xFF22_1D12, "on_surface (body ink)");
        assert_eq!(
            tokens.surface_bg, 0xFFFF_FFFF,
            "surface (bg-surface, light — the lightest slot)"
        );
        // Shape/type scales don't vary by brightness.
        assert_eq!(tokens.corner_radius_dp, 6.0);
        assert_eq!(tokens.button_text_size_sp, 12.5);
        assert_eq!(tokens.body_text_size_sp, 13.0);
        // Neither does the typeface choice — `design_language`, not
        // `brightness`, gates it (module doc's *typography* section).
        assert_eq!(tokens.button_typeface, Typeface::GlyphMono);
        assert_eq!(tokens.body_typeface, Typeface::GlyphPlex);
    }

    #[test]
    fn non_glyph_baselines_resolve_the_system_typeface() {
        // Imposing Glyph's bundled monospace faces on a Material3/Cupertino
        // theme that never asked for them would be a worse regression than
        // leaving the platform's own face alone (module doc's *typography,
        // gated on `design_language`* section).
        let m3 = resolve(&Theme::m3_baseline());
        assert_eq!(m3.button_typeface, Typeface::System);
        assert_eq!(m3.body_typeface, Typeface::System);

        let cupertino = resolve(&Theme::cupertino_baseline());
        assert_eq!(cupertino.button_typeface, Typeface::System);
        assert_eq!(cupertino.body_typeface, Typeface::System);
    }

    // --- theme ladder L3: the extension-first typeface ladder ---------------

    static DISPLAY_FACE: &[u8] = b"design-system-display-face";
    static BODY_FACE: &[u8] = b"design-system-body-face";

    /// A theme carrying an attached [`NativeTypefaces`] over `base`.
    fn with_faces(base: Theme, faces: NativeTypefaces) -> Theme {
        Theme::builder(base).extension(faces).build()
    }

    #[test]
    fn an_attached_extension_beats_the_glyph_shortcut_and_the_system_default() {
        // Step 1 of the module doc's ladder, on a theme that would otherwise
        // land on step 3 (Material3 tag, no bundled faces of its own).
        let theme = with_faces(
            Theme::m3_baseline(),
            NativeTypefaces {
                button: Some(FontFace::new("Acme Display", DISPLAY_FACE)),
                body: Some(FontFace::new("Acme Text", BODY_FACE)),
            },
        );

        let (button, body) = plan_typefaces(&theme);
        assert_eq!(button.typeface, Typeface::GlyphMono, "custom slot 0");
        assert_eq!(body.typeface, Typeface::GlyphPlex, "custom slot 1");
        assert_eq!(button.bytes, DISPLAY_FACE);
        assert_eq!(body.bytes, BODY_FACE);

        // And it reaches the props every builder folds, not just the plan.
        let tokens = resolve(&theme);
        assert_eq!(tokens.button_typeface, Typeface::GlyphMono);
        assert_eq!(tokens.body_typeface, Typeface::GlyphPlex);
    }

    #[test]
    fn an_attached_extension_displaces_the_bundled_glyph_faces() {
        // Same step 1, but over the one baseline that DOES have step-2 faces
        // — the extension still wins, and the published payload is the design
        // system's, not Glyph's.
        let glyph_bytes = glyph_font_bytes();
        let theme = with_faces(
            Theme::glyph_baseline(),
            NativeTypefaces::uniform(FontFace::new("Acme Text", BODY_FACE)),
        );

        let (button, body) = plan_typefaces(&theme);
        assert_eq!(button.bytes, BODY_FACE);
        assert_eq!(body.bytes, BODY_FACE);
        assert_ne!(
            Some(button.bytes),
            glyph_bytes.0,
            "an attached face must displace Space Mono, not sit behind it"
        );
    }

    #[test]
    fn a_half_filled_extension_falls_back_per_slot() {
        // The ladder runs per slot: the filled one takes step 1, the empty
        // one carries on to step 2 (Glyph theme) or step 3 (Material3).
        let faces = NativeTypefaces {
            button: Some(FontFace::new("Acme Display", DISPLAY_FACE)),
            ..NativeTypefaces::default()
        };

        let (button, body) = plan_typefaces(&with_faces(Theme::m3_baseline(), faces));
        assert_eq!(button.typeface, Typeface::GlyphMono);
        assert_eq!(button.bytes, DISPLAY_FACE);
        assert_eq!(body.typeface, Typeface::System, "step 3 for the body slot");
        assert_eq!(
            body.bytes,
            &[] as &[u8],
            "the empty slot publishes no bytes — the one-sided pair \
             (DISPLAY_FACE, &[]) still crosses the platform seam since it \
             isn't the all-empty case, but nothing ever asks the platform \
             half to register GlyphPlex off it because this same resolve \
             already picked System for the body slot"
        );

        let (button, body) = plan_typefaces(&with_faces(Theme::glyph_baseline(), faces));
        assert_eq!(button.bytes, DISPLAY_FACE, "still step 1");
        assert_eq!(
            body.typeface,
            Typeface::GlyphPlex,
            "step 2 for the body slot: Glyph's own bundled IBM Plex Mono"
        );
        assert_eq!(
            body.bytes,
            glyph_font_bytes()
                .1
                .expect("bundled with the `glyph-fonts` feature, on by default")
        );
    }

    #[test]
    fn an_empty_extension_resolves_exactly_like_no_extension_at_all() {
        // `NativeTypefaces::default()` attaches the type without filling
        // either slot — the ladder must treat that as "nothing attached".
        let attached = plan_typefaces(&with_faces(
            Theme::m3_baseline(),
            NativeTypefaces::default(),
        ));
        assert_eq!(attached, plan_typefaces(&Theme::m3_baseline()));

        let attached = plan_typefaces(&with_faces(
            Theme::glyph_baseline(),
            NativeTypefaces::default(),
        ));
        assert_eq!(attached, plan_typefaces(&Theme::glyph_baseline()));
    }

    #[test]
    fn resolve_slot_covers_all_three_ladder_steps() {
        let attached = FontFace::new("Acme Display", DISPLAY_FACE);
        let bundled: &'static [u8] = b"bundled-glyph-face";

        // 1. attached face wins, whatever the design language.
        for is_glyph in [true, false] {
            assert_eq!(
                resolve_slot(Some(attached), Some(bundled), is_glyph, Typeface::GlyphMono),
                ResolvedFace {
                    typeface: Typeface::GlyphMono,
                    bytes: DISPLAY_FACE,
                }
            );
        }

        // 2. no attached face + a Glyph theme: the bundled face.
        assert_eq!(
            resolve_slot(None, Some(bundled), true, Typeface::GlyphMono),
            ResolvedFace {
                typeface: Typeface::GlyphMono,
                bytes: bundled,
            }
        );

        // 3. no attached face and not a Glyph theme: the platform's own —
        // and it publishes NO bytes. An earlier revision let the bundled
        // Glyph bytes ride along here, which meant a plain
        // Material3/no-extension theme published a non-empty pair on its
        // first resolve and permanently latched Glyph's faces platform-side
        // — see the module doc's *System publishes nothing* section.
        assert_eq!(
            resolve_slot(None, Some(bundled), false, Typeface::GlyphMono),
            ResolvedFace {
                typeface: Typeface::System,
                bytes: &[],
            },
            "System must not carry bundled bytes along — nothing selects \
             them, and doing so latches them process-wide via the platform \
             halves' first-publish-wins OnceLock"
        );
        // …and with nothing bundled either, step 2 cannot apply at all —
        // same empty-bytes outcome.
        assert_eq!(
            resolve_slot(None, None, true, Typeface::GlyphMono),
            ResolvedFace {
                typeface: Typeface::System,
                bytes: &[],
            }
        );
    }

    // --- the publish guard (theme swap) --------------------------------------

    #[test]
    fn the_publish_guard_crosses_the_seam_once_per_distinct_pair() {
        // A local guard, not the process-global one: the sequence below is
        // the whole contract, and asserting it against shared state would
        // depend on whatever else this test binary already resolved.
        let guard = PublishGuard::new();
        let (mono, plex): (&'static [u8], &'static [u8]) = (DISPLAY_FACE, BODY_FACE);
        let swapped: &'static [u8] = b"a-swapped-in-face";

        assert!(guard.take_if_changed(mono, plex), "first pair publishes");
        assert!(
            !guard.take_if_changed(mono, plex),
            "an unchanged theme re-resolving every frame must not re-cross the \
             FFI seam"
        );
        assert!(
            guard.take_if_changed(swapped, plex),
            "a theme swap that changes only the button face republishes"
        );
        assert!(
            guard.take_if_changed(swapped, swapped),
            "…and so does one that then changes only the body face"
        );
        assert!(
            guard.take_if_changed(mono, plex),
            "swapping back is a change too — this is last-pair-wins, not a \
             set of every pair ever published"
        );
    }

    #[test]
    fn a_no_extension_material3_resolve_publishes_nothing_so_a_later_design_system_gets_the_first_real_publish()
     {
        // With `glyph-fonts` on by default, a plain Material3 theme with no
        // `NativeTypefaces` extension used to publish the non-empty pair
        // (Space Mono, IBM Plex Mono) on its very first resolve — because
        // step 3's ride-along bytes made an otherwise-empty pair look
        // non-empty to `publish_font_bytes`'s empty-pair skip. Since the
        // platform halves latch their FIRST published pair, any native
        // control resolving under a plain Material3/no-extension theme
        // before a design system was installed would permanently latch
        // Glyph's own faces, leaving a later design system's real faces to
        // register correctly host-side but never on device.
        //
        // A local guard, not the process-global one — this replicates
        // `publish_font_bytes`'s exact skip/take-if-changed logic so the
        // sequence is self-contained regardless of what else this test
        // binary already resolved.
        let guard = PublishGuard::new();
        let would_cross_the_seam = |theme: &Theme| {
            let (button, body) = plan_typefaces(theme);
            !(button.bytes.is_empty() && body.bytes.is_empty())
                && guard.take_if_changed(button.bytes, body.bytes)
        };

        assert!(
            !would_cross_the_seam(&Theme::m3_baseline()),
            "a plain Material3 theme with no attached extension must \
             publish nothing at all — the all-empty pair must never cross \
             the platform seam"
        );

        // A design system now attaches its own faces. This must be the
        // FIRST publish that actually reaches the platform half — nothing
        // latched before it.
        let acme = with_faces(
            Theme::m3_baseline(),
            NativeTypefaces::uniform(FontFace::new("Acme Text", BODY_FACE)),
        );
        assert!(
            would_cross_the_seam(&acme),
            "the design system's pair must be the first real publish, \
             unblocked by the earlier no-op Material3 resolve"
        );
    }

    #[test]
    fn face_key_is_stable_per_payload_and_distinct_across_payloads() {
        // The guard's whole cheapness argument: a payload's identity, not a
        // byte compare of a megabyte-scale face on every frame's resolve.
        assert_eq!(face_key(DISPLAY_FACE), face_key(DISPLAY_FACE));
        assert_ne!(face_key(DISPLAY_FACE), face_key(BODY_FACE));
    }

    #[test]
    fn a_theme_swap_reaches_the_props_the_builders_fold() {
        // The end-to-end shape of a live design-system swap: same baseline,
        // different attached faces, and the resolved props follow.
        let acme = with_faces(
            Theme::m3_baseline(),
            NativeTypefaces::uniform(FontFace::new("Acme Text", BODY_FACE)),
        );
        let plain = Theme::m3_baseline();

        assert_eq!(resolve(&acme).body_typeface, Typeface::GlyphPlex);
        assert_eq!(resolve(&plain).body_typeface, Typeface::System);
        assert_eq!(
            resolve(&acme).body_typeface,
            Typeface::GlyphPlex,
            "swapping back is not latched by the publish guard"
        );
    }

    // --- the zero-FFI property: an unchanged theme resolves identically ---

    #[test]
    fn an_unchanged_theme_resolves_to_partial_eq_equal_tokens() {
        let theme = Theme::glyph_baseline();
        assert_eq!(
            resolve(&theme),
            resolve(&theme),
            "resolving the same Theme value twice must produce PartialEq-equal \
             ResolvedTheme — the whole-struct gate that keeps an unchanged theme \
             off the FFI boundary depends on it"
        );
    }

    #[test]
    fn argb_u32_matches_the_wire_s_established_packing() {
        // Matches `crate::controls::color`'s own round-trip doc/test: the
        // unsigned spelling of opaque black-alpha-full-red is
        // 0xFFFF0000 == 4_294_901_760.
        let red = Color::from_rgb8(0xFF, 0x00, 0x00);
        assert_eq!(argb_u32(red), 0xFFFF_0000);
    }

    // --- The light-theme dark-on-dark regression guard ---------------------

    /// WCAG 2.x relative luminance, `[0.0, 1.0]`, of a packed ARGB colour —
    /// test-only, used solely to compute [`contrast_ratio`] below.
    fn relative_luminance(argb: u32) -> f64 {
        let [_a, r, g, b] = argb.to_be_bytes();
        let channel = |raw: u8| {
            let c = f64::from(raw) / 255.0;
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
    }

    /// WCAG contrast ratio between two packed ARGB colours (`>= 1.0`; `1.0`
    /// is identical colours, no contrast at all) — the standard
    /// `(lighter + 0.05) / (darker + 0.05)` formula, order-independent.
    fn contrast_ratio(a: u32, b: u32) -> f64 {
        let (la, lb) = (relative_luminance(a), relative_luminance(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn text_bearing_controls_pair_a_contrasting_background_and_foreground_in_both_brightnesses() {
        // An earlier device gate found flipping to a light theme left `Label`'s
        // background pinned dark (L1, baked at creation) while its text
        // colour (L2) followed the theme live, landing dark text on a dark
        // background — unreadable. A bare `!=` check (the test style
        // used elsewhere in this crate) would NOT catch this: two colours can
        // differ and still both be dark. A WCAG contrast-ratio floor would —
        // this is that test, for every control this crate actually renders
        // visible text on (`Button`, `Label`), across BOTH brightnesses.
        const MIN_CONTRAST: f64 = 4.5; // WCAG AA, normal text

        for brightness in [Brightness::Dark, Brightness::Light] {
            let theme = Theme::glyph_baseline().with_brightness(brightness);
            let t = resolve(&theme);

            let label = contrast_ratio(t.body_text, t.surface_bg);
            assert!(
                label >= MIN_CONTRAST,
                "{brightness:?}: Label's body_text {:#010x} over surface_bg \
                 {:#010x} contrasts only {label:.2}:1 (need >= {MIN_CONTRAST}:1) \
                 — exactly the dark-on-dark defect VERIFY-P1.md bar 3 found",
                t.body_text,
                t.surface_bg
            );

            // Button already had an explicit background
            // (`Setter::ThemedBackground`) — pinned here so a future change
            // can't silently regress the one control this bug never hit.
            let button = contrast_ratio(t.on_accent_fill, t.accent_fill);
            assert!(
                button >= MIN_CONTRAST,
                "{brightness:?}: Button's on_accent_fill {:#010x} over \
                 accent_fill {:#010x} contrasts only {button:.2}:1 (need >= \
                 {MIN_CONTRAST}:1)",
                t.on_accent_fill,
                t.accent_fill
            );
        }
    }
}
