//! The render root: the object each platform shell drives each frame.
//!
//! It owns the widget [`WidgetTree`] and the previous [`View`], and exposes the
//! three framework passes in Masonry order (the subset relevant to v0):
//!
//! * [`RenderRoot::rebuild`] — run `app_logic`, diff against the previous view,
//!   producing/mutating the retained widget.
//! * [`RenderRoot::layout`] — hand the root widget window-sized constraints and
//!   record the size it returns.
//! * [`RenderRoot::paint`] — emit the root widget's draw commands into a scene.
//!
//! v0 is single-root: `app_logic` returns one `impl View<State>` whose concrete
//! type is fixed, so the root's previous view and element are stored typed.
//! ViewSequence / multiple children are explicitly out of scope for now.

use std::any::Any;
use std::cell::Cell;
use std::num::NonZeroU64;

use kurbo::{Point, Rect, Size};

use crate::anim::FrameTime;
use crate::event::{
    CursorIcon, EventCtx, EventOutcome, EventResult, ImeState, InputEvent, PointerButton,
    PointerEvent, PointerPhase, clear_cursor_request, take_cursor_request,
};
use crate::insets::WindowInsets;
use crate::layout::BoxConstraints;
use crate::semantics::{ROOT_NODE_ID, SemanticsCtx, SemanticsUpdate};
use crate::tree::{InspectNode, WidgetPod, WidgetTree};
use crate::view::{BuildCtx, ChangeFlags, View, WidgetId};
use crate::widget::{LayoutCtx, PaintCtx, PaintOutcome, PaintScene, PlatformViewFrame};

/// The window's shape and platform-occlusion state, delivered to app code as a
/// plain [`provide_context`](reactive_graph::owner::provide_context)-carried
/// value — logical size, device-pixel scale, a
/// [derived](Orientation::from_size) orientation, and the current
/// [`WindowInsets`].
///
/// # Plain value, not a signal
///
/// `WindowMetrics` is delivered exactly like `Theme` and [`WindowInsets`]
/// already are: a shell calls `provide_context` with a freshly-built value on
/// change, and app code recovers it with `use_context::<WindowMetrics>()`
/// inside `Component::build`. It is **not** an `RwSignal` — only `deep_link`
/// and `back` are true signals in `frust-reactive`; every other host-signal
/// carrier (theme, insets, and now this) is a re-provided plain value.
///
/// # Alongside `WindowInsets`, not superseding it
///
/// `WindowInsets` already reaches `Component::build` on Android and iOS today
/// (each shell's `push_insets` calls `provide_context(insets)` independently
/// of anything here — desktop has no such arm for either value yet).
/// `WindowMetrics` is additive: a shell that starts providing it keeps
/// providing the standalone `WindowInsets` context too, so an existing
/// `use_context::<WindowInsets>()` call site never breaks. `insets` on this
/// type is a **copy** of that same value for convenience (a widget laying
/// itself out around window shape wants size/scale/orientation/insets
/// together), not a replacement for the independent context.
///
/// # Orientation is derived, not platform-sourced
///
/// No platform callback in either mobile shell carries an orientation enum —
/// Android's `nativeOnSurfaceChanged` and iOS's `frust_resize` each hand the
/// shell only a `(width, height, scale)` triple. [`Orientation`] is therefore
/// always computed from `size` via [`Orientation::from_size`]
/// (portrait when `height >= width`, so an exact square reads as portrait);
/// it never tracks a device orientation-lock setting or a platform rotation
/// event directly.
///
/// # Context is not reactive
///
/// `provide_context` is a plain insert into the owner's context map — it
/// notifies nothing — and `use_context` inside `Component::build` (or the
/// root `app_logic`) creates no subscription, so re-providing a changed
/// `WindowMetrics` does not itself mark anything dirty or wake a frame. A new
/// value becomes visible only on the next rebuild, which the resize or inset
/// change that produced it already drives; do not write a shell that assumes
/// a `provide_context` write triggers one. A shell wiring this up (see
/// `docs/SHELLS_ARCHITECTURE.md`) must still re-provide `WindowMetrics` only
/// on an actual change (mirroring `RenderRoot::set_insets`'s
/// `PartialEq`-guarded no-op) — the reason is cost at the FFI boundary (a
/// lock write plus an allocation every frame), not a rebuild storm.
/// Separately, there is no per-component rebuild skipping in this framework
/// (a component always re-runs `build` on any rebuild it does take part in),
/// which is affordable only because builds are cheap by construction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowMetrics {
    /// The window's logical (density-independent) size.
    pub size: Size,
    /// The device-pixel scale factor (logical → physical px multiplier).
    pub scale: f64,
    /// The orientation derived from `size` — see the type's doc for why this
    /// is computed, never platform-sourced.
    pub orientation: Orientation,
    /// A copy of the window's current insets — see the type's doc for why
    /// this does not replace the standalone `WindowInsets` context.
    pub insets: WindowInsets,
}

impl WindowMetrics {
    /// Construct a [`WindowMetrics`] from its transported fields, deriving
    /// [`orientation`](Self::orientation) from `size` rather than accepting it
    /// as an input — see the type's doc for why orientation is never
    /// platform-sourced.
    pub fn new(size: Size, scale: f64, insets: WindowInsets) -> Self {
        Self {
            size,
            scale,
            orientation: Orientation::from_size(size),
            insets,
        }
    }
}

/// A window's derived portrait/landscape orientation.
///
/// Always computed from a [`WindowMetrics::size`] via [`Orientation::from_size`]
/// — see [`WindowMetrics`]'s doc for why no platform callback carries this as
/// an enum directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    /// `size.height >= size.width`, including the exact-square case.
    Portrait,
    /// `size.height < size.width`.
    Landscape,
}

impl Orientation {
    /// Derives orientation from a logical window size: portrait when
    /// `height >= width` (an exact square reads as portrait), landscape
    /// otherwise.
    pub fn from_size(size: Size) -> Self {
        if size.height >= size.width {
            Orientation::Portrait
        } else {
            Orientation::Landscape
        }
    }
}

/// How many [`InputEvent::Housekeeping`] flush passes one
/// [`RenderRoot::rebuild`] will run before deferring the rest to the next frame.
///
/// A flushed pop-result callback may itself push or pop, queueing another
/// callback — so the flush/re-diff cycle has to be allowed to iterate, but it
/// must never be allowed to spin: a pair of callbacks that push each other would
/// otherwise hang the frame. Three passes covers every shape observed in
/// practice (a result that navigates once, and that page's own result), while
/// keeping the worst case at four `app_logic` runs per frame — `app_logic` is
/// cheap by construction (see [`RenderRoot::rebuild`]).
///
/// Past the cap the mark stays raised and one more frame is requested, so the
/// remaining work lands next frame instead of being lost.
const MAX_PENDING_RESULT_FLUSH_PASSES: usize = 3;

/// Change-guarded write of the shell-facing IME surface: replaces `slot` and
/// bumps `generation` **only** when the value actually moves ([`ImeState`] is
/// `PartialEq`).
///
/// A free function over the two fields rather than a `&mut self` method because
/// [`RenderRoot::paint`] writes it while holding disjoint borrows of the tree
/// and the theme, where no whole-`self` call is possible;
/// [`RenderRoot::store_ime_state`] is the method form the event pass uses, so
/// both paths share this one implementation.
///
/// The change guard is load-bearing, not an optimisation: the paint pass
/// re-publishes the focused widget's IME surface every frame, so an
/// unconditional bump would make the shell's `focus_or_ime_changed` edge fire on
/// every vsync for the whole life of a focus session — the level-input behavior
/// the generation exists to replace.
fn store_ime_state_in(slot: &mut Option<ImeState>, generation: &mut u64, next: Option<ImeState>) {
    if *slot != next {
        *slot = next;
        *generation = generation.wrapping_add(1);
    }
}

/// Release the whole focus/IME session: drop `focus_active` **and** the
/// shell-facing surface together, bumping `generation` **exactly once** if
/// either actually moved.
///
/// The paired form of [`RenderRoot::set_focus_active`]`(false)` +
/// [`RenderRoot::store_ime_state`]`(None)`, and the single primitive every
/// release site goes through — the blur-on-outside-tap `Down`, an explicit
/// [`EventCtx::release_focus`](crate::event::EventCtx::release_focus), a widget
/// publishing an *inactive* surface (see [`RenderRoot::paint`]), and the
/// generic-unmount orphan drain in [`RenderRoot::rebuild`]. Keeping them on one
/// primitive is what makes "a session ends" mean the same thing everywhere,
/// rather than four hand-assembled pairs that can drift apart.
///
/// **One release is one edge.** The two field writers bump on each field's own
/// change, so calling them in sequence would move
/// [`focus_ime_generation`](RenderRoot::focus_ime_generation) *twice* for the
/// ordinary release (focus `true`→`false` and surface `Some`→`None`). A shell
/// only ever compares the value, so two bumps and one bump raise the same single
/// `focus_or_ime_changed` edge — but a counter that moves once per observable
/// transition is the contract the field doc states, and is what the release
/// tests pin. The change guard itself is unchanged: an already-released root
/// writes the same values back and moves nothing.
///
/// A free function over the three fields (not a `&mut self` method) for the same
/// reason [`store_ime_state_in`] is: [`RenderRoot::paint`] releases while holding
/// disjoint borrows of the tree and the theme, where no whole-`self` call is
/// possible. [`RenderRoot::release_focus_session`] is the method form.
fn release_focus_session_in(
    focus_active: &mut bool,
    ime_slot: &mut Option<ImeState>,
    generation: &mut u64,
) {
    let moved = *focus_active || ime_slot.is_some();
    *focus_active = false;
    *ime_slot = None;
    if moved {
        *generation = generation.wrapping_add(1);
    }
}

/// Owns the retained tree and drives the rebuild/layout/paint passes for a
/// single-root application.
///
/// Generic over the application `State` and the concrete root view type `V`
/// returned by `app_logic`.
pub struct RenderRoot<State: 'static, V: View<State>> {
    tree: WidgetTree,
    root_id: Option<WidgetId>,
    /// The previous view, retained to diff against on the next rebuild.
    prev_view: Option<V>,
    /// Monotonic widget-id counter, borrowed by each `BuildCtx`.
    next_id: u64,
    window_size: Size,
    /// Whether a pointer is currently down-and-captured somewhere in the tree.
    /// Set on a `Down` whose dispatch requested capture, cleared on `Up`/`Cancel`.
    /// Root-level mirror of the per-container `active` path bookkeeping.
    pointer_captured: bool,
    /// Whether some widget in the tree currently holds focus. Root-level mirror of
    /// the per-container `focused` path bookkeeping (the focus analog of
    /// `pointer_captured`): set when a dispatch requested focus, cleared by a
    /// session release — a blur-on-outside-tap `Down`, an explicit focus release,
    /// a widget publishing an inactive IME surface, or the generic-unmount orphan
    /// drain in [`RenderRoot::rebuild`] (see [`release_focus_session_in`]).
    focus_active: bool,
    /// Whether the last completed hover pass left some widget in the tree holding
    /// the hover link. Root-level mirror of the per-pod hover stamp (the hover
    /// analog of `focus_active`), seeded into every event/paint pass so nothing
    /// below can read as hovered while the root says nothing is.
    hover_active: bool,
    /// The live hover epoch: the identity of the most recent completed hover pass.
    ///
    /// Advanced by exactly one per hover pass — an **uncaptured**
    /// [`PointerPhase::Move`] (which may record a claim), or the `Down`/`Cancel`
    /// that ends a hover outright (which may not) — and by nothing else, so a
    /// scroll, key, IME, or housekeeping pass leaves a live hover standing.
    /// A [`crate::widget::ChildPod`]'s recorded stamp counts as hovered only while
    /// it equals this, which is what strands the previous claimant's path with no
    /// container having to clear it (see the [`crate::event`] module docs).
    ///
    /// Starts at `1`, not `0`: a freshly built pod's stamp is `0`, and starting the
    /// epoch past it means a never-claimed pod cannot match the live epoch by
    /// accident before the first hover pass ever runs.
    hover_epoch: u64,
    /// The cursor the last cursor pass resolved — hover's sibling channel, and
    /// the value a desktop shell reads through [`RenderRoot::cursor`].
    ///
    /// Deliberately **not** derived from `hover_active`: that mirror is
    /// identity-free (it knows *that* something is hovered, not which widget or
    /// what shape it wants), so a request travels its own pass-scoped slot
    /// ([`EventCtx::set_cursor`]) and is resolved here.
    ///
    /// Re-resolved on every pointer [`PointerPhase::Move`], captured or not:
    /// whatever the pass requested, or [`CursorIcon::Default`] when it requested
    /// nothing. Every other pass leaves it standing — see [`RenderRoot::event`]
    /// for why a `Down`/`Up` must not reset it. There is no generation counter
    /// beside it: the shell compares the value it last applied (see
    /// [`RenderRoot::cursor`]).
    cursor: CursorIcon,
    /// The IME surface the focused widget last published (via
    /// [`EventCtx::publish_ime_state`]), surfaced to the shell by
    /// [`RenderRoot::ime_state`]. Persists across rebuilds/events until refreshed
    /// by a new publish or dropped by a release (a blur, a focus release, an
    /// inactive publish, or a generic-unmount orphan drain — see
    /// [`release_focus_session_in`]).
    ///
    /// Only ever `None` or an **active** surface: an inactive publish is a
    /// release, never a stored value (see [`RenderRoot::ime_state`]).
    ime_state: Option<ImeState>,
    /// A monotonically-increasing generation bumped on every **actual** change
    /// of `focus_active` or `ime_state` — the focus/IME session's edge signal,
    /// read by a shell through [`RenderRoot::focus_ime_generation`].
    ///
    /// The mobile frame gate turns this into an *edge* input
    /// (`FrameInputs::focus_or_ime_changed`): a shell caches the last value it
    /// saw and runs a frame when it moves. A *level* input ("something holds
    /// focus") forced a frame every vsync for as long as a field stayed focused,
    /// which made caret pacing unreachable — measured at 62–120 fps on a static
    /// screen whose only live input was focus (Xiaomi 12).
    ///
    /// Same-value writes deliberately do **not** bump it (see
    /// [`RenderRoot::set_focus_active`]/[`RenderRoot::store_ime_state`]): the
    /// paint pass republishes the focused widget's IME surface on *every* frame,
    /// so bumping on write rather than on change would re-create exactly the
    /// per-vsync forcing this edge exists to remove.
    focus_ime_gen: u64,
    /// The [`PlatformViewFrame`]s the tree published during the most recent
    /// [`RenderRoot::paint`], surfaced to the shell via
    /// [`RenderRoot::platform_view_frames`]. Unlike `ime_state` above, this is
    /// REPLACED wholesale every pass (never merged with the previous one), so
    /// a pass that publishes none yields an empty `Vec` — a culled/removed
    /// slot from the prior frame does not linger as a stale frame. Core stays
    /// dumb here: the shell's differ owns absent-means-hide/dispose semantics.
    platform_view_frames: Vec<PlatformViewFrame>,
    /// The z-shield rects the tree reported during the most recent
    /// [`RenderRoot::paint`] (via [`crate::widget::PaintCtx::report_input_shield`]),
    /// surfaced to the shell via [`RenderRoot::input_shields`].
    ///
    /// Exactly the `platform_view_frames` discipline above — REPLACED wholesale
    /// every pass, so a pass whose shields stopped painting reports none. Core
    /// stays dumb: it never associates a shield with a slot, that is the
    /// shell-side differ's job.
    input_shields: Vec<Rect>,
    /// Dirtiness accumulated since the last [`RenderRoot::take_change_flags`] —
    /// merged from each rebuild so a shell can decide, in one place, whether a
    /// frame needs layout/paint at all.
    pending: ChangeFlags,
    /// The app's active theme, stored type-erased so `frust-core` needs no
    /// `frust-theme` dependency (the concrete `Theme` is boxed by the shell —
    /// see [`RenderRoot::set_theme`]). Lent as `Option<&dyn Any>` into each
    /// [`LayoutCtx`]/[`PaintCtx`]; `None` until a shell sets one (a supported
    /// state — bare-core tests and pre-theme apps run without a theme).
    theme: Option<Box<dyn Any>>,
    /// The window's insets ([`WindowInsets`]), delivered by the shell via
    /// [`RenderRoot::set_insets`] and threaded into every subsequent
    /// layout/paint pass (recovered by widgets through
    /// [`crate::widget::LayoutCtx::window_insets`]/
    /// [`crate::widget::PaintCtx::window_insets`]). Unlike the theme this is a
    /// concrete core-owned type (only `f64` scalars), stored by value — no
    /// `Box<dyn Any>` erasure needed. Defaults to the zero inset until a shell
    /// pushes one (a supported state — bare-core tests and pre-insets apps).
    insets: WindowInsets,
    /// The shell's running count of frames the render thread has actually
    /// presented, threaded into every subsequent paint pass and recovered by
    /// widgets through [`crate::widget::PaintCtx::presented_frames`]. A plain
    /// `u64` core stores by value (like the insets). `None` until a shell pushes
    /// one via [`RenderRoot::set_presented_frames`] — a supported state
    /// (bare-core tests and pre-wiring shells run without it), so widgets can
    /// fall back to a paint-cadence measure. Unlike the theme/insets this is a
    /// pure observation: [`RenderRoot::set_presented_frames`] deliberately marks
    /// NO [`ChangeFlags`] and bumps NO semantics generation (see its doc), so a
    /// ticking presented count never forces a relayout or feeds the mobile frame
    /// gate.
    presented_frames: Option<u64>,
    /// Whether the shell created a translucent (alpha-channel, "Mode B") GPU
    /// surface, threaded into every subsequent paint pass and recovered by
    /// widgets through [`crate::widget::PaintCtx::is_translucent`]. A plain
    /// `bool` core stores by value (like the insets); `false` (opaque, "Mode A")
    /// until a shell pushes one via [`RenderRoot::set_surface_translucent`] — the
    /// supported default for every desktop app and bare-core test. The
    /// platform-view hole-punch is the sole reader: a slot clears its rect only
    /// on a translucent surface (see `frust-widgets`' `PlatformViewWidget`).
    surface_translucent: bool,
    /// The persistent, never-reused per-pod semantics base-id allocator's next
    /// value. Seeded at `2` (ids `0`/`1` reserved: `0` keeps
    /// `NonZeroU64` valid, `1` is the [`ROOT_NODE_ID`] window node), advanced as
    /// [`ChildPod`](crate::widget::ChildPod)s are assigned bases on their first
    /// semantics visit, and carried across passes so a pod that first appears on a
    /// later frame never collides with an already-assigned one. A `Cell` because
    /// [`RenderRoot::semantics`] runs behind `&self`.
    semantics_alloc: Cell<u64>,
    /// The root widget's stable semantics base id (the root pod is arena-backed,
    /// not a [`ChildPod`](crate::widget::ChildPod), so it caches its base here
    /// rather than in a pod). Lazily assigned on the first semantics pass.
    root_semantics_id: Cell<Option<NonZeroU64>>,
    /// A monotonically-increasing generation bumped whenever a rebuild or theme
    /// swap could have changed the semantics tree, so a shell can cheaply skip
    /// re-pulling + re-pushing an unchanged accessibility tree (the semantics
    /// dirty gate — see [`RenderRoot::semantics_if_changed`]). v1 recompute is
    /// acceptable; this is the seam a shell gates on.
    semantics_gen: u64,
    /// Set by [`RenderRoot::rebuild`] when the deferred-callback flush owes the
    /// shell a frame, and folded into the next [`RenderRoot::paint`]'s
    /// [`PaintOutcome::needs_frame`] (then cleared). Two raisers, both in the
    /// flush loop: hitting [`MAX_PENDING_RESULT_FLUSH_PASSES`] with work still
    /// owed, and a dispatched [`InputEvent::Housekeeping`] whose
    /// [`EventOutcome::needs_redraw`] came back set.
    ///
    /// The frame-request half of the deferral: `pending |= PAINT` already tells
    /// the mobile frame gate to run its next tick, but the desktop loop is
    /// dirty-driven (`ControlFlow::Wait`) and schedules off `needs_frame`, so the
    /// deferral has to surface there too — otherwise the remaining flush would
    /// wait for whatever input happens to arrive next, which is the exact
    /// failure this whole mechanism exists to remove.
    deferred_frame: bool,
    _state: core::marker::PhantomData<fn(&mut State)>,
}

