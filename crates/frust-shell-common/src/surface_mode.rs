//! HOST-declared translucent-surface slot:
//! [`declare_host_translucent_surface`]/[`SurfaceModeWatcher::current`], plus
//! its outward-facing sibling — the RESOLVED slot
//! ([`publish_resolved_surface_mode`]/[`resolved_surface_mode`]) an app or
//! plugin reads to learn what the platform actually gave
//! us, including a refusal (`translucencyRefused`).
//!
//! # The gap this closes
//!
//! Platform-view compositing (Mode B) needs the GPU
//! surface itself to be created with an alpha channel (Android's `EGLConfig`,
//! iOS's `CAMetalLayer.isOpaque`) so a native sibling view placed *behind* it
//! can show through wherever frust paints nothing. That surface-format choice
//! happens once, at surface-creation time, well before any app code runs — so
//! there is no "widget asks for translucency" moment the way there is for,
//! say, `set_app_theme`. **Only the generated host glue** (Android's
//! `jni_glue::native_set_surface_mode`, iOS's `ffi_glue::set_surface_mode`)
//! calls [`declare_host_translucent_surface`] during startup, from the same
//! branch that already set `SurfaceHolder`'s `PixelFormat.TRANSLUCENT` /
//! `CAMetalLayer.isOpaque = false` on the native window itself — and each
//! shell's surface-creation path reads
//! [`SurfaceModeWatcher::current`] **before** configuring the surface.
//!
//! # Layering choice
//!
//! Same rationale as [`crate::theme_override`]/[`crate::system_ui`]: a
//! process-global `Mutex` slot living in `frust-shell-common`, the crate every
//! shell already polls this kind of state from. Unlike those two, though,
//! this slot is **not** a per-frame generation/poll pair — see the Latch
//! contract below — so [`SurfaceModeWatcher`] carries no per-instance
//! "last seen" cursor; `current` is an associated function, a plain peek at
//! the process-wide slot.
//!
//! # This latch is a HOST DECLARATION, not the outcome
//!
//! Setting this latch is a claim by the host glue that the native window is
//! *already* configured translucent — **calling it from anywhere else, or
//! without that window configuration already in place, is a host-template
//! bug** (the exact failure vector this guards against: an app opting in
//! from Rust alone with no matching host window config). That is why
//! [`declare_host_translucent_surface`]
//! is not re-exported past `frust-shell-common` — see the Callers section.
//!
//! Declaring it doesn't fully decide the outcome either. Each shell
//! translates this latch into a `frust_render::SurfaceAlphaRequest`, and
//! `frust-render` resolves *that* against the platform's advertised
//! `CompositeAlphaMode`s at configure time — falling back to an opaque
//! swapchain (with a `log::warn!`) when the platform advertises no translucent
//! mode, and the GPU-tier blit fallback can further refuse a
//! premultiply-expecting mode it can't reproduce. **The resolved truth lives
//! at a different seam**: `frust_render::SurfaceRenderer::surface_resolved_translucent`,
//! read by each shell after every surface (re)install and threaded into
//! `RenderRoot::set_surface_translucent` — that seam
//! still governs paint.
//!
//! So: read this latch to decide what to *request*; never to decide whether to
//! paint the Mode B contract (a transparent base clear, a `platform_view`
//! hole punch). Keying paint off the declaration alone (ignoring the M1/M2
//! resolved seam) means a fallback clears to `TRANSPARENT` and
//! `DestOut`-punches every slot rect on an OPAQUE swapchain — black
//! rectangles instead of a graceful degrade to Mode A.
//!
//! # Callers
//!
//! [`declare_host_translucent_surface`] is called **only** by
//! `frust-shell-android::jni_glue`'s `native_set_surface_mode` and
//! `frust-shell-ios::ffi_glue`'s `set_surface_mode` — the fixed JNI/C-ABI
//! exports the generated host template's Kotlin/Swift calls in the same
//! branch it sets `PixelFormat.TRANSLUCENT`/`isOpaque = false` and arranges
//! the native-sibling z-order. It is deliberately **not** re-exported from
//! the `frust` facade: an app Rust call with no matching host window config
//! is a reachable black-rectangle vector, so that capability was
//! removed rather than documented around.
//!
//! # Latch contract (one-way, v1)
//!
//! [`declare_host_translucent_surface`] only ever moves the slot from
//! [`SurfaceMode::Opaque`] to [`SurfaceMode::Translucent`] — there is no
//! "undo" call, and once observed as `Translucent` it never reverts. This is
//! deliberate, not an oversight: the surface format is fixed at creation (the
//! platform APIs above expose no supported runtime toggle), so "reverting"
//! would mean destroying and recreating the whole surface — out of scope for
//! v1, and no current use case needs it (an app either wants platform-view
//! compositing for the process's lifetime, or it doesn't). A future version
//! needing a live flip would have to plumb a full surface-recreation
//! round-trip through each shell's `SurfacePhase` state machine
//! (`docs/ARCHITECTURE.md`'s frame pipeline) — not a slot-shape change.
//!
//! One-way applies to the DECLARATION only. The *resolved* state above is not
//! one-way and is not fixed before the surface exists: every (re)install
//! re-resolves it, and a failed install clears it — which is exactly why the
//! shells re-read it per frame rather than caching it at construction.
//!
//! # The RESOLVED slot
//!
//! Everything above is the *inward* half: what the host declared, read by the
//! shells to decide what to request. [`publish_resolved_surface_mode`]/
//! [`resolved_surface_mode`] are the *outward* half — the resolved verdict
//! ([`ResolvedSurfaceMode`]) travelling back out to app/plugin code, whose
//! whole reason to exist is
//! [`ResolvedSurfaceMode::RefusedTranslucent`]: the host declared Mode B and
//! the platform resolved opaque anyway (`docs/LIMITATIONS.md`'s
//! `cam-blit-opaque`). Before this slot existed, that case was invisible *and*
//! unsignalled — a native sibling arranged behind a now-opaque frust surface
//! simply vanished, with no way for app code to fall back deliberately.
//!
//! Publishing is again shell-glue-only (pinned by the same
//! `crates/frust/tests/surface_mode_conformance.rs` scan): each mobile shell
//! publishes whatever
//! `frust_render::SurfaceRenderer::surface_resolved_translucent` reports for
//! the live surface, on the same UI-thread beat it pushes
//! `RenderRoot::set_surface_translucent` — seeded at handle construction,
//! re-published on every later (re)install. Reading is open to anyone; the
//! `frust` facade re-exports [`resolved_surface_mode`] +
//! [`ResolvedSurfaceMode`] (never the publisher).
//!
//! **Polling contract, not a reactive one.** Nothing here wakes a frame: this
//! slot is a plain process-global read, exactly like
//! [`crate::theme_override::theme_override_active`]. App code reads it *during
//! a rebuild* (or during paint/an event handler) and branches on what it finds;
//! since both mobile shells run a continuous per-frame loop and re-publish
//! every frame, a downgrade is observed on the rebuild after the install that
//! caused it. A value read on a frame where nothing else is dirty does **not**
//! by itself schedule another frame — pair a fallback decision with an actual
//! state write (a signal set, a component-state change) if the UI must change
//! shape because of it.
//!
//! # Thread contract
//!
//! Like [`crate::theme_override::set_app_theme`]/
//! [`crate::system_ui::set_system_ui_mode`], [`declare_host_translucent_surface`]
//! is callable from any thread — a plain `Mutex` guards the slot. In
//! practice it must be called before the shell's surface-creation path reads
//! [`SurfaceModeWatcher::current`] (startup-time only — see the module docs
//! above), but that ordering is a caller responsibility, not something this
//! module enforces.
//!
//! [`publish_resolved_surface_mode`]/[`resolved_surface_mode`] have the same
//! shape over their own `Mutex` slot: callable from any thread, no ordering
//! enforced. In practice each shell publishes from its UI thread (the thread
//! that owns the `RenderRoot`) and app code reads from that same thread during
//! a rebuild — the read is a plain lock-load either way, so an off-thread
//! reader sees a consistent value, just possibly one frame old.

