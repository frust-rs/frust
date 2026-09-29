//! [`NativeComponent`] — the **public** trait a plugin author writes a native
//! component against, from pure Rust, with no per-component Kotlin or Swift.
//!
//! **An app crate cannot implement this trait**: `create` constructs real native
//! views, which means naming `jni`/`objc2-ui-kit` types in the implementing
//! crate, and this plugin re-exports neither FFI crate — see
//! `docs/NATIVE_WIDGETS_ARCHITECTURE.md` for that wall and the audience it
//! leaves. The only implementor here is the non-default `demo-components`
//! composite (`crate::demo`).
//!
//! Where `crate::runtime`'s `NativeWidget` is the plugin's **internal** dispatch
//! contract, this trait is the shape a *third party* implements:
//!
//! | | internal `NativeWidget` | public [`NativeComponent`] |
//! |---|---|---|
//! | receiver | associated functions | `&self` — the value the app constructs each rebuild |
//! | props | decoded from `params_json` inside the impl | **already-typed Rust values** the app hands over, staged beside the wire (*Props travel beside the wire*, below) |
//! | errors | every method returns `Result` | latched on the context ([`ComponentCtx::report_error`]); `create` may answer `None` |
//! | events | decoded into the crate's typed `EventPayload` | the [`NativeEvent`] pair from a listener the component attached ([`ComponentCtx::attach_listener`]), answered with the event (if any) the app's `.on_event` hook receives |
//! | context | the platform's own `NativeCtx` | the opaque [`ComponentCtx`] wrapper |
//!
//! The six v1 controls are **not** ported onto it: they stay internal
//! `NativeWidget` impls, and this module **bridges** to them through
//! `Bridge<C>` (crate-private), one `NativeWidget` impl generic over every
//! public component, so both kinds reach the same runtime, registry, props diff
//! gate and disposal path. Porting them is not on the table — their whole wire
//! is `params_json`, and a public trait implemented *by* an internal one would
//! need a blanket impl that then blocks every third-party impl on coherence
//! grounds. Every guarantee below is therefore the runtime's own.
//!
//! # The lifecycle contract (what the runtime guarantees)
//!
//! **Every method here runs on the platform main thread** — the host's
//! post-frame command poll or a platform listener firing, never a frust rebuild
//! and never off-thread (`crate::runtime`'s *main-thread confinement*).
//!
//! 1. **`create` arrives a frame or more after the widget mounts.** Mounting a
//!    slot publishes a `Create` command; the host drains its backlog on the next
//!    post-frame poll, and *that* is what calls [`NativeComponent::create`].
//!    Anything the component retains lives in [`NativeComponent::State`], born
//!    there — there is nothing native to hold before it.
//! 2. **Props coalesce until then, and are always whole state, never a delta.**
//!    Each rebuild replaces a slot's staged props outright, so a create landing
//!    after three rebuilds sees only the newest, and replaying a backlog prefix
//!    (a surface-recreate replay, a compaction) lands in the same place.
//! 3. **`update` runs only when props actually differ.** The runtime compares
//!    the typed props with `PartialEq` **before** any platform call, so an
//!    unchanged rebuild costs zero FFI crossings. The one exception is the
//!    bounded retry window a failed `update` opens (below): the next **two**
//!    rebuilds each cost one dispatch for byte-identical props. Field-level
//!    diffing inside a changed props value is the component's own job — only it
//!    knows which setter is cheap and which forces a re-layout.
//! 4. **`on_event` fires between frames, for the listeners the component
//!    attached.** A component attaches the platform's one listener to any view
//!    it built — root or child — with [`ComponentCtx::attach_listener`], and
//!    from then on that view's clicks/toggles/value changes reach
//!    [`NativeComponent::on_event`] through the same slot-id routing the six
//!    built-in controls use (*Listener attachment*, below). A slot that
//!    attached nothing for an event's family never reaches the method at all.
//!    A native interaction bypasses `RenderRoot::event` entirely: no
//!    `EventCtx`, no capture/focus, none of `docs/CODE_STANDARDS.md`'s
//!    Interaction Semantics. And a listener that fires while the runtime is
//!    already borrowed — the classic case is a setter provoking its own
//!    listener synchronously from inside `update` — is **dropped with a
//!    warning**, not delivered re-entrantly.
//! 5. **The staged-`&self` re-read is component-only, and its `dispose` half is
//!    live today.** The `&self` carried into `on_event`/`dispose` is re-read
//!    from the staging table on **every** dispatch, not only when props changed
//!    (`BridgeState::refresh_component`): the diff gate skips `update` on an
//!    equal-props rebuild, so anything less would run a stale rebuild's closures
//!    ([`NativeComponent::dispose`]). The six controls decode `params_json` and
//!    never read this table.
//! 6. **`dispose` is best-effort-prompt, and may be late** — below.
//!
//! # Three design decisions this trait settles
//!
//! **1. The context a third-party impl receives** is [`ComponentCtx`], an
//! **opaque wrapper** rather than the plugin's own per-platform `NativeCtx`, so
//! the internal helper surface stays free to change without breaking a public
//! impl; it also holds the error latch, which is what lets the trait's methods
//! stay `Result`-free. Curated helpers can never cover "construct an arbitrary
//! native view", so it carries each platform's own `#[cfg]`-gated escape hatch
//! too (`ComponentCtx::env`, `ComponentCtx::mtm`) — already public types, so no
//! new dependency.
//!
//! **2. A component reaches the dispatch table through**
//! [`register_component`], explicitly, from app or plugin init:
//! `inventory`-style link-time auto-registration stays **banned**, being exactly
//! the mechanism that fails silently in a stripped, LTO'd device build.
//! Registration is **first-wins** — a kind already registered, including any of
//! the six built-in controls (which the backend registers when the thread's
//! runtime is first touched), is refused with a warning rather than replaced, so
//! a third-party kind can never shadow a shipped one.
//!
//! **3. Disposal promptness** is **exactly the guarantee the six controls get,
//! and no more**: the framework's `retire()` (driven from the mounting widget's
//! teardown) is the prompt primary path, and the differ's missing-frame streak
//! is the backstop. That streak only advances on gate-`Run` frames, so on an
//! idle screen a `dispose` can arrive many frames late — or after a replacement
//! `create` already re-used the slot id, in which case the stale command
//! resolves against the *old* view by identity, finds nothing, and is dropped.
//! So `dispose` may run long after the widget disappeared, and at process exit
//! may not run at all: anything whose release cannot wait belongs in `State`,
//! dropped immediately after [`NativeComponent::dispose`] returns, alongside
//! the runtime's paired delete of the [`NativeRoot`].
//!
//! # Listener attachment
//!
//! There is still exactly **one listener class per platform** — Android's
//! `dev.frust.nativewidgets.FrustNativeListener`, and each Apple arm's
//! `FrustNativeControlTarget` — and a component reaches it the same way the
//! six controls do, through one call: [`ComponentCtx::attach_listener`], given
//! the view and the [`ListenerKinds`] to wire (click, toggled, value changed).
//! The context constructs the listener bound to **this slot's own id**, which
//! it knows privately and never hands to the component, so a component cannot
//! route an event anywhere but home. It answers a [`ListenerHandle`] the
//! component keeps in its [`NativeComponent::State`]; dropping it with the
//! state is the release (and, on the two Apple arms, the target-action
//! detach), and [`ComponentCtx::detach_listener`] detaches explicitly.
//!
//! The dispatch then runs the path the six controls already use: the platform
//! listener fires on the main thread, `crate::runtime`'s `on_event` routes it
//! by slot id to this module's `Bridge`, which hands the [`NativeEvent`] (and
//! the component's own state and last-applied props) to
//! [`NativeComponent::on_event`]. Whatever event that answers rides the six
//! controls' `EventPayload` callback table to the app's
//! [`NativeComponentView::on_event`](crate::api::NativeComponentView::on_event)
//! hook — the events-as-signals idiom, unchanged. The bridge remembers which
//! [`ListenerKinds`] the slot attached and drops any other family before the
//! component sees it, so a component that attached nothing still answers
//! nothing.
//!
//! # A component owns its own native subtree
//!
//! One component may build a whole native view *hierarchy* — a parent with
//! native children — and ship it as ONE slot, which is what stops a composite
//! from leaking three slots to the consuming app. Five calls on
//! [`ComponentCtx`], which documents each, are the entire surface: build a
//! child (`ComponentCtx::new_view`, or an `objc2-ui-kit`/`objc2-app-kit`
//! constructor off `ComponentCtx::mtm` — the one `#[cfg]`-gated pair), attach it
//! ([`ComponentCtx::add_child`]), keep talking to it
//! ([`ComponentCtx::retain_child`] → [`NativeChild`]), hear from it
//! ([`ComponentCtx::attach_listener`] → [`ListenerHandle`]), and bound the JNI
//! reference table ([`ComponentCtx::with_local_frame`], a no-op under ARC). The
//! last four exist on every target, and the host arm's stand-ins let an
//! ordinary `cargo test` assert a component's create/update/dispose plan.
//!
//! **The platform lays the subtree out, and frust deliberately does not know
//! the children exist** — the wire carries per-slot geometry only (a `rect`, an
//! optional `clip`, `shields`), so a component positions its own children the
//! platform's way while frust keeps seeing one opaque slot with one rect
//! ([`NativeComponent`]'s *No frust `View` children* has the model and why).
//! **No wire change**: a subtree costs the differ exactly what a single leaf
//! control costs it, and a11y comes out ahead — the platform owns the subtree,
//! so it traverses it natively.
//!
//! **Teardown releases children with the parent**, so peak global refs return
//! to zero over a dispose cycle by construction: a merely *attached* child
//! needs no handle at all (Android's `ViewGroup` holds its own strong
//! reference, UIKit and AppKit retain a subview) and dies with the parent,
//! while a child you keep talking to lives in [`NativeComponent::State`] as a
//! [`NativeChild`], whose `Drop` *is* the release (`DeleteGlobalRef` on
//! Android, `Retained`'s own `Drop` on iOS and macOS). The leak bar is the six controls'
//! own; `tests::a_component_builds_a_native_subtree_and_releases_every_child`
//! counts refs rather than merely surviving the cycle.
//!
//! # Props travel beside the wire, not on it
//!
//! The platform-view wire carries one `params_json` string per slot, and that
//! is what the differ diffs to decide whether to emit an `UpdateParams` at all.
//! A public component's props are typed Rust values that never touch JSON, so
//! they ride a **thread-local staging table** here (written by this module's
//! crate-private `publish`/`forget` pair, whose one production caller is the
//! generic mounting builder) while the slot's `params_json` carries only the
//! runtime's two identity keys plus a props **generation** counter that
//! `publish` bumps when — and only when — the published props actually changed.
//! The wire therefore changes exactly when the props do, which is what makes
//! the differ emit the `UpdateParams` the typed props ride along with. An app
//! stages props by rebuilding
//! [`native_component`](crate::api::native_component); none of this is public.
//!
//! Like every other slot-keyed table here, it is bounded by an explicit reaper
//! (`forget`, from the mounting widget's teardown), never by disposal alone —
//! the leak shape `crate::runtime`'s `forget_pending_callback` guards against
//! applies verbatim: a culled slot's dispose resolves by view identity and
//! never sees this table.
//!
//! # A failed `update` is retried, up to a cap
//!
//! The retry trigger is a **generation bump**, not `instance.props` differing,
//! and that distinction is load-bearing. A failed `update` leaves `old` as the
//! runtime's diff baseline, so the change *would* be re-applied by the next
//! `UpdateParams` — but the differ only emits one when `params_json` changes,
//! and `publish` moves the generation only when the app's props change, so an
//! app republishing the same (already failed) props forever would emit none at
//! all and the view would stay stale, silently, for the process lifetime. The
//! staging table therefore carries the retry: a failed dispatch marks the slot
//! (`request_update_retry`), and the **next `publish` for that slot bumps the
//! generation even for identical props** — one wire change, one `UpdateParams`,
//! one retry. Three consequences:
//!
//! - **A retry needs a rebuild.** Nothing here schedules one — the mark is
//!   consumed by the next rebuild that publishes this slot, so on a screen that
//!   never rebuilds again the change stays unapplied, exactly as any other
//!   props change would.
//! - **Three consecutive failed dispatches spend the budget and the slot goes
//!   inert.** Failures one and two each re-mark the slot, so a transient
//!   refusal gets two more attempts; failure three reports once at `warn` and
//!   marks nothing further, so an unchanged rebuild is back to zero FFI
//!   crossings and zero log lines. Re-marking forever would instead cost a
//!   permanently failing slot one dispatch **plus** a `log::warn!` on *every*
//!   rebuild — per-frame main-thread JNI traffic and log volume, with one more
//!   warning per refused ctx call on top. Inert is the trade taken; retrying
//!   cleverly (backoff, a schedule of its own) is not a goal.
//! - **The cap counts *consecutive* failures and a success clears it**, so a
//!   flaky platform never accumulates its way to inert. Only this synthetic
//!   retry is capped: a genuine props change bumps the generation on its own
//!   and is always dispatched, capped or not — the app's intent, not ours.
//!
//! # Kind and type must agree, and a mismatch fails closed four different ways
//!
//! `kind` is passed twice — once to [`register_component`], once to the
//! mounting builder — and nothing mechanically ties the two, so each mismatch
//! is named with its *actual* error (only the first is `UnknownControl`):
//!
//! | case | what surfaces | logged | slot |
//! |---|---|---|---|
//! | kind never registered | `NativeWidgetError::UnknownControl(kind)` from the runtime's own dispatch, before any decode | yes, by the platform export | dead |
//! | registered to `C`, mounted with `C` | nothing — the ordinary path | — | live |
//! | registered to `C`, mounted with `D` | `NativeWidgetError::Params`, from `Bridge::<C>::decode_props` failing to downcast the staged props to `C::Props` | yes, by the platform export | dead |
//! | registered to `C`, mounted with `D` where `D::Props == C::Props` | `NativeWidgetError::Params`, one step later — the props downcast *succeeds* and `Bridge::<C>::create` fails to downcast the staged component to `C` | yes, by the platform export | dead |
//!
//! Registering two components under one kind reduces to the third row, since
//! first-wins refuses the second. Every case **fails closed** — no half-created
//! slot, no instance retained, no silent no-op — and both mismatch rows name the
//! wiring bug in their message rather than reading like an ordinary "nothing
//! staged here any more".
//!
//! # Dispatch-boundary exception guard (Android)
//!
//! `ComponentCtx::env` (Android-only) hands a component the live `jni::Env`,
//! and a component is free to leave a Java exception pending on it — undefined
//! behaviour for the *next* JNI call, not for the one that threw. So **all
//! four** dispatches through this module — `create`, `update`, `dispose` and
//! `on_event` — check and clear one on the way out through the crate's own
//! `run_jni` helper, whose report names the throwable's class and message. On
//! the three carrying a context it folds into the same first-wins error channel
//! as a latched error; `on_event` has none, so it logs at `warn`.
//!
//! **`on_event` is guarded through the VM, not through a context**, because it
//! is handed neither: `NativeWidget::on_event` takes only `(state, event)`. Its
//! `Env` comes from `JavaVM::with_top_local_frame` on the process VM
//! (`frust_plugin::android::vm`), which borrows the *existing* top JNI frame
//! rather than pushing one, so the clean path is a `GetEnv` plus an
//! `ExceptionCheck` and a report's locals die with `nativeOnEvent`'s own frame.
//! It is deliberately **not** `frust_plugin::android::with_jni_env`: jni 0.22's
//! scoped attach defaults to `AttachmentExceptionPolicy::PreReThrowPostCatch`,
//! which stashes an already-pending exception before running the closure and
//! re-throws it after, so a check inside would read *clean* every time and clear
//! nothing. (That policy is also why a component reaching JNI through
//! `with_jni_env` is caught on its own way out; this guard covers the paths that
//! do not — a raw `jni-sys` call, an attach configured with `Ignore`, a
//! hand-recovered `Env`.)
//!
//! `env` itself stays a **safe** fn: making it `unsafe` would tax the one
//! audience that can use this trait at all, for a hazard this guard contains.

