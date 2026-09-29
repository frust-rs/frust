//! The v1 controls — eight shared (`Button`, `Label`, `Switch`, `Slider`,
//! `ProgressBar`, `Image`, `Spinner`, `DatePicker`), the two
//! Apple-arm-only controls
//! `Segmented` and `Stepper` ([`APPLE_KINDS`]), and the iOS-only `TabBar`
//! ([`IOS_ONLY_KINDS`]) — each an internal
//! [`NativeWidget`](crate::runtime::NativeWidget)
//! impl over [`crate::runtime`], with every property write a direct platform
//! setter on the main thread.
//!
//! # Two halves per control, one file
//!
//! Each control module is split by *compilation target*, not by file:
//!
//! 1. a **platform-agnostic half** — the typed `Props`, its `decode` out of a
//!    slot's `params_json`, and a pure `plan(old, new)` turning a props diff
//!    into an ordered list of [`Setter`]s. Compiled everywhere, so `cargo test`
//!    pins every control's diff behaviour on any host with no JNI at all (which
//!    is why these modules live in `src/controls/` rather than under
//!    `src/android/`, which compiles on Android only);
//! 2. a **platform half** — one `mod platform` per target, side by side in the
//!    same file, each holding that platform's `NativeWidget` impl: the Android
//!    arm builds the `android.widget.*` view, hands each planned [`Setter`] to
//!    `platform::apply` and retains/releases the global refs; the iOS arm builds
//!    the UIKit view, applies the **same plan** through typed `objc2-ui-kit`
//!    setters, and lets ARC own the references; the macOS arm registers every
//!    shared control plus the Apple-only pair (`Segmented`, `Stepper` — see
//!    `src/appkit/mod.rs`'s `register_controls` and [`APPLE_KINDS`] / [`IOS_ONLY_KINDS`]),
//!    and applies the same plan over `objc2-app-kit`.
//!
//! Half 1 is shared verbatim — one `Props`, one `decode`, one `plan`, two arms —
//! which is the point of the split: diff behaviour is asserted once, on a host,
//! and both platforms execute the identical plan. The *only* thing a plan cannot
//! express is view construction, so `create` is "apply the plan from the
//! platform's own freshly-constructed state" (`Props::platform_default`) — one
//! code path for create and update, and a `create` that sets nothing when the
//! app asked for the platform defaults.
//!
//! # Property tiers — which setter you call is the whole cost story
//!
//! On-device measurement (an optimized `--profile` build, 50 retained
//! `TextView`s, method ids cached) found the FFI crossing is not what costs —
//! *which* Android property you set is.
//!
//! | Tier | What it costs | Measured, per call | Setters |
//! |---|---|---|---|
//! | (floor) | the bare JNI crossing (`isEnabled()`) | **~0.14–0.27 µs** | — no setter is cheaper |
//! | [`Tier::Cheap`] | invalidate/repaint only | **~0.8 µs** (`setTextColor`) | [`Setter::Enabled`], [`Setter::TextColor`], [`Setter::ContentDescription`], [`Setter::Checked`], [`Setter::Progress`] (`Slider`/`Stepper`), [`Setter::Max`] (`Slider`/`Stepper`), [`Setter::Indeterminate`], [`Setter::Animating`] (`View.setVisibility`, not `GONE` — invalidate-only, same class as `setEnabled`), the five tint setters; Apple-only (no Android measurement exists — tiered by the same shape): [`Setter::SelectedSegment`], [`Setter::Momentary`], [`Setter::SegmentTint`], [`Setter::Step`], [`Setter::Wraps`], [`Setter::StepperTint`]; not independently measured, tiered by shape: [`Setter::Date`] (`DatePicker.updateDate`, the controlled value write — `Progress`'s class), [`Setter::DatePickerTint`], [`Setter::DatePickerTextColor`]; iOS-only, tiered by shape: [`Setter::SelectedTab`], [`Setter::TabBarTint`], [`Setter::TabBarUnselectedTint`], [`Setter::TabBarBackground`] |
//! | [`Tier::Relayout`] | `requestLayout()` + a measure/layout pass | **~29 µs** (`setText`) — ~35× a colour set | [`Setter::Text`], [`Setter::TextSizeSp`], [`Setter::BackgroundColor`], [`Setter::ScaleType`], [`Setter::ThemedBackground`], [`Setter::SizeClass`] (not independently measured — grouped here because a style/size swap re-measures the view, the same reasoning [`Setter::ThemedBackground`] itself is grouped by), [`Setter::Segments`] (Apple-only — a segment-list replace re-measures every segment), [`Setter::MinDate`]/[`Setter::MaxDate`] (not independently measured — a range change repopulates a calendar's month pages/year list), [`Setter::DatePickerStyle`] (a presentation swap rebuilds the picker's layout; Apple-only in effect — Android warns and ignores), [`Setter::TabItems`] (iOS-only — an item-array replace re-lays-out every item and decodes any byte icon) |
//! | [`Tier::Decode`] | bytes → `Bitmap`, allocation + image decode | milliseconds, size-dependent (not micro-benchmarked) | [`Setter::ImageBytes`] |
//!
//! **Per-frame guidance:** ~500 [`Tier::Cheap`] setters per frame ≈ 0.4 ms and
//! fits a 16 ms budget comfortably; the same 500 at [`Tier::Relayout`] is
//! ~14.5 ms and does not. **Event-driven `setText` is fine; per-frame text
//! streaming is not** — a high-rate surface should stream a cheap property
//! (colour, progress, checked) and leave text/size/scale-type to
//! interaction-rate changes. [`Tier::Decode`] never belongs on a frame path at
//! all, which is why [`Setter::ImageBytes`] is emitted only when the bytes'
//! *identity* changed (`crate::controls::image`), never per rebuild. Debug
//! builds are 2–5× worse across the board — only judge these numbers on an
//! optimized build.
//!
//! **The table is Android-measured, and [`Tier`] is deliberately not re-derived
//! per platform.** No equivalent UIKit measurement exists, so the iOS arm
//! applies the same plan without claiming the same costs. The *shape* is
//! expected to carry — a UIKit caption/font change invalidates intrinsic content
//! size and re-lays-out the view, a colour change only redisplays it — but the
//! numbers above are not evidence for iOS and must not be cited as if they were.
//!
//! # Field-level diffing is the control's job
//!
//! [`crate::runtime`]'s `Props: PartialEq` gate is **whole-struct**: it decides
//! only whether `update` runs at all. Which *setters* run is decided here, per
//! field, because only a control knows its text setter costs 35× its colour
//! setter (the table above). Every `plan` therefore emits a setter only for a
//! field that actually changed, in a deterministic order the tests pin.
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
//!   version** (AOSP: the same call stack as the setter, guarded only by the
//!   widget's own reentrancy flag, never posted or animation-deferred;
//!   `switch.rs`'s module doc has the full account). `update` runs inside
//!   `crate::runtime::with_runtime`, so that echo re-enters the same
//!   thread-local `RefCell` mid-borrow and is dropped there, before
//!   `NativeWidget::on_event`/each control's `decode_toggled`/`decode_event`
//!   ever see it — the runtime's re-entrancy tolerance is the SOLE guard. There
//!   is no per-instance suppression flag, and therefore nothing a panic
//!   mid-`update` could leave latched. **That safety is incidental, not
//!   designed**: it holds only because
//!   `crate::runtime::NativeRuntime::update_params` — which calls `update` — is
//!   itself always invoked from inside `with_runtime` (`crate::android`'s
//!   `nativeUpdateParams`), so a refactor moving `update` outside that borrow
//!   would silently remove the only echo protection this crate has.
//!
//! **On iOS there is no echo to guard at all — and still no iOS-specific
//! machinery.** UIKit documents that it does not send control events for
//! programmatic changes, so `setOn:animated:`/`setValue:` are not expected to
//! re-enter this crate the way `setChecked` does; that expectation carries two
//! caveats (`switch.rs`'s module doc, the reference description for this arm
//! too), and the same `with_runtime` re-entrancy drop covers both if either
//! bites. **Do not add a per-instance suppression flag on either platform** —
//! there is nothing to reinstate.

