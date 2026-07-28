//! [`NativeComponent`] — the **public** trait a plugin author writes a native
//! component against, from pure Rust, with no per-component Kotlin or Swift
//! (native-widgets Phase 3, p3-01;
//! `workflow/plans/features/frust-native-widgets/research/RESEARCH-NATIVE-COMPONENT.md`
//! is the verified design this productizes).
//!
//! **An app crate cannot implement this trait today** — which is why the line
//! above says *plugin* author. `create` has to construct real native views,
//! which means naming `jni::objects::JObject` on Android and `objc2-ui-kit`'s
//! classes on iOS *in the implementing crate*; this plugin re-exports neither
//! FFI crate, and `docs/CODE_STANDARDS.md`'s State & Reactivity Conventions
//! sanction only a `frust-core`/`kurbo`/`peniko` escape hatch, and only for an
//! `examples/*` app. So the practical audience today is plugin authors, not
//! app authors, and the only implementor in this repo is this crate's own
//! non-default `demo-components` composite (`crate::demo`, which spells the
//! same wall out at length). Closing the gap — re-exporting a curated
//! view-construction surface, or the FFI crates themselves — is a separate,
//! unscheduled decision.
//!
//! Where `crate::runtime`'s `NativeWidget` is the plugin's **internal** dispatch
//! contract — associated functions, `Result` returns, a `decode_props` step
//! that reads a control's fields out of the slot's `params_json` — this trait
//! is the shape a *third party* implements:
//!
//! | | internal `NativeWidget` | public [`NativeComponent`] |
//! |---|---|---|
//! | receiver | associated functions | `&self` — the value the app constructs each rebuild |
//! | props | decoded from `params_json` inside the impl | **already-typed Rust values** the app hands over, staged beside the wire (*Props travel beside the wire*, below) |
//! | errors | every method returns `Result` | latched on the context ([`ComponentCtx::report_error`]); `create` may answer `None` |
//! | events | decoded into the crate's typed `EventPayload` | the raw [`NativeEvent`] pair, handled by the component itself |
//! | context | the platform's own `NativeCtx` | the opaque [`ComponentCtx`] wrapper |
//!
//! # The six v1 controls are NOT ported onto this trait
//!
//! They stay internal `NativeWidget` impls, and this module **bridges** to
//! them rather than rewriting them: `Bridge<C>` (crate-private) is one
//! `NativeWidget` impl,
//! generic over every public component, so both kinds of implementation are
//! dispatched by the same runtime, the same registry, the same props diff
//! gate and the same disposal path. That is a deliberate p3-01 decision, on
//! three grounds: the six controls' whole wire is `params_json` (they cannot
//! use the typed props channel below without their api-layer builders changing
//! shape), a public trait implemented *by* an internal one would either need a
//! blanket impl — which would then block every third-party impl on coherence
//! grounds — or a rewrite of six shipped files, and "behaviour must not
//! change" is only provable if their code does not move.
//! The bridge is what makes the public trait a real dispatch path instead of a
//! re-labelling: every guarantee documented below is the runtime's own, not a
//! second implementation of it.
//!
//! # The lifecycle contract (what the runtime guarantees)
//!
//! **Every method here runs on the platform main thread**, from the host's
//! post-frame command poll or from a platform listener firing — never from a
//! frust rebuild, and never off-thread (`crate::runtime`'s *main-thread
//! confinement*).
//!
//! 1. **`create` arrives a frame or more after the widget mounts.** Mounting a
//!    slot publishes a `Create` command; the host drains its backlog on the
//!    next post-frame poll, and *that* is what calls
//!    [`NativeComponent::create`]. Anything the component retains lives in
//!    [`NativeComponent::State`], which is born there — there is nothing
//!    native to hold before it.
//! 2. **Props coalesce until then, and are always whole state, never a
//!    delta.** Each rebuild replaces a slot's staged props outright, so a
//!    create landing after three rebuilds sees only the newest; replaying a
//!    prefix of the command backlog (a surface-recreate replay, a backlog
//!    compaction) lands in the same place.
//! 3. **`update` runs only when props actually differ.** The runtime compares
//!    the typed props with `PartialEq` **before** any platform call, so an
//!    unchanged rebuild costs zero FFI crossings. Field-level diffing inside a
//!    changed props value is the component's own job — only it knows which
//!    setter is cheap and which forces a re-layout.
//! 4. **`on_event` fires between frames — except that for a public component
//!    it never fires at all in this build.** A native interaction bypasses
//!    `RenderRoot::event` entirely (see the crate doc): no `EventCtx`, no
//!    capture/focus, none of `docs/CODE_STANDARDS.md`'s Interaction Semantics.
//!    A listener that fires while the runtime is already borrowed — the
//!    classic case is a setter provoking its own listener synchronously from
//!    inside `update` — is **dropped with a warning**, not delivered
//!    re-entrantly. Those ordering and re-entrancy guarantees are the
//!    runtime's own, true of the internal `NativeWidget` path the six built-in
//!    controls take — and **not observable through this trait yet**: no
//!    production path attaches a listener to a view a component built, so this
//!    step of the lifecycle currently never runs for a [`NativeComponent`].
//!    See [`NativeComponent::on_event`], which owns the detail and names the
//!    deferred Phase 4 gap.
//!
//!    **The staged-`&self` re-read is a separate guarantee, it is
//!    component-only, and its `dispose` half is live today.** The `&self`
//!    carried into [`on_event`](NativeComponent::on_event) and
//!    [`dispose`](NativeComponent::dispose) is re-read from the staging table
//!    on **every** dispatch, not only when props changed
//!    (`BridgeState::refresh_component`) — the props diff gate skips `update`
//!    on an equal-props rebuild, so anything less would run a stale rebuild's
//!    closures. This has no counterpart on the six controls' path, which
//!    decodes `params_json` and never reads the staging table. Unlike
//!    `on_event`, `dispose` **does** reach a component in this build, so the
//!    re-read is load-bearing now — see [`NativeComponent::dispose`].
//! 5. **`dispose` is best-effort-prompt, and may be late.** See *Disposal
//!    promptness* below.
//!
//! # The three decisions p3-01 owed (RESEARCH-NATIVE-COMPONENT's open points)
//!
//! **1. What a third-party impl receives as a context.** [`ComponentCtx`] — an
//! **opaque wrapper**, never the plugin's own per-platform `NativeCtx`. That
//! keeps the internal helper surface (hot cached method ids, the classloader
//! cache, the local-frame wrapper) free to change without breaking a public
//! impl, and it is where the error latch lives, which is what lets the trait's
//! own methods stay `Result`-free. The wrapper carries a deliberately narrow
//! per-platform surface plus the platform's own escape hatch
//! (`ComponentCtx::env` on Android, `ComponentCtx::mtm` on iOS — each
//! `#[cfg]`-gated to its own target, so only a docs build for that target
//! renders it): curated
//! helpers can never cover "construct an arbitrary native view", and both
//! escape-hatch types are already in this crate's public API (`AndroidHandle`
//! names `jni`'s `Global<JObject>`, `AppleHandle` names objc2's
//! `Retained<UIView>`), so no new dependency is exposed by admitting them
//! here. The hierarchy-building half of that surface landed in p3-02 — see
//! *A component owns its own native subtree* below.
//!
//! **2. How a component reaches the runtime's dispatch table.**
//! [`register_component`], explicitly, from app or plugin init —
//! `inventory`-style link-time auto-registration stays **banned** (it is
//! exactly the mechanism that fails silently in a stripped, LTO'd device
//! build; RESEARCH-NATIVE-COMPONENT §Open questions). Registration is
//! **first-wins**: a kind string already registered — including any of the six
//! built-in controls, which this build's backend registers when the thread's
//! runtime is first touched — is refused with a warning rather than replaced,
//! so a third-party kind can never shadow a shipped control.
//!
//! **3. Disposal promptness.** A public component gets **exactly the same
//! guarantee the six controls get, and no more**: the framework's `retire()`
//! (driven from the mounting widget's teardown, p1-09) is the primary path and
//! disposes promptly; the differ's missing-frame streak is the backstop, and
//! it only advances on gate-`Run` frames, so on an idle screen a `dispose` can
//! arrive many frames late — or after a replacement `create` already re-used
//! the slot id, in which case the stale command resolves against the *old*
//! view by identity, finds nothing, and is dropped. Two consequences a
//! component must design for: `dispose` may run long after the widget
//! disappeared, and (at process exit) may not run at all. Anything whose
//! release cannot wait belongs in the component's own `State`, released when
//! `State` drops — which happens immediately after [`NativeComponent::dispose`]
//! returns, alongside the runtime's paired delete of the [`NativeRoot`].
//!
//! # A component owns its own native subtree (p3-02)
//!
//! One component may build a whole native view *hierarchy* — a parent with
//! native children — and ship it as ONE slot, which is what lets a composite
//! (a card with an image and two buttons) stop leaking three slots to the
//! consuming app. Four calls are the entire surface:
//!
//! | | Android | iOS |
//! |---|---|---|
//! | build a child | `ComponentCtx::new_view` | any `objc2-ui-kit` constructor, off `ComponentCtx::mtm` |
//! | attach it | [`ComponentCtx::add_child`] (JNI `addView`) | [`ComponentCtx::add_child`] (`addSubview`) |
//! | keep talking to it | [`ComponentCtx::retain_child`] → [`NativeChild`] (a global ref) | the same, or just keep your own `Retained<T>` |
//! | bound the reference table | [`ComponentCtx::with_local_frame`] (real `PushLocalFrame`) | the same call, which does nothing here (ARC) |
//!
//! The two per-platform view constructors in the first row are `#[cfg]`-gated
//! to Android/iOS respectively, so they render only in a docs build for that
//! target — unlike the three below them, which every target carries (the host
//! arm's stand-ins are what make a component's create/update/dispose plan
//! assertable by an ordinary `cargo test`).
//!
//! **The platform lays the subtree out, and frust deliberately does not know
//! the children exist.** frust's wire carries per-slot geometry only — a
//! `rect`, an optional `clip`, `shields` — with no hierarchical child
//! geometry, so a component positions its own children the platform's way (a
//! `LinearLayout`, a `UIStackView`, explicit frames) and frust keeps seeing
//! one opaque slot with one rect. This is the SwiftUI `UIViewRepresentable` /
//! Compose `AndroidView` model, chosen over React Native's — where the
//! framework's own layout engine walks into native containers — because
//! buying that would mean re-acquiring, per child, the frame-pairing
//! synchronisation, shield collection, culling and accessibility bridging
//! frust gets per slot today (`research/RESEARCH-P3.md` §4). **No wire
//! change**: a subtree costs the differ exactly what a single leaf control
//! costs it. A11y comes out ahead, in fact — the platform owns the subtree,
//! so it traverses it natively.
//!
//! ## Teardown: children are released with the parent
//!
//! Phase 0 spike 1 built 50 native children in ONE slot on device and
//! measured **52 global refs at peak → 0 after the dispose cycle**
//! (`research/SPIKE.md`); this surface keeps that property by construction:
//!
//! - A child that is merely *attached* needs no handle at all — the platform
//!   parent owns it (Android's `ViewGroup` holds its own strong reference,
//!   UIKit retains a subview), so it dies with the parent.
//! - A child you keep talking to lives in [`NativeComponent::State`] as a
//!   [`NativeChild`], and `State` is dropped immediately after
//!   [`NativeComponent::dispose`] returns, alongside the runtime's paired
//!   delete of the [`NativeRoot`]. Dropping a [`NativeChild`] *is* the
//!   release: `DeleteGlobalRef` on Android, `Retained`'s own `Drop` on iOS.
//!
//! So the leak bar is the same one the six controls already answer to, and
//! `tests::a_component_builds_a_native_subtree_and_releases_every_child`
//! counts it rather than merely surviving it.
//!
//! # Props travel beside the wire, not on it
//!
//! The framework's platform-view wire carries one `params_json` string per
//! slot, and that is what the differ diffs to decide whether to emit an
//! `UpdateParams` at all. A public component's props are typed Rust values
//! that never touch JSON, so they ride a **thread-local staging table** here
//! (written by this module's crate-private `publish`/`forget` pair, whose only
//! production caller is the generic mounting builder) while the slot's
//! `params_json` carries only the runtime's two identity keys plus a props
//! **generation** counter that `publish` bumps when — and only when — the
//! published props actually changed. The wire therefore changes exactly when
//! the props do, which is what makes the differ emit the `UpdateParams` the
//! typed props ride along with. None of that machinery is public: an app
//! stages props by rebuilding
//! [`native_component`](crate::api::native_component), never by calling into
//! the table itself.
//!
//! Like every other slot-keyed table in this crate, the staging table is
//! bounded by an explicit reaper (`forget`, from the mounting widget's
//! teardown), never by disposal alone — the c1-01 leak shape
//! (`crate::runtime`'s `forget_pending_callback`) applies here verbatim: a
//! culled slot's dispose resolves by view identity and never sees this table.

