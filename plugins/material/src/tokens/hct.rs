//! The HCT seed-color engine: build a full Material 3 [`ColorScheme`] (plus
//! its [`MaterialTokens`] semantic extension) from *any* seed color at
//! runtime, instead of only the hand-baked `#6750A4` tables in
//! [`super::color`].
//!
//! Ported from material-color-utilities 0.11.1 (Apache-2.0, © 2021 Google
//! LLC; <https://github.com/material-foundation/material-color-utilities>,
//! the Dart package Flutter itself ships), retrieved 2026-08-19 — the
//! `utils/color_utils`, `hct/viewing_conditions`, `hct/cam16`,
//! `hct/src/hct_solver`, `palettes/tonal_palette` and `palettes/core_palette`
//! files. See this crate's `NOTICE`.
//!
//! # Porting decisions
//!
//! - **Only the scheme-generation path is ported.** CAM16 keeps its forward
//!   transform, and of its nine dimensions only `hue`/`chroma` (the two HCT
//!   needs) are returned; the inverse transform is the HCT solver's job, so
//!   `Cam16.viewed`/`fromUcs`/`distance` and the blend, quantize, contrast,
//!   dislike, score and `DynamicScheme` modules are all absent.
//! - **[`TonalPalette`] drops MCU's key-color search and its per-tone
//!   cache.** The key color feeds `DynamicScheme`, which this module does not
//!   implement, and computing it eagerly costs up to 99 extra solves per
//!   palette; the cache would cost interior mutability for a table each
//!   scheme builds exactly once.
//! - **The 255-entry critical-plane table is computed, not transcribed.** Its
//!   entry `i` is by construction the linear-RGB value whose delinearization
//!   is `i + 0.5`, i.e. MCU's own `linearized()` evaluated at a fractional
//!   8-bit input ([`critical_plane`]); `critical_planes_match_upstream` pins
//!   four sampled entries against MCU's literals.
//! - **Palette definition** (see [`CorePalette::of`]): primary chroma follows
//!   MCU's `CorePalette` (`max(48, seed chroma)`) so the seed itself surfaces
//!   as `primary`, while the neutral palette follows the 2023 `tonalSpot`
//!   value (6, not `CorePalette`'s 4) because the `surface_container_*` tone
//!   table below *is* the 2023 spec's and was drawn against that palette.
//! - **The nine semantic roles reuse [`super::color`]'s derivation rules
//!   exactly** — six aliases of scheme fields, `success`/`warning` read off
//!   the baked constants (they are seed-independent in the reference too),
//!   and `surface_strong` computed with the same 6% blend.
//!
//! # Relationship to the baked baseline
//!
//! [`from_seed`] at seed `#6750A4` does **not** reproduce
//! [`super::color::color_scheme_light`]/`_dark` byte for byte, and cannot:
//! those tables are Google's *published* Material 3 baseline ref palette
//! (material-web tokens v0.192), generated years earlier by a different
//! material-color-utilities release. This module matches *current* MCU
//! exactly instead (the golden tests), which leaves it very close to those
//! tables but not identical: outside the error family, 18 light / 21 dark
//! roles agree byte for byte, 22 light / 20 dark differ by a single 8-bit
//! step in a single channel, and three roles in all (`surface_container`
//! both ways, `surface_container_high` light) differ by two or three. The
//! error family is a wholesale difference — the baseline's error ramp is not
//! MCU's `TonalPalette.of(25, 84)` at all; its tone-40 entry measures hue
//! 26.0/chroma 76.3, not 25/84.
//! `from_seed_diverges_from_the_baked_baseline_only_as_recorded` pins every
//! divergent role's pair of values, so the set can only change deliberately.
//!
//! # Cost
//!
//! Each role is an independent solve; a [`from_seed`] call builds both
//! brightnesses (92 solves, sub-millisecond but not free) and allocates
//! nothing. Call it when the seed changes — never per frame, and never from
//! a widget's `paint`/`layout`.

use std::sync::LazyLock;

use frust::{Brightness, ColorScheme, Theme};
use peniko::Color;

use super::color::{MaterialSemanticColors, semantic_dark, semantic_light};
use super::extension::MaterialTokens;

// ---- sRGB / XYZ / L* conversions (MCU `ColorUtils`) ---------------------

/// sRGB primaries to XYZ under the D65 white point.
const SRGB_TO_XYZ: [[f64; 3]; 3] = [
    [0.41233895, 0.35762064, 0.18051042],
    [0.2126, 0.7152, 0.0722],
    [0.01932141, 0.11916382, 0.95034478],
];

/// D65, "white on a sunny day" — the white point every conversion here
/// assumes.
const WHITE_POINT_D65: [f64; 3] = [95.047, 100.0, 108.883];

fn matrix_multiply(row: [f64; 3], matrix: &[[f64; 3]; 3]) -> [f64; 3] {
    [
        row[0] * matrix[0][0] + row[1] * matrix[0][1] + row[2] * matrix[0][2],
        row[0] * matrix[1][0] + row[1] * matrix[1][1] + row[2] * matrix[1][2],
        row[0] * matrix[2][0] + row[1] * matrix[2][1] + row[2] * matrix[2][2],
    ]
}

/// MCU's `MathUtils.signum`: **zero for zero**, unlike `f64::signum` (which
/// answers `1.0` for `+0.0`). The CAM16 chromatic-adaptation terms depend on
/// the zero case, so this cannot be swapped for the std method.
fn signum(value: f64) -> f64 {
    if value < 0.0 {
        -1.0
    } else if value == 0.0 {
        0.0
    } else {
        1.0
    }
}

fn sanitize_degrees(degrees: f64) -> f64 {
    degrees.rem_euclid(360.0)
}

/// One 8-bit channel to its linear-RGB value, `0.0..=100.0`.
fn linearized(component: u8) -> f64 {
    linearized_f64(component as f64)
}

/// [`linearized`] over a *fractional* 8-bit input — the form
/// [`critical_plane`] needs.
fn linearized_f64(component: f64) -> f64 {
    let normalized = component / 255.0;
    if normalized <= 0.040_449_936 {
        normalized / 12.92 * 100.0
    } else {
        ((normalized + 0.055) / 1.055).powf(2.4) * 100.0
    }
}

/// One linear-RGB value back to an 8-bit channel, clamped.
fn delinearized(rgb_component: f64) -> u8 {
    let value = (true_delinearized(rgb_component)).round() as i32;
    value.clamp(0, 255) as u8
}

/// [`delinearized`] without the rounding/clamping — the solver bisects
/// against 8-bit *plane* coordinates, which are fractional.
fn true_delinearized(rgb_component: f64) -> f64 {
    let normalized = rgb_component / 100.0;
    let delinearized = if normalized <= 0.003_130_8 {
        normalized * 12.92
    } else {
        1.055 * normalized.powf(1.0 / 2.4) - 0.055
    };
    delinearized * 255.0
}

fn lab_f(t: f64) -> f64 {
    const E: f64 = 216.0 / 24389.0;
    const KAPPA: f64 = 24389.0 / 27.0;
    if t > E {
        t.cbrt()
    } else {
        (KAPPA * t + 16.0) / 116.0
    }
}

fn lab_inv_f(ft: f64) -> f64 {
    const E: f64 = 216.0 / 24389.0;
    const KAPPA: f64 = 24389.0 / 27.0;
    let ft3 = ft * ft * ft;
    if ft3 > E {
        ft3
    } else {
        (116.0 * ft - 16.0) / KAPPA
    }
}

/// L* (perceptual luminance) to Y (relative luminance).
fn y_from_lstar(lstar: f64) -> f64 {
    100.0 * lab_inv_f((lstar + 16.0) / 116.0)
}

fn lstar_from_y(y: f64) -> f64 {
    lab_f(y / 100.0) * 116.0 - 16.0
}

fn argb_from_rgb(red: u8, green: u8, blue: u8) -> u32 {
    0xFF00_0000 | (red as u32) << 16 | (green as u32) << 8 | blue as u32
}

fn argb_from_linrgb(linrgb: [f64; 3]) -> u32 {
    argb_from_rgb(
        delinearized(linrgb[0]),
        delinearized(linrgb[1]),
        delinearized(linrgb[2]),
    )
}

/// The neutral gray with the requested L* — what the solver answers with
/// when chroma is unreachable (a fully desaturated request, or a tone at
/// either end of the range).
fn argb_from_lstar(lstar: f64) -> u32 {
    let component = delinearized(y_from_lstar(lstar));
    argb_from_rgb(component, component, component)
}

fn xyz_from_argb(argb: u32) -> [f64; 3] {
    let r = linearized((argb >> 16 & 255) as u8);
    let g = linearized((argb >> 8 & 255) as u8);
    let b = linearized((argb & 255) as u8);
    matrix_multiply([r, g, b], &SRGB_TO_XYZ)
}

fn lstar_from_argb(argb: u32) -> f64 {
    lstar_from_y(xyz_from_argb(argb)[1])
}

fn argb_from_color(color: Color) -> u32 {
    let [r, g, b, _] = color.to_rgba8().to_u8_array();
    argb_from_rgb(r, g, b)
}

fn color_from_argb(argb: u32) -> Color {
    Color::from_rgb8(
        (argb >> 16 & 255) as u8,
        (argb >> 8 & 255) as u8,
        (argb & 255) as u8,
    )
}

