//! The six app-facing builders (native-widgets Phase 1, p1-06): `native_button`/
//! `native_label`/`native_switch`/`native_slider`/`native_progress`/
//! `native_image`, each composing exactly one [`platform_view`] slot behind
//! this crate's one Android factory (PLAN.md 3.1's "N controls = N slots"
//! envelope).
//!
//! # Retained identity via `Component`, not a hand-rolled `Widget`
//!
//! Each builder is a plain, `Clone`-able data struct implementing
//! [`Component`] (`frust-core`'s retained-local-state seam,
//! `docs/ARCHITECTURE.md`'s Component state boundary): `Component::State` is
//! a bare [`SlotId`], allocated once in `Component::init` and retained across
//! every rebuild by the `ComponentWidget` frust-core builds around it. That
//! stable id is what gets injected into `params_json` as `__frustSlot`
//! (`crate::runtime`'s generic-factory contract) — the builder function
//! itself runs fresh every rebuild (a new struct value, current props), but
//! the SAME slot id round-trips through every `create`/`update_params` call
//! this plugin's runtime ever sees for that widget instance. Note this slot
//! id is this plugin's OWN bookkeeping key (drawn from a private counter,
//! below) — it never needs to equal `frust_core::widget::next_slot_id()`'s
//! differ-facing id for the SAME `platform_view` widget, because nothing on
//! the platform side ever compares the two: `create`/`update_params` key
//! `NativeRuntime`'s own registry by whatever `__frustSlot` says, and
//! `disposeView` resolves by native-view **object identity**, never a slot
//! id at all (`crate::android`'s module doc, "Which call carries the slot
//! id").
//!
//! Each builder therefore implements `View<Outer>` for **every** `Outer` by
//! hand-delegating to [`frust_core::component`] (mirroring
//! `ComponentView<C>`'s own blanket impl) — see the [`impl_native_view!`]
//! macro at the bottom of this file.
//!
//! # No public `NativeWidget` trait
//!
//! None of this reaches for `crate::runtime::NativeWidget` (which stays
//! `pub(crate)`, Phase 3's business) — a builder only ever calls the
//! runtime's already-`pub(crate)` `with_runtime`/`set_callback` seam, same
//! crate.
//!
//! # The translucency-refused fallback (Ed's Phase 1 ruling)
//!
//! Every builder consults `frust::resolved_surface_mode()` before composing
//! its native slot: on [`ResolvedSurfaceMode::RefusedTranslucent`], it
//! renders [`placeholder`] instead — a frust-drawn, semantics-labelled box —
//! rather than an invisible, untappable native slot
//! (`docs/ARCHITECTURE.md`'s Platform-view flow: "App Rust now is told ...
//! so a plugin can fall back deliberately instead of a dead slot"). The
//! refusal is logged once, crate-wide, not once per control per frame.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Once};

use frust::glyph::{AlertVariant, alert};
use frust::{
    PlatformViewView, ResolvedSurfaceMode, SizedBox, platform_view, resolved_surface_mode,
};
use frust_core::{
    AnyView, BuildCtx, ChangeFlags, Component, ComponentWidget, View, any, component,
};

use crate::controls::{
    CHECKED, CONTENT_DESCRIPTION, ENABLED, FIT, INDETERMINATE, MAX, MIN, TEXT, VALUE,
};
use crate::controls::{button, image, label, progress, slider, switch};
use crate::registry::SlotId;
use crate::runtime::{escape, with_identity, with_runtime};

use super::signals::{on_click, on_toggled, on_value_changed};

/// The one Android factory class every control resolves through
/// (`dev.frust.FrustNativeControlFactory`, this crate's canonical Kotlin
/// file — this task hand-copies it into the catalog app module; Phase 3
/// packages it as a Gradle module, `crate`'s module doc). iOS's Phase 2
/// factory has not landed yet (`p2-01`); the bare-runtime-name spelling below
/// is a placeholder matching `platform_views.rs`'s own per-target
/// `DEMO_STREAM_VIEW_TYPE` precedent, unused until then.
#[cfg(target_os = "android")]
const VIEW_TYPE: &str = "dev.frust.FrustNativeControlFactory";
#[cfg(target_os = "ios")]
const VIEW_TYPE: &str = "FrustNativeControlFactory";
#[cfg(not(any(target_os = "android", target_os = "ios")))]
const VIEW_TYPE: &str = "dev.frust.FrustNativeControlFactory";