// The publication half of this module (`publish`/`forget`/`component_params`)
// got its production caller in p3-02: `crate::api::mount`'s generic builder
// runs exactly the sequence the host tests below drive (`publish` → mount →
// `create` → `update` → `forget`). `staged_count` stays test-only, and keeps
// its own `allow` rather than a module-level one, so anything else falling
// dead here still warns.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::Rc;

use crate::NativeWidgetError;
use crate::events::EventPayload;
use crate::registry::SlotId;
use crate::runtime::{
    NativeCtx as PlatformCtx, NativeEvent as WireEvent, NativeView, NativeWidget, Params,
    with_runtime,
};

/// The reserved `params_json` key carrying a component slot's props
/// **generation** — the wire-visible value that changes when (and only when)
/// [`publish`] is handed props that differ from the ones already staged.
///
/// Deliberately a sibling of `crate::runtime`'s two identity keys rather than
/// a re-use of either: the runtime's `__frustControl`/`__frustSlot` say *which*
/// component a slot is, this says *which version of its props* the slot last
/// published. A component's real props never appear on the wire at all.
const PROPS_GENERATION_KEY: &str = "__frustProps";

// --- the public trait --------------------------------------------------------

/// One retained native view — or native view *hierarchy* — created, updated
/// and torn down entirely from Rust.
///
/// Implement this for a plain marker/config type, register it once under a
/// kind string ([`register_component`]), and the same runtime that serves this
/// crate's six built-in controls will serve yours: one generic platform
/// factory, one generic listener, **no per-component Kotlin or Swift, ever**.
///
/// Read the module doc first — it is the lifecycle contract (when each method
/// runs, what the runtime guarantees about ordering, and what it does *not*
/// guarantee about disposal promptness). The short version:
///
/// - every method runs on the **platform main thread**;
/// - `create` runs at the host's post-frame poll, a frame or more after the
///   mounting widget appeared;
/// - `update` runs only when [`Props`](Self::Props) compare unequal;
/// - `on_event` would run from a platform listener between frames, but no
///   production path attaches one to a component's view, so it never runs at
///   all in this build (a deferred Phase 4 gap — [`on_event`](Self::on_event));
/// - `dispose` is prompt on teardown but may be late, and at process exit may
///   not run at all.
///
/// # No frust `View` children
///
/// A component's native hierarchy is **its own**: frust sees one opaque slot
/// with one rect, and the platform (a `LinearLayout`, a `UIStackView`,
/// explicit frames) lays the subtree out. This is the SwiftUI
/// `UIViewRepresentable` / Compose `AndroidView` model, chosen deliberately
/// over React Native's — where the framework's own layout engine walks into
/// native containers — because frust's wire carries no hierarchical child
/// geometry and buying one would mean re-acquiring, per child, the
/// frame-pairing, shield collection, culling and accessibility bridging it
/// gets per slot today (`research/RESEARCH-P3.md` §4).
pub trait NativeComponent: 'static {
    /// The Rust-side-diffed create/update payload: everything the app tells
    /// this component, as one value it constructs directly.
    ///
    /// `PartialEq` is load-bearing, not a formality — it is the gate that
    /// keeps an unchanged rebuild from crossing the FFI boundary at all (a
    /// comparison costs nanoseconds; a redundant JNI setter costs ~0.14–29 µs
    /// depending on whether it re-layouts). `Send` is inherited from the
    /// runtime's internal contract; props never actually leave the main
    /// thread.
    type Props: Clone + PartialEq + Send + 'static;

    /// Per-instance retained state: native handles, listeners, buffers —
    /// whatever [`create`](Self::create) needs to keep to drive the view
    /// later. Created at attach time and held by the runtime's slot registry,
    /// dropped immediately after [`dispose`](Self::dispose) returns.
    type State: 'static;

    /// Build this component's native view (hierarchy) for a freshly attached
    /// slot, returning its root and the state that drives it.
    ///
    /// Return `None` to fail the slot — the runtime then reports it dead to
    /// the host rather than leaving a half-created control mounted. That is
    /// the *only* failure channel `create` has, and it exists because there is
    /// no meaningful `State` to hand back when the native side did not come
    /// up; use [`ComponentCtx::report_error`] (or let a ctx helper latch its
    /// own failure) first, so the log names what went wrong.
    ///
    /// Returning `Some` after an error was latched is legal and means *"I
    /// recovered"*: the slot lives and the latched error is logged as a
    /// warning. The impl always has the final say.
    fn create(
        &self,
        ctx: &mut ComponentCtx<'_, '_, '_>,
        props: &Self::Props,
    ) -> Option<(NativeRoot, Self::State)>;

    /// Apply a props change to the live view with direct setters on the
    /// handles [`State`](Self::State) retained.
    ///
    /// Called only when `old != new` (the module doc's props diff gate), on
    /// the main thread, ordered with this slot's create/dispose. A failure
    /// latched on `ctx` leaves the runtime's diff baseline at `old`, so the
    /// change is retried the next time the props differ.
    fn update(
        &self,
        ctx: &mut ComponentCtx<'_, '_, '_>,
        state: &mut Self::State,
        old: &Self::Props,
        new: &Self::Props,
    );

    /// What a platform listener *would* deliver for this slot — but nothing
    /// attaches one in this build, so it never fires. See below.
    ///
    /// # Nothing reaches this in the current build
    ///
    /// **No production path attaches a listener to a view a component built —
    /// its root as much as its children — so overriding this method has no
    /// effect today, and every public component is display-only.** The two
    /// listener objects that exist are constructed from a slot id (Android's
    /// shared `FrustNativeListener`, iOS's target-action object), and the only
    /// callers of either constructor are the six built-in controls;
    /// [`ComponentCtx`] exposes neither a slot id nor a way to build one, and
    /// the generic mounting builder registers no callback either
    /// (`crate::api::mount`'s `init`, whose comment says so, and
    /// `Bridge::on_event` answers `None` unconditionally).
    ///
    /// This is a **deliberately deferred Phase 4 gap**, not an oversight: the
    /// dispatch half — runtime → bridge → this method — is wired and
    /// unit-tested, and only the attach half is missing. The method stays on
    /// the trait so the contract it will be given is already stated.
    ///
    /// # The shape the channel carries when it opens
    ///
    /// The pair is the generic listener glue's raw wire
    /// ([`NativeEvent::kind`]/[`NativeEvent::detail`]); this crate's own typed
    /// event vocabulary stays internal in v1, so a component would decode the
    /// pair itself. The `&self` a dispatch runs against is the value the app
    /// published this rebuild — re-read from the staging table on every
    /// dispatch (the module doc's point 4) — so it can carry the closures such
    /// an event should reach.
    ///
    /// Defaults to doing nothing, which is also the only behaviour v1 can
    /// observe.
    fn on_event(&self, state: &mut Self::State, event: NativeEvent) {
        let _ = (state, event);
    }

    /// The slot is going away: detach listeners and release anything `state`
    /// owns beyond the [`NativeRoot`], which the runtime releases immediately
    /// afterwards — the paired delete, in that order.
    ///
    /// Defaults to doing nothing, which is correct whenever dropping `State`
    /// already releases everything (the iOS arm's `Retained` fields, an
    /// Android state whose only refs are its own `Global`s).
    ///
    /// # Which `&self` this runs against
    ///
    /// The most recently published one — re-read from the staging table on
    /// dispatch, not the value whose `create` produced `state`. Those differ
    /// whenever a rebuild republished an equal `Props` with a different
    /// component value (new closures, a different `Rc`): the props diff gate
    /// skips `update` entirely on an equal-props rebuild, so without the
    /// re-read this would run a stale rebuild's closures.
    ///
    /// One exception, and it is the *common* teardown path: when the mounting
    /// widget's `on_cleanup` has already reaped the staging entry, the
    /// retained value is kept instead. That is correct — a torn-down widget
    /// published nothing newer. The re-read matters for a `dispose` that
    /// reaches a **still-mounted** slot: the differ's missing-frame-streak
    /// culling backstop, and `suspend_all` on surface teardown.
    ///
    /// Unlike [`on_event`](Self::on_event), this method **does** run in the
    /// current build.
    fn dispose(&self, ctx: &mut ComponentCtx<'_, '_, '_>, state: Self::State) {
        let _ = (ctx, state);
    }
}

