//! Ports beUI's `shader-background` component.
//!
//! **Source:** `components/motion/shader-background.tsx`, beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01. Upstream
//! is a *dispatcher*, not a shader: it maps 21 variant slugs onto
//! `@paper-design/shaders-react` components, forwards each one's own props, and
//! adds exactly one behaviour of its own — `useReducedMotion` freezes `speed` to
//! `0` for the variants that expose it. Everything a user actually sees is
//! authored inside that third-party WebGL library.
//!
//! There is no `@paper-design/shaders` for frust, so a port has to bring the
//! pixels too: each variant here is a **hand-written WGSL fragment program**
//! reproducing the upstream shader's *semantics* (what the effect is: a mesh of
//! colour wells, a grid of dots, a wave field, a radial ramp), not a
//! transliteration of its GLSL — that source is neither vendored nor readable
//! from here. Colours, layout and motion follow the upstream preview presets
//! (`components/previews/motion/shader-background.preview.tsx`); the exact
//! noise/warp constants are this port's own, chosen to land in the same visual
//! family.
//!
//! # Five ported, sixteen deferred
//!
//! [`ShaderBackgroundVariant`] carries the five variants this task ports —
//! `mesh-gradient`, `dot-grid`, `waves`, `static-radial-gradient` and
//! `static-mesh-gradient`. The other sixteen upstream slugs are listed in
//! [`DEFERRED_VARIANTS`] and are **not** ported: each is a distinct shader
//! (Perlin/simplex fields, Voronoi cells, metaballs, god rays, dithering), and
//! each needs its own hand-written WGSL plus its own GPU verification. They are
//! a documented gap, not a silent one — the catalog page names them and they
//! are listed for the accepted-limitations register.
//!
//! # How a variant reaches the GPU
//!
//! The widget records one [`Command::ShaderQuad`](frust::authoring::scene::Command)
//! per frame through [`PaintScene::draw_shader`], over its own laid-out box. The
//! engine compiles the program once (keyed by [`ShaderProgram::id`]), renders it
//! into a pooled offscreen target, and draws that target over the destination
//! rectangle. Three contracts of that seam shape the code here:
//!
//! - **The fragment source is all we supply.** The engine prepends a fixed
//!   prelude declaring the uniform block and a vertex-buffer-free fullscreen
//!   triangle, so every source below defines `fs_main` only, and reads
//!   `frust_u.resolution` / `frust_u.time`. It declares **no bind group of its
//!   own** — the prelude's `@group(0) @binding(0)` uniform is the only binding
//!   in the module and nothing here samples a texture, so the four-bind-group
//!   WebGL2 ceiling and the engine's single bilinear-clamp sampler are both
//!   respected by construction rather than by budget.
//! - **Output is premultiplied.** `frust_scene::ShaderProgram`'s alpha contract:
//!   a returned `vec4(rgb, a)` must already have `rgb` multiplied by `a`. Every
//!   source returns through its own `frust_premultiply` helper so the rule is
//!   applied in exactly one place per program — and so a translucent variant
//!   ([`ShaderBackgroundVariant::DotGrid`], whose backdrop is transparent by
//!   default) composites over what is behind it instead of over black.
//! - **A program is compiled once, not per frame.** `ShaderProgram::new` mints a
//!   process-unique id and the engine caches a pipeline against it, so this
//!   widget mints one lazily on its first paint and re-mints **only** when the
//!   resolved WGSL actually changes (a theme flip, or a rebuilt view carrying
//!   new colours). See [`ShaderBackgroundWidget::sync_program`].
//!
//! # Colour params are baked, not uniform
//!
//! The engine's uniform block is fixed at `resolution` + `time`; there is no
//! user-parameter channel. Every colour is therefore a WGSL `const` compiled
//! into the source ([`variant_wgsl`], a pure function). That is why the palette
//! is resolved from the theme and compared before a re-mint: changing a colour
//! is a recompile, so it must happen on a change and never on a frame.
//!
//! `speed`, by contrast, is *not* baked — it scales the `time` value the widget
//! hands `draw_shader`, so upstream's `speed` prop is a free runtime knob that
//! costs no recompile.
//!
//! # Frames only while animating
//!
//! Two of the five variants animate ([`ShaderBackgroundVariant::animates`]); the
//! three static ones never reference `frust_u.time` at all and never ask for a
//! frame. An animating one is a perpetual decorative loop, so it asks for a
//! **paced** frame ([`DEFAULT_FRAME_INTERVAL`]) rather than every vsync — the
//! same cadence contract [`crate::components::marquee`] follows. Reduced motion
//! freezes the clock at `t = 0` and stops requesting frames, which is upstream's
//! own `speed: 0` rule expressed in frames rather than in props.
//!
//! # The kill switch
//!
//! `FRUST_ENGINE_NO_SHADER_EFFECTS=1` turns the engine's whole shader-effect
//! path off (a driver that miscompiles a user program, where the alternative is
//! losing the application rather than one effect). A background that then draws
//! *nothing* is a blank hero section, so this widget reads the same switch and
//! degrades to a flat fill of the palette's own backdrop token
//! ([`ShaderPalette::fallback_fill`]) — and to nothing at all when that backdrop
//! is transparent, which is the correct degrade for an overlay pattern.
//!
//! # Degradations against upstream
//!
//! - **Effects are approximations, not ports of the GLSL.** See the header: the
//!   upstream sources are a third-party npm package, so these shaders reproduce
//!   the *idea* of each effect. Side-by-side pixel parity with
//!   `@paper-design/shaders` is not claimed and is not achievable this way.
//! - **No per-variant prop surface.** Upstream forwards each shader's own props
//!   (`distortion`, `swirl`, `softness`, `frequency`, ...). Here the knobs are
//!   the shared ones — [`ShaderBackgroundView::colors`],
//!   [`ShaderBackgroundView::back`], [`ShaderBackgroundView::speed`] — and each
//!   variant's shape constants are fixed at the preset values. Widening that is
//!   another recompile-per-change parameter surface, deliberately not invented
//!   ahead of a caller who needs it.
//! - **Dot metrics are device pixels.** The shader sees its target's texel
//!   resolution, not the window's scale factor, so `dot-grid`'s cell and dot
//!   radius are in device px: the grid is denser (in logical terms) on a HiDPI
//!   display than on a 1x one. Upstream's own grid is in CSS px.
//! - **`static-mesh-gradient` is its own program, not `mesh-gradient` frozen.**
//!   Upstream ships two components; this ports two sources. The static one
//!   contains no `frust_u.time` reference whatsoever, which is what makes
//!   "static" checkable rather than a promise about the value passed in.

use std::sync::OnceLock;
use std::time::Duration;

use frust::authoring::scene::ShaderProgram;
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Rect, View, Widget,
};
use frust::{FrameTime, Theme};
use kurbo::Size;
use peniko::Color;

use crate::style::with_alpha;
use crate::tokens::{BEUI_LIGHT, BeuiTokens};

/// The environment variable that turns the engine's offscreen shader-effect
/// path off — `crates/frust-engine/src/config.rs`'s
/// `shader_effects_disabled`, named here so this catalog reads the *same*
/// switch the renderer does rather than inventing a second one.
///
/// A plugin cannot call the engine (this crate names the facade and nothing
/// else — see the crate charter), so the flag is read here directly, with the
/// engine's own accepted spellings: `1` or `true`, case-insensitively.
pub const SHADER_EFFECTS_KILL_SWITCH: &str = "FRUST_ENGINE_NO_SHADER_EFFECTS";

/// Upstream's preview preset `speed` for the animated variants (`0.4`).
pub const DEFAULT_SPEED: f32 = 0.4;

/// The cadence an animating background asks the frame gate for — a perpetual
/// cosmetic loop, paced rather than run at every vsync. ~60Hz is a *ceiling*
/// the theme's own `cosmetic_loop_rate` may lower further (see
/// `PaintCtx::request_frame_paced_at`).
pub const DEFAULT_FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// Floor on [`ShaderBackgroundView::frame_interval`]: asking for less than this
/// is asking for every vsync, which a decorative background never is.
pub const MIN_FRAME_INTERVAL: Duration = Duration::from_millis(8);

/// Ceiling on [`ShaderBackgroundView::frame_interval`], so a badly configured
/// background still animates visibly rather than stepping once a second.
pub const MAX_FRAME_INTERVAL: Duration = Duration::from_millis(250);