// ---- CAM16 viewing conditions -------------------------------------------

/// The precomputed CAM16 environment every conversion in this module is
/// measured in — MCU's `ViewingConditions.sRgb`, i.e. its `make()` defaults
/// (D65 white point, an adapting luminance derived from L*50, a 50 L*
/// background, "average" surround 2.0, eyes not discounting the
/// illuminant). MCU exposes the general constructor; only this one
/// instantiation is ever used to build a scheme, so only it is ported —
/// likewise its `fl_root` field, which only CAM16's unported
/// brightness/colorfulness/saturation dimensions consume.
struct ViewingConditions {
    background_y_to_white_point_y: f64,
    aw: f64,
    nbb: f64,
    ncb: f64,
    c: f64,
    n_c: f64,
    rgb_d: [f64; 3],
    fl: f64,
    z: f64,
}

static SRGB_VIEWING_CONDITIONS: LazyLock<ViewingConditions> = LazyLock::new(|| {
    let white_point = WHITE_POINT_D65;
    let adapting_luminance = 200.0 / std::f64::consts::PI * y_from_lstar(50.0) / 100.0;
    let background_lstar = 50.0_f64;

    // Test illuminant white, in 'cone'/RGB responses.
    let r_w = white_point[0] * 0.401288 + white_point[1] * 0.650173 + white_point[2] * -0.051461;
    let g_w = white_point[0] * -0.250268 + white_point[1] * 1.204414 + white_point[2] * 0.045854;
    let b_w = white_point[0] * -0.002079 + white_point[1] * 0.048952 + white_point[2] * 0.953127;

    // Surround 2.0 ("average"), scaled into CAM16's (0.8, 1.0) domain.
    let f = 0.8 + 2.0 / 10.0;
    let c = lerp(0.59, 0.69, (f - 0.9) * 10.0);
    let d = (f * (1.0 - (1.0 / 3.6) * ((-adapting_luminance - 42.0) / 92.0).exp())).clamp(0.0, 1.0);
    let n_c = f;

    // 100.0 rather than the white point's own luminance is deliberate:
    // later stages already scale appearance relative to it (Fairchild,
    // *Color Appearance Models* 3rd ed., on the CIE 2004a CIECAM02 report).
    let rgb_d = [
        d * (100.0 / r_w) + 1.0 - d,
        d * (100.0 / g_w) + 1.0 - d,
        d * (100.0 / b_w) + 1.0 - d,
    ];

    let k = 1.0 / (5.0 * adapting_luminance + 1.0);
    let k4 = k * k * k * k;
    let k4_f = 1.0 - k4;
    let fl = (k4 * adapting_luminance) + (0.1 * k4_f * k4_f * (5.0 * adapting_luminance).cbrt());
    let n = y_from_lstar(background_lstar) / white_point[1];
    let z = 1.48 + n.sqrt();
    let nbb = 0.725 / n.powf(0.2);

    let rgb_a_factors = [
        (fl * rgb_d[0] * r_w / 100.0).powf(0.42),
        (fl * rgb_d[1] * g_w / 100.0).powf(0.42),
        (fl * rgb_d[2] * b_w / 100.0).powf(0.42),
    ];
    let rgb_a = [
        400.0 * rgb_a_factors[0] / (rgb_a_factors[0] + 27.13),
        400.0 * rgb_a_factors[1] / (rgb_a_factors[1] + 27.13),
        400.0 * rgb_a_factors[2] / (rgb_a_factors[2] + 27.13),
    ];

    ViewingConditions {
        background_y_to_white_point_y: n,
        aw: (40.0 * rgb_a[0] + 20.0 * rgb_a[1] + rgb_a[2]) / 20.0 * nbb,
        nbb,
        ncb: nbb,
        c,
        n_c,
        rgb_d,
        fl,
        z,
    }
});

fn lerp(start: f64, stop: f64, amount: f64) -> f64 {
    (1.0 - amount) * start + amount * stop
}

// ---- CAM16 (forward only) -----------------------------------------------

/// The two CAM16 dimensions HCT is built on. MCU's `Cam16` carries nine:
/// lightness `J` is computed here as a local (chroma depends on it), and the
/// remaining six — brightness, colorfulness, saturation and the three
/// CAM16-UCS coordinates — serve perceptual-distance work this module has no
/// caller for.
struct Cam16 {
    hue: f64,
    chroma: f64,
}

impl Cam16 {
    fn from_argb(argb: u32) -> Self {
        let vc = &*SRGB_VIEWING_CONDITIONS;
        let [x, y, z] = xyz_from_argb(argb);

        // XYZ to 'cone'/RGB responses, then discount the illuminant.
        let r_c = 0.401288 * x + 0.650173 * y - 0.051461 * z;
        let g_c = -0.250268 * x + 1.204414 * y + 0.045854 * z;
        let b_c = -0.002079 * x + 0.048952 * y + 0.953127 * z;
        let r_d = vc.rgb_d[0] * r_c;
        let g_d = vc.rgb_d[1] * g_c;
        let b_d = vc.rgb_d[2] * b_c;

        // Chromatic adaptation.
        let r_af = (vc.fl * r_d.abs() / 100.0).powf(0.42);
        let g_af = (vc.fl * g_d.abs() / 100.0).powf(0.42);
        let b_af = (vc.fl * b_d.abs() / 100.0).powf(0.42);
        let r_a = signum(r_d) * 400.0 * r_af / (r_af + 27.13);
        let g_a = signum(g_d) * 400.0 * g_af / (g_af + 27.13);
        let b_a = signum(b_d) * 400.0 * b_af / (b_af + 27.13);

        // Redness-greenness, yellowness-blueness, and the two auxiliaries.
        let a = (11.0 * r_a + -12.0 * g_a + b_a) / 11.0;
        let b = (r_a + g_a - 2.0 * b_a) / 9.0;
        let u = (20.0 * r_a + 20.0 * g_a + 21.0 * b_a) / 20.0;
        let p2 = (40.0 * r_a + 20.0 * g_a + b_a) / 20.0;

        let atan_degrees = b.atan2(a) * 180.0 / std::f64::consts::PI;
        let hue = if atan_degrees < 0.0 {
            atan_degrees + 360.0
        } else if atan_degrees >= 360.0 {
            atan_degrees - 360.0
        } else {
            atan_degrees
        };

        let ac = p2 * vc.nbb;
        let j = 100.0 * (ac / vc.aw).powf(vc.c * vc.z);

        // `hue_prime` closes CAM16's hue quadrature at the red end.
        let hue_prime = if hue < 20.14 { hue + 360.0 } else { hue };
        let e_hue = 0.25 * ((hue_prime * std::f64::consts::PI / 180.0 + 2.0).cos() + 3.8);
        let p1 = 50000.0 / 13.0 * e_hue * vc.n_c * vc.ncb;
        let t = p1 * (a * a + b * b).sqrt() / (u + 0.305);
        let alpha =
            t.powf(0.9) * (1.64 - 0.29_f64.powf(vc.background_y_to_white_point_y)).powf(0.73);

        Self {
            hue,
            chroma: alpha * (j / 100.0).sqrt(),
        }
    }
}

// ---- The HCT solver (MCU `HctSolver`) -----------------------------------

const SCALED_DISCOUNT_FROM_LINRGB: [[f64; 3]; 3] = [
    [
        0.001200833568784504,
        0.002389694492170889,
        0.0002795742885861124,
    ],
    [
        0.0005891086651375999,
        0.0029785502573438758,
        0.0003270666104008398,
    ],
    [
        0.00010146692491640572,
        0.0005364214359186694,
        0.0032979401770712076,
    ],
];

const LINRGB_FROM_SCALED_DISCOUNT: [[f64; 3]; 3] = [
    [1373.2198709594231, -1100.4251190754821, -7.278681089101213],
    [-271.815969077903, 559.6580465940733, -32.46047482791194],
    [1.9622899599665666, -57.173814538844006, 308.7233197812385],
];

/// Y (relative luminance) coefficients of the linear-RGB primaries.
const Y_FROM_LINRGB: [f64; 3] = [0.2126, 0.7152, 0.0722];

/// The linear-RGB value that delinearizes to exactly `index + 0.5`, i.e. the
/// boundary plane between 8-bit outputs `index` and `index + 1`. MCU ships
/// these 255 numbers as a literal table; the generating identity is exact,
/// so this computes them instead (pinned by
/// `critical_planes_match_upstream`).
fn critical_plane(index: usize) -> f64 {
    linearized_f64(index as f64 + 0.5)
}

/// A coterminal angle in `0..2pi`, for angles that stay near zero.
fn sanitize_radians(angle: f64) -> f64 {
    (angle + std::f64::consts::PI * 8.0).rem_euclid(std::f64::consts::PI * 2.0)
}

fn chromatic_adaptation(component: f64) -> f64 {
    let af = component.abs().powf(0.42);
    signum(component) * 400.0 * af / (af + 27.13)
}

fn inverse_chromatic_adaptation(adapted: f64) -> f64 {
    let adapted_abs = adapted.abs();
    let base = (27.13 * adapted_abs / (400.0 - adapted_abs)).max(0.0);
    signum(adapted) * base.powf(1.0 / 0.42)
}