use std::sync::Mutex;

/// Whether a shell's GPU surface should be created with an alpha channel.
/// See the module docs' Latch contract — this only ever moves
/// `Opaque` → `Translucent`, never back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SurfaceMode {
    /// Default: an opaque surface, matching every shell's pre-platform-views
    /// behavior.
    #[default]
    Opaque,
    /// Create the surface with an alpha channel so a native sibling view
    /// placed behind it can show through unpainted regions.
    Translucent,
}

/// The process-wide latch. No generation counter (unlike
/// [`crate::theme_override`]/[`crate::system_ui`]'s slots) — see the module
/// docs' Latch contract for why a per-frame "changed since last poll" concept
/// doesn't apply here.
static SURFACE_MODE: Mutex<SurfaceMode> = Mutex::new(SurfaceMode::Opaque);

/// Declare that the native host window has already been configured
/// translucent (`PixelFormat.TRANSLUCENT`/`isOpaque = false`) so this
/// shell's next GPU surface should be created with an alpha channel too.
///
/// **Called only by the generated host glue** (`jni_glue::native_set_surface_mode`
/// on Android, `ffi_glue::set_surface_mode` on iOS), from the same branch
/// that actually configured the window — see the module docs' Callers
/// section. Calling this without that window configuration in place is a
/// host-template bug, not a supported app-Rust opt-in.
///
/// Callable from any thread (see the module docs' Thread contract), and
/// idempotent — calling it more than once, or after the surface already
/// latched translucent, has no additional effect.
///
/// Must be called before the running shell's surface-creation path reads
/// [`SurfaceModeWatcher::current`] (see the module docs) — calling it after
/// the surface already exists has no effect on that surface.
///
/// Declaring translucency does not guarantee the resolved outcome: the
/// platform may refuse (see the module docs' *This latch is a HOST
/// DECLARATION, not the outcome*), in which case the app degrades to the
/// opaque Mode A contract.
pub fn declare_host_translucent_surface() {
    let mut slot = SURFACE_MODE.lock().unwrap_or_else(|e| e.into_inner());
    *slot = SurfaceMode::Translucent;
}