/// This plugin's own per-widget-instance identity counter (module doc: "this
/// slot id is this plugin's OWN bookkeeping key"). Deliberately independent
/// of `frust_core::widget::next_slot_id()` — nothing on the platform side
/// ever compares the two, so a private counter avoids reaching into
/// `frust-core`'s widget-tree internals for a value nothing downstream reads
/// as a differ id.
fn next_local_slot() -> SlotId {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Log the translucency-refused fallback exactly once, crate-wide — never
/// once per control, never once per frame (the task's "Log once" rule).
static REFUSAL_LOGGED: Once = Once::new();

fn warn_refusal_once() {
    REFUSAL_LOGGED.call_once(|| {
        log::warn!(
            "frust-native-widgets: the host declared a translucent surface but the platform \
             refused it (ResolvedSurfaceMode::RefusedTranslucent) — rendering frust-drawn \
             placeholders instead of native controls; see docs/ARCHITECTURE.md's Platform-view \
             flow"
        );
    });
}

/// The frust-drawn placeholder every builder degrades to under
/// [`ResolvedSurfaceMode::RefusedTranslucent`]: a sized box, a warning fill
/// (`frust::glyph::alert`'s `Warning` variant, which also contributes a
/// `Role::Alert` semantics node carrying `control` as its label — the
/// "semantics label" half of the task's fallback spec), instead of an
/// invisible, untappable native slot.
fn placeholder<State: 'static>(size: Option<(f64, f64)>, control: &str) -> AnyView<State> {
    warn_refusal_once();
    let banner = alert(
        AlertVariant::Warning,
        format!("Native {control} unavailable"),
        "the host declared a translucent surface but the platform refused it — rendering a \
         frust placeholder instead of an invisible native slot.",
    );
    any(SizedBox(size.map(|(w, _)| w), size.map(|(_, h)| h)).child(banner))
}

/// Apply an explicit `.size(w, h)` if the caller provided one, else leave the
/// slot at [`PlatformViewView`]'s own default (fill the parent) — v1 has no
/// measure step either way (the task spec: "explicit `.size(w,h)` required
/// (no measure in v1)"), so an omitted call degrades to filling the parent
/// rather than a made-up constant.
fn resolve_size(size: Option<(f64, f64)>, view: PlatformViewView) -> PlatformViewView {
    match size {
        Some((w, h)) => view.size(w, h),
        None => view,
    }
}

/// A tiny flat-JSON body writer — this crate hand-rolls JSON at the wire
/// boundary rather than pulling in `serde` (`docs/CODE_STANDARDS.md`'s
/// Language Idioms; `crate::runtime::Params`/`with_identity` are the
/// reader/identity-encoder halves this writes the *body* half for). Only the
/// handful of primitive field shapes the six controls need.
struct ParamsBody(String);

impl ParamsBody {
    fn new() -> Self {
        Self(String::new())
    }

    fn push_key(&mut self, key: &str) {
        if !self.0.is_empty() {
            self.0.push(',');
        }
        self.0.push('"');
        self.0.push_str(key);
        self.0.push_str("\":");
    }

    /// A raw (unquoted) literal — a bool or integer, whose `Display` already
    /// matches JSON's own spelling (`true`/`false`, plain digits).
    fn push_raw(&mut self, key: &str, value: impl std::fmt::Display) {
        self.push_key(key);
        self.0.push_str(&value.to_string());
    }

    /// A JSON string value, escaped via [`crate::runtime::escape`].
    fn push_str(&mut self, key: &str, value: &str) {
        self.push_key(key);
        self.0.push('"');
        self.0.push_str(&escape(value));
        self.0.push('"');
    }

    /// A string field only when present — a missing optional field decodes
    /// to the control's own platform default (`crate::controls`'s
    /// degrade-don't-fail rule), so omitting the key entirely is correct.
    fn push_opt_str(&mut self, key: &str, value: Option<&str>) {
        if let Some(v) = value {
            self.push_str(key, v);
        }
    }

    fn finish(self) -> String {
        self.0
    }
}

