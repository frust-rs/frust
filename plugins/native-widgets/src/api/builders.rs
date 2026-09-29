//! The ten app-facing builders: `native_button`/
//! `native_label`/`native_switch`/`native_slider`/`native_progress`/
//! `native_image`/`native_spinner`/`native_date_picker`/`native_segmented`/
//! `native_stepper`, each
//! composing exactly one [`platform_view`] slot behind this crate's one
//! factory per platform (the "N controls = N slots" envelope) —
//! `native_segmented`/`native_stepper` only on iOS and macOS; elsewhere each
//! renders its own refusal banner at compile time ([`SEGMENTED_ARM`]/
//! [`STEPPER_ARM`]).
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
//! `pub(crate)` by design) — a builder only ever calls the
//! runtime's already-`pub(crate)` `with_runtime`/`set_callback` seam, same
//! crate.
//!
//! # The translucency-refused fallback
//!
//! Every builder consults `frust::resolved_surface_mode()` before composing
//! its native slot: on [`ResolvedSurfaceMode::RefusedTranslucent`], it
//! renders [`placeholder`] instead — a frust-drawn box that both *paints*
//! the refusal as visible warning prose and publishes the same wording to a
//! screen reader — rather than an invisible, untappable native slot
//! (`docs/ARCHITECTURE.md`'s Platform-view flow: "App Rust now is told ...
//! so a plugin can fall back deliberately instead of a dead slot"). The
//! refusal is logged once, crate-wide, not once per control per frame.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Once};

use frust::authoring::text::{FontWeight, LineHeight, TextContext, TextLayout, TextStyle};
use frust::{
    Color, PlatformViewView, ResolvedSurfaceMode, SizedBox, Theme, on_cleanup, platform_view,
    resolved_surface_mode, use_context,
};
use frust_core::accesskit::Role;
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, Component, ComponentWidget, LayoutCtx,
    PaintCtx, PaintScene, SemanticsCtx, View, Widget, any, component,
};
use kurbo::{Size, Vec2};

use crate::controls::date_picker::{self, CivilDate};
use crate::controls::{
    BACKGROUND_COLOR, CHECKED, CONTENT_DESCRIPTION, CORNER_RADIUS_DP, DARK, ENABLED, FIT,
    INDETERMINATE, MAX, MIN, PROGRESS_TINT, STEP, TEXT, TEXT_COLOR, TEXT_SIZE_SP, THUMB_TINT, TINT,
    TRACK_TINT, TYPEFACE, VALUE, WRAPS,
};
use crate::controls::{
    button, image, label, progress, segmented, slider, spinner, stepper, switch,
};
use crate::registry::SlotId;
use crate::runtime::{escape, with_identity, with_runtime};

use super::signals::{on_click, on_date, on_selected, on_toggled, on_value_changed};
use super::theme::{self, ResolvedTheme};

/// The one factory class every control resolves through, per platform.
///
/// - **Android**: `dev.frust.nativewidgets.FrustNativeControlFactory`, the
///   class in this plugin's own `com.android.library` module
///   (`plugins/native-widgets/platform/android`), which a consuming app wires
///   in via `Contribution::GradleModule` — never a copied file. The
///   `dev.frust.` prefix is required by `FrustViewHost`'s factory resolution
///   and the `nativewidgets` subpackage by the packaging rule; both halves
///   are baked into the JNI export symbol names (`crate::android`'s *Package*
///   note), so this string is fixed once shipped.
/// - **iOS**: the bare Objective-C runtime name `FrustNativeControlFactory`,
///   which `FrustViewHost.resolveFactory` feeds to `NSClassFromString`
///   (`docs/CODE_STANDARDS.md`'s Naming Conventions: iOS has no package
///   prefix). That class is a Rust `define_class!` class — no
///   Swift — and **this string must stay byte-identical to
///   `crate::apple::factory::FACTORY_CLASS_NAME`**, which is the name that
///   class registers under. A mismatch is silent: the lookup returns nil, the
///   host takes its unresolvable-factory branch, and every native control on
///   iOS renders nothing.
/// - **macOS**: the desktop registry key `crate::appkit::factory::VIEW_TYPE`
///   (`"dev.frust.nativewidgets.FrustNativeControlFactory"`, the Android
///   spelling), named here rather than repeated: the desktop Mode-A host
///   resolves a slot's factory by looking this exact string up in
///   `frust_plugin::desktop`'s registry, where the AppKit arm registered it.
///   A mismatch would be just as silent as on iOS — no factory, an empty slot.
/// - **Anywhere else** (Linux/Windows/web): no factory exists; the Android
///   spelling stands in so the constant is always defined.
///
/// `pub(super)` rather than private: the generic mounting builder
/// ([`crate::api::mount`]) composes the same one factory these eight do —
/// a public component is served by the same runtime, so it must resolve
/// through the same class.
#[cfg(target_os = "android")]
pub(super) const VIEW_TYPE: &str = "dev.frust.nativewidgets.FrustNativeControlFactory";
#[cfg(target_os = "ios")]
pub(super) const VIEW_TYPE: &str = "FrustNativeControlFactory";
#[cfg(target_os = "macos")]
pub(super) const VIEW_TYPE: &str = crate::appkit::factory::VIEW_TYPE;
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
pub(super) const VIEW_TYPE: &str = "dev.frust.nativewidgets.FrustNativeControlFactory";

/// This plugin's own per-widget-instance identity counter (module doc: "this
/// slot id is this plugin's OWN bookkeeping key"). Deliberately independent
/// of `frust_core::widget::next_slot_id()` — nothing on the platform side
/// ever compares the two, so a private counter avoids reaching into
/// `frust-core`'s widget-tree internals for a value nothing downstream reads
/// as a differ id.
///
/// `pub(super)` — the generic mounting builder
/// ([`crate::api::mount`]) draws its slot ids from the same counter, so a
/// public component and a built-in control can never collide on one.
pub(super) fn next_local_slot() -> SlotId {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Log the translucency-refused fallback exactly once, crate-wide — never
/// once per control, never once per frame.
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
/// [`ResolvedSurfaceMode::RefusedTranslucent`]: a sized box holding a
/// [`RefusalBanner`] — a warning fill and border, the label plus explanation
/// painted as visible prose, and the same wording published as a
/// `Role::Alert` semantics node — instead of an invisible, untappable native
/// slot. Both halves matter: a sighted user sees the warning, a screen-reader
/// user hears it.
///
/// # Why the banner is crate-local
///
/// No design-system catalog is reachable from here: this is a platform
/// plugin, and its dependency charter is `frust`/`frust-core` plus FFI
/// (`docs/CODE_STANDARDS.md`'s Plugin Conventions), while every design system
/// — including whichever one the consuming app installed — ships as its own
/// plugin crate beside this one. So the banner is a thin crate-local
/// `View`/`Widget` pair, the same shape [`ClipToSlot`] below already uses,
/// shaping its own runs through [`BannerText`] and painting from the ambient
/// [`Theme`]'s own error roles with an unthemed fallback
/// (`docs/WIDGETS_CODE_STANDARDS.md`'s token-resolution rule) rather than
/// borrowing a catalog's alert widget.
///
/// # The clip-to-slot fix for oversized placeholder prose
///
/// The explanatory prose is longer than most slot boxes allow (e.g.
/// `native_switch`'s 70x40, `native_progress`'s 260x24), and `SizedBox` only
/// tightens the reported [`Size`] the layout pass sees — it does not stop a
/// child from drawing content sized off its own unclamped natural extent. The
/// banner budgets its runs against the slot width, but a slot too short for
/// even one wrapped line still overflows vertically. Clipping the whole
/// banner's paint to the slot rect (below) is therefore load-bearing: it lets
/// the full explanation stay painted, in the semantics tree, and in the
/// one-time [`warn_refusal_once`] log while guaranteeing the placeholder never
/// paints outside its own slot, at any slot size the builders allow.
///
/// `pub(super)`: the generic mounting builder
/// ([`crate::api::mount`]) degrades through this same placeholder — including
/// its clip wrapper — rather than re-deriving the refusal path.
pub(super) fn placeholder<State: 'static>(
    size: Option<(f64, f64)>,
    control: &str,
) -> AnyView<State> {
    warn_refusal_once();
    banner_placeholder(
        size,
        format!("Native {control} unavailable"),
        "the host declared a translucent surface but the platform refused it — rendering a \
         frust placeholder instead of an invisible native slot."
            .to_string(),
    )
}

/// The sized, slot-clipped [`RefusalBanner`] itself, with caller-chosen
/// wording — [`placeholder`]'s body, shared with the compile-time
/// no-platform-arm fallback ([`NativeSegmentedView`] on Android, see
/// [`SEGMENTED_ARM`]), which refuses for a different reason and so says a
/// different thing. Logging is the caller's: each refusal reason logs once
/// under its own `Once`.
fn banner_placeholder<State: 'static>(
    size: Option<(f64, f64)>,
    label: String,
    description: String,
) -> AnyView<State> {
    let banner: AnyView<State> = any(RefusalBanner { label, description });
    let sized = SizedBox(size.map(|(w, _)| w), size.map(|(_, h)| h)).child(banner);
    any(ClipToSlot { child: any(sized) })
}

/// Unthemed fallback fill for [`RefusalBanner`] — a muted warning amber, used
/// only when no [`Theme`] is threaded into the paint pass (a bare-core test).
/// A themed paint reads `error_container`/`outline` instead.
const REFUSAL_FILL: Color = Color::from_rgb8(0xFF, 0xDD, 0xB0);

/// Unthemed fallback border for [`RefusalBanner`]. See [`REFUSAL_FILL`].
const REFUSAL_BORDER: Color = Color::from_rgb8(0x8A, 0x53, 0x00);

/// Unthemed fallback prose colour for [`RefusalBanner`] — the "on" role for
/// [`REFUSAL_FILL`], dark enough to read over that amber. A themed paint reads
/// `on_error_container` instead. See [`REFUSAL_FILL`].
const REFUSAL_TEXT: Color = Color::from_rgb8(0x3B, 0x24, 0x00);

/// Inset between the banner's border hairline and its prose, in logical px.
const REFUSAL_PAD: f64 = 4.0;
/// Gap between the label row and the description row, in logical px.
const REFUSAL_ROW_GAP: f64 = 2.0;
/// Label font size, in logical px — deliberately small, because the slots
/// these placeholders stand in for are themselves small (`native_switch`'s
/// 70x40 is the reference case).
const REFUSAL_LABEL_SIZE: f32 = 12.0;
/// Description font size, in logical px. See [`REFUSAL_LABEL_SIZE`].
const REFUSAL_DESC_SIZE: f32 = 11.0;
/// Prose line height, as a multiple of the font size.
const REFUSAL_LINE_HEIGHT: f32 = 1.3;

/// A minimal retained text run: shape once per (content, style, width),
/// measure during `layout`, emit glyph runs during `paint`.
///
/// Mirrors `plugins/glyph/src/alert.rs`'s `GlyphLabel` — that module's own
/// docs sanction duplicating this small helper per crate rather than sharing
/// one, and a platform plugin could not share it anyway: it may not depend on
/// a design-system plugin at all (`docs/PLUGINS_CODE_STANDARDS.md`'s charter
/// line).
///
/// The shaping vocabulary comes through `frust::authoring::text` rather than a
/// direct `frust-text` dependency — the one place this file reaches through
/// the facade instead of naming a framework crate (`frust-core` is named
/// directly, per this crate's `Cargo.toml`). `frust-text` is a **dev**-only
/// dependency here, and the refusal placeholder is not worth promoting it to a
/// production one: the facade re-export costs nothing and stays behind the
/// same default-on `frust-api` feature gate as `frust` itself, so the
/// `--no-default-features` charter line is untouched.
struct BannerText {
    content: String,
    layout: Option<TextLayout>,
    laid_out_style: Option<TextStyle>,
    laid_out_max_width: Option<f32>,
}