// --- the public context ------------------------------------------------------

/// The scoped, opaque call context every [`NativeComponent`] method builds
/// through — see the module doc's decision **1**.
///
/// Two things it is not: it is not the plugin's own per-platform `NativeCtx`
/// (which stays internal, so its helper surface can keep changing), and it is
/// not a handle you may store — it borrows the live platform context of the
/// one create/update/dispose call on the stack and cannot outlive it.
///
/// # The error latch
///
/// [`NativeComponent`]'s methods return no `Result`. Instead every fallible
/// helper here latches its failure on the context and answers `None`, and
/// [`report_error`](Self::report_error) lets a component latch one of its own.
/// The runtime reads the latch when the method returns: on `create` a latched
/// error is fatal only if the component also answered `None`; on `update` it
/// keeps the diff baseline unchanged so the change is retried; on `dispose` it
/// is logged. **The first error wins** — later ones are dropped, so the log
/// names the failure that started the cascade rather than its last symptom.
pub struct ComponentCtx<'ctx, 'local, 'env> {
    inner: &'ctx mut PlatformCtx<'local, 'env>,
    error: Option<NativeWidgetError>,
}

impl<'ctx, 'local, 'env> ComponentCtx<'ctx, 'local, 'env> {
    /// Wrap the platform context of one runtime call.
    fn new(inner: &'ctx mut PlatformCtx<'local, 'env>) -> Self {
        Self { inner, error: None }
    }

    /// Consume the wrapper, reporting whatever was latched.
    fn into_error(self) -> Option<NativeWidgetError> {
        self.error
    }

    /// Record the first failure and keep it (see this type's *error latch*).
    fn latch(&mut self, error: NativeWidgetError) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }
}

impl ComponentCtx<'_, '_, '_> {
    /// Whether a platform call made through this context has already failed —
    /// the early-out a component checks before continuing to build against a
    /// handle that may not exist.
    pub fn failed(&self) -> bool {
        self.error.is_some()
    }

    /// Latch a failure of the component's own (a platform call it made through
    /// the escape hatch, a precondition it found broken). Only the first
    /// latched error is kept.
    pub fn report_error(&mut self, message: impl Into<String>) {
        self.latch(NativeWidgetError::Platform(message.into()));
    }
}

#[cfg(target_os = "android")]
impl ComponentCtx<'_, '_, '_> {
    /// `parent.addView(child)` — attach one native child, the subtree call
    /// (module doc's *A component owns its own native subtree*).
    ///
    /// The parent owns the child from here: a `ViewGroup` holds its own
    /// strong reference, so a child you never touch again needs no handle of
    /// yours at all. Answers `Option` so it chains with `?` beside every
    /// other fallible helper here; latches and answers `None` when the call
    /// throws (the usual cause being a child that already has a parent).
    pub fn add_child(
        &mut self,
        parent: &jni::objects::JObject<'_>,
        child: &jni::objects::JObject<'_>,
    ) -> Option<()> {
        match self.inner.add_child(parent, child) {
            Ok(()) => Some(()),
            Err(error) => {
                self.latch(error);
                None
            }
        }
    }

    /// Promote a child's local reference into a [`NativeChild`] — one global
    /// reference the component keeps in its [`NativeComponent::State`] to
    /// drive that child later, released when `State` drops (module doc's
    /// *Teardown*).
    ///
    /// Only a child you keep talking to needs this. Latches and answers
    /// `None` when the JVM cannot allocate the reference — which, ART
    /// aborting the process at 51,200 live global refs, is itself a leak
    /// signal rather than an ordinary failure.
    pub fn retain_child(&mut self, view: &jni::objects::JObject<'_>) -> Option<NativeChild> {
        match self.inner.retain(view) {
            Ok(global) => Some(NativeChild(global)),
            Err(error) => {
                self.latch(error);
                None
            }
        }
    }

    /// Run `f` inside a pushed JNI local frame, so every local reference it
    /// creates is released the moment it returns — **mandatory** around a
    /// loop building more than a handful of children (module doc's subtree
    /// table; `capacity` is the JVM's pre-allocation hint, not a cap).
    ///
    /// Fifty children built without one would pin fifty-plus local references
    /// for the whole `create` call; Phase 0 spike 1 built exactly that,
    /// inside this frame, on device (`research/SPIKE.md`). A value the
    /// closure returns must not *be* a local reference — that is what
    /// [`Self::retain_child`] is for, and its [`NativeChild`] outlives the
    /// frame.
    ///
    /// An error the closure latched propagates to this context, and a frame
    /// that cannot be pushed at all latches too; either way the answer is
    /// `None`.
    pub fn with_local_frame<T>(
        &mut self,
        capacity: usize,
        f: impl FnOnce(&mut ComponentCtx<'_, '_, '_>) -> Option<T>,
    ) -> Option<T> {
        let mut inner_error = None;
        let outcome = self.inner.with_frame(capacity, |inner| {
            let mut cx = ComponentCtx::new(inner);
            let value = f(&mut cx);
            inner_error = cx.into_error();
            Ok::<Option<T>, NativeWidgetError>(value)
        });
        if let Some(error) = inner_error {
            self.latch(error);
        }
        match outcome {
            Ok(value) => value,
            Err(error) => {
                self.latch(error);
                None
            }
        }
    }
}

#[cfg(target_os = "android")]
impl<'local, 'env> ComponentCtx<'_, 'local, 'env> {
    /// The live JNI `Env` — the escape hatch for everything the helpers here
    /// do not cover (a component's own cached `JMethodID`s and
    /// `call_method_unchecked` hot path, any class this crate never names).
    ///
    /// A pending Java exception is undefined behaviour for the next JNI call,
    /// so a component using this directly must check and clear its own —
    /// [`Self::new_view`] and [`Self::root`] do that for the calls they make.
    pub fn env(&mut self) -> &mut jni::Env<'local> {
        self.inner.env()
    }

    /// The hosting `Context` (the platform-view factory passes the `Activity`
    /// as one), or `None` on a call path that carries none — only `create`
    /// does. A component needing a `Context` later must retain what it needs
    /// in its own [`NativeComponent::State`]; asking here off the create path
    /// is not an error and latches nothing.
    pub fn context(&self) -> Option<&'env jni::objects::JObject<'local>> {
        self.inner.context().ok()
    }

    /// `new <binary_name>(context)` — the one-argument `Context` constructor
    /// every `android.widget` view has, with the class resolved through the
    /// **application** classloader (`FindClass` cannot see app classes at all)
    /// and cached process-wide.
    ///
    /// Latches and answers `None` when the class cannot be loaded, the call
    /// path carries no `Context`, or the constructor throws.
    pub fn new_view(&mut self, binary_name: &'static str) -> Option<jni::objects::JObject<'local>> {
        match self.inner.new_view(binary_name) {
            Ok(view) => Some(view),
            Err(error) => {
                self.latch(error);
                None
            }
        }
    }

    /// Promote a local reference into the slot's [`NativeRoot`]: one global
    /// reference the runtime owns and pair-deletes on dispose.
    ///
    /// Latches and answers `None` when the JVM cannot allocate the reference
    /// — which, ART aborting the process at 51,200 live global refs, is itself
    /// a leak signal rather than an ordinary failure.
    pub fn root(&mut self, view: &jni::objects::JObject<'_>) -> Option<NativeRoot> {
        match self.inner.retain(view) {
            Ok(global) => Some(NativeRoot(NativeView::new(global))),
            Err(error) => {
                self.latch(error);
                None
            }
        }
    }
}