pub(crate) mod button;
pub(crate) mod date_picker;
pub(crate) mod image;
pub(crate) mod label;
pub(crate) mod progress;
pub(crate) mod segmented;
pub(crate) mod slider;
pub(crate) mod spinner;
pub(crate) mod stepper;
pub(crate) mod switch;
pub(crate) mod tab_bar;
pub(crate) mod typeface;

use std::borrow::Cow;

use crate::NativeWidgetError;
use crate::registry::SlotId;
use crate::runtime::Params;

use self::date_picker::{CivilDate, DatePickerStyle};
use self::image::{Fit, ImageBytes};
use self::spinner::SizeClass;
use self::tab_bar::TabItemProps;
use self::typeface::Typeface;

// --- the kind tables ----------------------------------------------------------
//
// Which control kinds each platform arm registers, as three tables — one per
// arm-membership shape, because membership is per ARM, not per vendor:
//
// | table | Android | iOS | macOS |
// |---|---|---|---|
// | `SHARED_KINDS` | yes | yes | yes |
// | `APPLE_KINDS` | — | yes | yes |
// | `IOS_ONLY_KINDS` | — | yes | — |
//
// A kind outside an arm's tables has no `NativeWidget` impl there, and its
// builder renders a refusal banner on that target at compile time
// (`crate::api::builders`). Every arm's `register_controls` registers exactly
// its tables, in this order (shared, then Apple, then iOS-only) — pinned
// host-side by
// `tests::every_register_controls_registers_exactly_its_kind_tables`, and on
// the two Apple arms additionally checked at runtime (a `debug_assert!` loop
// over their tables in their `register_controls`). The tables are
// append-only, like the event kinds.

/// The kinds all three platform arms (Android, iOS, macOS) register.
pub(crate) const SHARED_KINDS: [&str; 8] = [
    button::KIND,
    label::KIND,
    switch::KIND,
    slider::KIND,
    progress::KIND,
    image::KIND,
    spinner::KIND,
    date_picker::KIND,
];

/// The kinds only the two Apple arms (iOS, macOS) register — Android has no
/// `NativeWidget` impl for these yet, and its builder path renders the
/// refusal banner instead of a slot.
pub(crate) const APPLE_KINDS: [&str; 2] = [segmented::KIND, stepper::KIND];

/// The kinds only the iOS arm registers — the macOS arm has no counterpart
/// (`TabBar`: macOS has no bottom-tab-bar idiom) and Android has none either
/// (`BottomNavigationView` is Material, decision D2); both builders render the
/// refusal banner. The first asymmetric kind between the two Apple arms, which
/// is why this is its own table rather than a row in [`APPLE_KINDS`]
/// (`tab_bar.rs`'s module doc).
pub(crate) const IOS_ONLY_KINDS: [&str; 1] = [tab_bar::KIND];

// --- the params wire keys ---------------------------------------------------
//
// One definition per key, shared by every control that carries it, so the api
// layer and the decoders here can never drift apart.

