//! The six v1 controls (native-widgets Phase 1, PLAN 1.2): `Button`,
//! `Label`, `Switch`, `Slider`, `ProgressBar`, `Image` — each an internal
//! [`NativeWidget`](crate::runtime::NativeWidget) impl over
//! [`crate::runtime`], with every property write a direct platform setter on
//! the main thread.
//!
//! # Two halves per control, one file
//!
//! Each control module is split by *compilation target*, not by file:
//!
//! 1. a **platform-agnostic half** — the typed `Props`, its `decode` out of a
//!    slot's `params_json`, and a pure `plan(old, new)` that turns a props
//!    diff into an ordered list of [`Setter`]s. Compiled everywhere, so
//!    `cargo test` pins every control's diff behaviour on any host with no
//!    JNI at all (this is why the modules live in `src/controls/` rather than
//!    under `src/android/`, which compiles on Android only);
//! 2. a **platform half** — one `mod platform` per target, side by side in
//!    the same file, each holding that platform's `NativeWidget` impl:
//!    - `#[cfg(target_os = "android")] mod platform` (p1-04) — build the
//!      `android.widget.*` view, hand each planned [`Setter`] to
//!      `platform::apply`, retain/release the global refs;
//!    - `#[cfg(target_os = "ios")] mod platform` (p2-02) — build the UIKit
//!      view, apply the **same plan** through typed `objc2-ui-kit` setters,
//!      and let ARC own the references.
//!
//!    Half 1 is shared verbatim: one `Props` definition, one `decode`, one
//!    `plan`, two arms. That is the whole point of the split — a control's
//!    diff behaviour is asserted once, on a host, and both platforms execute
//!    the identical plan.
//!
//! The *only* thing a plan cannot express is view construction, so `create`
//! is written as "apply the plan from the platform's own freshly-constructed
//! state" (`Props::platform_default`) — one code path for create and update,
//! and a `create` that sets nothing when the app asked for the platform
//! defaults.
//!
//! # Property tiers — which setter you call is the whole cost story
//!
//! SPIKE.md's headline finding (Phase 0 spike 1, an optimized `--profile`
//! build, 50 retained `TextView`s, method ids cached): the FFI crossing is
//! not what costs — *which* Android property you set is.
//!
//! | Tier | What it costs | Measured, per call | Setters |
//! |---|---|---|---|
//! | (floor) | the bare JNI crossing (`isEnabled()`) | **~0.14–0.27 µs** | — no setter is cheaper |
//! | [`Tier::Cheap`] | invalidate/repaint only | **~0.8 µs** (`setTextColor`) | [`Setter::Enabled`], [`Setter::TextColor`], [`Setter::ContentDescription`], [`Setter::Checked`], [`Setter::Progress`], [`Setter::Max`], [`Setter::Indeterminate`], the four tint setters |
//! | [`Tier::Relayout`] | `requestLayout()` + a measure/layout pass | **~29 µs** (`setText`) — ~35× a colour set | [`Setter::Text`], [`Setter::TextSizeSp`], [`Setter::BackgroundColor`], [`Setter::ScaleType`], [`Setter::ThemedBackground`] |
//! | [`Tier::Decode`] | bytes → `Bitmap`, allocation + image decode | milliseconds, size-dependent (not micro-benchmarked) | [`Setter::ImageBytes`] |
//!
//! **Per-frame guidance:** ~500 [`Tier::Cheap`] setters per frame ≈ 0.4 ms and
//! fits a 16 ms budget comfortably; the same 500 at [`Tier::Relayout`] is
//! ~14.5 ms and does not. **Event-driven `setText` is fine; per-frame text
//! streaming is not** — a high-rate surface should stream a cheap property
//! (colour, progress, checked) and leave text/size/scale-type to
//! interaction-rate changes. [`Tier::Decode`] never belongs on a frame path
//! at all, which is why [`Setter::ImageBytes`] is emitted only when the bytes'
//! *identity* changed (`crate::controls::image`), never per rebuild.
//!
//! Debug builds are 2–5× worse across the board (SPIKE.md) — only judge these
//! numbers on an optimized build.
//!
//! **The table is Android-measured, and [`Tier`] is deliberately not
//! re-derived per platform.** No equivalent UIKit measurement exists (Phase 0
//! spike 1 ran on Android only), so the iOS arm applies the same plan without
//! claiming the same costs. The *shape* is expected to carry — a UIKit
//! caption/font change invalidates intrinsic content size and re-lays-out the
//! view, a colour change only redisplays it — but the numbers above are not
//! evidence for iOS and must not be cited as if they were. Re-measuring on
//! device is p2-05's business, not a claim this module gets to make.
//!
//! # Field-level diffing is the control's job
//!
//! [`crate::runtime`]'s `Props: PartialEq` gate is **whole-struct**: it only
//! decides whether `update` runs at all. Which *setters* run is decided here,
//! per field, because only a control knows that its text setter costs 35× its
//! colour setter (the table above). Every `plan` therefore emits a setter only
//! for a field that actually changed, in a deterministic order the tests pin.
//!
//! # Controlled components, and the echo the platform sends back
//!
//! `Switch`/`Slider` follow frust's controlled-component convention
//! (`docs/CODE_STANDARDS.md`'s Interaction Semantics): the platform reports a
//! *requested* value through its listener, and `update` writes the
//! app-confirmed value back. Two seams make that safe:
//!
//! - **write-back**: `plan` takes the value the platform last reported
//!   (`observed`), so a props change that leaves the value untouched still
//!   re-asserts it when the platform has drifted;
//! - **echo guard**: a value setter (`setChecked`/`setProgress`) notifies the
//!   platform's own listener **synchronously, on every supported Android
//!   version** (AOSP source: the same call stack as the setter, guarded only
//!   by the widget's own reentrancy flag, never posted or animation-deferred —
//!   see `switch.rs`'s module doc for the full account). `update` runs inside
//!   `crate::runtime::with_runtime`, so that echo re-enters the same
//!   thread-local `RefCell` mid-borrow and is dropped there, before
//!   `NativeWidget::on_event`/each control's `decode_toggled`/`decode_event`
//!   ever see it — the runtime's re-entrancy tolerance is the SOLE guard.
//!   There is no per-instance suppression flag, and therefore nothing a panic
//!   mid-`update` could leave latched. **This safety is incidental, not
//!   designed**: it holds only because
//!   `crate::runtime::NativeRuntime::update_params` — which calls `update` —
//!   is itself always invoked from inside `with_runtime`
//!   (`crate::android`'s `nativeUpdateParams`). A future refactor that moved
//!   `update` outside that borrow would silently remove the only echo
//!   protection this crate has.
//!
//! **On iOS there is no echo to guard at all — and still no iOS-specific
//! machinery.** UIKit's documented rule is that it does not send control
//! events for programmatic changes, so `setOn:animated:`/`setValue:` are not
//! expected to re-enter this crate the way `setChecked` does. That
//! expectation carries two caveats worth keeping honest, and the same
//! `with_runtime` re-entrancy drop above covers both if either bites — see
//! `switch.rs`'s module doc, which is the reference description for this arm
//! too. **Do not add a per-instance suppression flag on either platform**
//! (f2-02 deleted Android's for good reasons; there is nothing to reinstate).

pub(crate) mod button;
pub(crate) mod image;
pub(crate) mod label;
pub(crate) mod progress;
pub(crate) mod slider;
pub(crate) mod switch;
pub(crate) mod typeface;

use std::borrow::Cow;