/// The CAM16 hue of a linear-RGB color, in radians.
fn hue_of(linrgb: [f64; 3]) -> f64 {
    let scaled_discount = matrix_multiply(linrgb, &SCALED_DISCOUNT_FROM_LINRGB);
    let r_a = chromatic_adaptation(scaled_discount[0]);
    let g_a = chromatic_adaptation(scaled_discount[1]);
    let b_a = chromatic_adaptation(scaled_discount[2]);
    let a = (11.0 * r_a + -12.0 * g_a + b_a) / 11.0;
    let b = (r_a + g_a - 2.0 * b_a) / 9.0;
    b.atan2(a)
}

fn are_in_cyclic_order(a: f64, b: f64, c: f64) -> bool {
    sanitize_radians(b - a) < sanitize_radians(c - a)
}

fn lerp_point(source: [f64; 3], t: f64, target: [f64; 3]) -> [f64; 3] {
    [
        source[0] + (target[0] - source[0]) * t,
        source[1] + (target[1] - source[1]) * t,
        source[2] + (target[2] - source[2]) * t,
    ]
}

/// Where the segment `source`..`target` crosses the plane `axis =
/// coordinate`.
fn set_coordinate(source: [f64; 3], coordinate: f64, target: [f64; 3], axis: usize) -> [f64; 3] {
    let t = (coordinate - source[axis]) / (target[axis] - source[axis]);
    lerp_point(source, t, target)
}

/// The `n`th (`0..12`) candidate vertex of the polygon where the plane
/// `Y = y` cuts the RGB cube, or `None` when that candidate falls outside
/// the cube.
fn nth_vertex(y: f64, n: usize) -> Option<[f64; 3]> {
    let [k_r, k_g, k_b] = Y_FROM_LINRGB;
    let coord_a = if n % 4 <= 1 { 0.0 } else { 100.0 };
    let coord_b = if n.is_multiple_of(2) { 0.0 } else { 100.0 };
    let bounded = |x: f64| (0.0..=100.0).contains(&x);
    if n < 4 {
        let (g, b) = (coord_a, coord_b);
        let r = (y - g * k_g - b * k_b) / k_r;
        bounded(r).then_some([r, g, b])
    } else if n < 8 {
        let (b, r) = (coord_a, coord_b);
        let g = (y - r * k_r - b * k_b) / k_g;
        bounded(g).then_some([r, g, b])
    } else {
        let (r, g) = (coord_a, coord_b);
        let b = (y - r * k_r - g * k_g) / k_b;
        bounded(b).then_some([r, g, b])
    }
}

/// The segment of that polygon containing `target_hue`, as its endpoints.
fn bisect_to_segment(y: f64, target_hue: f64) -> ([f64; 3], [f64; 3]) {
    let mut left = [-1.0; 3];
    let mut right = left;
    let mut left_hue = 0.0;
    let mut right_hue = 0.0;
    let mut initialized = false;
    let mut uncut = true;
    for n in 0..12 {
        let Some(mid) = nth_vertex(y, n) else {
            continue;
        };
        let mid_hue = hue_of(mid);
        if !initialized {
            left = mid;
            right = mid;
            left_hue = mid_hue;
            right_hue = mid_hue;
            initialized = true;
            continue;
        }
        if uncut || are_in_cyclic_order(left_hue, mid_hue, right_hue) {
            uncut = false;
            if are_in_cyclic_order(left_hue, target_hue, mid_hue) {
                right = mid;
                right_hue = mid_hue;
            } else {
                left = mid;
                left_hue = mid_hue;
            }
        }
    }
    (left, right)
}

/// The color of luminance `y` and hue `target_hue` on the sRGB cube's
/// boundary — the maximum-chroma answer, used when the requested chroma is
/// out of gamut.
fn bisect_to_limit(y: f64, target_hue: f64) -> [f64; 3] {
    let (mut left, mut right) = bisect_to_segment(y, target_hue);
    let mut left_hue = hue_of(left);
    for axis in 0..3 {
        if left[axis] == right[axis] {
            continue;
        }
        let (mut l_plane, mut r_plane) = if left[axis] < right[axis] {
            (
                (true_delinearized(left[axis]) - 0.5).floor() as i32,
                (true_delinearized(right[axis]) - 0.5).ceil() as i32,
            )
        } else {
            (
                (true_delinearized(left[axis]) - 0.5).ceil() as i32,
                (true_delinearized(right[axis]) - 0.5).floor() as i32,
            )
        };
        for _ in 0..8 {
            if (r_plane - l_plane).abs() <= 1 {
                break;
            }
            let m_plane = ((l_plane + r_plane) as f64 / 2.0).floor() as i32;
            let mid = set_coordinate(
                left,
                critical_plane(m_plane.clamp(0, 254) as usize),
                right,
                axis,
            );
            let mid_hue = hue_of(mid);
            if are_in_cyclic_order(left_hue, target_hue, mid_hue) {
                right = mid;
                r_plane = m_plane;
            } else {
                left = mid;
                left_hue = mid_hue;
                l_plane = m_plane;
            }
        }
    }
    [
        (left[0] + right[0]) / 2.0,
        (left[1] + right[1]) / 2.0,
        (left[2] + right[2]) / 2.0,
    ]
}

/// Newton-iterates CAM16's lightness `J` for the color with this hue,
/// chroma and luminance. `None` means the request left the sRGB gamut, and
/// the caller falls back to [`bisect_to_limit`].
fn find_result_by_j(hue_radians: f64, chroma: f64, y: f64) -> Option<u32> {
    let vc = &*SRGB_VIEWING_CONDITIONS;
    let mut j = y.sqrt() * 11.0;
    let t_inner_coeff = 1.0 / (1.64 - 0.29_f64.powf(vc.background_y_to_white_point_y)).powf(0.73);
    let e_hue = 0.25 * ((hue_radians + 2.0).cos() + 3.8);
    let p1 = e_hue * (50000.0 / 13.0) * vc.n_c * vc.ncb;
    let h_sin = hue_radians.sin();
    let h_cos = hue_radians.cos();

    for iteration in 0..5 {
        let j_normalized = j / 100.0;
        let alpha = if chroma == 0.0 || j == 0.0 {
            0.0
        } else {
            chroma / j_normalized.sqrt()
        };
        let t = (alpha * t_inner_coeff).powf(1.0 / 0.9);
        let ac = vc.aw * j_normalized.powf(1.0 / vc.c / vc.z);
        let p2 = ac / vc.nbb;
        let gamma = 23.0 * (p2 + 0.305) * t / (23.0 * p1 + 11.0 * t * h_cos + 108.0 * t * h_sin);
        let a = gamma * h_cos;
        let b = gamma * h_sin;
        let r_a = (460.0 * p2 + 451.0 * a + 288.0 * b) / 1403.0;
        let g_a = (460.0 * p2 - 891.0 * a - 261.0 * b) / 1403.0;
        let b_a = (460.0 * p2 - 220.0 * a - 6300.0 * b) / 1403.0;
        let linrgb = matrix_multiply(
            [
                inverse_chromatic_adaptation(r_a),
                inverse_chromatic_adaptation(g_a),
                inverse_chromatic_adaptation(b_a),
            ],
            &LINRGB_FROM_SCALED_DISCOUNT,
        );
        if linrgb[0] < 0.0 || linrgb[1] < 0.0 || linrgb[2] < 0.0 {
            return None;
        }
        let fnj = Y_FROM_LINRGB[0] * linrgb[0]
            + Y_FROM_LINRGB[1] * linrgb[1]
            + Y_FROM_LINRGB[2] * linrgb[2];
        if fnj <= 0.0 {
            return None;
        }
        if iteration == 4 || (fnj - y).abs() < 0.002 {
            if linrgb[0] > 100.01 || linrgb[1] > 100.01 || linrgb[2] > 100.01 {
                return None;
            }
            return Some(argb_from_linrgb(linrgb));
        }
        // Newton's method, approximating fn'(j) as 2 * fn(j) / j.
        j -= (fnj - y) * j / (2.0 * fnj);
    }
    None
}

/// The sRGB color closest to the requested HCT triple: hue and tone are
/// honored, chroma is maximized when the request is out of gamut.
fn solve_to_argb(hue_degrees: f64, chroma: f64, lstar: f64) -> u32 {
    if chroma < 0.0001 || !(0.0001..=99.9999).contains(&lstar) {
        return argb_from_lstar(lstar);
    }
    let hue_radians = sanitize_degrees(hue_degrees) / 180.0 * std::f64::consts::PI;
    let y = y_from_lstar(lstar);
    if let Some(exact) = find_result_by_j(hue_radians, chroma, y) {
        return exact;
    }
    argb_from_linrgb(bisect_to_limit(y, hue_radians))
}

// ---- Public HCT types ----------------------------------------------------

/// A color in HCT: **h**ue (0..360), **c**hroma (0..~130, hue- and
/// tone-dependent), **t**one (0..100, L\* from L\*a\*b\*).
///
/// Constructing one from a hue/chroma/tone triple maps it into sRGB
/// immediately, so [`Hct::to_color`] round-trips exactly; a request outside
/// the gamut comes back with its chroma reduced until it fits, never with a
/// shifted hue or tone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hct {
    hue: f64,
    chroma: f64,
    tone: f64,
    argb: u32,
}