/// The extent an unconstrained axis falls back to. A background fills what it
/// is given; asked to size itself freely (an unbounded max constraint), it
/// takes a tile rather than growing without limit.
pub const DEFAULT_EXTENT: f64 = 320.0;

/// Wrap the shader clock at an hour, dodging `f32` precision drift on a
/// long-running session — the same wrap `examples/shadertoy`'s `ShaderView`
/// applies, for the same reason.
const TIME_WRAP_SECS: f64 = 3600.0;

/// The sixteen upstream variant slugs this port does **not** cover, in
/// `shader-background.tsx`'s own `VARIANT_COMPONENTS` order.
///
/// Each is a distinct `@paper-design/shaders` program needing its own
/// hand-written WGSL and its own GPU verification; the catalog page names them
/// and they are listed for the accepted-limitations register. Together with
/// [`ShaderBackgroundVariant::ALL`] this accounts for all 21 upstream slugs —
/// which the module's own tests assert, so the list cannot silently drift.
pub const DEFERRED_VARIANTS: [&str; 16] = [
    "grain-gradient",
    "dot-orbit",
    "warp",
    "water",
    "voronoi",
    "swirl",
    "smoke-ring",
    "neuro-noise",
    "metaballs",
    "god-rays",
    "spiral",
    "dithering",
    "pulsing-border",
    "color-panels",
    "simplex-noise",
    "perlin-noise",
];

/// Which shader a [`ShaderBackgroundView`] paints — the five upstream slugs this
/// port covers. See [`DEFERRED_VARIANTS`] for the sixteen it does not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ShaderBackgroundVariant {
    /// `mesh-gradient`: four drifting colour wells blended through a swirled,
    /// distorted field. Upstream's own default and the preview's first entry.
    #[default]
    MeshGradient,
    /// `dot-grid`: a regular grid of stroked dots over a (by default
    /// transparent) backdrop. Static — upstream's dispatcher names this one as
    /// its example of a variant with no `speed` prop at all.
    DotGrid,
    /// `waves`: a travelling wave field of stacked bands, front colour over
    /// backdrop.
    Waves,
    /// `static-radial-gradient`: an off-centre radial ramp through three stops
    /// into the backdrop. Static by name and by construction.
    StaticRadialGradient,
    /// `static-mesh-gradient`: the mesh field with fixed wells and no clock.
    /// Static by name and by construction.
    StaticMeshGradient,
}

impl ShaderBackgroundVariant {
    /// Every ported variant, in upstream's own `VARIANT_COMPONENTS` order.
    pub const ALL: [ShaderBackgroundVariant; 5] = [
        ShaderBackgroundVariant::MeshGradient,
        ShaderBackgroundVariant::DotGrid,
        ShaderBackgroundVariant::Waves,
        ShaderBackgroundVariant::StaticRadialGradient,
        ShaderBackgroundVariant::StaticMeshGradient,
    ];

    /// The upstream slug this variant ports — the string `shader-background.tsx`
    /// keys its component map with.
    pub fn slug(self) -> &'static str {
        match self {
            ShaderBackgroundVariant::MeshGradient => "mesh-gradient",
            ShaderBackgroundVariant::DotGrid => "dot-grid",
            ShaderBackgroundVariant::Waves => "waves",
            ShaderBackgroundVariant::StaticRadialGradient => "static-radial-gradient",
            ShaderBackgroundVariant::StaticMeshGradient => "static-mesh-gradient",
        }
    }

    /// The preview's own short label for this variant (`"Mesh"`, `"Grid"`, ...).
    pub fn label(self) -> &'static str {
        match self {
            ShaderBackgroundVariant::MeshGradient => "Mesh",
            ShaderBackgroundVariant::DotGrid => "Grid",
            ShaderBackgroundVariant::Waves => "Waves",
            ShaderBackgroundVariant::StaticRadialGradient => "Radial",
            ShaderBackgroundVariant::StaticMeshGradient => "Static Mesh",
        }
    }

    /// Whether this variant has a clock at all.
    ///
    /// `false` is a property of the *source*, not of a runtime flag: a static
    /// variant's WGSL never mentions `frust_u.time`, so it cannot animate even
    /// if a caller asks it to (and [`ShaderBackgroundView::animate`] is ignored
    /// for one). The three `static-*`/`dot-grid` slugs are exactly upstream's
    /// own "variant does not expose `speed`" set.
    pub fn animates(self) -> bool {
        matches!(
            self,
            ShaderBackgroundVariant::MeshGradient | ShaderBackgroundVariant::Waves
        )
    }

    /// The backdrop alpha this variant resolves from tokens by default.
    ///
    /// `dot-grid` is a *pattern*, not a picture: upstream's grid sits over page
    /// content, so its backdrop here is transparent and the dots composite over
    /// whatever is behind them — the case the engine's premultiplied-alpha
    /// contract exists to serve. Every other variant is a full-bleed background
    /// and resolves an opaque backdrop.
    fn default_back_alpha(self) -> f32 {
        match self {
            ShaderBackgroundVariant::DotGrid => 0.0,
            _ => 1.0,
        }
    }
}

/// The colour parameters one variant is compiled with: four stops plus a
/// backdrop.
///
/// Upstream's variants disagree about how many colours they take (`colors[]`
/// for the mesh ones, `colorFront`/`colorBack` for waves,
/// `colorFill`/`colorStroke`/`colorBack` for the grid), so this is the union
/// rather than a per-variant struct — each variant's WGSL reads the slots it
/// needs, documented on [`variant_wgsl`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShaderPalette {
    /// The four colour stops, in the order a variant reads them.
    pub colors: [Color; 4],
    /// The backdrop. Its **alpha is load-bearing**: a transparent backdrop is
    /// what makes a variant composite over the scene behind it.
    pub back: Color,
}

impl ShaderPalette {
    /// Resolve `variant`'s palette from the live theme — tokens only, with the
    /// unthemed beUI light table as the pre-context fallback every component in
    /// this catalog carries.
    ///
    /// The mapping, one token per slot:
    ///
    /// - stop 0 — `ColorScheme::tertiary`, which beUI's own fold binds to
    ///   `--accent` (the signature cyan);
    /// - stop 1 — [`BeuiTokens::violet`] (`--violet`, a decorative brand hue);
    /// - stop 2 — [`BeuiTokens::neon`] (`--neon`, the other decorative hue);
    /// - stop 3 — `ColorScheme::on_surface`, beUI's `--foreground`, which anchors
    ///   the field's dark end on light themes and its light end on dark ones;
    /// - backdrop — `ColorScheme::surface` (`--background`) at the variant's own
    ///   default alpha ([`ShaderBackgroundVariant::default_back_alpha`]).
    ///
    /// This is the same three brand hues upstream's presets reach for (cyan /
    /// violet / green), sourced from the theme instead of from hex literals, so
    /// a light/dark flip carries the background with it.
    pub fn from_theme(variant: ShaderBackgroundVariant, theme: Option<&Theme>) -> Self {
        let tokens = BeuiTokens::resolve(theme);
        let scheme = theme.map(Theme::scheme);
        let role =
            |pick: fn(&frust::ColorScheme) -> Color, fallback: Color| scheme.map_or(fallback, pick);
        let surface = role(|s| s.surface, BEUI_LIGHT.background);
        Self {
            colors: [
                role(|s| s.tertiary, BEUI_LIGHT.accent),
                tokens.violet,
                tokens.neon,
                role(|s| s.on_surface, BEUI_LIGHT.foreground),
            ],
            back: with_alpha(surface, variant.default_back_alpha()),
        }
    }

    /// The flat colour this palette degrades to when the engine's shader path is
    /// off ([`SHADER_EFFECTS_KILL_SWITCH`]): the backdrop itself.
    ///
    /// `None` when that backdrop is fully transparent — an overlay pattern with
    /// no shader to draw is *nothing*, and painting its dot colour across the
    /// whole box instead would obliterate the content the pattern was meant to
    /// sit over.
    pub fn fallback_fill(&self) -> Option<Color> {
        (self.back.components[3] > 0.0).then_some(self.back)
    }
}