impl BannerText {
    fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            laid_out_style: None,
            laid_out_max_width: None,
        }
    }

    fn set_content(&mut self, content: impl Into<String>) {
        let content = content.into();
        if self.content != content {
            self.content = content;
            self.layout = None;
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle, max_width: Option<f32>) -> Size {
        if let Some(cached) = &self.layout
            && self.laid_out_max_width == max_width
            && self.laid_out_style.as_ref() == Some(style)
        {
            return cached.size();
        }
        let laid = ctx
            .text_context::<TextContext>()
            .layout(&self.content, style, max_width);
        let size = laid.size();
        self.layout = Some(laid);
        self.laid_out_style = Some(style.clone());
        self.laid_out_max_width = max_width;
        size
    }

    /// Emit this run's glyphs at `origin`. A no-op before the first
    /// [`BannerText::layout`] — a paint without a preceding layout pass draws
    /// no text rather than panicking for want of a text context.
    fn paint(&self, origin: kurbo::Point, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for run in layout.to_scene_runs(origin) {
                scene.draw_glyph_run(run);
            }
        }
    }
}

fn refusal_label_style(color: Color) -> TextStyle {
    TextStyle {
        weight: FontWeight::SEMI_BOLD,
        line_height: LineHeight::FontSizeRelative(REFUSAL_LINE_HEIGHT),
        ..TextStyle::new(REFUSAL_LABEL_SIZE, color)
    }
}

fn refusal_description_style(color: Color) -> TextStyle {
    TextStyle {
        weight: FontWeight::REGULAR,
        line_height: LineHeight::FontSizeRelative(REFUSAL_LINE_HEIGHT),
        ..TextStyle::new(REFUSAL_DESC_SIZE, color)
    }
}

/// Explicit builder value > theme > fallback constant
/// (`docs/WIDGETS_CODE_STANDARDS.md`) — there is no explicit override on the
/// banner, so: the theme's `on_error_container` (the "on" role for the
/// `error_container` fill the banner paints under it), else [`REFUSAL_TEXT`].
fn refusal_text_color(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().on_error_container,
        None => REFUSAL_TEXT,
    }
}

/// The refusal placeholder's own visual + accessible body — see
/// [`placeholder`] for why it is crate-local rather than a catalog widget.
///
/// Paints a filled, outlined box across whatever the slot gave it (so a
/// refused control still reads as a deliberate "something is wrong here"
/// marker rather than a hole) carrying `label` and `description` as visible
/// prose, and publishes exactly one `Role::Alert` semantics node with the same
/// two strings — the accessible half of the same refusal contract.
struct RefusalBanner {
    label: String,
    description: String,
}

/// The retained widget for [`RefusalBanner`].
///
/// `label`/`description` are kept as plain strings alongside their
/// [`BannerText`] runs because [`RefusalBannerWidget::semantics`] reports the
/// wording whether or not a layout pass ever shaped it (the same split
/// `AlertWidget` uses in `plugins/glyph/src/alert.rs`).
struct RefusalBannerWidget {
    label: String,
    description: String,
    label_run: BannerText,
    description_run: BannerText,
    label_size: Size,
}

impl<State: 'static> View<State> for RefusalBanner {
    type Element = RefusalBannerWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> Self::Element {
        RefusalBannerWidget {
            label: self.label.clone(),
            description: self.description.clone(),
            label_run: BannerText::new(self.label.clone()),
            description_run: BannerText::new(self.description.clone()),
            label_size: Size::ZERO,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if prev.label == self.label && prev.description == self.description {
            return ChangeFlags::NONE;
        }
        element.label = self.label.clone();
        element.description = self.description.clone();
        element.label_run.set_content(self.label.clone());
        element
            .description_run
            .set_content(self.description.clone());
        // New wording re-shapes, so this is a layout change, not paint-only.
        ChangeFlags::LAYOUT | ChangeFlags::PAINT
    }

    fn teardown(&self, _element: &mut Self::Element, _ctx: &mut BuildCtx<'_>) {}
}

impl Widget for RefusalBannerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Fill whatever the enclosing `SizedBox` tightened to; with no
        // explicit slot size the builders leave it filling the parent.
        let size = bc.max();

        // Colour is baked into the shaped runs, so it is resolved here rather
        // than at paint time — the same reason `AlertWidget::layout` reads the
        // theme during layout.
        let color = refusal_text_color(Theme::from_layout_ctx(ctx));
        // Budget both runs against the slot, minus the prose inset on each
        // side, so the wording wraps inside the slot instead of running off
        // its natural single-line extent. A non-finite max (an unconstrained
        // parent, i.e. no explicit `.size(w, h)`) means "no wrap budget"
        // rather than a wrap at infinity.
        let max_width = size
            .width
            .is_finite()
            .then(|| (size.width - REFUSAL_PAD * 2.0).max(0.0) as f32);

        self.label_size = self
            .label_run
            .layout(ctx, &refusal_label_style(color), max_width);
        self.description_run
            .layout(ctx, &refusal_description_style(color), max_width);

        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Explicit builder value > theme > fallback constant
        // (`docs/WIDGETS_CODE_STANDARDS.md`) — there is no explicit override
        // here, so: theme, else the two `REFUSAL_*` constants.
        let (fill, border) = match Theme::from_paint_ctx(ctx) {
            Some(theme) => {
                let s = theme.scheme();
                (s.error_container, s.error)
            }
            None => (REFUSAL_FILL, REFUSAL_BORDER),
        };
        let (origin, size) = (ctx.origin(), ctx.size());
        scene.fill_rect(origin, size, fill);
        // A 1px inset hairline, drawn as four edge fills rather than a
        // stroked path so it needs no `BezPath`/`Brush` vocabulary here.
        const EDGE: f64 = 1.0;
        let edge = EDGE.min(size.width / 2.0).min(size.height / 2.0);
        if edge > 0.0 {
            scene.fill_rect(origin, Size::new(size.width, edge), border);
            scene.fill_rect(
                kurbo::Point::new(origin.x, origin.y + size.height - edge),
                Size::new(size.width, edge),
                border,
            );
            scene.fill_rect(origin, Size::new(edge, size.height), border);
            scene.fill_rect(
                kurbo::Point::new(origin.x + size.width - edge, origin.y),
                Size::new(edge, size.height),
                border,
            );
        }
        // The prose paints last, over the fill and the border hairline. What
        // keeps a run that outgrows a short slot from escaping it is
        // [`ClipToSlot`], which wraps the whole banner (see [`placeholder`]) —
        // deliberately, so the wording stays complete rather than truncated.
        self.label_run
            .paint(origin + Vec2::new(REFUSAL_PAD, REFUSAL_PAD), scene);
        let description_y = REFUSAL_PAD + self.label_size.height + REFUSAL_ROW_GAP;
        self.description_run
            .paint(origin + Vec2::new(REFUSAL_PAD, description_y), scene);
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = format!("{}. {}", self.label, self.description);
        ctx.push_node(Role::Alert, |node| node.set_label(label));
    }
}

/// Clips its child's paint to this widget's own laid-out bounds — see
/// [`placeholder`]'s "The clip-to-slot fix for oversized placeholder prose"
/// for why this exists instead of truncating the prose itself. A thin,
/// crate-local wrapper (not a `frust-widgets` container) built directly
/// against [`AnyView`]/[`Widget`] rather than `frust-widgets`' crate-private
/// `ChildPod` plumbing, which this crate has no access to
/// (`docs/CODE_STANDARDS.md`'s Plugin Conventions).
struct ClipToSlot<State: 'static> {
    child: AnyView<State>,
}

/// The retained widget for [`ClipToSlot`].
struct ClipToSlotWidget {
    child: Box<dyn Widget>,
}

impl<State: 'static> View<State> for ClipToSlot<State> {
    type Element = ClipToSlotWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
        ClipToSlotWidget {
            child: self.child.build(ctx),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        self.child.rebuild(&prev.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut Self::Element, ctx: &mut BuildCtx<'_>) {
        self.child.teardown(&mut element.child, ctx);
    }
}

impl Widget for ClipToSlotWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.child.layout(ctx, bc)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The clip rect is THIS widget's own laid-out origin/size — exactly
        // the slot rect the placeholder was given, never the child's
        // unclamped natural content size.
        scene.push_clip(ctx.origin(), ctx.size());
        self.child.paint(ctx, scene);
        scene.pop_clip();
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Transparent wrapper: forward unchanged so the full title+body
        // detail still reaches a screen reader regardless of what got
        // visually clipped (`docs/CODE_STANDARDS.md`'s Semantics
        // Conventions — a container must forward, never drop, a child's
        // subtree).
        self.child.semantics(ctx);
    }
}

/// Apply an explicit `.size(w, h)` if the caller provided one, else leave the
/// slot at [`PlatformViewView`]'s own default (fill the parent) — v1 has no
/// measure step either way, so an explicit `.size(w,h)` is required and an
/// omitted call degrades to filling the parent rather than a made-up
/// constant. `pub(super)`, for
/// [`crate::api::mount`]'s generic builder.
pub(super) fn resolve_size(size: Option<(f64, f64)>, view: PlatformViewView) -> PlatformViewView {
    match size {
        Some((w, h)) => view.size(w, h),
        None => view,
    }
}

/// The active theme's resolved tokens (theme ladder L2), or `None`
/// when no theme has been threaded — `use_context::<Theme>()`'s own
/// documented `None` cases (a bare-core test, a build running outside any
/// reactive `Owner`; `reactive_graph::owner::use_context`'s own doc: "Panics
/// if no value is found" only applies to its `expect_context` sibling, never
/// this one). Every builder's `Component::build` calls this once per
/// rebuild — the mechanism `crates/frust/tests/
/// theme_reactivity_spike.rs` proves is what makes that rebuild re-run, and
/// therefore re-resolve, on `set_app_theme` — and threads the result into
/// `build_with_mode` explicitly, the same "thread it as a parameter so a
/// test can force it" shape `resolved_surface_mode()` already uses above.
fn ambient_theme_tokens() -> Option<ResolvedTheme> {
    use_context::<Theme>().as_ref().map(theme::resolve)
}