impl Hct {
    /// The HCT coordinates of an existing color (its alpha is ignored).
    pub fn from_color(color: Color) -> Self {
        Self::from_argb(argb_from_color(color))
    }

    /// The in-gamut sRGB color nearest the requested triple — see the type's
    /// docs for what "nearest" gives up.
    pub fn new(hue: f64, chroma: f64, tone: f64) -> Self {
        Self::from_argb(solve_to_argb(hue, chroma, tone))
    }

    fn from_argb(argb: u32) -> Self {
        let cam = Cam16::from_argb(argb);
        Self {
            hue: cam.hue,
            chroma: cam.chroma,
            tone: lstar_from_argb(argb),
            argb,
        }
    }

    /// Hue in degrees, `0.0..360.0`.
    pub fn hue(&self) -> f64 {
        self.hue
    }

    /// Chroma — informally colorfulness; its maximum depends on hue and
    /// tone.
    pub fn chroma(&self) -> f64 {
        self.chroma
    }

    /// Tone (L\*), `0.0..=100.0`.
    pub fn tone(&self) -> f64 {
        self.tone
    }

    /// The opaque sRGB color this HCT resolves to.
    pub fn to_color(&self) -> Color {
        color_from_argb(self.argb)
    }
}

/// One Material tonal ramp: a fixed hue and chroma, sampled by tone.
///
/// This is the unit a Material scheme is built from — every role in a
/// [`from_seed`] scheme is one `(palette, tone)` lookup. Sampling is *not*
/// cached: each [`TonalPalette::tone`] call runs a full HCT solve, which is
/// why scheme generation belongs to a seed change and not to a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TonalPalette {
    hue: f64,
    chroma: f64,
}

impl TonalPalette {
    /// The ramp at `hue` (degrees) and `chroma`.
    pub fn of(hue: f64, chroma: f64) -> Self {
        Self { hue, chroma }
    }

    /// The ramp through an existing color's own hue and chroma.
    pub fn from_color(color: Color) -> Self {
        let hct = Hct::from_color(color);
        Self::of(hct.hue(), hct.chroma())
    }

    /// This ramp's hue, in degrees.
    pub fn hue(&self) -> f64 {
        self.hue
    }

    /// This ramp's requested chroma — a given tone may resolve to less if
    /// the gamut cannot hold it.
    pub fn chroma(&self) -> f64 {
        self.chroma
    }

    /// The color at `tone` (0 = black, 100 = white).
    pub fn tone(&self, tone: f64) -> Color {
        color_from_argb(self.tone_argb(tone))
    }

    fn tone_argb(&self, tone: f64) -> u32 {
        solve_to_argb(self.hue, self.chroma, tone)
    }
}

/// Material's minimum primary chroma: a seed duller than this is boosted so
/// the resulting theme still reads as colored.
const PRIMARY_MIN_CHROMA: f64 = 48.0;
const SECONDARY_CHROMA: f64 = 16.0;
const TERTIARY_HUE_ROTATION: f64 = 60.0;
const TERTIARY_CHROMA: f64 = 24.0;
/// The 2023 spec's neutral chroma (MCU `SchemeTonalSpot`); `CorePalette`'s
/// own value is 4, which predates the `surface_container_*` roles this
/// module's tone table carries — see the module docs' porting decisions.
const NEUTRAL_CHROMA: f64 = 6.0;
const NEUTRAL_VARIANT_CHROMA: f64 = 8.0;
/// The error ramp is fixed, not seed-derived — in MCU too.
const ERROR_HUE: f64 = 25.0;
const ERROR_CHROMA: f64 = 84.0;

/// The six tonal ramps a Material scheme is drawn from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CorePalette {
    /// The seed's own hue at chroma `max(48, seed chroma)`.
    pub primary: TonalPalette,
    /// The seed's hue, muted (chroma 16).
    pub secondary: TonalPalette,
    /// The seed's hue rotated +60°, chroma 24.
    pub tertiary: TonalPalette,
    /// The seed's hue, near-gray (chroma 6) — surfaces and their content.
    pub neutral: TonalPalette,
    /// The seed's hue, slightly more colored than [`Self::neutral`] (chroma
    /// 8) — outlines and surface-variant content.
    pub neutral_variant: TonalPalette,
    /// A fixed red ramp (hue 25, chroma 84), independent of the seed.
    pub error: TonalPalette,
}

impl CorePalette {
    /// Derive all six ramps from one seed color (its alpha is ignored).
    ///
    /// `primary` keeps the seed's own chroma whenever that clears Material's
    /// floor of 48, so a sufficiently colorful seed reappears verbatim as the
    /// light scheme's `primary` (tone 40). A gray or black
    /// seed has no meaningful hue of its own: CAM16 reports ~209.5° for
    /// grays and whites and exactly 0° for pure black, and this module keeps
    /// that behavior rather than substituting a hue of its own — the chroma
    /// floor alone is what turns a colorless seed into a usable theme.
    pub fn of(seed: Color) -> Self {
        let hct = Hct::from_color(seed);
        let hue = hct.hue();
        Self {
            primary: TonalPalette::of(hue, hct.chroma().max(PRIMARY_MIN_CHROMA)),
            secondary: TonalPalette::of(hue, SECONDARY_CHROMA),
            tertiary: TonalPalette::of(
                sanitize_degrees(hue + TERTIARY_HUE_ROTATION),
                TERTIARY_CHROMA,
            ),
            neutral: TonalPalette::of(hue, NEUTRAL_CHROMA),
            neutral_variant: TonalPalette::of(hue, NEUTRAL_VARIANT_CHROMA),
            error: TonalPalette::of(ERROR_HUE, ERROR_CHROMA),
        }
    }
}

// ---- Scheme assembly -----------------------------------------------------

/// `surface_strong`'s mix, matching [`super::color`]'s hand-baked value: a
/// 6% wash of `primary` over `surface`.
const SURFACE_STRONG_ALPHA: f64 = 0.06;

/// `accent` at `alpha` flattened over an opaque `surface`, rounded per
/// channel — the same formula (and rounding) [`super::color`] applied by
/// hand for the baked `surface_strong` constants.
fn alpha_blend(accent: Color, surface: Color, alpha: f64) -> Color {
    let [ar, ag, ab, _] = accent.to_rgba8().to_u8_array();
    let [sr, sg, sb, _] = surface.to_rgba8().to_u8_array();
    let mix = |a: u8, s: u8| -> u8 {
        ((a as f64 / 255.0 * alpha + s as f64 / 255.0 * (1.0 - alpha)) * 255.0).round() as u8
    };
    Color::from_rgb8(mix(ar, sr), mix(ag, sg), mix(ab, sb))
}

/// The Material 3 tone-role table: each role is one `(palette, tone)` pick,
/// with the light tone first and the dark tone second. Source: Material 3's
/// 2023 color roles (the `surface_container_*` generation), as encoded by
/// material-color-utilities' `MaterialDynamicColors`.
fn scheme_from_palettes(core: &CorePalette, brightness: Brightness) -> ColorScheme {
    let dark = matches!(brightness, Brightness::Dark);
    let pick = |palette: &TonalPalette, light_tone: f64, dark_tone: f64| {
        palette.tone(if dark { dark_tone } else { light_tone })
    };
    let (p, s, t, n, v, e) = (
        &core.primary,
        &core.secondary,
        &core.tertiary,
        &core.neutral,
        &core.neutral_variant,
        &core.error,
    );
    ColorScheme {
        primary: pick(p, 40.0, 80.0),
        on_primary: pick(p, 100.0, 20.0),
        primary_container: pick(p, 90.0, 30.0),
        on_primary_container: pick(p, 10.0, 90.0),
        // The four `*_fixed*` roles are brightness-independent by design:
        // they hold still so a shared element can cross a light/dark
        // boundary without recoloring.
        primary_fixed: p.tone(90.0),
        primary_fixed_dim: p.tone(80.0),
        on_primary_fixed: p.tone(10.0),
        on_primary_fixed_variant: p.tone(30.0),

        secondary: pick(s, 40.0, 80.0),
        on_secondary: pick(s, 100.0, 20.0),
        secondary_container: pick(s, 90.0, 30.0),
        on_secondary_container: pick(s, 10.0, 90.0),
        secondary_fixed: s.tone(90.0),
        secondary_fixed_dim: s.tone(80.0),
        on_secondary_fixed: s.tone(10.0),
        on_secondary_fixed_variant: s.tone(30.0),

        tertiary: pick(t, 40.0, 80.0),
        on_tertiary: pick(t, 100.0, 20.0),
        tertiary_container: pick(t, 90.0, 30.0),
        on_tertiary_container: pick(t, 10.0, 90.0),
        tertiary_fixed: t.tone(90.0),
        tertiary_fixed_dim: t.tone(80.0),
        on_tertiary_fixed: t.tone(10.0),
        on_tertiary_fixed_variant: t.tone(30.0),

        error: pick(e, 40.0, 80.0),
        on_error: pick(e, 100.0, 20.0),
        error_container: pick(e, 90.0, 30.0),
        on_error_container: pick(e, 10.0, 90.0),

        surface: pick(n, 98.0, 6.0),
        on_surface: pick(n, 10.0, 90.0),
        on_surface_variant: pick(v, 30.0, 80.0),
        surface_dim: pick(n, 87.0, 6.0),
        surface_bright: pick(n, 98.0, 24.0),
        surface_container_lowest: pick(n, 100.0, 4.0),
        surface_container_low: pick(n, 96.0, 10.0),
        surface_container: pick(n, 94.0, 12.0),
        surface_container_high: pick(n, 92.0, 17.0),
        surface_container_highest: pick(n, 90.0, 22.0),

        outline: pick(v, 50.0, 60.0),
        outline_variant: pick(v, 80.0, 30.0),
        shadow: n.tone(0.0),
        scrim: n.tone(0.0),
        inverse_surface: pick(n, 20.0, 90.0),
        inverse_on_surface: pick(n, 95.0, 20.0),
        inverse_primary: pick(p, 80.0, 40.0),
        surface_tint: pick(p, 40.0, 80.0),
    }
}