use crate::NativeWidgetError;
use crate::registry::SlotId;
use crate::runtime::Params;

use self::image::{Fit, ImageBytes};
use self::typeface::Typeface;

// --- the params wire keys ---------------------------------------------------
//
// One definition per key, shared by every control that carries it, so the api
// layer (p1-06) and the decoders here can never drift apart.

/// `"text"` — a `Button`/`Label` caption.
pub(crate) const TEXT: &str = "text";
/// `"enabled"` — `View.setEnabled`; defaults to `true` when absent.
pub(crate) const ENABLED: &str = "enabled";
/// `"textColor"` — packed ARGB (see [`color`]).
pub(crate) const TEXT_COLOR: &str = "textColor";
/// `"textSizeSp"` — text size in scale-independent pixels.
pub(crate) const TEXT_SIZE_SP: &str = "textSizeSp";
/// `"backgroundColor"` — packed ARGB (see [`color`]).
pub(crate) const BACKGROUND_COLOR: &str = "backgroundColor";
/// `"contentDescription"` — the TalkBack label.
pub(crate) const CONTENT_DESCRIPTION: &str = "contentDescription";
/// `"checked"` — `Switch` state.
pub(crate) const CHECKED: &str = "checked";
/// `"value"` — `Slider`/`ProgressBar` position, in the app's own `[min, max]`.
pub(crate) const VALUE: &str = "value";
/// `"min"` — the app-space range floor (mapped away below, see
/// [`slider`]).
pub(crate) const MIN: &str = "min";
/// `"max"` — the app-space range ceiling.
pub(crate) const MAX: &str = "max";
/// `"indeterminate"` — `ProgressBar`'s spinner mode.
pub(crate) const INDETERMINATE: &str = "indeterminate";
/// `"progressTint"` — packed ARGB, `null`-able (see [`Setter::ProgressTint`]).
pub(crate) const PROGRESS_TINT: &str = "progressTint";
/// `"thumbTint"` — packed ARGB, `null`-able.
pub(crate) const THUMB_TINT: &str = "thumbTint";
/// `"trackTint"` — packed ARGB, `null`-able.
pub(crate) const TRACK_TINT: &str = "trackTint";
/// `"tint"` — `Image`'s packed ARGB tint, `null`-able.
pub(crate) const TINT: &str = "tint";
/// `"fit"` — `Image`'s scale-type hint (see [`Fit`]).
pub(crate) const FIT: &str = "fit";
/// `"dark"` — whether the active theme's `Brightness` is `Dark` (theme
/// ladder L1, p1-07): the input to the night-qualified `Context`
/// `crate::android`'s `create_control` builds every control's view against
/// (`crate::android::theme::night_qualified_context`). Read straight off a
/// slot's raw params by `crate::android::theme::brightness_is_dark` *before*
/// any per-control typed decode runs — at that point in `create_control`
/// there is no registered kind yet to decode against — so it is never a
/// field on any control's own `Props`, unlike every other key in this list.
pub(crate) const DARK: &str = "dark";
/// `"cornerRadiusDp"` — [`Setter::ThemedBackground`]'s corner radius, dp
/// (theme ladder L2, p1-07: `button::ButtonProps::corner_radius_dp`).
pub(crate) const CORNER_RADIUS_DP: &str = "cornerRadiusDp";
/// `"typeface"` — [`Setter::Typeface`]'s wire spelling (theme ladder L3,
/// p1-08: `typeface::Typeface::wire`/`decode`).
pub(crate) const TYPEFACE: &str = "typeface";

// --- tiers ------------------------------------------------------------------

/// What one [`Setter`] costs on the platform — the module doc's tier table.
///
/// A tier is a property of the *Android setter*, not of the value: setting a
/// colour is cheap however often you do it, and setting text is expensive even
/// once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tier {
    /// Invalidate/repaint only — **~0.8 µs** measured (`setTextColor`).
    /// Streaming these at frame rate is the sanctioned high-rate path.
    Cheap,
    /// Triggers `requestLayout()` — **~29 µs** measured (`setText`), ~35× a
    /// colour set. Fine per interaction, never per frame.
    Relayout,
    /// Decodes bytes into a platform image — milliseconds. Emitted only on a
    /// bytes-identity change, never per rebuild.
    Decode,
}

// --- the setter plan --------------------------------------------------------