/// Format a colour's RGB as a WGSL `vec3<f32>` literal.
///
/// Components travel verbatim, in the encoded (sRGB) space every other paint in
/// a frame carries — the engine writes a fragment's return value into an
/// `Rgba8Unorm` target unblended, so a colour written this way lands on the same
/// value a `fill_rect` of it would.
fn wgsl_rgb(color: Color) -> String {
    // Sanitised at the sink, not only at the builders: the theme-resolved
    // palette and the public `variant_wgsl` route reach here too, and a
    // non-finite literal is not WGSL.
    let c = finite_color(color).components;
    format!("vec3<f32>({:.6}, {:.6}, {:.6})", c[0], c[1], c[2])
}

/// Format a colour's alpha as a WGSL `f32` literal (sanitised like
/// [`wgsl_rgb`]).
fn wgsl_alpha(color: Color) -> String {
    format!("{:.6}", finite_color(color).components[3])
}

/// The premultiply helper every generated source returns through, so the
/// engine's alpha contract is honoured in exactly one place per program.
const PREMULTIPLY_FN: &str = "\
// The engine samples this target as premultiplied colour (see
// `frust_scene::ShaderProgram`'s alpha contract): rgb is multiplied by a here,
// once, and every `fs_main` below returns through this function.
fn frust_premultiply(rgb: vec3<f32>, a: f32) -> vec4<f32> {
    return vec4<f32>(rgb * a, a);
}
";

/// Build the WGSL **fragment** source for `variant` under `palette` — pure, and
/// the single place a colour becomes a shader constant.
///
/// The result is what [`ShaderProgram::new`] is handed: it defines `fs_main`
/// and nothing else the engine's prelude already provides (no `Uniforms`
/// struct, no `@vertex` stage, no bind group of its own).
///
/// How each variant reads [`ShaderPalette`]:
///
/// - [`MeshGradient`](ShaderBackgroundVariant::MeshGradient) /
///   [`StaticMeshGradient`](ShaderBackgroundVariant::StaticMeshGradient) — all
///   four stops are colour wells; the backdrop supplies the output alpha.
/// - [`DotGrid`](ShaderBackgroundVariant::DotGrid) — stop 0 is the dot fill,
///   stop 1 the dot stroke (upstream's `colorFill`/`colorStroke`); the backdrop
///   is `colorBack`, transparent by default.
/// - [`Waves`](ShaderBackgroundVariant::Waves) — stop 0 is `colorFront`, the
///   backdrop is `colorBack`.
/// - [`StaticRadialGradient`](ShaderBackgroundVariant::StaticRadialGradient) —
///   stops 0..2 are the ramp centre-outwards, the backdrop closes the edge.
pub fn variant_wgsl(variant: ShaderBackgroundVariant, palette: &ShaderPalette) -> String {
    let c0 = wgsl_rgb(palette.colors[0]);
    let c1 = wgsl_rgb(palette.colors[1]);
    let c2 = wgsl_rgb(palette.colors[2]);
    let c3 = wgsl_rgb(palette.colors[3]);
    let back = wgsl_rgb(palette.back);
    let alpha = wgsl_alpha(palette.back);
    match variant {
        ShaderBackgroundVariant::MeshGradient => mesh_gradient_wgsl(&c0, &c1, &c2, &c3, &alpha),
        ShaderBackgroundVariant::StaticMeshGradient => {
            static_mesh_gradient_wgsl(&c0, &c1, &c2, &c3, &alpha)
        }
        ShaderBackgroundVariant::DotGrid => dot_grid_wgsl(&c0, &c1, &back, &alpha),
        ShaderBackgroundVariant::Waves => waves_wgsl(&c0, &back, &alpha),
        ShaderBackgroundVariant::StaticRadialGradient => {
            static_radial_gradient_wgsl(&c0, &c1, &c2, &back, &alpha)
        }
    }
}

/// `mesh-gradient`: four colour wells drifting under a swirl + distortion warp.
///
/// Upstream preset: `distortion: 0.8`, `swirl: 0.3`, `speed: 0.4` — the first
/// two are the shape constants below, the third is applied to the clock by the
/// widget rather than baked in.
fn mesh_gradient_wgsl(c0: &str, c1: &str, c2: &str, c3: &str, alpha: &str) -> String {
    format!(
        "{PREMULTIPLY_FN}
const C0: vec3<f32> = {c0};
const C1: vec3<f32> = {c1};
const C2: vec3<f32> = {c2};
const C3: vec3<f32> = {c3};
const ALPHA: f32 = {alpha};

// Upstream preset props, as shape constants.
const DISTORTION: f32 = 0.8;
const SWIRL: f32 = 0.3;
// How tightly a well falls off; larger is a smaller, harder blob.
const FALLOFF: f32 = 5.5;
const TAU: f32 = 6.2831853;

fn well(p: vec2<f32>, centre: vec2<f32>) -> f32 {{
    let d = p - centre;
    return exp(-FALLOFF * dot(d, d));
}}

@fragment
fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32> {{
    let res = max(frust_u.resolution, vec2<f32>(1.0, 1.0));
    let aspect = res.x / res.y;
    let t = frust_u.time;
    let p = in.position.xy / res;

    // Swirl about the centre, strongest in the middle and unwinding outwards.
    let c = (p - vec2<f32>(0.5, 0.5)) * vec2<f32>(aspect, 1.0);
    let r = length(c);
    let a = atan2(c.y, c.x) + SWIRL * (1.0 - smoothstep(0.0, 0.7, r)) * sin(t * 0.6);
    var q = vec2<f32>(0.5, 0.5) + (vec2<f32>(cos(a), sin(a)) * r) / vec2<f32>(aspect, 1.0);

    // Distortion: a low-frequency ripple on both axes.
    q = q + DISTORTION * 0.08 * vec2<f32>(sin(TAU * q.y + t * 0.7), cos(TAU * q.x - t * 0.5));

    let w0 = well(q, vec2<f32>(0.30 + 0.16 * sin(t * 0.42), 0.28 + 0.14 * cos(t * 0.37)));
    let w1 = well(q, vec2<f32>(0.74 + 0.14 * cos(t * 0.31), 0.30 + 0.16 * sin(t * 0.45)));
    let w2 = well(q, vec2<f32>(0.28 + 0.15 * sin(t * 0.27 + 1.7), 0.76 + 0.13 * cos(t * 0.39)));
    let w3 = well(q, vec2<f32>(0.72 + 0.13 * cos(t * 0.35 + 2.4), 0.74 + 0.15 * sin(t * 0.29)));

    let sum = w0 + w1 + w2 + w3 + 0.0001;
    let rgb = (C0 * w0 + C1 * w1 + C2 * w2 + C3 * w3) / sum;
    return frust_premultiply(rgb, ALPHA);
}}
"
    )
}

/// `static-mesh-gradient`: the same field with fixed wells and **no clock** —
/// the source never mentions `frust_u.time`, which is what makes "static"
/// checkable.
fn static_mesh_gradient_wgsl(c0: &str, c1: &str, c2: &str, c3: &str, alpha: &str) -> String {
    format!(
        "{PREMULTIPLY_FN}
const C0: vec3<f32> = {c0};
const C1: vec3<f32> = {c1};
const C2: vec3<f32> = {c2};
const C3: vec3<f32> = {c3};
const ALPHA: f32 = {alpha};

const DISTORTION: f32 = 0.6;
const FALLOFF: f32 = 5.0;
const TAU: f32 = 6.2831853;

fn well(p: vec2<f32>, centre: vec2<f32>) -> f32 {{
    let d = p - centre;
    return exp(-FALLOFF * dot(d, d));
}}

@fragment
fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32> {{
    let res = max(frust_u.resolution, vec2<f32>(1.0, 1.0));
    let p = in.position.xy / res;
    // One fixed ripple — the frozen counterpart of the animated variant's warp.
    let q = p + DISTORTION * 0.07 * vec2<f32>(sin(TAU * p.y), cos(TAU * p.x));

    let w0 = well(q, vec2<f32>(0.24, 0.22));
    let w1 = well(q, vec2<f32>(0.78, 0.30));
    let w2 = well(q, vec2<f32>(0.30, 0.80));
    let w3 = well(q, vec2<f32>(0.76, 0.74));

    let sum = w0 + w1 + w2 + w3 + 0.0001;
    let rgb = (C0 * w0 + C1 * w1 + C2 * w2 + C3 * w3) / sum;
    return frust_premultiply(rgb, ALPHA);
}}
"
    )
}