/// The nine M3E semantic roles for an already-built `scheme`, following
/// [`super::color`]'s derivation rules exactly: six are aliases of scheme
/// fields, `success`/`warning` are the reference's two seed-independent
/// constants (read off the baked tables rather than re-transcribed), and
/// `surface_strong` is the 6% `primary`-over-`surface` wash.
fn semantic_from_scheme(scheme: &ColorScheme, brightness: Brightness) -> MaterialSemanticColors {
    let baked = match brightness {
        Brightness::Light => semantic_light(),
        Brightness::Dark => semantic_dark(),
    };
    MaterialSemanticColors {
        emphasis: scheme.primary,
        on_emphasis: scheme.on_primary,
        info: scheme.tertiary,
        success: baked.success,
        warning: baked.warning,
        danger: scheme.error,
        surface_strong: alpha_blend(scheme.primary, scheme.surface, SURFACE_STRONG_ALPHA),
        on_surface_strong: scheme.on_surface,
        outline_strong: scheme.outline,
    }
}

/// Generate a Material 3 color scheme from any seed color.
///
/// Returns the [`ColorScheme`] for `brightness` paired with the
/// [`MaterialTokens`] semantic extension — which, like
/// [`MaterialTokens::material`], always carries *both* brightnesses, since a
/// themed app may flip live. Both halves are computed either way, so ask for
/// the pair once and reuse it rather than calling twice.
///
/// ```
/// use frust::Brightness;
/// use frust_material::from_seed;
/// use peniko::Color;
///
/// let (scheme, tokens) = from_seed(Color::from_rgb8(0x00, 0xA8, 0x6B), Brightness::Light);
/// assert_eq!(tokens.light.emphasis, scheme.primary);
/// ```
///
/// See [`theme_from_seed`] for the whole-[`Theme`] form, and this module's
/// docs for how the output relates to the hand-baked `#6750A4` baseline
/// ([`super::baseline`]) — near, deliberately not identical.
pub fn from_seed(seed: Color, brightness: Brightness) -> (ColorScheme, MaterialTokens) {
    let (light, dark, tokens) = seeded_schemes(seed);
    let scheme = match brightness {
        Brightness::Light => light,
        Brightness::Dark => dark,
    };
    (scheme, tokens)
}

/// Both schemes and the extension in one pass over the palettes — the shared
/// body of [`from_seed`] and [`theme_from_seed`], so neither solves the same
/// tone twice.
fn seeded_schemes(seed: Color) -> (ColorScheme, ColorScheme, MaterialTokens) {
    let core = CorePalette::of(seed);
    let light = scheme_from_palettes(&core, Brightness::Light);
    let dark = scheme_from_palettes(&core, Brightness::Dark);
    let tokens = MaterialTokens {
        light: semantic_from_scheme(&light, Brightness::Light),
        dark: semantic_from_scheme(&dark, Brightness::Dark),
    };
    (light, dark, tokens)
}

/// [`super::baseline`] with its colors regenerated from `seed`: the same M3
/// type scale, shape scale, elevation table, motion scheme and
/// [`super::status_palette`], with both color schemes and the
/// [`MaterialTokens`] extension seeded instead of hand-baked, starting at
/// `brightness`.
///
/// This is the one-call form an app's theme picker wants; the `StatusPalette`
/// extension is deliberately left at its baseline value, since its
/// success/warning/info roles are semantic constants rather than seed-derived
/// (the same rule [`from_seed`]'s own `success`/`warning` follow).
pub fn theme_from_seed(seed: Color, brightness: Brightness) -> Theme {
    let (light, dark, tokens) = seeded_schemes(seed);
    let mut theme = super::baseline();
    theme.light = light;
    theme.dark = dark;
    theme.brightness = brightness;
    theme.extensions.insert(tokens);
    theme
}

#[cfg(test)]
mod tests {
    use frust::StatusPalette;

    use super::*;
    use crate::tokens::color::{color_scheme_dark, color_scheme_light};

    /// The Material seed every baked table in [`super::super::color`] came
    /// from.
    const BASELINE_SEED: Color = Color::from_rgb8(0x67, 0x50, 0xA4);

    fn rgb(color: Color) -> u32 {
        argb_from_color(color) & 0xFF_FFFF
    }

    fn max_channel_delta(a: Color, b: Color) -> i32 {
        let [ar, ag, ab, _] = a.to_rgba8().to_u8_array();
        let [br, bg, bb, _] = b.to_rgba8().to_u8_array();
        [
            (ar as i32 - br as i32).abs(),
            (ag as i32 - bg as i32).abs(),
            (ab as i32 - bb as i32).abs(),
        ]
        .into_iter()
        .max()
        .expect("three channels")
    }

    /// Every one of `ColorScheme`'s 46 roles, by name — the shape the golden
    /// and divergence tables below are keyed on.
    fn roles(scheme: &ColorScheme) -> Vec<(&'static str, Color)> {
        vec![
            ("primary", scheme.primary),
            ("on_primary", scheme.on_primary),
            ("primary_container", scheme.primary_container),
            ("on_primary_container", scheme.on_primary_container),
            ("primary_fixed", scheme.primary_fixed),
            ("primary_fixed_dim", scheme.primary_fixed_dim),
            ("on_primary_fixed", scheme.on_primary_fixed),
            ("on_primary_fixed_variant", scheme.on_primary_fixed_variant),
            ("secondary", scheme.secondary),
            ("on_secondary", scheme.on_secondary),
            ("secondary_container", scheme.secondary_container),
            ("on_secondary_container", scheme.on_secondary_container),
            ("secondary_fixed", scheme.secondary_fixed),
            ("secondary_fixed_dim", scheme.secondary_fixed_dim),
            ("on_secondary_fixed", scheme.on_secondary_fixed),
            (
                "on_secondary_fixed_variant",
                scheme.on_secondary_fixed_variant,
            ),
            ("tertiary", scheme.tertiary),
            ("on_tertiary", scheme.on_tertiary),
            ("tertiary_container", scheme.tertiary_container),
            ("on_tertiary_container", scheme.on_tertiary_container),
            ("tertiary_fixed", scheme.tertiary_fixed),
            ("tertiary_fixed_dim", scheme.tertiary_fixed_dim),
            ("on_tertiary_fixed", scheme.on_tertiary_fixed),
            (
                "on_tertiary_fixed_variant",
                scheme.on_tertiary_fixed_variant,
            ),
            ("error", scheme.error),
            ("on_error", scheme.on_error),
            ("error_container", scheme.error_container),
            ("on_error_container", scheme.on_error_container),
            ("surface", scheme.surface),
            ("on_surface", scheme.on_surface),
            ("on_surface_variant", scheme.on_surface_variant),
            ("surface_dim", scheme.surface_dim),
            ("surface_bright", scheme.surface_bright),
            ("surface_container_lowest", scheme.surface_container_lowest),
            ("surface_container_low", scheme.surface_container_low),
            ("surface_container", scheme.surface_container),
            ("surface_container_high", scheme.surface_container_high),
            (
                "surface_container_highest",
                scheme.surface_container_highest,
            ),
            ("outline", scheme.outline),
            ("outline_variant", scheme.outline_variant),
            ("shadow", scheme.shadow),
            ("scrim", scheme.scrim),
            ("inverse_surface", scheme.inverse_surface),
            ("inverse_on_surface", scheme.inverse_on_surface),
            ("inverse_primary", scheme.inverse_primary),
            ("surface_tint", scheme.surface_tint),
        ]
    }

    fn assert_golden(seed: Color, brightness: Brightness, golden: &[(&str, u32)]) {
        let (scheme, _) = from_seed(seed, brightness);
        let produced = roles(&scheme);
        for (role, want) in golden {
            let (_, got) = produced
                .iter()
                .find(|(name, _)| name == role)
                .unwrap_or_else(|| panic!("unknown role `{role}`"));
            assert_eq!(
                rgb(*got),
                *want,
                "role `{role}` at seed {:06X} ({brightness:?}): got {:06X}, want {:06X}",
                rgb(seed),
                rgb(*got),
                want
            );
        }
    }