/// One property write a control decided to make: the *plan* a props diff
/// produces, and the unit `platform::apply` turns into a JNI call.
///
/// Modelling the write as a value (rather than calling the platform straight
/// from the diff) is what makes every control's diff behaviour assertable on a
/// host with no JNI — `plan(old, new)` **is** the setter-call plan the tests
/// pin (`docs/DEVELOPMENT.md`'s host gate; the JNI calls themselves are
/// device-gated in p1-10).
///
/// Each variant's doc names the exact Java setter and its [`Tier`]; the
/// module doc's table is the same information grouped by tier.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Setter<'a> {
    /// `TextView.setText(CharSequence)` — **[`Tier::Relayout`]**, ~29 µs: the
    /// text change re-measures the view. The most expensive routine setter in
    /// the set; keep it on interaction paths.
    Text(&'a str),

    /// `View.setEnabled(boolean)` — **[`Tier::Cheap`]**: refreshes the
    /// drawable state and invalidates, no layout.
    Enabled(bool),

    /// `TextView.setTextColor(int)` — **[`Tier::Cheap`]**, the ~0.8 µs
    /// reference setter the whole tier table is calibrated against.
    ///
    /// The value is a packed Android colour int (see [`color`]). A colour can
    /// only be *changed*, never un-set: a `Some` → `None` props transition
    /// plans nothing (the platform has no "restore the default" call).
    TextColor(i32),

    /// `TextView.setTextSize(float)` (scale-independent pixels) —
    /// **[`Tier::Relayout`]**: same class as [`Self::Text`].
    TextSizeSp(f32),

    /// `View.setBackgroundColor(int)` — **[`Tier::Relayout`]** (conservative).
    ///
    /// Cheap *only* when the current background already is a `ColorDrawable`
    /// (then it mutates the colour in place); a control still wearing its
    /// themed `StateListDrawable` — every freshly constructed `Button` —
    /// takes the `setBackground` path instead, which requests a layout when
    /// the new drawable's padding differs. Tiered by the worse branch, and it
    /// also replaces the material ripple, so an app that sets it gets a flat
    /// background by design.
    BackgroundColor(i32),

    /// `View.setContentDescription(CharSequence)` — **[`Tier::Cheap`]**:
    /// stores the label and fires one accessibility event. `None` clears it
    /// (the setter takes `null`).
    ///
    /// Native controls are exposed to TalkBack by the platform itself, never
    /// through frust's semantics pass (`crate`'s event-bypass note).
    ContentDescription(Option<&'a str>),

    /// `CompoundButton.setChecked(boolean)` — **[`Tier::Cheap`]**, *and* the
    /// one setter that calls back into us: it notifies
    /// `OnCheckedChangeListener` synchronously, which is what the runtime's
    /// re-entrancy tolerance exists to drop (module doc).
    Checked(bool),

    /// `ProgressBar.setProgress(int)` — **[`Tier::Cheap`]**. Platform-space:
    /// already offset by the app's `min` (see [`slider`]).
    ///
    /// Must be planned *after* [`Self::Max`] in the same plan — the platform
    /// clamps progress to the current max, so raising both in the other order
    /// silently truncates the value.
    Progress(i32),

    /// `ProgressBar.setMax(int)` — **[`Tier::Cheap`]**. Platform-space span
    /// (`max - min`), since `SeekBar.setMin` needs API 26 and this crate's
    /// floor is 24.
    Max(i32),

    /// `ProgressBar.setIndeterminate(boolean)` — **[`Tier::Cheap`]**: swaps
    /// the progress drawable and invalidates.
    Indeterminate(bool),

    /// `ProgressBar.setProgressTintList(ColorStateList)` —
    /// **[`Tier::Cheap`]**, plus one `ColorStateList.valueOf` allocation per
    /// call (a second crossing). `None` passes `null`, which restores the
    /// platform's own tint — unlike [`Self::TextColor`], tint lists *are*
    /// clearable.
    ProgressTint(Option<i32>),

    /// `AbsSeekBar.setThumbTintList` / `Switch.setThumbTintList` —
    /// **[`Tier::Cheap`]**, same shape as [`Self::ProgressTint`].
    ThumbTint(Option<i32>),

    /// `Switch.setTrackTintList(ColorStateList)` — **[`Tier::Cheap`]**, same
    /// shape as [`Self::ProgressTint`].
    TrackTint(Option<i32>),

    /// `ImageView.setImageTintList(ColorStateList)` — **[`Tier::Cheap`]**,
    /// same shape as [`Self::ProgressTint`].
    ImageTint(Option<i32>),

    /// `ImageView.setScaleType(ImageView.ScaleType)` —
    /// **[`Tier::Relayout`]**: the platform requests a layout on every change.
    ScaleType(Fit),

    /// `BitmapFactory.decodeByteArray` + `ImageView.setImageBitmap(Bitmap)` —
    /// **[`Tier::Decode`]**: milliseconds for a real image.
    ///
    /// Planned **only when the bytes' identity changed** (`Arc` pointer +
    /// publish revision — see [`ImageBytes`]), never per rebuild; empty bytes
    /// plan a `setImageBitmap(null)`, which clears the view.
    ImageBytes(&'a ImageBytes),

    /// A `GradientDrawable` background carrying BOTH a fill colour and a
    /// corner radius — `View.setBackground(Drawable)` — **[`Tier::Relayout`]**,
    /// the same tier and ripple-replacing caveat as [`Self::BackgroundColor`]
    /// (which this variant supersedes whenever a corner radius is also
    /// requested — see `Button`'s `plan`).
    ///
    /// Theme ladder L2 (p1-07): `Button`'s background/corner-radius pinning
    /// ([`crate::api::theme::ResolvedTheme`]'s `accent_fill`/
    /// `corner_radius_dp`). A fill colour and a corner radius can't be two
    /// independent setters the way a colour and a tint list can: Android has
    /// no "just round this `ColorDrawable`'s corners" call, so both values
    /// have to go into ONE freshly built `GradientDrawable` together. `radius_dp`
    /// is in dp, converted to device pixels at apply time
    /// (`crate::android::theme::dp_to_px`) — unlike [`Setter::TextSizeSp`]
    /// (already SP, self-scaling), `GradientDrawable.setCornerRadius` takes
    /// raw pixels.
    ThemedBackground { fill: i32, radius_dp: f32 },

    /// `TextView.setTypeface(Typeface)` — **[`Tier::Relayout`]**: a font swap
    /// changes every glyph's metrics (ascent/descent/advance width), so the
    /// platform re-measures and re-lays-out the view exactly like
    /// [`Self::TextSizeSp`] — never a per-frame setter.
    ///
    /// Theme ladder L3 (p1-08): the resolved
    /// [`typeface::Typeface`] a text-bearing control (`Button`/`Label`/
    /// `Switch`) renders in. [`typeface::Typeface::System`] plans this same
    /// setter with a `null` argument (`crate::android::fonts::typeface_for`
    /// returns `None`), restoring the platform's own face — both the
    /// explicit choice and the registration-failure degrade path
    /// (`crate::android::fonts`'s module doc) land on the exact same call.
    Typeface(Typeface),
}

impl Setter<'_> {
    /// This setter's cost tier — the module doc's table, in code.
    pub(crate) fn tier(&self) -> Tier {
        match self {
            Self::Text(_)
            | Self::TextSizeSp(_)
            | Self::BackgroundColor(_)
            | Self::ScaleType(_)
            | Self::ThemedBackground { .. }
            | Self::Typeface(_) => Tier::Relayout,
            Self::ImageBytes(_) => Tier::Decode,
            Self::Enabled(_)
            | Self::TextColor(_)
            | Self::ContentDescription(_)
            | Self::Checked(_)
            | Self::Progress(_)
            | Self::Max(_)
            | Self::Indeterminate(_)
            | Self::ProgressTint(_)
            | Self::ThumbTint(_)
            | Self::TrackTint(_)
            | Self::ImageTint(_) => Tier::Cheap,
        }
    }
}

/// An ordered list of property writes — what a control's `plan` returns and
/// `platform::apply_all` executes, front to back.
pub(crate) type Plan<'a> = Vec<Setter<'a>>;

// --- shared decode helpers --------------------------------------------------

/// The slot id this params payload names — every control needs it (the
/// interactive ones to construct their listener in `create`, `Image` to find
/// its published bytes).
///
/// # Errors
/// [`NativeWidgetError::Params`] when the reserved identity keys are missing;
/// the runtime has already validated them before a control's `decode_props`
/// runs, so this only fires for a directly-constructed payload (tests).
pub(crate) fn slot_of(params: &Params<'_>) -> Result<SlotId, NativeWidgetError> {
    params.identity().map(|(_, slot)| slot)
}

/// An optional packed-ARGB colour field.
///
/// Both spellings of an Android colour decode identically: the signed colour
/// int Java uses (`-16777216`) and the unsigned `u32` an encoder is more
/// likely to write (`4278190080`) differ only above the sign bit, which the
/// truncation to `i32` discards.
pub(crate) fn color(params: &Params<'_>, key: &str) -> Option<i32> {
    params.int(key).map(|packed| packed as i32)
}

/// An optional owned string field.
pub(crate) fn owned_text(params: &Params<'_>, key: &str) -> Option<String> {
    params.string(key).map(Cow::into_owned)
}

/// A required-with-default string field (a missing caption is an empty one,
/// never a dead slot — the trait's degrade-don't-fail rule).
pub(crate) fn text_or_empty(params: &Params<'_>, key: &str) -> String {
    owned_text(params, key).unwrap_or_default()
}

/// Plan an optional-colour field: emitted when it changed *to* a colour.
///
/// A `Some` → `None` transition plans nothing, because the int-taking colour
/// setters ([`Setter::TextColor`], [`Setter::BackgroundColor`]) have no
/// "restore the platform default" call. The tint setters are `Option`-typed
/// end to end instead (they take a nullable `ColorStateList`), so they diff
/// directly rather than through this helper.
pub(crate) fn plan_color<'a>(
    plan: &mut Plan<'a>,
    old: Option<i32>,
    new: Option<i32>,
    setter: fn(i32) -> Setter<'a>,
) {
    if old != new
        && let Some(argb) = new
    {
        plan.push(setter(argb));
    }
}

/// Split a packed ARGB colour into its four `[0.0, 1.0]` channels, in the
/// `(red, green, blue, alpha)` order every Apple colour constructor takes.
///
/// Platform-neutral and host-tested on purpose, even though only the Apple
/// arm calls it ([`platform::ui_color`] on iOS): the packing convention is
/// *this crate's wire format*, not UIKit's, so it is worth pinning where a
/// plain `cargo test` can see it. The Android arm needs no counterpart —
/// its colour setters take the packed int verbatim.
pub(crate) fn argb_channels(argb: i32) -> (f64, f64, f64, f64) {
    // `as u32` first: the wire carries both spellings of an Android colour
    // (see [`color`]), and shifting a negative `i32` right would sign-extend.
    let packed = argb as u32;
    let channel = |shift: u32| f64::from((packed >> shift) & 0xFF) / 255.0;
    (channel(16), channel(8), channel(0), channel(24))
}