/// `dot-grid`: a regular grid of stroked dots over the backdrop.
///
/// The one variant whose alpha varies *per pixel*: with the default transparent
/// backdrop, only the dots are opaque and everything between them composites
/// through to the scene behind.
fn dot_grid_wgsl(fill: &str, stroke: &str, back: &str, alpha: &str) -> String {
    format!(
        "{PREMULTIPLY_FN}
const FILL: vec3<f32> = {fill};
const STROKE: vec3<f32> = {stroke};
const BACK: vec3<f32> = {back};
const ALPHA: f32 = {alpha};

// Device-pixel metrics — the shader sees texels, not logical px (see the
// module docs' degradation note).
const CELL: f32 = 24.0;
const DOT_RADIUS: f32 = 2.5;
const STROKE_WIDTH: f32 = 1.5;
const AA: f32 = 0.75;

@fragment
fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32> {{
    let cell = vec2<f32>(CELL, CELL);
    // Distance, in texels, to the nearest cell centre.
    let g = (fract(in.position.xy / cell) - vec2<f32>(0.5, 0.5)) * cell;
    let d = length(g);

    let fill_cov = 1.0 - smoothstep(DOT_RADIUS - AA, DOT_RADIUS + AA, d);
    let outer = DOT_RADIUS + STROKE_WIDTH;
    let disc_cov = 1.0 - smoothstep(outer - AA, outer + AA, d);
    let ring_cov = clamp(disc_cov - fill_cov, 0.0, 1.0);

    var rgb = BACK;
    var a = ALPHA;
    rgb = mix(rgb, STROKE, ring_cov);
    a = max(a, ring_cov);
    rgb = mix(rgb, FILL, fill_cov);
    a = max(a, fill_cov);
    return frust_premultiply(rgb, a);
}}
"
    )
}

/// `waves`: stacked bands travelling across the box, front colour over backdrop.
///
/// The preview preset gives this variant colours only (`colorFront`,
/// `colorBack`), leaving the library's own defaults for everything else; the
/// band geometry below is this port's.
fn waves_wgsl(front: &str, back: &str, alpha: &str) -> String {
    format!(
        "{PREMULTIPLY_FN}
const FRONT: vec3<f32> = {front};
const BACK: vec3<f32> = {back};
const ALPHA: f32 = {alpha};

const FREQUENCY: f32 = 2.0;
const AMPLITUDE: f32 = 0.06;
// Bands down the box.
const SPACING: f32 = 7.0;
const THICKNESS: f32 = 0.45;
const SOFTNESS: f32 = 0.22;
const TAU: f32 = 6.2831853;

@fragment
fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32> {{
    let res = max(frust_u.resolution, vec2<f32>(1.0, 1.0));
    let p = in.position.xy / res;
    let t = frust_u.time;

    // Two summed sines so the crest line is not a pure sinusoid.
    let phase = p.x * FREQUENCY * TAU + t * 0.9;
    let wave = sin(phase) * AMPLITUDE + sin(phase * 0.5 + 1.7) * AMPLITUDE * 0.5;

    // Repeating bands, measured from each band's own centre.
    let band = abs(fract((p.y + wave) * SPACING) - 0.5) * 2.0;
    let cov = 1.0 - smoothstep(THICKNESS - SOFTNESS, THICKNESS + SOFTNESS, band);

    let rgb = mix(BACK, FRONT, cov);
    let a = max(ALPHA, cov);
    return frust_premultiply(rgb, a);
}}
"
    )
}

/// `static-radial-gradient`: an off-centre ramp through three stops into the
/// backdrop. No clock.
fn static_radial_gradient_wgsl(c0: &str, c1: &str, c2: &str, back: &str, alpha: &str) -> String {
    format!(
        "{PREMULTIPLY_FN}
const C0: vec3<f32> = {c0};
const C1: vec3<f32> = {c1};
const C2: vec3<f32> = {c2};
const BACK: vec3<f32> = {back};
const ALPHA: f32 = {alpha};

// The focal point, slightly above and left of centre.
const FOCUS: vec2<f32> = vec2<f32>(0.42, 0.38);
const REACH: f32 = 1.35;

@fragment
fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32> {{
    let res = max(frust_u.resolution, vec2<f32>(1.0, 1.0));
    let aspect = res.x / res.y;
    let p = (in.position.xy / res - FOCUS) * vec2<f32>(aspect, 1.0);
    let r = clamp(length(p) * REACH, 0.0, 1.0);

    var rgb = mix(C0, C1, smoothstep(0.0, 0.40, r));
    rgb = mix(rgb, C2, smoothstep(0.35, 0.72, r));
    rgb = mix(rgb, BACK, smoothstep(0.68, 1.0, r));
    return frust_premultiply(rgb, ALPHA);
}}
"
    )
}

/// Whether a raw environment value enables the kill switch, using the engine's
/// own accepted spellings (`1`/`true`, case-insensitive) — pure, so the policy
/// is testable without touching the process environment.
fn env_disables_shader_effects(raw: Option<&str>) -> bool {
    raw.is_some_and(|v| v.eq_ignore_ascii_case("1") || v.eq_ignore_ascii_case("true"))
}

/// The engine's own precedence for [`SHADER_EFFECTS_KILL_SWITCH`]: a value in
/// the process environment wins; otherwise the value baked in at compile
/// time counts. Mobile shells generally cannot hand a process an environment
/// variable at run time, so the compile-time half is the one they use — and
/// the widget must fall back on exactly the builds where the engine drops the
/// quad, or it would keep minting programs nobody draws.
fn kill_switch_enabled(compile_time: Option<&str>, runtime: Option<&str>) -> bool {
    match runtime {
        Some(value) => env_disables_shader_effects(Some(value)),
        None => env_disables_shader_effects(compile_time),
    }
}

/// Whether [`SHADER_EFFECTS_KILL_SWITCH`] is set for this process — at run
/// time or at compile time, resolved once, exactly as the engine's own
/// `config::shader_effects_disabled` resolves it.
fn shader_effects_disabled() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| {
        kill_switch_enabled(
            option_env!("FRUST_ENGINE_NO_SHADER_EFFECTS"),
            std::env::var(SHADER_EFFECTS_KILL_SWITCH).ok().as_deref(),
        )
    })
}

/// A colour safe to bake into a shader source: every component finite and in
/// `0..=1`. A non-finite component becomes `0.0` — the same "ignore the
/// nonsense" rule [`ShaderBackgroundView::speed`] applies to its input — so no
/// caller value can put `NaN` or `inf` into a WGSL constant.
fn finite_color(color: Color) -> Color {
    let c = color.components;
    let fix = |v: f32| {
        if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    Color::new([fix(c[0]), fix(c[1]), fix(c[2]), fix(c[3])])
}

/// A declarative beUI shader background. See the [module docs](self).
///
/// # Example
///
/// ```
/// use frust_beui::components::shader_background::{ShaderBackgroundVariant, shader_background};
///
/// let hero = shader_background::<()>(ShaderBackgroundVariant::MeshGradient).speed(0.4);
/// ```
pub struct ShaderBackgroundView<State: 'static> {
    variant: ShaderBackgroundVariant,
    palette: Option<ShaderPalette>,
    colors: Option<[Color; 4]>,
    back: Option<Color>,
    speed: f32,
    animate: Option<bool>,
    frame_interval: Duration,
    _state: std::marker::PhantomData<fn(&mut State)>,
}

/// Create a shader background painting `variant` at upstream's preset speed
/// ([`DEFAULT_SPEED`]) with its colours resolved from the theme.
pub fn shader_background<State: 'static>(
    variant: ShaderBackgroundVariant,
) -> ShaderBackgroundView<State> {
    ShaderBackgroundView {
        variant,
        palette: None,
        colors: None,
        back: None,
        speed: DEFAULT_SPEED,
        animate: None,
        frame_interval: DEFAULT_FRAME_INTERVAL,
        _state: std::marker::PhantomData,
    }
}