/// Per-shell-instance reader over the process-wide latch. Kept as a type
/// (mirroring [`crate::theme_override::ThemeOverrideWatcher`]/
/// [`crate::system_ui::SystemUiWatcher`]'s shape) even though it carries no
/// state of its own — [`current`](Self::current) is a plain peek, not a
/// diffed poll, per the module docs' Latch contract.
#[derive(Debug, Default)]
pub struct SurfaceModeWatcher;

impl SurfaceModeWatcher {
    /// A fresh (stateless) watcher.
    pub fn new() -> Self {
        Self
    }

    /// Read the latch's current value. Not a "since last call" diff — a
    /// shell's surface-creation path calls this once, at surface-creation
    /// time, and applies whatever it reads.
    pub fn current() -> SurfaceMode {
        *SURFACE_MODE.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// What the platform **actually gave us**, as opposed to what the host
/// declared ([`SurfaceMode`]) — see the module docs' *The RESOLVED slot*.
///
/// Read via [`resolved_surface_mode`] (re-exported by the `frust` facade);
/// published only by the two mobile shells, once per frame, from the same
/// value that drives `RenderRoot::set_surface_translucent`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResolvedSurfaceMode {
    /// No shell has published a resolution yet: no surface exists, or the
    /// running shell doesn't participate (the desktop preview shell never
    /// publishes — it has no Mode B host seam). **Not** a synonym for
    /// `Opaque`: app code that must distinguish "opaque" from "don't know yet"
    /// (e.g. deferring a fallback decision to the next frame) can.
    #[default]
    Unknown,
    /// The live surface came up opaque, and that is what the host asked for —
    /// the ordinary Mode A contract every pre-platform-views app runs under.
    Opaque,
    /// The live surface came up translucent: frust clears to a transparent
    /// base and `platform_view` punches its slot rects (Mode B).
    Translucent,
    /// **The refusal** (`translucencyRefused`): the host declared Mode B, and
    /// the platform resolved the surface opaque anyway — no matching
    /// `CompositeAlphaMode`, or a GPU-tier blit-fallback surface that cannot
    /// premultiply (`docs/LIMITATIONS.md`'s `cam-blit-opaque`).
    ///
    /// frust's own paint side degrades correctly (opaque clear, no hole
    /// punch), but the host's native-sibling z-order was fixed at build time:
    /// a sibling arranged *behind* the now-opaque surface is invisible and
    /// untappable. That is what this variant exists to tell app/plugin code —
    /// so it can render its own fallback content instead of a dead rect.
    RefusedTranslucent,
}

impl ResolvedSurfaceMode {
    /// The pure mapping [`publish_resolved_surface_mode`] applies, split out so
    /// it is testable without touching the process-global slots: a resolved
    /// translucent surface is [`Translucent`](Self::Translucent); a resolved
    /// opaque one is [`RefusedTranslucent`](Self::RefusedTranslucent) when the
    /// host declared translucency and [`Opaque`](Self::Opaque) when it didn't.
    ///
    /// Never returns [`Unknown`](Self::Unknown) — that variant means "nobody
    /// published", which is the slot's initial state, not a resolution.
    pub const fn resolve(declared: SurfaceMode, resolved_translucent: bool) -> Self {
        match (declared, resolved_translucent) {
            (_, true) => Self::Translucent,
            (SurfaceMode::Translucent, false) => Self::RefusedTranslucent,
            (SurfaceMode::Opaque, false) => Self::Opaque,
        }
    }

