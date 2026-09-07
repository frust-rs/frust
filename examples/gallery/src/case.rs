//! [`Case`]: one entry in the shared registry [`crate::cases`] concatenates —
//! a slug, a pure [`frust_core::View`] constructor, and the design/variant it
//! is meant to be rendered under. See the [crate docs](crate) for the
//! registry-wide contract (pure `View`, no reactive runtime).

use kurbo::Size;

/// Which theme brightness a [`Case`] (or a design-system theme lookup) wants
/// — see [`crate::theme::theme`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Light,
    Dark,
}

/// Which design system a [`Case`] is rendered under. `Base` is the
/// framework's own [`frust_theme::Theme::neutral`] baseline — no design
/// system plugin involved; the rest name a `plugins/*` catalog. See
/// [`crate::theme::theme`] for how each maps onto a concrete theme
/// constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Design {
    Base,
    Material,
    Cupertino,
    Glyph,
    Shadcn,
    Beui,
}

/// One registry entry: a slug, display title, the logical viewport/scale/
/// frame-time to record it at, which design/variant pairing it targets, and
/// the pure constructor that builds its [`frust_core::View`] tree.
///
/// `build` returns a type-erased [`frust_core::AnyView`] over `()` state — a
/// `Case` carries no reactive runtime, no owner and no clock (see the
/// [crate docs](crate) and `crates/frust-testing/src/frame.rs`'s
/// `record_view`, the pure-`View` recorder every consumer of this registry
/// records a `Case` through).
#[derive(Clone, Copy)]
pub struct Case {
    /// The website page's file stem (e.g. `button`, `icon-button`,
    /// `animated-opacity`), prefixed for a design-system variant
    /// (`material/button`, `shadcn/button`, `glyph/button`, `beui/button`,
    /// `cupertino/button`). Unique across the whole registry — see
    /// `tests/registry.rs`.
    pub slug: &'static str,
    /// Human-readable case title, for a gallery UI to display.
    pub title: &'static str,
    /// The logical viewport this case is laid out at. [`Case::DEFAULT_SIZE`]
    /// (360x240) unless a case has a reason to differ.
    pub size: Size,
    /// The device-pixel scale the recorded frame is painted at.
    /// [`Case::DEFAULT_SCALE`] (2.0) unless a case has a reason to differ.
    pub scale: f64,
    /// The fixed frame timestamp, in milliseconds, an animating case is
    /// recorded at. [`Case::DEFAULT_TIME_MS`] (0, i.e. rest) unless a case
    /// deliberately pins a later point in an animation.
    pub time_ms: u64,
    /// Which design system (and, transitively via [`crate::theme::theme`],
    /// which theme) this case renders under.
    pub design: Design,
    /// The pure `View` constructor this case records — called fresh on every
    /// rebuild pass, exactly like an app's `app_logic`.
    pub build: fn() -> frust_core::AnyView<()>,
}

impl Case {
    /// The default logical viewport a case is laid out at when it has no
    /// reason to pick a different one: 360x240.
    pub const DEFAULT_SIZE: Size = Size::new(360.0, 240.0);
    /// The default device-pixel scale a case is recorded at: 2.0.
    pub const DEFAULT_SCALE: f64 = 2.0;
    /// The default fixed frame time a case is recorded at: 0ms (rest, no
    /// animation offset).
    pub const DEFAULT_TIME_MS: u64 = 0;
}