// --- the Android half -------------------------------------------------------

#[cfg(target_os = "android")]
pub(crate) mod platform {
    //! Turning a [`Setter`] into a real JNI call — the one place in this
    //! crate that talks to `android.widget`.
    //!
    //! # Method-id caching
    //!
    //! The hot setters (the ones a per-frame path may touch) resolve their
    //! `JMethodID` **once per process** into [`HOT`] and call through
    //! `Env::call_method_unchecked` — the spike's proven shape (commit
    //! `92b7674`), and the reason its measured floor is ~0.14–0.27 µs per
    //! crossing rather than three crossings plus a string lookup.
    //! `JMethodID` is `Send + Sync` in `jni` 0.22 precisely so it can be
    //! cached this way, and the ids stay valid for the process because the
    //! framework classes they come from are never unloaded.
    //!
    //! The cold setters (tints, scale type, text size, background, content
    //! description — set once per create or per theme change, never per
    //! frame) deliberately keep the **checked** `NativeCtx::call_void` path
    //! instead: its two extra crossings cost ~0.5 µs, and in exchange the
    //! signature is compile-time checked by `jni_sig!` rather than asserted
    //! in a `# Safety` comment.

    use std::sync::OnceLock;

    use jni::objects::{JClass, JMethodID, JObject, JValue};
    use jni::signature::{MethodSignature, Primitive, ReturnType};
    use jni::strings::JNIStr;
    use jni::sys::jvalue;
    use jni::{jni_sig, jni_str};

    use super::{Plan, Setter};
    use crate::NativeWidgetError;
    use crate::android::NativeCtx;

    /// `android.view.View` — the enabled/background/content-description base
    /// every control shares.
    pub(crate) const VIEW_CLASS: &str = "android.view.View";
    /// `android.widget.TextView` — `Label`'s own class and `Button`'s
    /// superclass; the text/colour/size setters resolve here.
    pub(crate) const TEXT_VIEW_CLASS: &str = "android.widget.TextView";
    /// `android.widget.CompoundButton` — `Switch`'s superclass
    /// (`setChecked`).
    pub(crate) const COMPOUND_BUTTON_CLASS: &str = "android.widget.CompoundButton";
    /// `android.widget.ProgressBar` — `ProgressBar`'s own class and
    /// `SeekBar`'s superclass (`setProgress`/`setMax`).
    pub(crate) const PROGRESS_BAR_CLASS: &str = "android.widget.ProgressBar";
    /// `android.widget.ImageView` — `Image`'s class.
    pub(crate) const IMAGE_VIEW_CLASS: &str = "android.widget.ImageView";
    /// `android.content.res.ColorStateList` — every tint setter's argument.
    pub(crate) const COLOR_STATE_LIST_CLASS: &str = "android.content.res.ColorStateList";
    /// `android.graphics.BitmapFactory` — [`Setter::ImageBytes`]'s decoder.
    pub(crate) const BITMAP_FACTORY_CLASS: &str = "android.graphics.BitmapFactory";
    /// `android.widget.ImageView$ScaleType` — [`Setter::ScaleType`]'s enum.
    pub(crate) const SCALE_TYPE_CLASS: &str = "android.widget.ImageView$ScaleType";
    /// `android.graphics.drawable.GradientDrawable` —
    /// [`Setter::ThemedBackground`]'s drawable (theme ladder L2, p1-07).
    pub(crate) const GRADIENT_DRAWABLE_CLASS: &str = "android.graphics.drawable.GradientDrawable";

    /// How many local references a control's create/update frame reserves.
    ///
    /// A whole plan allocates at most two locals per setter (a string, a
    /// `ColorStateList`) and the widest control plans seven, so 16 covers
    /// every case with room to spare — and the frame is what releases them
    /// all on return, instead of pinning them for the whole JNI call
    /// (`NativeCtx`'s local-frame discipline).
    pub(crate) const FRAME_CAPACITY: usize = 16;

    /// The hot setters' cached method ids (module doc). One table for all six
    /// controls: it is seeded on the first control creation of any kind and
    /// costs five class loads plus six `GetMethodID`s, once per process.
    struct HotMethods {
        /// `android.view.View.setEnabled(boolean)`.
        set_enabled: JMethodID,
        /// `android.widget.TextView.setText(java.lang.CharSequence)`.
        set_text: JMethodID,
        /// `android.widget.TextView.setTextColor(int)`.
        set_text_color: JMethodID,
        /// `android.widget.CompoundButton.setChecked(boolean)`.
        set_checked: JMethodID,
        /// `android.widget.ProgressBar.setProgress(int)`.
        set_progress: JMethodID,
        /// `android.widget.ImageView.setImageBitmap(android.graphics.Bitmap)`.
        set_image_bitmap: JMethodID,
    }

    /// See [`HotMethods`].
    static HOT: OnceLock<HotMethods> = OnceLock::new();

    impl HotMethods {
        /// Resolve every hot id, each from the class that declares it (a
        /// subclass instance may be called through a superclass's id — the
        /// JNI rule this table relies on).
        fn resolve(ctx: &mut NativeCtx<'_, '_>) -> Result<Self, NativeWidgetError> {
            let view = ctx.class(VIEW_CLASS)?;
            let set_enabled = method_id(ctx, &view, jni_str!("setEnabled"), jni_sig!("(Z)V"))?;

            let text_view = ctx.class(TEXT_VIEW_CLASS)?;
            let set_text = method_id(
                ctx,
                &text_view,
                jni_str!("setText"),
                jni_sig!("(Ljava/lang/CharSequence;)V"),
            )?;
            let set_text_color =
                method_id(ctx, &text_view, jni_str!("setTextColor"), jni_sig!("(I)V"))?;

            let compound = ctx.class(COMPOUND_BUTTON_CLASS)?;
            let set_checked = method_id(ctx, &compound, jni_str!("setChecked"), jni_sig!("(Z)V"))?;

            let progress_bar = ctx.class(PROGRESS_BAR_CLASS)?;
            let set_progress = method_id(
                ctx,
                &progress_bar,
                jni_str!("setProgress"),
                jni_sig!("(I)V"),
            )?;

            let image_view = ctx.class(IMAGE_VIEW_CLASS)?;
            let set_image_bitmap = method_id(
                ctx,
                &image_view,
                jni_str!("setImageBitmap"),
                jni_sig!("(Landroid/graphics/Bitmap;)V"),
            )?;

            Ok(Self {
                set_enabled,
                set_text,
                set_text_color,
                set_checked,
                set_progress,
                set_image_bitmap,
            })
        }
    }