    /// Sampled entries of MCU's hard-coded `HctSolver._criticalPlanes`
    /// (indices 0, 1, 127, 254), proving [`critical_plane`]'s generating
    /// identity reproduces the published table rather than approximating it.
    #[test]
    fn critical_planes_match_upstream() {
        let upstream = [
            (0usize, 0.015176349177441876),
            (1, 0.045529047532325624),
            (127, 21.404114048223256),
            (254, 99.55452497210776),
        ];
        for (index, want) in upstream {
            let got = critical_plane(index);
            assert!(
                (got - want).abs() <= want.abs() * 1e-12,
                "critical plane {index}: got {got}, upstream {want}"
            );
        }
    }

    /// The CAM16 forward transform, against material-color-utilities 0.11.1's
    /// own `Cam16.fromInt` output.
    #[test]
    fn cam16_matches_upstream_for_the_baseline_seed() {
        let hct = Hct::from_color(BASELINE_SEED);
        assert!((hct.hue() - 298.980997210704).abs() < 1e-9, "{}", hct.hue());
        assert!(
            (hct.chroma() - 47.8565263749703).abs() < 1e-9,
            "{}",
            hct.chroma()
        );
        assert!(
            (hct.tone() - 40.08324408746242).abs() < 1e-9,
            "{}",
            hct.tone()
        );
    }

    /// A color's HCT coordinates, resolved back to sRGB, must land on the
    /// color it came from. The loose assertion is the ±1 an 8-bit round trip
    /// through a perceptual space is entitled to; the strict one pins that
    /// upstream is in fact exact over this grid, so a solver regression
    /// cannot hide inside the tolerance.
    #[test]
    fn hct_round_trips_across_the_rgb_cube() {
        let mut exact = 0;
        let mut samples = 0;
        for r in (0..=255).step_by(17) {
            for g in (0..=255).step_by(17) {
                for b in (0..=255).step_by(17) {
                    let color = Color::from_rgb8(r as u8, g as u8, b as u8);
                    let hct = Hct::from_color(color);
                    let round_tripped = Hct::new(hct.hue(), hct.chroma(), hct.tone()).to_color();
                    let delta = max_channel_delta(color, round_tripped);
                    assert!(
                        delta <= 1,
                        "round trip of {:06X} landed on {:06X}",
                        rgb(color),
                        rgb(round_tripped)
                    );
                    samples += 1;
                    if delta == 0 {
                        exact += 1;
                    }
                }
            }
        }
        assert_eq!(samples, 16 * 16 * 16);
        assert_eq!(exact, samples, "upstream round-trips this grid exactly");
    }

    /// Tones outside the solvable range fall back to the neutral gray of
    /// that L*, exactly as upstream does.
    #[test]
    fn extreme_tones_resolve_to_black_and_white() {
        let palette = TonalPalette::of(298.98, 48.0);
        assert_eq!(rgb(palette.tone(0.0)), 0x000000);
        assert_eq!(rgb(palette.tone(100.0)), 0xFFFFFF);
    }

    /// Full 46-role light scheme for the baseline seed, generated by running
    /// material-color-utilities 0.11.1 directly.
    #[test]
    fn from_seed_matches_mcu_golden_light() {
        assert_golden(
            BASELINE_SEED,
            Brightness::Light,
            &[
                ("primary", 0x6750A4),
                ("on_primary", 0xFFFFFF),
                ("primary_container", 0xE9DDFF),
                ("on_primary_container", 0x22005D),
                ("primary_fixed", 0xE9DDFF),
                ("primary_fixed_dim", 0xCFBCFF),
                ("on_primary_fixed", 0x22005D),
                ("on_primary_fixed_variant", 0x4F378A),
                ("secondary", 0x625B71),
                ("on_secondary", 0xFFFFFF),
                ("secondary_container", 0xE8DEF8),
                ("on_secondary_container", 0x1E192B),
                ("secondary_fixed", 0xE8DEF8),
                ("secondary_fixed_dim", 0xCBC2DB),
                ("on_secondary_fixed", 0x1E192B),
                ("on_secondary_fixed_variant", 0x4A4458),
                ("tertiary", 0x7E5260),
                ("on_tertiary", 0xFFFFFF),
                ("tertiary_container", 0xFFD9E3),
                ("on_tertiary_container", 0x31101D),
                ("tertiary_fixed", 0xFFD9E3),
                ("tertiary_fixed_dim", 0xEFB8C8),
                ("on_tertiary_fixed", 0x31101D),
                ("on_tertiary_fixed_variant", 0x633B48),
                ("error", 0xBA1A1A),
                ("on_error", 0xFFFFFF),
                ("error_container", 0xFFDAD6),
                ("on_error_container", 0x410002),
                ("surface", 0xFDF7FF),
                ("on_surface", 0x1D1B20),
                ("on_surface_variant", 0x49454E),
                ("surface_dim", 0xDED8E0),
                ("surface_bright", 0xFDF7FF),
                ("surface_container_lowest", 0xFFFFFF),
                ("surface_container_low", 0xF8F2FA),
                ("surface_container", 0xF2ECF4),
                ("surface_container_high", 0xECE6EE),
                ("surface_container_highest", 0xE6E0E9),
                ("outline", 0x7A757F),
                ("outline_variant", 0xCAC4CF),
                ("shadow", 0x000000),
                ("scrim", 0x000000),
                ("inverse_surface", 0x322F35),
                ("inverse_on_surface", 0xF5EFF7),
                ("inverse_primary", 0xCFBCFF),
                ("surface_tint", 0x6750A4),
            ],
        );
    }

    /// The dark half of the same golden run.
    #[test]
    fn from_seed_matches_mcu_golden_dark() {
        assert_golden(
            BASELINE_SEED,
            Brightness::Dark,
            &[
                ("primary", 0xCFBCFF),
                ("on_primary", 0x381E72),
                ("primary_container", 0x4F378A),
                ("on_primary_container", 0xE9DDFF),
                ("primary_fixed", 0xE9DDFF),
                ("primary_fixed_dim", 0xCFBCFF),
                ("on_primary_fixed", 0x22005D),
                ("on_primary_fixed_variant", 0x4F378A),
                ("secondary", 0xCBC2DB),
                ("on_secondary", 0x332D41),
                ("secondary_container", 0x4A4458),
                ("on_secondary_container", 0xE8DEF8),
                ("secondary_fixed", 0xE8DEF8),
                ("secondary_fixed_dim", 0xCBC2DB),
                ("on_secondary_fixed", 0x1E192B),
                ("on_secondary_fixed_variant", 0x4A4458),
                ("tertiary", 0xEFB8C8),
                ("on_tertiary", 0x4A2532),
                ("tertiary_container", 0x633B48),
                ("on_tertiary_container", 0xFFD9E3),
                ("tertiary_fixed", 0xFFD9E3),
                ("tertiary_fixed_dim", 0xEFB8C8),
                ("on_tertiary_fixed", 0x31101D),
                ("on_tertiary_fixed_variant", 0x633B48),
                ("error", 0xFFB4AB),
                ("on_error", 0x690005),
                ("error_container", 0x93000A),
                ("on_error_container", 0xFFDAD6),
                ("surface", 0x141218),
                ("on_surface", 0xE6E0E9),
                ("on_surface_variant", 0xCAC4CF),
                ("surface_dim", 0x141218),
                ("surface_bright", 0x3B383E),
                ("surface_container_lowest", 0x0F0D13),
                ("surface_container_low", 0x1D1B20),
                ("surface_container", 0x211F24),
                ("surface_container_high", 0x2B292F),
                ("surface_container_highest", 0x36343A),
                ("outline", 0x948F99),
                ("outline_variant", 0x49454E),
                ("shadow", 0x000000),
                ("scrim", 0x000000),
                ("inverse_surface", 0xE6E0E9),
                ("inverse_on_surface", 0x322F35),
                ("inverse_primary", 0x6750A4),
                ("surface_tint", 0xCFBCFF),
            ],
        );
    }