// The publication half of this module (`publish`/`forget`/`component_params`)
// has its production caller in `crate::api::mount`'s generic builder, which
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
use crate::controls::DARK;
use crate::events::{
    EVENT_KIND_CLICK, EVENT_KIND_DRAG_END, EVENT_KIND_DRAG_START, EVENT_KIND_TOGGLED,
    EVENT_KIND_VALUE_CHANGED, EventPayload, pack_bool, pack_value_changed, unpack_bool,
    unpack_value_changed,
};
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
/// - `update` runs only when [`Props`](Self::Props) compare unequal (plus the
///   bounded retry window a failed one opens);
/// - `on_event` runs from a platform listener between frames, for every view
///   the component attached one to with [`ComponentCtx::attach_listener`]
///   ([`on_event`](Self::on_event));
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
/// gets per slot today.
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
    /// the main thread, ordered with this slot's create/dispose.
    ///
    /// A failure latched on `ctx` leaves the runtime's diff baseline at `old`
    /// **and marks the slot for retry**, so the next rebuild that publishes
    /// this slot re-applies the change — even if the app's props never differ
    /// again. The trigger is a props-generation bump, not the app's props
    /// moving; the module doc's *A failed `update` is retried, up to a cap* has
    /// the mechanism and its limits (a retry needs a rebuild; three consecutive
    /// failures spend the budget and the slot then stops asking, so a broken
    /// component degrades to inert rather than to per-frame FFI traffic).
    fn update(
        &self,
        ctx: &mut ComponentCtx<'_, '_, '_>,
        state: &mut Self::State,
        old: &Self::Props,
        new: &Self::Props,
    );

    /// A platform listener the component attached fired for this slot: act on
    /// it, and answer the event (if any) the app's
    /// [`NativeComponentView::on_event`](crate::api::NativeComponentView::on_event)
    /// hook should receive.
    ///
    /// # Which events arrive here
    ///
    /// Exactly the ones a listener this component attached reports: a view it
    /// built (its root as much as a child) wired with
    /// [`ComponentCtx::attach_listener`], for the [`ListenerKinds`] it asked
    /// for. The listener is the platform's one shared class, bound to this
    /// slot's own id by the context — the component never sees the id, so it
    /// cannot route anything but home — and its events reach this method
    /// through the runtime's slot-id routing, the path the six built-in
    /// controls' events take. An event whose family this slot never attached
    /// (a stray, or a hand-built Android listener carrying this slot's number)
    /// is dropped at the bridge and never reaches this method.
    ///
    /// # The pair, the state and the props
    ///
    /// `event` is the listener's raw wire ([`NativeEvent::kind`]/
    /// [`NativeEvent::detail`], with [`NativeEvent::checked`] and
    /// [`NativeEvent::value`] decoding the two payload-carrying kinds).
    /// `state` is the component's own, and `props` the last props a `create`
    /// or successful `update` applied — the typed baseline the runtime diffs
    /// against. The `&self` a dispatch runs against is the value the app
    /// published this rebuild — re-read from the staging table on every
    /// dispatch (the module doc's point 5) — so it can carry the closures such
    /// an event should reach.
    ///
    /// # The answer
    ///
    /// `Some(event)` forwards that event to the app's hook (the six builders'
    /// events-as-signals idiom: the hook typically writes a signal, which wakes
    /// exactly one frust frame); `None` swallows it. The answer need not be
    /// the event that arrived — a component may translate one kind into
    /// another — but only the kinds [`NativeEvent`] names are forwarded; any
    /// other kind is dropped (logged at `debug`). The default forwards every
    /// event unchanged, which is right for a component whose listeners exist
    /// to report straight to the app.
    ///
    /// It runs on the main thread inside the runtime's borrow, like every
    /// listener dispatch here: a setter provoking its own listener
    /// synchronously from inside this method is dropped, not re-entered.
    fn on_event(
        &self,
        state: &mut Self::State,
        props: &Self::Props,
        event: NativeEvent,
    ) -> Option<NativeEvent> {
        let _ = (state, props);
        Some(event)
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
    /// A [`ListenerHandle`] kept in `state` needs no call here: dropping it
    /// with the state is its release ([`ListenerHandle`]'s own doc).
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
/// keeps the diff baseline unchanged and marks the slot for retry (the module
/// doc's *A failed `update` is retried on the next rebuild*); on `dispose` it
/// is logged.
///
/// **The first error wins, and every later one is logged rather than
/// dropped.** Reporting keeps the failure that *started* the cascade, not its
/// last symptom — but a symptom is still evidence, so a latch that refuses a
/// later error says so in the log (`log::warn!`), naming both. That holds
/// across [`with_local_frame`](Self::with_local_frame) too, which is the one
/// place a second context exists to lose an error in: the latch travels into
/// the frame and back out, so `failed()` answers the same inside it as
/// outside, on every platform arm.
///
/// # The slot it speaks for
///
/// A context also knows **which slot** the call is for — privately. That is
/// what [`attach_listener`](Self::attach_listener) binds the platform's one
/// listener to, and it is never handed to the component: a component can
/// only ever route an event back to its own slot (the module doc's *Listener
/// attachment*).
pub struct ComponentCtx<'ctx, 'local, 'env> {
    inner: &'ctx mut PlatformCtx<'local, 'env>,
    error: Option<NativeWidgetError>,
    /// The slot this call is for — the id every listener this context attaches
    /// reports under. Private on purpose (the type doc's *The slot it speaks
    /// for*).
    slot: SlotId,
    /// Every [`ListenerKinds`] family an [`attach_listener`](Self::attach_listener)
    /// made through this context succeeded for — read back by `Bridge` so the
    /// slot's event gate knows what it may deliver.
    attached: ListenerKinds,
}

impl<'ctx, 'local, 'env> ComponentCtx<'ctx, 'local, 'env> {
    /// Wrap the platform context of one runtime call for `slot`.
    fn new(inner: &'ctx mut PlatformCtx<'local, 'env>, slot: SlotId) -> Self {
        Self {
            inner,
            error: None,
            slot,
            attached: ListenerKinds::NONE,
        }
    }

    /// Consume the wrapper, reporting whatever was latched and every listener
    /// family attached through it.
    fn into_parts(self) -> (Option<NativeWidgetError>, ListenerKinds) {
        (self.error, self.attached)
    }

    /// Record a successful attach: the families join what `Bridge` will let
    /// through for this slot, and the attach is logged at `debug` — the one
    /// line a device or desktop run can grep to see a component's listener
    /// wiring happen (the slot id appears in the log, never in the component).
    fn note_attached(&mut self, kinds: ListenerKinds) {
        self.attached |= kinds;
        log::debug!(
            "frust-native-widgets: component slot {} attached a {kinds} listener",
            self.slot
        );
    }

    /// Refuse an [`attach_listener`](Self::attach_listener) asking for no
    /// family at all — shared by every arm, so an empty request latches the
    /// same error everywhere rather than attaching a listener that could never
    /// report anything.
    fn refuse_empty(&mut self, kinds: ListenerKinds) -> bool {
        if kinds.is_empty() {
            self.latch(NativeWidgetError::Params(
                "attach_listener was asked for no ListenerKinds — nothing to attach".into(),
            ));
            return true;
        }
        false
    }

