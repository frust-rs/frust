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
//! | `accent_ink` | `scheme().primary` | `Switch`/`Slider` thumb tint, `Spinner` tint, `Stepper` tint (`UIStepper.tintColor` only — `NSStepper` exposes no tint property at all, logged and no-op'd, `crate::controls::stepper`'s module doc's *Tint* section) |
//! | `accent_fill` | `scheme().primary_container` | `Button` background, `Switch` track tint, `Slider`/`ProgressBar` progress tint, `Segmented` selected-segment tint (`UISegmentedControl.selectedSegmentTintColor` on iOS, `NSSegmentedControl.selectedSegmentBezelColor` on macOS; no Android arm) |
//! | `on_accent_fill` | `scheme().on_primary_container` | `Button` text colour |
//! | `body_text` | `scheme().on_surface` | `Label` text colour |
//! | `surface_bg` | `scheme().surface` | `Label`/`ProgressBar` background (explicit — see *Explicit backgrounds* below for why `Switch`/`Slider` are deliberately excluded) |
//! | `corner_radius_dp` | `shape.small` | `Button` background (via a `GradientDrawable`) |
//! | `button_text_size_sp` | `type_scale.label_large.size` | `Button` text size |
//! | `body_text_size_sp` | `type_scale.body_large.size` | `Label` text size |
//! | `dark` | `brightness == Brightness::Dark` | every control (L1's `Context` qualification) |
//! | `button_typeface` | `NativeTypefaces::button` ⇒ that face, else the platform's own | `Button` `Typeface` (theme ladder L3) |
//! | `body_typeface` | `NativeTypefaces::body` ⇒ that face, else the platform's own | `Label`/`Switch` `Typeface` (theme ladder L3) |
//!
//! # Theme ladder L3: typography, extension-first
//!
//! Unlike every other row above (folded unconditionally from whichever
//! `Theme` is active), the two typeface rows resolve through a two-step
//! ladder, **independently per slot** ([`resolve_slot`]):
//!
//! 1. **[`frust_theme::NativeTypefaces`]**, the theme extension a design
//!    system attaches to carry its own faces (see that type's module doc — no
//!    CORE baseline attaches it). Its `button`/`body` face bytes are published
//!    straight through this module's two-payload platform seam and selected
//!    for that slot.
//! 2. **[`typeface::Typeface::System`]** — the platform's own face, for a
//!    slot with no attached face. This arm publishes **no bytes** (`&[]`) —
//!    see *System publishes nothing* below.
//!
//! **There is no design-language shortcut, and deliberately so.** A design
//! system's fonts reach a native control through `NativeTypefaces` and
//! nothing else: [`Theme::design_language`] is an identity tag this module
//! never reads. `frust-glyph`'s baseline attaches the extension (its
//! `tokens::native_typefaces()`), which is the whole of how its monospace
//! faces reach `Button`/`Label`; a design system that attaches nothing gets
//! the platform's own face, which is the correct outcome rather than a gap.
//!
//! ## System publishes nothing
//!
//! The `System` arm publishes the empty payload rather than any fallback
//! bytes, and that is load-bearing. The *platform* halves latch their FIRST
//! published pair ([`crate::android::fonts::set_glyph_bytes`], mirrored on
//! iOS and macOS by `crate::coretext`), so any non-empty pair crossing the
//! seam before a design system is installed would permanently latch those
//! bytes: the design system's later, real publish would register correctly
//! host-side (this module's own `ResolvedTheme`/props) but the platform half
//! would never re-register the device-side font object, and the device would
//! keep rendering the latched faces. With `System` publishing `&[]`, a
//! no-extension resolve produces the empty pair `(&[], &[])`,
//! [`publish_font_bytes`] skips it entirely (its own empty-pair
//! short-circuit), and nothing latches — so a design system installed
//! afterward gets the first real publish and registers correctly.
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
//! for that same slot (e.g. the design system later fills the body slot too);
//! that is exactly the existing first-publish-latch gap (see *Publishing*
//! below and `docs/LIMITATIONS.md`'s `native-typeface-first-publish-latch`),
//! not a new one.
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
//! Both variants mean "custom face slot 0 (button) / slot 1 (body)", whatever
//! bytes were published into them — the Glyph faces are merely the first
//! occupants shipped. The variants keep their Glyph-era names on purpose:
//! their spellings are the frozen FFI wire strings
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

