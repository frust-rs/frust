use kurbo::Size;

use crate::insets::WindowInsets;

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
/// root build closure) creates no subscription, so re-providing a changed
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