// ============================================================================
// Button
// ============================================================================

/// A real `android.widget.Button` rendered from pure Rust — see the
/// [module docs](self). Build one with [`native_button`].
#[derive(Clone)]
pub struct NativeButtonView {
    text: String,
    enabled: bool,
    content_description: Option<String>,
    size: Option<(f64, f64)>,
    on_press: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// A native `Button` captioned `text` — see [`NativeButtonView`].
pub fn native_button(text: impl Into<String>) -> NativeButtonView {
    NativeButtonView {
        text: text.into(),
        enabled: true,
        content_description: None,
        size: None,
        on_press: None,
    }
}

impl NativeButtonView {
    /// `View.setEnabled` — default `true`.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The TalkBack label; falls back to the caption when unset.
    pub fn content_description(mut self, label: impl Into<String>) -> Self {
        self.content_description = Some(label.into());
        self
    }

    /// Explicit slot size — see [`resolve_size`]'s doc for the no-call
    /// fallback.
    pub fn size(mut self, width: f64, height: f64) -> Self {
        self.size = Some((width, height));
        self
    }

    /// Fires on a tap, on the platform main thread (events-as-signals — see
    /// `crate::api::signals`): write an `RwSignal` from inside for the
    /// blessed one-frame-wake idiom.
    pub fn on_press(mut self, handler: impl Fn() + Send + Sync + 'static) -> Self {
        self.on_press = Some(Arc::new(handler));
        self
    }

    /// The encoded `params_json` for `slot` — split out from
    /// [`Component::build`] so a test can snapshot it directly.
    fn params_for(&self, slot: SlotId) -> String {
        let mut body = ParamsBody::new();
        body.push_str(TEXT, &self.text);
        body.push_raw(ENABLED, self.enabled);
        body.push_opt_str(CONTENT_DESCRIPTION, self.content_description.as_deref());
        with_identity(button::KIND, slot, &body.finish())
    }

    /// [`Component::build`]'s real body, with `mode` threaded explicitly so a
    /// test can force the [`ResolvedSurfaceMode::RefusedTranslucent`] branch
    /// without touching the process-global resolved-mode slot (whose writer
    /// is pinned to the two shells' own FFI glue,
    /// `crates/frust/tests/surface_mode_conformance.rs`).
    fn build_with_mode(&self, slot: SlotId, mode: ResolvedSurfaceMode) -> AnyView<SlotId> {
        if mode.translucency_refused() {
            return placeholder(self.size, "Button");
        }
        let params = self.params_for(slot);
        if let Some(on_press) = self.on_press.clone() {
            with_runtime(|rt| rt.set_callback(slot, on_click(on_press)));
        }
        let view = platform_view(VIEW_TYPE)
            .params_json(params)
            .interactive()
            .semantics_label(
                self.content_description
                    .clone()
                    .unwrap_or_else(|| self.text.clone()),
            );
        any(resolve_size(self.size, view))
    }
}

impl Component for NativeButtonView {
    type State = SlotId;

    fn init(&self) -> SlotId {
        next_local_slot()
    }

    fn build(&self, state: &mut SlotId) -> AnyView<SlotId> {
        self.build_with_mode(*state, resolved_surface_mode())
    }
}

// ============================================================================
// Label
// ============================================================================

/// A real `android.widget.TextView` rendered from pure Rust — display-only
/// (no listener, no `.interactive()`). Build one with [`native_label`].
#[derive(Clone)]
pub struct NativeLabelView {
    text: String,
    enabled: bool,
    content_description: Option<String>,
    size: Option<(f64, f64)>,
}

/// A native `Label` showing `text` — see [`NativeLabelView`].
pub fn native_label(text: impl Into<String>) -> NativeLabelView {
    NativeLabelView {
        text: text.into(),
        enabled: true,
        content_description: None,
        size: None,
    }
}

impl NativeLabelView {
    /// `View.setEnabled` — a `TextView` renders its disabled state through
    /// the colour state list, so this is visible even without interaction.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The TalkBack label; falls back to `text` when unset.
    pub fn content_description(mut self, label: impl Into<String>) -> Self {
        self.content_description = Some(label.into());
        self
    }