    /// Record the first failure and keep it (see this type's *error latch*).
    ///
    /// A later error cannot replace it — that is the whole point of the latch
    /// — but it is **logged rather than swallowed**: on a device the cascade's
    /// symptoms are what let a reader judge the root failure's blast radius,
    /// and a second platform failure that vanished without trace is exactly
    /// the defect this guards against.
    fn latch(&mut self, error: NativeWidgetError) {
        match &self.error {
            Some(first) => log::warn!(
                "frust-native-widgets: component context already failed ({first}) — keeping that \
                 error and reporting this later one here only: {error}"
            ),
            None => self.error = Some(error),
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

    /// Attach this crate's one listener class, `FrustNativeListener`, to
    /// `view` for `kinds` — the module doc's *Listener attachment*. `view` may
    /// be the component's root or any child it built.
    ///
    /// [`ListenerKinds::CLICK`] sets it as the view's `View.OnClickListener`
    /// (any view); [`ListenerKinds::TOGGLED`] as a `CompoundButton`'s
    /// `OnCheckedChangeListener`; [`ListenerKinds::VALUE_CHANGED`] as a
    /// `SeekBar`'s `OnSeekBarChangeListener`, which also reports the drag
    /// edges ([`NativeEvent::KIND_DRAG_START`]/[`NativeEvent::KIND_DRAG_END`]).
    /// The listener is constructed bound to this slot's id — which this
    /// context never hands you — so its events come home to
    /// [`NativeComponent::on_event`] and nowhere else.
    ///
    /// Keep the returned [`ListenerHandle`] in your
    /// [`NativeComponent::State`]: it holds one global reference to `view`,
    /// released when the state drops (the view itself holds the listener).
    ///
    /// Latches and answers `None` when `kinds` is empty, the listener class
    /// cannot be loaded, or a setter throws — typically
    /// [`ListenerKinds::TOGGLED`]/[`ListenerKinds::VALUE_CHANGED`] asked of a
    /// view that is no `CompoundButton`/`SeekBar` (`NoSuchMethodError`). A
    /// failure part-way can leave an interface already set, but the slot
    /// routes only the families of an attach that completed, so a stray event
    /// from it is dropped before [`NativeComponent::on_event`].
    pub fn attach_listener(
        &mut self,
        view: &jni::objects::JObject<'_>,
        kinds: ListenerKinds,
    ) -> Option<ListenerHandle> {
        if self.refuse_empty(kinds) {
            return None;
        }
        match self.inner.attach_listener(
            view,
            self.slot,
            kinds.contains(ListenerKinds::CLICK),
            kinds.contains(ListenerKinds::TOGGLED),
            kinds.contains(ListenerKinds::VALUE_CHANGED),
        ) {
            Ok(view) => {
                self.note_attached(kinds);
                Some(ListenerHandle {
                    kinds,
                    inner: ListenerInner { view },
                })
            }
            Err(error) => {
                self.latch(error);
                None
            }
        }
    }

    /// Detach what `handle` attached — `setOn…Listener(null)` for each
    /// interface it set — and release it: the explicit form of dropping the
    /// handle, for a component that stops listening while its view lives on
    /// (or that detaches in `dispose`, as the six controls do). Latches and
    /// answers `None` when a setter throws; the handle is released either way.
    pub fn detach_listener(&mut self, handle: ListenerHandle) -> Option<()> {
        let ListenerHandle { kinds, inner } = handle;
        match self.inner.detach_listener(
            &inner.view,
            kinds.contains(ListenerKinds::CLICK),
            kinds.contains(ListenerKinds::TOGGLED),
            kinds.contains(ListenerKinds::VALUE_CHANGED),
        ) {
            Ok(()) => Some(()),
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
    /// for the whole `create` call. A value the closure returns must not *be* a
    /// local reference — that is what [`Self::retain_child`] is for, and its
    /// [`NativeChild`] outlives the frame.
    ///
    /// **The latch travels with you.** The closure runs against a context that
    /// already carries whatever this one latched — so `failed()` answers the
    /// same inside the frame as outside it, exactly as on the iOS and host
    /// arms, which hand the closure this very context — and whatever survives
    /// comes back out. A frame that cannot be pushed at all latches too, and
    /// leaves an already-latched error untouched; either way the answer is
    /// `None`. Per the latch's first-wins rule, an error raised inside the
    /// frame behind an already-latched one is logged rather than reported.
    pub fn with_local_frame<T>(
        &mut self,
        capacity: usize,
        f: impl FnOnce(&mut ComponentCtx<'_, '_, '_>) -> Option<T>,
    ) -> Option<T> {
        // A *fresh* inner context is structurally forced here — the pushed
        // frame's references carry different lifetimes than this context's —
        // but a fresh *latch* must not be: that would make `failed()` read
        // `false` inside a frame where the other two arms read `true`, and
        // re-latching the inner error on return would silently drop it whenever
        // this context already held one.
        //
        // The slot and the attached-listener record travel the same way: a
        // listener attached inside the frame is this slot's like any other.
        let mut latched = self.error.take();
        let slot = self.slot;
        let mut attached = self.attached;
        let outcome = self.inner.with_frame(capacity, |inner| {
            let mut cx = ComponentCtx {
                inner,
                error: latched.take(),
                slot,
                attached,
            };
            let value = f(&mut cx);
            (latched, attached) = cx.into_parts();
            Ok::<Option<T>, NativeWidgetError>(value)
        });
        // A frame that could not be pushed never ran the closure, so this puts
        // the carried-in error back rather than erasing it.
        self.error = latched;
        self.attached = attached;
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
    /// so a component using this directly should check and clear its own —
    /// [`Self::new_view`] and [`Self::root`] do that for the calls they make.
    /// **The runtime does not take that on trust:** every dispatch through this
    /// module — the three that carry this context (`create`, `update`,
    /// `dispose`) and `on_event`, which reaches the VM instead — checks and
    /// clears a leftover exception on the way out and reports it (the module
    /// doc's *Dispatch-boundary exception guard*). Clearing your own is still
    /// the right discipline — it keeps the *rest of your own call* on defined
    /// ground, which the boundary guard cannot do for you — but forgetting it
    /// cannot poison the next unrelated JNI call.
    ///
    /// This stays a **safe** fn on purpose: the hazard is bounded by the guard
    /// above, and an `unsafe` escape hatch would tax the small audience that
    /// can implement this trait at all for no further protection.
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

    /// Attach this crate's one target-action class, `FrustNativeControlTarget`,
    /// to `view` for `kinds` — the module doc's *Listener attachment*. `view`
    /// may be the component's root or any child it built.
    ///
    /// [`ListenerKinds::CLICK`] wires `TouchUpInside` on any `UIControl`;
    /// [`ListenerKinds::TOGGLED`] a `UISwitch`'s `ValueChanged`;
    /// [`ListenerKinds::VALUE_CHANGED`] a `UISlider`'s `ValueChanged` plus its
    /// `TouchDown`/`TouchUpInside|TouchUpOutside` drag edges — exactly the
    /// wiring the six controls use. The target is bound to this slot's id,
    /// which this context never hands you.
    ///
    /// UIKit holds a control's targets **weakly**, so the returned
    /// [`ListenerHandle`] is the target's only strong reference: keep it in
    /// your [`NativeComponent::State`]. Dropping it removes the target-action
    /// pairs it added, then releases the target.
    ///
    /// Latches and answers `None` when `kinds` is empty or `view` is not the
    /// class a requested kind reads its payload from (no `UIControl` at all,
    /// or `TOGGLED`/`VALUE_CHANGED` on a view that is no
    /// `UISwitch`/`UISlider`) — checked before anything is attached, so a
    /// refusal leaves nothing half-wired.
    pub fn attach_listener(
        &mut self,
        view: &objc2_ui_kit::UIView,
        kinds: ListenerKinds,
    ) -> Option<ListenerHandle> {
        if self.refuse_empty(kinds) {
            return None;
        }
        let mtm = self.inner.mtm();
        match crate::apple::FrustNativeControlTarget::attach_component(
            mtm,
            self.slot,
            view,
            kinds.contains(ListenerKinds::CLICK),
            kinds.contains(ListenerKinds::TOGGLED),
            kinds.contains(ListenerKinds::VALUE_CHANGED),
        ) {
            Ok((target, control)) => {
                self.note_attached(kinds);
                Some(ListenerHandle {
                    kinds,
                    inner: ListenerInner { target, control },
                })
            }
            Err(message) => {
                self.latch(NativeWidgetError::Platform(message));
                None
            }
        }
    }

    /// Detach what `handle` attached and release it — the explicit spelling
    /// of dropping the handle, whose `Drop` already removes its target-action
    /// pairs on this arm. Never latches.
    pub fn detach_listener(&mut self, handle: ListenerHandle) -> Option<()> {
        drop(handle);
        Some(())
    }

    /// The Apple counterpart of Android's local-frame wrapper: it runs `f`
    /// and nothing else.
    ///
    /// There is no reference table to bound here — a `Retained`'s own `Drop`
    /// is the release (`crate::apple::ctx`'s module doc) — so `capacity` is
    /// accepted and ignored. The method exists on this arm so a component's
    /// `create` is written once and compiles on both.
    ///
    /// Handing the closure this very context is also what makes the error
    /// latch shared, which the Android arm now matches deliberately rather
    /// than by accident: `failed()` reads the same inside the frame as outside
    /// it on every arm.
    pub fn with_local_frame<T>(
        &mut self,
        _capacity: usize,
        f: impl FnOnce(&mut ComponentCtx<'_, '_, '_>) -> Option<T>,
    ) -> Option<T> {
        f(self)
    }
}

/// The macOS arm: the `NSView` twin of the iOS block above, method for method
/// — the main-thread proof (AppKit's whole escape hatch, exactly as on iOS),
/// `root`/`add_child`/`retain_child` over `Retained<NSView>`, and the same
/// run-`f`-and-nothing-else local frame.
///
/// `add_child` goes through `crate::appkit::NativeCtx::add_child`
/// (`addSubview:`), the seam `crate::appkit::ctx`'s *Hierarchy* section keeps
/// for exactly this caller. There is no host-style `record` here: this arm
/// builds real views, and the `demo-components` `DemoCard` has its own AppKit
/// `mod platform` (`crate::demo`).
///
/// [`attach_listener`](Self::attach_listener) wires `crate::appkit::events`'
/// one target class onto an `NSControl` the component built, the AppKit
/// counterpart of the other two arms' listener attach.
#[cfg(target_os = "macos")]
impl ComponentCtx<'_, '_, '_> {
    /// The main-thread proof this call carries — what every `objc2-app-kit`
    /// constructor demands, and the macOS arm's whole escape hatch (the
    /// Objective-C runtime is globally reachable; there is no `Env` to thread).
    pub fn mtm(&self) -> objc2::MainThreadMarker {
        self.inner.mtm()
    }

    /// Take this slot's root view. ARC owns it from here — `Retained`'s own
    /// `Drop` is the release, so there is no paired-delete discipline on this
    /// arm.
    ///
    /// A typed view converts with objc2's own upcast, e.g.
    /// `Retained::clone(&button).into_super().into_super()` for an `NSButton`
    /// (`NSButton` → `NSControl` → `NSView`). Answers `Option` only so a
    /// component reads the same on every platform (`let root =
    /// ctx.root(view)?;`); this arm never latches here.
    pub fn root(&mut self, view: objc2::rc::Retained<objc2_app_kit::NSView>) -> Option<NativeRoot> {
        let mtm = self.inner.mtm();
        Some(NativeRoot(NativeView::new(view, mtm)))
    }

    /// `parent.addSubview(child)` — attach one native child, the subtree call
    /// (module doc's *A component owns its own native subtree*).
    ///
    /// AppKit retains a subview, so a child you never touch again needs no
    /// handle of yours at all — it is released when the parent is. Answers
    /// `Option` only so a component reads the same on every platform; this
    /// arm never latches here.
    ///
    /// A typed view passes with objc2's own upcast, e.g. `&label` for a
    /// `Retained<NSTextField>` derefs through `NSView`'s superclass chain.
    pub fn add_child(
        &mut self,
        parent: &objc2_app_kit::NSView,
        child: &objc2_app_kit::NSView,
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
    /// This exists so a component that wants one `State` shape on every arm
    /// can name [`NativeChild`] on this one too. Never latches.
    pub fn retain_child(&mut self, view: &objc2_app_kit::NSView) -> Option<NativeChild> {
        use objc2::Message as _;
        Some(NativeChild(view.retain()))
    }

    /// Attach this crate's one target-action class, `FrustNativeControlTarget`,
    /// to `view` for `kinds` — the module doc's *Listener attachment*. `view`
    /// may be the component's root or any child it built, and must be an
    /// `NSControl`.
    ///
    /// **Exactly one family per control on this arm**: an `NSControl` carries
    /// a single `target`/`action` pair, sent at its one "value committed"
    /// moment, so [`ListenerKinds::CLICK`] (any control — a button click),
    /// [`ListenerKinds::TOGGLED`] (an `NSSwitch`, whose `state` is the payload)
    /// and [`ListenerKinds::VALUE_CHANGED`] (any control's `doubleValue` — set
    /// an `NSSlider` `continuous` to hear every drag step) are alternatives
    /// here, not a mask. AppKit reports no drag edges (`crate::appkit::events`'
    /// module doc). The target is bound to this slot's id, which this context
    /// never hands you.
    ///
    /// `NSControl.target` is weak, so the returned [`ListenerHandle`] is the
    /// target's only strong reference: keep it in your
    /// [`NativeComponent::State`]. Dropping it clears the control's
    /// target/action (if they are still this handle's), then releases the
    /// target.
    ///
    /// Latches and answers `None` when `kinds` is empty or names more than one
    /// family, `view` is no `NSControl`, or `TOGGLED` is asked of a control
    /// that is no `NSSwitch` — checked before anything is attached.
    pub fn attach_listener(
        &mut self,
        view: &objc2_app_kit::NSView,
        kinds: ListenerKinds,
    ) -> Option<ListenerHandle> {
        if self.refuse_empty(kinds) {
            return None;
        }
        let Some(kind) = kinds.single_event_kind() else {
            self.latch(NativeWidgetError::Params(format!(
                "macOS attach_listener: an NSControl carries one target/action pair, so attach \
                 exactly one ListenerKinds family per control (asked for {kinds})"
            )));
            return None;
        };
        let mtm = self.inner.mtm();
        match crate::appkit::FrustNativeControlTarget::attach_view(mtm, view, self.slot, kind) {
            Ok((target, control)) => {
                self.note_attached(kinds);
                Some(ListenerHandle {
                    kinds,
                    inner: ListenerInner { target, control },
                })
            }
            Err(message) => {
                self.latch(NativeWidgetError::Platform(message));
                None
            }
        }
    }

    /// Detach what `handle` attached and release it — the explicit spelling
    /// of dropping the handle, whose `Drop` already clears the control's
    /// target/action on this arm. Never latches.
    pub fn detach_listener(&mut self, handle: ListenerHandle) -> Option<()> {
        drop(handle);
        Some(())
    }

    /// The Apple counterpart of Android's local-frame wrapper: it runs `f`
    /// and nothing else.
    ///
    /// There is no reference table to bound here — a `Retained`'s own `Drop`
    /// is the release (`crate::appkit::ctx`'s module doc) — so `capacity` is
    /// accepted and ignored. Handing the closure this very context is what
    /// makes the error latch shared, exactly as on the iOS arm: `failed()`
    /// reads the same inside the frame as outside it on every arm.
    pub fn with_local_frame<T>(
        &mut self,
        _capacity: usize,
        f: impl FnOnce(&mut ComponentCtx<'_, '_, '_>) -> Option<T>,
    ) -> Option<T> {
        f(self)
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
impl ComponentCtx<'_, '_, '_> {
    /// Record one would-be platform call — the host stand-in's whole surface,
    /// so a component's create/update/dispose can be asserted by an ordinary
    /// `cargo test` on a machine with no JNI and no Objective-C runtime at
    /// all. Compiled only on a host with no platform arm; there is no native
    /// view to build there.
    pub fn record(&mut self, call: impl Into<String>) {
        self.inner.record(call);
    }

    /// Take a stand-in root view with the given identity — the host mirror of
    /// the three platform arms' `root`, where `identity` plays the role
    /// `Env::is_same_object` plays on Android (what a dispose resolves
    /// against).
    pub fn root(&mut self, identity: u64) -> Option<NativeRoot> {
        Some(NativeRoot(NativeView { identity }))
    }

    /// Record one would-be `addView`/`addSubview` — the host mirror of the
    /// three platform arms' `add_child`, so a subtree's *plan* is assertable
    /// (which child went under which parent, in what order) on a machine with
    /// no view hierarchy at all.
    pub fn add_child(&mut self, parent: u64, child: u64) -> Option<()> {
        self.inner.record(format!("addChild {parent} <- {child}"));
        Some(())
    }

    /// Take a stand-in retained child handle — the host mirror of Android's
    /// global reference and the Apple arms' `Retained`.
    ///
    /// Live handles are counted process-thread-wide (this module's
    /// crate-private `live_child_count`), so a host test asserts the paired
    /// release the way ART's global-ref count does on device (module doc's
    /// *Teardown*) rather than merely asserting that nothing panicked.
    pub fn retain_child(&mut self, identity: u64) -> Option<NativeChild> {
        self.inner.record(format!("retainChild {identity}"));
        Some(NativeChild::new(identity))
    }

    /// Record a would-be listener attach on the stand-in view `view` — the
    /// host mirror of the three platform arms' `attach_listener`, so a
    /// component's listener wiring is part of its assertable plan.
    ///
    /// Like the real arms it binds the stand-in to this context's own slot
    /// (logged, never recorded in the plan, never handed out), and it latches
    /// and answers `None`
    /// for an empty `kinds`. Live stand-in handles are counted
    /// (`live_listener_count`) the way [`Self::retain_child`]'s are.
    pub fn attach_listener(&mut self, view: u64, kinds: ListenerKinds) -> Option<ListenerHandle> {
        if self.refuse_empty(kinds) {
            return None;
        }
        self.inner.record(format!("attachListener {view} {kinds}"));
        self.note_attached(kinds);
        Some(ListenerHandle {
            kinds,
            inner: HostListener::new(view),
        })
    }

    /// Record a would-be detach of what `handle` attached, then release it —
    /// the host mirror of the three platform arms' `detach_listener`.
    pub fn detach_listener(&mut self, handle: ListenerHandle) -> Option<()> {
        self.inner.record(format!(
            "detachListener {} {}",
            handle.inner.identity, handle.kinds
        ));
        Some(())
    }

    /// Record a would-be `PushLocalFrame`/`PopLocalFrame` pair around `f` —
    /// the host mirror of Android's real local frame, so a test can assert a
    /// subtree build actually ran inside one.
    ///
    /// Like the iOS arm, this hands the closure the caller's own context, so
    /// the error latch is shared: `failed()` reads the same inside the frame
    /// as outside it, and a second error raised inside it is logged by the
    /// latch rather than dropped. Android reproduces both properties over a
    /// context it is forced to build fresh.
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
/// the paired global-ref delete; iOS and macOS: ARC).
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
/// release** — `DeleteGlobalRef` on Android, `Retained`'s own `Drop` on iOS and
/// macOS —
/// and `State` is dropped immediately after [`NativeComponent::dispose`]
/// returns, so a child is released with its parent by construction rather
/// than by remembering to.
///
/// A child you never talk to again needs none of this: the platform parent
/// already owns it.
pub struct NativeChild(ChildHandle);

/// [`NativeChild`]'s per-platform payload: a global reference on Android, an
/// ARC retain on iOS (`UIView`) and macOS (`NSView`), a counted stand-in on a
/// host with no platform arm (Linux/Windows/web).
#[cfg(target_os = "android")]
type ChildHandle = jni::refs::Global<jni::objects::JObject<'static>>;
#[cfg(target_os = "ios")]
type ChildHandle = objc2::rc::Retained<objc2_ui_kit::UIView>;
#[cfg(target_os = "macos")]
type ChildHandle = objc2::rc::Retained<objc2_app_kit::NSView>;
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
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

#[cfg(target_os = "macos")]
impl NativeChild {
    /// The retained child, for the `objc2-app-kit` setters a component's
    /// `update` calls on it. Takes the same main-thread proof
    /// [`AppKitHandle::view`](crate::registry::appkit::AppKitHandle::view)
    /// does — a typestate guard, not a runtime cost.
    pub fn view(&self, _mtm: objc2::MainThreadMarker) -> &objc2_app_kit::NSView {
        &self.0
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
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
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
pub(crate) struct HostChild {
    identity: u64,
}

#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
impl Drop for HostChild {
    fn drop(&mut self) {
        LIVE_CHILDREN.with(|live| live.set(live.get().saturating_sub(1)));
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
thread_local! {
    /// How many host stand-in child handles are alive on this thread — the
    /// mirror of ART's live-global-ref count ("52 at peak → 0 after the
    /// dispose cycle" on device). Thread-local for the same reason
    /// [`STAGED`] is, which also keeps each `cargo test` thread's count its
    /// own.
    static LIVE_CHILDREN: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many retained subtree children are currently alive — the host arm's
/// leak bar, which every create/dispose cycle must return to `0`.
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
#[allow(dead_code)] // the leak bar's only caller is this module's own tests
pub(crate) fn live_child_count() -> usize {
    LIVE_CHILDREN.with(|live| live.get())
}

// --- listener attachment -----------------------------------------------------

/// Which of the platform listener's event families
/// [`ComponentCtx::attach_listener`] wires onto a view — the module doc's
/// *Listener attachment*. Combine with `|`.
///
/// | family | Android (`FrustNativeListener` as…) | iOS (`FrustNativeControlTarget` on…) | macOS (`FrustNativeControlTarget` on…) | [`NativeEvent`] kinds it delivers |
/// |---|---|---|---|---|
/// | [`CLICK`](Self::CLICK) | `View.OnClickListener`, any view | `TouchUpInside`, any `UIControl` | the action, any `NSControl` | [`NativeEvent::KIND_CLICK`] |
/// | [`TOGGLED`](Self::TOGGLED) | `OnCheckedChangeListener`, a `CompoundButton` | `ValueChanged`, a `UISwitch` | the action, an `NSSwitch` | [`NativeEvent::KIND_TOGGLED`] |
/// | [`VALUE_CHANGED`](Self::VALUE_CHANGED) | `OnSeekBarChangeListener`, a `SeekBar` | `ValueChanged` + drag edges, a `UISlider` | the action, any `NSControl` (`doubleValue`) | [`NativeEvent::KIND_VALUE_CHANGED`], plus the drag edges on the two mobile arms |
///
/// macOS takes exactly one family per control (one target/action pair per
/// `NSControl` — [`ComponentCtx::attach_listener`]'s macOS doc). Later
/// families (a selection, a date) land beside these three and route through
/// the same table.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ListenerKinds(u8);

impl ListenerKinds {
    /// No family at all — what [`ComponentCtx::attach_listener`] refuses.
    pub const NONE: Self = Self(0);
    /// A click / tap.
    pub const CLICK: Self = Self(1);
    /// A two-state control flipped.
    pub const TOGGLED: Self = Self(1 << 1);
    /// A ranged control moved (and, on Android and iOS, its drag began/ended).
    pub const VALUE_CHANGED: Self = Self(1 << 2);

    /// Whether every family in `other` is in `self` (so `contains(NONE)` is
    /// always `true`).
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether no family is set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Every family in either.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// The single `crate::events` kind code a one-family mask means on the
    /// macOS arm (one target/action pair per `NSControl`), or `None` for an
    /// empty or multi-family mask.
    #[cfg(target_os = "macos")]
    fn single_event_kind(self) -> Option<i32> {
        match self {
            Self::CLICK => Some(EVENT_KIND_CLICK),
            Self::TOGGLED => Some(EVENT_KIND_TOGGLED),
            Self::VALUE_CHANGED => Some(EVENT_KIND_VALUE_CHANGED),
            _ => None,
        }
    }
}

impl std::ops::BitOr for ListenerKinds {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        self.union(other)
    }
}

impl std::ops::BitOrAssign for ListenerKinds {
    fn bitor_assign(&mut self, other: Self) {
        *self = self.union(other);
    }
}

/// `click|toggled|value_changed`, or `none` — the spelling log lines and the
/// host arm's recorded plan use.
impl std::fmt::Display for ListenerKinds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_empty() {
            return f.write_str("none");
        }
        let names = [
            (Self::CLICK, "click"),
            (Self::TOGGLED, "toggled"),
            (Self::VALUE_CHANGED, "value_changed"),
        ];
        let mut first = true;
        for (family, name) in names {
            if self.contains(family) {
                if !first {
                    f.write_str("|")?;
                }
                f.write_str(name)?;
                first = false;
            }
        }
        Ok(())
    }
}

impl std::fmt::Debug for ListenerKinds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ListenerKinds({self})")
    }
}

/// One platform listener a component attached through
/// [`ComponentCtx::attach_listener`] — keep it in your
/// [`NativeComponent::State`].
///
/// **Dropping it is the release**, and `State` is dropped immediately after
/// [`NativeComponent::dispose`] returns, so a listener is released with the
/// view it listens to by construction: on Android the handle's global
/// reference to the view is deleted (the view itself holds the listener and
/// dies with the subtree); on iOS and macOS the target-action pairs it added
/// are removed from the control first — its target is held weakly by UIKit and
/// AppKit, so this handle is the target's only strong reference — and then the
/// target is released. [`ComponentCtx::detach_listener`] is the explicit form.
#[must_use = "a dropped ListenerHandle releases its listener at once — keep it in State"]
pub struct ListenerHandle {
    kinds: ListenerKinds,
    inner: ListenerInner,
}

impl ListenerHandle {
    /// The families this handle's listener was attached for.
    pub fn kinds(&self) -> ListenerKinds {
        self.kinds
    }
}

/// [`ListenerHandle`]'s per-platform payload.
#[cfg(target_os = "android")]
struct ListenerInner {
    /// A global reference to the view the listener was set on — what
    /// [`ComponentCtx::detach_listener`] clears the listener off.
    view: jni::refs::Global<jni::objects::JObject<'static>>,
}

/// [`ListenerHandle`]'s per-platform payload.
#[cfg(target_os = "ios")]
struct ListenerInner {
    /// The target, whose only strong reference this is.
    target: objc2::rc::Retained<crate::apple::FrustNativeControlTarget>,
    /// The control it is attached to — kept so the handle's `Drop` can remove
    /// exactly the target-action pairs it added.
    control: objc2::rc::Retained<objc2_ui_kit::UIControl>,
}

/// [`ListenerHandle`]'s per-platform payload.
#[cfg(target_os = "macos")]
struct ListenerInner {
    /// The target, whose only strong reference this is.
    target: objc2::rc::Retained<crate::appkit::FrustNativeControlTarget>,
    /// The control it is attached to — kept so the handle's `Drop` can clear
    /// the control's target/action while they are still this target's.
    control: objc2::rc::Retained<objc2_app_kit::NSControl>,
}

/// [`ListenerHandle`]'s per-platform payload: a counted stand-in on a host
/// with no platform arm.
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
type ListenerInner = HostListener;

/// The two Apple arms detach on drop: the platform holds the target weakly,
/// so releasing it while still attached would leave a control whose action
/// has nowhere to go. Runs on the main thread by construction — a handle
/// lives in a component's `State`, which the runtime drops there.
#[cfg(target_os = "ios")]
impl Drop for ListenerHandle {
    fn drop(&mut self) {
        self.inner.target.detach_component(
            &self.inner.control,
            self.kinds.contains(ListenerKinds::CLICK),
            self.kinds.contains(ListenerKinds::TOGGLED),
            self.kinds.contains(ListenerKinds::VALUE_CHANGED),
        );
    }
}

/// See the iOS arm's `Drop`.
#[cfg(target_os = "macos")]
impl Drop for ListenerHandle {
    fn drop(&mut self) {
        self.inner.target.detach_if_current(&self.inner.control);
    }
}

/// The host arm's stand-in listener: it decrements [`LIVE_LISTENERS`] when
/// dropped, the leak bar [`HostChild`] keeps for retained children.
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
pub(crate) struct HostListener {
    identity: u64,
}

#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
impl HostListener {
    /// A counted stand-in attached to the view with the given identity.
    fn new(identity: u64) -> Self {
        LIVE_LISTENERS.with(|live| live.set(live.get() + 1));
        Self { identity }
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
impl Drop for HostListener {
    fn drop(&mut self) {
        LIVE_LISTENERS.with(|live| live.set(live.get().saturating_sub(1)));
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
thread_local! {
    /// How many host stand-in listener handles are alive on this thread — the
    /// [`LIVE_CHILDREN`] leak bar's twin for [`ListenerHandle`].
    static LIVE_LISTENERS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many attached listener handles are currently alive — every
/// create/dispose cycle must return it to `0`.
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
#[allow(dead_code)] // the leak bar's only callers are this crate's own tests
pub(crate) fn live_listener_count() -> usize {
    LIVE_LISTENERS.with(|live| live.get())
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
    /// A click / tap ([`ListenerKinds::CLICK`]); `detail` unused (`0`).
    pub const KIND_CLICK: i32 = EVENT_KIND_CLICK;
    /// A two-state control flipped ([`ListenerKinds::TOGGLED`]); the new
    /// state is [`Self::checked`].
    pub const KIND_TOGGLED: i32 = EVENT_KIND_TOGGLED;
    /// A ranged control moved ([`ListenerKinds::VALUE_CHANGED`]); the new
    /// position is [`Self::value`].
    pub const KIND_VALUE_CHANGED: i32 = EVENT_KIND_VALUE_CHANGED;
    /// A ranged control's drag began (Android and iOS only); `detail` unused.
    pub const KIND_DRAG_START: i32 = EVENT_KIND_DRAG_START;
    /// A ranged control's drag ended (Android and iOS only); `detail` unused.
    pub const KIND_DRAG_END: i32 = EVENT_KIND_DRAG_END;

    /// Which listener fired — one of the `KIND_*` codes above, which are
    /// `crate::events`' wire codes, shared verbatim with Android's listener.
    pub fn kind(self) -> i32 {
        self.kind
    }

    /// The listener's primitive payload; `0` for a kind that carries none.
    pub fn detail(self) -> i64 {
        self.detail
    }

    /// Whether this is a [`Self::KIND_CLICK`].
    pub fn is_click(self) -> bool {
        self.kind == EVENT_KIND_CLICK
    }

    /// The reported checked state of a [`Self::KIND_TOGGLED`]; `None` for
    /// any other kind.
    pub fn checked(self) -> Option<bool> {
        (self.kind == EVENT_KIND_TOGGLED).then(|| unpack_bool(self.detail))
    }

    /// The reported position of a [`Self::KIND_VALUE_CHANGED`], in the
    /// **platform's own** space (a `SeekBar`'s zero-based progress, a
    /// `UISlider`'s/`NSSlider`'s value rounded to the nearest integer) —
    /// mapping it into an app range is the component's job, since only it
    /// configured the control's range. `None` for any other kind.
    pub fn value(self) -> Option<i32> {
        (self.kind == EVENT_KIND_VALUE_CHANGED).then(|| unpack_value_changed(self.detail).0)
    }

    /// The [`ListenerKinds`] family whose attach delivers this kind — what the
    /// bridge checks a slot attached before delivering (the module doc's
    /// *Listener attachment*); [`ListenerKinds::NONE`] for a kind no family
    /// delivers.
    pub fn family(self) -> ListenerKinds {
        match self.kind {
            EVENT_KIND_CLICK => ListenerKinds::CLICK,
            EVENT_KIND_TOGGLED => ListenerKinds::TOGGLED,
            EVENT_KIND_VALUE_CHANGED | EVENT_KIND_DRAG_START | EVENT_KIND_DRAG_END => {
                ListenerKinds::VALUE_CHANGED
            }
            _ => ListenerKinds::NONE,
        }
    }

    /// Adapt the runtime's internal wire event.
    fn from_wire(event: WireEvent) -> Self {
        Self {
            kind: event.kind,
            detail: event.detail,
        }
    }

    /// This event in the six controls' typed [`EventPayload`] vocabulary —
    /// the table a component's answer rides to the app's hook
    /// (`crate::api::mount`). `None` for a kind that vocabulary has no word
    /// for, which the bridge then drops.
    ///
    /// Exactly inverted by [`Self::from_payload`] for every kind it maps: the
    /// same `crate::events` codec packs and unpacks both directions.
    pub(crate) fn into_payload(self) -> Option<EventPayload> {
        match self.kind {
            EVENT_KIND_CLICK => Some(EventPayload::Click),
            EVENT_KIND_TOGGLED => Some(EventPayload::Toggled(unpack_bool(self.detail))),
            EVENT_KIND_VALUE_CHANGED => {
                let (value, from_user) = unpack_value_changed(self.detail);
                Some(EventPayload::ValueChanged { value, from_user })
            }
            EVENT_KIND_DRAG_START => Some(EventPayload::DragStart),
            EVENT_KIND_DRAG_END => Some(EventPayload::DragEnd),
            _ => None,
        }
    }

    /// [`Self::into_payload`]'s inverse — how `crate::api::mount` hands the
    /// app's `.on_event` hook the public pair back.
    pub(crate) fn from_payload(payload: EventPayload) -> Self {
        let (kind, detail) = match payload {
            EventPayload::Click => (EVENT_KIND_CLICK, 0),
            EventPayload::Toggled(checked) => (EVENT_KIND_TOGGLED, pack_bool(checked)),
            EventPayload::ValueChanged { value, from_user } => (
                EVENT_KIND_VALUE_CHANGED,
                pack_value_changed(value, from_user),
            ),
            EventPayload::DragStart => (EVENT_KIND_DRAG_START, 0),
            EventPayload::DragEnd => (EVENT_KIND_DRAG_END, 0),
            EventPayload::Selected(index) => (
                crate::events::EVENT_KIND_SELECTION,
                crate::events::pack_index(index as isize),
            ),
            EventPayload::Date(date) => (
                crate::events::EVENT_KIND_DATE,
                crate::events::pack_date(date),
            ),
        };
        Self { kind, detail }
    }
}

// --- registration ------------------------------------------------------------

/// Register `C` under `kind`, so a slot whose params name that kind is served
/// by this component — the module doc's decision **2**.
///
/// Call it once, from app or plugin init, **on the platform main thread**.
/// Returns whether the registration was accepted: **first-wins**, so a `kind`
/// already taken — including the six built-in control kinds this build's
/// backend registers itself — is refused with a warning rather than replaced.
///
/// # A wrong-thread call is refused, not silently accepted
///
/// The runtime is a `thread_local!`, and only the platform main thread's copy
/// is ever dispatched through: a registration made anywhere else lands in a
/// runtime nothing will ever consult, and the symptom arrives much later, as a
/// component that simply never appears. So this asks the platform first —
/// `Looper.myLooper() == Looper.getMainLooper()` on Android (through the
/// plugin substrate's scoped attach), `MainThreadMarker` on iOS — and a
/// definitive *no* is refused outright: nothing is registered, `false` comes
/// back, and the reason is logged at error level.
///
/// A platform that cannot answer proceeds as before. On Android that means
/// before the shell installs the plugin handles (`NotInitialized`) or if the
/// probe itself fails; on a desktop/CI host there is no platform main thread
/// to be wrong about at all, and nothing dispatches there in any case. Guessing
/// "wrong thread" from an unknown answer would turn a legitimate registration
/// into a silent no-show, which is the exact failure this is meant to prevent.
///
/// Registration is explicit on purpose: `inventory`-style link-time discovery
/// is banned in this crate, because a stripped, LTO'd device build is exactly
/// where it fails silently.
pub fn register_component<C: NativeComponent>(kind: &'static str) -> bool {
    if on_platform_main_thread() == Some(false) {
        log::error!(
            "frust-native-widgets: register_component('{kind}') was called off the platform main \
             thread — the runtime is thread-local, so this registration would land in a runtime \
             nothing ever dispatches through and the component would never appear. Refused; \
             register from app or plugin init on the main thread."
        );
        return false;
    }
    // Also the moment the iOS factory class must exist by, if an app registers
    // long before it mounts anything: `crate::runtime`'s own encode path forces
    // this too, and it is idempotent (a `Once`), so paying it here as well only
    // moves the cost earlier.
    crate::runtime::ensure_platform_factory();
    with_runtime(|runtime| runtime.register_if_free::<Bridge<C>>(kind)).unwrap_or(false)
}

/// Whether this is the platform main thread — the one thread whose
/// thread-local runtime the host actually dispatches through.
///
/// `None` is *unknown*, and every unknown answer means "proceed" at the one
/// call site ([`register_component`]): refusing a legitimate main-thread
/// registration would be far worse than missing a wrong-thread one.
///
/// `Looper.myLooper()` answers null on a thread with no looper and
/// `getMainLooper()` never does, so the pair is never both-null — the trap
/// `IsSameObject` sets for two nulls, which `frust-camera`'s own
/// `ensure_off_ui_thread` names in the same words. The scoped attach is
/// `frust_plugin`'s (`docs/CODE_STANDARDS.md`'s never-`attach_permanently`
/// rule), and any JNI failure — including the pre-init `NotInitialized` — maps
/// to *unknown* rather than to a refusal.
#[cfg(target_os = "android")]
fn on_platform_main_thread() -> Option<bool> {
    use jni::{Env, jni_sig, jni_str};

    fn on_main_looper(env: &mut Env<'_>) -> Result<bool, jni::errors::Error> {
        let looper = env.find_class(jni_str!("android/os/Looper"))?;
        let mine = env
            .call_static_method(
                &looper,
                jni_str!("myLooper"),
                jni_sig!("()Landroid/os/Looper;"),
                &[],
            )?
            .l()?;
        let main = env
            .call_static_method(
                &looper,
                jni_str!("getMainLooper"),
                jni_sig!("()Landroid/os/Looper;"),
                &[],
            )?
            .l()?;
        env.is_same_object(&mine, &main)
    }

    frust_plugin::android::with_jni_env(|env, _context| {
        let probe = on_main_looper(env);
        // Never leave a pending exception behind for the next JNI call, even
        // on a path whose whole answer is "I don't know".
        if env.exception_check() {
            env.exception_clear();
            return None;
        }
        probe.ok()
    })
    .ok()
    .flatten()
}

/// See the Android arm: `MainThreadMarker::new()` is `NSThread.isMainThread`,
/// so both Apple arms (UIKit and AppKit — the desktop host dispatches on
/// winit's event-loop thread, which is the process main thread) always have a
/// definitive answer.
#[cfg(any(target_os = "ios", target_os = "macos"))]
fn on_platform_main_thread() -> Option<bool> {
    Some(objc2::MainThreadMarker::new().is_some())
}

/// See the Android arm: a host with no platform arm (Linux/Windows/web) has no
/// platform main thread to be wrong about — and no platform dispatch either —
/// so the answer is always *unknown*.
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
fn on_platform_main_thread() -> Option<bool> {
    None
}

// --- the typed props channel -------------------------------------------------

/// How many **consecutive** failed `update` dispatches a slot may cost before
/// the runtime stops asking for another one (module doc's *A failed `update` is
/// retried, up to a cap*).
///
/// Three, and the number is a judgement rather than a measurement — nothing in
/// this repo has ever measured a native-widgets retry on device, and the
/// Android arm is compile-gated only. The reasoning it encodes: one attempt is
/// the app's own change and buys nothing extra; a second covers the transient
/// refusal this mechanism exists for (a setter that threw because a sibling
/// view had not been laid out yet, a resource that arrived one frame late); a
/// third is the cheap benefit of the doubt. Beyond that the evidence says the
/// component is broken, not unlucky, and every further attempt is a dispatch
/// and a `log::warn!` per rebuild on the platform main thread — which on a
/// screen that rebuilds every frame is per-frame JNI traffic and per-frame log
/// volume, and would falsify the crate's headline zero-crossing invariant for
/// the process lifetime. Small enough to bound the damage, large enough that a
/// component has to fail three times in a row to be given up on.
const RETRY_UPDATE_BUDGET: u32 = 3;

/// One slot's staged component value and props, as [`publish`] left them.
struct Staged {
    /// `Rc<C>` for the registered `C`, erased — cloned into the instance at
    /// create/update time.
    component: Rc<dyn Any>,
    /// `C::Props`, erased.
    props: Box<dyn Any>,
    /// Bumped by [`publish`] only when the incoming props differ from these —
    /// the wire-visible change signal (module doc's *Props travel beside the
    /// wire*) — or when [`Self::retry_update`] asks for one.
    generation: u64,
    /// Set by [`request_update_retry`] when a dispatch for this slot failed,
    /// and consumed by the next [`publish`], which then bumps the generation
    /// even for identical props.
    ///
    /// This is the whole retry mechanism (module doc's *A failed `update` is
    /// retried, up to a cap*): the runtime keeps its diff baseline on a
    /// failure, but only a wire change makes the differ hand it a second
    /// chance, and only this makes the wire change when the app's props do not.
    retry_update: bool,
    /// How many `update` dispatches for this slot have failed in a row, reset
    /// by [`clear_update_retry_budget`] on the first one that succeeds.
    ///
    /// The bound on the field above: once it reaches [`RETRY_UPDATE_BUDGET`],
    /// [`request_update_retry`] reports once and stops re-marking, so an
    /// unchanged rebuild costs nothing again. It survives a [`publish`]
    /// deliberately — the whole point is to count across the rebuilds that
    /// carry the retries — and dies with the entry when the mounting widget's
    /// [`forget`] reaper runs, which is the same slot-lifetime bound
    /// everything else in this table lives under.
    consecutive_update_failures: u32,
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
/// staged — or a failed dispatch marked the slot for retry
/// ([`request_update_retry`]) — which is what makes the platform-view differ
/// emit an `UpdateParams` exactly when a component's props actually changed, or
/// when a change it already reported failed to apply, and nothing at all
/// otherwise.
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
        let (generation, failures) = match staged.get(&slot) {
            // A slot whose staged props are a *different* component's (a kind
            // swap on a re-used slot id) counts as changed, not as equal.
            Some(previous) => {
                let unchanged = previous
                    .props
                    .downcast_ref::<C::Props>()
                    .is_some_and(|staged| *staged == props);
                // The retry mark forces the bump an unchanged republish would
                // not otherwise make — and is consumed by making it, so one
                // failure buys exactly one retry.
                let generation = if unchanged && !previous.retry_update {
                    previous.generation
                } else {
                    previous.generation.wrapping_add(1)
                };
                // Carried across the republish on purpose: the budget counts
                // failures across the very rebuilds that carry the retries, so
                // resetting it here would restore the unbounded loop exactly.
                (generation, previous.consecutive_update_failures)
            }
            None => (0, 0),
        };
        staged.insert(
            slot,
            Staged {
                component,
                props: Box::new(props),
                generation,
                retry_update: false,
                consecutive_update_failures: failures,
            },
        );
        generation
    })
    .unwrap_or(0)
}

/// Mark `slot` so the next [`publish`] bumps its props generation whatever the
/// app publishes — the retry half of the module doc's *A failed `update` is
/// retried, up to a cap* — **unless this slot has spent its budget.**
///
/// Called from [`Bridge`]'s dispatch when a component reports a failure, which
/// leaves the runtime's diff baseline behind the app's intent; without the
/// forced bump, an app that republishes the same props forever would emit no
/// further `UpdateParams` and the runtime would never get to try again.
///
/// The bound is [`RETRY_UPDATE_BUDGET`] **consecutive** failures. Reaching it
/// reports once at `warn` and leaves the mark unset, so a permanently failing
/// slot settles into inert rather than costing a dispatch and a log line on
/// every rebuild for the process lifetime; later calls for the same slot are
/// silent, since the one report is the point and repeating it would be the very
/// log volume the cap exists to stop. A success in between clears the count
/// ([`clear_update_retry_budget`]), so the cap never accumulates across
/// unrelated flakes.
///
/// A slot with nothing staged (its reaper already ran, or it was never a
/// component slot) is a silent no-op: there is no rebuild left to retry from.
fn request_update_retry(slot: SlotId) {
    with_staged(|staged| {
        let Some(entry) = staged.get_mut(&slot) else {
            return;
        };
        if entry.consecutive_update_failures >= RETRY_UPDATE_BUDGET {
            // Already given up on and already reported: stay inert and quiet.
            return;
        }
        entry.consecutive_update_failures += 1;
        if entry.consecutive_update_failures >= RETRY_UPDATE_BUDGET {
            log::warn!(
                "frust-native-widgets: native component slot {slot} failed \
                 {RETRY_UPDATE_BUDGET} consecutive updates — no further retries will be \
                 scheduled for it. Its native view keeps whatever state the last successful \
                 update left; a later props change is still dispatched (only the runtime's own \
                 retry is capped), and one that succeeds restores the full budget."
            );
            return;
        }
        entry.retry_update = true;
    });
}

/// Clear `slot`'s consecutive-failure count — called from [`Bridge::update`]
/// the moment a dispatch succeeds, which is what makes [`RETRY_UPDATE_BUDGET`]
/// a bound on a *run* of failures rather than on a slot's lifetime total.
///
/// A slot already at zero (the overwhelmingly common case) is left untouched,
/// so the ordinary success path costs one hash lookup and no write.
fn clear_update_retry_budget(slot: SlotId) {
    with_staged(|staged| {
        if let Some(entry) = staged.get_mut(&slot)
            && entry.consecutive_update_failures != 0
        {
            entry.consecutive_update_failures = 0;
        }
    });
}

/// Drop `slot`'s staged component and props — the teardown reaper, registered
/// from the mounting widget's `on_cleanup` exactly like
/// `crate::runtime`'s `forget_pending_callback`.
///
/// **Disposal alone does not bound this table.** A culled slot's dispose
/// resolves by native-view identity and never names a slot id, so a slot
/// disposed while off-screen and then re-published by a still-mounted widget
/// would strand its entry for the process lifetime without this. Idempotent: a
/// slot with nothing staged is a silent no-op.
pub(crate) fn forget(slot: SlotId) {
    with_staged(|staged| staged.remove(&slot));
}

/// The `params_json` a component slot's `platform_view` carries: the
/// runtime's two identity keys, the props generation [`publish`] returned,
/// and the app's active brightness (theme ladder L1's [`DARK`] wire bit) —
/// never a component's real props.
///
/// `dark` is the caller's to resolve (`crate::api::mount`'s `build_with_mode`
/// reads it off the same `use_context::<Theme>()` the six builders'
/// `ambient_theme_tokens` already does) — this module has no reactive
/// context of its own. Carrying the bit directly on the wire, rather than
/// folding it into a component's typed `Props` (which this crate
/// deliberately never touches — `crate::api::mount`'s *What a component's
/// builder does NOT carry*), means a brightness-only flip still changes
/// `params_json` byte-for-byte, bumping `PlatformViewFrame::params_generation`
/// and reaching `crate::appkit::theme`'s/`crate::apple::theme`'s shared
/// `brightness_is_dark`, which both read this exact key straight off a
/// slot's raw wire — unconditionally, for every registered kind, before any
/// per-kind decode — the same mechanism the six built-in controls' own
/// `params_for` already rides.
pub(crate) fn component_params(kind: &str, slot: SlotId, generation: u64, dark: bool) -> String {
    crate::runtime::with_identity(
        kind,
        slot,
        &format!("\"{PROPS_GENERATION_KEY}\":{generation},\"{DARK}\":{dark}"),
    )
}

/// How many slots currently have staged props — the staging table's own leak
/// bar, which every mount/unmount cycle must return to `0`.
#[allow(dead_code)] // the staging table's leak bar: tests only, by design
pub(crate) fn staged_count() -> usize {
    with_staged(|staged| staged.len()).unwrap_or(0)
}

/// Why a staged lookup missed — two answers a caller must never conflate
/// (module doc's *Kind and type must agree*).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StagedMiss {
    /// Nothing at all is staged for the slot: the ordinary shape of a replay
    /// or a dispatch arriving after the mounting widget's reaper ran.
    Unstaged,
    /// Something *is* staged, but it belongs to a different component than the
    /// one this kind is registered to — a kind/type mismatch, and a wiring bug
    /// rather than the lifecycle race the "nothing staged" wording would
    /// suggest.
    OtherComponent,
}

impl StagedMiss {
    /// The error this miss surfaces as, naming `what` was looked for (`props`
    /// or `component`) and, where the caller knows it, the `kind` the slot was
    /// mounted under.
    fn describe(self, slot: SlotId, what: &str, kind: Option<&str>) -> NativeWidgetError {
        match self {
            Self::Unstaged => NativeWidgetError::Params(format!(
                "native component slot {slot} has no published {what}"
            )),
            Self::OtherComponent => {
                let named = kind
                    .map(|kind| format!(" (kind '{kind}')"))
                    .unwrap_or_default();
                NativeWidgetError::Params(format!(
                    "native component slot {slot}{named} staged a different component's {what} — \
                     the kind a slot is mounted under must be the one that component was \
                     registered under"
                ))
            }
        }
    }
}

/// `slot`'s staged props, cloned for the runtime's own baseline copy.
///
/// A staging table already borrowed on this thread (the re-entrant publish
/// `with_staged` warns about) reads as [`StagedMiss::Unstaged`] — there is
/// nothing this can answer with, and the borrow itself is logged where it is
/// detected.
fn staged_props<C: NativeComponent>(slot: SlotId) -> Result<C::Props, StagedMiss> {
    with_staged(|staged| match staged.get(&slot) {
        None => Err(StagedMiss::Unstaged),
        Some(entry) => entry
            .props
            .downcast_ref::<C::Props>()
            .cloned()
            .ok_or(StagedMiss::OtherComponent),
    })
    .unwrap_or(Err(StagedMiss::Unstaged))
}

/// `slot`'s staged component value; see [`staged_props`] for the miss rules.
fn staged_component<C: NativeComponent>(slot: SlotId) -> Result<Rc<C>, StagedMiss> {
    with_staged(|staged| match staged.get(&slot) {
        None => Err(StagedMiss::Unstaged),
        Some(entry) => Rc::clone(&entry.component)
            .downcast::<C>()
            .map_err(|_| StagedMiss::OtherComponent),
    })
    .unwrap_or(Err(StagedMiss::Unstaged))
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
///
/// It also carries the two things an event dispatch needs that
/// `NativeWidget::on_event`'s `(state, event)` signature does not: the props
/// last applied (the typed baseline [`NativeComponent::on_event`] is handed)
/// and the [`ListenerKinds`] this slot's component attached — the event gate
/// (module doc's *Listener attachment*).
pub(crate) struct BridgeState<C: NativeComponent> {
    /// The slot whose staged entry [`Self::refresh_component`] re-reads. Slot
    /// ids are handed out by a process-wide monotonic counter
    /// (`crate::api::builders`' `next_local_slot`) and never recycled, so this
    /// can only ever name this slot's own staged entry.
    slot: SlotId,
    component: Rc<C>,
    state: C::State,
    /// The props the last `create` or successful `update` applied — the same
    /// value the runtime keeps as its diff baseline, so a failed `update`
    /// leaves this at `old` exactly as it leaves the runtime's.
    props: C::Props,
    /// Every family an `attach_listener` succeeded for on this slot, across
    /// `create` and every `update` — the union, never narrowed, since a
    /// listener detached on the platform side simply stops firing.
    listening: ListenerKinds,
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
    /// event or dispose.
    ///
    /// A slot with nothing staged (its `forget` reaper already ran) keeps the
    /// retained value: there is no newer value to be had, and nothing here
    /// resurrects a reaped entry or panics on its absence.
    fn refresh_component(&mut self) {
        if let Ok(component) = staged_component::<C>(self.slot) {
            self.component = component;
        }
    }
}

/// Check for — and clear — a Java exception a component left pending at a
/// dispatch boundary (module doc's *Dispatch-boundary exception guard*).
///
/// `ComponentCtx::env` is a safe fn handing a third party the live `Env`, and
/// "clear your own pending exception" is prose, not a type: a component that
/// forgets leaves the *next* JNI call — anyone's — on undefined ground. So the
/// runtime checks after every dispatch that carried a context. The crate's own
/// `run_jni` helper is the check: it extracts the throwable's class and message
/// before clearing, so the report names what was thrown. Nothing is called
/// inside it — whatever it finds was raised before we got here.
#[cfg(target_os = "android")]
fn guard_pending_exception(ctx: &mut PlatformCtx<'_, '_>, op: &str) -> Option<NativeWidgetError> {
    ctx.run_jni(op, |_env| Ok::<(), jni::errors::Error>(()))
        .err()
}

/// The same guard, for the one dispatch that is handed **no context** —
/// [`NativeWidget::on_event`] takes `(state, event)` and nothing else.
///
/// The `Env` comes from the process VM instead: `with_top_local_frame` borrows
/// the JNI stack frame the `nativeOnEvent` export is already running in rather
/// than pushing a new one, so the clean path is a `GetEnv` plus an
/// `ExceptionCheck`, and the handful of local references a *report* costs are
/// released when that export returns.
///
/// **Not `frust_plugin::android::with_jni_env`, deliberately.** That helper
/// attaches with jni 0.22's default `PreReThrowPostCatch` policy, which stashes
/// an already-pending exception before running the closure and re-throws it
/// after: a check inside it would read clean every time and clear nothing. (The
/// same policy also means a component that reaches JNI *through* that helper is
/// already caught on its own way out — this guard is for everything that does
/// not, from a raw `jni-sys` call to an attachment configured with `Ignore`.)
///
/// Answers `None` — *unknown*, never *clean* — when the platform handles are
/// not installed yet or the thread is not attached, both of which mean nothing
/// dispatched through here in the first place.
#[cfg(target_os = "android")]
fn guard_pending_exception_off_context(op: &str) -> Option<NativeWidgetError> {
    let vm = match frust_plugin::android::vm() {
        Ok(vm) => vm,
        Err(error) => {
            log::debug!(
                "frust-native-widgets: {op} boundary guard skipped — no platform handles: {error}"
            );
            return None;
        }
    };
    vm.with_top_local_frame(|env| {
        let mut ctx = PlatformCtx::detached(env);
        Ok::<Option<NativeWidgetError>, jni::errors::Error>(guard_pending_exception(&mut ctx, op))
    })
    .unwrap_or_else(|error: jni::errors::Error| {
        log::warn!("frust-native-widgets: {op} boundary guard could not reach a JNI env: {error}");
        None
    })
}

/// The Apple arms (iOS and macOS) have nothing to guard: there is no
/// pending-exception channel between a component and the runtime here (an ObjC
/// exception is not a return path — `crate::apple::factory`'s and
/// `crate::appkit::factory`'s contracts answer a failed create with a
/// placeholder view instead), and no `Env` whose next call could be poisoned.
#[cfg(any(target_os = "ios", target_os = "macos"))]
fn guard_pending_exception(_ctx: &mut PlatformCtx<'_, '_>, _op: &str) -> Option<NativeWidgetError> {
    None
}

/// See the context-carrying arm above: this platform has nothing to guard.
#[cfg(any(target_os = "ios", target_os = "macos"))]
fn guard_pending_exception_off_context(_op: &str) -> Option<NativeWidgetError> {
    None
}

/// The host arm has no JNI to check, so it counts instead — see
/// `dispatch_guard_count`.
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
fn guard_pending_exception(_ctx: &mut PlatformCtx<'_, '_>, _op: &str) -> Option<NativeWidgetError> {
    DISPATCH_GUARDS.with(|guards| guards.set(guards.get() + 1));
    None
}

/// The host arm of the context-free guard: it counts through the same tally, so
/// the wiring bar covers all four dispatches and not just the three that carry
/// a context.
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
fn guard_pending_exception_off_context(_op: &str) -> Option<NativeWidgetError> {
    DISPATCH_GUARDS.with(|guards| guards.set(guards.get() + 1));
    None
}

#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
thread_local! {
    /// How many times the dispatch-boundary guard ran on this thread — the
    /// host arm's stand-in for a check it cannot make, the same shape
    /// [`LIVE_CHILDREN`] uses for the leak bar. What a host test can prove is
    /// that **every** dispatch is *wired* to the guard — the three that carry a
    /// context and `on_event`, which reaches the VM instead; whether the JNI
    /// check itself finds a pending exception is Android-only, and this repo
    /// has no embedded-JVM harness to run it (`crate::android`'s own
    /// `create_failure_message` note).
    static DISPATCH_GUARDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many times the dispatch-boundary exception guard has run on this
/// thread.
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
#[allow(dead_code)] // the guard's wiring bar: tests only, by design
pub(crate) fn dispatch_guard_count() -> usize {
    DISPATCH_GUARDS.with(|guards| guards.get())
}

/// Keep the first failure and log the second — [`ComponentCtx::latch`]'s rule
/// one level up, where a component's own latched error meets whatever the
/// dispatch-boundary guard found behind it.
fn fold_error(
    first: Option<NativeWidgetError>,
    later: Option<NativeWidgetError>,
) -> Option<NativeWidgetError> {
    match (first, later) {
        (Some(first), Some(later)) => {
            log::warn!(
                "frust-native-widgets: component dispatch reported '{first}' and also left \
                 '{later}' behind it — reporting the first"
            );
            Some(first)
        }
        (first, later) => first.or(later),
    }
}

impl<C: NativeComponent> NativeWidget for Bridge<C> {
    type Props = BridgeProps<C>;
    type State = BridgeState<C>;

    /// The wire carries only identity and a generation, so "decoding" a
    /// component's props means taking the typed value the app staged for this
    /// slot (module doc's *Props travel beside the wire*).
    fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
        let (kind, slot) = params.identity()?;
        // A miss here is one of two very different things — a reaped entry, or
        // a slot mounted under a kind registered to another component — and
        // the message says which (module doc's *Kind and type must agree*).
        let props =
            staged_props::<C>(slot).map_err(|miss| miss.describe(slot, "props", Some(&kind)))?;
        Ok(BridgeProps { slot, props })
    }

    fn create(
        ctx: &mut PlatformCtx<'_, '_>,
        props: &Self::Props,
    ) -> Result<(NativeView, Self::State), NativeWidgetError> {
        // The kind is not carried this far (the runtime holds it, the props do
        // not), so the mismatch message names the slot alone — reachable here
        // rather than in `decode_props` only when the two components happen to
        // share one `Props` type (module doc's fourth row).
        let component = staged_component::<C>(props.slot)
            .map_err(|miss| miss.describe(props.slot, "component", None))?;

        let mut cx = ComponentCtx::new(ctx, props.slot);
        let built = component.create(&mut cx, &props.props);
        let (error, listening) = cx.into_parts();
        let latched = fold_error(
            error,
            guard_pending_exception(ctx, "NativeComponent::create"),
        );
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
                        props: props.props.clone(),
                        listening,
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
        let mut cx = ComponentCtx::new(ctx, new.slot);
        component.update(&mut cx, &mut state.state, &old.props, &new.props);
        let (error, attached) = cx.into_parts();
        // Recorded whatever the update's outcome: a listener attached before a
        // later setter failed is live, and its handle is in the component's
        // state now.
        state.listening |= attached;
        let error = fold_error(
            error,
            guard_pending_exception(ctx, "NativeComponent::update"),
        );
        match error {
            Some(error) => {
                // Reported, so the runtime keeps `old` as the diff baseline —
                // and marked, so the next rebuild's `publish` bumps the wire
                // even if the app republishes identical props, which is the
                // only thing that gets the runtime a second attempt. The mark
                // is refused once this slot has spent its consecutive-failure
                // budget, which is what keeps a permanently failing component
                // from costing a dispatch per rebuild forever (module doc's
                // *A failed `update` is retried, up to a cap*).
                request_update_retry(new.slot);
                Err(error)
            }
            None => {
                // A success ends the run of failures, so a flaky platform never
                // accumulates its way to the cap — and moves the baseline
                // `on_event` is handed, in step with the runtime's own.
                clear_update_retry_budget(new.slot);
                state.props = new.props.clone();
                Ok(())
            }
        }
    }

    /// The event gate, then the component's own answer (module doc's
    /// *Listener attachment*).
    ///
    /// **The gate:** an event whose [`NativeEvent::family`] this slot never
    /// attached a listener for answers `None` without reaching the component
    /// at all. The runtime routes on the slot id alone, so without this a
    /// hand-built Android `FrustNativeListener` carrying a fabricated id that
    /// named a live component's slot would be delivered like a real event; with
    /// it, only the families the component itself asked for arrive. (A
    /// fabricated id naming a slot that *did* attach that family is still
    /// indistinguishable from the real listener — the runtime asks nothing
    /// about which object fired, for components and the six controls alike.)
    ///
    /// **The answer:** [`NativeComponent::on_event`]'s, mapped into the six
    /// controls' `EventPayload` vocabulary so it rides their callback table to
    /// the app's hook (`crate::api::mount`). A kind that vocabulary has no word
    /// for is dropped and logged.
    ///
    /// The staged value is re-read first — an event can arrive after any
    /// number of rebuilds that changed the component but not its props, and
    /// the props diff gate skips `update` on every one of them
    /// ([`BridgeState::refresh_component`]).
    ///
    /// **Guarded like the other three, through the VM rather than a context**,
    /// because this signature carries neither a context nor an `Env`: the
    /// leftover-exception hazard is the dispatch's, not the context's, and the
    /// only error channel here is the log (this returns `Option`, not
    /// `Result`). See `guard_pending_exception_off_context` and the module
    /// doc's *Dispatch-boundary exception guard*. The gate's early `None`
    /// crosses no FFI and runs no component code, so it has nothing to guard.
    fn on_event(state: &mut Self::State, event: WireEvent) -> Option<EventPayload> {
        let event = NativeEvent::from_wire(event);
        let family = event.family();
        if family.is_empty() || !state.listening.contains(family) {
            log::debug!(
                "frust-native-widgets: component slot {} got event kind {} but attached no \
                 listener for it (attached: {}) — dropped",
                state.slot,
                event.kind(),
                state.listening
            );
            return None;
        }
        state.refresh_component();
        let component = Rc::clone(&state.component);
        let answer = component.on_event(&mut state.state, &state.props, event);
        if let Some(error) = guard_pending_exception_off_context("NativeComponent::on_event") {
            log::warn!(
                "frust-native-widgets: component slot {} left a Java exception pending after \
                 on_event — cleared here, but the rest of that dispatch ran on undefined \
                 ground: {error}",
                state.slot
            );
        }
        let answer = answer?;
        let payload = answer.into_payload();
        if payload.is_none() {
            log::debug!(
                "frust-native-widgets: component slot {} answered event kind {}, which the \
                 app-facing callback has no vocabulary for — dropped",
                state.slot,
                answer.kind()
            );
        }
        payload
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
            slot,
            component,
            state,
            props: _,
            listening: _,
        } = state;
        let mut cx = ComponentCtx::new(ctx, slot);
        component.dispose(&mut cx, state);
        let (error, _attached) = cx.into_parts();
        match fold_error(
            error,
            guard_pending_exception(ctx, "NativeComponent::dispose"),
        ) {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

// Gated on the host arm, not merely on `test` (the same gate `crate::runtime`'s
// own tests carry): these drive the public trait through the real runtime using
// the host stand-in context, which a platform build (macOS included, since it
// has a real AppKit arm) replaces with the platform-only types.
#[cfg(all(
    test,
    not(any(target_os = "android", target_os = "ios", target_os = "macos"))
))]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::sync::{Mutex, Once};

    use super::*;
    use crate::runtime::{DisposeOutcome, UpdateOutcome};

    // --- the log sink -------------------------------------------------------

    /// Everything logged since the sink was installed.
    static CAPTURED: Mutex<Vec<String>> = Mutex::new(Vec::new());

    /// A process-wide `log` sink, so a test can prove a failure was *logged*
    /// rather than merely dropped.
    ///
    /// The latch keeps the first error and reports nothing else — by design —
    /// so "a later error is not silently swallowed" has exactly one observable
    /// channel, and this is it. Shared across test threads and never cleared:
    /// assertions match on a marker string unique to their own test rather
    /// than on the sink's contents as a whole.
    struct CapturedLog;

    impl log::Log for CapturedLog {
        fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
            true
        }

        fn log(&self, record: &log::Record<'_>) {
            if let Ok(mut captured) = CAPTURED.lock() {
                captured.push(record.args().to_string());
            }
        }

        fn flush(&self) {}
    }

    static CAPTURE: CapturedLog = CapturedLog;

    /// Install the sink, once per process. `log`'s runtime max level defaults
    /// to `Off`, so it is raised here too or `log::warn!` would compile to
    /// nothing observable.
    fn install_log_capture() {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(|| {
            if log::set_logger(&CAPTURE).is_ok() {
                log::set_max_level(log::LevelFilter::Warn);
            }
        });
    }

    /// Whether any captured line contains `needle`.
    fn logged(needle: &str) -> bool {
        CAPTURED
            .lock()
            .map(|captured| captured.iter().any(|line| line.contains(needle)))
            .unwrap_or(false)
    }

    /// A component defined **outside the six** — the whole point of the
    /// acceptance bar: it implements nothing but the public
    /// [`NativeComponent`] trait, using only the public [`ComponentCtx`]/
    /// [`NativeRoot`]/[`NativeEvent`] surface, exactly as a third-party crate
    /// would.
    struct Gauge {
        /// Where this component records what it did. `Rc` on purpose: a test
        /// holding the other end can assert both the calls *and* (via
        /// `strong_count`) that the runtime actually dropped the component
        /// value rather than stranding it — the same leak-probe pattern
        /// established for every slot-keyed table in this crate.
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
        /// The listener `create` attached to the gauge's root — a click and a
        /// value-changed listener, the two kinds these tests fire.
        _listener: ListenerHandle,
    }

    const GAUGE_KIND: &str = "test-gauge";

    /// The [`GaugeProps::label`] that makes `create` answer `None` — the
    /// failed-create test's opt-in, mirroring `crate::runtime`'s own
    /// `FAIL_CREATE` sentinel.
    const FAIL_CREATE: &str = "FAIL_CREATE";

    /// The [`GaugeProps::label`] that makes `update` latch an error.
    const FAIL_UPDATE: &str = "FAIL_UPDATE";

    /// The [`GaugeProps::label`] that makes `update` latch an error on its
    /// **first** attempt only — the retry test's opt-in, so a retry can be
    /// observed succeeding rather than merely being attempted again.
    const FAIL_UPDATE_ONCE: &str = "FAIL_UPDATE_ONCE";

    /// The [`GaugeProps::label`] that makes `update` latch an error on **every**
    /// attempt *and record each one* — the retry-cap test's opt-in. It is a
    /// separate sentinel from [`FAIL_UPDATE`] only because that one records
    /// nothing, and the cap is a statement about *how many times* the component
    /// was asked.
    const FAIL_UPDATE_ALWAYS: &str = "FAIL_UPDATE_ALWAYS";

    thread_local! {
        /// Whether [`FAIL_UPDATE_ONCE`]'s single failure has been spent. Per
        /// test thread, like every other thread-local this module's tests
        /// lean on (the staging table, the runtime itself).
        static UPDATE_FAILED_ONCE: Cell<bool> = const { Cell::new(false) };
    }

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
            let listener = ctx.attach_listener(
                identity,
                ListenerKinds::CLICK | ListenerKinds::VALUE_CHANGED,
            )?;
            let root = ctx.root(identity)?;
            Some((
                root,
                GaugeState {
                    identity,
                    events: Vec::new(),
                    _listener: listener,
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
            if new.label == FAIL_UPDATE_ALWAYS {
                self.note(format!("{} update refused", self.tag));
                ctx.report_error("gauge: setter always throws");
                return;
            }
            if new.label == FAIL_UPDATE_ONCE && !UPDATE_FAILED_ONCE.replace(true) {
                self.note(format!("{} update failed", self.tag));
                ctx.report_error("gauge: setter threw once");
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

        fn on_event(
            &self,
            state: &mut Self::State,
            _props: &Self::Props,
            event: NativeEvent,
        ) -> Option<NativeEvent> {
            state.events.push(event);
            self.note(format!(
                "{} event {}/{}",
                self.tag,
                event.kind(),
                event.detail()
            ));
            Some(event)
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
    /// builder will run on every rebuild. Brightness is out of scope for
    /// every test that calls this (`publish_bumps_the_generation_only_when_
    /// the_props_change` exercises the `dark` bit directly instead), so it is
    /// pinned to `false` here.
    fn mount(slot: SlotId, component: Gauge, props: GaugeProps) -> String {
        let generation = publish(slot, Rc::new(component), props);
        component_params(GAUGE_KIND, slot, generation, false)
    }

    #[test]
    fn a_component_outside_the_six_lives_the_whole_lifecycle() {
        // The acceptance bar: create → update → event → dispose, driven
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
        assert_eq!(
            calls.len(),
            4,
            "one platform call each, plus create's listener attach: {calls:?}"
        );
        assert!(calls[0].starts_with("gauge create"));
        assert!(calls[1].starts_with("attachListener "), "{calls:?}");
        assert!(calls[2].contains("10 -> 42"));
        assert!(calls[3].starts_with("gauge dispose"));
    }

    #[test]
    fn an_unchanged_props_republish_still_reaches_the_newest_component_value() {
        // The regression this test exists for: the props diff gate
        // (`crate::runtime`'s `update_params`) returns `Unchanged` before
        // touching the vtable, so if `update` were the only thing refreshing
        // the bridge's retained component value, a rebuild republishing equal
        // props with functionally different closures would leave
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
            vec![
                "gauge",
                "attachListener",
                "legacy",
                "gauge",
                "gauge",
                "legacy"
            ],
            "each command reached its own kind's impl and nobody else's: {calls:?}"
        );
    }

    #[test]
    fn the_staging_table_is_reaped_by_forget_and_strands_nothing() {
        // The same leak shape, one table over: `forget` (the mounting
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
            "disposal alone leaves the staged entry — the same leak shape as above"
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

        // Theme ladder L1: `component_params` also carries the app's active
        // brightness directly on the wire, not through the staged/diffed
        // props above — so an unchanged republish with only the brightness
        // flipped must still differ, and the flag round-trips exactly the
        // way `crate::appkit::theme`'s/`crate::apple::theme`'s shared
        // `brightness_is_dark` reads it back (`Params::flag(DARK)`, the same
        // call both make).
        let generation = publish(3, Rc::new(Gauge::new(&log, "c")), props("Disk", 2));
        let light = component_params(GAUGE_KIND, 3, generation, false);
        let dark = component_params(GAUGE_KIND, 3, generation, true);
        assert_ne!(
            light, dark,
            "an unchanged props republish with a flipped brightness must still change the wire"
        );
        assert_eq!(Params::new(&light).flag(DARK), Some(false));
        assert_eq!(Params::new(&dark).flag(DARK), Some(true));

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
        let params = component_params(GAUGE_KIND, 4, 0, false);
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

    #[test]
    fn a_failed_update_retries_even_when_the_app_republishes_identical_props() {
        // Keeping the diff baseline (the test above) is only half
        // a retry: the runtime cannot re-apply anything it is never handed
        // again, and it is handed props only when the differ emits an
        // `UpdateParams`, which it does only when the wire changes. An app
        // whose props settled — the ordinary shape of a value that failed to
        // apply once and is simply still true — would republish a
        // byte-identical wire forever and the failed change would never be
        // retried at all.
        let log = Rc::new(RefCell::new(Vec::new()));
        register_component::<Gauge>(GAUGE_KIND);
        let created = mount(60, Gauge::new(&log, "g"), props("Net", 1));

        let mut calls = Vec::new();
        {
            let mut ctx = PlatformCtx::new(&mut calls);
            with_runtime(|runtime| {
                runtime.create(&mut ctx, &created).unwrap();

                // The change the platform refuses on its first attempt.
                let failing = mount(60, Gauge::new(&log, "g"), props(FAIL_UPDATE_ONCE, 2));
                assert_ne!(failing, created, "changed props changed the wire");
                assert!(runtime.update_params(&mut ctx, &failing).is_err());

                // The next rebuild publishes *exactly the same props again* —
                // and must still move the wire, or nothing will ever ask the
                // runtime to try again.
                let republished = mount(60, Gauge::new(&log, "g"), props(FAIL_UPDATE_ONCE, 2));
                assert_ne!(
                    republished, failing,
                    "a failed update must force the next publish to bump the props \
                     generation, identical props or not — otherwise the differ emits \
                     nothing and the change is lost for the process lifetime"
                );
                assert_eq!(
                    runtime.update_params(&mut ctx, &republished).unwrap(),
                    UpdateOutcome::Applied,
                    "the retry reached the component and applied"
                );

                // One failure buys exactly one retry: with the change applied,
                // an unchanged rebuild is back to costing nothing.
                let settled = mount(60, Gauge::new(&log, "g"), props(FAIL_UPDATE_ONCE, 2));
                assert_eq!(
                    settled, republished,
                    "the retry mark is consumed by the bump it forced"
                );
                assert_eq!(
                    runtime.update_params(&mut ctx, &settled).unwrap(),
                    UpdateOutcome::Unchanged
                );

                assert_eq!(runtime.dispose_slot(&mut ctx, 60), DisposeOutcome::Disposed);
            })
            .expect("the thread's runtime");
        }
        forget(60);

        assert_eq!(
            *log.borrow(),
            vec![
                "g create 'Net'".to_string(),
                "g update failed".to_string(),
                "g update 1 -> 2".to_string(),
                "g dispose".to_string(),
            ],
            "the change was attempted, failed, retried against the ORIGINAL baseline \
             (1, not 2 — the failed attempt advanced nothing), and applied"
        );
    }

    /// Drive `rebuilds` rebuilds that republish **byte-identical** props for
    /// `slot`, dispatching one `update_params` per rebuild whose wire actually
    /// moved — which is exactly what the platform-view differ does — and return
    /// the `params_json` each rebuild produced.
    ///
    /// The wire check is the whole point: the differ emits an `UpdateParams`
    /// only when a slot's `params_json` changes, so a rebuild whose params come
    /// back identical costs the runtime nothing at all. Calling
    /// `update_params` unconditionally would model a differ this repo does not
    /// have and would make the cap look ineffective (the runtime compares props
    /// against its own baseline, which a failed update never advances).
    fn republish_identical(
        runtime: &mut crate::runtime::NativeRuntime,
        ctx: &mut PlatformCtx<'_, '_>,
        slot: SlotId,
        log: &Rc<RefCell<Vec<String>>>,
        label: &str,
        value: i32,
        rebuilds: usize,
    ) -> Vec<String> {
        let mut wire = Vec::with_capacity(rebuilds);
        let mut previous: Option<String> = None;
        for _ in 0..rebuilds {
            let params = mount(slot, Gauge::new(log, "g"), props(label, value));
            if previous.as_deref() != Some(params.as_str()) {
                let _ = runtime.update_params(ctx, &params);
            }
            previous = Some(params.clone());
            wire.push(params);
        }
        wire
    }

    #[test]
    fn a_permanently_failing_update_stops_retrying_at_the_cap() {
        // The retry mechanism above is unbounded on its own:
        // every failed dispatch re-marks the slot, the next rebuild's `publish`
        // bumps the generation for byte-identical props, the differ emits an
        // `UpdateParams`, the retry fails and re-marks — one FFI dispatch plus
        // one `log::warn!` per rebuild, forever, on the platform main thread.
        // On a screen that rebuilds every frame that is per-frame JNI traffic
        // and per-frame log volume, and it falsifies the crate's headline
        // "an unchanged rebuild costs zero FFI crossings" for the process
        // lifetime.
        //
        // So the budget caps it. What is asserted here is the *observable*
        // consequence: the wire stops moving, so the differ stops asking, so
        // the component stops being called.
        install_log_capture();
        let log = Rc::new(RefCell::new(Vec::new()));
        register_component::<Gauge>(GAUGE_KIND);
        let created = mount(62, Gauge::new(&log, "g"), props("Net", 1));

        let mut calls = Vec::new();
        {
            let mut ctx = PlatformCtx::new(&mut calls);
            with_runtime(|runtime| {
                runtime.create(&mut ctx, &created).unwrap();

                // Ten rebuilds of a screen whose props settled on a value the
                // platform refuses. Pre-cap, all ten would have dispatched.
                let wire =
                    republish_identical(runtime, &mut ctx, 62, &log, FAIL_UPDATE_ALWAYS, 2, 10);

                // The first rebuild moved the wire because the props really
                // changed; the next two moved it because a failure re-marked
                // the slot. From the fourth on the budget is spent and the wire
                // is frozen — no `UpdateParams`, nothing dispatched, nothing
                // logged.
                assert_ne!(wire[0], created, "changed props changed the wire");
                assert_ne!(wire[1], wire[0], "failure 1 forced a retry bump");
                assert_ne!(wire[2], wire[1], "failure 2 forced a retry bump");
                for (index, params) in wire.iter().enumerate().skip(3) {
                    assert_eq!(
                        *params, wire[2],
                        "rebuild {index} must leave the wire untouched: three consecutive \
                         failures spend the budget, and an unchanged rebuild is back to \
                         costing zero FFI crossings"
                    );
                }

                assert_eq!(runtime.dispose_slot(&mut ctx, 62), DisposeOutcome::Disposed);
            })
            .expect("the thread's runtime");
        }
        forget(62);

        assert_eq!(
            *log.borrow(),
            vec![
                "g create 'Net'".to_string(),
                "g update refused".to_string(),
                "g update refused".to_string(),
                "g update refused".to_string(),
                "g dispose".to_string(),
            ],
            "the component was asked exactly RETRY_UPDATE_BUDGET (3) times across ten \
             rebuilds, then never again — the observed behaviour at the cap is *inert*, \
             not slower retries"
        );
        assert!(
            logged("no further retries will be scheduled"),
            "giving up is reported once, at warn — a slot that silently stops trying is \
             the other way to lose a native view"
        );
    }

    #[test]
    fn a_successful_update_restores_the_whole_retry_budget() {
        // The cap counts *consecutive* failures, which is what keeps a flaky
        // platform from accumulating its way to inert over a long session: two
        // refusals, one success, and the next bad patch gets the full three
        // attempts again rather than the one it would have left.
        let log = Rc::new(RefCell::new(Vec::new()));
        register_component::<Gauge>(GAUGE_KIND);
        let created = mount(63, Gauge::new(&log, "g"), props("Net", 1));

        let mut calls = Vec::new();
        {
            let mut ctx = PlatformCtx::new(&mut calls);
            with_runtime(|runtime| {
                runtime.create(&mut ctx, &created).unwrap();

                // Two failures — one short of the cap.
                republish_identical(runtime, &mut ctx, 63, &log, FAIL_UPDATE_ALWAYS, 2, 2);

                // A props change the component accepts. This is what clears the
                // count; the retry mark plays no part (changed props bump the
                // generation on their own).
                let recovered = mount(63, Gauge::new(&log, "g"), props("Net", 3));
                assert_eq!(
                    runtime.update_params(&mut ctx, &recovered).unwrap(),
                    UpdateOutcome::Applied
                );

                // A fresh bad patch now gets three attempts, not one.
                republish_identical(runtime, &mut ctx, 63, &log, FAIL_UPDATE_ALWAYS, 6, 10);

                assert_eq!(runtime.dispose_slot(&mut ctx, 63), DisposeOutcome::Disposed);
            })
            .expect("the thread's runtime");
        }
        forget(63);

        assert_eq!(
            *log.borrow(),
            vec![
                "g create 'Net'".to_string(),
                "g update refused".to_string(),
                "g update refused".to_string(),
                "g update 1 -> 3".to_string(),
                "g update refused".to_string(),
                "g update refused".to_string(),
                "g update refused".to_string(),
                "g dispose".to_string(),
            ],
            "two refusals, a success that cleared the count, then a full budget of three"
        );
    }

    #[test]
    fn a_later_error_inside_a_local_frame_is_logged_rather_than_swallowed() {
        // Two properties, pinned at once.
        //
        // The latch is first-wins, so the frame's error cannot *replace* the
        // root failure — but it must not vanish without a trace either, and it
        // would vanish twice over on Android if that arm handed the closure a
        // FRESH latch: `failed()` would read `false` inside a frame where this
        // arm (and iOS) read `true`, and the merge back would be a second,
        // silent first-wins drop on top of the latch's own.
        //
        // Android's fresh *context* is structurally forced (a pushed frame's
        // references carry other lifetimes); what it carries is this arm's
        // semantics, which is what this test pins.
        const ROOT: &str = "m-01 root failure";
        const INSIDE_FRAME: &str = "m-01 failure raised inside the frame";
        install_log_capture();

        let mut calls = Vec::new();
        let mut ctx = PlatformCtx::new(&mut calls);
        let mut cx = ComponentCtx::new(&mut ctx, 0);

        cx.report_error(ROOT);
        assert!(cx.failed());

        let value = cx.with_local_frame(4, |inner| {
            assert!(
                inner.failed(),
                "a frame carries its caller's latch — the property Android used to \
                 answer `false` to"
            );
            inner.report_error(INSIDE_FRAME);
            None::<u64>
        });

        assert!(value.is_none());
        let error = cx.into_parts().0.expect("the root failure still reports");
        assert!(
            matches!(&error, NativeWidgetError::Platform(message) if message == ROOT),
            "the first error stays the reported one: {error:?}"
        );
        assert!(
            logged(INSIDE_FRAME),
            "the frame's error must still reach the log — a swallowed platform \
             failure is the defect, not the first-wins report"
        );
    }

    #[test]
    fn every_dispatch_runs_the_boundary_exception_guard() {
        // What a host can prove is the wiring: ALL FOUR dispatches run the
        // guard. `create`, `update` and `dispose` reach it through their
        // context; `on_event` carries none — `NativeWidget::on_event` takes
        // `(state, event)` and nothing else — so it reaches a JNI env through
        // the process VM instead (`guard_pending_exception_off_context`).
        //
        // Four, not three: "on_event has no context to guard through" is not a
        // licence to skip it. The export runs `debug_assert_main_thread` and
        // further JNI after `runtime.on_event` returns, so a leftover exception
        // is *ours* to trip over, and an attached listener's event reaches
        // the trait method on every platform arm (module doc's *Listener
        // attachment*). A reachable UB path guarded on three of four
        // dispatches is not a resting place.
        //
        // Whether the guard's JNI check actually *finds* a pending exception is
        // Android-only and unrunnable here (no embedded-JVM harness in this
        // repo); the host arm counts instead.
        let log = Rc::new(RefCell::new(Vec::new()));
        register_component::<Gauge>(GAUGE_KIND);
        let before = dispatch_guard_count();

        let params = mount(61, Gauge::new(&log, "g"), props("Net", 1));
        let mut calls = Vec::new();
        {
            let mut ctx = PlatformCtx::new(&mut calls);
            with_runtime(|runtime| {
                runtime.create(&mut ctx, &params).unwrap();
                let changed = mount(61, Gauge::new(&log, "g"), props("Net", 2));
                assert_eq!(
                    runtime.update_params(&mut ctx, &changed).unwrap(),
                    UpdateOutcome::Applied
                );
                runtime.on_event(61, WireEvent { kind: 1, detail: 0 });
                assert_eq!(runtime.dispose_slot(&mut ctx, 61), DisposeOutcome::Disposed);
            })
            .expect("the thread's runtime");
        }
        forget(61);

        assert_eq!(
            dispatch_guard_count() - before,
            4,
            "create, update, dispose AND on_event are guarded — the fourth is \
             the one this test used to license the absence of"
        );
    }

    // --- The four kind/type mismatch cases -----------------------------------

    /// A component that shares [`Gauge`]'s `Props` type but not its identity —
    /// the case where the props downcast *succeeds* and the mismatch surfaces
    /// one step later, at the component downcast.
    struct Twin;

    impl NativeComponent for Twin {
        type Props = GaugeProps;
        type State = u64;

        fn create(
            &self,
            ctx: &mut ComponentCtx<'_, '_, '_>,
            _props: &Self::Props,
        ) -> Option<(NativeRoot, Self::State)> {
            let identity = next_identity();
            Some((ctx.root(identity)?, identity))
        }

        fn update(
            &self,
            _ctx: &mut ComponentCtx<'_, '_, '_>,
            _state: &mut Self::State,
            _old: &Self::Props,
            _new: &Self::Props,
        ) {
        }
    }

    #[test]
    fn case_a_a_kind_nothing_registered_fails_with_unknown_control() {
        // The only one of the four that surfaces as `UnknownControl`, raised by
        // the runtime's own dispatch before any decode runs.
        let log = Rc::new(RefCell::new(Vec::new()));
        let generation = publish(70, Rc::new(Gauge::new(&log, "g")), props("CPU", 1));
        let params = component_params("test-never-registered", 70, generation, false);

        let mut calls = Vec::new();
        let mut ctx = PlatformCtx::new(&mut calls);
        let error = with_runtime(|runtime| {
            let outcome = runtime.create(&mut ctx, &params);
            assert_eq!(runtime.live_count(), 0, "fails closed: no instance");
            outcome
        })
        .expect("the thread's runtime")
        .expect_err("nothing is registered under that kind");

        assert!(
            matches!(&error, NativeWidgetError::UnknownControl(kind)
                if kind == "test-never-registered"),
            "{error:?}"
        );
        assert!(calls.is_empty(), "nothing crossed the boundary");
        forget(70);
    }

    #[test]
    fn case_c_a_kind_registered_to_another_component_fails_at_decode() {
        // Registered to `Gauge`, mounted with `Card`: the staged props are
        // `CardProps`, so `Bridge::<Gauge>::decode_props` cannot downcast them.
        // This is a `Params` error, not `UnknownControl`, and its message names
        // the mismatch rather than reading like an ordinary reaped entry.
        let log = Rc::new(RefCell::new(Vec::new()));
        register_component::<Gauge>(GAUGE_KIND);
        // A `Card` staged under the `Gauge` kind — the typo a `kind` string
        // passed twice invites, and one nothing type-checks.
        let generation = publish(
            71,
            Rc::new(Card {
                children: 1,
                fail_after: None,
                log: Rc::clone(&log),
            }),
            CardProps {
                title: "wrong kind".to_string(),
            },
        );
        let params = component_params(GAUGE_KIND, 71, generation, false);

        let mut calls = Vec::new();
        let mut ctx = PlatformCtx::new(&mut calls);
        let error = with_runtime(|runtime| {
            let outcome = runtime.create(&mut ctx, &params);
            assert_eq!(runtime.live_count(), 0, "fails closed: no instance");
            outcome
        })
        .expect("the thread's runtime")
        .expect_err("the staged props are another component's");

        assert!(
            matches!(&error, NativeWidgetError::Params(message)
                if message.contains("staged a different component's props")
                    && message.contains(GAUGE_KIND)),
            "the message must name the wiring bug, not read as a reaped entry: {error:?}"
        );
        assert!(calls.is_empty(), "nothing crossed the boundary");
        forget(71);
    }

    #[test]
    fn case_c_two_components_sharing_a_props_type_fail_one_step_later() {
        // Registered to `Gauge`, mounted with `Twin`, whose `Props` type IS
        // `GaugeProps`: the props downcast succeeds, so the mismatch surfaces
        // in `create` instead — still a `Params` error, still fail-closed, but
        // a different message and a different call.
        register_component::<Gauge>(GAUGE_KIND);
        let generation = publish(72, Rc::new(Twin), props("CPU", 1));
        let params = component_params(GAUGE_KIND, 72, generation, false);

        let mut calls = Vec::new();
        let mut ctx = PlatformCtx::new(&mut calls);
        let error = with_runtime(|runtime| {
            let outcome = runtime.create(&mut ctx, &params);
            assert_eq!(runtime.live_count(), 0, "fails closed: no instance");
            outcome
        })
        .expect("the thread's runtime")
        .expect_err("the staged component is a `Twin`, not a `Gauge`");

        assert!(
            matches!(&error, NativeWidgetError::Params(message)
                if message.contains("staged a different component's component")),
            "{error:?}"
        );
        assert!(calls.is_empty(), "nothing crossed the boundary");
        forget(72);
    }

    #[test]
    fn case_d_a_second_component_under_one_kind_is_refused_and_never_shadows() {
        // Two components, one kind: registration is first-wins, so the second
        // is refused with a warning and the incumbent keeps serving the kind.
        // Mounting the loser under it then reduces to case (c) — proven here
        // rather than assumed, since "refused" would be worth little if the
        // loser's slots quietly ran the winner's code.
        install_log_capture();
        assert!(register_component::<Gauge>(GAUGE_KIND));
        assert!(
            !register_component::<Twin>(GAUGE_KIND),
            "the second component under one kind is refused"
        );
        assert!(
            logged("is already registered"),
            "and says so — a silent refusal is how a component 'never appears'"
        );

        let generation = publish(73, Rc::new(Twin), props("CPU", 1));
        let params = component_params(GAUGE_KIND, 73, generation, false);
        let mut calls = Vec::new();
        let mut ctx = PlatformCtx::new(&mut calls);
        let error = with_runtime(|runtime| runtime.create(&mut ctx, &params))
            .expect("the thread's runtime")
            .expect_err("the loser's slot must not run the incumbent's code");

        assert!(matches!(&error, NativeWidgetError::Params(_)), "{error:?}");
        forget(73);
    }

    // --- A component owns its own native subtree -----------------------------

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
            // proved on device with fifty of them (module doc).
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
        component_params(CARD_KIND, slot, generation, false)
    }

    #[test]
    fn a_component_builds_a_native_subtree_and_releases_every_child() {
        // The same on-device stress count that motivated this design: 50
        // children in ONE slot, 52 global refs at peak → 0 after the dispose
        // cycle.
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

        // The teardown bar, counted rather than assumed: both leak shapes
        // this design guards against looked fine until something counted.
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

    // --- listener attachment (the retired display-only gap) -----------------

    thread_local! {
        /// How many times [`Relay::on_event`] actually ran on this thread — the
        /// probe that tells "the bridge's gate dropped it" apart from "the
        /// component was asked and answered `None`".
        static RELAYED: Cell<usize> = const { Cell::new(0) };
    }

    fn relayed() -> usize {
        RELAYED.with(Cell::get)
    }

    /// The [`GaugeProps::label`] that makes [`Relay::create`] attach its click
    /// listener; any other label attaches nothing.
    const LISTEN: &str = "listen";

    const RELAY_KIND: &str = "test-relay";

    /// A component whose answer is distinguishable from a pass-through: it
    /// attaches a click listener to its root (when told to) and answers every
    /// click with a `Toggled` carrying whether its last-applied props' value is
    /// positive — so a test sees the *component's* answer, built from the
    /// props the bridge handed it, not the event that arrived.
    struct Relay;

    impl NativeComponent for Relay {
        type Props = GaugeProps;
        type State = (u64, Option<ListenerHandle>);

        fn create(
            &self,
            ctx: &mut ComponentCtx<'_, '_, '_>,
            props: &Self::Props,
        ) -> Option<(NativeRoot, Self::State)> {
            let identity = next_identity();
            let listener = if props.label == LISTEN {
                Some(ctx.attach_listener(identity, ListenerKinds::CLICK)?)
            } else {
                None
            };
            Some((ctx.root(identity)?, (identity, listener)))
        }

        fn update(
            &self,
            _ctx: &mut ComponentCtx<'_, '_, '_>,
            _state: &mut Self::State,
            _old: &Self::Props,
            _new: &Self::Props,
        ) {
        }

        fn on_event(
            &self,
            _state: &mut Self::State,
            props: &Self::Props,
            event: NativeEvent,
        ) -> Option<NativeEvent> {
            RELAYED.with(|count| count.set(count.get() + 1));
            event
                .is_click()
                .then(|| NativeEvent::from_payload(EventPayload::Toggled(props.value > 0)))
        }
    }

    fn click() -> WireEvent {
        WireEvent {
            kind: EVENT_KIND_CLICK,
            detail: 0,
        }
    }

    #[test]
    fn bridge_on_event_returns_the_component_answer_only_for_an_attached_family() {
        // The deliverable this test pins: `Bridge::on_event` used to answer
        // `None` unconditionally; now a routed event returns whatever the
        // component answers, and a slot without an attached listener still
        // answers `None` — without the component ever being asked.
        let mut calls = Vec::new();
        let mut ctx = PlatformCtx::new(&mut calls);

        publish(90, Rc::new(Relay), props(LISTEN, 5));
        let (_view, mut listening) = Bridge::<Relay>::create(
            &mut ctx,
            &BridgeProps {
                slot: 90,
                props: props(LISTEN, 5),
            },
        )
        .expect("a listening relay builds");
        assert_eq!(live_listener_count(), 1, "create attached one listener");

        let before = relayed();
        assert_eq!(
            Bridge::<Relay>::on_event(&mut listening, click()),
            Some(EventPayload::Toggled(true)),
            "a click on an attached CLICK listener returns the component's own \
             answer (a Toggled built from its props), not the click that arrived"
        );
        assert_eq!(relayed(), before + 1);

        // A family this slot never attached is gated at the bridge.
        assert_eq!(
            Bridge::<Relay>::on_event(
                &mut listening,
                WireEvent {
                    kind: EVENT_KIND_VALUE_CHANGED,
                    detail: pack_value_changed(3, true),
                },
            ),
            None
        );
        assert_eq!(relayed(), before + 1, "the gate never asked the component");

        // A slot that attached nothing answers `None` for every kind.
        publish(91, Rc::new(Relay), props("quiet", 5));
        let (_view, mut quiet) = Bridge::<Relay>::create(
            &mut ctx,
            &BridgeProps {
                slot: 91,
                props: props("quiet", 5),
            },
        )
        .expect("a quiet relay builds");
        assert_eq!(Bridge::<Relay>::on_event(&mut quiet, click()), None);
        assert_eq!(
            relayed(),
            before + 1,
            "an unattached slot never reaches the component — the misroute a \
             fabricated slot id used to be able to cause stops at the bridge"
        );

        drop((listening, quiet));
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.starts_with("attachListener "))
                .count(),
            1,
            "only the listening relay attached: {calls:?}"
        );
        assert_eq!(
            live_listener_count(),
            0,
            "the handle is released with the state"
        );
        forget(90);
        forget(91);
    }

    #[test]
    fn an_attached_listener_reaches_the_slot_callback_through_the_runtime() {
        // End to end below the app hook: the platform listener's
        // `runtime.on_event(slot, ..)` → the bridge → the component → its
        // answer handed to the slot's registered callback, the table
        // `crate::api::mount`'s `.on_event` hook registers into.
        use std::sync::Arc;

        assert!(register_component::<Relay>(RELAY_KIND));
        let generation = publish(92, Rc::new(Relay), props(LISTEN, 0));
        let params = component_params(RELAY_KIND, 92, generation, false);
        let fired: Arc<Mutex<Vec<EventPayload>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&fired);

        let mut calls = Vec::new();
        {
            let mut ctx = PlatformCtx::new(&mut calls);
            with_runtime(|runtime| {
                assert!(runtime.set_callback(
                    92,
                    Arc::new(move |payload| recorder.lock().unwrap().push(payload)),
                ));
                runtime.create(&mut ctx, &params).unwrap();
                runtime.on_event(92, click());
                assert_eq!(runtime.dispose_slot(&mut ctx, 92), DisposeOutcome::Disposed);
            })
            .expect("the thread's runtime");
        }
        forget(92);

        assert_eq!(
            *fired.lock().unwrap(),
            vec![EventPayload::Toggled(false)],
            "the component's answer — built from its value-0 props — reached the callback"
        );
        assert_eq!(live_listener_count(), 0);
    }

    #[test]
    fn a_listener_attached_in_update_opens_the_gate_too() {
        // `listening` is the union across create AND update: a component that
        // wires a listener later (a child it only builds on some props) must
        // not be gated out of its own events.
        struct Late;

        impl NativeComponent for Late {
            type Props = GaugeProps;
            type State = (u64, Option<ListenerHandle>);

            fn create(
                &self,
                ctx: &mut ComponentCtx<'_, '_, '_>,
                _props: &Self::Props,
            ) -> Option<(NativeRoot, Self::State)> {
                let identity = next_identity();
                Some((ctx.root(identity)?, (identity, None)))
            }

            fn update(
                &self,
                ctx: &mut ComponentCtx<'_, '_, '_>,
                state: &mut Self::State,
                _old: &Self::Props,
                _new: &Self::Props,
            ) {
                state.1 = ctx.attach_listener(state.0, ListenerKinds::TOGGLED);
            }
        }

        let mut calls = Vec::new();
        let mut ctx = PlatformCtx::new(&mut calls);
        publish(93, Rc::new(Late), props("late", 1));
        let first = BridgeProps {
            slot: 93,
            props: props("late", 1),
        };
        let (_view, mut state) = Bridge::<Late>::create(&mut ctx, &first).unwrap();
        let toggled = WireEvent {
            kind: EVENT_KIND_TOGGLED,
            detail: pack_bool(true),
        };
        assert_eq!(Bridge::<Late>::on_event(&mut state, toggled), None);

        let second = BridgeProps {
            slot: 93,
            props: props("late", 2),
        };
        Bridge::<Late>::update(&mut ctx, &mut state, &first, &second).unwrap();
        assert_eq!(
            Bridge::<Late>::on_event(&mut state, toggled),
            Some(EventPayload::Toggled(true)),
            "the default `on_event` forwards the event unchanged once attached"
        );
        forget(93);
    }

    #[test]
    fn an_empty_listener_request_latches_instead_of_attaching() {
        let mut calls = Vec::new();
        let mut ctx = PlatformCtx::new(&mut calls);
        let mut cx = ComponentCtx::new(&mut ctx, 94);
        assert!(cx.attach_listener(1, ListenerKinds::NONE).is_none());
        assert!(cx.failed(), "an empty request is a wiring bug, reported");
        let (error, attached) = cx.into_parts();
        assert!(
            matches!(error, Some(NativeWidgetError::Params(_))),
            "{error:?}"
        );
        assert!(attached.is_empty());
        assert!(calls.is_empty(), "nothing was recorded as attached");
        assert_eq!(live_listener_count(), 0);
    }

    #[test]
    fn detach_listener_records_the_detach_and_releases_the_handle() {
        let mut calls = Vec::new();
        let mut ctx = PlatformCtx::new(&mut calls);
        let mut cx = ComponentCtx::new(&mut ctx, 95);
        let handle = cx
            .attach_listener(7, ListenerKinds::CLICK | ListenerKinds::TOGGLED)
            .expect("the host arm always attaches");
        assert_eq!(
            handle.kinds(),
            ListenerKinds::CLICK | ListenerKinds::TOGGLED
        );
        assert_eq!(live_listener_count(), 1);
        assert_eq!(cx.detach_listener(handle), Some(()));
        assert_eq!(live_listener_count(), 0);
        drop(cx);
        assert_eq!(
            calls,
            vec![
                "attachListener 7 click|toggled".to_string(),
                "detachListener 7 click|toggled".to_string(),
            ]
        );
    }
}

// Platform-neutral: the listener-kind mask and the public event pair's
// codec, which every arm (macOS included) shares — so, unlike `tests` above,
// these run on a Mac too.
#[cfg(test)]
mod listener_tests {
    use super::*;

    #[test]
    fn listener_kinds_combine_and_spell_themselves() {
        let both = ListenerKinds::CLICK | ListenerKinds::VALUE_CHANGED;
        assert!(both.contains(ListenerKinds::CLICK));
        assert!(both.contains(ListenerKinds::VALUE_CHANGED));
        assert!(!both.contains(ListenerKinds::TOGGLED));
        assert!(both.contains(ListenerKinds::NONE));
        assert!(ListenerKinds::NONE.is_empty());
        assert!(!both.is_empty());
        assert_eq!(both.to_string(), "click|value_changed");
        assert_eq!(ListenerKinds::NONE.to_string(), "none");
        let mut grown = ListenerKinds::NONE;
        grown |= ListenerKinds::TOGGLED;
        assert_eq!(grown, ListenerKinds::TOGGLED);
    }

    #[test]
    fn every_payload_round_trips_through_the_public_pair() {
        // The component answer's ride to the app hook: NativeEvent →
        // EventPayload (the six's callback table) → NativeEvent (the hook's
        // parameter). A lossy leg would hand the app something other than
        // what the component answered.
        for payload in [
            EventPayload::Click,
            EventPayload::Toggled(true),
            EventPayload::Toggled(false),
            EventPayload::ValueChanged {
                value: 42,
                from_user: true,
            },
            EventPayload::ValueChanged {
                value: 0,
                from_user: false,
            },
            EventPayload::DragStart,
            EventPayload::DragEnd,
        ] {
            let event = NativeEvent::from_payload(payload);
            assert_eq!(event.into_payload(), Some(payload), "{payload:?}");
        }
        let unknown = NativeEvent {
            kind: 99,
            detail: 0,
        };
        assert_eq!(unknown.into_payload(), None);
        assert_eq!(unknown.family(), ListenerKinds::NONE);
    }

    #[test]
    fn the_public_decoders_read_only_their_own_kind() {
        let toggled = NativeEvent::from_payload(EventPayload::Toggled(true));
        assert_eq!(toggled.checked(), Some(true));
        assert_eq!(toggled.value(), None);
        assert!(!toggled.is_click());
        assert_eq!(toggled.family(), ListenerKinds::TOGGLED);

        let moved = NativeEvent::from_payload(EventPayload::ValueChanged {
            value: 17,
            from_user: true,
        });
        assert_eq!(moved.value(), Some(17));
        assert_eq!(moved.checked(), None);
        assert_eq!(moved.family(), ListenerKinds::VALUE_CHANGED);
        assert_eq!(
            NativeEvent::from_payload(EventPayload::DragEnd).family(),
            ListenerKinds::VALUE_CHANGED,
            "the drag edges ride the VALUE_CHANGED attach"
        );

        let click = NativeEvent::from_payload(EventPayload::Click);
        assert!(click.is_click());
        assert_eq!(click.kind(), NativeEvent::KIND_CLICK);
        assert_eq!(click.family(), ListenerKinds::CLICK);
    }
}