impl<State: 'static, V: View<State>> RenderRoot<State, V> {
    /// Create an empty render root with no widget yet built.
    pub fn new() -> Self {
        Self {
            tree: WidgetTree::new(),
            root_id: None,
            prev_view: None,
            next_id: 0,
            window_size: Size::ZERO,
            pointer_captured: false,
            focus_active: false,
            hover_active: false,
            // Past a fresh pod's `0` stamp — see the field doc.
            hover_epoch: 1,
            cursor: CursorIcon::Default,
            ime_state: None,
            focus_ime_gen: 0,
            platform_view_frames: Vec::new(),
            input_shields: Vec::new(),
            pending: ChangeFlags::NONE,
            theme: None,
            insets: WindowInsets::default(),
            presented_frames: None,
            surface_translucent: false,
            // Ids 0 and 1 are reserved (see the field doc); pods start at 2.
            semantics_alloc: Cell::new(2),
            root_semantics_id: Cell::new(None),
            semantics_gen: 0,
            deferred_frame: false,
            _state: core::marker::PhantomData,
        }
    }

    /// Store the app's active theme, threaded into every subsequent
    /// layout/paint pass as `Option<&dyn Any>` and recovered by widgets via
    /// [`crate::widget::PaintCtx::theme_as`]/[`crate::widget::LayoutCtx::theme_as`].
    ///
    /// The theme is boxed **type-erased** (`Box<dyn Any>`) so this crate stays
    /// independent of `frust-theme`; the shell boxes the concrete `Theme`
    /// (and re-boxes it on a live appearance change, e.g. dark-mode toggle).
    /// Calling again replaces the stored theme.
    ///
    /// Marks `LAYOUT | PAINT` pending (drained by
    /// [`RenderRoot::take_change_flags`]): a theme swap can change baked-in
    /// paint state a widget resolves at layout time (e.g. `Text`'s themed
    /// glyph color, cached into its `TextLayout` — see
    /// `frust-widgets::text`), so a shell that later gates layout/paint on
    /// this seam must still see a bare `set_theme` as dirty even though no
    /// view changed.
    pub fn set_theme(&mut self, theme: Box<dyn Any>) {
        self.theme = Some(theme);
        self.pending |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        // A theme swap can change semantics-visible state (e.g. a relabelled or
        // re-bounded node once layout re-runs); treat it as semantics-dirty too.
        self.semantics_gen = self.semantics_gen.wrapping_add(1);
    }

    /// Store the window's insets ([`WindowInsets`]), threaded into every
    /// subsequent layout/paint pass and recovered by widgets via
    /// [`crate::widget::LayoutCtx::window_insets`]/
    /// [`crate::widget::PaintCtx::window_insets`].
    ///
    /// Mirrors [`RenderRoot::set_theme`]'s dirty-tracking contract: a change
    /// marks `LAYOUT | PAINT` pending (drained by
    /// [`RenderRoot::take_change_flags`]) so a shell gating layout/paint on that
    /// seam still relayouts when the insets move — a `SafeArea` widget resolves
    /// its inset at layout time, so the mobile layout-skip gate must see a bare
    /// `set_insets` as dirty even though no view changed (the same reasoning as
    /// the theme swap — see `docs/ARCHITECTURE.md`'s Theme delivery and Frame
    /// gate). A change also bumps the semantics generation, since a moved inset
    /// shifts laid-out node bounds.
    ///
    /// No-op guarded by [`WindowInsets`]'s `PartialEq`: pushing the current
    /// value marks nothing dirty, so a shell that polls the platform insets
    /// every frame and forwards unconditionally never forces a needless
    /// relayout. (A shell may also skip the call itself by comparing first —
    /// this is the same guard, held on the core side.)
    pub fn set_insets(&mut self, insets: WindowInsets) {
        if self.insets == insets {
            return;
        }
        self.insets = insets;
        self.pending |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        // A moved inset shifts laid-out node bounds once layout re-runs; treat
        // it as semantics-dirty too (mirrors `set_theme`).
        self.semantics_gen = self.semantics_gen.wrapping_add(1);
    }

    /// The window's insets currently threaded into the layout/paint passes.
    pub fn insets(&self) -> WindowInsets {
        self.insets
    }

    /// Store the shell's running count of frames the render thread has actually
    /// presented, threaded into every subsequent paint pass and recovered by
    /// widgets via [`crate::widget::PaintCtx::presented_frames`]. A shell loads
    /// the atomic its render side increments (once per presented frame) and
    /// pushes it here once per UI frame, before `paint`.
    ///
    /// **Deliberately dirties nothing.** Unlike [`RenderRoot::set_theme`] and
    /// [`RenderRoot::set_insets`] — which mark `LAYOUT | PAINT` pending because a
    /// widget bakes their value in at layout time — this setter marks NO
    /// [`ChangeFlags`] and bumps NO semantics generation. The presented count is
    /// a paint-only *observation* a widget reads live every paint (never baked at
    /// layout), so treating it as dirty would be wrong twice over: it would force
    /// a needless relayout, and — critically — on the mobile shells a
    /// monotonically ticking counter would keep the frame gate's pending-flags
    /// input perpetually true, so the menu would never idle (the 32s-idle
    /// behavior must survive). Keeping this setter dirt-free
    /// is exactly what keeps the frame gate unaware of it (see
    /// `docs/ARCHITECTURE.md`'s Frame gate).
    pub fn set_presented_frames(&mut self, presented: u64) {
        self.presented_frames = Some(presented);
    }

    /// The presented-frame count currently threaded into the paint pass, or
    /// `None` if no shell has pushed one.
    pub fn presented_frames(&self) -> Option<u64> {
        self.presented_frames
    }

    /// Store whether the shell's GPU surface is translucent (alpha-channel,
    /// "Mode B"), threaded into every subsequent paint pass and recovered by
    /// widgets through [`crate::widget::PaintCtx::is_translucent`]. A shell
    /// pushes the surface's **resolved** translucency here — what the GPU
    /// backend reports after the surface is installed, not what the app
    /// requested via `frust-shell-common::surface_mode`'s latch: a translucency
    /// request the platform refuses must degrade to the opaque contract, or
    /// every `platform_view` slot punches a hole in an opaque swapchain
    /// (black rectangles). Every desktop app leaves the default `false`
    /// (opaque, "Mode A").
    ///
    /// Marks `PAINT` pending on an actual change (`PartialEq`-guarded, mirroring
    /// [`RenderRoot::set_insets`]'s no-op guard): translucency is read purely at
    /// paint time (the hole-punch runs in `paint`, never baked at layout), so a
    /// flip must repaint but need not relayout. A flip is rare but **real**: a
    /// surface (re)install can resolve differently from the previous one, and
    /// both mobile shells re-push this every frame (the no-op-if-unchanged
    /// guard is what makes that free).
    pub fn set_surface_translucent(&mut self, translucent: bool) {
        if self.surface_translucent == translucent {
            return;
        }
        self.surface_translucent = translucent;
        self.pending |= ChangeFlags::PAINT;
    }

    /// Whether the shell's GPU surface is currently marked translucent.
    pub fn is_surface_translucent(&self) -> bool {
        self.surface_translucent
    }

    /// Whether a captured pointer gesture is currently in flight.
    pub fn is_pointer_captured(&self) -> bool {
        self.pointer_captured
    }

    /// Whether some widget in the tree currently holds keyboard/IME focus.
    pub fn is_focus_active(&self) -> bool {
        self.focus_active
    }

    /// Whether some widget in the tree currently holds the hover link — i.e.
    /// whether the last hover pass (an uncaptured [`PointerPhase::Move`]) left the
    /// pointer over a widget that claimed it.
    ///
    /// The hover analog of [`RenderRoot::is_focus_active`], and a level accessor
    /// like it: hover is not a session (nothing has to be released), so there is no
    /// generation counterpart. `false` on a touch-only app and on any app whose
    /// widgets never call [`EventCtx::claim_hover`](crate::event::EventCtx::claim_hover).
    pub fn is_hover_active(&self) -> bool {
        self.hover_active
    }

    /// The cursor the tree last asked the host to show — what a desktop shell
    /// pushes to its window (`frust-shell-desktop` maps it onto winit's own
    /// cursor icons).
    ///
    /// A **level** accessor like [`RenderRoot::is_hover_active`], not an edge one:
    /// the value re-resolves on every pointer [`PointerPhase::Move`] and stands
    /// unchanged through every other pass, so a shell caches what it last applied
    /// and calls the platform only when this differs. There is deliberately no
    /// generation counter — a cursor is a *value*, not a session, and an unmoved
    /// cursor is indistinguishable from one re-resolved to the same shape.
    ///
    /// [`CursorIcon::Default`] before the first `Move`, and after any `Move` in
    /// which no widget called
    /// [`EventCtx::set_cursor`](crate::event::EventCtx::set_cursor) — so a
    /// touch-only app, and any app whose widgets never request a cursor, reads
    /// `Default` forever and the mobile shells simply never read this at all.
    ///
    /// **Residual:** a widget that is torn down (or moves out from under a
    /// stationary pointer) while its request stands leaves the last shape in
    /// place until the next `Move` re-resolves it — the same self-correction
    /// window hover has, and for the same reason: nothing re-resolves without
    /// pointer motion.
    pub fn cursor(&self) -> CursorIcon {
        self.cursor
    }

    /// The IME surface the focused widget published, for the shell to drive the
    /// platform input method (winit `set_ime_cursor_area`, Android
    /// `updateSelection`, iOS `inputDelegate`). `None` when nothing is focused or
    /// the focused widget publishes no IME surface.
    ///
    /// Written by the focused widget through [`EventCtx::publish_ime_state`] during
    /// the event pass and refreshed on every event; it survives a rebuild (so the
    /// shell can query it between frames) and is cleared when focus is lost.
    ///
    /// # `None` is the only "no session" form — an inactive surface is never stored
    ///
    /// A widget publishing `ImeState { active: false, .. }` is ending the session,
    /// not describing it, so both publish paths turn that into a full release
    /// (see [`RenderRoot::paint`]) and this returns `None` rather than
    /// `Some(inactive)`. A shell therefore never has to distinguish the two, and
    /// `is_some()` means "a live IME session" with no second check.
    ///
    /// **The platform still sees the keyboard-hide.** All three shells already
    /// map `None` onto the inactive form on the way out, so the observable wire
    /// behavior is unchanged: `frust-shell-android`'s `ime_state_to_json` returns
    /// `ImeJsonState::default()` (`active:false`, empty text, `-1` indices, null
    /// caret, `"normal"`) and `frust-shell-ios`' returns the byte-identical
    /// `ime_state_json(false, "", -1, -1, -1, -1, None, "normal")` — exactly what
    /// the navigator's own cleared surface serialised to before. Kotlin's
    /// `pollImeAfterDispatch` and Swift's `syncImeFocus` both branch on `active`
    /// alone (an inactive surface's text/caret/content-type are ignored), and the
    /// desktop shell's `sync_ime` reads `is_some_and(|s| s.active)`. Dropping the
    /// inactive surface's payload also stops a disabled *secret* field's text
    /// riding to the platform after its session ended.
    pub fn ime_state(&self) -> Option<ImeState> {
        self.ime_state.clone()
    }

    /// The focus/IME session generation — bumped on every **actual** change of
    /// [`is_focus_active`](RenderRoot::is_focus_active) or
    /// [`ime_state`](RenderRoot::ime_state), and on nothing else.
    ///
    /// The *edge* counterpart of those two level accessors, for a shell that
    /// needs "did the focus/IME session move since I last looked?" rather than
    /// "is something focused?". A shell caches the value it last saw and
    /// compares (mirroring [`semantics_generation`](RenderRoot::semantics_generation)'s
    /// cheap dirty gate) — that comparison is the mobile frame gate's
    /// `FrameInputs::focus_or_ime_changed` input.
    ///
    /// A same-value write never moves it: re-publishing an identical IME
    /// surface (which the paint pass does on every frame a field stays focused)
    /// or re-blurring an already-blurred root is not an edge. Wrapping is
    /// deliberate and harmless — a comparison, never an ordering.
    pub fn focus_ime_generation(&self) -> u64 {
        self.focus_ime_gen
    }

    /// Set the root's focus flag, bumping [`RenderRoot::focus_ime_generation`]
    /// only when the value actually moves.
    ///
    /// One of the two writers of `focus_active` outside construction (the other
    /// is [`release_focus_session_in`], which clears it together with the IME
    /// surface as one edge): every focus/blur arm of [`RenderRoot::event`] goes
    /// through one of them, so the edge generation cannot drift from the state it
    /// describes. In practice this one only ever *sets* focus — a clear is always
    /// a session release.
    fn set_focus_active(&mut self, active: bool) {
        if self.focus_active != active {
            self.focus_active = active;
            self.focus_ime_gen = self.focus_ime_gen.wrapping_add(1);
        }
    }

    /// Store (or clear) the shell-facing IME surface, bumping
    /// [`RenderRoot::focus_ime_generation`] only when the stored value actually
    /// moves — the `&mut self` form of [`store_ime_state_in`], for the event
    /// pass (the paint pass holds disjoint field borrows and calls that
    /// function directly).
    fn store_ime_state(&mut self, ime: Option<ImeState>) {
        store_ime_state_in(&mut self.ime_state, &mut self.focus_ime_gen, ime);
    }

    /// End the focus/IME session: clear `focus_active` and drop the shell-facing
    /// surface together, moving [`RenderRoot::focus_ime_generation`] exactly once
    /// if either was set. The `&mut self` form of [`release_focus_session_in`]
    /// (whose doc carries the full contract), for the event and rebuild passes;
    /// the paint pass holds disjoint field borrows and calls that function
    /// directly.
    ///
    /// Idempotent: releasing an already-released root writes the same values
    /// back and fires no edge.
    fn release_focus_session(&mut self) {
        release_focus_session_in(
            &mut self.focus_active,
            &mut self.ime_state,
            &mut self.focus_ime_gen,
        );
    }

    /// The [`PlatformViewFrame`]s published during the most recent
    /// [`RenderRoot::paint`], in paint order.
    ///
    /// Replaced wholesale every pass (see the `platform_view_frames` field
    /// doc), so a pass with no publishers yields an empty slice — a shell
    /// never sees a stale frame for a slot that stopped painting.
    pub fn platform_view_frames(&self) -> &[PlatformViewFrame] {
        &self.platform_view_frames
    }

    /// The z-shield rects reported during the most recent [`RenderRoot::paint`]
    /// (see [`crate::widget::PaintCtx::report_input_shield`]), in paint order.
    ///
    /// Replaced wholesale every pass, exactly like
    /// [`RenderRoot::platform_view_frames`] — a shell feeds both into the same
    /// differ ingest call, and the differ intersects these against each
    /// interactive slot's rect.
    pub fn input_shields(&self) -> &[Rect] {
        &self.input_shields
    }

    /// Drain the slot ids whose `platform_view` widgets were torn down since the
    /// last call (`View::teardown` ran on them — see
    /// [`crate::widget::report_retired_slot`]).
    ///
    /// The prompt-teardown channel: a shell calls this once per frame, right
    /// after its rebuild, and retires each id in its platform-view differ
    /// (`PlatformViewState::retire`) so a disposed slot's native view goes away
    /// immediately instead of waiting out the differ's missing-streak
    /// heuristic. Draining is destructive, mirroring
    /// [`RenderRoot::take_change_flags`]: an id is reported exactly once, so a
    /// shell that drains and drops the result loses the prompt path (the
    /// missing-streak backstop still covers it).
    ///
    /// A merely *culled* slot (scrolled offscreen, a parent skipping paint)
    /// never appears here — culling doesn't run `teardown` — which is what
    /// keeps the camera keep-alive contract intact.
    pub fn take_retired_platform_views(&mut self) -> Vec<u64> {
        crate::widget::take_retired_slots()
    }

    /// Take (and clear) the dirtiness accumulated since the last call.
    ///
    /// A shell can consult this to skip the layout/paint passes when nothing has
    /// changed and no redraw was requested (a desktop optimisation; the mobile
    /// continuous-loop shells may ignore it and repaint every tick). Each
    /// [`RenderRoot::rebuild`] merges its result here; this drains it.
    pub fn take_change_flags(&mut self) -> ChangeFlags {
        let flags = self.pending;
        self.pending = ChangeFlags::NONE;
        flags
    }

    /// Non-draining peek at the dirtiness accumulated since the last
    /// [`RenderRoot::take_change_flags`] — `true` when any `LAYOUT`/`PAINT`
    /// bit is pending, without clearing it.
    ///
    /// Complements [`take_change_flags`](RenderRoot::take_change_flags) for a
    /// shell frame gate: the gate reads this as one of its
    /// "should this frame run" inputs *before* deciding, so a frame it chooses
    /// to skip leaves `pending` intact for the next non-skipped frame to drain
    /// and act on. Draining stays the job of `take_change_flags`, called only
    /// on a frame that actually runs its layout/paint passes. No behavioral
    /// change to rebuild/layout/paint.
    pub fn has_pending_change_flags(&self) -> bool {
        !self.pending.is_empty()
    }

    /// The root widget id, once built.
    pub fn root_id(&self) -> Option<WidgetId> {
        self.root_id
    }

    /// Shared access to the retained tree (for the shell / tests).
    pub fn tree(&self) -> &WidgetTree {
        &self.tree
    }

    /// A read-only, pre-order snapshot of the retained tree for tooling: per
    /// node an id, its parent and children, the concrete widget's type name, an
    /// optional debug label, and its absolute border box in logical px.
    ///
    /// Computed on demand in O(nodes) and takes `&self` — no per-frame
    /// bookkeeping, no mutation, and nothing here participates in
    /// build/layout/paint. Bounds reflect the **last layout pass**, so call it
    /// after one (before the first, every rect is zero-sized).
    ///
    /// Scope: the walk covers the [`WidgetTree`] arena *and* the
    /// [`ChildPod`](crate::widget::ChildPod)s containers own, reached through
    /// [`Widget::visit_children`](crate::widget::Widget::visit_children) — so it
    /// is the real retained hierarchy, not just the arena (which holds little
    /// more than the root pod). A container that leaves that seam defaulted
    /// reads as a leaf.
    pub fn inspect(&self) -> Vec<InspectNode> {
        self.tree.inspect()
    }

    /// Run `app_logic`, then build (first call) or rebuild (subsequent calls)
    /// the root widget, returning what changed.
    ///
    /// `app_logic` is expected to be cheap and re-entrant: it is
    /// re-run in full every rebuild.
    ///
    /// # Deferred-callback flush
    ///
    /// The view diff itself is state-free (`rebuild_view` below takes no
    /// `State`), so a widget applying a structural op there — the navigator
    /// draining its queued `push`/`pop` is the shipped case — cannot run an app
    /// callback that needs `&mut State`. It instead queues the callback and calls
    /// [`mark_pending_result_flush`](crate::event::mark_pending_result_flush);
    /// this method drains that flag and dispatches an
    /// [`InputEvent::Housekeeping`] broadcast through the ordinary
    /// [`event`](RenderRoot::event) plumbing, where `state` *is* in scope. This
    /// is the only unconditional per-frame pass that holds `&mut State`, which is
    /// why the dispatch lives here and not in a shell (flushing on the next
    /// real input meant waiting seconds for a touch, or forever when the next
    /// touch went to chrome outside the navigator).
    ///
    /// A flushed callback mutates `State`, so the view built before it ran is
    /// stale — the `app_logic` + `rebuild_view` cycle therefore re-runs after
    /// each flush, and the same frame shows the result. Results can queue further
    /// nav ops, so the loop is **bounded**; past the cap the flag is left standing
    /// and one more frame is requested rather than spinning (see
    /// `MAX_PENDING_RESULT_FLUSH_PASSES`, this module's private cap constant).
    ///
    /// The broadcast's [`EventOutcome`] is propagated, not discarded: a
    /// `needs_redraw` coming back from the dispatch folds into this rebuild's
    /// [`ChangeFlags::PAINT`] and the deferred frame request, so a callback
    /// whose only effect is [`EventCtx::request_redraw`]
    /// — invisible to the re-diff, since no view-visible state changed — still
    /// wakes both the mobile frame gate and the desktop `Wait` loop.
    pub fn rebuild(
        &mut self,
        app_logic: &mut impl FnMut(&mut State) -> V,
        state: &mut State,
    ) -> ChangeFlags {
        let view = app_logic(state);
        let mut flags = self.rebuild_view(view);

        // Deferred-callback convergence loop (see the method doc). Each pass:
        // drain the flag, run the queued callbacks against real state, then
        // re-diff so this frame reflects them.
        let mut passes = 0usize;
        while crate::event::take_pending_result_flush() {
            if passes >= MAX_PENDING_RESULT_FLUSH_PASSES {
                // Cap reached. Put the flag back — the work is still owed — and
                // ask for one more frame instead of spinning inside this one.
                // `pending |= PAINT` is what the mobile frame gate reads
                // (`has_pending_change_flags`); `deferred_frame` is what surfaces
                // on the next `paint` as `needs_frame`, which is how the desktop
                // `ControlFlow::Wait` loop learns to wake.
                crate::event::mark_pending_result_flush();
                flags |= ChangeFlags::PAINT;
                self.deferred_frame = true;
                break;
            }
            // The dispatch's own outcome is load-bearing, not noise: a flushed
            // callback whose *only* effect is `EventCtx::request_redraw` (no
            // signal write, no state the next `app_logic` run reads) leaves the
            // re-diff below reporting `ChangeFlags::NONE`, so nothing else in
            // this method would ever mark the frame dirty and the requested
            // redraw would be dropped on the floor. Fold it into exactly the
            // wake the cap branch above raises: `PAINT` reaches `self.pending`,
            // which is what the mobile frame gate reads
            // (`has_pending_change_flags`), and `deferred_frame` surfaces on the
            // next `paint` as `needs_frame`, which is how the desktop
            // `ControlFlow::Wait` loop learns to schedule a frame. Both
            // Housekeeping producers need it (a navigator pop-result callback
            // and `frust-widgets`' gesture long-press latch), and without it a
            // redraw-only effect waits for whatever input happens to arrive
            // next — exactly the failure this mechanism exists to remove.
            //
            // Non-empty flags also bump the semantics generation below, which
            // is correct: the callback just mutated real `State` through a live
            // `EventCtx`, so the accessibility tree may genuinely have changed,
            // and every other paint-class path here bumps it the same way (a
            // spurious bump costs one recompute of an unchanged tree, a missed
            // one strands a stale tree).
            let outcome = self.event(state, &InputEvent::Housekeeping);
            if outcome.needs_redraw {
                flags |= ChangeFlags::PAINT;
                self.deferred_frame = true;
            }
            let view = app_logic(state);
            flags |= self.rebuild_view(view);
            passes += 1;
        }

        // Generic-unmount focus release. A reconciler that tears down (or
        // type-swaps, or clears the `focused` flag of) a child pod holding the
        // recorded focus path *on the live focus chain* has severed that path,
        // but runs over a `BuildCtx` with no `RenderRoot` in scope — so it raises
        // `mark_focus_orphaned` and this drain performs the release the
        // reconciler could not. "On the live chain" is what `rebuild_view`'s seed
        // buys: a mark means a live session lost its owner, never that some stale
        // flag deep in an already-blurred branch went away (see
        // `mark_focus_orphaned`). Without it the root's mirror stays standing over
        // a widget that no longer exists: `is_focus_active()` keeps reporting
        // true and `ime_state()` keeps handing the shell a surface for a dead
        // field, self-correcting only on the next event pass — which never
        // arrives on a screen the user has stopped touching (the pop-into-idle
        // case this whole seam exists for).
        //
        // Drained *after* the flush loop so one release covers every pass: a
        // flushed callback that navigates re-diffs, and either diff may orphan
        // the focus. `Housekeeping` claims no focus of its own (its root arm is
        // inert), so nothing the loop dispatched can be undone here.
        //
        // The release marks no `ChangeFlags` of its own: the structural change
        // that severed the path already flagged `LAYOUT | PAINT`, and the
        // generation bump is what wakes the mobile frame gate's
        // `focus_or_ime_changed` edge for the one repaint the release needs.
        if crate::event::take_focus_orphaned() {
            self.release_focus_session();
        }

        self.pending |= flags;
        // A rebuild that changed layout/paint could have changed the semantics
        // tree (added/removed/relabelled nodes); bump the dirty gate a shell polls
        // via `semantics_if_changed`.
        if !flags.is_empty() {
            self.semantics_gen = self.semantics_gen.wrapping_add(1);
        }
        flags
    }

    /// The rebuild body, split out so [`RenderRoot::rebuild`] can accumulate the
    /// result into [`RenderRoot::pending`] in one place.
    ///
    /// # Seeding the diff's focus chain
    ///
    /// The root is where the effective focus chain ([`BuildCtx::has_focus`])
    /// starts: the root widget sits in no `ChildPod`, so its "link above" is the
    /// root's own session mirror. A reconciler deep in the diff ANDs its pod's
    /// `focused` flag onto this seed and marks an orphan only if the whole chain
    /// holds — which is why the seed is "is there a session to lose" rather than
    /// `focus_active` alone: an active surface parked without the flag is still a
    /// live session `release_focus_session` would move. With neither set there is
    /// nothing to release, so the seed is `false` and the diff marks nothing.
    fn rebuild_view(&mut self, view: V) -> ChangeFlags {
        // Read before the `&mut self.next_id` borrow below (disjoint fields, but
        // spelled out for the reader).
        let session_live = self.focus_active || self.ime_state.is_some();
        match (self.root_id, self.prev_view.take()) {
            // Reconcile against the previous view of the same type.
            (Some(root_id), Some(prev)) => {
                let mut ctx = BuildCtx::new(&mut self.next_id);
                ctx.set_has_focus(session_live);
                let flags = {
                    let pod = self
                        .tree
                        .pod_mut(root_id)
                        .expect("root pod present when root_id is set");
                    let element = pod
                        .widget_mut()
                        .downcast_mut::<V::Element>()
                        .expect("root widget type matches its originating view");
                    view.rebuild(&prev, element, &mut ctx)
                };
                if let Some(pod) = self.tree.pod_mut(root_id) {
                    pod.merge_flags(flags);
                }
                self.prev_view = Some(view);
                flags
            }
            // First build: materialise the widget and insert it as the root.
            _ => {
                let mut ctx = BuildCtx::new(&mut self.next_id);
                // A first build tears nothing down, so the seed is moot — set it
                // anyway so the rule is "the root always seeds the chain", with no
                // arm exempt.
                ctx.set_has_focus(session_live);
                let id = ctx.alloc_id();
                let element = view.build(&mut ctx);
                // `new_typed` boxes the element exactly like `new` would, and
                // additionally records `V::Element`'s type name for
                // introspection — the concrete type is only nameable here.
                let pod = WidgetPod::new_typed(id, element);
                let root_id = self.tree.insert_root(pod);
                self.root_id = Some(root_id);
                self.prev_view = Some(view);
                ChangeFlags::LAYOUT | ChangeFlags::PAINT
            }
        }
    }

    /// Lay out the root widget against `window_size` and record its geometry.
    ///
    /// The root receives loose constraints (zero up to the window size) and is
    /// placed at the origin. Returns the size the root chose. No text context is
    /// threaded in (use [`RenderRoot::layout_with_text`] when the tree contains
    /// text widgets); the stored theme, if any, is still threaded down.
    pub fn layout(&mut self, window_size: Size) -> Size {
        self.layout_inner(window_size, None)
    }

    /// Lay out the root widget, threading a shared text-shaping context down to
    /// text widgets.
    ///
    /// `text_ctx` is the shell-owned `frust_text::TextContext`, passed
    /// type-erased so this crate needs no `frust-text` dependency. Text
    /// widgets recover it via [`crate::widget::LayoutCtx::text_context`]. The
    /// stored theme, if any, is threaded down alongside it.
    pub fn layout_with_text(&mut self, window_size: Size, text_ctx: &mut dyn Any) -> Size {
        self.layout_inner(window_size, Some(text_ctx))
    }

    /// Shared layout body: hands the root loose window constraints, lends the
    /// optional text context and the stored theme into a [`LayoutCtx`], and
    /// records the size the root returns.
    fn layout_inner(&mut self, window_size: Size, text_ctx: Option<&mut dyn Any>) -> Size {
        self.window_size = window_size;
        let Some(root_id) = self.root_id else {
            return Size::ZERO;
        };
        let bc = BoxConstraints::loose(window_size);
        // Disjoint field borrows: the theme (immut) and the tree (mut) are
        // different fields of `self`, so both borrows coexist through the layout.
        let theme = self.theme.as_deref();
        // Copied out before the `&mut self.tree` borrow below (a disjoint,
        // `Copy` field read).
        let insets = self.insets;
        let Some(pod) = self.tree.pod_mut(root_id) else {
            return Size::ZERO;
        };
        let mut ctx = LayoutCtx::with_resources(text_ctx, theme);
        // Thread the window insets down; one layout context reaches the whole
        // tree, so the global insets are set once here (see `crate::insets`).
        ctx.set_window_insets(insets);
        let size = pod.widget_mut().layout(&mut ctx, &bc);
        pod.set_layout(Point::ZERO, size);
        size
    }

    /// Paint the root widget into `scene`, returning whether the tree wants
    /// another frame to continue an animation.
    ///
    /// A widget whose paint advances animation state (e.g. a scroll fling) signals
    /// [`PaintCtx::request_frame`]; that flag bubbles up through the container
    /// [`ChildPod`](crate::widget::ChildPod)s and out here as
    /// [`PaintOutcome::needs_frame`], which the shell honors by scheduling the next
    /// frame (desktop `window.request_redraw()`; the mobile continuous loops
    /// already do so). Mirrors how [`RenderRoot::event`] surfaces `needs_redraw`.
    ///
    /// A widget whose animation changes its *layout* signals
    /// [`PaintCtx::request_layout`] instead (or as well); that bubbles up the same
    /// way and is folded here into the render root's pending [`ChangeFlags`]
    /// (`LAYOUT`), so the *next* frame's
    /// [`take_change_flags`](RenderRoot::take_change_flags)`().needs_layout()`
    /// reports it and the mobile intra-frame layout skip relayouts while the
    /// animation is in flight. It is also surfaced on the returned
    /// [`PaintOutcome::needs_layout`].
    ///
    /// `frame_time` is the shell's shared monotonic clock for this frame
    /// (time enters `frust-core` from the shell, never `Instant::now()` here). It
    /// is seeded onto the root [`PaintCtx`] and threaded unchanged to every child
    /// ([`crate::widget::ChildPod::paint_child`]), so an animating widget advances
    /// against one consistent timestamp — see [`PaintCtx::frame_time`].
    pub fn paint(&mut self, scene: &mut dyn PaintScene, frame_time: FrameTime) -> PaintOutcome {
        let Some(root_id) = self.root_id else {
            return PaintOutcome::default();
        };
        // Disjoint field borrows: the theme (immut) vs the tree (mut).
        let theme = self.theme.as_deref();
        // Copied out before the `&mut self.tree` borrow (a disjoint `Copy` read).
        let insets = self.insets;
        // Same disjoint `Copy` read: the presented-frame count threaded to widgets.
        let presented_frames = self.presented_frames;
        // Same disjoint `Copy` read: the surface-translucency flag the
        // platform-view hole-punch reads (see `PaintCtx::is_translucent`).
        let surface_translucent = self.surface_translucent;
        if let Some(pod) = self.tree.pod_mut(root_id) {
            let mut ctx = PaintCtx::new(pod.origin(), pod.size());
            // Seed the shared shell clock so the whole paint pass sees one time.
            ctx.set_frame_time(frame_time);
            // Lend the stored theme (type-erased) into the paint pass; widgets
            // recover it via `PaintCtx::theme_as`.
            ctx.set_theme(theme);
            // Thread the window insets down (global — see `crate::insets`).
            ctx.set_window_insets(insets);
            // Thread the shell's presented-frame count down (global; a widget
            // measuring FPS differences it — see `PaintCtx::presented_frames`).
            ctx.set_presented_frames(presented_frames);
            // Thread the surface-translucency flag down (global; the
            // platform-view hole-punch gates its rect-clear on it — see
            // `PaintCtx::is_translucent`).
            ctx.set_translucent(surface_translucent);
            // Seed the root widget's paint-time focus from the cached focus path
            // so a leaf-root editable observes its own focus; deeper focus is
            // threaded per-pod by `ChildPod::paint_child`.
            ctx.set_has_focus(self.focus_active);
            // Thread the hover mirror + live epoch the same way: the root widget's
            // own hover comes from the mirror (a leaf root can claim hover itself),
            // and deeper links are resolved per-pod by `ChildPod::paint_child`
            // against this epoch.
            ctx.set_hovered(self.hover_active);
            ctx.set_hover_epoch(self.hover_epoch);
            pod.widget_mut().paint(&mut ctx, scene);
            pod.clear_flags();
            // A focused editable republishes its IME surface during paint (which
            // runs after every rebuild), so a controlled change applied by the
            // rebuild — e.g. a submit clearing the field — refreshes the
            // shell-facing `ime_state` that the event pass alone would leave
            // stale. Defense-in-depth: only accept a bubbled publish while
            // focus is actually active. A widget whose pod focus was just
            // cleared by a container-routed blur (but whose internal flag lags
            // one frame) can then never resurrect the `ime_state` the blur
            // cleared — even before it observes the blur via `PaintCtx::has_focus`.
            //
            // Routed through `store_ime_state_in` (the field form — this pass
            // holds disjoint borrows of the tree and the theme, so no
            // whole-`self` call is possible here) so an *unchanged* republish —
            // the overwhelmingly common case, a focused field re-publishing the
            // same surface frame after frame — moves no generation and
            // therefore fires no `focus_or_ime_changed` edge at the shell.
            //
            // An **inactive** publish is not a surface refresh at all: it is the
            // publishing widget saying "this session is over" — the navigator's
            // post-pop `cleared_ime_state`, `PatternSwitcher`'s equivalent, and
            // a `TextInput` turned disabled/read-only under a live focus are the
            // three shipped producers, and every one of them is an unmount or a
            // de-focus. Storing it as `Some(inactive)` and leaving `focus_active`
            // standing is what leaked the session after a pop: the shells' gate
            // saw `ime_state().is_some()`, `is_focus_active()` kept lying, and the
            // next real focus interaction started from a corrupt baseline. So it
            // takes the *full* release instead — the same one a blur-on-outside-tap
            // `Down` performs — through the shared primitive, which fires exactly
            // one edge.
            //
            // The `self.focus_active` guard stays, and covers both arms: an
            // inactive publish arriving when focus is already false is a no-op
            // (nothing to release — `release_focus_session_in` would move
            // nothing anyway, but the guard means we never even take it), and an
            // active publish from a widget whose pod focus was just cleared
            // still cannot resurrect the surface that blur dropped.
            if self.focus_active
                && let Some(ime) = ctx.take_ime_state()
            {
                if ime.active {
                    store_ime_state_in(&mut self.ime_state, &mut self.focus_ime_gen, Some(ime));
                } else {
                    release_focus_session_in(
                        &mut self.focus_active,
                        &mut self.ime_state,
                        &mut self.focus_ime_gen,
                    );
                }
            }
            // Replace (never merge) the whole platform-view collection with
            // whatever this pass published — unlike `ime_state` above there is
            // no single "the" published instance to guard behind a focus
            // check, and a pass that publishes none must clear out every
            // stale frame from the previous one (see the field's doc comment).
            self.platform_view_frames = ctx.take_platform_views();
            // Same replace-per-pass discipline for the z-shield channel: a pass
            // whose shields stopped painting reports none, so a stale shield can
            // never keep stealing input from an interactive slot (see
            // `PaintCtx::report_input_shield`).
            self.input_shields = ctx.take_input_shields();
            // Fold a bubbled `request_layout` into `pending` so the *next* frame
            // relayouts. `pending` survives to the next frame and feeds both the
            // frame gate (`has_pending_change_flags`) and the Android layout-skip
            // (`take_change_flags().needs_layout()`), so no shell change is needed
            // on any platform. Deliberately opt-in: `request_frame` alone never
            // sets LAYOUT, keeping paint-only animations layout-free.
            let needs_layout = ctx.needs_layout();
            if needs_layout {
                self.pending |= ChangeFlags::LAYOUT;
            }
            // A rebuild that ran out of flush passes owes one more frame; surface
            // it here (and clear it) so a dirty-driven shell schedules the frame
            // that finishes the flush — see the `deferred_frame` field doc.
            let deferred_frame = std::mem::take(&mut self.deferred_frame);
            PaintOutcome {
                needs_frame: ctx.needs_frame() || deferred_frame,
                needs_layout,
                // Aggregate tick class: paced-only iff a frame was requested and
                // every request was CosmeticLoop-class. The mobile frame gate
                // may throttle such a frame; any Transition request
                // (including the LAYOUT-implying `request_layout` above) leaves
                // this false so the frame runs every vsync.
                needs_frame_paced_only: ctx.needs_frame_paced_only(),
                // ...and, when it IS paceable, how fast it asked to be re-run:
                // the MIN over every paced request this pass (`Duration::ZERO`
                // / `None` meaning the theme's own cosmetic rate). The gate
                // resolves it against the live theme's cap — see
                // `PaintCtx::request_frame_paced_at`.
                paced_interval: ctx.paced_interval(),
            }
        } else {
            PaintOutcome::default()
        }
    }

    /// Collect the accessibility tree for the current frame,
    /// returning a [`SemanticsUpdate`] a platform adapter (`accesskit_*`)
    /// can consume.
    ///
    /// Pull-based and stateless: the shell calls this when a platform a11y client
    /// asks for the tree (or after a change), *never* per frame — this crate owns
    /// no scheduling. Must run **after** [`RenderRoot::layout`], since node bounds
    /// come from the pods' post-layout geometry.
    ///
    /// The result is always rooted at a synthetic [`accesskit::Role::Window`]
    /// node covering the window, whose children are whatever the root widget
    /// contributed. An unbuilt tree yields a bare window node with no children.
    pub fn semantics(&self) -> SemanticsUpdate {
        let mut ctx = SemanticsCtx::new(self.window_size, self.semantics_alloc.get());
        let window = self.window_size;
        let root_pod = self.root_id.and_then(|id| self.tree.pod(id));
        // The root pod is arena-backed (not a `ChildPod`), so it caches its stable
        // base id in `root_semantics_id` rather than in a pod — assigned on first
        // pass and reused thereafter, exactly like `ChildPod::semantics_base`.
        let root_widget_base = match self.root_semantics_id.get() {
            Some(id) => id,
            None => {
                let id = ctx.alloc_base();
                self.root_semantics_id.set(Some(id));
                id
            }
        };
        let root_node = ctx.push_container_with_id(
            ROOT_NODE_ID,
            accesskit::Role::Window,
            |node| {
                node.set_bounds(accesskit::Rect {
                    x0: 0.0,
                    y0: 0.0,
                    x1: window.width,
                    y1: window.height,
                });
            },
            |ctx| {
                if let Some(pod) = root_pod {
                    // The root pod sits at its recorded origin (ZERO today) with
                    // its laid-out size; descend into that geometry and its stable
                    // id scope, mirroring `ChildPod::semantics_child`.
                    ctx.descend_into_pod(
                        root_widget_base,
                        pod.origin().to_vec2(),
                        pod.size(),
                        |ctx| {
                            pod.widget().semantics(ctx);
                        },
                    );
                }
            },
        );
        // Persist the allocator's high-water mark so the next pass keeps handing
        // out fresh, never-reused bases to pods that first appear later.
        self.semantics_alloc.set(ctx.next_base());
        ctx.finish(root_node)
    }

    /// The current semantics generation — bumped by every rebuild/theme swap that
    /// could have changed the accessibility tree (the semantics dirty gate).
    ///
    /// A shell records the value it last pushed and compares; see
    /// [`RenderRoot::semantics_if_changed`].
    pub fn semantics_generation(&self) -> u64 {
        self.semantics_gen
    }

    /// Pull a fresh [`SemanticsUpdate`] **only if** the semantics tree may have
    /// changed since generation `last_seen`.
    ///
    /// Returns `None` when nothing relevant changed, letting a shell skip both the
    /// tree walk and the platform `accesskit_*` push. Call it post-layout (bounds
    /// must be valid). A shell threads its stored generation in and, on `Some`,
    /// updates it from [`RenderRoot::semantics_generation`]. v1 pushes the whole
    /// tree when it does recompute (stable ids make that valid); finer-grained
    /// diffing is a later optimization.
    pub fn semantics_if_changed(&self, last_seen: u64) -> Option<SemanticsUpdate> {
        (self.semantics_gen != last_seen).then(|| self.semantics())
    }

    /// Deliver an input event to the widget tree, returning what happened.
    ///
    /// Builds a root [`EventCtx`] over the (type-erased) `state`, dispatches to
    /// the root widget — which routes the event down through its container
    /// children — and folds the result into an [`EventOutcome`]. The outcome's
    /// `needs_redraw` is set whenever a widget consumed the event or explicitly
    /// requested a redraw; the shell turns that into a `window.request_redraw()`.
    ///
    /// Root capture bookkeeping mirrors the per-container `active`-child model: a
    /// `Down` whose dispatch requested capture marks a gesture in flight; `Up`
    /// and `Cancel` release it (never a window-leave).
    ///
    /// Root hover bookkeeping is the third recorded path, and the one this pass
    /// *derives* rather than merely mirrors: an **uncaptured** `Move` opens a hover
    /// pass (widgets on the hit-tested path may claim it — see
    /// [`EventCtx::claim_hover`](crate::event::EventCtx::claim_hover)), a
    /// `Down`/`Cancel` ends whatever hover stood, and every other event leaves it
    /// alone. There is nothing to release and no generation to bump: the epoch
    /// advance strands the previous claimant's path by itself, and the outcome's
    /// `needs_redraw` carries the one repaint a widget that *lost* hover cannot ask
    /// for. **No shell change is required for hover** — the desktop shell already
    /// dispatches a `Move` on every cursor move, and the mobile shells only ever
    /// produce mid-gesture (hence captured, hence ineligible) moves.
    ///
    /// The cursor is hover's sibling channel and the fourth thing this pass
    /// resolves: any pointer `Move` (captured included) re-resolves
    /// [`RenderRoot::cursor`] from the pass's last
    /// [`EventCtx::set_cursor`](crate::event::EventCtx::set_cursor), defaulting to
    /// [`CursorIcon::Default`] when nothing asked. It deliberately does **not**
    /// fold into the outcome's `needs_redraw`: applying a cursor is a platform
    /// call a desktop shell makes straight after this pass returns, with no frame
    /// involved, and folding it in would repaint the tree on every hover move.
    ///
    /// # Reentrancy
    ///
    /// This pass **never rebuilds or repaints**. Event handlers mutate `state`
    /// synchronously through the context; the shell is expected to run a single
    /// [`RenderRoot::rebuild`] (then layout/paint) *after* the event pass returns,
    /// driven by the outcome. Rebuilding re-entrantly here would invalidate the
    /// widget references the dispatch still holds and turn the event→state→view
    /// feedback into recursion.
    ///
    /// [`RenderRoot::rebuild`] calls this itself with
    /// [`InputEvent::Housekeeping`] to flush deferred state-bearing callbacks.
    /// That is *sequential*, not re-entrant — the dispatch fully returns before
    /// the next diff starts — so the rule above is intact.
    pub fn event(&mut self, state: &mut State, event: &InputEvent) -> EventOutcome {
        let Some(root_id) = self.root_id else {
            return EventOutcome::default();
        };
        let Some(pod) = self.tree.pod_mut(root_id) else {
            return EventOutcome::default();
        };

        // Hover is derived per **uncaptured** pointer `Move`: that pass, and only
        // that pass, may record a claim, so a captured drag can never paint hover
        // under the pointer. A `Down`/`Cancel` is the other epoch-advancing pass —
        // it ends whatever hover stood, without opening a new one (a press is not a
        // hover, and a touch `Down` must not inherit one). `Up`, scroll, key, IME,
        // and the housekeeping broadcast leave a live hover exactly as it was.
        let hover_pass = matches!(event, InputEvent::Pointer(p) if p.phase == PointerPhase::Move)
            && !self.pointer_captured;
        let hover_ends = matches!(
            event,
            InputEvent::Pointer(p) if matches!(p.phase, PointerPhase::Down | PointerPhase::Cancel)
        );
        let hover_epoch = self.hover_epoch;
        let hover_was_active = self.hover_active;

        // The cursor pass is hover's pass widened by one case: **any** pointer
        // `Move`, captured included, because a captured `Move` routes only to the
        // capturing widget and that is exactly how a drag keeps its own cursor
        // while the pointer is outside its bounds. Every other pass leaves the
        // resolved cursor standing — notably `Down`/`Up`, whose handlers have no
        // reason to restate a cursor and whose reset would blink the shape back to
        // `Default` for the length of a click.
        //
        // The slot is cleared here rather than trusted to be empty: a `set_cursor`
        // from a dispatch no root drove (a reconciler's synthesized `Cancel`)
        // must not leak into this pass's resolution.
        let cursor_pass = matches!(event, InputEvent::Pointer(p) if p.phase == PointerPhase::Move);
        clear_cursor_request();

        let (handled, needs_redraw, captured, hover_claimed, focus_req, focus_rel, ime) = {
            let state_any: &mut dyn Any = state;
            let mut ctx = EventCtx::new(state_any, pod.origin(), pod.size());
            // Seed the root widget's focus flag so a leaf-root editable that holds
            // focus can observe `has_focus()`; deeper focus is threaded per-pod.
            ctx.set_has_focus(self.focus_active);
            // Same for the hover link, plus the live epoch every pod compares its
            // stamp against and the eligibility gate that decides whether a claim
            // is recordable at all this pass.
            ctx.set_hovered(hover_was_active);
            ctx.set_hover_epoch(hover_epoch);
            ctx.set_hover_eligible(hover_pass);
            let result = pod.widget_mut().event(&mut ctx, event);
            let handled = matches!(result, EventResult::Handled);
            (
                handled,
                ctx.needs_redraw() || handled,
                ctx.is_pointer_captured(),
                ctx.is_hover_claimed(),
                ctx.is_focus_requested(),
                ctx.is_focus_released(),
                ctx.take_ime_state(),
            )
        };

        // A published IME surface refreshes the stored one (persists past this
        // event, survives rebuild) until a blur clears it below. `store_ime_state`
        // bumps the focus/IME edge generation only if the surface actually moved
        // (a keystroke that changes nothing observable is not an edge).
        //
        // An **inactive** publish carries the same release intent here as it does
        // in `paint` (see that method's take path): a widget that publishes
        // `active: false` is ending the session, not describing it, so it takes
        // the full release. Reachable from this pass too — every paint-time
        // producer of an inactive surface is a container/widget whose `event` arm
        // can run first — and the two passes must not disagree about what an
        // inactive surface means. No `focus_active` guard is needed (unlike
        // `paint`, which must refuse to *resurrect* a cleared surface): releasing
        // an already-released root moves nothing and fires no edge.
        //
        // Ordering: this runs before the focus match below, so a dispatch that
        // both published an inactive surface and requested focus still ends up
        // focused — the later, more specific claim wins.
        match ime {
            Some(ime) if !ime.active => {
                self.release_focus_session();
            }
            Some(ime) => self.store_ime_state(Some(ime)),
            None => {}
        }

        // Root-level capture path: a captured `Down` opens a gesture; `Up`/`Cancel`
        // close it. `Move` leaves the flag untouched so it survives the drag.
        //
        // Root-level focus path (the capture mirror): a `Down` that requested
        // focus opens the focus session; a `Down` that did not is a
        // blur-on-outside-tap and closes it (the per-container `focused` flags are
        // cleared by the routing helpers). Key/Ime/Scroll only adjust focus if the
        // dispatch explicitly requested or released it.
        //
        // Every arm mutates through `set_focus_active`/`store_ime_state` or the
        // paired `release_focus_session`, the change-guarded writers that own the
        // focus/IME edge generation: a `Down` on already-blurred chrome (the
        // commonest event of all) writes the same values back and must therefore
        // NOT fire an edge.
        match event {
            InputEvent::Pointer(pointer) => match pointer.phase {
                PointerPhase::Down => {
                    if captured {
                        self.pointer_captured = true;
                    }
                    if focus_req {
                        self.set_focus_active(true);
                    } else {
                        // Blur: no widget on the tapped path took focus. The
                        // canonical release — flag and surface drop together, as
                        // one edge (see `release_focus_session_in`).
                        self.release_focus_session();
                    }
                }
                PointerPhase::Up | PointerPhase::Cancel => self.pointer_captured = false,
                PointerPhase::Move => {}
            },
            InputEvent::Scroll { .. } | InputEvent::Key(_) | InputEvent::Ime(_) => {
                if focus_req {
                    self.set_focus_active(true);
                }
                if focus_rel {
                    self.release_focus_session();
                }
            }
            // A broadcast is not user input: it opens no gesture, claims no
            // focus, and blurs nothing. Deliberately inert here — a housekeeping
            // pass that moved the root's capture/focus bookkeeping would change
            // what the *next* real event does, which is exactly what this
            // mechanism must not do (see `InputEvent::Housekeeping`).
            //
            // The one thing a broadcast *can* still move is the IME surface, via
            // the publish handling above (which is pass-agnostic by design, and
            // was before this arm existed): a widget that publishes while
            // flushing has said something about its session either way, and an
            // inactive publish releases it. No shipped widget does — `TextInput`
            // ignores a broadcast outright, and both cleared-surface publishers
            // are paint-time — so this is a contract note, not live behavior.
            InputEvent::Housekeeping => {}
        }

        // Close the hover pass: advance the epoch (which strands every stamp this
        // pass did not renew, wherever in the tree it sits) and refresh the mirror.
        // A claim only counts on a `hover_pass` — an ineligible pass records none
        // anyway, but stating it here keeps the mirror true by construction rather
        // than by the eligibility gate alone.
        if hover_pass || hover_ends {
            self.hover_epoch = self.hover_epoch.wrapping_add(1);
            self.hover_active = hover_pass && hover_claimed;
        }

        // Close the cursor pass: the last request of the pass wins, and its
        // absence resolves to `Default` — which is what makes the request
        // stateless (a widget that stops asking needs no clearing) and what a
        // pointer moving off every requesting widget resolves to. Drained
        // unconditionally so a request made on a non-cursor pass cannot survive
        // into the next one; only a cursor pass commits it.
        let requested = take_cursor_request();
        if cursor_pass {
            self.cursor = requested.unwrap_or_default();
        }
        // A hover that *ended* needs one repaint the widget losing it cannot ask
        // for: the pointer moved onto something else (or a press/cancel cleared the
        // link), so the old claimant's `event()` was never called. A hover that
        // *began* or *moved to another widget* is already covered — the new
        // claimant's own changed-state redraw repaints the whole tree, which is what
        // lets the old one drop its overlay from `PaintCtx::is_hovered`.
        let needs_redraw = needs_redraw || (hover_was_active && !self.hover_active);

        EventOutcome {
            handled,
            needs_redraw,
        }
    }

    /// Perform a platform accessibility action, returning the same
    /// [`EventOutcome`] the synthesized input produced.
    ///
    /// A platform `accesskit_*` adapter delivers an `ActionRequest(node_id,
    /// action)`; a shell forwards it here. v1 routes actions through the **normal
    /// event path** by synthesizing pointer events at the target node's absolute
    /// bounds center (recovered from a fresh semantics pass — the id → bounds map
    /// the pass produces), so *every* fire-on-up-inside widget is operable with
    /// **zero** widget-side changes:
    ///
    /// * [`accesskit::Action::Click`] → a `Down` then an `Up` at the center,
    ///   activating any button/switch/checkbox exactly as a real tap would.
    /// * [`accesskit::Action::Focus`] → a `Down` then a synthetic `Cancel` at
    ///   the center: the `Down` claims focus for a widget that opts in on `Down`
    ///   (the recorded-focus contract), and the `Cancel` releases the capture
    ///   that same `Down` opened without touching the recorded focus path — so
    ///   the action claims focus without leaving the widget permanently
    ///   capturing every later pointer event. A widget that does not claim focus
    ///   on `Down` is unaffected, and `Cancel` never fires an on-press callback.
    /// * any other action → ignored (a no-op [`EventOutcome`]); richer actions are
    ///   deferred.
    ///
    /// An unknown `node_id` (not in the current tree) is a benign no-op. Requires
    /// a prior [`RenderRoot::layout`] so the bounds are valid. Synthetic-pointer
    /// activation cannot drive widgets that require a real drag (e.g. a slider) —
    /// an accepted v1 limitation.
    pub fn perform_accessibility_action(
        &mut self,
        state: &mut State,
        node_id: accesskit::NodeId,
        action: accesskit::Action,
    ) -> EventOutcome {
        // Recover the node's absolute bounds from a fresh semantics pass (the
        // id → absolute-bounds map this action routing needs; recomputing keeps
        // it in step with the live tree without a stored cache).
        let update = self.semantics();
        let Some(bounds) = update
            .nodes
            .iter()
            .find(|(id, _)| *id == node_id)
            .and_then(|(_, node)| node.bounds())
        else {
            return EventOutcome::default();
        };
        let center = Point::new((bounds.x0 + bounds.x1) / 2.0, (bounds.y0 + bounds.y1) / 2.0);
        let synth = |phase| {
            InputEvent::Pointer(PointerEvent {
                phase,
                position: center,
                button: PointerButton::Primary,
            })
        };
        match action {
            accesskit::Action::Click => {
                let down = self.event(state, &synth(PointerPhase::Down));
                let up = self.event(state, &synth(PointerPhase::Up));
                EventOutcome {
                    handled: down.handled || up.handled,
                    needs_redraw: down.needs_redraw || up.needs_redraw,
                }
            }
            accesskit::Action::Focus => {
                // A `Down` claims focus for a widget that opts in on `Down`; a
                // synthetic `Cancel` then releases the capture that `Down` also
                // opened (mirroring a real gesture steal), leaving the recorded
                // focus path intact — `Cancel` clears both `active` and
                // `pointer_captured` while never touching `focused`/`focus_active`.
                // Without the `Cancel`, the `Down` alone would leave the widget
                // permanently capturing every subsequent pointer event.
                let down = self.event(state, &synth(PointerPhase::Down));
                let cancel = self.event(state, &synth(PointerPhase::Cancel));
                EventOutcome {
                    handled: down.handled || cancel.handled,
                    needs_redraw: down.needs_redraw || cancel.needs_redraw,
                }
            }
            // Other actions are not modelled in v1: ignore rather than guess.
            _ => EventOutcome::default(),
        }
    }
}