    /// Three further seeds — a blue, a green and an orange — spot-checked
    /// against the same upstream run, both brightnesses.
    #[test]
    fn from_seed_matches_mcu_goldens_for_further_seeds() {
        let blue = Color::from_rgb8(0x0B, 0x57, 0xD0);
        assert_golden(
            blue,
            Brightness::Light,
            &[
                ("primary", 0x0856CF),
                ("on_primary", 0xFFFFFF),
                ("primary_container", 0xDAE2FF),
                ("on_primary_container", 0x001847),
                ("secondary", 0x585E71),
                ("tertiary", 0x735471),
                ("surface", 0xFAF8FF),
                ("on_surface", 0x1A1B21),
                ("surface_container", 0xEEEDF4),
                ("outline", 0x757780),
                ("inverse_primary", 0xB2C5FF),
            ],
        );
        assert_golden(
            blue,
            Brightness::Dark,
            &[
                ("primary", 0xB2C5FF),
                ("on_primary", 0x002B72),
                ("primary_container", 0x0040A1),
                ("on_primary_container", 0xDAE2FF),
                ("secondary", 0xC0C6DD),
                ("tertiary", 0xE1BBDD),
                ("surface", 0x121318),
                ("on_surface", 0xE2E2E9),
                ("surface_container", 0x1E1F25),
                ("outline", 0x8F909A),
                ("inverse_primary", 0x0856CF),
            ],
        );

        let green = Color::from_rgb8(0x00, 0xA8, 0x6B);
        assert_golden(
            green,
            Brightness::Light,
            &[
                ("primary", 0x006D43),
                ("primary_container", 0x78FBB6),
                ("on_primary_container", 0x002111),
                ("secondary", 0x4E6355),
                ("tertiary", 0x3C6471),
                ("surface", 0xF6FBF4),
                ("on_surface", 0x171D19),
                ("outline", 0x717972),
            ],
        );
        assert_golden(
            green,
            Brightness::Dark,
            &[
                ("primary", 0x59DE9B),
                ("primary_container", 0x005232),
                ("on_primary_container", 0x78FBB6),
                ("secondary", 0xB5CCBB),
                ("tertiary", 0xA3CDDC),
                ("surface", 0x0F1511),
                ("on_surface", 0xDFE4DD),
                ("outline", 0x8A938B),
            ],
        );

        let orange = Color::from_rgb8(0xFF, 0x57, 0x22);
        assert_golden(
            orange,
            Brightness::Light,
            &[
                ("primary", 0xB02F00),
                ("primary_container", 0xFFDBD1),
                ("on_primary_container", 0x3B0900),
                ("secondary", 0x77574E),
                ("tertiary", 0x6C5D2F),
                ("surface", 0xFFF8F6),
                ("on_surface", 0x231917),
                ("outline", 0x85736E),
            ],
        );
        assert_golden(
            orange,
            Brightness::Dark,
            &[
                ("primary", 0xFFB5A0),
                ("primary_container", 0x862200),
                ("on_primary_container", 0xFFDBD1),
                ("secondary", 0xE7BDB2),
                ("tertiary", 0xD8C58D),
                ("surface", 0x1A110F),
                ("on_surface", 0xF1DFDA),
                ("outline", 0xA08C87),
            ],
        );
    }

    /// The error ramp is fixed upstream, so it must not move with the seed.
    #[test]
    fn the_error_ramp_is_seed_independent() {
        let (blue, _) = from_seed(Color::from_rgb8(0x0B, 0x57, 0xD0), Brightness::Light);
        let (green, _) = from_seed(Color::from_rgb8(0x00, 0xA8, 0x6B), Brightness::Light);
        assert_eq!(blue.error, green.error);
        assert_eq!(blue.error_container, green.error_container);
        assert_ne!(blue.primary, green.primary);
    }

    /// The roles where [`from_seed`] at the baseline seed differs from the
    /// hand-baked tables, as `(role, generated, baked)`. Every entry is a
    /// generator-vintage difference, not a solver bug: the baked tables are
    /// Google's *published* baseline ref palette (material-web tokens
    /// v0.192), and the generated column is what current
    /// material-color-utilities answers (pinned by the golden tests above).
    /// See the module docs.
    const LIGHT_DIVERGENCE: &[(&str, u32, u32)] = &[
        ("primary_container", 0xE9DDFF, 0xEADDFF),
        ("on_primary_container", 0x22005D, 0x21005D),
        ("primary_fixed", 0xE9DDFF, 0xEADDFF),
        ("primary_fixed_dim", 0xCFBCFF, 0xD0BCFF),
        ("on_primary_fixed", 0x22005D, 0x21005D),
        ("on_primary_fixed_variant", 0x4F378A, 0x4F378B),
        ("on_secondary_container", 0x1E192B, 0x1D192B),
        ("secondary_fixed_dim", 0xCBC2DB, 0xCCC2DC),
        ("on_secondary_fixed", 0x1E192B, 0x1D192B),
        ("tertiary", 0x7E5260, 0x7D5260),
        ("tertiary_container", 0xFFD9E3, 0xFFD8E4),
        ("on_tertiary_container", 0x31101D, 0x31111D),
        ("tertiary_fixed", 0xFFD9E3, 0xFFD8E4),
        ("on_tertiary_fixed", 0x31101D, 0x31111D),
        ("error", 0xBA1A1A, 0xB3261E),
        ("error_container", 0xFFDAD6, 0xF9DEDC),
        ("on_error_container", 0x410002, 0x410E0B),
        ("surface", 0xFDF7FF, 0xFEF7FF),
        ("on_surface_variant", 0x49454E, 0x49454F),
        ("surface_dim", 0xDED8E0, 0xDED8E1),
        ("surface_bright", 0xFDF7FF, 0xFEF7FF),
        ("surface_container_low", 0xF8F2FA, 0xF7F2FA),
        ("surface_container", 0xF2ECF4, 0xF3EDF7),
        ("surface_container_high", 0xECE6EE, 0xECE6F0),
        ("outline", 0x7A757F, 0x79747E),
        ("outline_variant", 0xCAC4CF, 0xCAC4D0),
        ("inverse_primary", 0xCFBCFF, 0xD0BCFF),
    ];

    /// The dark half of [`LIGHT_DIVERGENCE`].
    const DARK_DIVERGENCE: &[(&str, u32, u32)] = &[
        ("primary", 0xCFBCFF, 0xD0BCFF),
        ("primary_container", 0x4F378A, 0x4F378B),
        ("on_primary_container", 0xE9DDFF, 0xEADDFF),
        ("primary_fixed", 0xE9DDFF, 0xEADDFF),
        ("primary_fixed_dim", 0xCFBCFF, 0xD0BCFF),
        ("on_primary_fixed", 0x22005D, 0x21005D),
        ("on_primary_fixed_variant", 0x4F378A, 0x4F378B),
        ("secondary", 0xCBC2DB, 0xCCC2DC),
        ("secondary_fixed_dim", 0xCBC2DB, 0xCCC2DC),
        ("on_secondary_fixed", 0x1E192B, 0x1D192B),
        ("on_tertiary", 0x4A2532, 0x492532),
        ("on_tertiary_container", 0xFFD9E3, 0xFFD8E4),
        ("tertiary_fixed", 0xFFD9E3, 0xFFD8E4),
        ("on_tertiary_fixed", 0x31101D, 0x31111D),
        ("error", 0xFFB4AB, 0xF2B8B5),
        ("on_error", 0x690005, 0x601410),
        ("error_container", 0x93000A, 0x8C1D18),
        ("on_error_container", 0xFFDAD6, 0xF9DEDC),
        ("on_surface_variant", 0xCAC4CF, 0xCAC4D0),
        ("surface_container", 0x211F24, 0x211F26),
        ("surface_container_high", 0x2B292F, 0x2B2930),
        ("surface_container_highest", 0x36343A, 0x36343B),
        ("outline", 0x948F99, 0x938F99),
        ("outline_variant", 0x49454E, 0x49454F),
        ("surface_tint", 0xCFBCFF, 0xD0BCFF),
    ];

    fn assert_divergence(
        brightness: Brightness,
        baked: &ColorScheme,
        divergence: &[(&str, u32, u32)],
    ) {
        let (generated, _) = from_seed(BASELINE_SEED, brightness);
        let baked_roles = roles(baked);
        for (role, got) in roles(&generated) {
            let (_, want) = baked_roles
                .iter()
                .find(|(name, _)| *name == role)
                .expect("both schemes carry the same role set");
            match divergence.iter().find(|(name, _, _)| *name == role) {
                None => assert_eq!(
                    rgb(got),
                    rgb(*want),
                    "role `{role}` ({brightness:?}) was expected to match the baked baseline"
                ),
                Some((_, expected_generated, expected_baked)) => {
                    assert_eq!(
                        rgb(got),
                        *expected_generated,
                        "recorded divergence for `{role}` ({brightness:?}) is stale"
                    );
                    assert_eq!(rgb(*want), *expected_baked, "baked `{role}` moved");
                }
            }
        }
    }

    /// Pins the whole relationship to the hand-baked baseline: every role
    /// not listed as divergent matches it byte for byte, and every listed
    /// role still holds exactly the pair of values recorded.
    #[test]
    fn from_seed_diverges_from_the_baked_baseline_only_as_recorded() {
        assert_divergence(Brightness::Light, &color_scheme_light(), LIGHT_DIVERGENCE);
        assert_divergence(Brightness::Dark, &color_scheme_dark(), DARK_DIVERGENCE);
    }

    /// Outside the error family — a wholly different upstream ramp — the
    /// divergence is a rounding-scale one, never a visible color shift. The
    /// counts asserted here are the ones this module's docs quote.
    #[test]
    fn baseline_divergence_outside_the_error_family_is_at_most_three_steps() {
        for (brightness, baked, divergence, expected_exact, expected_one_step) in [
            (
                Brightness::Light,
                color_scheme_light(),
                LIGHT_DIVERGENCE,
                18,
                22,
            ),
            (
                Brightness::Dark,
                color_scheme_dark(),
                DARK_DIVERGENCE,
                21,
                20,
            ),
        ] {
            let (generated, _) = from_seed(BASELINE_SEED, brightness);
            let baked_roles = roles(&baked);
            let (mut exact, mut one_step) = (0, 0);
            for (role, got) in roles(&generated) {
                let (_, want) = baked_roles
                    .iter()
                    .find(|(name, _)| *name == role)
                    .expect("same role set");
                let delta = max_channel_delta(got, *want);
                if role.contains("error") {
                    continue;
                }
                assert!(
                    delta <= 3,
                    "role `{role}` ({brightness:?}) drifted more than three steps"
                );
                match delta {
                    0 => exact += 1,
                    1 => one_step += 1,
                    _ => {}
                }
            }
            assert_eq!(exact, expected_exact, "exact-match count ({brightness:?})");
            assert_eq!(
                one_step, expected_one_step,
                "single-step divergence count ({brightness:?})"
            );
            assert!(divergence.iter().any(|(name, _, _)| name.contains("error")));
        }
    }