    /// The process-wide hot-method table, seeded on first use.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when a class or method cannot be
    /// resolved — which for these framework classes means the call path has
    /// no classloader yet (only `createView` carries a `Context`), not a
    /// missing API.
    fn hot(ctx: &mut NativeCtx<'_, '_>) -> Result<&'static HotMethods, NativeWidgetError> {
        if let Some(methods) = HOT.get() {
            return Ok(methods);
        }
        let resolved = HotMethods::resolve(ctx)?;
        Ok(HOT.get_or_init(|| resolved))
    }

    /// `GetMethodID`, with the failing lookup named in the error.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when the method does not exist on
    /// `class`.
    fn method_id<'sig, 'sig_args>(
        ctx: &mut NativeCtx<'_, '_>,
        class: &JClass<'_>,
        name: &JNIStr,
        signature: impl AsRef<MethodSignature<'sig, 'sig_args>>,
    ) -> Result<JMethodID, NativeWidgetError> {
        ctx.run_jni(&format!("GetMethodID({name})"), |env| {
            env.get_method_id(class, name, signature)
        })
    }

    /// Call a cached `void` method.
    ///
    /// # Safety
    /// `method` must be a [`JMethodID`] resolved from a class `view` is an
    /// instance of, it must return `void`, and `args` must match its
    /// descriptor exactly in count and JNI type. Every caller is an arm of
    /// [`apply`] below, each pairing one [`HotMethods`] field with the exact
    /// signature that field's doc comment names — the pairing is the proof.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when the call throws.
    unsafe fn call_void_cached(
        ctx: &mut NativeCtx<'_, '_>,
        op: &str,
        view: &JObject<'_>,
        method: JMethodID,
        args: &[jvalue],
    ) -> Result<(), NativeWidgetError> {
        ctx.run_jni(op, |env| {
            // SAFETY: forwarded verbatim from this function's own contract,
            // which every call site upholds by construction.
            unsafe {
                env.call_method_unchecked(
                    view,
                    method,
                    ReturnType::Primitive(Primitive::Void),
                    args,
                )
            }?;
            Ok(())
        })
    }

    /// Execute a whole [`Plan`], front to back — the order a control planned
    /// them in is load-bearing (see [`Setter::Progress`]).
    ///
    /// # Errors
    /// The first [`NativeWidgetError::Platform`] any setter reported; the
    /// setters before it have already been applied, and the runtime keeps the
    /// previous props as the diff baseline so the rest are retried on the next
    /// differing params.
    pub(crate) fn apply_all(
        ctx: &mut NativeCtx<'_, '_>,
        view: &JObject<'_>,
        plan: &Plan<'_>,
    ) -> Result<(), NativeWidgetError> {
        for setter in plan {
            apply(ctx, view, setter)?;
        }
        Ok(())
    }

    /// Execute one planned property write against `view`.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when the underlying JNI call throws or
    /// a supporting class cannot be loaded.
    pub(crate) fn apply(
        ctx: &mut NativeCtx<'_, '_>,
        view: &JObject<'_>,
        setter: &Setter<'_>,
    ) -> Result<(), NativeWidgetError> {
        match *setter {
            Setter::Text(text) => {
                let value = ctx.run_jni("NewStringUTF(text)", |env| env.new_string(text))?;
                let method = hot(ctx)?.set_text;
                // SAFETY: `set_text` is TextView.setText(CharSequence)V and
                // `view` is a TextView (Label) or Button (a TextView
                // subclass); one object argument, void return.
                unsafe {
                    call_void_cached(
                        ctx,
                        "TextView.setText",
                        view,
                        method,
                        &[JValue::Object(&value).as_jni()],
                    )
                }
            }
            Setter::Enabled(enabled) => {
                let method = hot(ctx)?.set_enabled;
                // SAFETY: `set_enabled` is View.setEnabled(Z)V and every
                // control is a View; one boolean argument, void return.
                unsafe {
                    call_void_cached(
                        ctx,
                        "View.setEnabled",
                        view,
                        method,
                        &[JValue::Bool(enabled).as_jni()],
                    )
                }
            }
            Setter::TextColor(argb) => {
                let method = hot(ctx)?.set_text_color;
                // SAFETY: `set_text_color` is TextView.setTextColor(I)V and
                // `view` is a TextView (or Button); one int, void return.
                unsafe {
                    call_void_cached(
                        ctx,
                        "TextView.setTextColor",
                        view,
                        method,
                        &[JValue::Int(argb).as_jni()],
                    )
                }
            }
            Setter::Checked(checked) => {
                let method = hot(ctx)?.set_checked;
                // SAFETY: `set_checked` is CompoundButton.setChecked(Z)V and
                // `view` is a Switch (a CompoundButton); one boolean, void.
                unsafe {
                    call_void_cached(
                        ctx,
                        "CompoundButton.setChecked",
                        view,
                        method,
                        &[JValue::Bool(checked).as_jni()],
                    )
                }
            }
            Setter::Progress(progress) => {
                let method = hot(ctx)?.set_progress;
                // SAFETY: `set_progress` is ProgressBar.setProgress(I)V and
                // `view` is a ProgressBar or SeekBar (a subclass); one int.
                unsafe {
                    call_void_cached(
                        ctx,
                        "ProgressBar.setProgress",
                        view,
                        method,
                        &[JValue::Int(progress).as_jni()],
                    )
                }
            }
            Setter::TextSizeSp(sp) => ctx.call_void(
                view,
                jni_str!("setTextSize"),
                jni_sig!("(F)V"),
                &[JValue::Float(sp)],
            ),
            Setter::BackgroundColor(argb) => ctx.call_void(
                view,
                jni_str!("setBackgroundColor"),
                jni_sig!("(I)V"),
                &[JValue::Int(argb)],
            ),
            Setter::ContentDescription(Some(label)) => ctx.set_content_description(view, label),
            Setter::ContentDescription(None) => ctx.call_void(
                view,
                jni_str!("setContentDescription"),
                jni_sig!("(Ljava/lang/CharSequence;)V"),
                &[JValue::Object(&JObject::null())],
            ),
            Setter::Max(max) => ctx.call_void(
                view,
                jni_str!("setMax"),
                jni_sig!("(I)V"),
                &[JValue::Int(max)],
            ),
            Setter::Indeterminate(indeterminate) => ctx.call_void(
                view,
                jni_str!("setIndeterminate"),
                jni_sig!("(Z)V"),
                &[JValue::Bool(indeterminate)],
            ),
            Setter::ProgressTint(argb) => {
                tint_list(ctx, view, jni_str!("setProgressTintList"), argb)
            }
            Setter::ThumbTint(argb) => tint_list(ctx, view, jni_str!("setThumbTintList"), argb),
            Setter::TrackTint(argb) => tint_list(ctx, view, jni_str!("setTrackTintList"), argb),
            Setter::ImageTint(argb) => tint_list(ctx, view, jni_str!("setImageTintList"), argb),
            Setter::ScaleType(fit) => {
                let class = ctx.class(SCALE_TYPE_CLASS)?;
                let value = ctx.run_jni("ImageView$ScaleType.<constant>", |env| {
                    env.get_static_field(
                        &class,
                        fit.scale_type_constant(),
                        jni_sig!("Landroid/widget/ImageView$ScaleType;"),
                    )?
                    .l()
                })?;
                ctx.call_void(
                    view,
                    jni_str!("setScaleType"),
                    jni_sig!("(Landroid/widget/ImageView$ScaleType;)V"),
                    &[JValue::Object(&value)],
                )
            }
            Setter::ImageBytes(bytes) => {
                let bitmap = match bytes.as_slice() {
                    Some(raw) => decode_bitmap(ctx, raw)?,
                    // No bytes at all: clear the view rather than leave a
                    // stale image behind.
                    None => JObject::null(),
                };
                if bitmap.is_null() && bytes.as_slice().is_some() {
                    log::warn!(
                        "frust-native-widgets: BitmapFactory could not decode {} image byte(s) \
                         — clearing the view",
                        bytes.len()
                    );
                }
                let method = hot(ctx)?.set_image_bitmap;
                // SAFETY: `set_image_bitmap` is
                // ImageView.setImageBitmap(Bitmap)V and `view` is an
                // ImageView; one object argument (null is legal — it clears),
                // void return.
                unsafe {
                    call_void_cached(
                        ctx,
                        "ImageView.setImageBitmap",
                        view,
                        method,
                        &[JValue::Object(&bitmap).as_jni()],
                    )
                }
            }
            Setter::ThemedBackground { fill, radius_dp } => {
                // Device px: `GradientDrawable.setCornerRadius` takes raw
                // pixels, unlike `setTextSize` (already SP) — see the
                // variant's own doc.
                let radius_px = crate::android::theme::dp_to_px(ctx, radius_dp);
                let class = ctx.class(GRADIENT_DRAWABLE_CLASS)?;
                let drawable = ctx.run_jni("new GradientDrawable()", |env| {
                    env.new_object(&class, jni_sig!("()V"), &[])
                })?;
                ctx.call_void(
                    &drawable,
                    jni_str!("setColor"),
                    jni_sig!("(I)V"),
                    &[JValue::Int(fill)],
                )?;
                ctx.call_void(
                    &drawable,
                    jni_str!("setCornerRadius"),
                    jni_sig!("(F)V"),
                    &[JValue::Float(radius_px)],
                )?;
                ctx.call_void(
                    view,
                    jni_str!("setBackground"),
                    jni_sig!("(Landroid/graphics/drawable/Drawable;)V"),
                    &[JValue::Object(&drawable)],
                )
            }
            Setter::Typeface(typeface) => {
                // `crate::android::fonts::typeface_for` resolves (and
                // process-wide caches) the real `android.graphics.Typeface`
                // for a Glyph face, or reports the registration-failure
                // degrade path with its own one-warning contract — either
                // way `None` here means "restore the platform's own face",
                // `Typeface::System`'s own meaning too (the variant's doc).
                match crate::android::fonts::typeface_for(ctx, typeface) {
                    Some(resolved) => ctx.call_void(
                        view,
                        jni_str!("setTypeface"),
                        jni_sig!("(Landroid/graphics/Typeface;)V"),
                        &[JValue::Object(resolved)],
                    ),
                    None => ctx.call_void(
                        view,
                        jni_str!("setTypeface"),
                        jni_sig!("(Landroid/graphics/Typeface;)V"),
                        &[JValue::Object(&JObject::null())],
                    ),
                }
            }
        }
    }

    /// `view.<setter>(ColorStateList.valueOf(argb))`, or `<setter>(null)` when
    /// the props cleared the tint.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when `ColorStateList` cannot be loaded
    /// or the setter throws (it does not exist on the view's class).
    fn tint_list(
        ctx: &mut NativeCtx<'_, '_>,
        view: &JObject<'_>,
        setter: &JNIStr,
        argb: Option<i32>,
    ) -> Result<(), NativeWidgetError> {
        let list = match argb {
            Some(argb) => {
                let class = ctx.class(COLOR_STATE_LIST_CLASS)?;
                ctx.run_jni("ColorStateList.valueOf", |env| {
                    env.call_static_method(
                        &class,
                        jni_str!("valueOf"),
                        jni_sig!("(I)Landroid/content/res/ColorStateList;"),
                        &[JValue::Int(argb)],
                    )?
                    .l()
                })?
            }
            None => JObject::null(),
        };
        ctx.call_void(
            view,
            setter,
            jni_sig!("(Landroid/content/res/ColorStateList;)V"),
            &[JValue::Object(&list)],
        )
    }

    /// `BitmapFactory.decodeByteArray(bytes, 0, bytes.len())`.
    ///
    /// Returns a `null` object (not an error) for bytes the platform cannot
    /// decode — that is `decodeByteArray`'s own contract, and a bad payload
    /// should clear the image, not kill the slot.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when the byte array cannot be
    /// allocated, the class cannot be loaded, or the decoder throws.
    fn decode_bitmap<'local>(
        ctx: &mut NativeCtx<'local, '_>,
        bytes: &[u8],
    ) -> Result<JObject<'local>, NativeWidgetError> {
        let length = i32::try_from(bytes.len()).map_err(|_| {
            NativeWidgetError::Platform(format!(
                "image payload of {} bytes exceeds the JNI array limit",
                bytes.len()
            ))
        })?;
        let class = ctx.class(BITMAP_FACTORY_CLASS)?;
        ctx.run_jni("BitmapFactory.decodeByteArray", |env| {
            let array = env.byte_array_from_slice(bytes)?;
            env.call_static_method(
                &class,
                jni_str!("decodeByteArray"),
                jni_sig!("([BII)Landroid/graphics/Bitmap;"),
                &[JValue::Object(&array), JValue::Int(0), JValue::Int(length)],
            )?
            .l()
        })
    }
}