impl<State: 'static, V: View<State>> Default for RenderRoot<State, V> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Application state for the tests.
    #[derive(Default)]
    struct AppState {
        label: String,
    }

    /// The retained widget produced by `MockTextView`: stores the current text
    /// and records what it painted.
    struct TextWidget {
        text: String,
    }

    impl crate::widget::Widget for TextWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            // A crude intrinsic size: width proportional to text length.
            let intrinsic = Size::new(self.text.len() as f64 * 8.0, 16.0);
            bc.constrain(intrinsic)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.draw_text(ctx.origin(), &self.text);
        }
    }

    /// The task's `MockTextView`: a real `View` impl living in tests.
    struct MockTextView {
        text: String,
    }

    impl View<AppState> for MockTextView {
        type Element = TextWidget;

        fn build(&self, _ctx: &mut BuildCtx<'_>) -> Self::Element {
            TextWidget {
                text: self.text.clone(),
            }
        }

        fn rebuild(
            &self,
            prev: &Self,
            element: &mut Self::Element,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            if prev.text != self.text {
                element.text = self.text.clone();
                // Text change: same size model would relayout, but the intrinsic
                // width can change, so signal PAINT here and let callers decide.
                ChangeFlags::PAINT
            } else {
                ChangeFlags::NONE
            }
        }
    }

    /// A scene recorder for asserting paint output.
    #[derive(Default)]
    struct RecordingScene {
        texts: Vec<(Point, String)>,
    }
    impl PaintScene for RecordingScene {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: peniko::Color) {}
        fn draw_text(&mut self, origin: Point, text: &str) {
            self.texts.push((origin, text.to_string()));
        }
    }

    fn app_logic(state: &mut AppState) -> MockTextView {
        MockTextView {
            text: state.label.clone(),
        }
    }

    #[test]
    fn build_inserts_widget_into_arena() {
        let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
        let mut state = AppState {
            label: "hello".to_string(),
        };
        let flags = root.rebuild(&mut app_logic, &mut state);
        // First build dirties both passes.
        assert!(flags.needs_layout());
        assert!(flags.needs_paint());
        let id = root.root_id().expect("root built");
        let pod = root.tree().pod(id).expect("pod in arena");
        assert!(pod.widget().downcast_ref_is::<TextWidget>());
    }

    #[test]
    fn rebuild_changed_data_yields_paint() {
        let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
        let mut state = AppState {
            label: "a".to_string(),
        };
        root.rebuild(&mut app_logic, &mut state);
        state.label = "b".to_string();
        let flags = root.rebuild(&mut app_logic, &mut state);
        assert_eq!(flags, ChangeFlags::PAINT);
    }

    #[test]
    fn rebuild_unchanged_data_yields_none() {
        let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
        let mut state = AppState {
            label: "same".to_string(),
        };
        root.rebuild(&mut app_logic, &mut state);
        let flags = root.rebuild(&mut app_logic, &mut state);
        assert_eq!(flags, ChangeFlags::NONE);
    }

    #[test]
    fn layout_stores_size_in_pod() {
        let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
        let mut state = AppState {
            label: "hi".to_string(),
        };
        root.rebuild(&mut app_logic, &mut state);
        let size = root.layout(Size::new(800.0, 600.0));
        // "hi" -> 2 * 8 = 16 wide, 16 tall, within the window.
        assert_eq!(size, Size::new(16.0, 16.0));
        let id = root.root_id().unwrap();
        let pod = root.tree().pod(id).unwrap();
        assert_eq!(pod.origin(), Point::ZERO);
        assert_eq!(pod.size(), Size::new(16.0, 16.0));
    }

    #[test]
    fn inspect_reports_the_laid_out_root() {
        let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
        let mut state = AppState {
            label: "hi".to_string(),
        };
        // Before the first build there is nothing to inspect.
        assert!(root.inspect().is_empty());

        root.rebuild(&mut app_logic, &mut state);
        root.layout(Size::new(800.0, 600.0));

        let nodes = root.inspect();
        assert_eq!(nodes.len(), 1);
        let node = &nodes[0];
        assert_eq!(node.id, root.root_id().unwrap());
        assert_eq!(node.parent, None);
        assert_eq!(node.depth, 0);
        assert!(node.children.is_empty());
        // The concrete element type is captured, not the erased box.
        assert!(node.type_name.ends_with("TextWidget"), "{}", node.type_name);
        assert_eq!(node.debug_label, None);
        // Bounds match what the layout pass recorded on the pod.
        let pod = root.tree().pod(node.id).unwrap();
        assert_eq!(
            node.bounds,
            Rect::from_origin_size(pod.origin(), pod.size())
        );
        assert_eq!(node.bounds, Rect::new(0.0, 0.0, 16.0, 16.0));
    }

    #[test]
    fn layout_clamps_to_window() {
        let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
        let mut state = AppState {
            label: "wwwwwwwwwww".to_string(), // 10 chars -> 80 wide intrinsic
        };
        root.rebuild(&mut app_logic, &mut state);
        let size = root.layout(Size::new(40.0, 40.0));
        // Intrinsic width 80 is clamped to the 40-wide window.
        assert_eq!(size.width, 40.0);
    }

    #[test]
    fn paint_emits_current_text() {
        let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
        let mut state = AppState {
            label: "one".to_string(),
        };
        root.rebuild(&mut app_logic, &mut state);
        root.layout(Size::new(200.0, 200.0));

        let mut scene = RecordingScene::default();
        root.paint(&mut scene, FrameTime::ZERO);
        assert_eq!(scene.texts, vec![(Point::ZERO, "one".to_string())]);

        // Change data, rebuild, repaint -> new text.
        state.label = "two".to_string();
        root.rebuild(&mut app_logic, &mut state);
        let mut scene2 = RecordingScene::default();
        root.paint(&mut scene2, FrameTime::ZERO);
        assert_eq!(scene2.texts, vec![(Point::ZERO, "two".to_string())]);
    }

    /// A leaf widget that publishes a fixed [`PlatformViewFrame`] — and reports
    /// a z-shield rect over its own bounds — on every paint, unless
    /// `should_publish` is false (the widget-level toggle that simulates a slot
    /// no longer publishing between two rebuilds). Both channels ride the same
    /// toggle so one fixture covers both replace-per-pass contracts.
    struct PlatformViewProbeWidget {
        slot_id: u64,
        should_publish: bool,
    }

    impl crate::widget::Widget for PlatformViewProbeWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(10.0, 10.0))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            if self.should_publish {
                ctx.publish_platform_view(PlatformViewFrame {
                    slot_id: self.slot_id,
                    view_type: "dev.frust.Probe".to_string(),
                    params_json: String::new(),
                    params_generation: 0,
                    rect: kurbo::Rect::from_origin_size(ctx.origin(), ctx.size()),
                    clip: None,
                    visible: true,
                    interactive: false,
                    shields: Vec::new(),
                });
                ctx.report_input_shield(kurbo::Rect::from_origin_size(ctx.origin(), ctx.size()));
            }
        }
    }

    /// A root widget owning two independently toggleable [`ChildPod`]s (a
    /// minimal two-slot container) so a rebuild can flip either slot's
    /// `should_publish` — the fixture the "two slots in one pass" and
    /// "empty-pass clears stale frames" tests below need.
    struct PlatformViewRootWidget {
        a: crate::widget::ChildPod,
        b: crate::widget::ChildPod,
    }

    impl crate::widget::Widget for PlatformViewRootWidget {
        fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            self.a.layout_child(ctx, bc);
            self.b.layout_child(ctx, bc);
            bc.constrain(Size::new(10.0, 10.0))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            self.a.paint_child(ctx, scene);
            self.b.paint_child(ctx, scene);
        }
    }

    /// The `View` producing [`PlatformViewRootWidget`], reconciling each
    /// slot's `should_publish` flag on rebuild like any controlled widget.
    struct PlatformViewRootView {
        publish_a: bool,
        publish_b: bool,
    }

    impl View<PvState> for PlatformViewRootView {
        type Element = PlatformViewRootWidget;

        fn build(&self, _ctx: &mut BuildCtx<'_>) -> Self::Element {
            PlatformViewRootWidget {
                a: crate::widget::ChildPod::new(Box::new(PlatformViewProbeWidget {
                    slot_id: 1,
                    should_publish: self.publish_a,
                })),
                b: crate::widget::ChildPod::new(Box::new(PlatformViewProbeWidget {
                    slot_id: 2,
                    should_publish: self.publish_b,
                })),
            }
        }

        fn rebuild(
            &self,
            prev: &Self,
            element: &mut Self::Element,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            if prev.publish_a != self.publish_a || prev.publish_b != self.publish_b {
                element
                    .a
                    .widget_mut()
                    .downcast_mut::<PlatformViewProbeWidget>()
                    .expect("slot a stays a PlatformViewProbeWidget")
                    .should_publish = self.publish_a;
                element
                    .b
                    .widget_mut()
                    .downcast_mut::<PlatformViewProbeWidget>()
                    .expect("slot b stays a PlatformViewProbeWidget")
                    .should_publish = self.publish_b;
                ChangeFlags::PAINT
            } else {
                ChangeFlags::NONE
            }
        }
    }

    /// App state for the platform-view frame-channel tests.
    #[derive(Default)]
    struct PvState {
        publish_a: bool,
        publish_b: bool,
    }

    fn platform_view_logic(state: &mut PvState) -> PlatformViewRootView {
        PlatformViewRootView {
            publish_a: state.publish_a,
            publish_b: state.publish_b,
        }
    }

    #[test]
    fn platform_view_frames_arrive_in_order_and_clear_on_empty_pass() {
        let mut root: RenderRoot<PvState, PlatformViewRootView> = RenderRoot::new();
        let mut state = PvState {
            publish_a: true,
            publish_b: true,
        };
        root.rebuild(&mut platform_view_logic, &mut state);
        root.layout(Size::new(200.0, 200.0));

        let mut scene = RecordingScene::default();
        root.paint(&mut scene, FrameTime::ZERO);

        // Two slots publishing in one pass both arrive, in paint order — the
        // regression test for the overwrite hazard (an Option-based `ime_state`
        // shape here would leave only the second slot's frame).
        let frames = root.platform_view_frames();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].slot_id, 1);
        assert_eq!(frames[1].slot_id, 2);

        // Next pass: neither slot publishes (simulates both going away/culled).
        // The collection is REPLACED, so the previous pass's frames must not
        // survive as stale entries.
        state.publish_a = false;
        state.publish_b = false;
        root.rebuild(&mut platform_view_logic, &mut state);
        root.layout(Size::new(200.0, 200.0));
        root.paint(&mut scene, FrameTime::ZERO);
        assert!(
            root.platform_view_frames().is_empty(),
            "a paint pass with no publishers must yield an empty slice"
        );
    }

    #[test]
    fn input_shields_arrive_in_order_and_clear_on_empty_pass() {
        // The shield channel's half of the contract above:
        // two shields reported in one pass both survive (the `Vec`
        // extend, not an `Option` overwrite), and a pass that reports none
        // replaces the collection rather than merging — a stale shield must
        // never keep stealing input from an interactive slot.
        let mut root: RenderRoot<PvState, PlatformViewRootView> = RenderRoot::new();
        let mut state = PvState {
            publish_a: true,
            publish_b: true,
        };
        root.rebuild(&mut platform_view_logic, &mut state);
        root.layout(Size::new(200.0, 200.0));

        let mut scene = RecordingScene::default();
        root.paint(&mut scene, FrameTime::ZERO);
        assert_eq!(root.input_shields().len(), 2);
        assert_eq!(
            root.input_shields()[0],
            Rect::from_origin_size(Point::ZERO, Size::new(10.0, 10.0)),
            "a shield is reported in absolute paint coordinates"
        );

        state.publish_a = false;
        state.publish_b = false;
        root.rebuild(&mut platform_view_logic, &mut state);
        root.layout(Size::new(200.0, 200.0));
        root.paint(&mut scene, FrameTime::ZERO);
        assert!(
            root.input_shields().is_empty(),
            "a paint pass reporting no shields must yield an empty slice"
        );
    }

    #[test]
    fn retired_platform_views_drain_exactly_once() {
        // The prompt-teardown channel: a reported slot
        // id is handed to the shell once and then gone, mirroring
        // `take_change_flags`. Serialized against the other test touching the
        // process-wide list (see `RETIRE_TEST_LOCK`).
        let _guard = RETIRE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut root: RenderRoot<PvState, PlatformViewRootView> = RenderRoot::new();
        let _ = root.take_retired_platform_views(); // clear anything a sibling left

        crate::widget::report_retired_slot(7);
        crate::widget::report_retired_slot(9);
        assert_eq!(root.take_retired_platform_views(), vec![7, 9]);
        assert!(
            root.take_retired_platform_views().is_empty(),
            "draining is destructive — a second drain reports nothing"
        );
    }

    #[test]
    fn retired_platform_views_are_capped_dropping_the_oldest() {
        // A shell that never drains (desktop: no native compositor) must not
        // grow this list forever; past the cap the OLDEST id is dropped and the
        // differ's missing-streak backstop covers it.
        let _guard = RETIRE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut root: RenderRoot<PvState, PlatformViewRootView> = RenderRoot::new();
        let _ = root.take_retired_platform_views();

        for slot_id in 0..1_000u64 {
            crate::widget::report_retired_slot(slot_id);
        }
        let drained = root.take_retired_platform_views();
        assert!(drained.len() <= 256, "the pending list stays bounded");
        assert_eq!(
            *drained.last().expect("non-empty"),
            999,
            "the newest report always survives"
        );
        assert!(
            !drained.contains(&0),
            "the oldest reports are the ones dropped"
        );
    }

    /// Serializes the two tests that drive the process-wide retire list
    /// (`crate::widget::report_retired_slot`), which `cargo test`'s parallel
    /// threads would otherwise interleave — the same shape
    /// `frust-shell-common::theme_override`'s tests use for its global slot.
    static RETIRE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A root widget that advances no state but requests a continuation frame on
    /// every paint — stands in for an animating widget (e.g. a scroll fling).
    struct FrameWidget;
    impl crate::widget::Widget for FrameWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(10.0, 10.0))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            ctx.request_frame();
        }
    }

    struct FrameView;
    impl View<AppState> for FrameView {
        type Element = FrameWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> FrameWidget {
            FrameWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut FrameWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    #[test]
    fn paint_reports_needs_frame_from_animating_root() {
        // A still root reports no continuation frame.
        let mut still: RenderRoot<AppState, MockTextView> = RenderRoot::new();
        let mut state = AppState {
            label: "x".to_string(),
        };
        still.rebuild(&mut app_logic, &mut state);
        still.layout(Size::new(100.0, 100.0));
        let mut scene = RecordingScene::default();
        assert!(!still.paint(&mut scene, FrameTime::ZERO).needs_frame);

        // An animating root bubbles request_frame out as PaintOutcome::needs_frame.
        let mut anim: RenderRoot<AppState, FrameView> = RenderRoot::new();
        anim.rebuild(&mut |_s: &mut AppState| FrameView, &mut state);
        anim.layout(Size::new(100.0, 100.0));
        let mut scene2 = RecordingScene::default();
        assert!(anim.paint(&mut scene2, FrameTime::ZERO).needs_frame);
    }

    /// A root widget whose animation changes its layout: it requests a layout
    /// re-run on every paint — stands in for an expanding accordion.
    struct LayoutFrameWidget;
    impl crate::widget::Widget for LayoutFrameWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(10.0, 10.0))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            ctx.request_layout();
        }
    }

    struct LayoutFrameView;
    impl View<AppState> for LayoutFrameView {
        type Element = LayoutFrameWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> LayoutFrameWidget {
            LayoutFrameWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut LayoutFrameWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    #[test]
    fn paint_folds_request_layout_into_pending_change_flags() {
        let mut state = AppState {
            label: "x".to_string(),
        };

        // A root calling `request_layout` in paint surfaces it on the outcome AND
        // folds LAYOUT into `pending`, so the NEXT frame's `take_change_flags`
        // reports `needs_layout()`.
        let mut anim: RenderRoot<AppState, LayoutFrameView> = RenderRoot::new();
        anim.rebuild(&mut |_s: &mut AppState| LayoutFrameView, &mut state);
        anim.layout(Size::new(100.0, 100.0));
        // Drain any rebuild/layout dirtiness so we observe only paint's fold.
        let _ = anim.take_change_flags();
        let mut scene = RecordingScene::default();
        let outcome = anim.paint(&mut scene, FrameTime::ZERO);
        assert!(outcome.needs_layout, "outcome reports needs_layout");
        // `request_layout` implies `request_frame`, so the animation still runs.
        assert!(outcome.needs_frame, "request_layout implies needs_frame");
        assert!(
            anim.has_pending_change_flags(),
            "the fold survives to the next frame"
        );
        assert!(
            anim.take_change_flags().needs_layout(),
            "next frame's take_change_flags reports needs_layout"
        );
    }

    #[test]
    fn paint_request_frame_only_does_not_fold_layout() {
        let mut state = AppState {
            label: "x".to_string(),
        };

        // A paint-only animation (request_frame, no request_layout) must NOT fold
        // LAYOUT — the mobile layout-skip win depends on this staying opt-in.
        let mut anim: RenderRoot<AppState, FrameView> = RenderRoot::new();
        anim.rebuild(&mut |_s: &mut AppState| FrameView, &mut state);
        anim.layout(Size::new(100.0, 100.0));
        let _ = anim.take_change_flags();
        let mut scene = RecordingScene::default();
        let outcome = anim.paint(&mut scene, FrameTime::ZERO);
        assert!(outcome.needs_frame);
        assert!(
            !outcome.needs_layout,
            "request_frame alone: no needs_layout"
        );
        assert!(
            !anim.has_pending_change_flags(),
            "request_frame alone must not fold LAYOUT into pending"
        );
    }

    /// A root whose paint requests a *pacable* cosmetic-loop frame — stands in
    /// for a skeleton shimmer whose cadence the mobile frame gate may throttle.
    struct PacedFrameWidget;
    impl crate::widget::Widget for PacedFrameWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(10.0, 10.0))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            ctx.request_frame_paced();
        }
    }

    struct PacedFrameView;
    impl View<AppState> for PacedFrameView {
        type Element = PacedFrameWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> PacedFrameWidget {
            PacedFrameWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut PacedFrameWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    #[test]
    fn paint_surfaces_paced_only_tick_class_on_outcome() {
        let mut state = AppState {
            label: "x".to_string(),
        };

        // A paced-only root surfaces `needs_frame_paced_only` on the outcome so
        // the mobile frame gate may throttle its cadence.
        let mut paced: RenderRoot<AppState, PacedFrameView> = RenderRoot::new();
        paced.rebuild(&mut |_s: &mut AppState| PacedFrameView, &mut state);
        paced.layout(Size::new(100.0, 100.0));
        let mut scene = RecordingScene::default();
        let outcome = paced.paint(&mut scene, FrameTime::ZERO);
        assert!(outcome.needs_frame);
        assert!(
            outcome.needs_frame_paced_only,
            "a purely-cosmetic frame surfaces as paced-only"
        );
        assert_eq!(
            outcome.paced_interval,
            Some(std::time::Duration::ZERO),
            "a bare `request_frame_paced` names no interval (the theme's own rate)"
        );

        // A Transition-class (`request_frame`) root is never paced-only, keeping
        // today's every-vsync behavior for existing callers.
        let mut anim: RenderRoot<AppState, FrameView> = RenderRoot::new();
        anim.rebuild(&mut |_s: &mut AppState| FrameView, &mut state);
        anim.layout(Size::new(100.0, 100.0));
        let mut scene2 = RecordingScene::default();
        let outcome2 = anim.paint(&mut scene2, FrameTime::ZERO);
        assert!(outcome2.needs_frame);
        assert!(
            !outcome2.needs_frame_paced_only,
            "request_frame stays unpaced (Transition)"
        );
        assert_eq!(
            outcome2.paced_interval, None,
            "an unpaced frame names no paced interval"
        );
    }

    /// A root widget whose decorative loop names its own slow cadence — the
    /// `request_frame_paced_at` counterpart of [`PacedFrameWidget`].
    struct SlowPacedFrameWidget;
    impl crate::widget::Widget for SlowPacedFrameWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(10.0, 10.0))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            ctx.request_frame_paced_at(std::time::Duration::from_millis(500));
        }
    }
    struct SlowPacedFrameView;
    impl View<AppState> for SlowPacedFrameView {
        type Element = SlowPacedFrameWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> SlowPacedFrameWidget {
            SlowPacedFrameWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut SlowPacedFrameWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    #[test]
    fn paint_surfaces_the_requested_paced_interval_on_outcome() {
        // The end-to-end core half of the per-request pacing seam: a widget's
        // `request_frame_paced_at` reaches the shell on `PaintOutcome`, which is
        // what the mobile gate latches into `FramePacing`.
        let mut state = AppState {
            label: "x".to_string(),
        };
        let mut root: RenderRoot<AppState, SlowPacedFrameView> = RenderRoot::new();
        root.rebuild(&mut |_s: &mut AppState| SlowPacedFrameView, &mut state);
        root.layout(Size::new(100.0, 100.0));
        let mut scene = RecordingScene::default();
        let outcome = root.paint(&mut scene, FrameTime::ZERO);
        assert!(outcome.needs_frame_paced_only);
        assert_eq!(
            outcome.paced_interval,
            Some(std::time::Duration::from_millis(500))
        );
    }

    /// A root widget that records the `frame_time` its paint observed, so a test
    /// can prove the shell-injected clock reaches `PaintCtx::frame_time()`.
    struct ClockWidget {
        seen: std::rc::Rc<std::cell::Cell<Option<FrameTime>>>,
    }
    impl crate::widget::Widget for ClockWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(10.0, 10.0))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            self.seen.set(Some(ctx.frame_time()));
        }
    }

    struct ClockView {
        seen: std::rc::Rc<std::cell::Cell<Option<FrameTime>>>,
    }
    impl View<AppState> for ClockView {
        type Element = ClockWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ClockWidget {
            ClockWidget {
                seen: self.seen.clone(),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut ClockWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    #[test]
    fn paint_threads_injected_frame_time_to_widget() {
        let seen = std::rc::Rc::new(std::cell::Cell::new(None));
        let mut root: RenderRoot<AppState, ClockView> = RenderRoot::new();
        let mut state = AppState::default();
        let seen_for_view = seen.clone();
        root.rebuild(
            &mut move |_s: &mut AppState| ClockView {
                seen: seen_for_view.clone(),
            },
            &mut state,
        );
        root.layout(Size::new(100.0, 100.0));

        // Two paints with distinct injected times: the widget observes each one,
        // proving the clock is shell-fed (not read from an ambient `Instant`).
        let mut scene = RecordingScene::default();
        root.paint(&mut scene, FrameTime::from_nanos(1_000));
        assert_eq!(seen.get(), Some(FrameTime::from_nanos(1_000)));
        root.paint(&mut scene, FrameTime::from_nanos(17_000));
        assert_eq!(seen.get(), Some(FrameTime::from_nanos(17_000)));
    }

    // --- Theme threading: a dummy theme recovered during paint/layout. ---

    /// A dummy theme type standing in for `frust_theme::Theme` — `frust-core`
    /// never names the real one, so this proves the type-erased slot works for
    /// any `'static` type.
    #[derive(Debug, Clone, PartialEq)]
    struct TestTheme {
        accent: u32,
    }

    /// A root widget recording the theme accent it recovered during paint (and
    /// during layout), or `None` when no theme was threaded in.
    struct ThemeWidget {
        seen_paint: std::rc::Rc<std::cell::Cell<Option<u32>>>,
        seen_layout: std::rc::Rc<std::cell::Cell<Option<u32>>>,
    }
    impl crate::widget::Widget for ThemeWidget {
        fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            self.seen_layout
                .set(ctx.theme_as::<TestTheme>().map(|t| t.accent));
            bc.constrain(Size::new(10.0, 10.0))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            self.seen_paint
                .set(ctx.theme_as::<TestTheme>().map(|t| t.accent));
        }
    }

    struct ThemeView {
        seen_paint: std::rc::Rc<std::cell::Cell<Option<u32>>>,
        seen_layout: std::rc::Rc<std::cell::Cell<Option<u32>>>,
    }
    impl View<AppState> for ThemeView {
        type Element = ThemeWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ThemeWidget {
            ThemeWidget {
                seen_paint: self.seen_paint.clone(),
                seen_layout: self.seen_layout.clone(),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut ThemeWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    fn drive_theme_root(theme: Option<TestTheme>) -> (Option<u32>, Option<u32>) {
        let seen_paint = std::rc::Rc::new(std::cell::Cell::new(None));
        let seen_layout = std::rc::Rc::new(std::cell::Cell::new(None));
        let mut root: RenderRoot<AppState, ThemeView> = RenderRoot::new();
        if let Some(theme) = theme {
            root.set_theme(Box::new(theme));
        }
        let mut state = AppState::default();
        let sp = seen_paint.clone();
        let sl = seen_layout.clone();
        root.rebuild(
            &mut move |_s: &mut AppState| ThemeView {
                seen_paint: sp.clone(),
                seen_layout: sl.clone(),
            },
            &mut state,
        );
        root.layout(Size::new(100.0, 100.0));
        let mut scene = RecordingScene::default();
        root.paint(&mut scene, FrameTime::ZERO);
        (seen_layout.get(), seen_paint.get())
    }

    #[test]
    fn set_theme_threads_into_layout_and_paint() {
        let (layout, paint) = drive_theme_root(Some(TestTheme { accent: 5 }));
        assert_eq!(layout, Some(5));
        assert_eq!(paint, Some(5));
    }

    #[test]
    fn no_theme_yields_none_in_layout_and_paint() {
        let (layout, paint) = drive_theme_root(None);
        assert_eq!(layout, None);
        assert_eq!(paint, None);
    }

    #[test]
    fn set_theme_replaces_the_previous_theme() {
        // A second `set_theme` (a live dark-mode flip on desktop) wins on the
        // next paint.
        let seen_paint = std::rc::Rc::new(std::cell::Cell::new(None));
        let seen_layout = std::rc::Rc::new(std::cell::Cell::new(None));
        let mut root: RenderRoot<AppState, ThemeView> = RenderRoot::new();
        root.set_theme(Box::new(TestTheme { accent: 1 }));
        let mut state = AppState::default();
        let sp = seen_paint.clone();
        let sl = seen_layout.clone();
        root.rebuild(
            &mut move |_s: &mut AppState| ThemeView {
                seen_paint: sp.clone(),
                seen_layout: sl.clone(),
            },
            &mut state,
        );
        root.layout(Size::new(100.0, 100.0));
        let mut scene = RecordingScene::default();
        root.paint(&mut scene, FrameTime::ZERO);
        assert_eq!(seen_paint.get(), Some(1));

        // Flip the theme, repaint — the new accent is observed.
        root.set_theme(Box::new(TestTheme { accent: 2 }));
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut scene, FrameTime::ZERO);
        assert_eq!(seen_paint.get(), Some(2));
    }

    // --- Event-pass fixtures: a widget that mutates state on pointer-down. ---

    #[derive(Default)]
    struct ClickState {
        clicks: u32,
    }

    struct ButtonWidget;
    impl crate::widget::Widget for ButtonWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(40.0, 20.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut crate::event::EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event {
                match p.phase {
                    PointerPhase::Down => {
                        ctx.state_mut::<ClickState>().clicks += 1;
                        ctx.request_redraw();
                        ctx.capture_pointer();
                        return EventResult::Handled;
                    }
                    PointerPhase::Up | PointerPhase::Cancel => return EventResult::Handled,
                    PointerPhase::Move => {}
                }
            }
            EventResult::Ignored
        }
    }

    struct ButtonView;
    impl View<ClickState> for ButtonView {
        type Element = ButtonWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ButtonWidget {
            ButtonWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut ButtonWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    fn button_logic(_state: &mut ClickState) -> ButtonView {
        ButtonView
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(crate::event::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: crate::event::PointerButton::Primary,
        })
    }

    #[test]
    fn event_reaches_root_widget_and_mutates_state() {
        let mut root: RenderRoot<ClickState, ButtonView> = RenderRoot::new();
        let mut state = ClickState::default();
        root.rebuild(&mut button_logic, &mut state);
        root.layout(Size::new(200.0, 200.0));

        let outcome = root.event(&mut state, &pointer(PointerPhase::Down, 5.0, 5.0));
        assert!(outcome.handled);
        assert!(outcome.needs_redraw);
        assert_eq!(state.clicks, 1);
        // A captured Down opens the root gesture.
        assert!(root.is_pointer_captured());
    }

    #[test]
    fn event_before_build_is_a_benign_no_op() {
        let mut root: RenderRoot<ClickState, ButtonView> = RenderRoot::new();
        let mut state = ClickState::default();
        let outcome = root.event(&mut state, &pointer(PointerPhase::Down, 1.0, 1.0));
        assert_eq!(outcome, EventOutcome::default());
        assert_eq!(state.clicks, 0);
    }

    #[test]
    fn capture_releases_on_pointer_up() {
        let mut root: RenderRoot<ClickState, ButtonView> = RenderRoot::new();
        let mut state = ClickState::default();
        root.rebuild(&mut button_logic, &mut state);
        root.layout(Size::new(200.0, 200.0));

        root.event(&mut state, &pointer(PointerPhase::Down, 5.0, 5.0));
        assert!(root.is_pointer_captured());
        root.event(&mut state, &pointer(PointerPhase::Up, 5.0, 5.0));
        assert!(!root.is_pointer_captured());
    }

    #[test]
    fn take_change_flags_drains_accumulated_dirtiness() {
        let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
        let mut state = AppState {
            label: "x".to_string(),
        };
        root.rebuild(&mut app_logic, &mut state);
        // First build accumulated LAYOUT|PAINT.
        let flags = root.take_change_flags();
        assert!(flags.needs_layout());
        // Draining leaves it empty until the next rebuild.
        assert!(root.take_change_flags().is_empty());
    }

    #[test]
    fn has_pending_change_flags_peeks_without_draining() {
        // The frame-gate peek: observe pending dirtiness without
        // clearing it, so a skipped frame preserves the flags for the next
        // frame that actually runs.
        let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
        assert!(
            !root.has_pending_change_flags(),
            "a fresh root has nothing pending"
        );
        let mut state = AppState {
            label: "x".to_string(),
        };
        root.rebuild(&mut app_logic, &mut state);
        // First build accumulated LAYOUT|PAINT — the peek sees it...
        assert!(root.has_pending_change_flags());
        // ...and repeated peeks do NOT drain it.
        assert!(root.has_pending_change_flags());
        // Only `take_change_flags` drains.
        assert!(!root.take_change_flags().is_empty());
        assert!(!root.has_pending_change_flags());
    }

    #[test]
    fn set_theme_marks_layout_and_paint_pending() {
        // `set_theme` alone (no rebuild) must dirty layout/paint so a shell
        // gating on `take_change_flags` doesn't skip re-resolving theme-baked
        // widget state (e.g. Text's themed glyph color) on a bare theme swap.
        let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
        root.set_theme(Box::new(TestTheme { accent: 1 }));
        let flags = root.take_change_flags();
        assert!(flags.needs_layout());
        assert!(flags.needs_paint());

        // Draining clears it until the next `set_theme`/rebuild.
        assert!(root.take_change_flags().is_empty());
        root.set_theme(Box::new(TestTheme { accent: 2 }));
        let flags = root.take_change_flags();
        assert!(flags.needs_layout());
        assert!(flags.needs_paint());
    }

    // --- Window insets: pushed value reaches layout/paint contexts. ---

    use crate::insets::{EdgeInsets, WindowInsets};

    /// A root widget recording the `WindowInsets` it observed during layout and
    /// paint, proving the shell-pushed value threads through both contexts.
    struct InsetsWidget {
        seen_layout: std::rc::Rc<std::cell::Cell<Option<WindowInsets>>>,
        seen_paint: std::rc::Rc<std::cell::Cell<Option<WindowInsets>>>,
    }
    impl crate::widget::Widget for InsetsWidget {
        fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            self.seen_layout.set(Some(ctx.window_insets()));
            bc.constrain(Size::new(10.0, 10.0))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            self.seen_paint.set(Some(ctx.window_insets()));
        }
    }

    struct InsetsView {
        seen_layout: std::rc::Rc<std::cell::Cell<Option<WindowInsets>>>,
        seen_paint: std::rc::Rc<std::cell::Cell<Option<WindowInsets>>>,
    }
    impl View<AppState> for InsetsView {
        type Element = InsetsWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> InsetsWidget {
            InsetsWidget {
                seen_layout: self.seen_layout.clone(),
                seen_paint: self.seen_paint.clone(),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut InsetsWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    fn drive_insets_root(
        insets: Option<WindowInsets>,
    ) -> (Option<WindowInsets>, Option<WindowInsets>) {
        let seen_layout = std::rc::Rc::new(std::cell::Cell::new(None));
        let seen_paint = std::rc::Rc::new(std::cell::Cell::new(None));
        let mut root: RenderRoot<AppState, InsetsView> = RenderRoot::new();
        if let Some(insets) = insets {
            root.set_insets(insets);
        }
        let mut state = AppState::default();
        let sl = seen_layout.clone();
        let sp = seen_paint.clone();
        root.rebuild(
            &mut move |_s: &mut AppState| InsetsView {
                seen_layout: sl.clone(),
                seen_paint: sp.clone(),
            },
            &mut state,
        );
        root.layout(Size::new(100.0, 100.0));
        let mut scene = RecordingScene::default();
        root.paint(&mut scene, FrameTime::ZERO);
        (seen_layout.get(), seen_paint.get())
    }

    #[test]
    fn set_insets_threads_into_layout_and_paint() {
        let insets = WindowInsets::new(
            EdgeInsets::new(0.0, 24.0, 0.0, 34.0),
            EdgeInsets::new(0.0, 0.0, 0.0, 0.0),
        );
        let (layout, paint) = drive_insets_root(Some(insets));
        assert_eq!(layout, Some(insets));
        assert_eq!(paint, Some(insets));
    }

    #[test]
    fn no_insets_yields_zero_in_layout_and_paint() {
        let (layout, paint) = drive_insets_root(None);
        assert_eq!(layout, Some(WindowInsets::default()));
        assert_eq!(paint, Some(WindowInsets::default()));
    }

    #[test]
    fn set_insets_marks_layout_and_paint_pending() {
        // Mirrors `set_theme_marks_layout_and_paint_pending`: a bare inset push
        // (no rebuild) must dirty layout/paint so a shell gating on
        // `take_change_flags` relayouts a `SafeArea` when the insets move.
        let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
        root.set_insets(WindowInsets::new(
            EdgeInsets::new(0.0, 24.0, 0.0, 0.0),
            EdgeInsets::ZERO,
        ));
        let flags = root.take_change_flags();
        assert!(flags.needs_layout());
        assert!(flags.needs_paint());
        // Drained until the next change.
        assert!(root.take_change_flags().is_empty());
    }

    #[test]
    fn set_insets_no_op_when_unchanged_marks_nothing() {
        // The `PartialEq` no-op guard: re-pushing the current insets dirties
        // nothing, so a shell that forwards the platform insets every frame
        // never forces a needless relayout.
        let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
        let insets = WindowInsets::new(EdgeInsets::new(0.0, 24.0, 0.0, 34.0), EdgeInsets::ZERO);
        root.set_insets(insets);
        assert!(!root.take_change_flags().is_empty());
        // Same value again: no dirtiness.
        root.set_insets(insets);
        assert!(root.take_change_flags().is_empty());
        // A different value dirties again.
        root.set_insets(WindowInsets::default());
        assert!(!root.take_change_flags().is_empty());
    }

    // --- Presented-frame count: pushed value reaches the paint context, unset
    //     yields `None`, and — unlike theme/insets — the setter dirties nothing. ---

    /// A root widget recording the `presented_frames` count it observed during
    /// paint, proving the shell-pushed value threads through `PaintCtx`.
    struct PresentedWidget {
        seen_paint: std::rc::Rc<std::cell::Cell<Option<Option<u64>>>>,
    }
    impl crate::widget::Widget for PresentedWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(10.0, 10.0))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            self.seen_paint.set(Some(ctx.presented_frames()));
        }
    }

    struct PresentedView {
        seen_paint: std::rc::Rc<std::cell::Cell<Option<Option<u64>>>>,
    }
    impl View<AppState> for PresentedView {
        type Element = PresentedWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> PresentedWidget {
            PresentedWidget {
                seen_paint: self.seen_paint.clone(),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut PresentedWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    fn drive_presented_root(presented: Option<u64>) -> Option<u64> {
        let seen_paint = std::rc::Rc::new(std::cell::Cell::new(None));
        let mut root: RenderRoot<AppState, PresentedView> = RenderRoot::new();
        if let Some(presented) = presented {
            root.set_presented_frames(presented);
        }
        let mut state = AppState::default();
        let sp = seen_paint.clone();
        root.rebuild(
            &mut move |_s: &mut AppState| PresentedView {
                seen_paint: sp.clone(),
            },
            &mut state,
        );
        root.layout(Size::new(100.0, 100.0));
        let mut scene = RecordingScene::default();
        root.paint(&mut scene, FrameTime::ZERO);
        // Unwrap the "did paint run" outer Option; the inner is what the widget saw.
        seen_paint.get().expect("paint ran")
    }

    #[test]
    fn set_presented_frames_threads_into_paint() {
        assert_eq!(drive_presented_root(Some(12)), Some(12));
    }

    #[test]
    fn unset_presented_frames_yields_none_in_paint() {
        assert_eq!(drive_presented_root(None), None);
    }

    #[test]
    fn set_presented_frames_marks_no_change_flags() {
        // Unlike `set_theme`/`set_insets`, a presented-count push is a paint-only
        // observation — it must dirty NOTHING, so a monotonically ticking counter
        // never forces a relayout or (on mobile) keeps the frame gate perpetually
        // "Run" (the menu-idle behavior depends on this).
        let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
        let gen_before = root.semantics_generation();
        root.set_presented_frames(1);
        assert!(root.take_change_flags().is_empty());
        assert!(!root.has_pending_change_flags());
        // A second, changed push still dirties nothing.
        root.set_presented_frames(2);
        assert!(root.take_change_flags().is_empty());
        // And bumps no semantics generation (mirrors the no-dirty contract).
        assert_eq!(root.semantics_generation(), gen_before);
    }

    // Small test helper: does the boxed widget downcast to `W`?
    trait DowncastRefIs {
        fn downcast_ref_is<W: crate::widget::Widget>(&self) -> bool;
    }
    impl DowncastRefIs for dyn crate::widget::Widget {
        fn downcast_ref_is<W: crate::widget::Widget>(&self) -> bool {
            (self as &dyn std::any::Any).is::<W>()
        }
    }

    // --- Focus / IME surface fixtures: a root editable that focuses + publishes
    //     an IME surface on a `Down` in its left half, and blurs (no focus) on a
    //     `Down` in its right half. ---

    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::event::{EditingState, ImeState};

    struct ImeWidget;
    impl crate::widget::Widget for ImeWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(100.0, 100.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut crate::event::EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event {
                if p.phase == PointerPhase::Down && p.position.x < 50.0 {
                    ctx.request_focus();
                    ctx.publish_ime_state(ImeState {
                        active: true,
                        editing: EditingState {
                            text: "abc".to_string(),
                            selection_base: 3,
                            selection_extent: 3,
                            composing_base: -1,
                            composing_extent: -1,
                        },
                        caret: Some(kurbo::Rect::new(0.0, 0.0, 1.0, 12.0)),
                        content_type: Default::default(),
                    });
                    return EventResult::Handled;
                }
                if p.phase == PointerPhase::Down {
                    // Right-half tap: a blur (no focus request).
                    return EventResult::Handled;
                }
            }
            EventResult::Ignored
        }
    }

    struct ImeView;
    impl View<ClickState> for ImeView {
        type Element = ImeWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ImeWidget {
            ImeWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut ImeWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    fn ime_logic(_state: &mut ClickState) -> ImeView {
        ImeView
    }

    #[test]
    fn focus_and_ime_state_surface_and_clear_on_blur() {
        let mut root: RenderRoot<ClickState, ImeView> = RenderRoot::new();
        let mut state = ClickState::default();
        root.rebuild(&mut ime_logic, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // No focus / no IME surface initially.
        assert!(!root.is_focus_active());
        assert!(root.ime_state().is_none());

        // A left-half Down focuses the widget and publishes an IME surface.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(root.is_focus_active());
        let ime = root
            .ime_state()
            .expect("focused widget published an IME surface");
        assert!(ime.active);
        assert_eq!(ime.editing.text, "abc");

        // The published surface survives a rebuild (shell can query it between
        // frames).
        root.rebuild(&mut ime_logic, &mut state);
        assert!(root.ime_state().is_some());

        // A right-half Down is a blur: focus and the IME surface both clear.
        root.event(&mut state, &pointer(PointerPhase::Down, 80.0, 10.0));
        assert!(!root.is_focus_active());
        assert!(root.ime_state().is_none());
    }

    // --- Session release: the focus session must die with its owner -----------
    //
    // Two routes end a session without any user input reaching the root:
    //
    //  (a) a widget publishes an INACTIVE IME surface (the navigator's post-pop
    //      `cleared_ime_state`, `PatternSwitcher`'s equivalent, a `TextInput`
    //      turned disabled under a live focus), and
    //  (b) a reconciler tears the focused pod out of the tree (any generic
    //      unmount — `frust-widgets`' `cancel_active_children`/`teardown_child`),
    //      which raises `mark_focus_orphaned` because it has no `RenderRoot` to
    //      reach from a `BuildCtx` pass.
    //
    // Both must perform the SAME full release a blur does. Leaving either half
    // standing — `focus_active` true, or `ime_state` parked at `Some(inactive)` —
    // is what stranded a popped screen: `is_focus_active()` kept lying, the
    // shell's IME poll kept seeing a surface, and the next real focus
    // interaction started from a corrupt baseline.

    /// The navigator's cleared surface, spelled out here so the fixture below
    /// publishes exactly the shape `nav::navigator::cleared_ime_state` does
    /// (`frust-core` cannot name it — `frust-widgets` sits above this crate).
    fn cleared_surface() -> ImeState {
        ImeState {
            active: false,
            editing: EditingState {
                text: String::new(),
                selection_base: -1,
                selection_extent: -1,
                composing_base: -1,
                composing_extent: -1,
            },
            caret: None,
            content_type: Default::default(),
        }
    }

    /// A focused editable that publishes its active surface from paint (like a
    /// real field), and — once `clear` is raised — publishes the *inactive*
    /// surface from paint instead: the container-after-a-pop shape.
    struct PopImeWidget {
        clear: Rc<Cell<bool>>,
    }
    impl crate::widget::Widget for PopImeWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(100.0, 100.0))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            if self.clear.get() {
                ctx.publish_ime_state(cleared_surface());
            } else if ctx.has_focus() {
                ctx.publish_ime_state(PaintImeWidget::surface("abc"));
            }
        }
        fn event(&mut self, ctx: &mut crate::event::EventCtx, event: &InputEvent) -> EventResult {
            match event {
                InputEvent::Pointer(p) if p.phase == PointerPhase::Down => {
                    ctx.request_focus();
                    ctx.publish_ime_state(PaintImeWidget::surface("abc"));
                    EventResult::Handled
                }
                _ => EventResult::Ignored,
            }
        }
    }

    struct PopImeView {
        clear: Rc<Cell<bool>>,
    }
    impl View<ClickState> for PopImeView {
        type Element = PopImeWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> PopImeWidget {
            PopImeWidget {
                clear: self.clear.clone(),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut PopImeWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    #[test]
    fn inactive_paint_publish_releases_the_whole_session() {
        // (a) The pop shape. Before this, the paint take stored `Some(inactive)`
        // and never touched `focus_active`, so the session outlived the page.
        let clear = Rc::new(Cell::new(false));
        let mut logic = {
            let clear = clear.clone();
            move |_state: &mut ClickState| PopImeView {
                clear: clear.clone(),
            }
        };
        let mut root: RenderRoot<ClickState, PopImeView> = RenderRoot::new();
        let mut state = ClickState::default();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(100.0, 100.0));

        let mut scene = RecordingScene::default();
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut scene, FrameTime::ZERO);
        assert!(root.is_focus_active());
        assert!(root.ime_state().is_some_and(|s| s.active));
        let focused = root.focus_ime_generation();

        // The "pop": the next paint publishes the cleared surface.
        clear.set(true);
        root.paint(&mut scene, FrameTime::ZERO);
        assert!(
            !root.is_focus_active(),
            "an inactive publish ends the session, not just the surface"
        );
        assert_eq!(
            root.ime_state(),
            None,
            "the surface is dropped, never parked at Some(inactive)"
        );
        assert_eq!(
            root.focus_ime_generation(),
            focused.wrapping_add(1),
            "one release is exactly one edge"
        );

        // The widget keeps publishing the cleared surface every frame (a real
        // one-shot flag would not, but an idle screen must survive the worst
        // case): the paint take's `focus_active` guard makes each a no-op, so
        // the released session neither resurrects nor spins the edge.
        let released = root.focus_ime_generation();
        for _ in 0..30 {
            root.paint(&mut scene, FrameTime::ZERO);
        }
        assert!(!root.is_focus_active());
        assert!(root.ime_state().is_none());
        assert_eq!(
            root.focus_ime_generation(),
            released,
            "an inactive publish against an already-released root is inert"
        );
    }

    /// A view whose rebuild raises the generic-unmount orphan mark on demand —
    /// standing in for `frust-widgets`' reconcilers, which clear a focused
    /// `ChildPod` mid-diff and raise exactly this flag (this crate has no
    /// multi-child container of its own to diff).
    struct UnmountView {
        orphan: Rc<Cell<bool>>,
    }
    impl View<ClickState> for UnmountView {
        type Element = ImeWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ImeWidget {
            ImeWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut ImeWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            if self.orphan.get() {
                crate::event::mark_focus_orphaned();
                return ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            ChangeFlags::NONE
        }
    }

    #[test]
    fn generic_unmount_orphan_releases_the_whole_session() {
        // (b) The child-list-diff shape: no publish, no event — the focused
        // widget simply stops existing. Nothing self-corrects this on an idle
        // screen, which is why the reconciler's mark is drained here.
        let _ = crate::event::take_focus_orphaned();
        let orphan = Rc::new(Cell::new(false));
        let mut logic = {
            let orphan = orphan.clone();
            move |_state: &mut ClickState| UnmountView {
                orphan: orphan.clone(),
            }
        };
        let mut root: RenderRoot<ClickState, UnmountView> = RenderRoot::new();
        let mut state = ClickState::default();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(100.0, 100.0));

        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(root.is_focus_active());
        assert!(root.ime_state().is_some());
        let focused = root.focus_ime_generation();

        // The unmount rebuild.
        orphan.set(true);
        root.rebuild(&mut logic, &mut state);
        assert!(
            !root.is_focus_active(),
            "the root's focus mirror does not outlive the widget it mirrors"
        );
        assert!(root.ime_state().is_none());
        assert_eq!(
            root.focus_ime_generation(),
            focused.wrapping_add(1),
            "one orphaned focus path is exactly one edge"
        );

        // The mark was drained, so an ordinary rebuild afterwards is inert...
        let released = root.focus_ime_generation();
        orphan.set(false);
        root.rebuild(&mut logic, &mut state);
        assert_eq!(root.focus_ime_generation(), released);

        // ...and re-marking against an already-released root fires no edge
        // either (a stale `focused` flag torn down later must not spin it).
        orphan.set(true);
        root.rebuild(&mut logic, &mut state);
        assert_eq!(root.focus_ime_generation(), released);
        assert!(!root.is_focus_active());
    }

    // --- The focus/IME EDGE generation ---------------------------------------
    //
    // `focus_ime_generation` is the shell-facing edge behind the mobile frame
    // gate's `FrameInputs::focus_or_ime_changed`: a shell caches the value and
    // runs a frame when it moves. Two properties make that safe, and both are
    // pinned below: EVERY real transition moves it (or a focus change strands
    // unpainted), and NO same-value write moves it (or a focused screen forces
    // a frame every vsync — the level-input behavior this replaced, measured at
    // 62–120 fps on a static focused screen).

    /// A widget that focuses on `Down`, releases focus on any `Key`, and — the
    /// point of the fixture — re-publishes an IME surface from its **paint**
    /// pass on every frame, reading the text from a shared cell so a test can
    /// make a republish genuinely change (or genuinely not).
    struct PaintImeWidget {
        published: Rc<RefCell<String>>,
    }
    impl PaintImeWidget {
        fn surface(text: &str) -> ImeState {
            ImeState {
                active: true,
                editing: EditingState {
                    text: text.to_string(),
                    selection_base: 0,
                    selection_extent: 0,
                    composing_base: -1,
                    composing_extent: -1,
                },
                caret: Some(kurbo::Rect::new(0.0, 0.0, 1.0, 12.0)),
                content_type: Default::default(),
            }
        }
    }
    impl crate::widget::Widget for PaintImeWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(100.0, 100.0))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            ctx.publish_ime_state(Self::surface(&self.published.borrow()));
        }
        fn event(&mut self, ctx: &mut crate::event::EventCtx, event: &InputEvent) -> EventResult {
            match event {
                InputEvent::Pointer(p) if p.phase == PointerPhase::Down => {
                    ctx.request_focus();
                    EventResult::Handled
                }
                InputEvent::Key(_) => {
                    ctx.release_focus();
                    EventResult::Handled
                }
                _ => EventResult::Ignored,
            }
        }
    }

    struct PaintImeView {
        published: Rc<RefCell<String>>,
    }
    impl View<ClickState> for PaintImeView {
        type Element = PaintImeWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> PaintImeWidget {
            PaintImeWidget {
                published: self.published.clone(),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut PaintImeWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    fn key_event() -> InputEvent {
        InputEvent::Key(crate::event::KeyEvent {
            key: crate::event::Key::Named(crate::event::NamedKey::Enter),
            modifiers: crate::event::Modifiers::default(),
            repeat: false,
        })
    }

    #[test]
    fn focus_ime_generation_moves_on_every_pointer_transition_only() {
        let mut root: RenderRoot<ClickState, ImeView> = RenderRoot::new();
        let mut state = ClickState::default();
        root.rebuild(&mut ime_logic, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // Idle: a rebuild/layout touches neither focus nor the IME surface.
        let idle = root.focus_ime_generation();
        root.rebuild(&mut ime_logic, &mut state);
        assert_eq!(
            root.focus_ime_generation(),
            idle,
            "a rebuild is not an edge"
        );

        // Focus gained + IME surface published (one transition for a shell,
        // however many field writes it took).
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        let focused = root.focus_ime_generation();
        assert_ne!(
            focused, idle,
            "focus + IME publish must move the generation"
        );

        // The SAME tap again, on the already-focused widget publishing the
        // identical surface: no state moved, so no edge. This is the case that
        // decides whether a live text field forces a frame per vsync.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert_eq!(
            root.focus_ime_generation(),
            focused,
            "a same-value focus/IME write must not spin the edge"
        );

        // Blur: focus cleared and the surface dropped — a real transition.
        root.event(&mut state, &pointer(PointerPhase::Down, 80.0, 10.0));
        let blurred = root.focus_ime_generation();
        assert_ne!(blurred, focused, "a blur must move the generation");

        // Blur while already blurred (a tap on inert chrome — the commonest
        // event there is) writes `false`/`None` back over `false`/`None`.
        root.event(&mut state, &pointer(PointerPhase::Down, 80.0, 20.0));
        assert_eq!(
            root.focus_ime_generation(),
            blurred,
            "blurring an already-blurred root must not move the generation"
        );
    }

    #[test]
    fn focus_ime_generation_ignores_an_unchanged_paint_republish() {
        // The paint pass re-publishes the focused widget's IME surface on EVERY
        // frame (that is how a rebuild-applied controlled change refreshes the
        // shell-facing state). If that unconditional write moved the
        // generation, the frame gate's edge would fire every single frame for
        // the whole life of a focus session — exactly the per-vsync forcing the
        // edge exists to remove.
        let published = Rc::new(RefCell::new("abc".to_string()));
        let mut logic = {
            let published = published.clone();
            move |_state: &mut ClickState| PaintImeView {
                published: published.clone(),
            }
        };
        let mut root: RenderRoot<ClickState, PaintImeView> = RenderRoot::new();
        let mut state = ClickState::default();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // Focus the field, then let it paint: the first paint publishes.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        let mut scene = RecordingScene::default();
        root.paint(&mut scene, FrameTime::ZERO);
        let steady = root.focus_ime_generation();
        assert_eq!(
            root.ime_state(),
            Some(PaintImeWidget::surface("abc")),
            "the paint pass published the focused widget's surface"
        );

        // 120 further frames of the same focused, unchanged field: the caret
        // blinks, nothing else moves. Not one edge.
        for _ in 0..120 {
            root.paint(&mut scene, FrameTime::ZERO);
        }
        assert_eq!(
            root.focus_ime_generation(),
            steady,
            "an unchanged paint republish must never move the generation"
        );

        // A real change (the app applied a controlled edit) publishes a
        // different surface: exactly one edge, then quiet again.
        *published.borrow_mut() = "abcd".to_string();
        root.paint(&mut scene, FrameTime::ZERO);
        let edited = root.focus_ime_generation();
        assert_ne!(edited, steady, "a changed republish IS an edge");
        for _ in 0..10 {
            root.paint(&mut scene, FrameTime::ZERO);
        }
        assert_eq!(
            root.focus_ime_generation(),
            edited,
            "the session goes quiet again at the new value"
        );
    }

    #[test]
    fn focus_ime_generation_moves_on_a_focus_release_only_once() {
        // The Key/Ime/Scroll arm of the root focus path: a dispatch that
        // RELEASES focus clears both the flag and the published surface.
        let published = Rc::new(RefCell::new("abc".to_string()));
        let mut logic = {
            let published = published.clone();
            move |_state: &mut ClickState| PaintImeView {
                published: published.clone(),
            }
        };
        let mut root: RenderRoot<ClickState, PaintImeView> = RenderRoot::new();
        let mut state = ClickState::default();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        let mut scene = RecordingScene::default();
        root.paint(&mut scene, FrameTime::ZERO);
        let focused = root.focus_ime_generation();
        assert!(root.is_focus_active());

        // A key that releases focus: one edge.
        root.event(&mut state, &key_event());
        let released = root.focus_ime_generation();
        assert!(!root.is_focus_active());
        assert!(root.ime_state().is_none());
        assert_ne!(
            released, focused,
            "a focus release must move the generation"
        );

        // A second release against an already-released root: no edge. (The
        // paint pass republishes nothing now — the paint-take arm only accepts
        // a publish while focus is active.)
        root.event(&mut state, &key_event());
        root.paint(&mut scene, FrameTime::ZERO);
        assert_eq!(
            root.focus_ime_generation(),
            released,
            "releasing an already-released focus must not move the generation"
        );
    }

    // --- Semantics: stable ids + accessibility action routing ---
    //
    // Fixtures: an accessibility-visible button (fire-on-up-inside, contributes a
    // `Role::Button` node) and a checkbox variant (`Role::CheckBox`), plus a
    // labelled leaf used to prove id stability survives a pod relocation.

    use accesskit::{Action, NodeId, Role};

    /// A fire-on-up-inside button that also contributes a semantics node — the
    /// end-to-end target for `perform_accessibility_action(Click)`.
    struct A11yButtonWidget;
    impl crate::widget::Widget for A11yButtonWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(40.0, 20.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event {
                match p.phase {
                    PointerPhase::Down => {
                        ctx.capture_pointer();
                        return EventResult::Handled;
                    }
                    PointerPhase::Up => {
                        let size = ctx.size();
                        let inside = p.position.x >= 0.0
                            && p.position.y >= 0.0
                            && p.position.x <= size.width
                            && p.position.y <= size.height;
                        if inside {
                            ctx.state_mut::<ClickState>().clicks += 1;
                            ctx.request_redraw();
                        }
                        return EventResult::Handled;
                    }
                    _ => {}
                }
            }
            EventResult::Ignored
        }
        fn semantics(&self, ctx: &mut SemanticsCtx) {
            ctx.push_node(Role::Button, |n| n.set_label("Go"));
        }
    }

    struct A11yButtonView;
    impl View<ClickState> for A11yButtonView {
        type Element = A11yButtonWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> A11yButtonWidget {
            A11yButtonWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _el: &mut A11yButtonWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    fn a11y_button_logic(_state: &mut ClickState) -> A11yButtonView {
        A11yButtonView
    }

    fn button_node_id(update: &SemanticsUpdate, role: Role) -> NodeId {
        update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == role)
            .map(|(id, _)| *id)
            .unwrap_or_else(|| panic!("a {role:?} node is present"))
    }

    #[test]
    fn semantics_ids_are_stable_across_frames() {
        let mut root: RenderRoot<ClickState, A11yButtonView> = RenderRoot::new();
        let mut state = ClickState::default();
        root.rebuild(&mut a11y_button_logic, &mut state);
        root.layout(Size::new(200.0, 200.0));

        let first = button_node_id(&root.semantics(), Role::Button);
        // Re-run rebuild+layout+semantics several times: the button keeps its id.
        for _ in 0..3 {
            root.rebuild(&mut a11y_button_logic, &mut state);
            root.layout(Size::new(200.0, 200.0));
            assert_eq!(
                button_node_id(&root.semantics(), Role::Button),
                first,
                "the same widget must keep its NodeId across frames"
            );
        }
        // The window root is the reserved constant id.
        assert_eq!(root.semantics().root, ROOT_NODE_ID);
    }

    #[test]
    fn semantics_ids_survive_a_pod_relocation() {
        // A keyed reorder relocates the whole `ChildPod` (preserving its cached
        // semantics id); simulate that here by swapping two pods in place and
        // asserting each labelled node keeps its id despite changing position.
        struct LabeledLeaf {
            label: &'static str,
        }
        impl crate::widget::Widget for LabeledLeaf {
            fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
                bc.constrain(Size::new(10.0, 10.0))
            }
            fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
            fn semantics(&self, ctx: &mut SemanticsCtx) {
                let label = self.label;
                ctx.push_node(Role::Label, |n| n.set_label(label));
            }
        }

        let mut pods = vec![
            crate::widget::ChildPod::new(Box::new(LabeledLeaf { label: "A" })),
            crate::widget::ChildPod::new(Box::new(LabeledLeaf { label: "B" })),
        ];

        // Collect (label -> id) for a given pod order. `SemanticsCtx` is
        // crate-private, so this drives the pods directly — the same allocation
        // path `RenderRoot::semantics` uses.
        let collect = |pods: &[crate::widget::ChildPod]| {
            let mut ctx = SemanticsCtx::new(Size::new(100.0, 100.0), 2);
            for pod in pods {
                pod.semantics_child(&mut ctx);
            }
            let update = ctx.finish(ROOT_NODE_ID);
            update
                .nodes
                .iter()
                .filter(|(id, _)| *id != ROOT_NODE_ID)
                .map(|(id, n)| (n.label().unwrap().to_string(), *id))
                .collect::<Vec<_>>()
        };

        let before = collect(&pods);
        // Relocate: swap the pods (the pods themselves, with their cached ids,
        // move — mirroring the keyed reconciler's `take`-and-reorder).
        pods.swap(0, 1);
        let after = collect(&pods);

        for (label, id) in &before {
            let relocated = after.iter().find(|(l, _)| l == label).unwrap().1;
            assert_eq!(
                *id, relocated,
                "widget {label:?} must keep its NodeId across the reorder"
            );
        }
        // And the reorder actually changed positions (A now second).
        assert_eq!(after[0].0, "B");
        assert_eq!(after[1].0, "A");
    }

    #[test]
    fn semantics_full_update_assembles_window_and_child() {
        let mut root: RenderRoot<ClickState, A11yButtonView> = RenderRoot::new();
        let mut state = ClickState::default();
        root.rebuild(&mut a11y_button_logic, &mut state);
        root.layout(Size::new(200.0, 200.0));

        let update = root.semantics();
        // Window root + the button.
        assert_eq!(update.nodes.len(), 2);
        assert_eq!(update.root, ROOT_NODE_ID);
        let root_node = update
            .nodes
            .iter()
            .find(|(id, _)| *id == update.root)
            .unwrap();
        assert_eq!(root_node.1.role(), Role::Window);
        let button = button_node_id(&update, Role::Button);
        assert_eq!(
            root_node.1.children(),
            &[button],
            "the button attaches under the window root"
        );
        // Nothing focused → the adapter-facing focus id defaults to the root.
        assert!(update.focus.is_none());
        assert_eq!(update.focus_id(), ROOT_NODE_ID);
    }

    #[test]
    fn perform_click_action_activates_a_button() {
        let mut root: RenderRoot<ClickState, A11yButtonView> = RenderRoot::new();
        let mut state = ClickState::default();
        root.rebuild(&mut a11y_button_logic, &mut state);
        root.layout(Size::new(200.0, 200.0));

        let button = button_node_id(&root.semantics(), Role::Button);
        let outcome = root.perform_accessibility_action(&mut state, button, Action::Click);
        assert!(outcome.handled, "the synthesized Down+Up was handled");
        assert!(outcome.needs_redraw);
        assert_eq!(state.clicks, 1, "Click synthesized a real up-inside tap");

        // An unknown node id is a benign no-op.
        let outcome = root.perform_accessibility_action(&mut state, NodeId(999_999), Action::Click);
        assert_eq!(outcome, EventOutcome::default());
        assert_eq!(state.clicks, 1);

        // An unmodelled action is ignored.
        let outcome = root.perform_accessibility_action(&mut state, button, Action::ScrollDown);
        assert_eq!(outcome, EventOutcome::default());
        assert_eq!(state.clicks, 1);
    }

    #[test]
    fn perform_click_action_toggles_a_checkbox() {
        // A checkbox-shaped widget (`Role::CheckBox`) reached through the same
        // synthetic-pointer path — proving Click drives any fire-on-up-inside
        // control, not just buttons.
        struct CheckboxWidget;
        impl crate::widget::Widget for CheckboxWidget {
            fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
                bc.constrain(Size::new(24.0, 24.0))
            }
            fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
            fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
                if let InputEvent::Pointer(p) = event {
                    match p.phase {
                        PointerPhase::Down => {
                            ctx.capture_pointer();
                            return EventResult::Handled;
                        }
                        PointerPhase::Up => {
                            let size = ctx.size();
                            if p.position.x >= 0.0
                                && p.position.y >= 0.0
                                && p.position.x <= size.width
                                && p.position.y <= size.height
                            {
                                ctx.state_mut::<ClickState>().clicks += 1;
                            }
                            return EventResult::Handled;
                        }
                        _ => {}
                    }
                }
                EventResult::Ignored
            }
            fn semantics(&self, ctx: &mut SemanticsCtx) {
                ctx.push_node(Role::CheckBox, |n| n.set_label("Agree"));
            }
        }
        struct CheckboxView;
        impl View<ClickState> for CheckboxView {
            type Element = CheckboxWidget;
            fn build(&self, _ctx: &mut BuildCtx<'_>) -> CheckboxWidget {
                CheckboxWidget
            }
            fn rebuild(
                &self,
                _p: &Self,
                _e: &mut CheckboxWidget,
                _c: &mut BuildCtx<'_>,
            ) -> ChangeFlags {
                ChangeFlags::NONE
            }
        }

        let mut root: RenderRoot<ClickState, CheckboxView> = RenderRoot::new();
        let mut state = ClickState::default();
        root.rebuild(&mut |_| CheckboxView, &mut state);
        root.layout(Size::new(200.0, 200.0));

        let cb = button_node_id(&root.semantics(), Role::CheckBox);
        root.perform_accessibility_action(&mut state, cb, Action::Click);
        assert_eq!(state.clicks, 1, "Click toggled the checkbox once");
    }

    #[test]
    fn perform_focus_action_claims_focus_and_clears_capture() {
        // Two focus-claiming, fire-on-up-inside buttons in a container. A11y
        // `Focus` on B must claim focus for B *and* release the capture the
        // synthesized `Down` opened — the CRITICAL leak this regresses: without
        // the trailing `Cancel`, B stayed captured and swallowed every later
        // pointer event, so a tap on A never reached A.

        #[derive(Default)]
        struct FocusState {
            a_press: u32,
            b_press: u32,
            b_move: u32,
        }

        #[derive(Clone, Copy)]
        enum Btn {
            A,
            B,
        }

        /// A button that opts into both recorded paths (capture + focus) on
        /// `Down` and fires its press only on `Up`-inside — never on `Cancel`.
        struct FocusButton {
            id: Btn,
        }
        impl crate::widget::Widget for FocusButton {
            fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
                bc.constrain(Size::new(40.0, 20.0))
            }
            fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
            fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
                let InputEvent::Pointer(p) = event else {
                    return EventResult::Ignored;
                };
                match p.phase {
                    PointerPhase::Down => {
                        ctx.capture_pointer();
                        ctx.request_focus();
                        EventResult::Handled
                    }
                    PointerPhase::Move => {
                        if let Btn::B = self.id {
                            ctx.state_mut::<FocusState>().b_move += 1;
                        }
                        EventResult::Handled
                    }
                    PointerPhase::Up => {
                        let size = ctx.size();
                        let inside = p.position.x >= 0.0
                            && p.position.y >= 0.0
                            && p.position.x <= size.width
                            && p.position.y <= size.height;
                        if inside {
                            match self.id {
                                Btn::A => ctx.state_mut::<FocusState>().a_press += 1,
                                Btn::B => ctx.state_mut::<FocusState>().b_press += 1,
                            }
                        }
                        EventResult::Handled
                    }
                    // A `Cancel` clears without firing on_press and never touches
                    // state — the contract the Focus action's trailing Cancel rides.
                    PointerPhase::Cancel => EventResult::Handled,
                }
            }
            fn semantics(&self, ctx: &mut SemanticsCtx) {
                let label = match self.id {
                    Btn::A => "A",
                    Btn::B => "B",
                };
                ctx.push_node(Role::Button, |n| n.set_label(label));
            }
        }

        /// A minimal two-child container mirroring `frust-widgets`'
        /// `route_event`: a captured gesture goes straight to the active child
        /// (auto-released on `Up`/`Cancel`), otherwise the event is hit-tested to
        /// the child under it.
        struct TwoButtons {
            a: crate::widget::ChildPod,
            b: crate::widget::ChildPod,
        }
        impl crate::widget::Widget for TwoButtons {
            fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
                self.a.layout_child(ctx, bc);
                self.a.set_origin(Point::new(0.0, 0.0));
                self.b.layout_child(ctx, bc);
                self.b.set_origin(Point::new(0.0, 30.0));
                bc.max()
            }
            fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
                self.a.paint_child(ctx, scene);
                self.b.paint_child(ctx, scene);
            }
            fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
                let releases = matches!(
                    event,
                    InputEvent::Pointer(p)
                        if matches!(p.phase, PointerPhase::Up | PointerPhase::Cancel)
                );
                // Capture fast-path: a recorded active child receives every event
                // until it releases on Up/Cancel, bypassing the hit test entirely.
                if self.a.is_active() {
                    let r = self.a.event_child(ctx, event);
                    if releases {
                        self.a.set_active(false);
                    }
                    return r;
                }
                if self.b.is_active() {
                    let r = self.b.event_child(ctx, event);
                    if releases {
                        self.b.set_active(false);
                    }
                    return r;
                }
                // Fresh event: route to the child under the point.
                let pos = event.position();
                if self.a.contains(pos) {
                    return self.a.event_child(ctx, event);
                }
                if self.b.contains(pos) {
                    return self.b.event_child(ctx, event);
                }
                EventResult::Ignored
            }
            fn semantics(&self, ctx: &mut SemanticsCtx) {
                self.a.semantics_child(ctx);
                self.b.semantics_child(ctx);
            }
        }

        struct TwoButtonsView;
        impl View<FocusState> for TwoButtonsView {
            type Element = TwoButtons;
            fn build(&self, _ctx: &mut BuildCtx<'_>) -> TwoButtons {
                TwoButtons {
                    a: crate::widget::ChildPod::new(Box::new(FocusButton { id: Btn::A })),
                    b: crate::widget::ChildPod::new(Box::new(FocusButton { id: Btn::B })),
                }
            }
            fn rebuild(
                &self,
                _p: &Self,
                _e: &mut TwoButtons,
                _c: &mut BuildCtx<'_>,
            ) -> ChangeFlags {
                ChangeFlags::NONE
            }
        }

        let mut root: RenderRoot<FocusState, TwoButtonsView> = RenderRoot::new();
        let mut state = FocusState::default();
        root.rebuild(&mut |_| TwoButtonsView, &mut state);
        root.layout(Size::new(200.0, 200.0));

        // B's semantics node (label "B") is the a11y Focus target.
        let b_id = root
            .semantics()
            .nodes
            .iter()
            .find(|(_, n)| n.label().is_some_and(|l| l == "B"))
            .map(|(id, _)| *id)
            .expect("button B contributes a semantics node");

        // A11y `Focus` on B: claims the focus session, fires no on_press, and —
        // crucially — leaves nothing captured (the Down+Cancel shape).
        root.perform_accessibility_action(&mut state, b_id, Action::Focus);
        assert!(root.is_focus_active(), "Focus opened the focus session");
        assert!(
            !root.is_pointer_captured(),
            "the trailing Cancel released the capture the Focus Down opened"
        );
        assert_eq!(
            state.b_press, 0,
            "Focus (Down+Cancel) must not fire B's on_press"
        );

        // B is not stuck-captured: a `Move` outside both buttons is ignored. Were
        // B still captured, the capture fast-path would route this to B regardless
        // of position (b_move would tick).
        let outside = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Move,
            position: Point::new(100.0, 100.0),
            button: PointerButton::Primary,
        });
        root.event(&mut state, &outside);
        assert_eq!(state.b_move, 0, "no leaked capture: B saw no stray Move");

        // A real Down+Up on A activates A exactly once and never reaches B.
        let at_a = |phase| {
            InputEvent::Pointer(PointerEvent {
                phase,
                position: Point::new(20.0, 10.0),
                button: PointerButton::Primary,
            })
        };
        root.event(&mut state, &at_a(PointerPhase::Down));
        root.event(&mut state, &at_a(PointerPhase::Up));
        assert_eq!(state.a_press, 1, "A fired once from its own tap");
        assert_eq!(
            state.b_press, 0,
            "B never fired — its capture never leaked onto A's tap"
        );
    }

    #[test]
    fn semantics_if_changed_gates_on_generation() {
        let mut root: RenderRoot<ClickState, A11yButtonView> = RenderRoot::new();
        let mut state = ClickState::default();
        // First build bumps the generation from 0.
        root.rebuild(&mut a11y_button_logic, &mut state);
        root.layout(Size::new(200.0, 200.0));

        let generation = root.semantics_generation();
        assert!(generation > 0);
        // A shell that already pushed `generation` sees no change.
        assert!(root.semantics_if_changed(generation).is_none());
        // A stale generation triggers a fresh pull.
        assert!(root.semantics_if_changed(generation - 1).is_some());

        // A theme swap marks the tree semantics-dirty.
        root.set_theme(Box::new(0u32));
        assert!(root.semantics_generation() > generation);
        assert!(root.semantics_if_changed(generation).is_some());
    }

    #[test]
    fn orientation_from_size_is_portrait_when_taller_than_wide() {
        assert_eq!(
            Orientation::from_size(Size::new(400.0, 800.0)),
            Orientation::Portrait
        );
    }

    #[test]
    fn orientation_from_size_is_landscape_when_wider_than_tall() {
        assert_eq!(
            Orientation::from_size(Size::new(800.0, 400.0)),
            Orientation::Landscape
        );
    }

    #[test]
    fn orientation_from_size_square_reads_as_portrait() {
        // Height >= width is the derivation rule (see `Orientation::from_size`'s
        // doc); an exact square satisfies `>=` and must not panic/ambiguously
        // resolve, so this is pinned explicitly rather than left implicit.
        assert_eq!(
            Orientation::from_size(Size::new(500.0, 500.0)),
            Orientation::Portrait
        );
    }

    #[test]
    fn window_metrics_new_derives_orientation_from_size() {
        let insets = WindowInsets::default();
        let portrait = WindowMetrics::new(Size::new(390.0, 844.0), 3.0, insets);
        assert_eq!(portrait.orientation, Orientation::Portrait);
        assert_eq!(portrait.size, Size::new(390.0, 844.0));
        assert_eq!(portrait.scale, 3.0);
        assert_eq!(portrait.insets, insets);

        let landscape = WindowMetrics::new(Size::new(844.0, 390.0), 3.0, insets);
        assert_eq!(landscape.orientation, Orientation::Landscape);

        let square = WindowMetrics::new(Size::new(500.0, 500.0), 2.0, insets);
        assert_eq!(square.orientation, Orientation::Portrait);
    }

    // --- Deferred state-bearing callbacks: the `InputEvent::Housekeeping` flush
    //     `RenderRoot::rebuild` dispatches. ---

    /// App state for the flush tests.
    #[derive(Default)]
    struct FlushState {
        /// How many deferred callbacks have run.
        flushes: u32,
        /// How many more times a running callback re-queues itself — the knob the
        /// chained/capped tests turn.
        chain_left: u32,
        /// The `flushes` value each `app_logic` run observed, in order. This is
        /// what proves the rebuild re-runs `app_logic` *after* a flush rather than
        /// shipping the now-stale pre-flush view.
        observed: Vec<u32>,
    }

    /// The navigator's deferred-callback shape reduced to one leaf: a shared
    /// `Rc<Cell<u32>>` op queue (the `NavigatorController` analog) is drained
    /// during the state-free [`View::rebuild`], which can therefore only *queue*
    /// the callback and raise the flush mark; [`crate::widget::Widget::event`]
    /// runs it when the broadcast arrives, where `&mut State` finally exists.
    struct FlushView {
        ops: std::rc::Rc<Cell<u32>>,
    }

    struct FlushWidget {
        ops: std::rc::Rc<Cell<u32>>,
        queued: u32,
    }

    impl FlushWidget {
        fn drain_ops(&mut self) {
            let ops = self.ops.replace(0);
            if ops > 0 {
                self.queued += ops;
                crate::event::mark_pending_result_flush();
            }
        }
    }

    impl View<FlushState> for FlushView {
        type Element = FlushWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> FlushWidget {
            let mut widget = FlushWidget {
                ops: self.ops.clone(),
                queued: 0,
            };
            widget.drain_ops();
            widget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut FlushWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.ops = self.ops.clone();
            element.drain_ops();
            ChangeFlags::NONE
        }
    }

    impl crate::widget::Widget for FlushWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(10.0, 10.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if !event.is_broadcast() || self.queued == 0 {
                return EventResult::Ignored;
            }
            let queued = std::mem::take(&mut self.queued);
            let state = ctx.state_mut::<FlushState>();
            for _ in 0..queued {
                state.flushes += 1;
                if state.chain_left > 0 {
                    state.chain_left -= 1;
                    self.ops.set(self.ops.get() + 1);
                }
            }
            EventResult::Ignored
        }
    }

    fn flush_logic(ops: std::rc::Rc<Cell<u32>>) -> impl FnMut(&mut FlushState) -> FlushView {
        move |state: &mut FlushState| {
            state.observed.push(state.flushes);
            FlushView { ops: ops.clone() }
        }
    }

    #[test]
    fn a_queued_callback_flushes_and_re_diffs_inside_one_rebuild() {
        let ops = std::rc::Rc::new(Cell::new(0u32));
        let mut root: RenderRoot<FlushState, FlushView> = RenderRoot::new();
        let mut app = flush_logic(ops.clone());
        let mut state = FlushState::default();

        root.rebuild(&mut app, &mut state);
        assert_eq!(state.flushes, 0);
        assert_eq!(
            state.observed,
            vec![0],
            "nothing queued ⇒ exactly one app_logic run, no broadcast"
        );

        // Queue one op — the `NavigatorController::pop_with_result` analog.
        state.observed.clear();
        ops.set(1);
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            state.flushes, 1,
            "the queued callback ran inside this rebuild — no event was dispatched \
             by anyone but the rebuild itself"
        );
        assert_eq!(
            state.observed,
            vec![0, 1],
            "app_logic re-ran after the flush and saw the post-callback state, so \
             the view this frame ships is not the stale pre-flush one"
        );
        assert!(
            !crate::event::take_pending_result_flush(),
            "the mark was consumed; nothing is owed to a later frame"
        );
    }

    #[test]
    fn a_runaway_callback_chain_is_capped_and_deferred_to_the_next_frame() {
        let ops = std::rc::Rc::new(Cell::new(0u32));
        let mut root: RenderRoot<FlushState, FlushView> = RenderRoot::new();
        let mut app = flush_logic(ops.clone());
        // Far more chaining than the cap allows: unbounded, this rebuild would
        // never return. Reaching the assertions below at all is the no-spin proof.
        let mut state = FlushState {
            chain_left: 100,
            ..Default::default()
        };
        root.rebuild(&mut app, &mut state);

        ops.set(1);
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            state.flushes, MAX_PENDING_RESULT_FLUSH_PASSES as u32,
            "exactly the cap's worth of flush passes, then stop"
        );

        // The remainder is owed, not lost: the mark still stands and the next
        // paint asks for the follow-up frame that will finish it.
        root.layout(Size::new(50.0, 50.0));
        let mut scene = RecordingScene::default();
        let outcome = root.paint(&mut scene, FrameTime::ZERO);
        assert!(
            outcome.needs_frame,
            "hitting the cap requests one more frame, so a dirty-driven shell \
             wakes instead of waiting for input"
        );
        assert!(
            !outcome.needs_frame_paced_only,
            "a deferred flush is not a cosmetic loop — the mobile frame gate must \
             not throttle it"
        );

        // That next frame picks up exactly where the capped one left off.
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            state.flushes,
            2 * MAX_PENDING_RESULT_FLUSH_PASSES as u32,
            "the deferred remainder resumed on the following frame"
        );

        // Leave this thread's flag clean for anything else in the binary.
        let _ = crate::event::take_pending_result_flush();
    }

    /// The redraw-only flush shape: a widget that queues a callback exactly like
    /// [`FlushView`] above, but whose broadcast handler touches **no** state at
    /// all — it only calls [`EventCtx::request_redraw`]. `frust-widgets`' gesture
    /// long-press latch is the shipped instance (its `on_long_press` consumer may
    /// mutate nothing the view diff can see), and the widget's own
    /// `ctx.request_redraw()` after firing is then the whole wake signal.
    struct RedrawOnlyView {
        ops: std::rc::Rc<Cell<u32>>,
    }

    struct RedrawOnlyWidget {
        ops: std::rc::Rc<Cell<u32>>,
        queued: bool,
    }

    impl RedrawOnlyWidget {
        fn drain_ops(&mut self) {
            if self.ops.replace(0) > 0 {
                self.queued = true;
                crate::event::mark_pending_result_flush();
            }
        }
    }

    impl View<()> for RedrawOnlyView {
        type Element = RedrawOnlyWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> RedrawOnlyWidget {
            let mut widget = RedrawOnlyWidget {
                ops: self.ops.clone(),
                queued: false,
            };
            widget.drain_ops();
            widget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut RedrawOnlyWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.ops = self.ops.clone();
            element.drain_ops();
            // The whole point: the re-diff after the flush reports nothing, so
            // the dispatch's own outcome is the only wake signal there is.
            ChangeFlags::NONE
        }
    }

    impl crate::widget::Widget for RedrawOnlyWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(10.0, 10.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if event.is_broadcast() && std::mem::take(&mut self.queued) {
                // No `state_mut`, no signal, no view-visible change — a repaint
                // request and nothing else.
                ctx.request_redraw();
            }
            EventResult::Ignored
        }
    }

    #[test]
    fn a_redraw_only_flushed_callback_wakes_both_loop_styles() {
        let ops = std::rc::Rc::new(Cell::new(0u32));
        let mut root: RenderRoot<(), RedrawOnlyView> = RenderRoot::new();
        let mut app = |_state: &mut ()| RedrawOnlyView { ops: ops.clone() };
        let mut state = ();

        // Settle the first build so the assertions below observe only the flush.
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(50.0, 50.0));
        let mut scene = RecordingScene::default();
        let settled = root.paint(&mut scene, FrameTime::ZERO);
        assert!(!settled.needs_frame, "nothing queued ⇒ the tree is at rest");
        let _ = root.take_change_flags();

        // Queue the redraw-only callback (the gesture long-press latch analog: a
        // prior pass marks, this rebuild flushes).
        ops.set(1);
        let flags = root.rebuild(&mut app, &mut state);
        assert!(
            flags.needs_paint(),
            "the broadcast's `needs_redraw` folds into the rebuild's flags even \
             though the re-diff saw no view change"
        );
        assert!(
            root.has_pending_change_flags(),
            "PAINT reached `pending`, which is the input the mobile frame gate \
             reads to decide the next tick runs at all"
        );

        let outcome = root.paint(&mut scene, FrameTime::ZERO);
        assert!(
            outcome.needs_frame,
            "the same wake surfaces as `needs_frame`, which is how the desktop \
             `ControlFlow::Wait` loop schedules a frame with no input pending"
        );
        assert!(
            !outcome.needs_frame_paced_only,
            "a flushed callback's repaint is not a cosmetic loop — the mobile \
             frame gate must not throttle it"
        );
        assert!(
            !crate::event::take_pending_result_flush(),
            "the mark was consumed; nothing is owed to a later frame"
        );

        // And it settles: the next frame asks for nothing, so neither loop spins.
        let _ = root.take_change_flags();
        root.rebuild(&mut app, &mut state);
        let settled = root.paint(&mut scene, FrameTime::ZERO);
        assert!(
            !settled.needs_frame,
            "one wake, not a perpetual one — the flush is over"
        );
        assert!(!root.has_pending_change_flags());
    }

    // --- Hover: the claim pipeline -------------------------------------------
    //
    // Hover has no Enter/Leave phase to lean on (adding one to `PointerPhase`
    // would break every out-of-tree exhaustive match). It is instead an opt-in
    // claim a widget makes from its uncaptured `Move` arm, recorded as an epoch
    // stamp down the pod chain — so the fixture below is deliberately shaped like
    // a real container: two hit-tested children, a capture fast-path, and paint
    // recording what `PaintCtx::is_hovered` reported.

    /// Which of the fixture's two leaves an assertion is about.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Leaf {
        Top,
        Bottom,
    }

    /// What one hover leaf observed, shared out of the widget tree.
    #[derive(Default)]
    struct HoverProbe {
        /// `PaintCtx::is_hovered()` as of the last paint.
        painted_hovered: Cell<bool>,
        /// `EventCtx::is_hovered()` as of the last event dispatch that reached it.
        event_hovered: Cell<bool>,
    }

    /// A leaf that claims hover on any `Move` landing inside its own bounds — the
    /// canonical opt-in shape — and optionally captures the pointer on `Down` (the
    /// drag fixture: a captured pointer must never create hover).
    struct HoverLeaf {
        captures: bool,
        probe: Rc<HoverProbe>,
    }

    impl crate::widget::Widget for HoverLeaf {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(100.0, 30.0))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            self.probe.painted_hovered.set(ctx.is_hovered());
        }
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            self.probe.event_hovered.set(ctx.is_hovered());
            let InputEvent::Pointer(p) = event else {
                return EventResult::Ignored;
            };
            match p.phase {
                PointerPhase::Move => {
                    let size = ctx.size();
                    let inside = p.position.x >= 0.0
                        && p.position.y >= 0.0
                        && p.position.x < size.width
                        && p.position.y < size.height;
                    if inside {
                        ctx.claim_hover();
                    }
                    // Deliberately `Ignored`: a hovering widget does not consume a
                    // move it merely watched (the shipped `ListItem` shape).
                    EventResult::Ignored
                }
                PointerPhase::Down => {
                    if self.captures {
                        ctx.capture_pointer();
                    }
                    EventResult::Handled
                }
                _ => EventResult::Ignored,
            }
        }
    }

    /// Two stacked hover leaves with a hit-tested route and a capture fast-path —
    /// the minimum container that can show a claim moving between siblings.
    struct HoverPair {
        top: crate::widget::ChildPod,
        bottom: crate::widget::ChildPod,
    }

    impl crate::widget::Widget for HoverPair {
        fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            self.top.layout_child(ctx, bc);
            self.top.set_origin(Point::ZERO);
            self.bottom.layout_child(ctx, bc);
            self.bottom.set_origin(Point::new(0.0, 30.0));
            bc.max()
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            self.top.paint_child(ctx, scene);
            self.bottom.paint_child(ctx, scene);
        }
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            let releases = matches!(
                event,
                InputEvent::Pointer(p)
                    if matches!(p.phase, PointerPhase::Up | PointerPhase::Cancel)
            );
            for pod in [&mut self.top, &mut self.bottom] {
                if pod.is_active() {
                    let r = pod.event_child(ctx, event);
                    if releases {
                        pod.set_active(false);
                    }
                    return r;
                }
            }
            let pos = event.position();
            for pod in [&mut self.top, &mut self.bottom] {
                if pod.contains(pos) {
                    return pod.event_child(ctx, event);
                }
            }
            EventResult::Ignored
        }
    }

    struct HoverPairView {
        top: Rc<HoverProbe>,
        bottom: Rc<HoverProbe>,
        captures: bool,
    }

    impl View<()> for HoverPairView {
        type Element = HoverPair;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> HoverPair {
            HoverPair {
                top: crate::widget::ChildPod::new(Box::new(HoverLeaf {
                    captures: self.captures,
                    probe: self.top.clone(),
                })),
                bottom: crate::widget::ChildPod::new(Box::new(HoverLeaf {
                    captures: self.captures,
                    probe: self.bottom.clone(),
                })),
            }
        }
        fn rebuild(&self, _p: &Self, _e: &mut HoverPair, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    /// A `RenderRoot` over the hover fixture, plus the two probes.
    struct HoverHarness {
        root: RenderRoot<(), HoverPairView>,
        top: Rc<HoverProbe>,
        bottom: Rc<HoverProbe>,
        state: (),
    }

    impl HoverHarness {
        fn new(captures: bool) -> Self {
            let top = Rc::new(HoverProbe::default());
            let bottom = Rc::new(HoverProbe::default());
            let mut root: RenderRoot<(), HoverPairView> = RenderRoot::new();
            let mut state = ();
            let (t, b) = (top.clone(), bottom.clone());
            root.rebuild(
                &mut move |_: &mut ()| HoverPairView {
                    top: t.clone(),
                    bottom: b.clone(),
                    captures,
                },
                &mut state,
            );
            root.layout(Size::new(100.0, 60.0));
            HoverHarness {
                root,
                top,
                bottom,
                state,
            }
        }

        /// Dispatch a pointer event at `(x, y)` in window space.
        fn dispatch(&mut self, phase: PointerPhase, x: f64, y: f64) -> EventOutcome {
            let event = InputEvent::Pointer(PointerEvent {
                phase,
                position: Point::new(x, y),
                button: PointerButton::Primary,
            });
            self.root.event(&mut self.state, &event)
        }

        /// Move the pointer over the given leaf's middle.
        fn move_over(&mut self, leaf: Leaf) -> EventOutcome {
            match leaf {
                Leaf::Top => self.dispatch(PointerPhase::Move, 50.0, 15.0),
                Leaf::Bottom => self.dispatch(PointerPhase::Move, 50.0, 45.0),
            }
        }

        /// Paint the tree, refreshing both probes' recorded hover state.
        fn paint(&mut self) {
            let mut scene = RecordingScene::default();
            self.root.paint(&mut scene, FrameTime::ZERO);
        }

        /// `(top, bottom)` hover as the last paint reported it.
        fn painted(&mut self) -> (bool, bool) {
            self.paint();
            (
                self.top.painted_hovered.get(),
                self.bottom.painted_hovered.get(),
            )
        }
    }

    #[test]
    fn an_uncaptured_move_claims_hover_and_paint_reports_it() {
        let mut h = HoverHarness::new(false);
        assert!(!h.root.is_hover_active(), "nothing is hovered at rest");
        assert_eq!(h.painted(), (false, false));

        let outcome = h.move_over(Leaf::Top);
        assert!(h.root.is_hover_active(), "the claim reached the root");
        assert!(
            !outcome.handled,
            "a hovering widget need not consume the move"
        );
        assert_eq!(
            h.painted(),
            (true, false),
            "exactly the claimant reads as hovered"
        );

        // A second move within the same leaf keeps the link (the claim is
        // re-recorded every pass) without re-reporting a change.
        h.dispatch(PointerPhase::Move, 60.0, 20.0);
        assert_eq!(h.painted(), (true, false));
    }

    #[test]
    fn a_second_widgets_claim_clears_the_first_and_asks_for_a_repaint() {
        let mut h = HoverHarness::new(false);
        h.move_over(Leaf::Top);
        assert_eq!(h.painted(), (true, false));

        // The pointer moves onto the sibling. The container never has to clear
        // anything: the epoch advance strands the top pod's stamp.
        h.move_over(Leaf::Bottom);
        assert!(h.root.is_hover_active());
        assert_eq!(
            h.painted(),
            (false, true),
            "the previous claimant lost its link when the new one recorded"
        );

        // Both widgets need a repaint, and a repaint is global — one request
        // covers them. The *losing* side is what the root itself must guarantee:
        // moving onto a leaf that claims nothing still repaints.
        let outcome = h.dispatch(PointerPhase::Move, 50.0, 200.0);
        assert!(
            !h.root.is_hover_active(),
            "a move claiming nothing ends the hover"
        );
        assert!(
            outcome.needs_redraw,
            "the widget that lost hover cannot ask for the repaint itself"
        );
        assert_eq!(h.painted(), (false, false));

        // ...and the same move repeated is not a change any more.
        let settled = h.dispatch(PointerPhase::Move, 50.0, 200.0);
        assert!(
            !settled.needs_redraw,
            "an already-hoverless move requests nothing"
        );
    }

    #[test]
    fn a_captured_move_cannot_claim_hover() {
        let mut h = HoverHarness::new(true);
        // Press the top leaf: it captures, and the `Down` itself ends any hover.
        h.dispatch(PointerPhase::Down, 50.0, 15.0);
        assert!(h.root.is_pointer_captured());
        assert!(!h.root.is_hover_active());

        // Drag: every one of these moves routes to the captured leaf, whose `Move`
        // arm hit-tests inside and calls `claim_hover()` — and must record nothing.
        h.dispatch(PointerPhase::Move, 50.0, 16.0);
        assert!(
            !h.root.is_hover_active(),
            "a captured pointer never creates hover"
        );
        assert_eq!(h.painted(), (false, false));

        // Dragging outside the leaf keeps routing to it (capture), still no hover.
        h.dispatch(PointerPhase::Move, 50.0, 45.0);
        assert!(!h.root.is_hover_active());
        assert_eq!(h.painted(), (false, false));

        // Release, then a fresh uncaptured move: hover is claimable again.
        h.dispatch(PointerPhase::Up, 50.0, 15.0);
        h.move_over(Leaf::Top);
        assert!(h.root.is_hover_active());
        assert_eq!(h.painted(), (true, false));
    }

    #[test]
    fn a_down_or_cancel_ends_the_hover() {
        for ending in [PointerPhase::Down, PointerPhase::Cancel] {
            let mut h = HoverHarness::new(false);
            h.move_over(Leaf::Top);
            assert!(h.root.is_hover_active());

            let outcome = h.dispatch(ending, 50.0, 15.0);
            assert!(
                !h.root.is_hover_active(),
                "{ending:?} ends the hover link outright"
            );
            assert!(
                outcome.needs_redraw,
                "{ending:?} that dropped a hover asks for the repaint"
            );
            assert_eq!(h.painted(), (false, false));
        }
    }

    #[test]
    fn a_non_pointer_pass_leaves_a_live_hover_standing() {
        let mut h = HoverHarness::new(false);
        h.move_over(Leaf::Top);
        assert_eq!(h.painted(), (true, false));

        // Neither a scroll, a key, nor the housekeeping broadcast is a hover pass:
        // the pointer has not moved, so the link must survive them untouched.
        h.root.event(
            &mut h.state,
            &InputEvent::Scroll {
                position: Point::new(50.0, 15.0),
                delta: crate::event::ScrollDelta::Lines(0.0, 1.0),
            },
        );
        assert!(h.root.is_hover_active());
        h.root.event(&mut h.state, &InputEvent::Housekeeping);
        assert!(h.root.is_hover_active());
        assert_eq!(h.painted(), (true, false));

        // An `Up` is not a hover pass either (a click without moving the mouse
        // keeps whatever the preceding `Down` left, which is nothing).
        h.dispatch(PointerPhase::Up, 50.0, 15.0);
        assert!(h.root.is_hover_active());
    }

    #[test]
    fn event_ctx_hover_reports_the_previous_pass_not_this_ones_claim() {
        let mut h = HoverHarness::new(false);
        // First move over the top leaf: it was not hovered when its handler ran.
        h.move_over(Leaf::Top);
        assert!(
            !h.top.event_hovered.get(),
            "a fresh claim does not retroactively flip `is_hovered`"
        );
        // Second move over the same leaf: now it observes the link it holds.
        h.move_over(Leaf::Top);
        assert!(
            h.top.event_hovered.get(),
            "the link recorded last pass is visible to this pass's handler"
        );
    }

    // --- Cursor: the per-pass request channel ---------------------------------
    //
    // The cursor is hover's sibling and is deliberately not derived from it (the
    // root's hover mirror is identity-free), so it gets its own fixture: two
    // leaves that ask for different shapes, a container that can speak before or
    // after routing (the last-writer case), and a capture fast-path (the drag
    // case, where the shape must survive the pointer leaving the widget).

    /// How the cursor fixture's container speaks relative to its children.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum ContainerCursor {
        /// Says nothing at all — the ordinary container.
        Silent,
        /// Asks *before* routing, so whatever the child asks for comes later.
        BeforeRouting(CursorIcon),
        /// Asks *after* routing, deliberately overriding its child.
        AfterRouting(CursorIcon),
    }

    /// A leaf that asks for one cursor while the pointer is over it and another
    /// while it holds the capture — the two shapes a real draggable control wants.
    struct CursorLeaf {
        hover_icon: Option<CursorIcon>,
        drag_icon: Option<CursorIcon>,
        captures: bool,
        captured: bool,
    }

    impl crate::widget::Widget for CursorLeaf {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(100.0, 30.0))
        }
        // A cursor is resolved entirely in the event pass — this fixture never
        // paints, unlike the hover one (whose authoritative read is at paint time).
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            let InputEvent::Pointer(p) = event else {
                return EventResult::Ignored;
            };
            match p.phase {
                PointerPhase::Move => {
                    if self.captured {
                        // The captured pass belongs to this widget wherever the
                        // pointer has gone — re-asking here is what keeps the drag
                        // shape alive outside its own bounds.
                        if let Some(icon) = self.drag_icon {
                            ctx.set_cursor(icon);
                        }
                        return EventResult::Handled;
                    }
                    let size = ctx.size();
                    let inside = p.position.x >= 0.0
                        && p.position.y >= 0.0
                        && p.position.x < size.width
                        && p.position.y < size.height;
                    if inside && let Some(icon) = self.hover_icon {
                        ctx.set_cursor(icon);
                    }
                    EventResult::Ignored
                }
                PointerPhase::Down => {
                    if self.captures {
                        ctx.capture_pointer();
                        self.captured = true;
                    }
                    EventResult::Handled
                }
                PointerPhase::Up | PointerPhase::Cancel => {
                    self.captured = false;
                    EventResult::Ignored
                }
            }
        }
    }

    /// Two stacked cursor leaves routed exactly like [`HoverPair`], plus the
    /// container's own optional request.
    struct CursorPair {
        top: crate::widget::ChildPod,
        bottom: crate::widget::ChildPod,
        own: ContainerCursor,
    }

    impl crate::widget::Widget for CursorPair {
        fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            self.top.layout_child(ctx, bc);
            self.top.set_origin(Point::ZERO);
            self.bottom.layout_child(ctx, bc);
            self.bottom.set_origin(Point::new(0.0, 30.0));
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let ContainerCursor::BeforeRouting(icon) = self.own {
                ctx.set_cursor(icon);
            }
            let result = self.route(ctx, event);
            if let ContainerCursor::AfterRouting(icon) = self.own {
                ctx.set_cursor(icon);
            }
            result
        }
    }

    impl CursorPair {
        /// The capture-first, then hit-test routing every real container does.
        fn route(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            let releases = matches!(
                event,
                InputEvent::Pointer(p)
                    if matches!(p.phase, PointerPhase::Up | PointerPhase::Cancel)
            );
            for pod in [&mut self.top, &mut self.bottom] {
                if pod.is_active() {
                    let r = pod.event_child(ctx, event);
                    if releases {
                        pod.set_active(false);
                    }
                    return r;
                }
            }
            let pos = event.position();
            for pod in [&mut self.top, &mut self.bottom] {
                if pod.contains(pos) {
                    return pod.event_child(ctx, event);
                }
            }
            EventResult::Ignored
        }
    }

    #[derive(Clone, Copy)]
    struct CursorPairView {
        own: ContainerCursor,
        captures: bool,
    }

    impl View<()> for CursorPairView {
        type Element = CursorPair;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> CursorPair {
            CursorPair {
                top: crate::widget::ChildPod::new(Box::new(CursorLeaf {
                    hover_icon: Some(CursorIcon::Pointer),
                    drag_icon: Some(CursorIcon::Grabbing),
                    captures: self.captures,
                    captured: false,
                })),
                bottom: crate::widget::ChildPod::new(Box::new(CursorLeaf {
                    hover_icon: Some(CursorIcon::Text),
                    drag_icon: None,
                    captures: false,
                    captured: false,
                })),
                own: self.own,
            }
        }
        fn rebuild(&self, _p: &Self, _e: &mut CursorPair, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    /// A `RenderRoot` over the cursor fixture.
    struct CursorHarness {
        root: RenderRoot<(), CursorPairView>,
        state: (),
    }

    impl CursorHarness {
        fn new(own: ContainerCursor, captures: bool) -> Self {
            let mut root: RenderRoot<(), CursorPairView> = RenderRoot::new();
            let mut state = ();
            root.rebuild(
                &mut move |_: &mut ()| CursorPairView { own, captures },
                &mut state,
            );
            root.layout(Size::new(100.0, 60.0));
            CursorHarness { root, state }
        }

        fn dispatch(&mut self, phase: PointerPhase, x: f64, y: f64) -> EventOutcome {
            let event = InputEvent::Pointer(PointerEvent {
                phase,
                position: Point::new(x, y),
                button: PointerButton::Primary,
            });
            self.root.event(&mut self.state, &event)
        }

        fn move_over(&mut self, leaf: Leaf) {
            match leaf {
                Leaf::Top => self.dispatch(PointerPhase::Move, 50.0, 15.0),
                Leaf::Bottom => self.dispatch(PointerPhase::Move, 50.0, 45.0),
            };
        }

        fn cursor(&self) -> CursorIcon {
            self.root.cursor()
        }
    }

    #[test]
    fn a_move_resolves_the_requested_cursor_and_absence_resolves_default() {
        let mut h = CursorHarness::new(ContainerCursor::Silent, false);
        assert_eq!(
            h.cursor(),
            CursorIcon::Default,
            "nothing has asked for anything yet"
        );

        h.move_over(Leaf::Top);
        assert_eq!(
            h.cursor(),
            CursorIcon::Pointer,
            "the request reached the root"
        );

        // Moving onto the sibling re-resolves to *its* shape with nothing cleared:
        // the pass simply has a different last writer.
        h.move_over(Leaf::Bottom);
        assert_eq!(h.cursor(), CursorIcon::Text);

        // Moving off both: the next pass has no writer at all, and absence is the
        // default rather than a stale value — the whole point of a stateless
        // request.
        h.dispatch(PointerPhase::Move, 50.0, 200.0);
        assert_eq!(
            h.cursor(),
            CursorIcon::Default,
            "a widget that stops asking falls back with nothing to clear"
        );
    }

    #[test]
    fn a_cursor_change_does_not_ask_for_a_repaint() {
        // Applying a cursor is a platform call the shell makes after the pass, with
        // no frame involved; folding it into `needs_redraw` would repaint the whole
        // tree on every hover move.
        let mut h = CursorHarness::new(ContainerCursor::Silent, false);
        let outcome = h.dispatch(PointerPhase::Move, 50.0, 15.0);
        assert_eq!(h.cursor(), CursorIcon::Pointer);
        assert!(
            !outcome.needs_redraw,
            "a cursor request alone never schedules a frame"
        );
    }

    #[test]
    fn the_last_writer_on_the_routed_path_wins() {
        // A container that asks before routing loses to its child: the child's
        // handler runs later in the same pass, which is what makes a specific
        // control override the generic surface behind it.
        let mut h =
            CursorHarness::new(ContainerCursor::BeforeRouting(CursorIcon::ColResize), false);
        h.move_over(Leaf::Top);
        assert_eq!(
            h.cursor(),
            CursorIcon::Pointer,
            "the innermost widget the route reached spoke last"
        );

        // ...and the container's own request still resolves where no child asks
        // (moving off both leaves leaves the container as the only writer).
        h.dispatch(PointerPhase::Move, 50.0, 200.0);
        assert_eq!(h.cursor(), CursorIcon::ColResize);

        // The deliberate override is the mirror case: asking *after* routing beats
        // the child.
        let mut h =
            CursorHarness::new(ContainerCursor::AfterRouting(CursorIcon::NotAllowed), false);
        h.move_over(Leaf::Top);
        assert_eq!(
            h.cursor(),
            CursorIcon::NotAllowed,
            "a container overriding its children asks after routing"
        );
    }

    #[test]
    fn only_a_pointer_move_re_resolves_the_cursor() {
        let mut h = CursorHarness::new(ContainerCursor::Silent, false);
        h.move_over(Leaf::Top);
        assert_eq!(h.cursor(), CursorIcon::Pointer);

        // A press/release says nothing about the cursor, and must not blink it back
        // to `Default` for the duration of a click.
        h.dispatch(PointerPhase::Down, 50.0, 15.0);
        assert_eq!(h.cursor(), CursorIcon::Pointer, "a Down leaves it standing");
        h.dispatch(PointerPhase::Up, 50.0, 15.0);
        assert_eq!(h.cursor(), CursorIcon::Pointer, "an Up leaves it standing");

        // Neither does a scroll, a key, or the housekeeping broadcast.
        h.root.event(
            &mut h.state,
            &InputEvent::Scroll {
                position: Point::new(50.0, 15.0),
                delta: crate::event::ScrollDelta::Lines(0.0, 1.0),
            },
        );
        assert_eq!(h.cursor(), CursorIcon::Pointer);
        h.root.event(&mut h.state, &InputEvent::Housekeeping);
        assert_eq!(h.cursor(), CursorIcon::Pointer);
    }

    #[test]
    fn a_captured_drag_keeps_the_capturing_widgets_cursor() {
        let mut h = CursorHarness::new(ContainerCursor::Silent, true);
        h.move_over(Leaf::Top);
        assert_eq!(h.cursor(), CursorIcon::Pointer);

        // Press the top leaf: it captures. The `Down` itself resolves nothing.
        h.dispatch(PointerPhase::Down, 50.0, 15.0);
        assert!(h.root.is_pointer_captured());
        assert_eq!(h.cursor(), CursorIcon::Pointer);

        // Drag inside, then well outside its own bounds and over the sibling: every
        // one of these moves routes to the captured leaf alone, so its drag shape is
        // the only request in the pass — the sibling's `Text` never gets a say.
        h.dispatch(PointerPhase::Move, 50.0, 16.0);
        assert_eq!(h.cursor(), CursorIcon::Grabbing);
        h.dispatch(PointerPhase::Move, 50.0, 45.0);
        assert_eq!(
            h.cursor(),
            CursorIcon::Grabbing,
            "a captured drag keeps its own cursor outside its bounds"
        );
        h.dispatch(PointerPhase::Move, 50.0, 500.0);
        assert_eq!(h.cursor(), CursorIcon::Grabbing);

        // Release, then a fresh uncaptured move: the ordinary hit-tested resolution
        // is back, and the drag shape is gone with nothing cleared.
        h.dispatch(PointerPhase::Up, 50.0, 45.0);
        h.move_over(Leaf::Bottom);
        assert_eq!(h.cursor(), CursorIcon::Text);
    }
}
