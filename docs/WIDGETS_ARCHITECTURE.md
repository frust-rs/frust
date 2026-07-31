# Frust - WIDGETS Architecture

## Overview

WIDGETS covers `frust-widgets`, the baseline widget set (layout, controls, text, gestures,
navigation, platform-view slots) plus three feature-gated design-system catalogs (Material,
Cupertino, Glyph), built over `frust-core`/`frust-scene`/`frust-text`/`frust-theme` through a
shared authoring toolkit; and its sibling `frust-theme`, a design-token crate bundling
color/type/shape/elevation/motion/glass values per design language into a `Theme` that widgets
recover from context and app code reads reactively.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how WIDGETS relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `frust-widgets::authoring` | Public container/callback toolkit every widget in the crate builds from, instead of touching `frust-core` primitives directly — reachable by app code as `frust::authoring` (the facade's re-export, CORE unit); the toolkit itself is unchanged |
| `frust-widgets` (baseline) | Baseline layout containers and interactive leaf widgets (text, forms, gestures, scrolling) |
| `frust-widgets::material` | Material 3 (+Expressive) widget catalog |
| `frust-widgets::cupertino` | iOS-styled widget catalog painting from `Theme.glass`, degrading to opaque fill when unsupported |
| `frust-widgets::glyph` | Third token-driven catalog shaping glyph runs directly rather than nesting `Text` |
| `frust-widgets::motion` | Implicit-animation and transition-pattern vocabulary |
| `frust-widgets::nav` | Imperative page-stack navigator, declarative router, and shared-element hero transitions |
| `frust-widgets::platform_view` | Native-sibling compositing slot and input-shield wrapper for translucent surfaces |
| `frust-theme` | `Theme` aggregate and its token tables (color, type, shape, elevation, motion, glass) |
| `frust-theme::glyph` | Feature-gated Glyph design-language token module |

## Layer Dependencies

`frust-widgets` depends on `frust-core` (`View`/`Widget` traits, layout, paint/event context,
semantics), `frust-scene` (renderer-agnostic draw commands), `frust-text` (text layout/editing
engine backing `frust-theme`'s typography tokens), `frust-theme` (design tokens bundled into
`Theme`), `peniko`/`kurbo` (shared color/geometry primitives), and the `image` crate (image decode
path). `frust-theme` is `frust-widgets`' primary consumer.

The one reverse edge is `frust-theme` depending on `frust-core`, but only for context-accessor
sugar (reading the active `Theme` from a context handle) — this does not create a cycle back into
`frust-widgets`.

Every container and interactive widget in `frust-widgets` is built through the crate's own public
authoring toolkit rather than touching `frust-core` primitives directly, a boundary enforced by a
conformance test. The three design-system catalogs (Material, Cupertino, Glyph) sit above both the
baseline widget set and the authoring toolkit, each consuming a matching `frust-theme` token
module; an app can disable all three to build its own design system on the same toolkit.

## Data Flow

- Theme delivery (detail; the cross-unit summary lives in [ARCHITECTURE.md](ARCHITECTURE.md)): a
  shell owns the active `Theme`; widgets recover it type-erased from paint/layout context so
  `frust-core` stays theme-agnostic, while app code reads a cloned `Theme` via reactive context.
- Widget resolution precedence: explicit builder value > theme token > unthemed-fallback constant,
  generally re-resolved every paint; `Text`/`TextInput` instead bake the resolved color at layout
  time.
- Container plumbing: every container and interactive widget is built through the shared public
  authoring toolkit rather than touching `frust-core` primitives directly.
- Design-system layering: the three feature-gated catalogs sit above the baseline set and the
  authoring seam, each consuming a matching `frust-theme` token module.
- Glass/Cupertino chrome flow: Cupertino chrome widgets paint from `frust-theme`'s glass recipes
  when supported, degrading to an opaque Material fill otherwise.
- Motion flow: implicit-animation and transition widgets resolve default timing from `Theme.motion`,
  collapsing to a short crossfade under `reduce_motion`; decorative loops request paced frames so
  the mobile frame gate can throttle them. `reduce_motion` is a floor, not an assignment: every
  baseline defaults it `false`, and a mobile shell OR's the OS accessibility preference over whatever
  the active theme authored (`effective = authored || os`, see SHELLS_ARCHITECTURE.md for the
  sensors) — an authored `true` is never un-reduced by an OS report of `false`.
- Navigation flow: `Navigator`/`Router` manage a page stack and declarative routes over the same
  container plumbing; `hero()` morphs a tagged child between pages during transitions.
- Platform-view flow: `platform_view()`/`shield()` publish native-compositing slots and input-shield
  rects each frame for the shell layer to reconcile against native views.

## Key Types

| Type | Purpose |
|------|---------|
| `Theme` / `DesignLanguage` / `ThemeBuilder` | `frust-theme`'s aggregate design-token bundle, tagged by baseline (Material3/Cupertino/Glyph), plus a fluent editor |
| `ColorScheme`, `TypeScale`, `ShapeScale`, `Elevation`, `MotionScheme`, `GlassScale`, `StatusPalette`, `GlyphInk` | Individual token-group types composed into `Theme` |
| authoring module (`build_child`/`rebuild_child`/`teardown_child`/`rebuild_children`, `route_event`) | The sanctioned seam for authoring any widget against `frust-core` |
| `Navigator` / `Router` / `hero()` | Page-stack and declarative routing plus shared-element transitions |
| `ButtonStyle`, `ScrollInfo`, `IconData`/`IconSource`, `ImageSource`/`ImageFit` | Small per-widget config/state types shared across the baseline widget set |