    /// `true` only for [`RefusedTranslucent`](Self::RefusedTranslucent) — the
    /// one-liner a native-widget fallback branches on
    /// (`if frust::resolved_surface_mode().translucency_refused() { ... }`).
    pub const fn translucency_refused(self) -> bool {
        matches!(self, Self::RefusedTranslucent)
    }

    /// `true` only for [`Translucent`](Self::Translucent), i.e. the frames
    /// being painted right now really do follow the Mode B contract. Both
    /// refusal and plain opacity answer `false`; so does
    /// [`Unknown`](Self::Unknown), since no surface has reported in.
    pub const fn is_translucent(self) -> bool {
        matches!(self, Self::Translucent)
    }
}

/// The process-wide RESOLVED slot, beside [`SURFACE_MODE`]'s declaration
/// latch. Unlike that latch this is **not** one-way: every surface (re)install
/// re-resolves, and a failed install downgrades — see the module docs.
static RESOLVED_SURFACE_MODE: Mutex<ResolvedSurfaceMode> = Mutex::new(ResolvedSurfaceMode::Unknown);

/// Publish the live surface's **resolved** translucency, mapped against the
/// host declaration into a [`ResolvedSurfaceMode`] (see
/// [`ResolvedSurfaceMode::resolve`]).
///
/// **Called only by the two mobile shells' `app.rs`** — the UI-thread beat
/// that already reads the resolved flag and pushes
/// `RenderRoot::set_surface_translucent` — pinned by
/// `crates/frust/tests/surface_mode_conformance.rs` the same way
/// [`declare_host_translucent_surface`] is. `resolved_translucent` is what
/// `frust_render::SurfaceRenderer::surface_resolved_translucent` reports for
/// the surface that is live *now*; a failed install reports `false`, which is
/// the honest answer (nothing to punch a hole in).
///
/// Idempotent and re-callable in any direction: publishing the same value
/// every frame is the expected usage, and a later install may legitimately
/// move the slot back (`Translucent` → `RefusedTranslucent`, or the reverse
/// once a re-created surface comes up capable again).
pub fn publish_resolved_surface_mode(resolved_translucent: bool) {
    let declared = SurfaceModeWatcher::current();
    let mut slot = RESOLVED_SURFACE_MODE
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *slot = ResolvedSurfaceMode::resolve(declared, resolved_translucent);
}

/// Read the resolved surface mode — [`ResolvedSurfaceMode::Unknown`] until a
/// shell publishes one (no surface yet, or a shell with no Mode B seam, e.g.
/// the desktop preview).
///
/// A **poll**, not a subscription: reading this never wakes a frame, and a
/// change here never marks anything dirty on its own. Read it during a rebuild
/// (the same place `theme_override`-style process-global state is read) and
/// branch; see the module docs' *The RESOLVED slot* for the full contract.
pub fn resolved_surface_mode() -> ResolvedSurfaceMode {
    *RESOLVED_SURFACE_MODE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use std::thread;

    // Serializes every test in this module against the shared process-wide
    // `SURFACE_MODE` static — mirrors `theme_override`'s `TEST_LOCK` pattern.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    fn reset_slot() {
        let mut slot = SURFACE_MODE.lock().unwrap_or_else(|e| e.into_inner());
        *slot = SurfaceMode::Opaque;
        let mut resolved = RESOLVED_SURFACE_MODE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *resolved = ResolvedSurfaceMode::Unknown;
    }