    /// Explicit slot size — see [`resolve_size`]'s doc for the no-call
    /// fallback.
    pub fn size(mut self, width: f64, height: f64) -> Self {
        self.size = Some((width, height));
        self
    }

    fn params_for(&self, slot: SlotId) -> String {
        let mut body = ParamsBody::new();
        body.push_str(TEXT, &self.text);
        body.push_raw(ENABLED, self.enabled);
        body.push_opt_str(CONTENT_DESCRIPTION, self.content_description.as_deref());
        with_identity(label::KIND, slot, &body.finish())
    }

    fn build_with_mode(&self, slot: SlotId, mode: ResolvedSurfaceMode) -> AnyView<SlotId> {
        if mode.translucency_refused() {
            return placeholder(self.size, "Label");
        }
        let params = self.params_for(slot);
        let view = platform_view(VIEW_TYPE)
            .params_json(params)
            .semantics_label(
                self.content_description
                    .clone()
                    .unwrap_or_else(|| self.text.clone()),
            );
        any(resolve_size(self.size, view))
    }
}

impl Component for NativeLabelView {
    type State = SlotId;

    fn init(&self) -> SlotId {
        next_local_slot()
    }

    fn build(&self, state: &mut SlotId) -> AnyView<SlotId> {
        self.build_with_mode(*state, resolved_surface_mode())
    }
}

// ============================================================================
// Switch
// ============================================================================

/// A real `android.widget.Switch` rendered from pure Rust — a **controlled**
/// component (`docs/CODE_STANDARDS.md`'s Interaction Semantics): the app owns
/// `checked`, and a user toggle only ever arrives through [`Self::on_toggle`]
/// as a *requested* value. Build one with [`native_switch`].
#[derive(Clone)]
pub struct NativeSwitchView {
    checked: bool,
    enabled: bool,
    content_description: Option<String>,
    size: Option<(f64, f64)>,
    on_toggle: Option<Arc<dyn Fn(bool) + Send + Sync>>,
}

/// A native `Switch` at the app-owned `checked` state — see
/// [`NativeSwitchView`].
pub fn native_switch(checked: bool) -> NativeSwitchView {
    NativeSwitchView {
        checked,
        enabled: true,
        content_description: None,
        size: None,
        on_toggle: None,
    }
}

impl NativeSwitchView {
    /// `View.setEnabled` — default `true`.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The TalkBack label.
    pub fn content_description(mut self, label: impl Into<String>) -> Self {
        self.content_description = Some(label.into());
        self
    }

    /// Explicit slot size — see [`resolve_size`]'s doc for the no-call
    /// fallback.
    pub fn size(mut self, width: f64, height: f64) -> Self {
        self.size = Some((width, height));
        self
    }

    /// Fires with the *requested* checked state on a user toggle — the app
    /// confirms (or rejects) it by feeding `checked` back through the next
    /// build, the controlled-component contract every interactive frust
    /// widget follows.
    pub fn on_toggle(mut self, handler: impl Fn(bool) + Send + Sync + 'static) -> Self {
        self.on_toggle = Some(Arc::new(handler));
        self
    }

    fn params_for(&self, slot: SlotId) -> String {
        let mut body = ParamsBody::new();
        body.push_raw(CHECKED, self.checked);
        body.push_raw(ENABLED, self.enabled);
        body.push_opt_str(CONTENT_DESCRIPTION, self.content_description.as_deref());
        with_identity(switch::KIND, slot, &body.finish())
    }

    fn build_with_mode(&self, slot: SlotId, mode: ResolvedSurfaceMode) -> AnyView<SlotId> {
        if mode.translucency_refused() {
            return placeholder(self.size, "Switch");
        }
        let params = self.params_for(slot);
        if let Some(on_toggle) = self.on_toggle.clone() {
            with_runtime(|rt| rt.set_callback(slot, on_toggled(on_toggle)));
        }
        let view = platform_view(VIEW_TYPE)
            .params_json(params)
            .interactive()
            .semantics_label(
                self.content_description
                    .clone()
                    .unwrap_or_else(|| "switch".into()),
            );
        any(resolve_size(self.size, view))
    }
}

impl Component for NativeSwitchView {
    type State = SlotId;

    fn init(&self) -> SlotId {
        next_local_slot()
    }