/// A tiny flat-JSON body writer — this crate hand-rolls JSON at the wire
/// boundary rather than pulling in `serde` (`docs/CODE_STANDARDS.md`'s
/// Language Idioms; `crate::runtime::Params`/`with_identity` are the
/// reader/identity-encoder halves this writes the *body* half for). Only the
/// handful of primitive field shapes the eight controls need.
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
    /// [`Component::build`] so a test can snapshot it directly. `tokens`
    /// (theme ladder L2) folds the active theme's background/text
    /// colour, corner radius, and text size in, threaded explicitly like
    /// `mode` below so a test can pin an exact resolved value without a live
    /// reactive context.
    fn params_for(&self, slot: SlotId, tokens: Option<ResolvedTheme>) -> String {
        let mut body = ParamsBody::new();
        body.push_str(TEXT, &self.text);
        body.push_raw(ENABLED, self.enabled);
        body.push_opt_str(CONTENT_DESCRIPTION, self.content_description.as_deref());
        body.push_raw(DARK, tokens.is_some_and(|t| t.dark));
        if let Some(t) = tokens {
            body.push_raw(TEXT_COLOR, t.on_accent_fill);
            body.push_raw(BACKGROUND_COLOR, t.accent_fill);
            body.push_raw(CORNER_RADIUS_DP, t.corner_radius_dp);
            body.push_raw(TEXT_SIZE_SP, t.button_text_size_sp);
            body.push_str(TYPEFACE, t.button_typeface.wire());
        }
        with_identity(button::KIND, slot, &body.finish())
    }

    /// [`Component::build`]'s real body, with `mode`/`tokens` threaded
    /// explicitly so a test can force the
    /// [`ResolvedSurfaceMode::RefusedTranslucent`] branch or an exact theme
    /// resolution without touching the process-global resolved-mode slot
    /// (whose writer is pinned to the two shells' own FFI glue,
    /// `crates/frust/tests/surface_mode_conformance.rs`) or a live reactive
    /// context.
    fn build_with_mode(
        &self,
        slot: SlotId,
        mode: ResolvedSurfaceMode,
        tokens: Option<ResolvedTheme>,
    ) -> AnyView<SlotId> {
        if mode.translucency_refused() {
            return placeholder(self.size, "Button");
        }
        let params = self.params_for(slot, tokens);
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
        let slot = next_local_slot();
        // Ties this slot's `NativeRuntime::pending_callbacks` entry to the
        // Component's own lifetime, not to the native create/dispose
        // lifecycle (the same remedy `NativeImageView::init` applies to
        // the identical leak shape one table over — see
        // `crate::runtime`'s doc on `pending_callbacks` and
        // `NativeRuntime::forget_pending_callback`). `init` runs exactly
        // once, under this component's own `Owner`, so `on_cleanup` fires
        // exactly once when that owner disposes — regardless of whether a
        // culled-then-republished cycle already parked a second pending
        // entry `dispose_slot` never sees.
        on_cleanup(move || {
            with_runtime(|rt| rt.forget_pending_callback(slot));
        });
        slot
    }

    fn build(&self, state: &mut SlotId) -> AnyView<SlotId> {
        self.build_with_mode(*state, resolved_surface_mode(), ambient_theme_tokens())
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

    /// `tokens` (theme ladder L2, including a later followup) folds the
    /// active theme's background/body-text colour and size in — see
    /// [`NativeButtonView::params_for`]'s doc for why it's threaded
    /// explicitly, and [`crate::api::theme`]'s module doc's *Explicit
    /// backgrounds* section for why an EXPLICIT background is folded here
    /// too — it used to be entirely absent, pinning `Label` to
    /// whichever brightness it was created under.
    fn params_for(&self, slot: SlotId, tokens: Option<ResolvedTheme>) -> String {
        let mut body = ParamsBody::new();
        body.push_str(TEXT, &self.text);
        body.push_raw(ENABLED, self.enabled);
        body.push_opt_str(CONTENT_DESCRIPTION, self.content_description.as_deref());
        body.push_raw(DARK, tokens.is_some_and(|t| t.dark));
        if let Some(t) = tokens {
            body.push_raw(BACKGROUND_COLOR, t.surface_bg);
            body.push_raw(TEXT_COLOR, t.body_text);
            body.push_raw(TEXT_SIZE_SP, t.body_text_size_sp);
            body.push_str(TYPEFACE, t.body_typeface.wire());
        }
        with_identity(label::KIND, slot, &body.finish())
    }

    fn build_with_mode(
        &self,
        slot: SlotId,
        mode: ResolvedSurfaceMode,
        tokens: Option<ResolvedTheme>,
    ) -> AnyView<SlotId> {
        if mode.translucency_refused() {
            return placeholder(self.size, "Label");
        }
        let params = self.params_for(slot, tokens);
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

    // No `on_cleanup` here: `Label` is display-only and never calls
    // `set_callback`, so its slot never has a `pending_callbacks` entry to
    // reap. `NativeButtonView::init`'s doc explains the cleanup this
    // Component deliberately omits.
    fn init(&self) -> SlotId {
        next_local_slot()
    }

    fn build(&self, state: &mut SlotId) -> AnyView<SlotId> {
        self.build_with_mode(*state, resolved_surface_mode(), ambient_theme_tokens())
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

    /// `tokens` (theme ladder L2) folds the active theme's
    /// thumb/track tints in — see [`NativeButtonView::params_for`]'s doc for
    /// why it's threaded explicitly. Deliberately **no** background fold
    /// (reversing an earlier followup that added one): see [`crate::api::theme`]'s
    /// module doc's *Explicit backgrounds* section for why `Switch` is
    /// excluded — a flat `View.setBackgroundColor` here would replace
    /// `?attr/selectableItemBackgroundBorderless`'s touch ripple, and the
    /// thumb/track tints below already carry the theme without it.
    fn params_for(&self, slot: SlotId, tokens: Option<ResolvedTheme>) -> String {
        let mut body = ParamsBody::new();
        body.push_raw(CHECKED, self.checked);
        body.push_raw(ENABLED, self.enabled);
        body.push_opt_str(CONTENT_DESCRIPTION, self.content_description.as_deref());
        body.push_raw(DARK, tokens.is_some_and(|t| t.dark));
        if let Some(t) = tokens {
            body.push_raw(THUMB_TINT, t.accent_ink);
            body.push_raw(TRACK_TINT, t.accent_fill);
            // `Switch` never sets on/off text through this plugin today, but
            // it's a `TextView` subclass under the hood (`android.widget.Switch
            // extends CompoundButton extends Button extends TextView`) — see
            // `crate::api::theme`'s module doc on why this shares `Label`'s
            // typeface rather than going unset.
            body.push_str(TYPEFACE, t.body_typeface.wire());
        }
        with_identity(switch::KIND, slot, &body.finish())
    }

    fn build_with_mode(
        &self,
        slot: SlotId,
        mode: ResolvedSurfaceMode,
        tokens: Option<ResolvedTheme>,
    ) -> AnyView<SlotId> {
        if mode.translucency_refused() {
            return placeholder(self.size, "Switch");
        }
        let params = self.params_for(slot, tokens);
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
        let slot = next_local_slot();
        // See `NativeButtonView::init`'s doc — `Switch` registers a
        // callback via `Self::on_toggle`, so it needs the same
        // `pending_callbacks` reaper.
        on_cleanup(move || {
            with_runtime(|rt| rt.forget_pending_callback(slot));
        });
        slot
    }

    fn build(&self, state: &mut SlotId) -> AnyView<SlotId> {
        self.build_with_mode(*state, resolved_surface_mode(), ambient_theme_tokens())
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

    /// `tokens` (theme ladder L2) folds the active theme's
    /// progress/thumb tints in — see [`NativeButtonView::params_for`]'s doc
    /// for why it's threaded explicitly. Deliberately **no** background fold
    /// (reversing an earlier followup that added one): see [`crate::api::theme`]'s
    /// module doc's *Explicit backgrounds* section for why `Slider` is
    /// excluded — a flat `View.setBackgroundColor` here would replace
    /// `AbsSeekBar`'s `?attr/selectableItemBackgroundBorderless` touch
    /// ripple, and the progress/thumb tints below already carry the theme
    /// without it.
    fn params_for(&self, slot: SlotId, tokens: Option<ResolvedTheme>) -> String {
        let mut body = ParamsBody::new();
        body.push_raw(VALUE, self.value);
        body.push_raw(MIN, self.min);
        body.push_raw(MAX, self.max);
        body.push_raw(ENABLED, self.enabled);
        body.push_opt_str(CONTENT_DESCRIPTION, self.content_description.as_deref());
        body.push_raw(DARK, tokens.is_some_and(|t| t.dark));
        if let Some(t) = tokens {
            body.push_raw(PROGRESS_TINT, t.accent_fill);
            body.push_raw(THUMB_TINT, t.accent_ink);
        }
        with_identity(slider::KIND, slot, &body.finish())
    }

    fn build_with_mode(
        &self,
        slot: SlotId,
        mode: ResolvedSurfaceMode,
        tokens: Option<ResolvedTheme>,
    ) -> AnyView<SlotId> {
        if mode.translucency_refused() {
            return placeholder(self.size, "Slider");
        }
        let params = self.params_for(slot, tokens);
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
        let slot = next_local_slot();
        // See `NativeButtonView::init`'s doc — `Slider` registers a
        // callback via `Self::on_change`, so it needs the same
        // `pending_callbacks` reaper.
        on_cleanup(move || {
            with_runtime(|rt| rt.forget_pending_callback(slot));
        });
        slot
    }

    fn build(&self, state: &mut SlotId) -> AnyView<SlotId> {
        self.build_with_mode(*state, resolved_surface_mode(), ambient_theme_tokens())
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

    /// `tokens` (theme ladder L2, including a later followup) folds the
    /// active theme's background/progress tint in — see
    /// [`NativeButtonView::params_for`]'s doc for why it's threaded
    /// explicitly.
    fn params_for(&self, slot: SlotId, tokens: Option<ResolvedTheme>) -> String {
        let mut body = ParamsBody::new();
        body.push_raw(VALUE, self.value);
        body.push_raw(MIN, self.min);
        body.push_raw(MAX, self.max);
        body.push_raw(INDETERMINATE, self.indeterminate);
        body.push_opt_str(CONTENT_DESCRIPTION, self.content_description.as_deref());
        body.push_raw(DARK, tokens.is_some_and(|t| t.dark));
        if let Some(t) = tokens {
            body.push_raw(BACKGROUND_COLOR, t.surface_bg);
            body.push_raw(PROGRESS_TINT, t.accent_fill);
        }
        with_identity(progress::KIND, slot, &body.finish())
    }

    fn build_with_mode(
        &self,
        slot: SlotId,
        mode: ResolvedSurfaceMode,
        tokens: Option<ResolvedTheme>,
    ) -> AnyView<SlotId> {
        if mode.translucency_refused() {
            return placeholder(self.size, "ProgressBar");
        }
        let params = self.params_for(slot, tokens);
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

    // No `on_cleanup` here: `ProgressBar` is display-only and never
    // calls `set_callback` — see `NativeLabelView::init`'s doc.
    fn init(&self) -> SlotId {
        next_local_slot()
    }

    fn build(&self, state: &mut SlotId) -> AnyView<SlotId> {
        self.build_with_mode(*state, resolved_surface_mode(), ambient_theme_tokens())
    }
}

// ============================================================================
// Spinner
// ============================================================================

/// How large the spinner renders — the public mirror of
/// `crate::controls::spinner::SizeClass`, which stays `pub(crate)`. iOS has
/// no distinct small style (`UIActivityIndicatorView.Style` offers only
/// `.medium`/`.large`), so [`Self::Small`] renders the same as
/// [`Self::Medium`] on that one arm — see `crate::controls::spinner`'s
/// module doc.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NativeSpinnerSize {
    /// The smallest stock size. Degrades to [`Self::Medium`] on iOS.
    Small,
    /// The platform's own default circular spinner size.
    #[default]
    Medium,
    /// The largest stock size.
    Large,
}

impl NativeSpinnerSize {
    /// The wire spelling `crate::controls::spinner::SizeClass::from_wire`
    /// decodes.
    fn wire(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }
}

/// A real indeterminate activity indicator rendered from pure Rust —
/// display-only (no listener, no `.interactive()`). Build one with
/// [`native_spinner`].
#[derive(Clone)]
pub struct NativeSpinnerView {
    animating: bool,
    size_class: NativeSpinnerSize,
    enabled: bool,
    content_description: Option<String>,
    size: Option<(f64, f64)>,
}

/// A native spinner, animating or not — see [`NativeSpinnerView`].
pub fn native_spinner(animating: bool) -> NativeSpinnerView {
    NativeSpinnerView {
        animating,
        size_class: NativeSpinnerSize::default(),
        enabled: true,
        content_description: None,
        size: None,
    }
}

impl NativeSpinnerView {
    /// The spinner's size — see [`NativeSpinnerSize`].
    pub fn size_class(mut self, size_class: NativeSpinnerSize) -> Self {
        self.size_class = size_class;
        self
    }

    /// `View.setEnabled` — Android only; the other two arms have no
    /// `enabled` property on this control at all (`crate::controls::spinner`'s
    /// module doc's *`enabled`* section). Default `true`.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The TalkBack/VoiceOver label.
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

    /// `tokens` (theme ladder L2) folds the active theme's `accent_ink` in
    /// as the spinner's tint — see [`NativeButtonView::params_for`]'s doc
    /// for why it's threaded explicitly.
    fn params_for(&self, slot: SlotId, tokens: Option<ResolvedTheme>) -> String {
        let mut body = ParamsBody::new();
        body.push_raw(spinner::ANIMATING, self.animating);
        body.push_str(spinner::SIZE_CLASS, self.size_class.wire());
        body.push_raw(ENABLED, self.enabled);
        body.push_opt_str(CONTENT_DESCRIPTION, self.content_description.as_deref());
        body.push_raw(DARK, tokens.is_some_and(|t| t.dark));
        if let Some(t) = tokens {
            body.push_raw(TINT, t.accent_ink);
        }
        with_identity(spinner::KIND, slot, &body.finish())
    }

    fn build_with_mode(
        &self,
        slot: SlotId,
        mode: ResolvedSurfaceMode,
        tokens: Option<ResolvedTheme>,
    ) -> AnyView<SlotId> {
        if mode.translucency_refused() {
            return placeholder(self.size, "Spinner");
        }
        let params = self.params_for(slot, tokens);
        let view = platform_view(VIEW_TYPE)
            .params_json(params)
            .semantics_label(
                self.content_description
                    .clone()
                    .unwrap_or_else(|| "spinner".into()),
            );
        any(resolve_size(self.size, view))
    }
}

impl Component for NativeSpinnerView {
    type State = SlotId;

    // No `on_cleanup` here: the spinner is display-only and never calls
    // `set_callback` — see `NativeLabelView::init`'s doc.
    fn init(&self) -> SlotId {
        next_local_slot()
    }

    fn build(&self, state: &mut SlotId) -> AnyView<SlotId> {
        self.build_with_mode(*state, resolved_surface_mode(), ambient_theme_tokens())
    }
}

// ============================================================================
// Date picker
// ============================================================================

/// How [`NativeDatePickerView`] presents itself — the public mirror of
/// `crate::controls::date_picker::DatePickerStyle`, which stays `pub(crate)`.
///
/// | Style | Android | iOS | macOS |
/// |---|---|---|---|
/// | [`Self::Compact`] | spinner mode | `.compact` | text field + stepper, calendar overlay on click |
/// | [`Self::Wheels`] | spinner mode | `.wheels` | text field + stepper (AppKit has no wheels) |
/// | [`Self::Inline`] | calendar mode | `.inline` | clock-and-calendar |
///
/// Android fixes the mode when the picker is created: a later style change
/// is not applied there (logged once) — see `crate::controls::date_picker`'s
/// module doc.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NativeDatePickerStyle {
    /// The smallest footprint the platform offers.
    #[default]
    Compact,
    /// Spinning wheels.
    Wheels,
    /// A full, always-visible calendar.
    Inline,
}

impl NativeDatePickerStyle {
    /// The wire spelling `crate::controls::date_picker::DatePickerStyle`
    /// decodes.
    fn wire(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Wheels => "wheels",
            Self::Inline => "inline",
        }
    }
}

/// A real DATE-mode picker (no time) rendered from pure Rust —
/// `android.widget.DatePicker`, `UIDatePicker`, `NSDatePicker`. Controlled,
/// like [`NativeSwitchView`]: the app owns `date`, and a pick only ever
/// arrives through [`Self::on_change`] as a *requested* [`CivilDate`] the app
/// confirms by feeding it back. Build one with [`native_date_picker`].
#[derive(Clone)]
pub struct NativeDatePickerView {
    date: CivilDate,
    min: Option<CivilDate>,
    max: Option<CivilDate>,
    style: NativeDatePickerStyle,
    enabled: bool,
    content_description: Option<String>,
    size: Option<(f64, f64)>,
    on_change: Option<Arc<dyn Fn(CivilDate) + Send + Sync>>,
}

/// A native date picker showing `date` — see [`NativeDatePickerView`].
pub fn native_date_picker(date: CivilDate) -> NativeDatePickerView {
    NativeDatePickerView {
        date,
        min: None,
        max: None,
        style: NativeDatePickerStyle::default(),
        enabled: true,
        content_description: None,
        size: None,
        on_change: None,
    }
}

impl NativeDatePickerView {
    /// The earliest selectable date — default: the platform's own floor. A
    /// `date` before it is shown (and reported) as `min`; a `max` before it
    /// collapses the range to the single day `min`.
    pub fn min(mut self, min: CivilDate) -> Self {
        self.min = Some(min);
        self
    }

    /// The latest selectable date — default: the platform's own ceiling. A
    /// `date` after it is shown (and reported) as `max`.
    pub fn max(mut self, max: CivilDate) -> Self {
        self.max = Some(max);
        self
    }

    /// The presentation — see [`NativeDatePickerStyle`]. Default
    /// [`NativeDatePickerStyle::Compact`].
    pub fn style(mut self, style: NativeDatePickerStyle) -> Self {
        self.style = style;
        self
    }

    /// `View.setEnabled` / `UIControl.enabled` / `NSControl.enabled` —
    /// default `true`.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The TalkBack/VoiceOver label.
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

    /// Fires with the **requested** date when the user picks one. Feed it
    /// back as the builder's `date` to accept it; keep the old one to refuse
    /// it (the picker snaps back on the next differing params).
    pub fn on_change(mut self, handler: impl Fn(CivilDate) + Send + Sync + 'static) -> Self {
        self.on_change = Some(Arc::new(handler));
        self
    }

    /// `tokens` (theme ladder L2) folds the active theme's `accent_ink` in as
    /// the tint and `body_text` as the text colour — see
    /// [`NativeButtonView::params_for`]'s doc for why it's threaded
    /// explicitly, and `crate::api::theme`'s mapping table for which arm
    /// honours which (Android's `DatePicker` honours neither).
    fn params_for(&self, slot: SlotId, tokens: Option<ResolvedTheme>) -> String {
        let mut body = ParamsBody::new();
        body.push_raw(date_picker::DATE, date_picker::wire(self.date));
        if let Some(min) = self.min {
            body.push_raw(date_picker::MIN_DATE, date_picker::wire(min));
        }
        if let Some(max) = self.max {
            body.push_raw(date_picker::MAX_DATE, date_picker::wire(max));
        }
        body.push_str(date_picker::STYLE, self.style.wire());
        body.push_raw(ENABLED, self.enabled);
        body.push_opt_str(CONTENT_DESCRIPTION, self.content_description.as_deref());
        body.push_raw(DARK, tokens.is_some_and(|t| t.dark));
        if let Some(t) = tokens {
            body.push_raw(TINT, t.accent_ink);
            body.push_raw(TEXT_COLOR, t.body_text);
        }
        with_identity(date_picker::KIND, slot, &body.finish())
    }

    fn build_with_mode(
        &self,
        slot: SlotId,
        mode: ResolvedSurfaceMode,
        tokens: Option<ResolvedTheme>,
    ) -> AnyView<SlotId> {
        if mode.translucency_refused() {
            return placeholder(self.size, "Date picker");
        }
        let params = self.params_for(slot, tokens);
        if let Some(on_change) = self.on_change.clone() {
            with_runtime(|rt| rt.set_callback(slot, on_date(on_change)));
        }
        let view = platform_view(VIEW_TYPE)
            .params_json(params)
            .interactive()
            .semantics_label(
                self.content_description
                    .clone()
                    .unwrap_or_else(|| "date picker".into()),
            );
        any(resolve_size(self.size, view))
    }
}

impl Component for NativeDatePickerView {
    type State = SlotId;

    fn init(&self) -> SlotId {
        let slot = next_local_slot();
        // See `NativeButtonView::init`'s doc — the picker registers a
        // callback via `Self::on_change`, so it needs the same
        // `pending_callbacks` reaper.
        on_cleanup(move || {
            with_runtime(|rt| rt.forget_pending_callback(slot));
        });
        slot
    }

    fn build(&self, state: &mut SlotId) -> AnyView<SlotId> {
        self.build_with_mode(*state, resolved_surface_mode(), ambient_theme_tokens())
    }
}

// ============================================================================
// Segmented
// ============================================================================

/// Whether this build's platform arm registers the segmented control — the
/// crate's first **compile-time** platform gate on a builder. `Segmented` is
/// in `crate::controls::APPLE_KINDS`: iOS and macOS carry a `NativeWidget`
/// impl, Android does not (decision D2 — `crate::controls::segmented`'s module
/// doc), and neither does any host target. Where it is `false`,
/// [`NativeSegmentedView`] renders the frust-drawn refusal banner
/// ([`RefusalBanner`], via [`banner_placeholder`]) instead of publishing a slot
/// no registered kind could serve — a visible, screen-reader-announced
/// refusal rather than the factory's silent empty dead-slot view.
///
/// A `cfg`-selected constant rather than `cfg`'d code paths so both branches
/// compile, and are host-tested ([`NativeSegmentedView::build_for_arm`]), on
/// every target.
#[cfg(any(target_os = "ios", target_os = "macos"))]
const SEGMENTED_ARM: bool = true;
/// See the Apple-arm definition above: no segmented arm on Android (D2) or on
/// any host target.
#[cfg(not(any(target_os = "ios", target_os = "macos")))]
const SEGMENTED_ARM: bool = false;

/// The banner's visible label on a target with no segmented arm.
const SEGMENTED_UNAVAILABLE_LABEL: &str = "Native segmented control unavailable";

/// The banner's explanation on a target with no segmented arm — names the
/// missing Android arm and where it is tracked.
const SEGMENTED_UNAVAILABLE_DESCRIPTION: &str = "native_segmented has no Android arm in this \
     build (the framework has no segmented control; a Material-backed Android arm is a \
     follow-up plan) — rendering a frust placeholder instead of an empty native slot.";

/// Log the no-segmented-arm fallback exactly once, crate-wide.
static SEGMENTED_NO_ARM_LOGGED: Once = Once::new();

fn warn_no_segmented_arm_once() {
    SEGMENTED_NO_ARM_LOGGED.call_once(|| {
        log::warn!(
            "frust-native-widgets: native_segmented is an iOS/macOS-only control in this build \
             (no Android arm yet — a Material-backed one is a follow-up plan); rendering the \
             frust-drawn refusal banner instead of a native slot"
        );
    });
}

/// A real segmented control rendered from pure Rust — `UISegmentedControl` on
/// iOS/iPadOS, `NSSegmentedControl` on macOS, and a frust-drawn refusal banner
/// everywhere else (Android included: see [`SEGMENTED_ARM`]). A
/// **controlled** component, like [`NativeSwitchView`]: the app owns
/// `selected`, and a tap only ever arrives through [`Self::on_select`] as a
/// *requested* index. Build one with [`native_segmented`].
#[derive(Clone)]
pub struct NativeSegmentedView {
    labels: Vec<String>,
    selected: usize,
    enabled: bool,
    momentary: bool,
    content_description: Option<String>,
    size: Option<(f64, f64)>,
    on_select: Option<Arc<dyn Fn(usize) + Send + Sync>>,
}

/// A native segmented control over `labels`, with the app-owned `selected`
/// segment — see [`NativeSegmentedView`]. An out-of-range `selected` shows no
/// selection rather than failing; at most 64 segments are shown
/// (`crate::controls::segmented::MAX_SEGMENTS`).
pub fn native_segmented(labels: Vec<String>, selected: usize) -> NativeSegmentedView {
    NativeSegmentedView {
        labels,
        selected,
        enabled: true,
        momentary: false,
        content_description: None,
        size: None,
        on_select: None,
    }
}

impl NativeSegmentedView {
    /// `UIControl.enabled` / `NSControl.enabled` — default `true`.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Momentary tracking: a tap flashes its segment and reports it through
    /// [`Self::on_select`] without leaving it selected (a toolbar of
    /// actions rather than a choice). Default `false`.
    pub fn momentary(mut self, momentary: bool) -> Self {
        self.momentary = momentary;
        self
    }

    /// The VoiceOver label.
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

    /// Fires with the *requested* segment index on a user tap — the app
    /// confirms (or rejects) it by feeding `selected` back through the next
    /// build, the controlled-component contract every interactive frust
    /// widget follows. Never fires on a target with no segmented arm.
    pub fn on_select(mut self, handler: impl Fn(usize) + Send + Sync + 'static) -> Self {
        self.on_select = Some(Arc::new(handler));
        self
    }

    /// `tokens` (theme ladder L2) folds the active theme's `accent_fill` in
    /// as the selected segment's tint — see [`NativeButtonView::params_for`]'s
    /// doc for why it's threaded explicitly, and
    /// `crate::controls::segmented`'s module doc for the per-arm property.
    /// The labels ride flat `segmentCount` + `segment<i>` keys (that module
    /// doc's *The labels ride flat params*).
    fn params_for(&self, slot: SlotId, tokens: Option<ResolvedTheme>) -> String {
        let mut body = ParamsBody::new();
        body.push_raw(segmented::SEGMENT_COUNT, self.labels.len());
        for (index, label) in self.labels.iter().enumerate() {
            body.push_str(&segmented::segment_key(index), label);
        }
        body.push_raw(segmented::SELECTED, self.selected);
        body.push_raw(segmented::MOMENTARY, self.momentary);
        body.push_raw(ENABLED, self.enabled);
        body.push_opt_str(CONTENT_DESCRIPTION, self.content_description.as_deref());
        body.push_raw(DARK, tokens.is_some_and(|t| t.dark));
        if let Some(t) = tokens {
            body.push_raw(TINT, t.accent_fill);
        }
        with_identity(segmented::KIND, slot, &body.finish())
    }

    fn build_with_mode(
        &self,
        slot: SlotId,
        mode: ResolvedSurfaceMode,
        tokens: Option<ResolvedTheme>,
    ) -> AnyView<SlotId> {
        self.build_for_arm(slot, mode, tokens, SEGMENTED_ARM)
    }

    /// [`Self::build_with_mode`] with the platform-arm gate threaded as a
    /// parameter, so a host test can drive both branches on any target.
    ///
    /// No arm → the unavailable banner, before anything else: the refusal is
    /// structural, whatever the surface mode, and no callback is registered
    /// (no event could ever arrive for it).
    fn build_for_arm(
        &self,
        slot: SlotId,
        mode: ResolvedSurfaceMode,
        tokens: Option<ResolvedTheme>,
        arm_available: bool,
    ) -> AnyView<SlotId> {
        if !arm_available {
            warn_no_segmented_arm_once();
            return banner_placeholder(
                self.size,
                SEGMENTED_UNAVAILABLE_LABEL.to_string(),
                SEGMENTED_UNAVAILABLE_DESCRIPTION.to_string(),
            );
        }
        if mode.translucency_refused() {
            return placeholder(self.size, "SegmentedControl");
        }
        let params = self.params_for(slot, tokens);
        if let Some(on_select) = self.on_select.clone() {
            with_runtime(|rt| rt.set_callback(slot, on_selected(on_select)));
        }
        let view = platform_view(VIEW_TYPE)
            .params_json(params)
            .interactive()
            .semantics_label(
                self.content_description
                    .clone()
                    .unwrap_or_else(|| "segmented control".into()),
            );
        any(resolve_size(self.size, view))
    }
}

impl Component for NativeSegmentedView {
    type State = SlotId;

    fn init(&self) -> SlotId {
        let slot = next_local_slot();
        // See `NativeButtonView::init`'s doc — `Segmented` registers a
        // callback via `Self::on_select`, so it needs the same
        // `pending_callbacks` reaper (a harmless no-op on a target with no
        // arm, where nothing is ever registered).
        on_cleanup(move || {
            with_runtime(|rt| rt.forget_pending_callback(slot));
        });
        slot
    }

    fn build(&self, state: &mut SlotId) -> AnyView<SlotId> {
        self.build_with_mode(*state, resolved_surface_mode(), ambient_theme_tokens())
    }
}

// ============================================================================
// Stepper
// ============================================================================

/// Whether this build's platform arm registers the stepper control — the
/// same compile-time platform gate [`SEGMENTED_ARM`] is, for `Stepper`
/// instead. `Stepper` is in `crate::controls::APPLE_KINDS`: iOS and macOS
/// carry a `NativeWidget` impl, Android does not (`android.widget` has no
/// increment/decrement control — `crate::controls::stepper`'s module doc),
/// and neither does any host target. Where it is `false`,
/// [`NativeStepperView`] renders the frust-drawn refusal banner
/// ([`RefusalBanner`], via [`banner_placeholder`]) instead of publishing a
/// slot no registered kind could serve.
///
/// A `cfg`-selected constant rather than `cfg`'d code paths so both branches
/// compile, and are host-tested ([`NativeStepperView::build_for_arm`]), on
/// every target.
#[cfg(any(target_os = "ios", target_os = "macos"))]
const STEPPER_ARM: bool = true;
/// See the Apple-arm definition above: no stepper arm on Android or on any
/// host target.
#[cfg(not(any(target_os = "ios", target_os = "macos")))]
const STEPPER_ARM: bool = false;

/// The banner's visible label on a target with no stepper arm.
const STEPPER_UNAVAILABLE_LABEL: &str = "Native stepper unavailable";

/// The banner's explanation on a target with no stepper arm — names the
/// missing Android arm and where it is tracked.
const STEPPER_UNAVAILABLE_DESCRIPTION: &str = "native_stepper has no Android arm in this build \
     (the framework has no increment/decrement control; a composite Android arm is a follow-up \
     plan) — rendering a frust placeholder instead of an empty native slot.";

/// Log the no-stepper-arm fallback exactly once, crate-wide.
static STEPPER_NO_ARM_LOGGED: Once = Once::new();

fn warn_no_stepper_arm_once() {
    STEPPER_NO_ARM_LOGGED.call_once(|| {
        log::warn!(
            "frust-native-widgets: native_stepper is an iOS/macOS-only control in this build (no \
             Android arm yet — a composite one is a follow-up plan); rendering the frust-drawn \
             refusal banner instead of a native slot"
        );
    });
}

/// A real increment/decrement control rendered from pure Rust — `UIStepper`
/// on iOS/iPadOS, `NSStepper` on macOS, and a frust-drawn refusal banner
/// everywhere else (Android included: see [`STEPPER_ARM`]). A **controlled**
/// component, like [`NativeSliderView`]: the app owns `value`, and a tap on
/// either button only ever arrives through [`Self::on_change`] as a
/// *requested* value. Build one with [`native_stepper`].
///
/// `min`/`max`/[`Self::step`] are normalized before they ever reach a
/// platform control (`crate::controls::stepper`'s module doc's *Range and
/// step invariants*: `UIStepper` aborts the process on a non-positive
/// `stepValue` or a `maximumValue` not strictly greater than
/// `minimumValue`). A degenerate range (`max <= min`) renders the control
/// **disabled** — the user can never tap it, and no value outside `[min,
/// max]` is ever reported — rather than refusing to mount or crashing; it
/// re-enables the moment a later update makes the range non-degenerate
/// again.
#[derive(Clone)]
pub struct NativeStepperView {
    value: i32,
    min: i32,
    max: i32,
    step: i32,
    wraps: bool,
    enabled: bool,
    content_description: Option<String>,
    size: Option<(f64, f64)>,
    on_change: Option<Arc<dyn Fn(i32) + Send + Sync>>,
}

/// A native `Stepper` at `value`, ranging over `[min, max]` — see
/// [`NativeStepperView`].
pub fn native_stepper(value: i32, min: i32, max: i32) -> NativeStepperView {
    NativeStepperView {
        value,
        min,
        max,
        step: 1,
        wraps: false,
        enabled: true,
        content_description: None,
        size: None,
        on_change: None,
    }
}

impl NativeStepperView {
    /// The increment a tap on either button applies — default `1`. A
    /// non-positive value (`0` or negative) is normalized to `1` before it
    /// ever reaches a platform control, logged once — `UIStepper` requires
    /// `stepValue > 0` (`crate::controls::stepper`'s module doc's *Range
    /// and step invariants*).
    pub fn step(mut self, step: i32) -> Self {
        self.step = step;
        self
    }

    /// Whether the value wraps from `max` back to `min` (and back) instead
    /// of clamping at the bounds — default `false`.
    pub fn wraps(mut self, wraps: bool) -> Self {
        self.wraps = wraps;
        self
    }

    /// `UIControl.enabled` / `NSControl.enabled` — default `true`.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The VoiceOver label.
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

    /// Fires with the requested **app-space** value on a tap
    /// (`crate::controls::stepper`'s platform-space mapping already undone —
    /// the same mapping [`NativeSliderView::on_change`] documents). Never
    /// fires on a target with no stepper arm.
    pub fn on_change(mut self, handler: impl Fn(i32) + Send + Sync + 'static) -> Self {
        self.on_change = Some(Arc::new(handler));
        self
    }

    /// `tokens` (theme ladder L2) folds the active theme's `accent_ink` in as
    /// `UIStepper.tintColor` — see [`NativeButtonView::params_for`]'s doc for
    /// why it's threaded explicitly, and `crate::controls::stepper`'s module
    /// doc's *Tint* section for why `NSStepper` never receives it (logged and
    /// no-op'd on that one arm, not a gap in this fold).
    fn params_for(&self, slot: SlotId, tokens: Option<ResolvedTheme>) -> String {
        let mut body = ParamsBody::new();
        body.push_raw(VALUE, self.value);
        body.push_raw(MIN, self.min);
        body.push_raw(MAX, self.max);
        body.push_raw(STEP, self.step);
        body.push_raw(WRAPS, self.wraps);
        body.push_raw(ENABLED, self.enabled);
        body.push_opt_str(CONTENT_DESCRIPTION, self.content_description.as_deref());
        body.push_raw(DARK, tokens.is_some_and(|t| t.dark));
        if let Some(t) = tokens {
            body.push_raw(TINT, t.accent_ink);
        }
        with_identity(stepper::KIND, slot, &body.finish())
    }

    fn build_with_mode(
        &self,
        slot: SlotId,
        mode: ResolvedSurfaceMode,
        tokens: Option<ResolvedTheme>,
    ) -> AnyView<SlotId> {
        self.build_for_arm(slot, mode, tokens, STEPPER_ARM)
    }

    /// [`Self::build_with_mode`] with the platform-arm gate threaded as a
    /// parameter, so a host test can drive both branches on any target —
    /// [`NativeSegmentedView::build_for_arm`]'s exact shape.
    ///
    /// No arm → the unavailable banner, before anything else: the refusal is
    /// structural, whatever the surface mode, and no callback is registered
    /// (no event could ever arrive for it).
    fn build_for_arm(
        &self,
        slot: SlotId,
        mode: ResolvedSurfaceMode,
        tokens: Option<ResolvedTheme>,
        arm_available: bool,
    ) -> AnyView<SlotId> {
        if !arm_available {
            warn_no_stepper_arm_once();
            return banner_placeholder(
                self.size,
                STEPPER_UNAVAILABLE_LABEL.to_string(),
                STEPPER_UNAVAILABLE_DESCRIPTION.to_string(),
            );
        }
        if mode.translucency_refused() {
            return placeholder(self.size, "Stepper");
        }
        let params = self.params_for(slot, tokens);
        if let Some(on_change) = self.on_change.clone() {
            with_runtime(|rt| rt.set_callback(slot, on_value_changed(on_change)));
        }
        let view = platform_view(VIEW_TYPE)
            .params_json(params)
            .interactive()
            .semantics_label(
                self.content_description
                    .clone()
                    .unwrap_or_else(|| "stepper".into()),
            );
        any(resolve_size(self.size, view))
    }
}

impl Component for NativeStepperView {
    type State = SlotId;

    fn init(&self) -> SlotId {
        let slot = next_local_slot();
        // See `NativeButtonView::init`'s doc — `Stepper` registers a
        // callback via `Self::on_change`, so it needs the same
        // `pending_callbacks` reaper (a harmless no-op on a target with no
        // arm, where nothing is ever registered).
        on_cleanup(move || {
            with_runtime(|rt| rt.forget_pending_callback(slot));
        });
        slot
    }

    fn build(&self, state: &mut SlotId) -> AnyView<SlotId> {
        self.build_with_mode(*state, resolved_surface_mode(), ambient_theme_tokens())
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
    /// re-publishing. `tokens` only ever contributes [`DARK`] here (theme
    /// ladder L1): an app-supplied image's *content* is arbitrary
    /// bytes, so folding an accent tint over it the way the other five
    /// controls fold colour tokens would corrupt a real photo rather than
    /// theme a control — [`NativeImageView`] exposes no tint builder yet for
    /// the same reason (`api::builders`' own "left for a future task" note
    /// on styling knobs).
    fn params_for(&self, slot: SlotId, rev: u64, tokens: Option<ResolvedTheme>) -> String {
        let mut body = ParamsBody::new();
        body.push_raw(image::REV, rev);
        body.push_str(FIT, self.fit.wire());
        body.push_opt_str(CONTENT_DESCRIPTION, self.content_description.as_deref());
        body.push_raw(DARK, tokens.is_some_and(|t| t.dark));
        with_identity(image::KIND, slot, &body.finish())
    }

    fn build_with_mode(
        &self,
        slot: SlotId,
        mode: ResolvedSurfaceMode,
        tokens: Option<ResolvedTheme>,
    ) -> AnyView<SlotId> {
        if mode.translucency_refused() {
            return placeholder(self.size, "Image");
        }
        // Publish (or re-confirm) this slot's bytes BEFORE encoding params —
        // `image::publish_bytes` is idempotent for the same `Arc` (module
        // doc: "an app that hands its buffer down every rebuild produces no
        // params change and no decode"), and the runtime's later
        // `ImageProps::decode` reads this same table back by slot.
        let rev = image::publish_bytes(slot, Arc::clone(&self.bytes));
        let params = self.params_for(slot, rev, tokens);
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
        let slot = next_local_slot();
        // Tie the publish-table entry's lifetime to this Component, not to
        // the native create/dispose lifecycle (see
        // `docs/CODE_STANDARDS.md`'s "Teardown disposes the component's
        // `Owner`; register cleanup via `on_cleanup`, not `Drop`"). `init`
        // runs exactly once, under this component's own `Owner`, so
        // `on_cleanup` here fires exactly once when that owner disposes —
        // regardless of whether paint culling already ran the counted
        // `claim_bytes`/`release_bytes` pair to zero and back on the
        // platform side in between (`crate::controls::image`'s module doc).
        //
        // No second `on_cleanup` for `NativeRuntime::pending_callbacks`:
        // `Image` is display-only and never calls `set_callback` —
        // see `NativeLabelView::init`'s doc for the same reasoning.
        on_cleanup(move || image::retire(slot));
        slot
    }

    fn build(&self, state: &mut SlotId) -> AnyView<SlotId> {
        self.build_with_mode(*state, resolved_surface_mode(), ambient_theme_tokens())
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
impl_native_view!(NativeSpinnerView);
impl_native_view!(NativeDatePickerView);
impl_native_view!(NativeSegmentedView);
impl_native_view!(NativeStepperView);

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{BoxConstraints, LayoutCtx, PaintCtx, PaintScene, Widget};
    use frust_text::TextContext;
    use kurbo::{Point, Rect, Shape, Size};
    use peniko::{Brush, Color};
    use std::any::Any;

    /// A minimal `PaintScene` — only `fill_rect`/`draw_text` have no default
    /// (see `frust_core::widget::PaintScene`'s trait definition); every other
    /// method a placeholder's banner/`SizedBox` might call is defaulted.
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

    // --- params_json snapshots, one per control (no theme) -------------------
    //
    // `None` is exactly what `ambient_theme_tokens()` returns absent a live
    // reactive context (this module's own doc comment) — every snapshot
    // below carries a plain `"dark":false` and no other theme field, the
    // original shape plus theme ladder L1's always-present flag.

    #[test]
    fn button_params_snapshot() {
        let view = native_button("Save")
            .enabled(false)
            .content_description("Save the note");
        assert_eq!(
            view.params_for(7, None),
            "{\"__frustControl\":\"button\",\"__frustSlot\":7,\"text\":\"Save\",\"enabled\":false,\
             \"contentDescription\":\"Save the note\",\"dark\":false}"
        );
    }

    #[test]
    fn label_params_snapshot() {
        let view = native_label("42 fps");
        assert_eq!(
            view.params_for(3, None),
            "{\"__frustControl\":\"label\",\"__frustSlot\":3,\"text\":\"42 fps\",\"enabled\":true,\
             \"dark\":false}"
        );
    }

    #[test]
    fn switch_params_snapshot() {
        let view = native_switch(true).content_description("wifi");
        assert_eq!(
            view.params_for(11, None),
            "{\"__frustControl\":\"switch\",\"__frustSlot\":11,\"checked\":true,\"enabled\":true,\
             \"contentDescription\":\"wifi\",\"dark\":false}"
        );
    }

    #[test]
    fn slider_params_snapshot() {
        let view = native_slider(25, 0, 50);
        assert_eq!(
            view.params_for(5, None),
            "{\"__frustControl\":\"slider\",\"__frustSlot\":5,\"value\":25,\"min\":0,\"max\":50,\
             \"enabled\":true,\"dark\":false}"
        );
    }

    #[test]
    fn progress_params_snapshot() {
        let view = native_progress(30, 10, 110).indeterminate(false);
        assert_eq!(
            view.params_for(2, None),
            "{\"__frustControl\":\"progress\",\"__frustSlot\":2,\"value\":30,\"min\":10,\
             \"max\":110,\"indeterminate\":false,\"dark\":false}"
        );
    }

    #[test]
    fn image_params_snapshot() {
        let view =
            native_image(Arc::from(vec![1u8, 2, 3].into_boxed_slice())).fit(NativeImageFit::Cover);
        assert_eq!(
            view.params_for(900, 42, None),
            "{\"__frustControl\":\"image\",\"__frustSlot\":900,\"imageRev\":42,\"fit\":\"cover\",\
             \"dark\":false}"
        );
    }

    #[test]
    fn spinner_params_snapshot() {
        let view = native_spinner(true)
            .size_class(NativeSpinnerSize::Large)
            .content_description("loading");
        assert_eq!(
            view.params_for(6, None),
            "{\"__frustControl\":\"spinner\",\"__frustSlot\":6,\"animating\":true,\
             \"sizeClass\":\"large\",\"enabled\":true,\"contentDescription\":\"loading\",\
             \"dark\":false}"
        );
    }

    fn civil(year: i32, month: u8, day: u8) -> CivilDate {
        CivilDate::new(year, month, day).expect("a real date")
    }

    #[test]
    fn date_picker_params_snapshot() {
        let view = native_date_picker(civil(2026, 9, 29))
            .min(civil(2026, 1, 1))
            .max(civil(2026, 12, 31))
            .style(NativeDatePickerStyle::Inline)
            .content_description("due date");
        assert_eq!(
            view.params_for(12, None),
            format!(
                "{{\"__frustControl\":\"date_picker\",\"__frustSlot\":12,\"date\":{},\
                 \"minDate\":{},\"maxDate\":{},\"style\":\"inline\",\"enabled\":true,\
                 \"contentDescription\":\"due date\",\"dark\":false}}",
                (2026 << 16) | (9 << 8) | 29,
                (2026 << 16) | (1 << 8) | 1,
                (2026 << 16) | (12 << 8) | 31,
            )
        );
    }

    #[test]
    fn date_picker_params_round_trip_through_the_control_decoder() {
        use crate::controls::date_picker::DatePickerProps;
        use crate::runtime::Params;

        let view = native_date_picker(civil(2024, 2, 29))
            .min(civil(2000, 1, 1))
            .style(NativeDatePickerStyle::Wheels)
            .enabled(false);
        let raw = view.params_for(12, Some(dark_tokens()));
        let props = DatePickerProps::decode(&Params::new(&raw)).expect("decodes");
        assert_eq!(props.date, Some(civil(2024, 2, 29)));
        assert_eq!(props.min, Some(civil(2000, 1, 1)));
        assert_eq!(props.max, None);
        assert!(!props.enabled);
        assert_eq!(props.tint, Some(dark_tokens().accent_ink as i32));
        assert_eq!(props.text_color, Some(dark_tokens().body_text as i32));
    }

    #[test]
    fn date_picker_publishes_one_interactive_slot_on_every_target() {
        // A shared control: no compile-time arm gate, unlike
        // `native_segmented`/`native_stepper`.
        let view = native_date_picker(civil(2026, 9, 29)).size(320.0, 216.0);
        let expected_params = view.params_for(4, None);
        let built = view.build_with_mode(4, ResolvedSurfaceMode::Opaque, None);
        let mut element = build_any(built);
        let mut lctx = LayoutCtx::new();
        element.layout(&mut lctx, &BoxConstraints::tight(Size::new(320.0, 216.0)));
        let mut scene = NullScene;
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(320.0, 216.0));
        element.paint(&mut pctx, &mut scene);
        let frames = pctx.take_platform_views();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].view_type, VIEW_TYPE);
        assert_eq!(frames[0].params_json, expected_params);
        assert!(
            frames[0].interactive,
            "a date picker slot forwards native input"
        );
    }

    #[test]
    fn date_picker_honours_the_translucency_refusal() {
        let view = native_date_picker(civil(2026, 9, 29)).build_with_mode(
            5,
            ResolvedSurfaceMode::RefusedTranslucent,
            None,
        );
        let mut element = build_any(view);
        let mut scene = NullScene;
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(320.0, 216.0));
        element.paint(&mut pctx, &mut scene);
        assert!(pctx.take_platform_views().is_empty());
    }

    #[test]
    fn segmented_params_snapshot() {
        let view = native_segmented(vec!["Day".into(), "Week \"W\"".into()], 1)
            .momentary(true)
            .content_description("range");
        assert_eq!(
            view.params_for(8, None),
            "{\"__frustControl\":\"segmented\",\"__frustSlot\":8,\"segmentCount\":2,\
             \"segment0\":\"Day\",\"segment1\":\"Week \\\"W\\\"\",\"selected\":1,\
             \"momentary\":true,\"enabled\":true,\"contentDescription\":\"range\",\
             \"dark\":false}"
        );
    }

    #[test]
    fn segmented_params_round_trip_through_the_control_decoder() {
        use crate::controls::segmented::SegmentedProps;
        use crate::runtime::Params;

        let view = native_segmented(vec!["A".into(), "B".into(), "C".into()], 2).enabled(false);
        let raw = view.params_for(8, Some(dark_tokens()));
        let props = SegmentedProps::decode(&Params::new(&raw)).expect("decodes");
        assert_eq!(props.labels, vec!["A", "B", "C"]);
        assert_eq!(props.selected, Some(2));
        assert!(!props.enabled);
        assert!(!props.momentary);
        assert_eq!(props.tint, Some(dark_tokens().accent_fill as i32));
    }

    #[test]
    fn the_segmented_arm_gate_is_exactly_the_apple_targets() {
        assert_eq!(
            SEGMENTED_ARM,
            cfg!(any(target_os = "ios", target_os = "macos")),
            "Android (decision D2) and every host target render the banner"
        );
        assert!(SEGMENTED_UNAVAILABLE_DESCRIPTION.contains("Android"));
        assert!(SEGMENTED_UNAVAILABLE_DESCRIPTION.contains("follow-up"));
    }

    #[test]
    fn segmented_without_a_platform_arm_paints_the_banner_and_publishes_no_slot() {
        // Every surface mode, including the ones a native slot would render
        // in: with no arm, the refusal is structural.
        for mode in [
            ResolvedSurfaceMode::Unknown,
            ResolvedSurfaceMode::Opaque,
            ResolvedSurfaceMode::Translucent,
            ResolvedSurfaceMode::RefusedTranslucent,
        ] {
            let view = native_segmented(vec!["A".into(), "B".into()], 0)
                .size(240.0, 60.0)
                .build_for_arm(3, mode, None, false);
            let mut element = build_any(view);
            let mut text_ctx = TextContext::new();
            let mut lctx = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
            let laid = element.layout(&mut lctx, &BoxConstraints::tight(Size::new(240.0, 60.0)));
            let mut rec = BoundsRecorder::default();
            let mut pctx = PaintCtx::new(Point::ZERO, laid);
            element.paint(&mut pctx, &mut rec);
            assert!(
                pctx.take_platform_views().is_empty(),
                "{mode:?}: no arm must publish no native platform_view frame"
            );
            assert!(
                rec.glyph_runs >= 2,
                "{mode:?}: the banner's label and description must paint, saw {} runs",
                rec.glyph_runs
            );
        }
    }

    #[test]
    fn segmented_with_a_platform_arm_publishes_one_interactive_slot() {
        let view = native_segmented(vec!["A".into(), "B".into()], 1).size(200.0, 32.0);
        let expected_params = view.params_for(4, None);
        let built = view.build_for_arm(4, ResolvedSurfaceMode::Opaque, None, true);
        let mut element = build_any(built);
        let mut lctx = LayoutCtx::new();
        element.layout(&mut lctx, &BoxConstraints::tight(Size::new(200.0, 32.0)));
        let mut scene = NullScene;
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(200.0, 32.0));
        element.paint(&mut pctx, &mut scene);
        let frames = pctx.take_platform_views();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].view_type, VIEW_TYPE);
        assert_eq!(frames[0].params_json, expected_params);
        assert!(
            frames[0].interactive,
            "a segmented slot forwards native input"
        );
    }

    #[test]
    fn segmented_with_an_arm_still_honours_the_translucency_refusal() {
        let view = native_segmented(vec!["A".into()], 0).build_for_arm(
            5,
            ResolvedSurfaceMode::RefusedTranslucent,
            None,
            true,
        );
        let mut element = build_any(view);
        let mut scene = NullScene;
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(120.0, 32.0));
        element.paint(&mut pctx, &mut scene);
        assert!(pctx.take_platform_views().is_empty());
    }

    #[test]
    fn stepper_params_snapshot() {
        let view = native_stepper(4, 0, 10)
            .step(2)
            .wraps(true)
            .content_description("count");
        assert_eq!(
            view.params_for(8, None),
            "{\"__frustControl\":\"stepper\",\"__frustSlot\":8,\"value\":4,\"min\":0,\"max\":10,\
             \"step\":2,\"wraps\":true,\"enabled\":true,\"contentDescription\":\"count\",\
             \"dark\":false}"
        );
    }

    #[test]
    fn stepper_params_round_trip_through_the_control_decoder() {
        use crate::controls::stepper::StepperProps;
        use crate::runtime::Params;

        let view = native_stepper(3, 0, 20).step(5).wraps(true).enabled(false);
        let raw = view.params_for(8, Some(dark_tokens()));
        let props = StepperProps::decode(&Params::new(&raw)).expect("decodes");
        assert_eq!(props.value, 3);
        assert_eq!(props.step, 5);
        assert!(props.wraps);
        assert!(!props.enabled);
        assert_eq!(props.tint, Some(dark_tokens().accent_ink as i32));
    }

    #[test]
    fn a_non_positive_step_mounts_as_one_through_the_control_decoder() {
        use crate::controls::stepper::StepperProps;
        use crate::runtime::Params;

        let view = native_stepper(3, 0, 20).step(0);
        let raw = view.params_for(8, None);
        assert!(
            raw.contains("\"step\":0"),
            "the builder still sends the app's raw value over the wire — \
             normalization is the control's job, not the builder's"
        );
        let props = StepperProps::decode(&Params::new(&raw)).expect("decodes");
        assert_eq!(props.step, 1, "a non-positive step mounts as 1");
    }

    #[test]
    fn the_stepper_arm_gate_is_exactly_the_apple_targets() {
        assert_eq!(
            STEPPER_ARM,
            cfg!(any(target_os = "ios", target_os = "macos")),
            "Android and every host target render the banner"
        );
        assert!(STEPPER_UNAVAILABLE_DESCRIPTION.contains("Android"));
        assert!(STEPPER_UNAVAILABLE_DESCRIPTION.contains("follow-up"));
    }

    #[test]
    fn stepper_without_a_platform_arm_paints_the_banner_and_publishes_no_slot() {
        // Every surface mode, including the ones a native slot would render
        // in: with no arm, the refusal is structural.
        for mode in [
            ResolvedSurfaceMode::Unknown,
            ResolvedSurfaceMode::Opaque,
            ResolvedSurfaceMode::Translucent,
            ResolvedSurfaceMode::RefusedTranslucent,
        ] {
            let view = native_stepper(0, 0, 10)
                .size(120.0, 40.0)
                .build_for_arm(3, mode, None, false);
            let mut element = build_any(view);
            let mut text_ctx = TextContext::new();
            let mut lctx = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
            let laid = element.layout(&mut lctx, &BoxConstraints::tight(Size::new(120.0, 40.0)));
            let mut rec = BoundsRecorder::default();
            let mut pctx = PaintCtx::new(Point::ZERO, laid);
            element.paint(&mut pctx, &mut rec);
            assert!(
                pctx.take_platform_views().is_empty(),
                "{mode:?}: no arm must publish no native platform_view frame"
            );
            assert!(
                rec.glyph_runs >= 2,
                "{mode:?}: the banner's label and description must paint, saw {} runs",
                rec.glyph_runs
            );
        }
    }

    #[test]
    fn stepper_with_a_platform_arm_publishes_one_interactive_slot() {
        let view = native_stepper(2, 0, 10).size(94.0, 29.0);
        let expected_params = view.params_for(4, None);
        let built = view.build_for_arm(4, ResolvedSurfaceMode::Opaque, None, true);
        let mut element = build_any(built);
        let mut lctx = LayoutCtx::new();
        element.layout(&mut lctx, &BoxConstraints::tight(Size::new(94.0, 29.0)));
        let mut scene = NullScene;
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(94.0, 29.0));
        element.paint(&mut pctx, &mut scene);
        let frames = pctx.take_platform_views();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].view_type, VIEW_TYPE);
        assert_eq!(frames[0].params_json, expected_params);
        assert!(
            frames[0].interactive,
            "a stepper slot forwards native input"
        );
    }

    #[test]
    fn stepper_with_an_arm_still_honours_the_translucency_refusal() {
        let view = native_stepper(0, 0, 10).build_for_arm(
            5,
            ResolvedSurfaceMode::RefusedTranslucent,
            None,
            true,
        );
        let mut element = build_any(view);
        let mut scene = NullScene;
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(94.0, 29.0));
        element.paint(&mut pctx, &mut scene);
        assert!(pctx.take_platform_views().is_empty());
    }

    // --- theme ladder L2: token folding, one snapshot per colour-bearing
    // control (dark + light, via `theme::resolve`) -------------------------

    fn dark_tokens() -> ResolvedTheme {
        theme::resolve(&Theme::neutral().with_brightness(frust::Brightness::Dark))
    }

    fn light_tokens() -> ResolvedTheme {
        theme::resolve(&Theme::neutral().with_brightness(frust::Brightness::Light))
    }

    #[test]
    fn button_folds_background_text_radius_and_size_from_a_dark_theme() {
        let view = native_button("Save");
        let tokens = dark_tokens();
        assert_eq!(
            view.params_for(7, Some(tokens)),
            format!(
                "{{\"__frustControl\":\"button\",\"__frustSlot\":7,\"text\":\"Save\",\"enabled\":\
                 true,\"dark\":true,\"textColor\":{},\"backgroundColor\":{},\"cornerRadiusDp\":{},\
                 \"textSizeSp\":{},\"typeface\":\"{}\"}}",
                tokens.on_accent_fill,
                tokens.accent_fill,
                tokens.corner_radius_dp,
                tokens.button_text_size_sp,
                tokens.button_typeface.wire()
            )
        );
    }

    #[test]
    fn button_folds_a_light_theme_distinctly_from_dark() {
        let view = native_button("Save");
        let dark = view.params_for(7, Some(dark_tokens()));
        let light = view.params_for(7, Some(light_tokens()));
        assert_ne!(
            dark, light,
            "a brightness flip must reach the folded params — both the `dark` \
             flag and every colour role the button folds change with it"
        );
        assert!(light.contains("\"dark\":false"));
        assert!(dark.contains("\"dark\":true"));
    }

    #[test]
    fn label_folds_body_text_colour_and_size() {
        let view = native_label("42 fps");
        let tokens = dark_tokens();
        assert_eq!(
            view.params_for(3, Some(tokens)),
            format!(
                "{{\"__frustControl\":\"label\",\"__frustSlot\":3,\"text\":\"42 fps\",\"enabled\":\
                 true,\"dark\":true,\"backgroundColor\":{},\"textColor\":{},\"textSizeSp\":{},\
                 \"typeface\":\"{}\"}}",
                tokens.surface_bg,
                tokens.body_text,
                tokens.body_text_size_sp,
                tokens.body_typeface.wire()
            )
        );
    }

    #[test]
    fn switch_folds_thumb_and_track_tint_but_never_a_background() {
        // `Switch` deliberately folds no background — the thumb/track
        // tints already carry the theme, and an explicit `backgroundColor`
        // would replace `?attr/selectableItemBackgroundBorderless`'s ripple
        // (`crate::api::theme`'s module doc's *Explicit backgrounds*
        // section).
        let view = native_switch(true);
        let tokens = dark_tokens();
        let params = view.params_for(11, Some(tokens));
        assert_eq!(
            params,
            format!(
                "{{\"__frustControl\":\"switch\",\"__frustSlot\":11,\"checked\":true,\"enabled\":\
                 true,\"dark\":true,\"thumbTint\":{},\"trackTint\":{},\
                 \"typeface\":\"{}\"}}",
                tokens.accent_ink,
                tokens.accent_fill,
                tokens.body_typeface.wire()
            )
        );
        assert!(
            !params.contains("backgroundColor"),
            "Switch must never fold an explicit background — it would defeat the ripple"
        );
    }

    #[test]
    fn slider_folds_progress_and_thumb_tint_but_never_a_background() {
        // Same rationale as the Switch test above — `AbsSeekBar` also
        // carries `?attr/selectableItemBackgroundBorderless`.
        let view = native_slider(25, 0, 50);
        let tokens = dark_tokens();
        let params = view.params_for(5, Some(tokens));
        assert_eq!(
            params,
            format!(
                "{{\"__frustControl\":\"slider\",\"__frustSlot\":5,\"value\":25,\"min\":0,\"max\":\
                 50,\"enabled\":true,\"dark\":true,\"progressTint\":{},\
                 \"thumbTint\":{}}}",
                tokens.accent_fill, tokens.accent_ink
            )
        );
        assert!(
            !params.contains("backgroundColor"),
            "Slider must never fold an explicit background — it would defeat the ripple"
        );
    }

    #[test]
    fn progress_folds_progress_tint_only() {
        let view = native_progress(30, 10, 110);
        let tokens = dark_tokens();
        assert_eq!(
            view.params_for(2, Some(tokens)),
            format!(
                "{{\"__frustControl\":\"progress\",\"__frustSlot\":2,\"value\":30,\"min\":10,\
                 \"max\":110,\"indeterminate\":false,\"dark\":true,\"backgroundColor\":{},\
                 \"progressTint\":{}}}",
                tokens.surface_bg, tokens.accent_fill
            )
        );
    }

    #[test]
    fn image_folds_only_the_dark_flag_never_a_tint() {
        // An app-supplied photo is arbitrary content — theming it would
        // corrupt the image, not style a control (module doc on
        // `NativeImageView::params_for`).
        let view = native_image(Arc::from(vec![1u8].into_boxed_slice()));
        assert_eq!(
            view.params_for(900, 42, Some(dark_tokens())),
            "{\"__frustControl\":\"image\",\"__frustSlot\":900,\"imageRev\":42,\"fit\":\"contain\",\
             \"dark\":true}"
        );
    }

    #[test]
    fn spinner_folds_accent_ink_as_its_tint() {
        let view = native_spinner(false);
        let tokens = dark_tokens();
        assert_eq!(
            view.params_for(6, Some(tokens)),
            format!(
                "{{\"__frustControl\":\"spinner\",\"__frustSlot\":6,\"animating\":false,\
                 \"sizeClass\":\"medium\",\"enabled\":true,\"dark\":true,\"tint\":{}}}",
                tokens.accent_ink
            )
        );
    }

    #[test]
    fn stepper_folds_accent_ink_as_its_tint() {
        // `UIStepper.tintColor` reads the same accent-ink role Switch/Slider's
        // thumb tint and Spinner's own tint already do — `NSStepper` simply
        // has no AppKit property to apply it through (module doc's *Tint*
        // section on `crate::controls::stepper`), a platform gap this fold
        // does not need to know about at the params level.
        let view = native_stepper(0, 0, 10);
        let tokens = dark_tokens();
        assert_eq!(
            view.params_for(6, Some(tokens)),
            format!(
                "{{\"__frustControl\":\"stepper\",\"__frustSlot\":6,\"value\":0,\"min\":0,\
                 \"max\":10,\"step\":1,\"wraps\":false,\"enabled\":true,\"dark\":true,\"tint\":{}}}",
                tokens.accent_ink
            )
        );
    }

    #[test]
    fn date_picker_folds_accent_ink_tint_and_body_text_colour() {
        let view = native_date_picker(civil(2026, 9, 29));
        let tokens = dark_tokens();
        assert_eq!(
            view.params_for(6, Some(tokens)),
            format!(
                "{{\"__frustControl\":\"date_picker\",\"__frustSlot\":6,\"date\":{},\
                 \"style\":\"compact\",\"enabled\":true,\"dark\":true,\"tint\":{},\
                 \"textColor\":{}}}",
                (2026 << 16) | (9 << 8) | 29,
                tokens.accent_ink,
                tokens.body_text
            )
        );
    }

    // --- the zero-FFI property: an unchanged theme yields PartialEq-equal
    // Props (acceptance criterion) -------------------------------------------

    #[test]
    fn an_unchanged_theme_yields_partial_eq_equal_button_props() {
        use crate::controls::button::ButtonProps;
        use crate::runtime::Params;

        let view = native_button("Save").content_description("Save the note");
        let tokens = dark_tokens();
        let a = ButtonProps::decode(&Params::new(&view.params_for(7, Some(tokens)))).unwrap();
        let b = ButtonProps::decode(&Params::new(&view.params_for(7, Some(tokens)))).unwrap();
        assert_eq!(
            a, b,
            "the SAME resolved theme, folded twice, must decode to PartialEq-equal \
             Props — this is what keeps the runtime's diff gate from crossing the FFI \
             boundary on a rebuild the theme didn't actually change"
        );

        // A genuinely different theme (light) must NOT compare equal.
        let c =
            ButtonProps::decode(&Params::new(&view.params_for(7, Some(light_tokens())))).unwrap();
        assert_ne!(a, c);
    }

    // --- the translucency-refused fallback ----------------------------------

    #[test]
    fn a_refused_slot_publishes_no_platform_view_frame() {
        for (name, view) in [
            (
                "Button",
                native_button("Save").build_with_mode(
                    1,
                    ResolvedSurfaceMode::RefusedTranslucent,
                    None,
                ),
            ),
            (
                "Switch",
                native_switch(true).build_with_mode(
                    2,
                    ResolvedSurfaceMode::RefusedTranslucent,
                    None,
                ),
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
            let expected_params = btn.params_for(9, None);
            let view = btn.build_with_mode(9, mode, None);
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
        let view = native_label("hi").build_with_mode(4, ResolvedSurfaceMode::Opaque, None);
        let mut element = build_any(view);
        let mut scene = NullScene;
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(100.0, 30.0));
        element.paint(&mut pctx, &mut scene);
        let frames = pctx.take_platform_views();
        assert_eq!(frames.len(), 1);
        assert!(!frames[0].interactive, "Label never forwards native input");
    }

    // --- The refusal placeholder's prose must never paint outside its own
    // slot rect -------------------------------------------------------------

    /// A recording [`PaintScene`] that actually honours the clip stack
    /// (unlike [`NullScene`] above), so it models what a real backend would
    /// visually produce: every primitive's own reported bounds get
    /// intersected against whatever clip is active *at paint time* before
    /// being recorded. With no clip pushed (the pre-fix shape), a
    /// primitive's raw, unclamped bounds are recorded as-is — which is
    /// exactly what makes this test capable of catching the original
    /// overflow rather than trivially passing by construction.
    #[derive(Default)]
    struct BoundsRecorder {
        clip_stack: Vec<Rect>,
        /// Every painted primitive's bounds, already clipped against
        /// whatever was active when it painted.
        painted: Vec<Rect>,
        /// How many glyph runs carrying at least one glyph reached the scene,
        /// counted *before* the clip intersection — the "the banner really
        /// paints its warning text" half of the contract, which the clipped
        /// bounds above cannot answer on their own (a fully-clipped run
        /// records nothing).
        glyph_runs: usize,
    }

    impl BoundsRecorder {
        fn record(&mut self, rect: Rect) {
            let bounded = match self.clip_stack.last() {
                Some(clip) => rect.intersect(*clip),
                None => rect,
            };
            // A fully-clipped-away rect never actually paints a pixel —
            // skip it rather than recording a degenerate zero-size rect.
            if bounded.width() > 0.0 && bounded.height() > 0.0 {
                self.painted.push(bounded);
            }
        }
    }

    impl PaintScene for BoundsRecorder {
        fn fill_rect(&mut self, origin: Point, size: Size, _color: Color) {
            self.record(Rect::from_origin_size(origin, size));
        }

        fn draw_text(&mut self, _origin: Point, _text: &str) {}

        fn fill_rounded_rect(&mut self, origin: Point, size: Size, _radius: f64, _color: Color) {
            self.record(Rect::from_origin_size(origin, size));
        }

        fn stroke_path(
            &mut self,
            origin: Point,
            path: &kurbo::BezPath,
            width: f64,
            _brush: &Brush,
        ) {
            let bbox = path.bounding_box() + origin.to_vec2();
            // Pad by the stroke width so the border's own ink is covered,
            // not just its centerline path.
            self.record(bbox.inflate(width, width));
        }

        fn push_clip(&mut self, origin: Point, size: Size) {
            let rect = Rect::from_origin_size(origin, size);
            let bounded = match self.clip_stack.last() {
                Some(prev) => rect.intersect(*prev),
                None => rect,
            };
            self.clip_stack.push(bounded);
        }

        fn pop_clip(&mut self) {
            self.clip_stack.pop();
        }

        fn draw_glyph_run(&mut self, run: frust_scene::GlyphRun) {
            let Some(first) = run.glyphs.first() else {
                return;
            };
            self.glyph_runs += 1;
            // The run's transform is a pure translation baked from the
            // paint-time origin (`frust_text::TextLayout::to_scene_runs`) —
            // `.translation()` recovers it directly. Glyph x/y are local
            // (pre-transform) positions; no font-metrics access exists at
            // this layer, so a generous `font_size`-wide margin around the
            // glyphs' local extent stands in for real ascent/descent —
            // over-approximating is fine here, since this test only needs
            // to catch genuine overflow, not measure exact ink bounds.
            let base = run.transform.translation();
            let margin = run.font_size as f64;
            let (min_x, max_x) = run
                .glyphs
                .iter()
                .map(|g| g.x as f64)
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), x| {
                    (lo.min(x), hi.max(x))
                });
            let rect = Rect::new(
                base.x + min_x - margin,
                base.y + first.y as f64 - margin,
                base.x + max_x + margin,
                base.y + first.y as f64 + margin,
            );
            self.record(rect);
        }
    }

    fn rect_fits_within(outer: Rect, inner: Rect) -> bool {
        const TOLERANCE: f64 = 0.01;
        inner.x0 >= outer.x0 - TOLERANCE
            && inner.y0 >= outer.y0 - TOLERANCE
            && inner.x1 <= outer.x1 + TOLERANCE
            && inner.y1 <= outer.y1 + TOLERANCE
    }

    #[test]
    fn placeholder_paints_visible_text_within_its_slot_rect_at_every_slot_size() {
        // A range of slot sizes the app-facing builders actually allow,
        // including `native_switch`'s own deliberately small box
        // (`examples/glyph-catalog/src/pages/native_widgets.rs`) and an
        // even smaller one to stress the invariant further.
        for (label, (w, h)) in [
            ("native_switch's own box (70x40)", (70.0, 40.0)),
            ("native_progress's own box (260x24)", (260.0, 24.0)),
            ("native_button's own box (160x48)", (160.0, 48.0)),
            ("a deliberately tiny box (40x16)", (40.0, 16.0)),
        ] {
            let view: AnyView<()> = placeholder(Some((w, h)), "Switch");
            let mut element = build_any(view);

            let mut text_ctx = TextContext::new();
            let mut lctx = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
            let bc = BoxConstraints::tight(Size::new(w, h));
            let laid = element.layout(&mut lctx, &bc);
            assert_eq!(
                laid,
                Size::new(w, h),
                "{label}: the placeholder's own reported size must stay exactly the \
                 slot's declared size"
            );

            let mut rec = BoundsRecorder::default();
            let mut pctx = PaintCtx::new(Point::ZERO, laid);
            element.paint(&mut pctx, &mut rec);

            assert!(
                !rec.painted.is_empty(),
                "{label}: expected the placeholder to paint something"
            );
            // The visible half of the refusal contract: a sighted user must
            // see the warning wording, not just the warning fill — the
            // `Role::Alert` node alone is not a substitute.
            assert!(
                rec.glyph_runs >= 2,
                "{label}: expected the banner's label AND description to paint as glyph \
                 runs, saw {} — a fill-only banner leaves a sighted user with no warning",
                rec.glyph_runs
            );
            // ... and the clip wrapper must keep every one of those runs
            // (plus the fill/border) inside the slot at the same time.
            let slot_rect = Rect::from_origin_size(Point::ZERO, laid);
            for bounds in &rec.painted {
                assert!(
                    rect_fits_within(slot_rect, *bounds),
                    "{label}: painted bounds {bounds:?} escaped the slot rect {slot_rect:?}"
                );
            }
        }
    }

    /// The original refusal test must still pass unchanged after the clip wrapper
    /// — re-asserted here (mirrors `a_refused_slot_publishes_no_platform_view_frame`
    /// above) against the `BoundsRecorder`'s clip-aware scene too, so both
    /// scenes agree the refusal path never touches platform-view frames.
    #[test]
    fn a_refused_slot_still_publishes_no_platform_view_frame_through_the_clip_wrapper() {
        let view =
            native_switch(true).build_with_mode(2, ResolvedSurfaceMode::RefusedTranslucent, None);
        let mut element = build_any(view);
        let mut text_ctx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        element.layout(&mut lctx, &BoxConstraints::tight(Size::new(70.0, 40.0)));
        let mut rec = BoundsRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(70.0, 40.0));
        element.paint(&mut pctx, &mut rec);
        assert!(
            pctx.take_platform_views().is_empty(),
            "a refused slot must render no native platform_view frame, even through the \
             clip wrapper"
        );
    }
}