/// `"text"` — a `Button`/`Label` caption.
pub(crate) const TEXT: &str = "text";
/// `"enabled"` — `View.setEnabled`; defaults to `true` when absent.
pub(crate) const ENABLED: &str = "enabled";
/// `"textColor"` — packed ARGB (see [`color`]). `DatePicker` decodes it
/// into its own [`Setter::DatePickerTextColor`] rather than
/// [`Setter::TextColor`], whose Android arm assumes a `TextView`.
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
/// `"step"` — `Stepper`'s increment ([`Setter::Step`]); a plain delta, never
/// offset by [`MIN`] (unlike [`VALUE`]/[`MAX`]) since a step size is
/// invariant to where the range starts.
pub(crate) const STEP: &str = "step";
/// `"wraps"` — `Stepper`'s wrap-at-the-bounds flag ([`Setter::Wraps`]).
pub(crate) const WRAPS: &str = "wraps";
/// `"indeterminate"` — `ProgressBar`'s spinner mode.
pub(crate) const INDETERMINATE: &str = "indeterminate";
/// `"progressTint"` — packed ARGB, `null`-able (see [`Setter::ProgressTint`]).
pub(crate) const PROGRESS_TINT: &str = "progressTint";
/// `"thumbTint"` — packed ARGB, `null`-able.
pub(crate) const THUMB_TINT: &str = "thumbTint";
/// `"trackTint"` — packed ARGB, `null`-able.
pub(crate) const TRACK_TINT: &str = "trackTint";
/// `"tint"` — packed ARGB, `null`-able: `Image`'s tint, `Segmented`'s
/// selected-segment tint ([`Setter::SegmentTint`]), `Stepper`'s tint
/// ([`Setter::StepperTint`]) and `DatePicker`'s tint
/// ([`Setter::DatePickerTint`]) all ride this one wire key — each decodes it
/// into its own typed `Setter`, so the shared key never implies a shared
/// apply.
pub(crate) const TINT: &str = "tint";
/// `"fit"` — `Image`'s scale-type hint (see [`Fit`]).
pub(crate) const FIT: &str = "fit";
/// `"dark"` — whether the active theme's `Brightness` is `Dark` (theme
/// ladder L1): the input to the night-qualified `Context`
/// `crate::android`'s `create_control` builds every control's view against
/// (`crate::android::theme::night_qualified_context`). Read straight off a
/// slot's raw params by `crate::android::theme::brightness_is_dark` *before*
/// any per-control typed decode runs — at that point in `create_control`
/// there is no registered kind yet to decode against — so it is never a
/// field on any control's own `Props`, unlike every other key in this list.
pub(crate) const DARK: &str = "dark";
/// `"cornerRadiusDp"` — [`Setter::ThemedBackground`]'s corner radius, dp
/// (theme ladder L2: `button::ButtonProps::corner_radius_dp`).
pub(crate) const CORNER_RADIUS_DP: &str = "cornerRadiusDp";
/// `"typeface"` — [`Setter::Typeface`]'s wire spelling (theme ladder L3:
/// `typeface::Typeface::wire`/`decode`).
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
/// device-gated separately).
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

    /// `ProgressBar.setProgress(int)` / `UIStepper.setValue:` /
    /// `NSStepper.setDoubleValue:` — **[`Tier::Cheap`]**. Platform-space:
    /// already offset by the app's `min` (see [`slider`]). `Stepper` reuses
    /// this exact variant rather than a control-specific one: its own
    /// `[0, span]` mapping is [`slider`]'s, verbatim (`stepper`'s module
    /// doc), and both Apple arms' setter selectors are the same ones
    /// [`slider`]'s own `apply` already calls (`setValue:`/`setDoubleValue:`)
    /// — a new variant would only rename an identical write.
    ///
    /// Must be planned *after* [`Self::Max`] in the same plan — the platform
    /// clamps progress to the current max, so raising both in the other order
    /// silently truncates the value.
    Progress(i32),

    /// `ProgressBar.setMax(int)` / `UIStepper.setMaximumValue:` /
    /// `NSStepper.setMaxValue:` — **[`Tier::Cheap`]**. Platform-space span
    /// (`max - min`), since `SeekBar.setMin` needs API 26 and this crate's
    /// floor is 24. Reused by `Stepper` for the same reason [`Self::Progress`]
    /// documents.
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

    /// `ProgressBar.setIndeterminateTintList(ColorStateList)` /
    /// `UIActivityIndicatorView.color` / `NSProgressIndicator` (no AppKit
    /// tint property at all, logged and no-op'd — `spinner.rs`'s module
    /// doc) — **[`Tier::Cheap`]**, same nullable-clearable shape as
    /// [`Self::ProgressTint`]. Named separately from the other four tints
    /// rather than reusing one of them: `Spinner` is its own control with
    /// its own wire key and its own Android setter name.
    SpinnerTint(Option<i32>),

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
    /// Theme ladder L2: `Button`'s background/corner-radius pinning
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
    /// Theme ladder L3: the resolved
    /// [`typeface::Typeface`] a text-bearing control (`Button`/`Label`/
    /// `Switch`) renders in. [`typeface::Typeface::System`] plans this same
    /// setter with a `null` argument (`crate::android::fonts::typeface_for`
    /// returns `None`), restoring the platform's own face — both the
    /// explicit choice and the registration-failure degrade path
    /// (`crate::android::fonts`'s module doc) land on the exact same call.
    Typeface(Typeface),

    /// `UIActivityIndicatorView.style` / `NSProgressIndicator.controlSize` —
    /// **[`Tier::Relayout`]**: a style/size swap re-measures the view.
    /// `Spinner`-only. `android.widget.ProgressBar` has no live equivalent
    /// at all — the size is baked in at construction, so this setter reaches
    /// that arm only on a genuine post-create change and is
    /// warned-and-ignored there (`spinner.rs`'s module doc's *`size_class`*
    /// section).
    SizeClass(SizeClass),

    /// `View.setVisibility(int)` / `UIActivityIndicatorView.startAnimating`/
    /// `stopAnimating` / `NSProgressIndicator.startAnimation:`/
    /// `stopAnimation:` — **[`Tier::Cheap`]**. `Spinner`-only; see
    /// `spinner.rs`'s module doc's *`animating`* section for why the
    /// Android mapping is a visibility toggle rather than a start/stop call.
    Animating(bool),

    /// Replace a segmented control's whole segment list —
    /// `UISegmentedControl.removeAllSegments` + one
    /// `insertSegmentWithTitle:atIndex:animated:` per label /
    /// `NSSegmentedControl.segmentCount` + one `setLabel:forSegment:` per
    /// label — **[`Tier::Relayout`]**: the segment widths are re-measured.
    /// `Segmented`-only and Apple-only (no Android arm — `segmented.rs`'s
    /// module doc). Wholesale, never per-segment: a label list changes at
    /// interaction rate, and one replace keeps the plan trivially correct
    /// across inserts/removals. Always followed by
    /// [`Self::SelectedSegment`] in the same plan, because replacing the
    /// segments resets the platform's selection.
    Segments(&'a [String]),

    /// `UISegmentedControl.selectedSegmentIndex` /
    /// `NSSegmentedControl.selectedSegment` — **[`Tier::Cheap`]**. `None`
    /// writes the platforms' shared "no segment" sentinel (`-1`). The
    /// controlled-component write-back setter of `Segmented`, the
    /// [`Self::Checked`] of this control.
    SelectedSegment(Option<usize>),

    /// `UISegmentedControl.momentary` / `NSSegmentedControl.trackingMode`
    /// (`Momentary` vs `SelectOne`) — **[`Tier::Cheap`]**. `Segmented`-only.
    Momentary(bool),

    /// `UISegmentedControl.selectedSegmentTintColor` /
    /// `NSSegmentedControl.selectedSegmentBezelColor` — **[`Tier::Cheap`]**,
    /// the same nullable-clearable shape as [`Self::ProgressTint`] (nil
    /// restores the platform's own selected-segment colour). `Segmented`-only.
    SegmentTint(Option<i32>),

    /// `UIStepper.setStepValue:` / `NSStepper.setIncrement:` —
    /// **[`Tier::Cheap`]**. `Stepper`-only and Apple-only (no Android arm —
    /// `stepper.rs`'s module doc). A plain delta, never offset by the app's
    /// `min` — see [`super::STEP`].
    Step(i32),

    /// `UIStepper.setWraps:` / `NSStepper.setValueWraps:` —
    /// **[`Tier::Cheap`]**. `Stepper`-only and Apple-only.
    Wraps(bool),

    /// `UIStepper.tintColor` — **[`Tier::Cheap`]**, the same
    /// nullable-clearable shape as [`Self::ProgressTint`]. `Stepper`-only and
    /// Apple-only; named separately from the other tint setters rather than
    /// reusing one, the same reason [`Self::SpinnerTint`] gives: `Stepper` has
    /// its own wire key reader (via the shared [`super::TINT`] key) and its
    /// own Apple setter name, shared with no other control's apply.
    /// `NSStepper` exposes no tint property at all — logged and no-op'd
    /// (`crate::controls::platform::set_tint`'s generic "this view has no
    /// tint property" branch), the same documented-gap shape as
    /// [`Self::ThumbTint`] on `NSSlider`.
    StepperTint(Option<i32>),

    /// `DatePicker.updateDate` / `UIDatePicker.setDate:` /
    /// `NSDatePicker.setDateValue:` — **[`Tier::Cheap`]**: the controlled
    /// value write of `DatePicker`, the [`Self::Progress`] of that control.
    /// Must be planned *after* [`Self::MinDate`]/[`Self::MaxDate`] in the
    /// same plan — every platform clamps the date to the current range.
    Date(CivilDate),

    /// `DatePicker.setMinDate` / `UIDatePicker.minimumDate` /
    /// `NSDatePicker.minDate` — **[`Tier::Relayout`]**. `None` restores the
    /// platform's own floor (Android's 1900-01-01, nil on Apple).
    MinDate(Option<CivilDate>),

    /// `DatePicker.setMaxDate` / `UIDatePicker.maximumDate` /
    /// `NSDatePicker.maxDate` — **[`Tier::Relayout`]**, the ceiling twin of
    /// [`Self::MinDate`] (Android's own default: 2100-12-31).
    MaxDate(Option<CivilDate>),

    /// `UIDatePicker.preferredDatePickerStyle` /
    /// `NSDatePicker.datePickerStyle` (+ `presentsCalendarOverlay`) —
    /// **[`Tier::Relayout`]**. Android bakes the mode into the constructor
    /// style, so there this setter reaches `apply` only on a genuine
    /// post-create change and is warned-and-ignored (`date_picker.rs`'s
    /// module doc — [`Self::SizeClass`]'s exact shape).
    DatePickerStyle(DatePickerStyle),

    /// `UIDatePicker.tintColor` — **[`Tier::Cheap`]**, the same
    /// nullable-clearable shape as [`Self::ProgressTint`]. `NSDatePicker` and
    /// Android's `DatePicker` expose no tint at all — logged and no-op'd.
    DatePickerTint(Option<i32>),

    /// `NSDatePicker.textColor` — **[`Tier::Cheap`]**; `None` restores
    /// `NSColor.controlTextColor`. Its own variant rather than
    /// [`Self::TextColor`], whose Android arm is a cached
    /// `TextView.setTextColor` a `DatePicker` does not have; UIKit and
    /// Android expose no public picker text colour — logged and no-op'd.
    DatePickerTextColor(Option<i32>),

    /// Replace a tab bar's whole item array — one `UITabBarItem` per item
    /// (`initWithTitle:image:tag:`, `selectedImage`, `badgeValue`, `enabled`)
    /// then `UITabBar.setItems:animated:` — **[`Tier::Relayout`]**: the bar
    /// re-lays-out every item, and a byte icon is decoded here. `TabBar`-only
    /// and iOS-only (`tab_bar.rs`'s module doc). Planned only when the items
    /// changed, and always followed by [`Self::SelectedTab`], because the new
    /// array holds new item objects.
    TabItems(&'a [TabItemProps]),

    /// `UITabBar.selectedItem` — **[`Tier::Cheap`]**. `None` (or an index
    /// past the items) clears the selection. The controlled-component
    /// write-back setter of `TabBar`, the [`Self::SelectedSegment`] of that
    /// control.
    SelectedTab(Option<usize>),

    /// `UITabBar.tintColor` (the selected item) — **[`Tier::Cheap`]**, the
    /// nullable-clearable shape of [`Self::ProgressTint`]. `TabBar`-only.
    TabBarTint(Option<i32>),

    /// `UITabBar.unselectedItemTintColor` — **[`Tier::Cheap`]**, nullable.
    /// `TabBar`-only.
    TabBarUnselectedTint(Option<i32>),

    /// The bar background through a `UITabBarAppearance` installed as both
    /// `standardAppearance` and `scrollEdgeAppearance` (opaque with the
    /// colour; `None` the default blurred material) — **[`Tier::Cheap`]**: a
    /// redisplay, no item relayout. `TabBar`-only.
    TabBarBackground(Option<i32>),
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
            | Self::Typeface(_)
            | Self::SizeClass(_)
            | Self::Segments(_)
            | Self::TabItems(_)
            | Self::MinDate(_)
            | Self::MaxDate(_)
            | Self::DatePickerStyle(_) => Tier::Relayout,
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
            | Self::ImageTint(_)
            | Self::SpinnerTint(_)
            | Self::Animating(_)
            | Self::SelectedSegment(_)
            | Self::Momentary(_)
            | Self::SegmentTint(_)
            | Self::Step(_)
            | Self::Wraps(_)
            | Self::StepperTint(_)
            | Self::Date(_)
            | Self::DatePickerTint(_)
            | Self::DatePickerTextColor(_)
            | Self::SelectedTab(_)
            | Self::TabBarTint(_)
            | Self::TabBarUnselectedTint(_)
            | Self::TabBarBackground(_) => Tier::Cheap,
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
/// arms call it (`platform::ui_color` on iOS, `platform::ns_color` and the
/// layer's `CGColor` on macOS): the packing convention is
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
    //! `Env::call_method_unchecked`, the proven shape behind its measured
    //! floor of ~0.14–0.27 µs per crossing rather than three crossings plus a
    //! string lookup.
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

    use std::sync::{Once, OnceLock};

    use jni::objects::{JClass, JMethodID, JObject, JValue};
    use jni::signature::{MethodSignature, Primitive, ReturnType};
    use jni::strings::JNIStr;
    use jni::sys::jvalue;
    use jni::{jni_sig, jni_str};

    use super::{Plan, Setter, SizeClass};
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
    /// [`Setter::ThemedBackground`]'s drawable (theme ladder L2).
    pub(crate) const GRADIENT_DRAWABLE_CLASS: &str = "android.graphics.drawable.GradientDrawable";

    /// How many local references a control's create/update frame reserves.
    ///
    /// A whole plan allocates at most two locals per setter (a string, a
    /// `ColorStateList`) and the widest control plans seven, so 16 covers
    /// every case with room to spare — and the frame is what releases them
    /// all on return, instead of pinning them for the whole JNI call
    /// (`NativeCtx`'s local-frame discipline).
    pub(crate) const FRAME_CAPACITY: usize = 16;

    /// The hot setters' cached method ids (module doc). One table for all
    /// eight Android controls (`Spinner` and `DatePicker` reach it only
    /// through the shared `Setter::Enabled` arm every control already rode —
    /// neither adds a hot class or method of its own): it is seeded on the first control
    /// creation of any kind and costs five class loads plus six
    /// `GetMethodID`s, once per process.
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
            Setter::SpinnerTint(argb) => {
                tint_list(ctx, view, jni_str!("setIndeterminateTintList"), argb)
            }
            Setter::Animating(animating) => ctx.call_void(
                view,
                jni_str!("setVisibility"),
                jni_sig!("(I)V"),
                &[JValue::Int(if animating { 0 } else { 4 })],
            ),
            Setter::SizeClass(size_class) => {
                // `spinner.rs`'s module doc's *`size_class`* section: a
                // `ProgressBar`'s spinner size is baked in at construction on
                // this arm alone — a Setter this control's own `create`
                // already avoided emitting for the size it was just built
                // with, so reaching here means a genuine post-create change,
                // which this arm cannot honour.
                warn_size_class_unsupported(size_class);
                Ok(())
            }
            // `Segmented`'s four setters: that control has no Android arm
            // (`crate::controls::APPLE_KINDS`; `segmented.rs`'s module doc),
            // so no control this arm registers ever plans one. Reaching here
            // is a plan/apply drift bug — warned, never a dead slot.
            Setter::Segments(_)
            | Setter::SelectedSegment(_)
            | Setter::Momentary(_)
            | Setter::SegmentTint(_) => {
                log::warn!(
                    "frust-native-widgets: an Android control planned a segmented-control \
                     setter ({setter:?}), which this arm does not implement — ignored"
                );
                Ok(())
            }
            // `Stepper`'s own three setters — the same shape as the segmented
            // group above: `Stepper` has no Android arm either
            // (`crate::controls::APPLE_KINDS`; `stepper.rs`'s module doc), so
            // no control this arm registers ever plans one. Its shared
            // `Setter::Max`/`Setter::Progress` reuse is handled by those
            // variants' own arms above and never reaches here.
            Setter::Step(_) | Setter::Wraps(_) | Setter::StepperTint(_) => {
                log::warn!(
                    "frust-native-widgets: an Android control planned a stepper-control setter \
                     ({setter:?}), which this arm does not implement — ignored"
                );
                Ok(())
            }
            // `DatePicker`'s own six setters are applied by that control's
            // own Android `apply` (`date_picker.rs`), which forwards only the
            // shared `Enabled`/`ContentDescription` here — so reaching this
            // arm is a plan/apply drift bug, warned like the groups above.
            Setter::Date(_)
            | Setter::MinDate(_)
            | Setter::MaxDate(_)
            | Setter::DatePickerStyle(_)
            | Setter::DatePickerTint(_)
            | Setter::DatePickerTextColor(_) => {
                log::warn!(
                    "frust-native-widgets: a date-picker setter ({setter:?}) reached the shared \
                     Android apply instead of date_picker.rs's own — ignored"
                );
                Ok(())
            }
            // `TabBar`'s five setters: that control is iOS-only
            // (`crate::controls::IOS_ONLY_KINDS`; `tab_bar.rs`'s module doc),
            // so no control this arm registers ever plans one — the
            // segmented group's shape.
            Setter::TabItems(_)
            | Setter::SelectedTab(_)
            | Setter::TabBarTint(_)
            | Setter::TabBarUnselectedTint(_)
            | Setter::TabBarBackground(_) => {
                log::warn!(
                    "frust-native-widgets: an Android control planned a tab-bar setter \
                     ({setter:?}), which this arm does not implement — ignored"
                );
                Ok(())
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

    /// One-time warning that [`Setter::SizeClass`] has no live Android
    /// equivalent — see `crate::controls::spinner`'s module doc's
    /// *`size_class`* section for why an indeterminate `ProgressBar`'s
    /// spinner size is baked into the style it was constructed with and
    /// cannot be swapped without rebuilding the view.
    fn warn_size_class_unsupported(size_class: SizeClass) {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            log::warn!(
                "frust-native-widgets: Android's indeterminate ProgressBar bakes its spinner \
                 size into the style it was constructed with (progressBarStyleSmall/-Normal/\
                 -Large), so a later sizeClass change to {size_class:?} is not applied — the \
                 spinner keeps showing the size it was created at"
            );
        });
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
    //! UIKit call main-thread-typed" a compile-time property
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
    use objc2_core_foundation::CFRetained;
    use objc2_core_text::CTFont;
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

    /// `view.layer.cornerRadius` + `masksToBounds` — the corner-radius half
    /// of [`Setter::ThemedBackground`] (theme ladder L2); the
    /// fill half is [`set_background_color`].
    ///
    /// UIKit's own coordinate system is already point-based
    /// (density-independent), so `radius_dp` — already the wire's
    /// density-independent unit, the same one every other `_dp`-suffixed
    /// value on this wire carries — applies to `CALayer.cornerRadius`
    /// directly as `CGFloat` points. Unlike Android's `dp_to_px`
    /// (`crate::android::theme`), which converts dp to raw device pixels for
    /// `GradientDrawable`'s pixel-space API, this arm needs no density
    /// lookup at all — a genuine platform difference, not a gap.
    pub(crate) fn set_corner_radius(view: &UIView, radius_dp: f32) {
        let layer = view.layer();
        layer.setCornerRadius(f64::from(radius_dp));
        layer.setMasksToBounds(radius_dp > 0.0);
    }

    /// A resolved [`UIFont`], from either the system font or a registered
    /// Glyph face — the two shapes [`resolve_font`] can hand back, unified
    /// behind one accessor so a control's `apply` never has to branch on
    /// which arm it got (theme ladder L3).
    ///
    /// [`Self::Glyph`] holds a **sized** `CTFont`
    /// (`CTFont::with_font_descriptor` bakes the point size in, mirroring
    /// `UIFont`'s own immutability — see [`FontState`]'s doc), bridged to
    /// `UIFont` via `objc2-ui-kit`'s `AsRef<UIFont> for CTFont` (a
    /// toll-free bridge — the same underlying object, not a cast) —
    /// gated by this crate's `objc2-core-text` feature on `objc2-ui-kit`
    /// (`Cargo.toml`).
    pub(crate) enum ResolvedFont {
        /// [`system_font`] — [`Typeface::System`], or the degrade target of
        /// a Glyph resolution failure.
        System(Retained<UIFont>),
        /// A registered Glyph face, sized for this call.
        Glyph(CFRetained<CTFont>),
    }

    impl ResolvedFont {
        /// Borrow this as a `&UIFont`, whichever arm it is — every UIKit
        /// setter this crate calls ([`set_label_font`]) takes exactly this.
        pub(crate) fn as_ui_font(&self) -> &UIFont {
            match self {
                Self::System(font) => font,
                Self::Glyph(font) => font.as_ref(),
            }
        }
    }

    /// Resolve `typeface` to a real [`UIFont`] at `size_sp` points — the
    /// Apple half of theme ladder L3, mirroring
    /// `crate::android::fonts::typeface_for`'s contract:
    /// [`Typeface::System`] is the platform default ([`system_font`]); a
    /// Glyph face resolves through `crate::apple::fonts`'s thread-locally
    /// cached [`objc2_core_text::CTFontDescriptor`], degrading to the system
    /// font (with that module's own one-time warning) on any resolution
    /// failure.
    ///
    /// Unlike Android's cached `Typeface` object (reusable at any size via an
    /// independent `setTextSize` call), a Glyph arm here rebuilds a freshly
    /// **sized** `CTFont` on every call — see [`FontState`]'s doc for why
    /// `UIFont`/`CTFont`'s own immutability forces that.
    pub(crate) fn resolve_font(typeface: Typeface, size_sp: f32) -> ResolvedFont {
        match crate::apple::fonts::descriptor_for(typeface) {
            Some(descriptor) => {
                // SAFETY: `descriptor` is a live `CTFontDescriptor`, owned
                // for the duration of this call
                // (`crate::apple::fonts::descriptor_for` hands back a fresh
                // `CFRetained` clone — a cheap `CFRetain`, not a copy);
                // `size_sp` is a finite point size and the null matrix
                // argument is `CTFontCreateWithFontDescriptor`'s own
                // documented "use the identity matrix" default.
                let font = unsafe {
                    CTFont::with_font_descriptor(&descriptor, f64::from(size_sp), std::ptr::null())
                };
                ResolvedFont::Glyph(font)
            }
            None => ResolvedFont::System(system_font(size_sp)),
        }
    }

    /// The point size a [`resolve_font`] call falls back to when neither
    /// this arm's own [`Setter::TextSizeSp`] has ever applied one — the size
    /// a freshly constructed `UILabel`/`UIButton.titleLabel`'s own system
    /// font already renders at.
    ///
    /// **Community-approximate**: UIKit exposes no single named constant for
    /// "a freshly constructed label/button's own font size" the way
    /// `layer.cornerRadius` is a real property read; 17pt is where a fresh
    /// `UILabel`'s default system font and `UIButton`'s default title font
    /// converge on iOS.
    const DEFAULT_POINT_SIZE: f32 = 17.0;

    /// The per-control state theme ladder L3 needs on this arm to
    /// keep [`Setter::TextSizeSp`]/[`Setter::Typeface`] independent, the way
    /// Android's `setTextSize`/`setTypeface` genuinely are (two calls,
    /// either one leaving the other alone).
    ///
    /// `UIFont`/`CTFont` bake family AND point size into one immutable
    /// object — there is no "just change the family, keep the size" UIKit
    /// call — so applying either setter alone on this arm means rebuilding
    /// the WHOLE font from (the field that changed, the other field's
    /// last-applied value). [`Self::apply_size`]/[`Self::apply_typeface`]
    /// are that: each records its own half and re-resolves through
    /// [`resolve_font`] with the OTHER half unchanged, so a
    /// `Setter::Typeface` alone never resets a previously-applied size back
    /// to the platform default, and vice versa. `Button`/`Label` (the only
    /// two controls that render real text through a `Typeface` field on
    /// this arm — `Switch`'s `Setter::Typeface` is a documented no-op, see
    /// `switch.rs`) each carry one of these in their Apple `State`.
    pub(crate) struct FontState {
        typeface: Typeface,
        size_sp: Option<f32>,
    }

    impl FontState {
        /// The state a freshly constructed control (no `TextSizeSp`/
        /// `Typeface` setter ever applied) is already in — mirrors
        /// `ButtonProps`/`LabelProps::platform_default`'s own
        /// `Typeface::System`, `text_size_sp: None`.
        pub(crate) fn platform_default() -> Self {
            Self {
                typeface: Typeface::System,
                size_sp: None,
            }
        }

        /// [`Setter::TextSizeSp`]'s apply: record the new size, keep the
        /// current typeface, and resolve the combined font.
        pub(crate) fn apply_size(&mut self, size_sp: f32) -> ResolvedFont {
            self.size_sp = Some(size_sp);
            resolve_font(self.typeface, size_sp)
        }

        /// [`Setter::Typeface`]'s apply: record the new typeface, keep the
        /// current size, and resolve the combined font.
        pub(crate) fn apply_typeface(&mut self, typeface: Typeface) -> ResolvedFont {
            self.typeface = typeface;
            resolve_font(typeface, self.size_sp.unwrap_or(DEFAULT_POINT_SIZE))
        }
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

// --- the macOS half ---------------------------------------------------------

#[cfg(target_os = "macos")]
pub(crate) mod platform {
    //! Turning a [`Setter`] into a real AppKit call — the macOS mirror of the
    //! iOS half above, and the only place in this crate's macOS arm that
    //! reaches for a shared `objc2-app-kit` helper.
    //!
    //! # Per-control apply, like iOS
    //!
    //! AppKit has more of a spine than UIKit — every v1 control is an
    //! `NSControl`, which owns `enabled` and `font` — but the caption still
    //! differs by class (`NSButton.title` vs `NSTextField.stringValue`), so
    //! each control applies its own plan against its own typed view (its
    //! `#[cfg(target_os = "macos")] mod platform`), and this module holds only
    //! the genuinely shared setters: `NSControl`'s enabled/font, `NSView`'s
    //! accessibility label, the theme ladder's L2/L3 setters, and the
    //! warnings. Theme ladder L1 (`NSAppearance`) is not a [`Setter`] at all —
    //! it rides the raw wire, applied per view by `crate::appkit::theme`.
    //!
    //! # The theme ladder's AppKit mapping (L2/L3)
    //!
    //! The shared setters take the superclass a control arm hands them
    //! (`&NSControl`/`&NSView`), so the ones whose AppKit property lives on a
    //! specific class resolve it with an `isKindOfClass:` downcast. What each
    //! planned [`Setter`] becomes here, per `crate::api::theme`'s fold:
    //!
    //! | Setter | Class | AppKit call |
    //! |---|---|---|
    //! | [`Setter::TextColor`] | `NSTextField` (`Label`'s `body_text`) | `textColor` |
    //! | [`Setter::TextColor`] | `NSButton` (`Button`'s `on_accent_fill`) | `contentTintColor` |
    //! | [`Setter::BackgroundColor`] / fill of [`Setter::ThemedBackground`] | any `NSView` | `wantsLayer` + `layer.backgroundColor` (an sRGB `CGColor`) |
    //! | the same, additionally | `NSButton` (`Button`'s `accent_fill`) | `bezelColor` |
    //! | radius of [`Setter::ThemedBackground`] | any `NSView` | `layer.cornerRadius` + `masksToBounds` |
    //! | [`Setter::ProgressTint`] | `NSSlider` (`accent_fill`) | `trackFillColor` |
    //! | [`Setter::ImageTint`] | `NSImageView` | `contentTintColor` on a template image — clearing restores the original, untinted image (`controls::image`'s macOS module doc) |
    //! | [`Setter::TextSizeSp`] / [`Setter::Typeface`] | `NSControl` | `font` (one immutable `NSFont`, see *Size and face* below) |
    //!
    //! **No AppKit API, so no call** (logged at debug, never a failure):
    //! `NSSwitch` exposes neither a thumb nor a track colour
    //! ([`Setter::ThumbTint`]/[`Setter::TrackTint`]) — it draws its "on" track
    //! in the system accent colour; `NSSlider` has no thumb colour
    //! ([`Setter::ThumbTint`]); `NSProgressIndicator` has no tint property at
    //! all ([`Setter::ProgressTint`]) — AppKit draws its fill in the system
    //! accent colour, and the one historical knob, `controlTint`, is
    //! deprecated and deliberately never called. All three still follow the
    //! app's brightness through L1. This is the plugin's documented
    //! "representative subset" policy (`docs/PLUGINS_CODE_STANDARDS.md`),
    //! recorded per platform rather than papered over.
    //!
    //! `contentTintColor` is documented by AppKit's header as applying to
    //! *borderless* buttons, while `Button`'s arm builds a push-bezel button;
    //! the button's accent therefore rides `bezelColor` (the push bezel's own
    //! fill) plus the layer fill, and the title colour is best-effort.
    //!
    //! `radius_dp` applies to `CALayer.cornerRadius` directly as points:
    //! AppKit's coordinate space is already density-independent, the same
    //! reasoning as the iOS half's `set_corner_radius`.
    //!
    //! # Size and face: one `NSFont`, derived from the control's own font
    //!
    //! `NSFont`, like `UIFont`, bakes family and point size into one immutable
    //! object, so [`Setter::TextSizeSp`] and [`Setter::Typeface`] cannot be two
    //! independent property writes. Where the iOS half keeps a `FontState` in
    //! each control's `State`, this half reads the missing half back off the
    //! control's CURRENT font — the one object that always reflects the last
    //! write of either setter: [`set_text_size`] keeps the current face (a
    //! registered Glyph face is re-sized with `CTFontCreateCopyWithAttributes`,
    //! which copies the in-memory font rather than looking it up by name) and
    //! [`set_typeface`] keeps the current point size. So either setter alone
    //! leaves the other's last value intact, in either order, with no state
    //! beside the view.
    //!
    //! # Nothing here can fail
    //!
    //! Every function returns `()`, for the same reason as the iOS half: a
    //! message send to a linked AppKit class has no exception channel and no
    //! classloader to fail.

    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSAccessibility, NSButton, NSColor, NSControl, NSFont, NSImageView, NSProgressIndicator,
        NSSlider, NSSwitch, NSTextField, NSView,
    };
    use objc2_core_foundation::CFRetained;
    use objc2_core_graphics::CGColor;
    use objc2_core_text::CTFont;
    use objc2_foundation::NSString;

    use super::{Setter, argb_channels};
    use crate::controls::typeface::Typeface;

    /// A packed ARGB colour as an sRGB [`NSColor`] (see
    /// [`super::argb_channels`]) — the channels are `CGFloat`s, `f64` on every
    /// 64-bit Mac this arm builds for.
    pub(crate) fn ns_color(argb: i32) -> Retained<NSColor> {
        let (red, green, blue, alpha) = argb_channels(argb);
        NSColor::colorWithSRGBRed_green_blue_alpha(red, green, blue, alpha)
    }

    /// The same colour as an sRGB [`CGColor`], for `CALayer.backgroundColor`.
    fn cg_color(argb: i32) -> CFRetained<CGColor> {
        let (red, green, blue, alpha) = argb_channels(argb);
        CGColor::new_srgb(red, green, blue, alpha)
    }

    /// `NSControl.enabled` — [`Setter::Enabled`]. Every v1 control is an
    /// `NSControl`, so this takes the superclass and lets deref coercion do
    /// the rest.
    pub(crate) fn set_enabled(control: &NSControl, enabled: bool) {
        control.setEnabled(enabled);
    }

    /// `NSView.accessibilityLabel` (the `NSAccessibility` protocol every
    /// `NSView` conforms to) — [`Setter::ContentDescription`]'s AppKit
    /// counterpart, read by VoiceOver exactly as TalkBack reads Android's
    /// `contentDescription`. `None` clears it, so a control falls back to what
    /// AppKit derives from its own content (a button's title).
    ///
    /// Reachable only with `objc2-app-kit`'s `NSAccessibilityProtocols`
    /// feature, which `Cargo.toml` enables for exactly this member. Native
    /// controls are exposed to VoiceOver by the platform itself, never through
    /// frust's semantics pass (`crate`'s event-bypass note).
    pub(crate) fn set_accessibility_label(view: &NSView, label: Option<&str>) {
        let text = label.map(NSString::from_str);
        view.setAccessibilityLabel(text.as_deref());
    }

    /// The system font at `size_sp` points. `sp` is read as points, unscaled,
    /// for the same reason as the iOS half's `system_font` (the value the
    /// theme asked for, not a platform scaling of it).
    pub(crate) fn system_font(size_sp: f32) -> Retained<NSFont> {
        NSFont::systemFontOfSize(f64::from(size_sp))
    }

    /// `NSControl.font` at a new size — [`Setter::TextSizeSp`]'s apply for any
    /// control whose text is its own cell's (`NSButton`, `NSTextField`). Keeps
    /// the control's current face (module doc's *Size and face*).
    pub(crate) fn set_text_size(control: &NSControl, size_sp: f32) {
        let font = font_at_size(control.font().as_deref(), size_sp);
        control.setFont(Some(font.as_ns_font()));
    }

    /// `NSControl.font` in a new face — [`Setter::Typeface`] (theme ladder
    /// L3). Keeps the control's current point size (module doc's *Size and
    /// face*); a control with no font yet starts from AppKit's own
    /// `systemFontSize`, the size a fresh control's cell already renders at.
    pub(crate) fn set_typeface(control: &NSControl, typeface: Typeface) {
        let size_sp = control
            .font()
            .map_or_else(NSFont::systemFontSize, |font| font.pointSize())
            as f32;
        let font = resolve_font(typeface, size_sp);
        log::debug!(
            "frust-native-widgets: macOS L3 typeface {typeface:?} at {size_sp}pt -> {}",
            font.as_ns_font().fontName()
        );
        control.setFont(Some(font.as_ns_font()));
    }

    /// [`Setter::TextColor`] (theme ladder L2): `NSTextField.textColor`, or
    /// `NSButton.contentTintColor` (module doc's table and its
    /// borderless-only caveat). Any other class has no text colour of its own.
    pub(crate) fn set_text_color(control: &NSControl, argb: i32) {
        let color = ns_color(argb);
        if let Some(field) = control.downcast_ref::<NSTextField>() {
            field.setTextColor(Some(&color));
        } else if let Some(button) = control.downcast_ref::<NSButton>() {
            button.setContentTintColor(Some(&color));
        } else {
            no_appkit_api("TextColor", "this control has no text colour of its own");
            return;
        }
        log::debug!("frust-native-widgets: macOS L2 text colour {argb:#010x}");
    }

    /// [`Setter::BackgroundColor`] and the fill half of
    /// [`Setter::ThemedBackground`] (theme ladder L2): the view's backing
    /// layer's `backgroundColor`, making the view layer-backed first; on an
    /// `NSButton` also its push bezel's `bezelColor` (module doc's table).
    pub(crate) fn set_background_color(view: &NSView, argb: i32) {
        view.setWantsLayer(true);
        match view.layer() {
            Some(layer) => layer.setBackgroundColor(Some(&cg_color(argb))),
            None => no_appkit_api("BackgroundColor", "the view has no backing layer"),
        }
        if let Some(button) = view.downcast_ref::<NSButton>() {
            button.setBezelColor(Some(&ns_color(argb)));
        }
        log::debug!("frust-native-widgets: macOS L2 background {argb:#010x}");
    }

    /// The corner-radius half of [`Setter::ThemedBackground`] (theme ladder
    /// L2): `layer.cornerRadius` + `masksToBounds`, in points (module doc).
    pub(crate) fn set_corner_radius(view: &NSView, radius_dp: f32) {
        view.setWantsLayer(true);
        match view.layer() {
            Some(layer) => {
                layer.setCornerRadius(f64::from(radius_dp));
                layer.setMasksToBounds(radius_dp > 0.0);
            }
            None => no_appkit_api("CornerRadius", "the view has no backing layer"),
        }
    }

    /// The four tint setters ([`Setter::ProgressTint`]/[`Setter::ThumbTint`]/
    /// [`Setter::TrackTint`]/[`Setter::ImageTint`], named by `which`) — the
    /// module doc's table: `NSSlider`'s progress tint is `trackFillColor`,
    /// `NSImageView`'s tint is `contentTintColor`, and every other pairing has
    /// no AppKit API. `None` passes `nil`, restoring AppKit's own colour — the
    /// clearable contract [`Setter::ProgressTint`] documents.
    ///
    /// For `ImageTint`, `contentTintColor` only renders when the installed
    /// image is a template (`NSImageView.rs:221`'s doc,
    /// `controls::image`'s macOS module doc) — the caller (`controls::image`)
    /// marks the image template *before* calling here, so this function reads
    /// that state back rather than assuming the colour landed, and logs
    /// accordingly instead of always claiming "applied".
    pub(crate) fn set_tint(view: &NSView, which: &'static str, argb: Option<i32>) {
        let color = argb.map(ns_color);
        if let Some(slider) = view.downcast_ref::<NSSlider>() {
            if which != "ProgressTint" {
                return no_appkit_api(which, "NSSlider has no thumb colour");
            }
            slider.setTrackFillColor(color.as_deref());
        } else if let Some(image_view) = view.downcast_ref::<NSImageView>() {
            if which != "ImageTint" {
                return no_appkit_api(which, "NSImageView has only a content tint");
            }
            image_view.setContentTintColor(color.as_deref());
            let applied =
                argb.is_some() && image_view.image().is_some_and(|image| image.isTemplate());
            log::debug!(
                "frust-native-widgets: macOS L2 ImageTint({argb:?}) {}",
                if argb.is_none() {
                    "cleared — restored the original, untinted image"
                } else if applied {
                    "applied (template image)"
                } else {
                    "contentTintColor set, but no template image is installed — no visible tint"
                }
            );
            return;
        } else if view.downcast_ref::<NSSwitch>().is_some() {
            return no_appkit_api(
                which,
                "NSSwitch has no thumb or track colour (it draws in the system accent colour)",
            );
        } else if view.downcast_ref::<NSProgressIndicator>().is_some() {
            return no_appkit_api(
                which,
                "NSProgressIndicator has no tint (the system accent colour; controlTint is \
                 deprecated and not used)",
            );
        } else {
            return no_appkit_api(which, "this view has no tint property");
        }
        log::debug!("frust-native-widgets: macOS L2 {which}({argb:?})");
    }

    /// A planned setter AppKit has no property for on this class — module
    /// doc's *No AppKit API* list. Debug level: it is a documented platform
    /// gap, not a defect, and a theme flip would otherwise warn per control.
    fn no_appkit_api(what: &str, why: &str) {
        log::debug!("frust-native-widgets: macOS {what} not applied — {why}");
    }

    /// A resolved [`NSFont`], from either the system font or a registered
    /// Glyph face — the macOS twin of the iOS half's `ResolvedFont`, unified
    /// behind [`Self::as_ns_font`] so a caller never branches on which arm it
    /// got (theme ladder L3).
    ///
    /// [`Self::Glyph`] holds a **sized** `CTFont`, bridged to `NSFont` via
    /// `objc2-app-kit`'s `AsRef<NSFont> for CTFont` — a toll-free bridge (the
    /// same underlying object, not a cast), gated by this crate's
    /// `objc2-core-text` feature on `objc2-app-kit` (`Cargo.toml`).
    pub(crate) enum ResolvedFont {
        /// [`system_font`] — [`Typeface::System`], or the degrade target of a
        /// Glyph resolution failure.
        System(Retained<NSFont>),
        /// A registered Glyph face, sized for this call.
        Glyph(CFRetained<CTFont>),
    }

    impl ResolvedFont {
        /// Borrow this as a `&NSFont`, whichever arm it is — every AppKit
        /// setter this crate calls (`NSControl.setFont:`) takes exactly this.
        pub(crate) fn as_ns_font(&self) -> &NSFont {
            match self {
                Self::System(font) => font,
                Self::Glyph(font) => font.as_ref(),
            }
        }
    }

    /// Resolve `typeface` to a real [`NSFont`] at `size_sp` points — the
    /// macOS half of theme ladder L3, the iOS half's `resolve_font` over the
    /// same shared `crate::coretext` descriptor cache: [`Typeface::System`] is
    /// the system font; a Glyph face resolves through its cached
    /// `CTFontDescriptor`, degrading to the system font (with that module's
    /// one-time warning) on any failure.
    pub(crate) fn resolve_font(typeface: Typeface, size_sp: f32) -> ResolvedFont {
        match crate::coretext::descriptor_for(typeface) {
            Some(descriptor) => {
                // SAFETY: `descriptor` is a live `CTFontDescriptor` owned for
                // this call (`descriptor_for` hands back a fresh `CFRetained`
                // clone); `size_sp` is a finite point size and the null matrix
                // is `CTFontCreateWithFontDescriptor`'s documented "identity
                // matrix" default.
                let font = unsafe {
                    CTFont::with_font_descriptor(&descriptor, f64::from(size_sp), std::ptr::null())
                };
                ResolvedFont::Glyph(font)
            }
            None => ResolvedFont::System(system_font(size_sp)),
        }
    }

    /// `current` re-sized to `size_sp` points, keeping its face — module
    /// doc's *Size and face*. The system font (or no font at all) resolves
    /// through [`system_font`], exactly as before the ladder existed; any
    /// other face — in practice a registered Glyph face — is copied at the new
    /// size with `CTFontCreateCopyWithAttributes`, which works from the font
    /// object itself, so an in-memory face that was never registered by name
    /// survives the resize.
    fn font_at_size(current: Option<&NSFont>, size_sp: f32) -> ResolvedFont {
        match current {
            Some(font) if !is_system_face(font) => {
                let face: &CTFont = font.as_ref();
                // SAFETY: `face` is the live `CTFont` behind `font` (toll-free
                // bridged) for this call; the null matrix and `None`
                // attributes are `CTFontCreateCopyWithAttributes`'s documented
                // "keep the original's" defaults, and the point size is finite.
                let sized = unsafe {
                    face.copy_with_attributes(f64::from(size_sp), std::ptr::null(), None)
                };
                ResolvedFont::Glyph(sized)
            }
            _ => ResolvedFont::System(system_font(size_sp)),
        }
    }

    /// Whether `font` is AppKit's system face (family compared with the
    /// system font's own), the only non-Glyph face this arm ever installs.
    fn is_system_face(font: &NSFont) -> bool {
        font.familyName() == system_font(0.0).familyName()
    }

    /// A [`Setter`] a control's own `plan` never emits reached its macOS
    /// `apply` — the iOS half's warning, for this arm.
    pub(crate) fn warn_unexpected_setter(kind: &str, setter: &Setter<'_>) {
        log::warn!(
            "frust-native-widgets: control '{kind}' planned a setter its macOS arm does not \
             implement ({setter:?}) — ignored"
        );
    }

    // `NSFont`/`CTFont`/`NSColor` are not main-thread-only, so the L2 colour
    // and L3 size/face logic runs on a plain `cargo test` worker; everything
    // that needs a live `NSView` is exercised by the playground gate instead.
    #[cfg(test)]
    mod tests {
        use super::*;

        /// ONE test, on purpose: AppKit's font system initializes lazily, and
        /// two `cargo test` workers touching it concurrently off the main
        /// thread were observed to get `Helvetica` back from
        /// `systemFontOfSize:` on one of them (roughly 1 run in 4 with these
        /// cases split across tests). Production only ever calls this on the
        /// main thread; keeping every `NSFont` call here on one worker makes
        /// the test deterministic without claiming otherwise.
        #[test]
        fn l3_resizes_keep_the_face_and_faces_keep_the_size() {
            // The system face re-sizes as the system font.
            let resized = font_at_size(Some(&system_font(13.0)), 21.0);
            assert!(matches!(resized, ResolvedFont::System(_)));
            assert_eq!(resized.as_ns_font().pointSize(), 21.0);
            assert!(is_system_face(resized.as_ns_font()));

            // No font yet: the system font at the requested size.
            let fresh = font_at_size(None, 15.0);
            assert!(matches!(fresh, ResolvedFont::System(_)));
            assert_eq!(fresh.as_ns_font().pointSize(), 15.0);

            // `Typeface::System` is the system font at the given size.
            let system = resolve_font(Typeface::System, 17.0);
            assert!(matches!(system, ResolvedFont::System(_)));
            assert_eq!(system.as_ns_font().pointSize(), 17.0);

            // A fixed-pitch face stands in for a registered Glyph face: both
            // are "not the system face", so both take the CTFont-copy path,
            // keeping their family across the resize.
            if let Some(mono) = NSFont::userFixedPitchFontOfSize(11.0) {
                let resized = font_at_size(Some(&mono), 24.0);
                assert!(matches!(resized, ResolvedFont::Glyph(_)));
                assert_eq!(resized.as_ns_font().pointSize(), 24.0);
                assert_eq!(resized.as_ns_font().familyName(), mono.familyName());
                assert!(!is_system_face(resized.as_ns_font()));
            }
        }

        #[test]
        fn argb_becomes_the_matching_srgb_colour() {
            let color = ns_color(0x80FF_0000_u32 as i32);
            assert!((color.alphaComponent() - 128.0 / 255.0).abs() < 1e-6);
            assert!((color.redComponent() - 1.0).abs() < 1e-6);
        }
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
            Setter::SelectedSegment(Some(1)),
            Setter::Momentary(true),
            Setter::SegmentTint(None),
            Setter::Step(1),
            Setter::Wraps(true),
            Setter::StepperTint(None),
            Setter::Date(CivilDate::MIN),
            Setter::DatePickerTint(None),
            Setter::DatePickerTextColor(Some(1)),
            Setter::SelectedTab(Some(0)),
            Setter::TabBarTint(None),
            Setter::TabBarUnselectedTint(Some(1)),
            Setter::TabBarBackground(None),
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
            Setter::Segments(&[]),
            Setter::MinDate(None),
            Setter::MaxDate(Some(CivilDate::MAX)),
            Setter::DatePickerStyle(DatePickerStyle::Inline),
            Setter::TabItems(&[]),
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

    // --- the Apple arm's shared pure logic -----------------------------------

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

    // --- shared-Props parity: three kind tables, three platform arms -------

    #[test]
    fn the_kind_tables_are_the_shipped_wire_strings() {
        // What every arm registers *by*: [`SHARED_KINDS`] (all three arms),
        // [`APPLE_KINDS`] (iOS + macOS only) and [`IOS_ONLY_KINDS`] (iOS
        // only). No arm spells a kind literally
        // (`crate::android`'s own registration note), so a drift in the wire
        // vocabulary has to pass through here.
        //
        // The other half of "shared-Props parity" needs no assertion at all:
        // there is exactly ONE `Props` type per control, in this same module
        // tree, used verbatim by every arm that registers it — same fields by
        // construction, not by agreement. A second, per-platform Props
        // definition is the thing this file's layout exists to prevent.
        assert_eq!(
            SHARED_KINDS,
            [
                "button",
                "label",
                "switch",
                "slider",
                "progress",
                "image",
                "spinner",
                "date_picker"
            ],
            "the control kind strings are a shipped wire contract — the api \
             layer's builders inject them and all three platform arms \
             register against them"
        );
        assert_eq!(
            APPLE_KINDS,
            ["segmented", "stepper"],
            "the Apple-arm-only kinds are a shipped wire contract too"
        );
        assert_eq!(
            IOS_ONLY_KINDS,
            ["tab_bar"],
            "the iOS-only kinds are a shipped wire contract too"
        );
        let mut unique: Vec<&str> = SHARED_KINDS
            .iter()
            .chain(&APPLE_KINDS)
            .chain(&IOS_ONLY_KINDS)
            .copied()
            .collect();
        let total = unique.len();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            total,
            "two controls share a kind (or a kind sits in two tables)"
        );
    }

    /// The kind every `runtime.register::<…>(<module>::KIND)` line in `source`
    /// names, in file order — resolved through the same `KIND` consts the
    /// tables hold, never a literal.
    fn registered_kinds(source: &str, file: &str) -> Vec<&'static str> {
        source
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("runtime.register::<"))
            .map(|line| {
                let module = line
                    .split_once(">(")
                    .and_then(|(_, arg)| arg.strip_suffix("::KIND);"))
                    .unwrap_or_else(|| panic!("{file}: unparsed registration line {line:?}"));
                match module {
                    "button" => button::KIND,
                    "label" => label::KIND,
                    "switch" => switch::KIND,
                    "slider" => slider::KIND,
                    "progress" => progress::KIND,
                    "image" => image::KIND,
                    "spinner" => spinner::KIND,
                    "date_picker" => date_picker::KIND,
                    "segmented" => segmented::KIND,
                    "stepper" => stepper::KIND,
                    "tab_bar" => tab_bar::KIND,
                    other => panic!(
                        "{file}: registers `{other}::KIND`, a module this test does not know"
                    ),
                }
            })
            .collect()
    }

    #[test]
    fn every_register_controls_registers_exactly_its_kind_tables() {
        // `crate::android::register_controls`, `crate::apple::register_controls`
        // and `crate::appkit::register_controls` are each
        // `#[cfg(target_os = ...)]`-gated, so no single host test can CALL all
        // three — but every host can READ all three. Each arm must register
        // exactly its tables, in table order, line for line: Android the
        // shared kinds only (an Apple-only kind registered there would be a
        // control with no Android `NativeWidget` impl — it would not even
        // compile), macOS the shared kinds then the Apple ones, and iOS the
        // shared kinds, the Apple ones, then the iOS-only ones — per-arm
        // membership, the kind-table matrix above `SHARED_KINDS`.
        let shared: Vec<&str> = SHARED_KINDS.to_vec();
        let macos: Vec<&str> = SHARED_KINDS.iter().chain(&APPLE_KINDS).copied().collect();
        let ios: Vec<&str> = SHARED_KINDS
            .iter()
            .chain(&APPLE_KINDS)
            .chain(&IOS_ONLY_KINDS)
            .copied()
            .collect();
        for (file, source, expected) in [
            ("android/mod.rs", include_str!("../android/mod.rs"), &shared),
            ("apple/mod.rs", include_str!("../apple/mod.rs"), &ios),
            ("appkit/mod.rs", include_str!("../appkit/mod.rs"), &macos),
        ] {
            assert_eq!(
                &registered_kinds(source, file),
                expected,
                "{file}'s register_controls drifted from SHARED_KINDS/APPLE_KINDS/IOS_ONLY_KINDS"
            );
        }
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