    fn build(&self, state: &mut SlotId) -> AnyView<SlotId> {
        self.build_with_mode(*state, resolved_surface_mode())
    }
}

// ============================================================================
// Slider
// ============================================================================

/// A real `android.widget.SeekBar` rendered from pure Rust — controlled, like
/// [`NativeSwitchView`]: the app owns `value`, and a drag only ever arrives
/// through [`Self::on_change`] as a requested value. Build one with
/// [`native_slider`].
#[derive(Clone)]
pub struct NativeSliderView {
    value: i32,
    min: i32,
    max: i32,
    enabled: bool,
    content_description: Option<String>,
    size: Option<(f64, f64)>,
    on_change: Option<Arc<dyn Fn(i32) + Send + Sync>>,
}

/// A native `Slider` at `value`, ranging over `[min, max]` — see
/// [`NativeSliderView`].
pub fn native_slider(value: i32, min: i32, max: i32) -> NativeSliderView {
    NativeSliderView {
        value,
        min,
        max,
        enabled: true,
        content_description: None,
        size: None,
        on_change: None,
    }
}

impl NativeSliderView {
    /// `View.setEnabled` — default `true`.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The TalkBack label.
    pub fn content_description(mut self, label: impl Into<String>) -> Self {
        self.content_description = Some(label.into());
        self
    }

    /// Explicit slot size — see [`resolve_size`]'s doc for the no-call
    /// fallback.
    pub fn size(mut self, width: f64, height: f64) -> Self {
        self.size = Some((width, height));
        self
    }

    /// Fires with the requested **app-space** value on a drag
    /// (`crate::controls::slider`'s platform-space mapping already undone).
    pub fn on_change(mut self, handler: impl Fn(i32) + Send + Sync + 'static) -> Self {
        self.on_change = Some(Arc::new(handler));
        self
    }

    fn params_for(&self, slot: SlotId) -> String {
        let mut body = ParamsBody::new();
        body.push_raw(VALUE, self.value);
        body.push_raw(MIN, self.min);
        body.push_raw(MAX, self.max);
        body.push_raw(ENABLED, self.enabled);
        body.push_opt_str(CONTENT_DESCRIPTION, self.content_description.as_deref());
        with_identity(slider::KIND, slot, &body.finish())
    }

    fn build_with_mode(&self, slot: SlotId, mode: ResolvedSurfaceMode) -> AnyView<SlotId> {
        if mode.translucency_refused() {
            return placeholder(self.size, "Slider");
        }
        let params = self.params_for(slot);
        if let Some(on_change) = self.on_change.clone() {
            with_runtime(|rt| rt.set_callback(slot, on_value_changed(on_change)));
        }
        let view = platform_view(VIEW_TYPE)
            .params_json(params)
            .interactive()
            .semantics_label(
                self.content_description
                    .clone()
                    .unwrap_or_else(|| "slider".into()),
            );
        any(resolve_size(self.size, view))
    }
}

impl Component for NativeSliderView {
    type State = SlotId;

    fn init(&self) -> SlotId {
        next_local_slot()
    }

    fn build(&self, state: &mut SlotId) -> AnyView<SlotId> {
        self.build_with_mode(*state, resolved_surface_mode())
    }
}

// ============================================================================
// Progress
// ============================================================================

/// A real `android.widget.ProgressBar` rendered from pure Rust — display-only
/// (no listener, no `.interactive()`). Build one with [`native_progress`].
#[derive(Clone)]
pub struct NativeProgressView {
    value: i32,
    min: i32,
    max: i32,
    indeterminate: bool,
    content_description: Option<String>,
    size: Option<(f64, f64)>,
}

/// A native `ProgressBar` at `value`, ranging over `[min, max]` — see
/// [`NativeProgressView`].
pub fn native_progress(value: i32, min: i32, max: i32) -> NativeProgressView {
    NativeProgressView {
        value,
        min,
        max,
        indeterminate: false,
        content_description: None,
        size: None,
    }
}

impl NativeProgressView {
    /// Spinner mode: `true` ignores `value` entirely.
    pub fn indeterminate(mut self, indeterminate: bool) -> Self {
        self.indeterminate = indeterminate;
        self
    }

    /// The TalkBack label.
    pub fn content_description(mut self, label: impl Into<String>) -> Self {
        self.content_description = Some(label.into());
        self
    }