// --- the Apple half ---------------------------------------------------------

#[cfg(target_os = "ios")]
pub(crate) mod platform {
    //! Turning a [`Setter`] into a real UIKit call — the Apple mirror of the
    //! Android half above, and the only place in this crate's iOS arm that
    //! reaches for a shared `objc2-ui-kit` helper.
    //!
    //! # Why there is no single `apply` here, unlike Android
    //!
    //! Android's whole control set descends from one `android.view.View`, and
    //! the interesting setters are declared on *shared superclasses*:
    //! `TextView.setText` serves `Label` **and** `Button`, because a `Button`
    //! IS a `TextView`. That is what lets one `apply` over an untyped
    //! `JObject` serve every control there.
    //!
    //! UIKit has no such spine. `UIButton` is not a `UILabel`, and its caption
    //! is `setTitle:forState:` — a *different selector with a different
    //! argument list* from `UILabel.setText:`. Dispatching [`Setter::Text`]
    //! therefore depends on the receiver's class, which only each control's
    //! own `#[cfg(target_os = "ios")] mod platform` knows (it holds a
    //! `Retained<UIButton>`, not a `Retained<UIView>`). So **each control
    //! applies its own plan against its own typed view**, and this module
    //! holds only what is genuinely shared:
    //!
    //! - the packed-ARGB → [`UIColor`] bridge ([`ui_color`]);
    //! - the setters every control inherits from `UIView`/`NSObject`
    //!   ([`set_background_color`], [`set_accessibility_label`]);
    //! - the three nullable-argument setters objc2 marks `unsafe`, confined
    //!   and documented once (below) rather than re-justified per control;
    //! - the one-time warnings for the setters this arm cannot honour yet.
    //!
    //! Keeping the typed views is not incidental: it is what makes "every
    //! UIKit call main-thread-typed" (PLAN 2.1) a compile-time property
    //! rather than a review promise — every class here is
    //! `#[thread_kind = MainThreadOnly]`, so a call without a
    //! [`MainThreadMarker`] in hand does not compile.
    //!
    //! # Nothing here can fail
    //!
    //! Every function returns `()`, where the Android arm threads a
    //! `Result<(), NativeWidgetError>` through every setter. That asymmetry is
    //! real, not laziness: a JNI call can throw and a class can fail to load,
    //! while an ObjC message send to a statically linked UIKit class has
    //! neither failure mode — there is no exception channel and no
    //! classloader. The one genuinely fallible step on this arm is
    //! `UIImage::imageWithData:` returning nil for undecodable bytes, which is
    //! `crate::controls::image`'s own clear-the-view path, exactly as
    //! `BitmapFactory.decodeByteArray` is on Android.
    //!
    //! # The `unsafe` here, and why objc2 asks for it
    //!
    //! objc2 marks a generated property setter `unsafe` when the ObjC header
    //! leaves its argument's nullability unannotated — the generated doc says
    //! only *"`x` might not allow `None`"*. It is not a memory-safety claim;
    //! it is "the header did not say". Each wrapper below is a **safe**
    //! function that closes exactly that question by construction (a
    //! non-optional parameter, or a documented-nullable one), carrying the
    //! `// SAFETY:` note at the single call site — the same
    //! one-confined-helper shape `docs/CODE_STANDARDS.md` sanctions for the
    //! Android arm's `call_void_cached`.
    //!
    //! [`MainThreadMarker`]: objc2::MainThreadMarker