impl<State: 'static> ShaderBackgroundView<State> {
    /// Replace the whole resolved palette, tokens included — the escape hatch a
    /// caller reaching for upstream's own hex presets needs.
    pub fn palette(mut self, palette: ShaderPalette) -> Self {
        self.palette = Some(ShaderPalette {
            colors: palette.colors.map(finite_color),
            back: finite_color(palette.back),
        });
        self
    }

    /// Override the four colour stops, keeping the token-resolved backdrop.
    /// Non-finite components are dropped to `0.0` and every component is
    /// clamped to `0..=1` before it can reach a shader constant.
    pub fn colors(mut self, colors: [Color; 4]) -> Self {
        self.colors = Some(colors.map(finite_color));
        self
    }

    /// Override the backdrop — upstream's `colorBack`. Its alpha decides whether
    /// the background is opaque or composites over the scene behind it; the
    /// components are sanitised like [`Self::colors`].
    pub fn back(mut self, back: Color) -> Self {
        self.back = Some(finite_color(back));
        self
    }

    /// Scale the shader clock — upstream's `speed` prop (default
    /// [`DEFAULT_SPEED`]). Costs no recompile: it multiplies the `time` value
    /// handed to the program, never the source.
    ///
    /// A negative speed runs the effect backwards, which is well-defined here;
    /// a non-finite one is ignored (the previous value stands).
    pub fn speed(mut self, speed: f32) -> Self {
        if speed.is_finite() {
            self.speed = speed;
        }
        self
    }

    /// Freeze (or unfreeze) the clock — the *static mode*. Ignored for a variant
    /// that has no clock at all ([`ShaderBackgroundVariant::animates`]).
    pub fn animate(mut self, animate: bool) -> Self {
        self.animate = Some(animate);
        self
    }

    /// Set the paced cadence an animating background asks for, clamped into
    /// [`MIN_FRAME_INTERVAL`]..=[`MAX_FRAME_INTERVAL`] (default
    /// [`DEFAULT_FRAME_INTERVAL`]).
    pub fn frame_interval(mut self, interval: Duration) -> Self {
        self.frame_interval = interval.clamp(MIN_FRAME_INTERVAL, MAX_FRAME_INTERVAL);
        self
    }
}

/// The retained widget for a [`ShaderBackgroundView`]. See the
/// [module docs](self).
pub struct ShaderBackgroundWidget {
    variant: ShaderBackgroundVariant,
    palette: Option<ShaderPalette>,
    colors: Option<[Color; 4]>,
    back: Option<Color>,
    speed: f32,
    animate: Option<bool>,
    frame_interval: Duration,
    /// The compiled-once program and the palette its source was generated from
    /// — `None` until the first paint resolves a theme. Re-minted only when
    /// that palette changes (see [`Self::sync_program`]).
    program: Option<(ShaderPalette, ShaderProgram)>,
    /// The frame this widget first painted an animating variant at; the shader
    /// clock is always relative to it, never an absolute shell time.
    started: Option<FrameTime>,
    /// Whether the engine's shader path is off for this process, latched at
    /// build so a paint never touches the environment.
    effects_disabled: bool,
}

impl ShaderBackgroundWidget {
    /// Which shader this background paints.
    pub fn variant(&self) -> ShaderBackgroundVariant {
        self.variant
    }

    /// The compiled program, once a first paint has minted one.
    pub fn program(&self) -> Option<&ShaderProgram> {
        self.program.as_ref().map(|(_, program)| program)
    }

    /// The palette the live program was compiled with.
    pub fn compiled_palette(&self) -> Option<ShaderPalette> {
        self.program.as_ref().map(|(palette, _)| *palette)
    }

    /// Whether this widget will drive frames: the variant has a clock, the
    /// caller has not frozen it, and reduced motion is not in force.
    fn animating(&self, reduce: bool) -> bool {
        self.variant.animates() && self.animate.unwrap_or(true) && !reduce
    }

    /// The palette this widget paints under: an explicit
    /// [`ShaderBackgroundView::palette`] wins outright, else the token-resolved
    /// one with any explicit stops/backdrop layered over it.
    ///
    /// Resolved on the *widget* rather than on the view because a theme is only
    /// reachable from a paint (or layout) context — `View::build` has none, so a
    /// build-time resolution would pin the pre-context fallback table forever.
    fn resolve_palette(&self, theme: Option<&Theme>) -> ShaderPalette {
        if let Some(palette) = self.palette {
            return palette;
        }
        let mut palette = ShaderPalette::from_theme(self.variant, theme);
        if let Some(colors) = self.colors {
            palette.colors = colors;
        }
        if let Some(back) = self.back {
            palette.back = back;
        }
        palette
    }

    /// Mint the program for `palette` if there isn't already one compiled for
    /// exactly it, and answer the live handle.
    ///
    /// This is the cache-once contract in code: `ShaderProgram::new` runs on the
    /// first paint and then only when the resolved palette genuinely differs (a
    /// theme flip, a rebuilt view with new colours) — never per frame, which
    /// would be a pipeline-cache miss and a full recompile every frame.
    fn sync_program(&mut self, palette: ShaderPalette) -> &ShaderProgram {
        let stale = self
            .program
            .as_ref()
            .is_none_or(|(compiled, _)| *compiled != palette);
        if stale {
            let source = variant_wgsl(self.variant, &palette);
            self.program = Some((palette, ShaderProgram::new(source)));
        }
        // Just ensured above.
        &self.program.as_ref().expect("program minted above").1
    }

    /// The shader clock for this frame, in seconds since the widget's first
    /// animating frame, scaled by `speed` and wrapped at [`TIME_WRAP_SECS`].
    fn clock(&mut self, now: FrameTime) -> f32 {
        let started = *self.started.get_or_insert(now);
        let elapsed = now.saturating_sub(started).as_secs_f64() % TIME_WRAP_SECS;
        (elapsed * self.speed as f64) as f32
    }
}