    /// Explicit slot size — see [`resolve_size`]'s doc for the no-call
    /// fallback.
    pub fn size(mut self, width: f64, height: f64) -> Self {
        self.size = Some((width, height));
        self
    }

    fn params_for(&self, slot: SlotId) -> String {
        let mut body = ParamsBody::new();
        body.push_raw(VALUE, self.value);
        body.push_raw(MIN, self.min);
        body.push_raw(MAX, self.max);
        body.push_raw(INDETERMINATE, self.indeterminate);
        body.push_opt_str(CONTENT_DESCRIPTION, self.content_description.as_deref());
        with_identity(progress::KIND, slot, &body.finish())
    }

    fn build_with_mode(&self, slot: SlotId, mode: ResolvedSurfaceMode) -> AnyView<SlotId> {
        if mode.translucency_refused() {
            return placeholder(self.size, "ProgressBar");
        }
        let params = self.params_for(slot);
        let view = platform_view(VIEW_TYPE)
            .params_json(params)
            .semantics_label(
                self.content_description
                    .clone()
                    .unwrap_or_else(|| "progress".into()),
            );
        any(resolve_size(self.size, view))
    }
}

impl Component for NativeProgressView {
    type State = SlotId;

    fn init(&self) -> SlotId {
        next_local_slot()
    }

    fn build(&self, state: &mut SlotId) -> AnyView<SlotId> {
        self.build_with_mode(*state, resolved_surface_mode())
    }
}

// ============================================================================
// Image
// ============================================================================

/// How a [`NativeImageView`] scales its bytes into the slot's box — the
/// public mirror of `crate::controls::image::Fit`, which stays `pub(crate)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NativeImageFit {
    /// Whole image, aspect kept, centred (the platform's own default).
    #[default]
    Contain,
    /// Fills the box, aspect kept, cropped.
    Cover,
    /// Fills the box, aspect ignored.
    Fill,
    /// No scaling at all, centred.
    Center,
}

impl NativeImageFit {
    /// The wire spelling `crate::controls::image::Fit::from_params` decodes.
    fn wire(self) -> &'static str {
        match self {
            Self::Contain => "contain",
            Self::Cover => "cover",
            Self::Fill => "fill",
            Self::Center => "center",
        }
    }
}

/// A real `android.widget.ImageView` rendered from pure Rust, showing
/// app-supplied encoded bytes (PNG/JPEG/WebP — whatever `BitmapFactory`
/// reads). Display-only (no listener, no `.interactive()`). Build one with
/// [`native_image`].
#[derive(Clone)]
pub struct NativeImageView {
    bytes: Arc<[u8]>,
    fit: NativeImageFit,
    content_description: Option<String>,
    size: Option<(f64, f64)>,
}

/// A native `Image` showing `bytes` — see [`NativeImageView`].
pub fn native_image(bytes: Arc<[u8]>) -> NativeImageView {
    NativeImageView {
        bytes,
        fit: NativeImageFit::default(),
        content_description: None,
        size: None,
    }
}

impl NativeImageView {
    /// How the image scales into the slot's box.
    pub fn fit(mut self, fit: NativeImageFit) -> Self {
        self.fit = fit;
        self
    }

    /// The TalkBack label. An unlabelled image is invisible to a screen
    /// reader — set this (or say so explicitly with an empty string).
    pub fn content_description(mut self, label: impl Into<String>) -> Self {
        self.content_description = Some(label.into());
        self
    }

    /// Explicit slot size — see [`resolve_size`]'s doc for the no-call
    /// fallback.
    pub fn size(mut self, width: f64, height: f64) -> Self {
        self.size = Some((width, height));
        self
    }

    /// The encoded `params_json` for `slot`, given the publish revision
    /// [`crate::controls::image::publish_bytes`] already returned — split out
    /// from [`Self::build_with_mode`] so a test can snapshot it without
    /// re-publishing.
    fn params_for(&self, slot: SlotId, rev: u64) -> String {
        let mut body = ParamsBody::new();
        body.push_raw(image::REV, rev);
        body.push_str(FIT, self.fit.wire());
        body.push_opt_str(CONTENT_DESCRIPTION, self.content_description.as_deref());
        with_identity(image::KIND, slot, &body.finish())
    }