    use std::sync::Once;

    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_foundation::NSString;
    use objc2_ui_kit::{NSObjectUIAccessibility, UIColor, UIFont, UIImageView, UILabel, UIView};

    use super::{Setter, argb_channels};
    use crate::controls::typeface::Typeface;

    /// A packed ARGB colour as a [`UIColor`] (see [`super::argb_channels`]).
    ///
    /// The four channel values are `CGFloat`s, which is `f64` on every 64-bit
    /// Apple target — i.e. on every target this crate's iOS arm compiles for
    /// (arm64 device, arm64/x86_64 Simulator). A hypothetical 32-bit Apple
    /// target would be a loud type error here, never a silent narrowing.
    pub(crate) fn ui_color(argb: i32) -> Retained<UIColor> {
        let (red, green, blue, alpha) = argb_channels(argb);
        UIColor::colorWithRed_green_blue_alpha(red, green, blue, alpha)
    }

    /// An optional packed ARGB colour. `None` stays `None`, which every tint
    /// setter on this arm passes through as nil — restoring the platform's own
    /// tint, the same clearable contract [`Setter::ProgressTint`] documents.
    pub(crate) fn optional_ui_color(argb: Option<i32>) -> Option<Retained<UIColor>> {
        argb.map(ui_color)
    }

    /// `UIView.backgroundColor` — [`Setter::BackgroundColor`], and the fill
    /// half of [`Setter::ThemedBackground`]. Every control inherits it, so
    /// this takes the `UIView` superclass and lets deref coercion do the rest.
    pub(crate) fn set_background_color(view: &UIView, argb: i32) {
        view.setBackgroundColor(Some(&ui_color(argb)));
    }

    /// `NSObject.accessibilityLabel` — [`Setter::ContentDescription`]'s Apple
    /// counterpart, read by VoiceOver exactly as TalkBack reads Android's
    /// `contentDescription`. `None` clears it (the property is nullable), so a
    /// control falls back to whatever UIKit derives from its own content.
    ///
    /// Native controls are exposed to VoiceOver by the platform itself, never
    /// through frust's semantics pass (`crate`'s event-bypass note).
    pub(crate) fn set_accessibility_label(
        view: &UIView,
        label: Option<&str>,
        mtm: MainThreadMarker,
    ) {
        let text = label.map(NSString::from_str);
        view.setAccessibilityLabel(text.as_deref(), mtm);
    }

    /// The system font at `size_sp` points — [`Setter::TextSizeSp`].
    ///
    /// **`sp` is read as points here, unscaled.** Android's scale-independent
    /// pixel already carries the user's font-size preference; UIKit's
    /// equivalent is Dynamic Type, which is opt-in per font
    /// (`UIFontMetrics`), and wiring it would change the *value* an app's
    /// theme asked for rather than the unit it is expressed in. A control
    /// therefore renders at exactly the size its `Props` named, and Dynamic
    /// Type participation stays a deliberate non-goal of v1.
    pub(crate) fn system_font(size_sp: f32) -> Retained<UIFont> {
        UIFont::systemFontOfSize(f64::from(size_sp))
    }

    /// `UILabel.textColor` — [`Setter::TextColor`] for `Label` (a `Button`
    /// uses the state-keyed `setTitleColor:forState:` instead).
    pub(crate) fn set_label_text_color(label: &UILabel, color: &UIColor) {
        // SAFETY: objc2 marks `setTextColor:` unsafe solely because the header
        // does not annotate whether the argument may be nil. This wrapper's
        // parameter is a non-optional `&UIColor`, so nil is unrepresentable at
        // the call site and the open question is closed by the signature.
        unsafe { label.setTextColor(Some(color)) };
    }

    /// `UILabel.font` — [`Setter::TextSizeSp`] for `Label`, and for a
    /// `Button` via its `titleLabel`.
    pub(crate) fn set_label_font(label: &UILabel, font: &UIFont) {
        // SAFETY: same as `set_label_text_color` — objc2's only stated concern
        // is whether nil is allowed, and this wrapper cannot pass nil.
        unsafe { label.setFont(Some(font)) };
    }

    /// `UIImageView.tintColor` — [`Setter::ImageTint`].
    pub(crate) fn set_image_tint(view: &UIImageView, color: Option<&UIColor>) {
        // SAFETY: objc2's stated concern is whether nil is allowed, and here
        // it is: `UIView.tintColor` is documented as inheriting from the
        // superview when set to nil, which is exactly the "restore the
        // platform's own tint" meaning `Setter::ImageTint(None)` carries.
        unsafe { view.setTintColor(color) };
    }