use frust::{Brightness, Color, Theme};
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
    /// *typography, extension-first* section.
    pub(crate) button_typeface: Typeface,
    /// `Label`/`Switch`'s `Typeface` (theme ladder L3) — see the
    /// module doc's *typography, extension-first* section.
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
    /// The bytes to publish into this slot. Empty when no face is attached
    /// for it, in which case [`Self::typeface`] is [`Typeface::System`] and
    /// the slot is never asked to register them.
    bytes: &'static [u8],
}

/// Both typeface slots, resolved through the module doc's extension-first
/// ladder — pure; [`resolve`] publishes the result.
fn plan_typefaces(theme: &Theme) -> (ResolvedFace, ResolvedFace) {
    let faces = theme.extension::<NativeTypefaces>();
    (
        resolve_slot(faces.and_then(|f| f.button), Typeface::GlyphMono),
        resolve_slot(faces.and_then(|f| f.body), Typeface::GlyphPlex),
    )
}

/// One slot of the module doc's ladder: an attached [`FontFace`] wins; else
/// the platform's own face.
///
/// `slot` is the wire-level slot this face would occupy
/// ([`Typeface::GlyphMono`] for button, [`Typeface::GlyphPlex`] for body —
/// see the module doc on why those names outlived their Glyph-only meaning).
/// The `System` arm publishes **no bytes at all** (`&[]`) — see the module
/// doc's *System publishes nothing* section for why any ride-along payload
/// here is a real bug, not a harmless belt-and-braces default. There is no
/// design-language arm: a design system's faces reach a native control ONLY
/// through the extension.
fn resolve_slot(attached: Option<FontFace>, slot: Typeface) -> ResolvedFace {
    match attached {
        Some(face) => ResolvedFace {
            typeface: slot,
            bytes: face.bytes,
        },
        None => ResolvedFace {
            typeface: Typeface::System,
            bytes: &[],
        },
    }
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
/// pair. Two payloads with nothing in either (no attached faces) publish
/// nothing at all — there would be nothing to register.
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

/// macOS half of the same publish (theme ladder L3) — straight into
/// `crate::coretext`, the CoreText module both Apple arms share (the iOS
/// call above reaches the very same function through its
/// `crate::apple::fonts` re-export), so the macOS half latches its first
/// published pair exactly like iOS.
#[cfg(target_os = "macos")]
fn set_platform_font_bytes(button: &'static [u8], body: &'static [u8]) {
    crate::coretext::set_glyph_bytes(button, body);
}

/// No other platform backend reads the published bytes at all (Linux/Windows
/// desktop, wasm) — a no-op here rather than a platform-module reference
/// none of those configurations can compile.
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
fn set_platform_font_bytes(_button: &'static [u8], _body: &'static [u8]) {}

#[cfg(test)]
mod tests {
    use super::*;

    // --- mapping-table snapshots: which ColorScheme/ShapeScale/TypeScale
    // source each ResolvedTheme field actually reads. No design system ships
    // in this crate's graph any more, so the fixture below is a theme built
    // with a DISTINCT, recognizable value per source role — which is what
    // makes these assertions a real row-by-row pin of the module doc's
    // mapping table rather than a re-derivation of `resolve` itself. ------

    /// A theme whose every mapped role carries its own distinctive value, so
    /// a row reading the wrong role is caught by the value, not just by a
    /// type check. `light` and `dark` differ role-for-role too, so the
    /// brightness selector is exercised alongside the mapping.
    fn mapped_theme() -> Theme {
        Theme::builder(Theme::neutral())
            .map_colors_light(|c| frust::ColorScheme {
                primary: Color::from_rgb8(0x11, 0x00, 0x00),
                primary_container: Color::from_rgb8(0x22, 0x00, 0x00),
                on_primary_container: Color::from_rgb8(0x33, 0x00, 0x00),
                on_surface: Color::from_rgb8(0x44, 0x00, 0x00),
                surface: Color::from_rgb8(0xFF, 0xFF, 0xFF),
                ..c
            })
            .map_colors_dark(|c| frust::ColorScheme {
                primary: Color::from_rgb8(0x00, 0x11, 0x00),
                primary_container: Color::from_rgb8(0x00, 0x22, 0x00),
                on_primary_container: Color::from_rgb8(0x00, 0x33, 0x00),
                on_surface: Color::from_rgb8(0x00, 0x44, 0x00),
                surface: Color::from_rgb8(0x00, 0x00, 0x00),
                ..c
            })
            .map_shape(|s| frust::ShapeScale { small: 6.0, ..s })
            .build()
    }

    #[test]
    fn resolve_maps_each_role_to_its_documented_field_in_dark() {
        let theme = mapped_theme().with_brightness(Brightness::Dark);
        let tokens = resolve(&theme);

        assert!(tokens.dark);
        assert_eq!(tokens.accent_ink, 0xFF00_1100, "primary (accent ink)");
        assert_eq!(
            tokens.accent_fill, 0xFF00_2200,
            "primary_container (the bright fill, never bare primary)"
        );
        assert_eq!(
            tokens.on_accent_fill, 0xFF00_3300,
            "on_primary_container (ink atop the fill)"
        );
        assert_eq!(tokens.body_text, 0xFF00_4400, "on_surface (body ink)");
        assert_eq!(tokens.surface_bg, 0xFF00_0000, "surface (page background)");
        assert_eq!(tokens.corner_radius_dp, 6.0, "shape.small");
        assert_eq!(
            tokens.button_text_size_sp, theme.type_scale.label_large.size,
            "type_scale.label_large"
        );
        assert_eq!(
            tokens.body_text_size_sp, theme.type_scale.body_large.size,
            "type_scale.body_large"
        );
    }

    #[test]
    fn resolve_maps_the_light_scheme_when_the_theme_is_light() {
        // The same rows, off the OTHER scheme — proving `resolve` reads
        // `theme.scheme()` rather than a hardcoded half.
        let theme = mapped_theme().with_brightness(Brightness::Light);
        let tokens = resolve(&theme);

        assert!(!tokens.dark);
        assert_eq!(tokens.accent_ink, 0xFF11_0000);
        assert_eq!(tokens.accent_fill, 0xFF22_0000);
        assert_eq!(tokens.on_accent_fill, 0xFF33_0000);
        assert_eq!(tokens.body_text, 0xFF44_0000);
        assert_eq!(tokens.surface_bg, 0xFFFF_FFFF);
        // Shape/type scales don't vary by brightness.
        assert_eq!(tokens.corner_radius_dp, 6.0);
    }

    #[test]
    fn a_theme_with_no_attached_faces_resolves_the_system_typeface() {
        // The ladder's step 2, and the whole of the no-design-language rule:
        // a theme that attaches no `NativeTypefaces` gets the platform's own
        // face, whatever its `design_language` tag says. Both a plain neutral
        // theme and a `Glyph`-TAGGED one land here — the tag is identity, not
        // a font selector (module doc's *typography, extension-first*).
        let plain = resolve(&Theme::neutral());
        assert_eq!(plain.button_typeface, Typeface::System);
        assert_eq!(plain.body_typeface, Typeface::System);

        let tagged = Theme::builder(Theme::neutral())
            .design_language(frust::DesignLanguage::Glyph)
            .build();
        assert_eq!(tagged.design_language, frust::DesignLanguage::Glyph);
        let tagged = resolve(&tagged);
        assert_eq!(
            tagged.button_typeface,
            Typeface::System,
            "a Glyph TAG with no attached faces selects nothing — only the \
             extension does"
        );
        assert_eq!(tagged.body_typeface, Typeface::System);
    }

    // --- theme ladder L3: the extension-first typeface ladder ---------------

    static DISPLAY_FACE: &[u8] = b"design-system-display-face";
    static BODY_FACE: &[u8] = b"design-system-body-face";

    /// A theme carrying an attached [`NativeTypefaces`] over `base`.
    fn with_faces(base: Theme, faces: NativeTypefaces) -> Theme {
        Theme::builder(base).extension(faces).build()
    }

    #[test]
    fn an_attached_extension_beats_the_system_default() {
        // Step 1 of the module doc's ladder, on a theme that would otherwise
        // land on step 2.
        let theme = with_faces(
            Theme::neutral(),
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
    fn a_half_filled_extension_falls_back_per_slot() {
        // The ladder runs per slot: the filled one takes step 1, the empty
        // one carries on to step 2 — including under a Glyph-TAGGED theme,
        // where no tag-driven shortcut exists to rescue it.
        let faces = NativeTypefaces {
            button: Some(FontFace::new("Acme Display", DISPLAY_FACE)),
            ..NativeTypefaces::default()
        };
        let tagged = Theme::builder(Theme::neutral())
            .design_language(frust::DesignLanguage::Glyph)
            .build();

        for (name, base) in [("neutral", Theme::neutral()), ("glyph-tagged", tagged)] {
            let (button, body) = plan_typefaces(&with_faces(base, faces));
            assert_eq!(button.typeface, Typeface::GlyphMono, "{name}");
            assert_eq!(button.bytes, DISPLAY_FACE, "{name}");
            assert_eq!(
                body.typeface,
                Typeface::System,
                "{name}: step 2 for the unfilled body slot"
            );
            assert_eq!(
                body.bytes,
                &[] as &[u8],
                "{name}: the empty slot publishes no bytes — the one-sided \
                 pair (DISPLAY_FACE, &[]) still crosses the platform seam \
                 since it isn't the all-empty case, but nothing ever asks the \
                 platform half to register GlyphPlex off it because this same \
                 resolve already picked System for the body slot"
            );
        }
    }

    #[test]
    fn an_empty_extension_resolves_exactly_like_no_extension_at_all() {
        // `NativeTypefaces::default()` attaches the type without filling
        // either slot — the ladder must treat that as "nothing attached".
        let attached = plan_typefaces(&with_faces(Theme::neutral(), NativeTypefaces::default()));
        assert_eq!(attached, plan_typefaces(&Theme::neutral()));
    }

    #[test]
    fn resolve_slot_covers_both_ladder_steps() {
        let attached = FontFace::new("Acme Display", DISPLAY_FACE);

        // 1. an attached face wins and occupies its wire slot.
        assert_eq!(
            resolve_slot(Some(attached), Typeface::GlyphMono),
            ResolvedFace {
                typeface: Typeface::GlyphMono,
                bytes: DISPLAY_FACE,
            }
        );

        // 2. no attached face: the platform's own — and it publishes NO
        // bytes. Any ride-along payload here would make an otherwise-empty
        // pair look non-empty to `publish_font_bytes`'s skip and permanently
        // latch those bytes platform-side — see the module doc's *System
        // publishes nothing* section.
        assert_eq!(
            resolve_slot(None, Typeface::GlyphMono),
            ResolvedFace {
                typeface: Typeface::System,
                bytes: &[],
            },
            "System must carry no bytes — nothing selects them, and doing so \
             latches them process-wide via the platform halves' \
             first-publish-wins OnceLock"
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
    fn a_no_extension_resolve_publishes_nothing_so_a_later_design_system_gets_the_first_real_publish()
     {
        // The platform halves latch their FIRST published pair, so any
        // native control resolving under a no-extension theme before a design
        // system is installed must publish nothing at all — otherwise those
        // bytes latch and the design system's real faces would register
        // host-side but never on device.
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
            !would_cross_the_seam(&Theme::neutral()),
            "a theme with no attached extension must publish nothing at all \
             — the all-empty pair must never cross the platform seam"
        );

        // A design system now attaches its own faces. This must be the
        // FIRST publish that actually reaches the platform half — nothing
        // latched before it.
        let acme = with_faces(
            Theme::neutral(),
            NativeTypefaces::uniform(FontFace::new("Acme Text", BODY_FACE)),
        );
        assert!(
            would_cross_the_seam(&acme),
            "the design system's pair must be the first real publish, \
             unblocked by the earlier no-op resolve"
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
            Theme::neutral(),
            NativeTypefaces::uniform(FontFace::new("Acme Text", BODY_FACE)),
        );
        let plain = Theme::neutral();

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
        let theme = mapped_theme();
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
            let theme = Theme::neutral().with_brightness(brightness);
            let t = resolve(&theme);

            let label = contrast_ratio(t.body_text, t.surface_bg);
            assert!(
                label >= MIN_CONTRAST,
                "{brightness:?}: Label's body_text {:#010x} over surface_bg \
                 {:#010x} contrasts only {label:.2}:1 (need >= {MIN_CONTRAST}:1) \
                 — exactly the dark-on-dark defect an earlier device gate found",
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