    /// A colorless seed has no hue to work from. Upstream substitutes none —
    /// CAM16 simply reports its own answer (0° for pure black, ~209.5° for
    /// grays and white) and the chroma floor does the rest. Pinned here
    /// because it is surprising, not because it is a fallback.
    #[test]
    fn degenerate_seeds_follow_upstream_rather_than_a_substituted_hue() {
        let black = Hct::from_color(Color::BLACK);
        assert_eq!(black.hue(), 0.0);
        assert_eq!(black.chroma(), 0.0);
        let gray = Hct::from_color(Color::from_rgb8(0x80, 0x80, 0x80));
        assert!(
            (gray.hue() - 209.49382556955806).abs() < 1e-9,
            "{}",
            gray.hue()
        );
        let white = Hct::from_color(Color::WHITE);
        assert!(
            (white.hue() - 209.49195947383808).abs() < 1e-9,
            "{}",
            white.hue()
        );

        // Upstream's own output for the three (light brightness).
        assert_golden(
            Color::BLACK,
            Brightness::Light,
            &[
                ("primary", 0x984061),
                ("surface", 0xFFF8F8),
                ("on_surface", 0x22191C),
                ("outline", 0x837377),
            ],
        );
        for colorless in [Color::from_rgb8(0x80, 0x80, 0x80), Color::WHITE] {
            assert_golden(
                colorless,
                Brightness::Light,
                &[
                    ("primary", 0x006874),
                    ("surface", 0xF5FAFB),
                    ("on_surface", 0x171D1E),
                    ("outline", 0x6F797A),
                ],
            );
        }
    }

    /// Whatever the seed, a scheme has to stay usable: opaque throughout,
    /// with light surfaces light and dark surfaces dark.
    #[test]
    fn every_seed_yields_a_well_formed_scheme() {
        for seed in [
            BASELINE_SEED,
            Color::BLACK,
            Color::WHITE,
            Color::from_rgb8(0x80, 0x80, 0x80),
            Color::from_rgb8(0xFF, 0x00, 0x00),
            Color::from_rgb8(0x00, 0x00, 0x01),
        ] {
            let (light, _) = from_seed(seed, Brightness::Light);
            let (dark, _) = from_seed(seed, Brightness::Dark);
            for (role, color) in roles(&light).into_iter().chain(roles(&dark)) {
                assert_eq!(
                    color.to_rgba8().to_u8_array()[3],
                    255,
                    "role `{role}` came back translucent"
                );
            }
            assert!(
                Hct::from_color(light.surface).tone() > Hct::from_color(light.on_surface).tone(),
                "light surface must be lighter than its content"
            );
            assert!(
                Hct::from_color(dark.surface).tone() < Hct::from_color(dark.on_surface).tone(),
                "dark surface must be darker than its content"
            );
            assert_ne!(light.surface, dark.surface);
            assert_ne!(light.primary, dark.primary);
        }
    }

    /// The nine semantic roles follow the baked tables' derivation rules,
    /// seeded.
    #[test]
    fn semantic_roles_reuse_the_baked_derivation_rules() {
        let seed = Color::from_rgb8(0x00, 0xA8, 0x6B);
        let (light, tokens) = from_seed(seed, Brightness::Light);
        let (dark, _) = from_seed(seed, Brightness::Dark);

        assert_eq!(tokens.light.emphasis, light.primary);
        assert_eq!(tokens.light.on_emphasis, light.on_primary);
        assert_eq!(tokens.light.info, light.tertiary);
        assert_eq!(tokens.light.danger, light.error);
        assert_eq!(tokens.light.on_surface_strong, light.on_surface);
        assert_eq!(tokens.light.outline_strong, light.outline);
        assert_eq!(
            tokens.light.surface_strong,
            alpha_blend(light.primary, light.surface, SURFACE_STRONG_ALPHA)
        );
        assert_eq!(tokens.dark.emphasis, dark.primary);
        assert_eq!(tokens.dark.danger, dark.error);

        // The two seed-independent constants stay put.
        assert_eq!(tokens.light.success, semantic_light().success);
        assert_eq!(tokens.light.warning, semantic_light().warning);
        assert_eq!(tokens.dark.success, semantic_dark().success);
        assert_eq!(tokens.dark.warning, semantic_dark().warning);
    }

    /// Both halves come back whichever brightness was asked for, and the
    /// requested one is the one returned.
    #[test]
    fn from_seed_returns_the_requested_brightness_and_both_token_halves() {
        let seed = Color::from_rgb8(0x0B, 0x57, 0xD0);
        let (light, light_tokens) = from_seed(seed, Brightness::Light);
        let (dark, dark_tokens) = from_seed(seed, Brightness::Dark);
        assert_eq!(light_tokens, dark_tokens);
        assert_eq!(
            light_tokens.colors(Brightness::Light).emphasis,
            light.primary
        );
        assert_eq!(dark_tokens.colors(Brightness::Dark).emphasis, dark.primary);
        assert_ne!(light_tokens.light, light_tokens.dark);
    }

    #[test]
    fn theme_from_seed_keeps_the_baseline_scales_and_swaps_the_colors() {
        let seed = Color::from_rgb8(0xFF, 0x57, 0x22);
        let theme = theme_from_seed(seed, Brightness::Dark);
        let baseline = super::super::baseline();

        assert_eq!(theme.brightness, Brightness::Dark);
        assert_eq!(theme.type_scale, baseline.type_scale);
        assert_eq!(theme.shape, baseline.shape);
        assert_eq!(theme.elevation, baseline.elevation);
        assert_eq!(theme.motion, baseline.motion);
        assert_eq!(
            theme.extension::<StatusPalette>(),
            baseline.extension::<StatusPalette>()
        );

        let (light, tokens) = from_seed(seed, Brightness::Light);
        assert_eq!(theme.light, light);
        assert_eq!(theme.extension::<MaterialTokens>(), Some(&tokens));
        assert_ne!(theme.light, baseline.light);
        assert_ne!(theme.dark, baseline.dark);
    }

    /// Re-seeding a theme with the baseline seed leaves the baked baseline's
    /// own extension attached only where the two agree — the guard against
    /// silently shipping two different `MaterialTokens` for one seed.
    #[test]
    fn theme_from_seed_attaches_the_seeded_tokens_not_the_baked_ones() {
        let theme = theme_from_seed(BASELINE_SEED, Brightness::Light);
        let attached = theme
            .extension::<MaterialTokens>()
            .expect("theme_from_seed attaches MaterialTokens");
        let (_, seeded) = from_seed(BASELINE_SEED, Brightness::Light);
        assert_eq!(attached, &seeded);
        assert_eq!(attached.light.emphasis, theme.light.primary);
    }

    /// A tonal palette is monotonic in tone and honors its own hue.
    #[test]
    fn tonal_palette_tones_ascend_and_hold_their_hue() {
        let palette = TonalPalette::of(120.0, 40.0);
        let mut previous = -1.0;
        for tone in [0.0, 10.0, 20.0, 40.0, 60.0, 80.0, 90.0, 100.0] {
            let resolved = Hct::from_color(palette.tone(tone));
            assert!(
                resolved.tone() > previous,
                "tone {tone} did not ascend ({} after {previous})",
                resolved.tone()
            );
            previous = resolved.tone();
            if tone > 0.0 && tone < 100.0 {
                assert!(
                    (resolved.hue() - 120.0).abs() < 2.0,
                    "tone {tone} drifted to hue {}",
                    resolved.hue()
                );
            }
        }
    }

    /// `TonalPalette::from_color` reads a color's own ramp — the entry point
    /// a seed-picker UI uses to preview a ramp without building a scheme.
    #[test]
    fn tonal_palette_from_color_matches_the_core_primary_ramp() {
        let seed = Color::from_rgb8(0x0B, 0x57, 0xD0);
        let core = CorePalette::of(seed);
        let direct = TonalPalette::from_color(seed);
        // The seed's chroma clears the floor, so both ramps coincide.
        assert!(Hct::from_color(seed).chroma() > PRIMARY_MIN_CHROMA);
        assert_eq!(direct.tone(40.0), core.primary.tone(40.0));
    }

    #[test]
    fn core_palette_applies_the_chroma_floor_and_hue_rotation() {
        let gray = CorePalette::of(Color::from_rgb8(0x80, 0x80, 0x80));
        assert_eq!(gray.primary.chroma(), PRIMARY_MIN_CHROMA);
        let vivid = CorePalette::of(Color::from_rgb8(0xFF, 0x57, 0x22));
        assert!(vivid.primary.chroma() > PRIMARY_MIN_CHROMA);
        assert_eq!(
            vivid.tertiary.hue(),
            sanitize_degrees(vivid.primary.hue() + TERTIARY_HUE_ROTATION)
        );
        assert_eq!(vivid.error.hue(), ERROR_HUE);
    }
}