#[cfg(target_os = "ios")]
impl ComponentCtx<'_, '_, '_> {
    /// The main-thread proof this call carries — what every `objc2-ui-kit`
    /// constructor demands, and the Apple arm's whole escape hatch (the
    /// Objective-C runtime is globally reachable; there is no `Env` to thread).
    pub fn mtm(&self) -> objc2::MainThreadMarker {
        self.inner.mtm()
    }

    /// Take this slot's root view. ARC owns it from here — `Retained`'s own
    /// `Drop` is the release, so there is no paired-delete discipline on this
    /// arm.
    ///
    /// A typed view converts with objc2's own upcast, e.g.
    /// `Retained::clone(&button).into_super().into_super()` for a `UIButton`.
    /// Answers `Option` only so a component reads the same on both platforms
    /// (`let root = ctx.root(view)?;`); this arm never latches here.
    pub fn root(&mut self, view: objc2::rc::Retained<objc2_ui_kit::UIView>) -> Option<NativeRoot> {
        let mtm = self.inner.mtm();
        Some(NativeRoot(NativeView::new(view, mtm)))
    }

    /// `parent.addSubview(child)` — attach one native child, the subtree call
    /// (module doc's *A component owns its own native subtree*).
    ///
    /// UIKit retains a subview, so a child you never touch again needs no
    /// handle of yours at all — it is released when the parent is. Answers
    /// `Option` only so a component reads the same on both platforms; this
    /// arm never latches here.
    ///
    /// A typed view passes with objc2's own upcast, e.g. `&label` for a
    /// `Retained<UILabel>` derefs through `UIView`'s superclass chain.
    pub fn add_child(
        &mut self,
        parent: &objc2_ui_kit::UIView,
        child: &objc2_ui_kit::UIView,
    ) -> Option<()> {
        self.inner.add_child(parent, child);
        Some(())
    }

    /// Retain a child view as a [`NativeChild`] the component keeps in its
    /// [`NativeComponent::State`], released when `State` drops (module doc's
    /// *Teardown*).
    ///
    /// ARC already does this for any `Retained<T>` a component keeps itself,
    /// with the child's own concrete type preserved — reach for that first.
    /// This exists so a component that wants one `State` shape on both arms
    /// can name [`NativeChild`] on both. Never latches.
    pub fn retain_child(&mut self, view: &objc2_ui_kit::UIView) -> Option<NativeChild> {
        use objc2::Message as _;
        Some(NativeChild(view.retain()))
    }

    /// The Apple counterpart of Android's local-frame wrapper: it runs `f`
    /// and nothing else.
    ///
    /// There is no reference table to bound here — a `Retained`'s own `Drop`
    /// is the release (`crate::apple::ctx`'s module doc) — so `capacity` is
    /// accepted and ignored. The method exists on this arm so a component's
    /// `create` is written once and compiles on both.
    pub fn with_local_frame<T>(
        &mut self,
        _capacity: usize,
        f: impl FnOnce(&mut ComponentCtx<'_, '_, '_>) -> Option<T>,
    ) -> Option<T> {
        f(self)
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl ComponentCtx<'_, '_, '_> {
    /// Record one would-be platform call — the host stand-in's whole surface,
    /// so a component's create/update/dispose can be asserted by an ordinary
    /// `cargo test` on a machine with no JNI and no Objective-C runtime at
    /// all. Compiled only on a non-mobile host; there is no native view to
    /// build there.
    pub fn record(&mut self, call: impl Into<String>) {
        self.inner.record(call);
    }

    /// Take a stand-in root view with the given identity — the host mirror of
    /// the two platform arms' `root`, where `identity` plays the role
    /// `Env::is_same_object` plays on Android (what a dispose resolves
    /// against).
    pub fn root(&mut self, identity: u64) -> Option<NativeRoot> {
        Some(NativeRoot(NativeView { identity }))
    }

    /// Record one would-be `addView`/`addSubview` — the host mirror of the
    /// two platform arms' `add_child`, so a subtree's *plan* is assertable
    /// (which child went under which parent, in what order) on a machine with
    /// no view hierarchy at all.
    pub fn add_child(&mut self, parent: u64, child: u64) -> Option<()> {
        self.inner.record(format!("addChild {parent} <- {child}"));
        Some(())
    }

    /// Take a stand-in retained child handle — the host mirror of Android's
    /// global reference and iOS's `Retained`.
    ///
    /// Live handles are counted process-thread-wide (this module's
    /// crate-private `live_child_count`), so a host test asserts the paired
    /// release the way ART's global-ref count does on device (module doc's
    /// *Teardown*) rather than merely asserting that nothing panicked.
    pub fn retain_child(&mut self, identity: u64) -> Option<NativeChild> {
        self.inner.record(format!("retainChild {identity}"));
        Some(NativeChild::new(identity))
    }

    /// Record a would-be `PushLocalFrame`/`PopLocalFrame` pair around `f` —
    /// the host mirror of Android's real local frame, so a test can assert a
    /// subtree build actually ran inside one.
    pub fn with_local_frame<T>(
        &mut self,
        capacity: usize,
        f: impl FnOnce(&mut ComponentCtx<'_, '_, '_>) -> Option<T>,
    ) -> Option<T> {
        self.inner.record(format!("pushLocalFrame {capacity}"));
        let value = f(self);
        self.inner.record("popLocalFrame");
        value
    }
}

/// The root native view a [`NativeComponent::create`] hands back — an opaque
/// handle the runtime retains for the slot and releases on dispose (Android:
/// the paired global-ref delete; iOS: ARC).
///
/// Built only through [`ComponentCtx::root`], so the platform handle type
/// itself never has to appear in a component's signature; a component that
/// wants to keep talking to its own view retains a second, typed reference in
/// its [`NativeComponent::State`], exactly as the six built-in controls do.
pub struct NativeRoot(NativeView);

impl NativeRoot {
    /// Hand the platform handle to the runtime.
    fn into_inner(self) -> NativeView {
        self.0
    }
}

/// One retained **child** of a component's native subtree — the handle a
/// component keeps in its [`NativeComponent::State`] when it wants to drive
/// that child later (module doc's *A component owns its own native subtree*).
///
/// Built only through [`ComponentCtx::retain_child`]. **Dropping it is the
/// release** — `DeleteGlobalRef` on Android, `Retained`'s own `Drop` on iOS —
/// and `State` is dropped immediately after [`NativeComponent::dispose`]
/// returns, so a child is released with its parent by construction rather
/// than by remembering to.
///
/// A child you never talk to again needs none of this: the platform parent
/// already owns it.
pub struct NativeChild(ChildHandle);

/// [`NativeChild`]'s per-platform payload: a global reference on Android, an
/// ARC retain on iOS, a counted stand-in on a non-mobile host.
#[cfg(target_os = "android")]
type ChildHandle = jni::refs::Global<jni::objects::JObject<'static>>;
#[cfg(target_os = "ios")]
type ChildHandle = objc2::rc::Retained<objc2_ui_kit::UIView>;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
type ChildHandle = HostChild;

#[cfg(target_os = "android")]
impl NativeChild {
    /// The retained child, for the setters a component's `update` calls on it
    /// (through [`ComponentCtx::env`], or a cached `JMethodID` of its own).
    pub fn as_object(&self) -> &jni::objects::JObject<'static> {
        &self.0
    }
}

#[cfg(target_os = "ios")]
impl NativeChild {
    /// The retained child, for the `objc2-ui-kit` setters a component's
    /// `update` calls on it. Takes the same main-thread proof
    /// [`AppleHandle::view`](crate::registry::apple::AppleHandle::view) does —
    /// a typestate guard, not a runtime cost.
    pub fn view(&self, _mtm: objc2::MainThreadMarker) -> &objc2_ui_kit::UIView {
        &self.0
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl NativeChild {
    /// A counted stand-in handle with the given identity.
    fn new(identity: u64) -> Self {
        LIVE_CHILDREN.with(|live| live.set(live.get() + 1));
        Self(HostChild { identity })
    }

    /// This child's stand-in identity — what a test asserts against.
    pub fn identity(&self) -> u64 {
        self.0.identity
    }
}

/// The host arm's stand-in child handle: it decrements [`LIVE_CHILDREN`] when
/// dropped, exactly as Android's `Global` issues its `DeleteGlobalRef` — the
/// leak bar `crate::registry`'s own `CountingHandle` established, one table
/// over.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) struct HostChild {
    identity: u64,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl Drop for HostChild {
    fn drop(&mut self) {
        LIVE_CHILDREN.with(|live| live.set(live.get().saturating_sub(1)));
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
thread_local! {
    /// How many host stand-in child handles are alive on this thread — the
    /// mirror of ART's live-global-ref count (`research/SPIKE.md`'s "52 at
    /// peak → 0 after the dispose cycle"). Thread-local for the same reason
    /// [`STAGED`] is, which also keeps each `cargo test` thread's count its
    /// own.
    static LIVE_CHILDREN: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many retained subtree children are currently alive — the host arm's
/// leak bar, which every create/dispose cycle must return to `0`.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[allow(dead_code)] // the leak bar's only caller is this module's own tests
pub(crate) fn live_child_count() -> usize {
    LIVE_CHILDREN.with(|live| live.get())
}

/// A platform listener firing for one component slot, as the generic listener
/// glue delivers it: a primitive `(kind, detail)` pair, never an allocation on
/// the hot path.
///
/// **This bypasses `RenderRoot::event` entirely** (the crate doc): it is a
/// platform interaction surfacing as a callback on the main thread, not a
/// frust pointer event — there is no `EventCtx`, no capture, no focus, and no
/// fire-on-up-inside semantics unless the platform control itself has them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeEvent {
    kind: i32,
    detail: i64,
}

impl NativeEvent {
    /// Which listener fired. A component attaching this crate's shared
    /// listener sees that class's own kind codes; one attaching its own
    /// listener sees whatever it sends.
    pub fn kind(self) -> i32 {
        self.kind
    }

    /// The listener's primitive payload; `0` for a kind that carries none.
    pub fn detail(self) -> i64 {
        self.detail
    }

    /// Adapt the runtime's internal wire event.
    fn from_wire(event: WireEvent) -> Self {
        Self {
            kind: event.kind,
            detail: event.detail,
        }
    }
}

// --- registration ------------------------------------------------------------

/// Register `C` under `kind`, so a slot whose params name that kind is served
/// by this component — the module doc's decision **2**.
///
/// Call it once, from app or plugin init, on the platform main thread (a call
/// from any other thread registers into that thread's runtime, which nothing
/// will ever dispatch through, and is a no-op in practice). Returns whether
/// the registration was accepted: **first-wins**, so a `kind` already taken —
/// including the six built-in control kinds this build's backend registers
/// itself — is refused with a warning rather than replaced.
///
/// Registration is explicit on purpose: `inventory`-style link-time discovery
/// is banned in this crate, because a stripped, LTO'd device build is exactly
/// where it fails silently (RESEARCH-NATIVE-COMPONENT §Open questions).
pub fn register_component<C: NativeComponent>(kind: &'static str) -> bool {
    // Also the moment the iOS factory class must exist by, if an app registers
    // long before it mounts anything: `crate::runtime`'s own encode path forces
    // this too, and it is idempotent (a `Once`), so paying it here as well only
    // moves the cost earlier.
    crate::runtime::ensure_platform_factory();
    with_runtime(|runtime| runtime.register_if_free::<Bridge<C>>(kind)).unwrap_or(false)
}

// --- the typed props channel -------------------------------------------------

/// One slot's staged component value and props, as [`publish`] left them.
struct Staged {
    /// `Rc<C>` for the registered `C`, erased — cloned into the instance at
    /// create/update time.
    component: Rc<dyn Any>,
    /// `C::Props`, erased.
    props: Box<dyn Any>,
    /// Bumped by [`publish`] only when the incoming props differ from these —
    /// the wire-visible change signal (module doc's *Props travel beside the
    /// wire*).
    generation: u64,
}

thread_local! {
    /// Per-slot staged props, main-thread-confined exactly like the runtime
    /// itself. A `thread_local!` rather than a `Mutex` global for the same
    /// reason (`crate::runtime`'s *main-thread confinement*): a component value
    /// may hold `!Send` platform handles, which a global would have to forbid.
    static STAGED: RefCell<HashMap<SlotId, Staged>> = RefCell::new(HashMap::new());
}

/// Run `f` against the calling thread's staging table, reporting `None` when
/// it is already borrowed on this thread — the same re-entrancy tolerance
/// `crate::runtime::with_runtime` has, and for the same reason: dropping a
/// pathological re-entrant call (a `Props: Clone` impl that itself publishes)
/// beats panicking anywhere near an FFI boundary.
fn with_staged<T>(f: impl FnOnce(&mut HashMap<SlotId, Staged>) -> T) -> Option<T> {
    STAGED
        .try_with(|cell| match cell.try_borrow_mut() {
            Ok(mut staged) => Some(f(&mut staged)),
            Err(_) => {
                log::warn!("frust-native-widgets: re-entrant component props publish ignored");
                None
            }
        })
        .unwrap_or_default()
}

/// Stage this rebuild's component value and props for `slot`, returning the
/// props **generation** the slot's `params_json` must carry
/// ([`component_params`]).
///
/// The generation changes if and only if `props` differ from what is already
/// staged, which is what makes the platform-view differ emit an `UpdateParams`
/// exactly when a component's props actually changed — and nothing at all when
/// they did not.
///
/// Publishing is idempotent and replaces outright: props are whole state, never
/// a delta (module doc's lifecycle contract, point 2).
///
/// The component arrives as an `Rc` the mounting builder already holds
/// (`crate::api::mount`), so a rebuild that changes nothing costs one
/// refcount bump rather than a deep clone of the app's component value — and
/// the public trait needs no `Clone` bound as a result.
pub(crate) fn publish<C: NativeComponent>(slot: SlotId, component: Rc<C>, props: C::Props) -> u64 {
    with_staged(|staged| {
        let generation = match staged.get(&slot) {
            // A slot whose staged props are a *different* component's (a kind
            // swap on a re-used slot id) counts as changed, not as equal.
            Some(previous) => {
                let unchanged = previous
                    .props
                    .downcast_ref::<C::Props>()
                    .is_some_and(|staged| *staged == props);
                if unchanged {
                    previous.generation
                } else {
                    previous.generation.wrapping_add(1)
                }
            }
            None => 0,
        };
        staged.insert(
            slot,
            Staged {
                component,
                props: Box::new(props),
                generation,
            },
        );
        generation
    })
    .unwrap_or(0)
}

/// Drop `slot`'s staged component and props — the teardown reaper, registered
/// from the mounting widget's `on_cleanup` exactly like
/// `crate::runtime`'s `forget_pending_callback` (c1-01).
///
/// **Disposal alone does not bound this table.** A culled slot's dispose
/// resolves by native-view identity and never names a slot id, so a slot
/// disposed while off-screen and then re-published by a still-mounted widget
/// would strand its entry for the process lifetime without this. Idempotent: a
/// slot with nothing staged is a silent no-op.
pub(crate) fn forget(slot: SlotId) {
    with_staged(|staged| staged.remove(&slot));
}

/// The `params_json` a component slot's `platform_view` carries: the runtime's
/// two identity keys plus the props generation [`publish`] returned, and
/// nothing else — a component's real props never cross the wire.
pub(crate) fn component_params(kind: &str, slot: SlotId, generation: u64) -> String {
    crate::runtime::with_identity(
        kind,
        slot,
        &format!("\"{PROPS_GENERATION_KEY}\":{generation}"),
    )
}

/// How many slots currently have staged props — the staging table's own leak
/// bar, which every mount/unmount cycle must return to `0`.
#[allow(dead_code)] // the staging table's leak bar: tests only, by design
pub(crate) fn staged_count() -> usize {
    with_staged(|staged| staged.len()).unwrap_or(0)
}

/// `slot`'s staged props, cloned for the runtime's own baseline copy.
fn staged_props<C: NativeComponent>(slot: SlotId) -> Option<C::Props> {
    with_staged(|staged| {
        staged
            .get(&slot)
            .and_then(|entry| entry.props.downcast_ref::<C::Props>())
            .cloned()
    })
    .flatten()
}

/// `slot`'s staged component value.
fn staged_component<C: NativeComponent>(slot: SlotId) -> Option<Rc<C>> {
    with_staged(|staged| {
        staged
            .get(&slot)
            .and_then(|entry| Rc::clone(&entry.component).downcast::<C>().ok())
    })
    .flatten()
}

// --- the bridge to the internal runtime --------------------------------------

/// One internal `NativeWidget` impl standing in for **every** public
/// [`NativeComponent`] — the module doc's bridge.
///
/// Never instantiated: like the six controls' own marker types it exists only
/// to name a vtable ([`register_component`] registers `Bridge<C>`, and the
/// runtime's dispatch table holds the monomorphised shims).
pub(crate) struct Bridge<C>(PhantomData<fn() -> C>);

/// [`Bridge`]'s props: the component's own, plus the slot id the runtime's
/// identity keys carried — which is how `create`/`update` find their way back
/// to the staging table.
pub(crate) struct BridgeProps<C: NativeComponent> {
    slot: SlotId,
    props: C::Props,
}

impl<C: NativeComponent> Clone for BridgeProps<C> {
    fn clone(&self) -> Self {
        Self {
            slot: self.slot,
            props: self.props.clone(),
        }
    }
}

impl<C: NativeComponent> PartialEq for BridgeProps<C> {
    /// The props diff gate itself: the slot id is part of the comparison for
    /// completeness, but it is the component's own `PartialEq` that decides
    /// whether anything crosses the FFI boundary.
    fn eq(&self, other: &Self) -> bool {
        self.slot == other.slot && self.props == other.props
    }
}

/// [`Bridge`]'s state: the newest component value this slot has resolved,
/// beside the component's own state and the slot id both are keyed by.
///
/// The value is **retained as a fallback, not as the source of truth**: every
/// dispatch that carries a `&self` into the public trait re-reads the staging
/// table first ([`Self::refresh_component`]), because the app constructs its
/// component value fresh on every rebuild and only the staging table sees all
/// of them. The retained copy answers the calls that arrive with no staged
/// entry left to read — a dispose landing after the mounting widget's
/// `forget` reaper already ran (`crate::api::mount`'s `on_cleanup`), which on
/// the primary teardown path is the *usual* order, since `retire`'s `Dispose`
/// is drained a frame or more later.
pub(crate) struct BridgeState<C: NativeComponent> {
    /// The slot whose staged entry [`Self::refresh_component`] re-reads. Slot
    /// ids are handed out by a process-wide monotonic counter
    /// (`crate::api::builders`' `next_local_slot`) and never recycled, so this
    /// can only ever name this slot's own staged entry.
    slot: SlotId,
    component: Rc<C>,
    state: C::State,
}

impl<C: NativeComponent> BridgeState<C> {
    /// Re-read this slot's staged component value, so the call about to run
    /// sees the value the app published on its **most recent rebuild** — the
    /// guarantee [`NativeComponent::on_event`] states.
    ///
    /// This cannot be left to [`Bridge::update`] alone: the runtime's props
    /// diff gate (`crate::runtime`'s `update_params`) returns `Unchanged`
    /// before touching the vtable's `update` at all, so a rebuild that
    /// republishes equal props with a functionally different component value —
    /// new closures capturing a loop index, a different `Rc` — would otherwise
    /// leave the retained value stale and run the *old* closures on the next
    /// event or dispose (review p3 M1).
    ///
    /// A slot with nothing staged (its `forget` reaper already ran) keeps the
    /// retained value: there is no newer value to be had, and nothing here
    /// resurrects a reaped entry or panics on its absence.
    fn refresh_component(&mut self) {
        if let Some(component) = staged_component::<C>(self.slot) {
            self.component = component;
        }
    }
}

impl<C: NativeComponent> NativeWidget for Bridge<C> {
    type Props = BridgeProps<C>;
    type State = BridgeState<C>;

    /// The wire carries only identity and a generation, so "decoding" a
    /// component's props means taking the typed value the app staged for this
    /// slot (module doc's *Props travel beside the wire*).
    fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
        let (_, slot) = params.identity()?;
        let props = staged_props::<C>(slot).ok_or_else(|| {
            NativeWidgetError::Params(format!(
                "native component slot {slot} has no published props"
            ))
        })?;
        Ok(BridgeProps { slot, props })
    }

    fn create(
        ctx: &mut PlatformCtx<'_, '_>,
        props: &Self::Props,
    ) -> Result<(NativeView, Self::State), NativeWidgetError> {
        let component = staged_component::<C>(props.slot).ok_or_else(|| {
            NativeWidgetError::Params(format!(
                "native component slot {} has no published component",
                props.slot
            ))
        })?;

        let mut cx = ComponentCtx::new(ctx);
        let built = component.create(&mut cx, &props.props);
        let latched = cx.into_error();
        match built {
            Some((root, state)) => {
                // The impl had the final say (the trait's `create` doc): a
                // latched error it recovered from is a warning, not a dead slot.
                if let Some(error) = latched {
                    log::warn!(
                        "frust-native-widgets: component slot {} created despite: {error}",
                        props.slot
                    );
                }
                Ok((
                    root.into_inner(),
                    BridgeState {
                        slot: props.slot,
                        component,
                        state,
                    },
                ))
            }
            None => Err(latched.unwrap_or_else(|| {
                NativeWidgetError::Platform(format!(
                    "native component slot {} built no view",
                    props.slot
                ))
            })),
        }
    }

    fn update(
        ctx: &mut PlatformCtx<'_, '_>,
        state: &mut Self::State,
        old: &Self::Props,
        new: &Self::Props,
    ) -> Result<(), NativeWidgetError> {
        // The app constructs its component value fresh every rebuild, so the
        // newest staged one wins here; a slot whose entry was already reaped
        // keeps the one it was created with.
        state.refresh_component();
        let component = Rc::clone(&state.component);
        let mut cx = ComponentCtx::new(ctx);
        component.update(&mut cx, &mut state.state, &old.props, &new.props);
        match cx.into_error() {
            // Reported, so the runtime keeps `old` as the diff baseline and
            // the change is retried on the next differing props.
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Always `None`: a public component is meant to handle its own events
    /// (its `&self` is the value the app published, closures and all), so
    /// nothing rides the six built-in controls' `EventPayload` callback
    /// channel. Unconditional, which is also why nothing in production reaches
    /// this method at all — the attach half a component would need does not
    /// exist yet ([`NativeComponent::on_event`]); the only callers today are
    /// this module's own tests, driving the runtime directly.
    ///
    /// The staged value is re-read first — an event can arrive after any
    /// number of rebuilds that changed the component but not its props, and
    /// the props diff gate skips `update` on every one of them
    /// ([`BridgeState::refresh_component`]).
    fn on_event(state: &mut Self::State, event: WireEvent) -> Option<EventPayload> {
        state.refresh_component();
        let component = Rc::clone(&state.component);
        component.on_event(&mut state.state, NativeEvent::from_wire(event));
        None
    }

    /// The staged value is re-read first, for the same reason `on_event` does
    /// it: a dispose reaching a still-mounted slot (the differ's culling
    /// backstop, a `suspend_all` on surface teardown) must run the component
    /// the app last published, not whatever the last props change left behind
    /// ([`BridgeState::refresh_component`]).
    fn dispose(
        ctx: &mut PlatformCtx<'_, '_>,
        mut state: Self::State,
    ) -> Result<(), NativeWidgetError> {
        state.refresh_component();
        let BridgeState {
            slot: _,
            component,
            state,
        } = state;
        let mut cx = ComponentCtx::new(ctx);
        component.dispose(&mut cx, state);
        match cx.into_error() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

// Gated on the host arm, not merely on `test` (the same gate `crate::runtime`'s
// own tests carry): these drive the public trait through the real runtime using
// the host stand-in context, which a platform build replaces with the
// device-only types.
#[cfg(all(test, not(any(target_os = "android", target_os = "ios"))))]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::runtime::{DisposeOutcome, UpdateOutcome};

    /// A component defined **outside the six** — the whole point of the
    /// acceptance bar: it implements nothing but the public
    /// [`NativeComponent`] trait, using only the public [`ComponentCtx`]/
    /// [`NativeRoot`]/[`NativeEvent`] surface, exactly as a third-party crate
    /// would.
    struct Gauge {
        /// Where this component records what it did. `Rc` on purpose: a test
        /// holding the other end can assert both the calls *and* (via
        /// `strong_count`) that the runtime actually dropped the component
        /// value rather than stranding it — the leak probe the c1-01/f2-03
        /// tests established for every slot-keyed table in this crate.
        log: Rc<RefCell<Vec<String>>>,
        /// Distinguishes the component *value* from its props, so the tests
        /// can prove `update` sees the value the app published this rebuild
        /// rather than the one `create` ran with.
        tag: &'static str,
    }

    #[derive(Clone, Debug, PartialEq)]
    struct GaugeProps {
        label: String,
        value: i32,
    }

    struct GaugeState {
        identity: u64,
        events: Vec<NativeEvent>,
    }

    const GAUGE_KIND: &str = "test-gauge";

    /// The [`GaugeProps::label`] that makes `create` answer `None` — the
    /// failed-create test's opt-in, mirroring `crate::runtime`'s own
    /// `FAIL_CREATE` sentinel.
    const FAIL_CREATE: &str = "FAIL_CREATE";

    /// The [`GaugeProps::label`] that makes `update` latch an error.
    const FAIL_UPDATE: &str = "FAIL_UPDATE";

    impl Gauge {
        fn new(log: &Rc<RefCell<Vec<String>>>, tag: &'static str) -> Self {
            Self {
                log: Rc::clone(log),
                tag,
            }
        }

        fn note(&self, entry: String) {
            self.log.borrow_mut().push(entry);
        }
    }

    impl NativeComponent for Gauge {
        type Props = GaugeProps;
        type State = GaugeState;

        fn create(
            &self,
            ctx: &mut ComponentCtx<'_, '_, '_>,
            props: &Self::Props,
        ) -> Option<(NativeRoot, Self::State)> {
            if props.label == FAIL_CREATE {
                ctx.report_error("gauge: no native view today");
                return None;
            }
            let identity = next_identity();
            ctx.record(format!("gauge create {identity} '{}'", props.label));
            self.note(format!("{} create '{}'", self.tag, props.label));
            let root = ctx.root(identity)?;
            Some((
                root,
                GaugeState {
                    identity,
                    events: Vec::new(),
                },
            ))
        }

        fn update(
            &self,
            ctx: &mut ComponentCtx<'_, '_, '_>,
            state: &mut Self::State,
            old: &Self::Props,
            new: &Self::Props,
        ) {
            if new.label == FAIL_UPDATE {
                ctx.report_error("gauge: setter threw");
                return;
            }
            ctx.record(format!(
                "gauge update {} {} -> {}",
                state.identity, old.value, new.value
            ));
            self.note(format!(
                "{} update {} -> {}",
                self.tag, old.value, new.value
            ));
        }

        fn on_event(&self, state: &mut Self::State, event: NativeEvent) {
            state.events.push(event);
            self.note(format!(
                "{} event {}/{}",
                self.tag,
                event.kind(),
                event.detail()
            ));
        }

        fn dispose(&self, ctx: &mut ComponentCtx<'_, '_, '_>, state: Self::State) {
            ctx.record(format!("gauge dispose {}", state.identity));
            self.note(format!("{} dispose", self.tag));
        }
    }

    /// Monotonic stand-in for "the platform handed us a fresh object".
    fn next_identity() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    fn props(label: &str, value: i32) -> GaugeProps {
        GaugeProps {
            label: label.to_string(),
            value,
        }
    }

    /// Publish `component` + `props` for `slot` and return the `params_json`
    /// its `platform_view` would carry — the exact sequence the app-facing
    /// builder will run on every rebuild.
    fn mount(slot: SlotId, component: Gauge, props: GaugeProps) -> String {
        let generation = publish(slot, Rc::new(component), props);
        component_params(GAUGE_KIND, slot, generation)
    }

    #[test]
    fn a_component_outside_the_six_lives_the_whole_lifecycle() {
        // The p3-01 acceptance bar: create → update → event → dispose, driven
        // by the real runtime through the public trait alone.
        let log = Rc::new(RefCell::new(Vec::new()));
        assert!(register_component::<Gauge>(GAUGE_KIND));

        let params = mount(1, Gauge::new(&log, "first"), props("CPU", 10));
        let mut calls = Vec::new();
        {
            let mut ctx = PlatformCtx::new(&mut calls);
            with_runtime(|runtime| {
                assert_eq!(runtime.create(&mut ctx, &params).unwrap(), 1);
                assert_eq!(runtime.live_count(), 1);

                // Same props republished: the diff gate stops before the
                // component is asked to do anything at all.
                let params = mount(1, Gauge::new(&log, "second"), props("CPU", 10));
                assert_eq!(
                    runtime.update_params(&mut ctx, &params).unwrap(),
                    UpdateOutcome::Unchanged
                );

                // Changed props: applied once, and by the component value the
                // app published most recently.
                let params = mount(1, Gauge::new(&log, "third"), props("CPU", 42));
                assert_eq!(
                    runtime.update_params(&mut ctx, &params).unwrap(),
                    UpdateOutcome::Applied
                );

                runtime.on_event(1, WireEvent { kind: 1, detail: 7 });
                assert_eq!(runtime.dispose_slot(&mut ctx, 1), DisposeOutcome::Disposed);
                assert_eq!(runtime.live_count(), 0);
            })
            .expect("the thread's runtime");
        }

        assert_eq!(
            *log.borrow(),
            vec![
                "first create 'CPU'".to_string(),
                "third update 10 -> 42".to_string(),
                "third event 1/7".to_string(),
                "third dispose".to_string(),
            ],
            "one create, one update (the equal republish crossed nothing), \
             and both later calls ran on the newest published component value"
        );
        assert_eq!(calls.len(), 3, "one platform call each: {calls:?}");
        assert!(calls[0].starts_with("gauge create"));
        assert!(calls[1].contains("10 -> 42"));
        assert!(calls[2].starts_with("gauge dispose"));
    }

    #[test]
    fn an_unchanged_props_republish_still_reaches_the_newest_component_value() {
        // Review p3 M1, the regression this test exists for: the props diff
        // gate (`crate::runtime`'s `update_params`) returns `Unchanged` before
        // touching the vtable, and `update` used to be the ONLY thing that
        // refreshed the bridge's retained component value — so a rebuild that
        // republished equal props with functionally different closures left
        // `on_event`/`dispose` running the value `create` ran with.
        //
        // Two distinct log sinks stand in for those closures: each rebuild's
        // component captures its own `Rc`, exactly as an app's `move |v|
        // sig.set(v)` captures this rebuild's signal.
        let first = Rc::new(RefCell::new(Vec::new()));
        let second = Rc::new(RefCell::new(Vec::new()));
        register_component::<Gauge>(GAUGE_KIND);

        let params = mount(20, Gauge::new(&first, "first"), props("CPU", 10));
        let mut calls = Vec::new();
        {
            let mut ctx = PlatformCtx::new(&mut calls);
            with_runtime(|runtime| {
                runtime.create(&mut ctx, &params).unwrap();

                // The rebuild the defect hid behind: same props, a different
                // component value. The wire is byte-identical, so the differ
                // emits no `UpdateParams` at all — nothing calls `update`, and
                // this republish is the whole of what the runtime is told.
                let republished = mount(20, Gauge::new(&second, "second"), props("CPU", 10));
                assert_eq!(
                    republished, params,
                    "an equal-props rebuild leaves the wire untouched, which is \
                     exactly why `update` never runs to refresh anything"
                );

                runtime.on_event(20, WireEvent { kind: 3, detail: 9 });
                assert_eq!(runtime.dispose_slot(&mut ctx, 20), DisposeOutcome::Disposed);
                assert_eq!(runtime.live_count(), 0);
            })
            .expect("the thread's runtime");
        }
        forget(20);

        assert_eq!(
            *first.borrow(),
            vec!["first create 'CPU'".to_string()],
            "the value `create` ran with must not keep serving a later \
             rebuild's events and disposal"
        );
        assert_eq!(
            *second.borrow(),
            vec!["second event 3/9".to_string(), "second dispose".to_string(),],
            "both later calls ran on the component value the app published \
             most recently — the trait's `on_event` guarantee, literally"
        );
    }

    #[test]
    fn a_dispose_after_the_staging_reaper_keeps_the_retained_component() {
        // The ordering seam of the fix above: on the PRIMARY teardown path the
        // mounting widget's `forget` reaper (`crate::api::mount`'s
        // `on_cleanup`) runs a frame or more BEFORE `retire`'s `Dispose` is
        // drained, so the staging lookup finds nothing by then. The retained
        // value answers, nothing resurrects the reaped entry, nothing panics —
        // and there is no newer value to be had, because a torn-down widget
        // published none.
        let first = Rc::new(RefCell::new(Vec::new()));
        let second = Rc::new(RefCell::new(Vec::new()));
        register_component::<Gauge>(GAUGE_KIND);

        let params = mount(21, Gauge::new(&first, "first"), props("Mem", 1));
        let mut calls = Vec::new();
        {
            let mut ctx = PlatformCtx::new(&mut calls);
            with_runtime(|runtime| {
                runtime.create(&mut ctx, &params).unwrap();
                mount(21, Gauge::new(&second, "second"), props("Mem", 1));

                // Teardown order: the reaper first, the native dispose after.
                forget(21);
                assert_eq!(staged_count(), 0);
                assert_eq!(runtime.dispose_slot(&mut ctx, 21), DisposeOutcome::Disposed);
                assert_eq!(runtime.live_count(), 0);
            })
            .expect("the thread's runtime");
        }

        assert_eq!(
            *first.borrow(),
            vec![
                "first create 'Mem'".to_string(),
                "first dispose".to_string()
            ],
            "with the staged entry gone the retained value runs the dispose"
        );
        assert!(
            second.borrow().is_empty(),
            "a reaped entry is never resurrected: {:?}",
            second.borrow()
        );
    }

    /// A control shaped exactly like the six built-ins: an **internal**
    /// `NativeWidget`, props decoded out of `params_json`, no staging table
    /// involved. Its only job here is to prove the two trait families share one
    /// dispatch table, since the real six compile on device targets only.
    struct LegacyControl;

    #[derive(Clone, Debug, PartialEq)]
    struct LegacyProps {
        text: String,
    }

    const LEGACY_KIND: &str = "test-legacy";

    impl NativeWidget for LegacyControl {
        type Props = LegacyProps;
        type State = u64;

        fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
            Ok(LegacyProps {
                text: params
                    .string("text")
                    .ok_or_else(|| NativeWidgetError::Params("no `text`".into()))?
                    .into_owned(),
            })
        }

        fn create(
            ctx: &mut PlatformCtx<'_, '_>,
            props: &Self::Props,
        ) -> Result<(NativeView, Self::State), NativeWidgetError> {
            let identity = next_identity();
            ctx.record(format!("legacy create {identity} '{}'", props.text));
            Ok((NativeView { identity }, identity))
        }

        fn update(
            ctx: &mut PlatformCtx<'_, '_>,
            state: &mut Self::State,
            _old: &Self::Props,
            new: &Self::Props,
        ) -> Result<(), NativeWidgetError> {
            ctx.record(format!("legacy update {state} '{}'", new.text));
            Ok(())
        }

        fn dispose(
            ctx: &mut PlatformCtx<'_, '_>,
            state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            ctx.record(format!("legacy dispose {state}"));
            Ok(())
        }
    }

    #[test]
    fn a_public_component_and_an_internal_widget_share_one_dispatch_table() {
        // The bridge's actual claim (module doc): a public `NativeComponent`
        // is not a second runtime beside the six controls' — it is the same
        // registry, the same diff gate, the same disposal path, dispatched by
        // kind. Both families live side by side here, in one runtime, with no
        // cross-talk.
        let log = Rc::new(RefCell::new(Vec::new()));
        assert!(register_component::<Gauge>(GAUGE_KIND));
        assert!(
            with_runtime(|runtime| runtime.register_if_free::<LegacyControl>(LEGACY_KIND))
                .expect("the thread's runtime")
        );

        let component = mount(10, Gauge::new(&log, "g"), props("Load", 1));
        let legacy = crate::runtime::with_identity(LEGACY_KIND, 11, "\"text\":\"Save\"");
        let mut calls = Vec::new();
        {
            let mut ctx = PlatformCtx::new(&mut calls);
            with_runtime(|runtime| {
                runtime.create(&mut ctx, &component).unwrap();
                runtime.create(&mut ctx, &legacy).unwrap();
                assert_eq!(runtime.live_count(), 2);

                // Each one's diff gate answers for its own props only.
                assert_eq!(
                    runtime.update_params(&mut ctx, &legacy).unwrap(),
                    UpdateOutcome::Unchanged
                );
                let changed = mount(10, Gauge::new(&log, "g"), props("Load", 2));
                assert_eq!(
                    runtime.update_params(&mut ctx, &changed).unwrap(),
                    UpdateOutcome::Applied
                );

                assert_eq!(runtime.dispose_slot(&mut ctx, 10), DisposeOutcome::Disposed);
                assert_eq!(runtime.dispose_slot(&mut ctx, 11), DisposeOutcome::Disposed);
                assert_eq!(runtime.live_count(), 0, "the shared leak bar returns to 0");
            })
            .expect("the thread's runtime");
        }
        forget(10);

        let kinds: Vec<&str> = calls
            .iter()
            .map(|call| call.split(' ').next().unwrap())
            .collect();
        assert_eq!(
            kinds,
            vec!["gauge", "legacy", "gauge", "gauge", "legacy"],
            "each command reached its own kind's impl and nobody else's: {calls:?}"
        );
    }

    #[test]
    fn the_staging_table_is_reaped_by_forget_and_strands_nothing() {
        // The c1-01 leak shape, one table over: `forget` (the mounting
        // widget's `on_cleanup`) is the bound, not disposal — Android's
        // production dispose resolves by view identity and never names a slot.
        let log = Rc::new(RefCell::new(Vec::new()));
        register_component::<Gauge>(GAUGE_KIND);

        let params = mount(2, Gauge::new(&log, "only"), props("Mem", 1));
        assert_eq!(staged_count(), 1);
        assert_eq!(Rc::strong_count(&log), 2, "the staging table holds it");

        let mut calls = Vec::new();
        {
            let mut ctx = PlatformCtx::new(&mut calls);
            with_runtime(|runtime| {
                runtime.create(&mut ctx, &params).unwrap();
                // The identity-resolved dispose the differ actually emits.
                let identity = runtime.instance(2).unwrap().view().identity;
                let (slot, instance) = runtime
                    .take_matching(|view| view.identity == identity)
                    .expect("the live view");
                assert_eq!(slot, 2);
                instance.dispose(&mut ctx).unwrap();
                assert_eq!(runtime.live_count(), 0);
            })
            .expect("the thread's runtime");
        }
        assert_eq!(
            staged_count(),
            1,
            "disposal alone leaves the staged entry — exactly the c1-01 shape"
        );

        forget(2);
        assert_eq!(staged_count(), 0);
        assert_eq!(
            Rc::strong_count(&log),
            1,
            "the component value was dropped, not merely unreachable"
        );
        // Idempotent.
        forget(2);
        assert_eq!(staged_count(), 0);
    }

    #[test]
    fn publish_bumps_the_generation_only_when_the_props_change() {
        // The wire-visible change signal: identical props must leave
        // `params_json` byte-identical, or the differ would emit an
        // `UpdateParams` for every rebuild and forfeit the whole diff gate.
        let log = Rc::new(RefCell::new(Vec::new()));
        let first = mount(3, Gauge::new(&log, "a"), props("Disk", 1));
        let same = mount(3, Gauge::new(&log, "b"), props("Disk", 1));
        let changed = mount(3, Gauge::new(&log, "c"), props("Disk", 2));

        assert_eq!(
            first, same,
            "an unchanged publish changes nothing on the wire"
        );
        assert_ne!(same, changed);
        assert!(changed.contains(PROPS_GENERATION_KEY));

        forget(3);
        // A fresh mount after teardown starts over at generation 0.
        assert_eq!(mount(3, Gauge::new(&log, "d"), props("Disk", 2)), first);
    }

    #[test]
    fn registration_is_first_wins_and_never_shadows_a_registered_kind() {
        assert!(register_component::<Gauge>(GAUGE_KIND));
        assert!(
            !register_component::<Gauge>(GAUGE_KIND),
            "a second registration is refused, not silently replaced"
        );
    }

    #[test]
    fn a_slot_with_no_published_props_fails_instead_of_dispatching() {
        register_component::<Gauge>(GAUGE_KIND);
        // Params for a slot whose staged entry was already reaped — the
        // replay-after-teardown case.
        let params = component_params(GAUGE_KIND, 4, 0);
        let mut calls = Vec::new();
        let mut ctx = PlatformCtx::new(&mut calls);

        let error = with_runtime(|runtime| runtime.create(&mut ctx, &params))
            .expect("the thread's runtime")
            .expect_err("nothing was published for slot 4");

        assert!(matches!(error, NativeWidgetError::Params(_)), "{error:?}");
        assert!(calls.is_empty(), "nothing crossed the boundary");
    }

    #[test]
    fn a_component_that_builds_no_view_fails_its_slot_with_the_latched_error() {
        let log = Rc::new(RefCell::new(Vec::new()));
        register_component::<Gauge>(GAUGE_KIND);
        let params = mount(5, Gauge::new(&log, "doomed"), props(FAIL_CREATE, 0));

        let mut calls = Vec::new();
        let mut ctx = PlatformCtx::new(&mut calls);
        let error = with_runtime(|runtime| {
            let outcome = runtime.create(&mut ctx, &params);
            assert_eq!(runtime.live_count(), 0, "no instance was retained");
            outcome
        })
        .expect("the thread's runtime")
        .expect_err("the component answered None");

        assert!(
            matches!(&error, NativeWidgetError::Platform(message)
                if message.contains("no native view today")),
            "the latched error names the failure, not a generic one: {error:?}"
        );
        forget(5);
    }

    #[test]
    fn an_update_that_latches_keeps_the_diff_baseline_so_the_change_retries() {
        let log = Rc::new(RefCell::new(Vec::new()));
        register_component::<Gauge>(GAUGE_KIND);
        let params = mount(6, Gauge::new(&log, "g"), props("Net", 1));

        let mut calls = Vec::new();
        {
            let mut ctx = PlatformCtx::new(&mut calls);
            with_runtime(|runtime| {
                runtime.create(&mut ctx, &params).unwrap();

                let failing = mount(6, Gauge::new(&log, "g"), props(FAIL_UPDATE, 2));
                assert!(runtime.update_params(&mut ctx, &failing).is_err());

                // The baseline is still the props `create` applied, so a
                // *different* change still reads as changed rather than being
                // swallowed by a baseline the failed update advanced.
                let recovered = mount(6, Gauge::new(&log, "g"), props("Net", 3));
                assert_eq!(
                    runtime.update_params(&mut ctx, &recovered).unwrap(),
                    UpdateOutcome::Applied
                );
            })
            .expect("the thread's runtime");
        }

        assert_eq!(
            *log.borrow(),
            vec!["g create 'Net'".to_string(), "g update 1 -> 3".to_string()],
            "the failed update applied nothing and left the baseline at create's props"
        );
        forget(6);
    }

    // --- p3-02: a component owns its own native subtree ---------------------

    /// A **composite**: one component, one slot, a parent view with N native
    /// children under it — the card-with-an-image-and-two-buttons shape the
    /// public trait exists to make shippable without leaking three slots to
    /// the consuming app.
    struct Card {
        /// How many children [`NativeComponent::create`] builds.
        children: usize,
        /// Stop after this many children and fail the slot — the
        /// half-built-subtree path, which must strand nothing.
        fail_after: Option<usize>,
        log: Rc<RefCell<Vec<String>>>,
    }

    #[derive(Clone, Debug, PartialEq)]
    struct CardProps {
        title: String,
    }

    struct CardState {
        root: u64,
        /// The children this component kept in order to drive them later.
        /// Dropping `State` — which the runtime does immediately after
        /// `dispose` returns — releases every one of them (module doc's
        /// *Teardown*).
        children: Vec<NativeChild>,
    }

    const CARD_KIND: &str = "test-card";

    impl NativeComponent for Card {
        type Props = CardProps;
        type State = CardState;

        fn create(
            &self,
            ctx: &mut ComponentCtx<'_, '_, '_>,
            props: &Self::Props,
        ) -> Option<(NativeRoot, Self::State)> {
            let root = next_identity();
            ctx.record(format!("card create {root} '{}'", props.title));
            // Every child is built inside ONE local frame — the discipline
            // spike 1 proved on device with fifty of them (module doc).
            let children = ctx.with_local_frame(self.children + 2, |ctx| {
                let mut retained = Vec::with_capacity(self.children);
                for index in 0..self.children {
                    if self.fail_after == Some(index) {
                        ctx.report_error("card: the platform ran out of views");
                        return None;
                    }
                    let child = next_identity();
                    ctx.add_child(root, child)?;
                    // Kept because this component drives its children later;
                    // a child it never touched again would need no handle.
                    retained.push(ctx.retain_child(child)?);
                }
                Some(retained)
            })?;
            self.log
                .borrow_mut()
                .push(format!("card create {} children", children.len()));
            Some((ctx.root(root)?, CardState { root, children }))
        }

        fn update(
            &self,
            ctx: &mut ComponentCtx<'_, '_, '_>,
            state: &mut Self::State,
            _old: &Self::Props,
            new: &Self::Props,
        ) {
            // A real component would drive individual children here, off the
            // handles `State` retained.
            ctx.record(format!(
                "card update {} '{}' over {} children",
                state.root,
                new.title,
                state.children.len()
            ));
        }

        fn dispose(&self, ctx: &mut ComponentCtx<'_, '_, '_>, state: Self::State) {
            ctx.record(format!(
                "card dispose {} with {} children",
                state.root,
                state.children.len()
            ));
            self.log.borrow_mut().push("card dispose".to_string());
            // `state` — every retained child with it — drops as this returns.
        }
    }

    /// Publish `card` for `slot` and return the `params_json` its
    /// `platform_view` would carry.
    fn mount_card(slot: SlotId, card: Card, title: &str) -> String {
        let generation = publish(
            slot,
            Rc::new(card),
            CardProps {
                title: title.to_string(),
            },
        );
        component_params(CARD_KIND, slot, generation)
    }

    #[test]
    fn a_component_builds_a_native_subtree_and_releases_every_child() {
        // Spike 1's own stress count (`research/SPIKE.md`: 50 children in ONE
        // slot, 52 global refs at peak → 0 after the dispose cycle).
        const CHILDREN: usize = 50;

        assert_eq!(live_child_count(), 0, "this test thread starts clean");
        assert!(register_component::<Card>(CARD_KIND));

        let log = Rc::new(RefCell::new(Vec::new()));
        let params = mount_card(
            7,
            Card {
                children: CHILDREN,
                fail_after: None,
                log: Rc::clone(&log),
            },
            "Now playing",
        );

        let mut calls = Vec::new();
        {
            let mut ctx = PlatformCtx::new(&mut calls);
            with_runtime(|runtime| {
                assert_eq!(runtime.create(&mut ctx, &params).unwrap(), 7);
                assert_eq!(
                    runtime.live_count(),
                    1,
                    "N children ship as ONE slot — that is the whole point"
                );
                assert_eq!(
                    live_child_count(),
                    CHILDREN,
                    "every child is retained while the component is live"
                );

                let changed = mount_card(
                    7,
                    Card {
                        children: CHILDREN,
                        fail_after: None,
                        log: Rc::clone(&log),
                    },
                    "Up next",
                );
                assert_eq!(
                    runtime.update_params(&mut ctx, &changed).unwrap(),
                    UpdateOutcome::Applied,
                    "a subtree component diffs exactly like a leaf one"
                );

                assert_eq!(runtime.dispose_slot(&mut ctx, 7), DisposeOutcome::Disposed);
                assert_eq!(runtime.live_count(), 0);
            })
            .expect("the thread's runtime");
        }
        forget(7);

        // The teardown bar, counted rather than assumed: the M1/c1-01 leaks
        // both looked fine until something counted.
        assert_eq!(
            live_child_count(),
            0,
            "every child was released with its parent"
        );
        assert_eq!(staged_count(), 0);

        // …and the plan the component actually executed.
        assert!(calls[0].starts_with("card create"), "{calls:?}");
        assert_eq!(calls[1], format!("pushLocalFrame {}", CHILDREN + 2));
        let root = calls[0]
            .split(' ')
            .nth(2)
            .expect("the root identity")
            .to_string();
        let attached: Vec<&String> = calls
            .iter()
            .filter(|call| call.starts_with("addChild "))
            .collect();
        assert_eq!(attached.len(), CHILDREN, "one addChild per child");
        assert!(
            attached
                .iter()
                .all(|call| call.starts_with(&format!("addChild {root} <- "))),
            "every child went under the component's own root: {attached:?}"
        );
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.starts_with("retainChild "))
                .count(),
            CHILDREN
        );

        let first_add = calls
            .iter()
            .position(|call| call.starts_with("addChild "))
            .expect("an addChild");
        let last_add = calls
            .iter()
            .rposition(|call| call.starts_with("addChild "))
            .expect("an addChild");
        let popped = calls
            .iter()
            .position(|call| call == "popLocalFrame")
            .expect("the frame pops");
        assert!(
            first_add > 1 && last_add < popped,
            "the whole subtree build ran inside ONE local frame: {calls:?}"
        );
        assert!(calls[popped + 1].contains("over 50 children"), "{calls:?}");
        assert!(
            calls.last().unwrap().starts_with("card dispose"),
            "{calls:?}"
        );
        assert_eq!(
            *log.borrow(),
            vec![
                "card create 50 children".to_string(),
                "card dispose".to_string()
            ],
        );
    }

    #[test]
    fn a_half_built_subtree_strands_no_children() {
        // The failure path of the same discipline: a component that gives up
        // partway through its subtree must release what it already retained,
        // and leave no live instance behind either.
        assert_eq!(live_child_count(), 0);
        register_component::<Card>(CARD_KIND);

        let log = Rc::new(RefCell::new(Vec::new()));
        let params = mount_card(
            8,
            Card {
                children: 20,
                fail_after: Some(12),
                log: Rc::clone(&log),
            },
            "doomed",
        );

        let mut calls = Vec::new();
        let mut ctx = PlatformCtx::new(&mut calls);
        let error = with_runtime(|runtime| {
            let outcome = runtime.create(&mut ctx, &params);
            assert_eq!(runtime.live_count(), 0, "no instance was retained");
            outcome
        })
        .expect("the thread's runtime")
        .expect_err("the component gave up on its subtree");

        assert!(
            matches!(&error, NativeWidgetError::Platform(message)
                if message.contains("ran out of views")),
            "the latched error names the failure: {error:?}"
        );
        assert_eq!(
            live_child_count(),
            0,
            "the twelve children built before the failure were released, not stranded"
        );
        assert!(
            calls.iter().any(|call| call == "popLocalFrame"),
            "the local frame is popped on the failure path too: {calls:?}"
        );
        forget(8);
    }
}