    #[test]
    fn defaults_to_opaque() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();
        assert_eq!(SurfaceModeWatcher::current(), SurfaceMode::Opaque);
    }

    /// The default is Opaque
    /// with no host call at all — same assertion as `defaults_to_opaque`
    /// above, spelled out explicitly since it's the important
    /// case (no `declare_host_translucent_surface()` call anywhere in this
    /// test body).
    #[test]
    fn opaque_by_default_with_no_host_declaration() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();
        assert_eq!(SurfaceModeWatcher::current(), SurfaceMode::Opaque);
    }

    #[test]
    fn declaration_latches_translucent() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        declare_host_translucent_surface();
        assert_eq!(SurfaceModeWatcher::current(), SurfaceMode::Translucent);
    }

    #[test]
    fn repeated_declarations_are_idempotent() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        declare_host_translucent_surface();
        declare_host_translucent_surface();
        assert_eq!(SurfaceModeWatcher::current(), SurfaceMode::Translucent);
    }

    #[test]
    fn declaration_from_a_spawned_thread_is_observed() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        thread::spawn(|| {
            declare_host_translucent_surface();
        })
        .join()
        .unwrap();

        assert_eq!(SurfaceModeWatcher::current(), SurfaceMode::Translucent);
    }

    // --- The RESOLVED slot ---------------------------------------------------

    /// Nothing published yet reads `Unknown` — the desktop/host
    /// case, and every mobile launch before the first surface resolution.
    #[test]
    fn resolved_defaults_to_unknown() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        assert_eq!(resolved_surface_mode(), ResolvedSurfaceMode::Unknown);
    }

    /// No host declaration + an opaque resolution is plain `Opaque`, never a
    /// refusal — a Mode A app must not look like it was refused anything.
    #[test]
    fn undeclared_opaque_resolution_is_opaque_not_refused() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        publish_resolved_surface_mode(false);
        assert_eq!(resolved_surface_mode(), ResolvedSurfaceMode::Opaque);
        assert!(!resolved_surface_mode().translucency_refused());
    }

    #[test]
    fn declared_translucent_resolution_is_translucent() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        declare_host_translucent_surface();
        publish_resolved_surface_mode(true);

        assert_eq!(resolved_surface_mode(), ResolvedSurfaceMode::Translucent);
        assert!(resolved_surface_mode().is_translucent());
        assert!(!resolved_surface_mode().translucency_refused());
    }

    /// The whole point of the slot: declared Mode B, resolved opaque
    /// (`docs/LIMITATIONS.md`'s `cam-blit-opaque`).
    #[test]
    fn declared_but_opaque_resolution_is_refused() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        declare_host_translucent_surface();
        publish_resolved_surface_mode(false);

        assert_eq!(
            resolved_surface_mode(),
            ResolvedSurfaceMode::RefusedTranslucent
        );
        assert!(resolved_surface_mode().translucency_refused());
        assert!(!resolved_surface_mode().is_translucent());
    }

    /// Unlike the declaration latch, the resolved slot is **not** one-way: a
    /// later (re)install may downgrade it, and a recovered surface may take it
    /// back up. Both directions must land.
    #[test]
    fn resolved_slot_moves_in_both_directions() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        declare_host_translucent_surface();

        publish_resolved_surface_mode(true);
        assert_eq!(resolved_surface_mode(), ResolvedSurfaceMode::Translucent);

        // A failed/incapable reinstall downgrades …
        publish_resolved_surface_mode(false);
        assert_eq!(
            resolved_surface_mode(),
            ResolvedSurfaceMode::RefusedTranslucent
        );

        // … and a recreated, capable surface takes it back up.
        publish_resolved_surface_mode(true);
        assert_eq!(resolved_surface_mode(), ResolvedSurfaceMode::Translucent);
    }

    /// Re-publishing the same value every frame (the shells' actual usage) is
    /// a no-op beyond the store.
    #[test]
    fn repeated_publishes_are_idempotent() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        publish_resolved_surface_mode(true);
        publish_resolved_surface_mode(true);
        publish_resolved_surface_mode(true);

        assert_eq!(resolved_surface_mode(), ResolvedSurfaceMode::Translucent);
    }

    #[test]
    fn publish_from_a_spawned_thread_is_observed() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();
        declare_host_translucent_surface();

        thread::spawn(|| {
            publish_resolved_surface_mode(false);
        })
        .join()
        .unwrap();

        assert_eq!(
            resolved_surface_mode(),
            ResolvedSurfaceMode::RefusedTranslucent
        );
    }

    /// The pure mapping, exhaustively — no globals touched, so this one needs
    /// no `TEST_LOCK`.
    #[test]
    fn resolve_maps_every_declaration_resolution_pair() {
        assert_eq!(
            ResolvedSurfaceMode::resolve(SurfaceMode::Opaque, false),
            ResolvedSurfaceMode::Opaque
        );
        assert_eq!(
            ResolvedSurfaceMode::resolve(SurfaceMode::Opaque, true),
            ResolvedSurfaceMode::Translucent
        );
        assert_eq!(
            ResolvedSurfaceMode::resolve(SurfaceMode::Translucent, true),
            ResolvedSurfaceMode::Translucent
        );
        assert_eq!(
            ResolvedSurfaceMode::resolve(SurfaceMode::Translucent, false),
            ResolvedSurfaceMode::RefusedTranslucent
        );
    }

    /// `Unknown` is the default and is never produced by a resolution — a
    /// published value always says something definite.
    #[test]
    fn unknown_is_the_default_and_never_a_resolution() {
        assert_eq!(ResolvedSurfaceMode::default(), ResolvedSurfaceMode::Unknown);
        for declared in [SurfaceMode::Opaque, SurfaceMode::Translucent] {
            for resolved in [false, true] {
                assert_ne!(
                    ResolvedSurfaceMode::resolve(declared, resolved),
                    ResolvedSurfaceMode::Unknown
                );
            }
        }
        assert!(!ResolvedSurfaceMode::Unknown.is_translucent());
        assert!(!ResolvedSurfaceMode::Unknown.translucency_refused());
    }
}