    /// One-time warning that [`Setter::ThemedBackground`]'s corner radius is
    /// not applied on this arm yet.
    ///
    /// The fill **is** applied (see [`set_background_color`]); only the radius
    /// waits, because rounding a UIKit view means `layer.cornerRadius`, and
    /// `UIView.layer` is gated behind an `objc2-quartz-core` dependency this
    /// crate does not carry yet. Theme ladder L2 on Apple is task p2-04's, and
    /// its own Cargo.toml touch is where that dependency lands — so this is a
    /// scheduled gap with a named owner, not an oversight.
    ///
    /// `Once`-guarded, mirroring
    /// [`crate::controls::typeface::degrade_on_failure`]'s "exactly one
    /// warning" contract: a themed `Button` plans this setter on every create,
    /// so a per-call log would drown the device console.
    pub(crate) fn warn_corner_radius_unsupported(radius_dp: f32) {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            log::warn!(
                "frust-native-widgets: iOS applies a themed background's fill but not its \
                 {radius_dp}dp corner radius yet (needs `layer.cornerRadius`, which task p2-04 \
                 wires up with the theme ladder's Apple arm) — the control renders square"
            );
        });
    }

    /// [`Setter::Typeface`] on this arm: keep the system font, and say so
    /// exactly once when the request was for a real Glyph face.
    ///
    /// [`Typeface::System`] is a genuine no-op here, not a degrade — nothing
    /// on this arm ever changes a control's font *family*, so "restore the
    /// platform's own face" (that variant's whole meaning) is already true.
    ///
    /// A Glyph face is a real request this arm cannot serve yet: resolving one
    /// needs the embedded bytes registered via
    /// `CTFontManagerRegisterFontsForData`, which is theme ladder L3 on Apple
    /// and therefore task p2-04's. Falling back to the system font is exactly
    /// what `crate::android::fonts`' own registration-failure path does
    /// (`crate::controls::typeface`'s [`Typeface::System`] doc: the platform
    /// default and the degrade target are deliberately the same case), so the
    /// visible behaviour is a shape this crate already contracts for.
    ///
    /// `Once`-guarded, mirroring
    /// [`crate::controls::typeface::degrade_on_failure`]'s "exactly one
    /// warning" contract: the Glyph design language is every shell's default,
    /// so a themed `Button`/`Label` plans this setter on every create.
    pub(crate) fn apply_typeface(face: Typeface) {
        if face == Typeface::System {
            return;
        }
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            log::warn!(
                "frust-native-widgets: iOS cannot resolve the Glyph typeface '{}' yet (needs \
                 CTFontManager registration, which task p2-04 lands) — the control keeps the \
                 system font",
                face.wire()
            );
        });
    }

    /// One-time warning that [`Setter::Indeterminate`] has no
    /// `UIProgressView` equivalent — see `crate::controls::progress`'s Apple
    /// module doc for why the gap is a UIKit class split rather than a missing
    /// setter, and what closing it would cost.
    ///
    /// Warns for **either** direction: an app switching a live spinner *off*
    /// gets no visible change on this arm either, so a one-sided warning would
    /// under-report the gap.
    pub(crate) fn warn_indeterminate_unsupported(indeterminate: bool) {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            log::warn!(
                "frust-native-widgets: iOS has no indeterminate UIProgressView (the spinner is a \
                 separate UIActivityIndicatorView class), so `indeterminate: {indeterminate}` is \
                 not applied — the bar keeps showing its determinate value"
            );
        });
    }

    /// A [`Setter`] a control's own `plan` never emits reached its `apply`.
    ///
    /// Only a bug can produce this (a plan and its apply drifting apart), so
    /// it warns rather than degrading silently — but it still does not fail
    /// the call: a mis-planned property is not worth a dead slot
    /// (`crate::controls`'s degrade-don't-fail rule).
    pub(crate) fn warn_unexpected_setter(kind: &str, setter: &Setter<'_>) {
        log::warn!(
            "frust-native-widgets: control '{kind}' planned a setter its iOS arm does not \
             implement ({setter:?}) — ignored"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setter_reports_the_tier_the_module_table_documents() {
        let bytes = ImageBytes::empty();
        let cheap = [
            Setter::Enabled(true),
            Setter::TextColor(0),
            Setter::ContentDescription(None),
            Setter::Checked(true),
            Setter::Progress(1),
            Setter::Max(2),
            Setter::Indeterminate(false),
            Setter::ProgressTint(None),
            Setter::ThumbTint(None),
            Setter::TrackTint(None),
            Setter::ImageTint(None),
        ];
        for setter in cheap {
            assert_eq!(setter.tier(), Tier::Cheap, "{setter:?}");
        }

        let relayout = [
            Setter::Text("x"),
            Setter::TextSizeSp(12.0),
            Setter::BackgroundColor(0),
            Setter::ScaleType(Fit::Cover),
            Setter::ThemedBackground {
                fill: 0,
                radius_dp: 6.0,
            },
            Setter::Typeface(Typeface::GlyphMono),
        ];
        for setter in relayout {
            assert_eq!(setter.tier(), Tier::Relayout, "{setter:?}");
        }

        assert_eq!(Setter::ImageBytes(&bytes).tier(), Tier::Decode);
    }

    #[test]
    fn colors_decode_from_both_the_signed_and_unsigned_spelling() {
        let signed = crate::runtime::with_identity("k", 1, "\"textColor\":-16777216");
        let unsigned = crate::runtime::with_identity("k", 1, "\"textColor\":4278190080");
        assert_eq!(
            color(&Params::new(&signed), TEXT_COLOR),
            Some(0xFF00_0000_u32 as i32)
        );
        assert_eq!(
            color(&Params::new(&unsigned), TEXT_COLOR),
            Some(0xFF00_0000_u32 as i32)
        );
    }

    // --- the Apple arm's shared pure logic (p2-02) -------------------------

    #[test]
    fn argb_splits_into_the_four_apple_channels_in_rgba_order() {
        // Opaque pure red: alpha 1.0, red 1.0, the rest 0. The tuple order is
        // (r, g, b, a) — the argument order `UIColor`'s constructor takes,
        // deliberately NOT the packing order the wire uses.
        assert_eq!(argb_channels(0xFFFF_0000_u32 as i32), (1.0, 0.0, 0.0, 1.0));
        assert_eq!(argb_channels(0xFF00_FF00_u32 as i32), (0.0, 1.0, 0.0, 1.0));
        assert_eq!(argb_channels(0xFF00_00FF_u32 as i32), (0.0, 0.0, 1.0, 1.0));
        // Fully transparent white, and fully transparent black: the alpha
        // channel is read independently of the colour channels.
        assert_eq!(argb_channels(0x00FF_FFFF), (1.0, 1.0, 1.0, 0.0));
        assert_eq!(argb_channels(0x0000_0000), (0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn argb_reads_the_signed_spelling_without_sign_extending() {
        // The wire carries both spellings of an Android colour (see
        // `colors_decode_from_both_the_signed_and_unsigned_spelling` above),
        // so the negative one — every opaque colour, since the alpha byte sets
        // the sign bit — must decode identically. A `>>` on the raw `i32`
        // would sign-extend and hand every channel `1.0`.
        let opaque_black_signed: i32 = -16_777_216;
        assert_eq!(argb_channels(opaque_black_signed), (0.0, 0.0, 0.0, 1.0));
        assert_eq!(
            argb_channels(opaque_black_signed),
            argb_channels(0xFF00_0000_u32 as i32)
        );
    }

    #[test]
    fn every_channel_is_a_unit_interval_fraction_of_255() {
        for byte in 0..=255u32 {
            let argb = ((byte << 24) | (byte << 16) | (byte << 8) | byte) as i32;
            let (r, g, b, a) = argb_channels(argb);
            let expected = f64::from(byte) / 255.0;
            assert_eq!((r, g, b, a), (expected, expected, expected, expected));
            assert!((0.0..=1.0).contains(&r), "channel out of range for {byte}");
        }
    }

    // --- shared-Props parity: one kind table, two platform arms ------------

    #[test]
    fn the_six_control_kinds_are_the_same_strings_both_platform_arms_register() {
        // `crate::android::register_controls` and
        // `crate::apple::register_controls` are each `#[cfg(target_os = ...)]`
        // -gated, so no host test can call either. What a host CAN pin is the
        // thing they both register *by*: these six `KIND` consts. Neither arm
        // spells a kind literally (`crate::android`'s own registration note),
        // so a drift in the wire vocabulary has to pass through here.
        //
        // The other half of "shared-Props parity" needs no assertion at all:
        // there is exactly ONE `Props` type per control, in this same module
        // tree, used verbatim by both arms — same fields by construction, not
        // by agreement. A second, per-platform Props definition is the thing
        // this file's layout exists to prevent.
        let kinds = [
            button::KIND,
            label::KIND,
            switch::KIND,
            slider::KIND,
            progress::KIND,
            image::KIND,
        ];
        assert_eq!(
            kinds,
            ["button", "label", "switch", "slider", "progress", "image"],
            "the control kind strings are a shipped wire contract — the api \
             layer's builders inject them and both platform arms register \
             against them"
        );
        let mut unique = kinds.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), kinds.len(), "two controls share a kind");
    }

    #[test]
    fn a_missing_color_is_none_rather_than_a_default() {
        let raw = crate::runtime::with_identity("k", 1, "\"text\":\"hi\"");
        let params = Params::new(&raw);
        assert_eq!(color(&params, TEXT_COLOR), None);
        assert_eq!(text_or_empty(&params, TEXT), "hi");
        assert_eq!(text_or_empty(&params, CONTENT_DESCRIPTION), "");
        assert_eq!(owned_text(&params, CONTENT_DESCRIPTION), None);
        assert_eq!(slot_of(&params).unwrap(), 1);
    }
}