    fn build_with_mode(&self, slot: SlotId, mode: ResolvedSurfaceMode) -> AnyView<SlotId> {
        if mode.translucency_refused() {
            return placeholder(self.size, "Image");
        }
        // Publish (or re-confirm) this slot's bytes BEFORE encoding params —
        // `image::publish_bytes` is idempotent for the same `Arc` (module
        // doc: "an app that hands its buffer down every rebuild produces no
        // params change and no decode"), and the runtime's later
        // `ImageProps::decode` reads this same table back by slot.
        let rev = image::publish_bytes(slot, Arc::clone(&self.bytes));
        let params = self.params_for(slot, rev);
        let view = platform_view(VIEW_TYPE)
            .params_json(params)
            .semantics_label(
                self.content_description
                    .clone()
                    .unwrap_or_else(|| "image".into()),
            );
        any(resolve_size(self.size, view))
    }
}

impl Component for NativeImageView {
    type State = SlotId;

    fn init(&self) -> SlotId {
        next_local_slot()
    }

    fn build(&self, state: &mut SlotId) -> AnyView<SlotId> {
        self.build_with_mode(*state, resolved_surface_mode())
    }
}

// ============================================================================
// The `View<Outer>` delegate (module doc)
// ============================================================================

/// Implement `View<Outer>` for every `Outer` state by delegating to
/// `frust_core::component` — mirroring `ComponentView<C>`'s own blanket impl,
/// which this crate cannot reach directly (its `component: C` field is
/// private): each call clones `self`/`prev` into a throwaway `ComponentView`,
/// cheap for these small builder structs, and correct because
/// `ComponentView::rebuild` never actually reads its `prev` argument (it
/// always re-runs `Component::build` against the retained `State` — see
/// `frust_core::component`'s own doc comment).
macro_rules! impl_native_view {
    ($ty:ty) => {
        impl<Outer: 'static> View<Outer> for $ty {
            type Element = ComponentWidget<$ty>;

            fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
                // Fully-qualified: `ComponentView<$ty>: View<Outer>` for every
                // `Outer`, so a plain `.build(ctx)` call leaves `Outer`
                // unconstrained — pin it to the impl we're writing.
                <frust_core::ComponentView<$ty> as View<Outer>>::build(
                    &component(self.clone()),
                    ctx,
                )
            }

            fn rebuild(
                &self,
                prev: &Self,
                element: &mut Self::Element,
                ctx: &mut BuildCtx<'_>,
            ) -> ChangeFlags {
                <frust_core::ComponentView<$ty> as View<Outer>>::rebuild(
                    &component(self.clone()),
                    &component(prev.clone()),
                    element,
                    ctx,
                )
            }

            fn teardown(&self, element: &mut Self::Element, ctx: &mut BuildCtx<'_>) {
                <frust_core::ComponentView<$ty> as View<Outer>>::teardown(
                    &component(self.clone()),
                    element,
                    ctx,
                );
            }
        }
    };
}