impl<State: 'static> View<State> for ShaderBackgroundView<State> {
    type Element = ShaderBackgroundWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ShaderBackgroundWidget {
        ShaderBackgroundWidget {
            variant: self.variant,
            palette: self.palette,
            colors: self.colors,
            back: self.back,
            speed: self.speed,
            animate: self.animate,
            frame_interval: self.frame_interval,
            program: None,
            started: None,
            effects_disabled: shader_effects_disabled(),
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut ShaderBackgroundWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if element.variant != self.variant {
            element.variant = self.variant;
            // A different shader is a different clock, and a different program:
            // dropping the compiled one is what forces the re-mint.
            element.program = None;
            element.started = None;
            flags |= ChangeFlags::PAINT;
        }
        if element.palette != self.palette || element.colors != self.colors {
            element.palette = self.palette;
            element.colors = self.colors;
            flags |= ChangeFlags::PAINT;
        }
        if element.back != self.back {
            element.back = self.back;
            flags |= ChangeFlags::PAINT;
        }
        if element.speed != self.speed {
            element.speed = self.speed;
            flags |= ChangeFlags::PAINT;
        }
        if element.animate != self.animate {
            element.animate = self.animate;
            flags |= ChangeFlags::PAINT;
        }
        if element.frame_interval != self.frame_interval {
            element.frame_interval = self.frame_interval;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

impl Widget for ShaderBackgroundWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // A background fills what it is given; an unbounded axis takes a tile
        // rather than growing without limit.
        let room = bc.max();
        let axis = |extent: f64| {
            if extent.is_finite() {
                extent
            } else {
                DEFAULT_EXTENT
            }
        };
        bc.constrain(Size::new(axis(room.width), axis(room.height)))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let palette = self.resolve_palette(theme);
        let origin = ctx.origin();
        let size = ctx.size();

        if self.effects_disabled {
            // The engine compiles nothing and draws nothing for a shader quad
            // under the kill switch, so a background would simply vanish. Degrade
            // to the palette's own backdrop — and to nothing when that backdrop
            // is transparent (see `ShaderPalette::fallback_fill`).
            if let Some(fill) = palette.fallback_fill() {
                scene.fill_rect(origin, size, fill);
            }
            return;
        }

        let animating = self.animating(reduce);
        // Reduced motion (and an explicitly frozen background) renders the
        // program at t = 0 — upstream's own `speed: 0` rule, expressed as a
        // clock rather than as a prop.
        let time = if animating {
            self.clock(ctx.frame_time())
        } else {
            0.0
        };
        let dest = Rect::new(
            origin.x,
            origin.y,
            origin.x + size.width,
            origin.y + size.height,
        );
        let program = self.sync_program(palette).clone();
        scene.draw_shader(&program, dest, time);

        if animating {
            // A perpetual decorative loop, asked for at its own cadence rather
            // than at every vsync — `request_frame_paced_at` already carries
            // `TickClass::CosmeticLoop`.
            ctx.request_frame_paced_at(self.frame_interval);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::any::Any;

    use frust::authoring::scene::{Command, Scene, SceneBuilder};
    use frust::authoring::text::TextContext;
    use kurbo::Point;

    /// The box every paint test lays a background into.
    const BOX: Size = Size::new(120.0, 80.0);

    /// Build and lay out a view at [`BOX`].
    fn laid_out(view: &ShaderBackgroundView<()>) -> ShaderBackgroundWidget {
        let mut next_id = 0u64;
        let mut widget = View::<()>::build(view, &mut BuildCtx::new(&mut next_id));
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut layout, &BoxConstraints::tight(BOX));
        widget
    }

    /// Paint into a real `SceneBuilder`, answering the recorded display list
    /// plus the frame request the pass made.
    fn painted(
        widget: &mut ShaderBackgroundWidget,
        ms: u64,
        theme: Option<&Theme>,
    ) -> (Scene, bool, Option<Duration>) {
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, BOX, FrameTime::from_nanos(ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            widget.paint(&mut ctx, &mut builder);
        }
        (scene, ctx.needs_frame(), ctx.paced_interval())
    }

    /// The one shader quad a paint recorded: its program id, destination and
    /// clock.
    fn shader_quad(scene: &Scene) -> (u64, Rect, f32) {
        let quads: Vec<_> = scene
            .commands()
            .iter()
            .filter_map(|command| match command {
                Command::ShaderQuad {
                    program,
                    dest,
                    time,
                    ..
                } => Some((program.id(), *dest, *time)),
                _ => None,
            })
            .collect();
        assert_eq!(quads.len(), 1, "expected exactly one shader quad");
        quads[0]
    }

    fn reduced() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    fn palette_of(variant: ShaderBackgroundVariant) -> ShaderPalette {
        ShaderPalette::from_theme(variant, Some(&crate::theme()))
    }

    /// Every variant's source upholds the engine's fragment-program contract
    /// as far as a string check can tell: the prelude's entry point and
    /// uniforms are used, nothing the prelude declares is redeclared, and the
    /// braces balance. No in-repo gate compiles these five programs — this
    /// crate has no GPU dev-dependency — so their device-side validation is
    /// the out-of-tree T400 render run recorded with the port (and owed to
    /// the accepted-limitations register until an ignored GPU test can land).
    #[test]
    fn every_variant_source_upholds_the_engine_fragment_contract() {
        for variant in ShaderBackgroundVariant::ALL {
            let source = variant_wgsl(variant, &palette_of(variant));
            let slug = variant.slug();
            assert!(
                source
                    .contains("@fragment\nfn fs_main(in: FrustVsOut) -> @location(0) vec4<f32> {"),
                "{slug}: missing the engine's fragment entry point"
            );
            assert_eq!(
                source.matches("fn fs_main").count(),
                1,
                "{slug}: exactly one entry point"
            );
            // The prelude owns all three of these; redeclaring any is a
            // duplicate-definition compile error.
            assert!(
                !source.contains("struct Uniforms"),
                "{slug}: redeclares the prelude's uniform block"
            );
            assert!(
                !source.contains("struct FrustVsOut"),
                "{slug}: redeclares the prelude's varyings"
            );
            assert!(
                !source.contains("@vertex"),
                "{slug}: redeclares the prelude's vertex stage"
            );
            assert_eq!(
                source.matches('{').count(),
                source.matches('}').count(),
                "{slug}: unbalanced braces"
            );
            assert_eq!(
                source.matches('(').count(),
                source.matches(')').count(),
                "{slug}: unbalanced parens"
            );
        }
    }

    /// No variant declares a binding, a texture or a sampler of its own: the
    /// prelude's `@group(0) @binding(0)` uniform is the whole bind-group
    /// surface, so the four-bind-group WebGL2 ceiling and the engine's single
    /// bilinear-clamp sampler are respected by construction.
    #[test]
    fn no_variant_declares_a_bind_group_of_its_own() {
        for variant in ShaderBackgroundVariant::ALL {
            let source = variant_wgsl(variant, &palette_of(variant));
            let slug = variant.slug();
            assert!(!source.contains("@group("), "{slug}: declares a bind group");
            assert!(!source.contains("@binding("), "{slug}: declares a binding");
            assert!(!source.contains("texture_"), "{slug}: declares a texture");
            assert!(!source.contains("sampler"), "{slug}: declares a sampler");
        }
    }

    /// Alpha reaches the output through the premultiply helper and nowhere else
    /// — the engine's premultiplied contract, enforced in one place per program.
    #[test]
    fn every_variant_returns_premultiplied_colour() {
        for variant in ShaderBackgroundVariant::ALL {
            let source = variant_wgsl(variant, &palette_of(variant));
            let slug = variant.slug();
            assert!(
                source.contains("fn frust_premultiply(rgb: vec3<f32>, a: f32) -> vec4<f32> {"),
                "{slug}: no premultiply helper"
            );
            assert!(
                source.contains("return frust_premultiply("),
                "{slug}: does not return through the helper"
            );
            // The helper itself is the only place a vec4 is constructed.
            assert_eq!(
                source.matches("vec4<f32>(rgb * a, a)").count(),
                1,
                "{slug}: premultiplication happens more than once"
            );
        }
    }

    /// A static variant has no clock in its *source*, so it cannot animate even
    /// if a caller asks; an animating one reads the uniform.
    #[test]
    fn only_animating_variants_reference_the_clock() {
        for variant in ShaderBackgroundVariant::ALL {
            let source = variant_wgsl(variant, &palette_of(variant));
            assert_eq!(
                source.contains("frust_u.time"),
                variant.animates(),
                "{}: clock reference disagrees with `animates()`",
                variant.slug()
            );
        }
    }

    /// The palette's colours are baked into the source — the only channel there
    /// is, since the engine's uniform block carries resolution and time alone.
    #[test]
    fn colours_are_baked_into_the_source_as_constants() {
        let mut palette = palette_of(ShaderBackgroundVariant::MeshGradient);
        palette.colors[0] = Color::from_rgb8(0xFF, 0x00, 0x80);
        let source = variant_wgsl(ShaderBackgroundVariant::MeshGradient, &palette);
        let c = palette.colors[0].components;
        assert!(
            source.contains(&format!(
                "const C0: vec3<f32> = vec3<f32>({:.6}, {:.6}, {:.6});",
                c[0], c[1], c[2]
            )),
            "the first stop is not compiled in:\n{source}"
        );
        // A different colour is a different source — which is exactly why the
        // widget re-mints its program on a palette change.
        let mut other = palette;
        other.colors[0] = Color::from_rgb8(0x00, 0xFF, 0x80);
        assert_ne!(
            source,
            variant_wgsl(ShaderBackgroundVariant::MeshGradient, &other)
        );
    }

    /// The port's coverage is exact: five ported plus sixteen deferred is
    /// upstream's whole 21-slug map, with no slug named twice.
    #[test]
    fn ported_and_deferred_variants_account_for_every_upstream_slug() {
        let mut slugs: Vec<&str> = ShaderBackgroundVariant::ALL
            .iter()
            .map(|v| v.slug())
            .chain(DEFERRED_VARIANTS)
            .collect();
        assert_eq!(slugs.len(), 21, "upstream ships 21 variants");
        slugs.sort_unstable();
        slugs.dedup();
        assert_eq!(slugs.len(), 21, "a slug is listed twice");
    }

    /// The paint records one `Command::ShaderQuad` over the widget's own box,
    /// carrying the program the widget compiled.
    #[test]
    fn painting_records_one_shader_quad_over_the_laid_out_box() {
        let theme = crate::theme();
        let mut widget = laid_out(&shader_background(ShaderBackgroundVariant::MeshGradient));
        let (scene, _, _) = painted(&mut widget, 0, Some(&theme));
        let (id, dest, _) = shader_quad(&scene);

        assert_eq!(dest, Rect::new(0.0, 0.0, BOX.width, BOX.height));
        assert_eq!(
            Some(id),
            widget.program().map(ShaderProgram::id),
            "the quad names the widget's own compiled program"
        );
        let program = widget.program().expect("a program was minted").clone();
        assert!(program.source().contains("fn fs_main"));
    }

    /// The program is minted once and reused: a second frame draws the same id,
    /// because a fresh one every frame is a permanent pipeline-cache miss.
    #[test]
    fn the_program_is_minted_once_and_reused_across_frames() {
        let theme = crate::theme();
        let mut widget = laid_out(&shader_background(ShaderBackgroundVariant::MeshGradient));
        let (first, _, _) = painted(&mut widget, 0, Some(&theme));
        let (second, _, _) = painted(&mut widget, 16, Some(&theme));
        let (third, _, _) = painted(&mut widget, 32, Some(&theme));
        let id = shader_quad(&first).0;
        assert_eq!(shader_quad(&second).0, id);
        assert_eq!(shader_quad(&third).0, id);
    }

    /// A palette change *does* re-mint — a recompile is the cost of a colour
    /// change, and skipping it would paint the old colours forever.
    #[test]
    fn a_palette_change_mints_a_new_program() {
        let theme = crate::theme();
        let mut widget = laid_out(&shader_background(ShaderBackgroundVariant::MeshGradient));
        let (first, _, _) = painted(&mut widget, 0, Some(&theme));
        let before = shader_quad(&first).0;

        let mut recoloured = palette_of(ShaderBackgroundVariant::MeshGradient);
        recoloured.colors[1] = Color::from_rgb8(0x12, 0x34, 0x56);
        widget.palette = Some(recoloured);

        let (second, _, _) = painted(&mut widget, 16, Some(&theme));
        assert_ne!(shader_quad(&second).0, before);
        assert_eq!(widget.compiled_palette(), Some(recoloured));
    }

    /// An animating variant advances its own clock from its first frame,
    /// scaled by `speed`, and asks for paced frames while it runs.
    #[test]
    fn an_animating_variant_advances_its_clock_and_paces_its_frames() {
        let theme = crate::theme();
        let mut widget =
            laid_out(&shader_background(ShaderBackgroundVariant::MeshGradient).speed(2.0));

        let (first, needs, interval) = painted(&mut widget, 1_000, Some(&theme));
        assert_eq!(shader_quad(&first).2, 0.0, "the first frame is the origin");
        assert!(needs, "an animating background asks for another frame");
        assert_eq!(interval, Some(DEFAULT_FRAME_INTERVAL));

        let (second, _, _) = painted(&mut widget, 1_500, Some(&theme));
        assert!(
            (shader_quad(&second).2 - 1.0).abs() < 1e-5,
            "half a second at speed 2.0 is t = 1.0, got {}",
            shader_quad(&second).2
        );
    }

    /// A static variant never moves and never drives a frame — the whole point
    /// of the class.
    #[test]
    fn static_variants_never_move_and_never_ask_for_a_frame() {
        let theme = crate::theme();
        for variant in ShaderBackgroundVariant::ALL
            .into_iter()
            .filter(|v| !v.animates())
        {
            let mut widget = laid_out(&shader_background(variant));
            let (first, needs, interval) = painted(&mut widget, 0, Some(&theme));
            let (second, _, _) = painted(&mut widget, 5_000, Some(&theme));
            assert_eq!(shader_quad(&first).2, 0.0, "{}", variant.slug());
            assert_eq!(shader_quad(&second).2, 0.0, "{}", variant.slug());
            assert!(!needs, "{} asked for a frame", variant.slug());
            assert_eq!(interval, None, "{}", variant.slug());
        }
    }

    /// `.animate(false)` freezes an animating variant into the same static
    /// contract: t = 0, no frames, quad still drawn.
    #[test]
    fn animate_false_freezes_an_animating_variant() {
        let theme = crate::theme();
        let mut widget =
            laid_out(&shader_background(ShaderBackgroundVariant::Waves).animate(false));
        let (first, needs, _) = painted(&mut widget, 0, Some(&theme));
        let (second, _, _) = painted(&mut widget, 4_000, Some(&theme));
        assert_eq!(shader_quad(&first).2, 0.0);
        assert_eq!(shader_quad(&second).2, 0.0);
        assert!(!needs);
    }

    /// Reduced motion freezes the clock and stops the frame requests — the
    /// frames-side spelling of upstream's `speed: 0`.
    #[test]
    fn reduced_motion_freezes_the_clock_and_stops_requesting_frames() {
        let theme = reduced();
        let mut widget = laid_out(&shader_background(ShaderBackgroundVariant::MeshGradient));
        let (first, needs, interval) = painted(&mut widget, 0, Some(&theme));
        let (second, _, _) = painted(&mut widget, 3_000, Some(&theme));
        assert_eq!(shader_quad(&first).2, 0.0);
        assert_eq!(shader_quad(&second).2, 0.0, "still frozen three seconds on");
        assert!(!needs, "a frozen background drives no frames");
        assert_eq!(interval, None);
    }

    /// The paced cadence is the caller's, clamped into the legal band.
    #[test]
    fn the_paced_cadence_is_the_callers_clamped_into_the_band() {
        let theme = crate::theme();
        let mut widget = laid_out(
            &shader_background(ShaderBackgroundVariant::MeshGradient)
                .frame_interval(Duration::from_millis(40)),
        );
        let (_, _, interval) = painted(&mut widget, 0, Some(&theme));
        assert_eq!(interval, Some(Duration::from_millis(40)));

        let floored = shader_background::<()>(ShaderBackgroundVariant::MeshGradient)
            .frame_interval(Duration::from_millis(1));
        let mut widget = laid_out(&floored);
        let (_, _, interval) = painted(&mut widget, 0, Some(&theme));
        assert_eq!(interval, Some(MIN_FRAME_INTERVAL), "floored, not honoured");

        let capped = shader_background::<()>(ShaderBackgroundVariant::MeshGradient)
            .frame_interval(Duration::from_secs(4));
        let mut widget = laid_out(&capped);
        let (_, _, interval) = painted(&mut widget, 0, Some(&theme));
        assert_eq!(interval, Some(MAX_FRAME_INTERVAL), "capped, not honoured");
    }

    /// Under the kill switch nothing is drawn through the shader seam at all:
    /// the background degrades to the palette's own backdrop token.
    #[test]
    fn the_kill_switch_degrades_to_the_tokens_fallback_fill() {
        let theme = crate::theme();
        let mut widget = laid_out(&shader_background(ShaderBackgroundVariant::MeshGradient));
        widget.effects_disabled = true;

        let (scene, needs, interval) = painted(&mut widget, 0, Some(&theme));
        assert!(
            !scene
                .commands()
                .iter()
                .any(|c| matches!(c, Command::ShaderQuad { .. })),
            "a disabled path must not record a shader quad"
        );
        assert!(
            widget.program().is_none(),
            "nothing is compiled while the path is off"
        );
        assert!(!needs, "a flat fill drives no frames");
        assert_eq!(interval, None);

        let expected =
            ShaderPalette::from_theme(ShaderBackgroundVariant::MeshGradient, Some(&theme))
                .fallback_fill()
                .expect("an opaque backdrop");
        let fills: Vec<_> = scene
            .commands()
            .iter()
            .filter_map(|command| match command {
                Command::FillRect { rect, brush, .. } => Some((*rect, brush.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(fills.len(), 1, "exactly one fallback fill");
        assert_eq!(fills[0].0, Rect::new(0.0, 0.0, BOX.width, BOX.height));
        assert_eq!(fills[0].1, peniko::Brush::Solid(expected));
    }

    /// A translucent-backdrop variant under the kill switch paints *nothing* —
    /// an overlay pattern with no shader is absence, not a wall of dot colour
    /// over the content it was meant to sit on.
    #[test]
    fn the_kill_switch_paints_nothing_for_a_transparent_backdrop() {
        let theme = crate::theme();
        let mut widget = laid_out(&shader_background(ShaderBackgroundVariant::DotGrid));
        widget.effects_disabled = true;
        let (scene, _, _) = painted(&mut widget, 0, Some(&theme));
        assert!(
            scene.commands().is_empty(),
            "expected an empty display list, got {:?}",
            scene.commands()
        );
    }

    /// The kill switch is spelled exactly as the engine spells it, and parsed
    /// with the engine's own accepted values.
    #[test]
    fn the_kill_switch_matches_the_engines_own_flag() {
        assert_eq!(SHADER_EFFECTS_KILL_SWITCH, "FRUST_ENGINE_NO_SHADER_EFFECTS");
        assert!(env_disables_shader_effects(Some("1")));
        assert!(env_disables_shader_effects(Some("true")));
        assert!(env_disables_shader_effects(Some("TRUE")));
        assert!(!env_disables_shader_effects(Some("0")));
        assert!(!env_disables_shader_effects(Some("no")));
        assert!(!env_disables_shader_effects(Some("")));
        assert!(!env_disables_shader_effects(None));
    }

    /// `dot-grid` resolves a transparent backdrop from tokens and every other
    /// variant an opaque one — the alpha the engine's premultiplied contract is
    /// what makes usable.
    #[test]
    fn only_the_grid_resolves_a_translucent_backdrop() {
        let theme = crate::theme();
        for variant in ShaderBackgroundVariant::ALL {
            let palette = ShaderPalette::from_theme(variant, Some(&theme));
            let alpha = palette.back.components[3];
            if variant == ShaderBackgroundVariant::DotGrid {
                assert_eq!(alpha, 0.0, "{}", variant.slug());
                assert_eq!(palette.fallback_fill(), None, "{}", variant.slug());
            } else {
                assert_eq!(alpha, 1.0, "{}", variant.slug());
                assert!(palette.fallback_fill().is_some(), "{}", variant.slug());
            }
            // The baked ALPHA constant is the backdrop's own alpha.
            let source = variant_wgsl(variant, &palette);
            assert!(
                source.contains(&format!("const ALPHA: f32 = {alpha:.6};")),
                "{}: backdrop alpha is not compiled in",
                variant.slug()
            );
        }
    }

    /// Colours come from the theme, so a light/dark flip carries the background
    /// with it rather than pinning a hex literal.
    #[test]
    fn the_palette_follows_the_theme() {
        let light = crate::theme();
        let dark = crate::theme().with_brightness(frust::Brightness::Dark);
        let variant = ShaderBackgroundVariant::MeshGradient;
        let from_light = ShaderPalette::from_theme(variant, Some(&light));
        let from_dark = ShaderPalette::from_theme(variant, Some(&dark));
        assert_ne!(
            from_light, from_dark,
            "the two brightnesses must not resolve the same palette"
        );
        assert_eq!(from_light.colors[0], light.scheme().tertiary);
        assert_eq!(from_light.back, with_alpha(light.scheme().surface, 1.0));

        // Pre-context (no theme yet) falls back to the vendored light table.
        let unthemed = ShaderPalette::from_theme(variant, None);
        assert_eq!(unthemed.colors[0], BEUI_LIGHT.accent);
        assert_eq!(unthemed.colors[3], BEUI_LIGHT.foreground);
    }

    /// An explicit palette, stops or backdrop win over the token-resolved ones.
    #[test]
    fn explicit_colours_override_the_token_defaults() {
        let theme = crate::theme();
        let stops = [
            Color::from_rgb8(0x11, 0x22, 0x33),
            Color::from_rgb8(0x44, 0x55, 0x66),
            Color::from_rgb8(0x77, 0x88, 0x99),
            Color::from_rgb8(0xAA, 0xBB, 0xCC),
        ];
        let back = with_alpha(Color::from_rgb8(0x00, 0x00, 0x00), 0.5);
        let mut widget = laid_out(
            &shader_background(ShaderBackgroundVariant::MeshGradient)
                .colors(stops)
                .back(back),
        );
        painted(&mut widget, 0, Some(&theme));
        let compiled = widget.compiled_palette().expect("a compiled palette");
        assert_eq!(compiled.colors, stops);
        assert_eq!(compiled.back, back);
    }

    /// A background fills its box, and takes a tile on an unbounded axis rather
    /// than growing without limit.
    #[test]
    fn layout_fills_the_box_and_tiles_an_unbounded_axis() {
        let view = shader_background::<()>(ShaderBackgroundVariant::Waves);
        let mut next_id = 0u64;
        let mut widget = View::<()>::build(&view, &mut BuildCtx::new(&mut next_id));
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);

        let filled = widget.layout(&mut layout, &BoxConstraints::loose(BOX));
        assert_eq!(filled, BOX);

        let unbounded = widget.layout(
            &mut layout,
            &BoxConstraints::loose(Size::new(f64::INFINITY, f64::INFINITY)),
        );
        assert_eq!(unbounded, Size::new(DEFAULT_EXTENT, DEFAULT_EXTENT));
    }

    /// Rebuilding onto a different variant re-mints the program and restarts the
    /// clock — a new shader is not the old one's continuation.
    #[test]
    fn a_variant_change_re_mints_the_program_and_restarts_the_clock() {
        let theme = crate::theme();
        let before = shader_background::<()>(ShaderBackgroundVariant::MeshGradient);
        let mut widget = laid_out(&before);
        painted(&mut widget, 0, Some(&theme));
        let (moved, _, _) = painted(&mut widget, 2_000, Some(&theme));
        assert!(shader_quad(&moved).2 > 0.0, "the clock was running");
        let first_id = shader_quad(&moved).0;

        let after = shader_background::<()>(ShaderBackgroundVariant::Waves);
        let mut next_id = 0u64;
        let mut ctx = BuildCtx::new(&mut next_id);
        let flags = View::<()>::rebuild(&after, &before, &mut widget, &mut ctx);
        assert!(flags.contains(ChangeFlags::PAINT));
        assert_eq!(widget.variant(), ShaderBackgroundVariant::Waves);

        let (restarted, _, _) = painted(&mut widget, 3_000, Some(&theme));
        assert_ne!(shader_quad(&restarted).0, first_id, "a new program");
        assert_eq!(shader_quad(&restarted).2, 0.0, "a new clock origin");
    }

    /// Rebuilding with the same view changes nothing — no re-mint, no flags.
    #[test]
    fn an_unchanged_rebuild_keeps_the_compiled_program() {
        let theme = crate::theme();
        let view = shader_background::<()>(ShaderBackgroundVariant::MeshGradient);
        let mut widget = laid_out(&view);
        let (first, _, _) = painted(&mut widget, 0, Some(&theme));
        let id = shader_quad(&first).0;

        let same = shader_background::<()>(ShaderBackgroundVariant::MeshGradient);
        let mut next_id = 0u64;
        let mut ctx = BuildCtx::new(&mut next_id);
        let flags = View::<()>::rebuild(&same, &view, &mut widget, &mut ctx);
        assert_eq!(flags, ChangeFlags::NONE);

        let (second, _, _) = painted(&mut widget, 16, Some(&theme));
        assert_eq!(shader_quad(&second).0, id);
    }

    /// A non-finite speed is ignored rather than poisoning the clock with a NaN
    /// the shader would then read as a uniform.
    #[test]
    fn a_non_finite_speed_is_ignored() {
        let theme = crate::theme();
        let mut widget = laid_out(
            &shader_background(ShaderBackgroundVariant::MeshGradient)
                .speed(f32::NAN)
                .speed(f32::INFINITY),
        );
        painted(&mut widget, 0, Some(&theme));
        let (second, _, _) = painted(&mut widget, 1_000, Some(&theme));
        let time = shader_quad(&second).2;
        assert!(time.is_finite(), "the clock stayed finite: {time}");
        assert!(
            (time - DEFAULT_SPEED).abs() < 1e-5,
            "the preset speed stood"
        );
    }

    /// The kill switch resolves the way the engine resolves it: a run-time
    /// value wins, a compile-time value counts when the run-time one is unset,
    /// and both halves accept the engine's spellings.
    #[test]
    fn the_kill_switch_honours_the_compile_time_half_like_the_engine() {
        assert!(kill_switch_enabled(Some("1"), None));
        assert!(kill_switch_enabled(Some("TRUE"), None));
        assert!(!kill_switch_enabled(Some("0"), None));
        assert!(!kill_switch_enabled(None, None));
        assert!(kill_switch_enabled(None, Some("true")));
        assert!(kill_switch_enabled(Some("0"), Some("1")), "run time wins");
        assert!(
            !kill_switch_enabled(Some("1"), Some("0")),
            "run time wins the other way too"
        );
        assert!(
            !kill_switch_enabled(Some("1"), Some("")),
            "an empty run-time value is a set value"
        );
    }

    /// No caller colour can put a non-finite or out-of-range component into a
    /// shader constant.
    #[test]
    fn colours_are_sanitised_before_they_reach_the_source() {
        let bad = Color::new([f32::NAN, 2.0, -1.0, f32::INFINITY]);
        let view = shader_background::<()>(ShaderBackgroundVariant::MeshGradient)
            .colors([bad, bad, bad, bad])
            .back(bad);
        for c in view.colors.expect("colours set") {
            assert_eq!(c.components, [0.0, 1.0, 0.0, 0.0]);
        }
        assert_eq!(
            view.back.expect("back set").components,
            [0.0, 1.0, 0.0, 0.0]
        );
    }
}