impl_native_view!(NativeButtonView);
impl_native_view!(NativeLabelView);
impl_native_view!(NativeSwitchView);
impl_native_view!(NativeSliderView);
impl_native_view!(NativeProgressView);
impl_native_view!(NativeImageView);

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{BoxConstraints, LayoutCtx, PaintCtx, PaintScene, Widget};
    use kurbo::{Point, Size};
    use peniko::Color;

    /// A minimal `PaintScene` — only `fill_rect`/`draw_text` have no default
    /// (see `frust_core::widget::PaintScene`'s trait definition); every other
    /// method a placeholder's `alert`/`SizedBox` might call is defaulted.
    #[derive(Default)]
    struct NullScene;

    impl PaintScene for NullScene {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
    }

    fn build_any<State: 'static>(view: AnyView<State>) -> Box<dyn Widget> {
        let mut counter = 0u64;
        view.build(&mut BuildCtx::new(&mut counter))
    }

    // --- params_json snapshots, one per control -----------------------------

    #[test]
    fn button_params_snapshot() {
        let view = native_button("Save")
            .enabled(false)
            .content_description("Save the note");
        assert_eq!(
            view.params_for(7),
            "{\"__frustControl\":\"button\",\"__frustSlot\":7,\"text\":\"Save\",\"enabled\":false,\
             \"contentDescription\":\"Save the note\"}"
        );
    }

    #[test]
    fn label_params_snapshot() {
        let view = native_label("42 fps");
        assert_eq!(
            view.params_for(3),
            "{\"__frustControl\":\"label\",\"__frustSlot\":3,\"text\":\"42 fps\",\"enabled\":true}"
        );
    }

    #[test]
    fn switch_params_snapshot() {
        let view = native_switch(true).content_description("wifi");
        assert_eq!(
            view.params_for(11),
            "{\"__frustControl\":\"switch\",\"__frustSlot\":11,\"checked\":true,\"enabled\":true,\
             \"contentDescription\":\"wifi\"}"
        );
    }

    #[test]
    fn slider_params_snapshot() {
        let view = native_slider(25, 0, 50);
        assert_eq!(
            view.params_for(5),
            "{\"__frustControl\":\"slider\",\"__frustSlot\":5,\"value\":25,\"min\":0,\"max\":50,\
             \"enabled\":true}"
        );
    }

    #[test]
    fn progress_params_snapshot() {
        let view = native_progress(30, 10, 110).indeterminate(false);
        assert_eq!(
            view.params_for(2),
            "{\"__frustControl\":\"progress\",\"__frustSlot\":2,\"value\":30,\"min\":10,\
             \"max\":110,\"indeterminate\":false}"
        );
    }

    #[test]
    fn image_params_snapshot() {
        let view =
            native_image(Arc::from(vec![1u8, 2, 3].into_boxed_slice())).fit(NativeImageFit::Cover);
        assert_eq!(
            view.params_for(900, 42),
            "{\"__frustControl\":\"image\",\"__frustSlot\":900,\"imageRev\":42,\"fit\":\"cover\"}"
        );
    }

    // --- the translucency-refused fallback ----------------------------------

    #[test]
    fn a_refused_slot_publishes_no_platform_view_frame() {
        for (name, view) in [
            (
                "Button",
                native_button("Save").build_with_mode(1, ResolvedSurfaceMode::RefusedTranslucent),
            ),
            (
                "Switch",
                native_switch(true).build_with_mode(2, ResolvedSurfaceMode::RefusedTranslucent),
            ),
        ] {
            let mut element = build_any(view);
            let mut scene = NullScene;
            let mut pctx = PaintCtx::new(Point::ZERO, Size::new(120.0, 44.0));
            element.paint(&mut pctx, &mut scene);
            assert!(
                pctx.take_platform_views().is_empty(),
                "{name}: a refused slot must render no native platform_view frame"
            );
        }
    }

    #[test]
    fn an_unrefused_slot_still_builds_the_native_platform_view() {
        for mode in [
            ResolvedSurfaceMode::Unknown,
            ResolvedSurfaceMode::Opaque,
            ResolvedSurfaceMode::Translucent,
        ] {
            let btn = native_button("Save").size(120.0, 44.0);
            let expected_params = btn.params_for(9);
            let view = btn.build_with_mode(9, mode);
            let mut element = build_any(view);
            let mut lctx = LayoutCtx::new();
            element.layout(&mut lctx, &BoxConstraints::tight(Size::new(120.0, 44.0)));
            let mut scene = NullScene;
            let mut pctx = PaintCtx::new(Point::ZERO, Size::new(120.0, 44.0));
            element.paint(&mut pctx, &mut scene);
            let frames = pctx.take_platform_views();
            assert_eq!(frames.len(), 1, "{mode:?}");
            assert_eq!(frames[0].view_type, VIEW_TYPE);
            assert_eq!(frames[0].params_json, expected_params);
            assert!(frames[0].interactive, "a Button slot is interactive");
        }
    }

    #[test]
    fn a_display_only_control_never_sets_interactive() {
        let view = native_label("hi").build_with_mode(4, ResolvedSurfaceMode::Opaque);
        let mut element = build_any(view);
        let mut scene = NullScene;
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(100.0, 30.0));
        element.paint(&mut pctx, &mut scene);
        let frames = pctx.take_platform_views();
        assert_eq!(frames.len(), 1);
        assert!(!frames[0].interactive, "Label never forwards native input");
    }
}
