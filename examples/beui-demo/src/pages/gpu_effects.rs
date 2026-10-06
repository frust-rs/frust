//! GPU Effects — the non-default `gpu-effects` cargo feature, live.
//!
//! Five components in this catalog carry an opt-in true-3D surface, each
//! rendered through `frust_beui::gpu_fx`'s offscreen perspective substrate
//! rather than the scene's own affine transform: [`tilt_card`]'s face,
//! [`wallet_card`]'s open account switcher, [`wheel_picker`]'s drum,
//! [`cylinder_carousel`]'s wall, and [`project_folder`]'s preview fan. This
//! page pairs each one's ordinary 2D path against its 3D opt-in, side by
//! side, plus a short showcase of `shader_background`'s own GPU tiles —
//! unconditional, since the engine's shader path is not behind this feature
//! at all.
//!
//! # The honest boundary
//!
//! The 3D path renders a **surface**, never a **subtree**: a face is a
//! colour, a ramp, or a caller-owned texture, and every component's ordinary
//! content — text, icons, avatars, captions — keeps compositing on the 2D
//! path above it, opt-in or not. There is no route from a widget's own paint
//! to a 3D face today (see `frust_beui::gpu_fx`'s module docs, "The widget
//! side" section), so a tilted card, a fanned account row or a spread
//! preview sheet still shows nothing but what its component already draws
//! for it — never arbitrary children.
//!
//! # Compiling both ways
//!
//! `cargo run -p beui-demo` (the default build) never touches `gpu_fx` at
//! all: every pane below renders its ordinary 2D path, and the would-be 3D
//! column names the flag that turns it on. `cargo run -p beui-demo --features
//! gpu-effects` compiles the five opt-ins in; whether a pane then actually
//! *renders* 3D still depends on [`GpuFx::availability`] — a device exists
//! only once a desktop shell has opened its first surface
//! (`docs/LIMITATIONS.md`'s `facade-gpu-context-desktop-only`), and
//! `FRUST_BEUI_NO_GPU_FX=1` turns the whole path off regardless of the
//! build. Every component already carries that degrade in its own contract,
//! so the "3D" pane below is always the real component call — it simply
//! paints its 2D path in the two cases above, same as it would without ever
//! calling the opt-in.

use frust::{
    AnyView, CrossAxisAlignment, Row, SizedBox, Theme, any, column, container, row, text,
    use_context,
};
use frust_beui::blocks::project_folder::{folder_preview, project_folder};
use frust_beui::blocks::wallet_card::{wallet_account, wallet_card};
use frust_beui::components::cylinder_carousel::{CylinderCarouselVariant, cylinder_carousel};
use frust_beui::components::shader_background::{ShaderBackgroundVariant, shader_background};
use frust_beui::components::tilt_card::tilt_card;
use frust_beui::components::wheel_picker::{WheelPickerOption, wheel_picker};
#[cfg(feature = "gpu-effects")]
use frust_beui::gpu_fx::{FxAvailability, GpuFx, KILL_SWITCH_ENV_VAR, QuadFace};
use frust_beui::style::with_alpha;

use crate::AppState;
use crate::nav::{caption, heading};

/// The command line every "compile this pane in" placeholder names.
#[cfg(not(feature = "gpu-effects"))]
const BUILD_FLAG_HINT: &str = "cargo run -p beui-demo --features gpu-effects";

/// The live theme, or the catalog's own baseline before a context exists.
fn theme() -> Theme {
    use_context::<Theme>().unwrap_or_else(frust_beui::theme)
}

/// A vertical spacer.
fn gap(height: f64) -> AnyView<AppState> {
    any(SizedBox(None, Some(height)))
}

/// A horizontal spacer.
fn hgap(width: f64) -> AnyView<AppState> {
    any(SizedBox(Some(width), None))
}

/// A muted line of small print.
fn muted(body: impl Into<String>) -> AnyView<AppState> {
    any(caption(body.into()).color(theme().scheme().on_surface_variant))
}

/// A placeholder pane, shown when the crate was not compiled with
/// `gpu-effects` at all. Sized to roughly match its 2D sibling so the pair
/// still reads as a pair.
#[cfg(not(feature = "gpu-effects"))]
fn build_flag_placeholder(width: f64, height: f64, component: &str) -> AnyView<AppState> {
    any(container(
        column()
            .child(text(format!("{component} \u{b7} 3D")).size(13.0))
            .child(gap(6.0))
            .child(muted(format!("Rebuild with `{BUILD_FLAG_HINT}`.")))
            .cross_axis(CrossAxisAlignment::Center),
    )
    .size_centered(width, height)
    .radius(12.0)
    .border(theme().scheme().outline, 1.0))
}

/// One 2D/3D pair: a title, a note, and the two panes side by side under
/// their own "2D"/"3D (gpu-effects)" captions.
fn pair(
    title: &str,
    note: &str,
    two_d: AnyView<AppState>,
    three_d: AnyView<AppState>,
) -> AnyView<AppState> {
    any(column()
        .child(text(title.to_string()).size(16.0))
        .child(gap(4.0))
        .child(muted(note.to_string()))
        .child(gap(12.0))
        .child(
            row()
                .child(
                    column()
                        .child(caption("2D"))
                        .child(gap(8.0))
                        .child(two_d)
                        .cross_axis(CrossAxisAlignment::Start),
                )
                .child(hgap(40.0))
                .child(
                    column()
                        .child(caption("3D \u{b7} gpu-effects"))
                        .child(gap(8.0))
                        .child(three_d)
                        .cross_axis(CrossAxisAlignment::Start),
                )
                .cross_axis(CrossAxisAlignment::Start),
        )
        .child(gap(32.0))
        .cross_axis(CrossAxisAlignment::Start))
}

// ---- tilt_card --------------------------------------------------------

/// The tilt card's static body: it needs no state, only a pointer.
fn tilt_body(label: &str) -> AnyView<AppState> {
    any(SizedBox(Some(220.0), Some(140.0)).child(
        column()
            .child(gap(16.0))
            .child(text(label.to_string()).size(15.0))
            .child(gap(8.0))
            .child(muted("Hover to lean."))
            .cross_axis(CrossAxisAlignment::Start),
    ))
}

#[cfg(feature = "gpu-effects")]
fn tilt_pane_3d() -> AnyView<AppState> {
    let theme = theme();
    let scheme = theme.scheme();
    any(tilt_card(tilt_body("Tilt card"))
        .shadow(true)
        .gpu_face(QuadFace::Gradient {
            from: scheme.primary_container,
            to: scheme.tertiary_container,
            angle_radians: std::f32::consts::FRAC_PI_4,
        }))
}

#[cfg(not(feature = "gpu-effects"))]
fn tilt_pane_3d() -> AnyView<AppState> {
    build_flag_placeholder(220.0, 140.0, "tilt_card")
}

fn tilt_pair() -> AnyView<AppState> {
    let two_d = any(tilt_card(tilt_body("Tilt card")).shadow(true));
    pair(
        "tilt_card",
        "The 2D pane leans on the scene's one transform primitive — an affine \
         shadow of upstream's two rotations, cos\u{3b8} foreshortening plus a shear. \
         The 3D pane replaces the card's surface (not its text) with a genuinely \
         projected face, so the near corner actually grows into view.",
        two_d,
        tilt_pane_3d(),
    )
}

// ---- wheel_picker -------------------------------------------------------

/// A dozen frozen rows — the picker is inert here (`disabled`) so both panes
/// hold still at the same value rather than drifting apart under a drag this
/// page does not track.
fn wheel_options() -> Vec<WheelPickerOption> {
    (1..=12)
        .map(|n| WheelPickerOption::from(n.to_string().as_str()))
        .collect()
}

#[cfg(feature = "gpu-effects")]
fn wheel_pane_3d() -> AnyView<AppState> {
    any(SizedBox(Some(140.0), None).child(
        wheel_picker(wheel_options(), "6", |_: &mut AppState, _| {})
            .label("Row (3D)")
            .disabled(true)
            .gpu_drum(true),
    ))
}

#[cfg(not(feature = "gpu-effects"))]
fn wheel_pane_3d() -> AnyView<AppState> {
    build_flag_placeholder(140.0, 180.0, "wheel_picker")
}

fn wheel_pair() -> AnyView<AppState> {
    let two_d = any(SizedBox(Some(140.0), None).child(
        wheel_picker(wheel_options(), "6", |_: &mut AppState, _| {})
            .label("Row (2D)")
            .disabled(true),
    ));
    pair(
        "wheel_picker",
        "The 2D drum paints each row's curved offset and shrink from a lookup \
         table. The 3D pane seats the same rows on a genuinely projected \
         cylinder, behind the drum's own text.",
        two_d,
        wheel_pane_3d(),
    )
}

// ---- cylinder_carousel ---------------------------------------------------

/// The carousel's items — six short labels, matching the catalog's own
/// contact-sheet scale elsewhere in this gallery.
const CAROUSEL_ITEMS: [&str; 6] = ["Alpha", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot"];

fn carousel_items() -> Vec<AnyView<AppState>> {
    CAROUSEL_ITEMS
        .iter()
        .map(|label| {
            any(column()
                .child(text(label.to_string()).size(14.0))
                .cross_axis(CrossAxisAlignment::Center))
        })
        .collect()
}

#[cfg(feature = "gpu-effects")]
fn carousel_pane_3d() -> AnyView<AppState> {
    any(SizedBox(Some(440.0), None).child(
        cylinder_carousel(carousel_items())
            .variant(CylinderCarouselVariant::Concave)
            .item_size(110.0)
            .visible_items(5)
            .height(150.0)
            .snap(true)
            .default_index(0)
            .gpu_cylinder(true),
    ))
}

#[cfg(not(feature = "gpu-effects"))]
fn carousel_pane_3d() -> AnyView<AppState> {
    build_flag_placeholder(440.0, 150.0, "cylinder_carousel")
}

fn carousel_pair() -> AnyView<AppState> {
    let two_d = any(SizedBox(Some(440.0), None).child(
        cylinder_carousel(carousel_items())
            .variant(CylinderCarouselVariant::Concave)
            .item_size(110.0)
            .visible_items(5)
            .height(150.0)
            .snap(true)
            .default_index(0),
    ));
    pair(
        "cylinder_carousel",
        "The 2D wall scales and dims each item by a lookup curve as it leaves \
         centre. The 3D pane lines the same wall with genuinely projected \
         plates, each item's own label still painting flat above it. Drag or \
         scroll either one — the position is this page's own.",
        two_d,
        carousel_pane_3d(),
    )
}

// ---- project_folder -------------------------------------------------------

fn folder_previews() -> Vec<frust_beui::blocks::project_folder::ProjectFolderPreview> {
    ["Moodboard", "Type study", "Palette", "Layouts"]
        .into_iter()
        .map(folder_preview)
        .collect()
}

#[cfg(feature = "gpu-effects")]
fn folder_pane_3d() -> AnyView<AppState> {
    any(project_folder("Case study (3D)", folder_previews())
        .item_label("file")
        .open(true)
        .gpu_fan(true))
}

#[cfg(not(feature = "gpu-effects"))]
fn folder_pane_3d() -> AnyView<AppState> {
    build_flag_placeholder(288.0, 224.0, "project_folder")
}

fn folder_pair() -> AnyView<AppState> {
    let two_d = any(project_folder("Case study (2D)", folder_previews())
        .item_label("file")
        .open(true));
    pair(
        "project_folder",
        "Both panes are forced open so the fan shows without a hover. The 2D \
         pane spreads flat cards; the 3D pane spreads genuinely projected \
         plates, each caption still painting flat above its own sheet.",
        two_d,
        folder_pane_3d(),
    )
}

// ---- wallet_card ------------------------------------------------------

fn wallet_accounts() -> Vec<frust_beui::blocks::wallet_card::WalletAccount> {
    vec![
        wallet_account(
            "main",
            "Main Wallet",
            "0x8f3Cb1a29e4D7c6F1B2a3E9d0C4b5A6f7D8e9C0b",
        ),
        wallet_account(
            "trading",
            "Trading",
            "0x1a2B3c4D5e6F7a8B9c0D1e2F3a4B5c6D7e8F9a0B",
        ),
    ]
}

#[cfg(feature = "gpu-effects")]
fn wallet_pane_3d() -> AnyView<AppState> {
    any(wallet_card(wallet_accounts(), 4_820.5)
        .account_id("main")
        .gpu_fan(true))
}

#[cfg(not(feature = "gpu-effects"))]
fn wallet_pane_3d() -> AnyView<AppState> {
    build_flag_placeholder(320.0, 220.0, "wallet_card")
}

fn wallet_pair() -> AnyView<AppState> {
    let two_d = any(wallet_card(wallet_accounts(), 4_820.5).account_id("main"));
    pair(
        "wallet_card",
        "Click the account row to open the switcher on either pane. The 2D \
         rows sit in a flat stack; the 3D pane fans the open rows in real \
         depth, avatars and addresses still painting flat above each one.",
        two_d,
        wallet_pane_3d(),
    )
}

// ---- shader_background ---------------------------------------------------

/// One shader tile — always the engine's own GPU path, feature or not.
fn shader_tile(variant: ShaderBackgroundVariant, over_card: bool) -> AnyView<AppState> {
    let background = shader_background::<AppState>(variant)
        .animate(variant.animates())
        .speed(0.4);
    let mut frame = container(background)
        .size_centered(170.0, 110.0)
        .radius(12.0);
    if over_card {
        frame = frame.fill(with_alpha(theme().scheme().tertiary, 0.35));
    }
    any(frame)
}

fn shader_showcase() -> AnyView<AppState> {
    let tiles = [
        (ShaderBackgroundVariant::MeshGradient, false),
        (ShaderBackgroundVariant::Waves, false),
        (ShaderBackgroundVariant::DotGrid, true),
    ];
    let mut row: Vec<AnyView<AppState>> = Vec::new();
    for (index, (variant, over_card)) in tiles.into_iter().enumerate() {
        if index > 0 {
            row.push(hgap(16.0));
        }
        row.push(any(column()
            .child(shader_tile(variant, over_card))
            .child(gap(6.0))
            .child(caption(variant.label()))
            .cross_axis(CrossAxisAlignment::Start)));
    }

    any(column()
        .child(text("shader_background").size(16.0))
        .child(gap(4.0))
        .child(muted(
            "Not behind `gpu-effects` at all — every tile here is a \
                 Command::ShaderQuad the engine renders unconditionally. Two of \
                 the three carry a clock (mesh-gradient, waves); the third \
                 (dot-grid) is a static, transparent-backdrop pattern shown here \
                 over a tinted card to demonstrate the premultiplied composite. \
                 The full five-variant contact sheet lives on Motion \u{b7} Shader.",
        ))
        .child(gap(12.0))
        .child(Row(row).cross_axis(CrossAxisAlignment::Start))
        .cross_axis(CrossAxisAlignment::Start))
}

// ---- availability ----------------------------------------------------

#[cfg(feature = "gpu-effects")]
fn availability_note() -> String {
    match GpuFx::availability() {
        FxAvailability::Ready => {
            "Compiled with `gpu-effects` and a GPU device is reachable: every 3D \
             pane below renders its true-3D surface."
                .to_string()
        }
        FxAvailability::Disabled => format!(
            "Compiled with `gpu-effects`, but {KILL_SWITCH_ENV_VAR}=1 is set: every \
             3D pane below degrades to its ordinary 2D path."
        ),
        FxAvailability::NoDevice => {
            "Compiled with `gpu-effects`, but no desktop shell has opened its \
             first surface yet, so no GPU device is reachable: every 3D pane \
             below degrades to its ordinary 2D path until one has."
                .to_string()
        }
    }
}

#[cfg(not(feature = "gpu-effects"))]
fn availability_note() -> String {
    format!("Compiled without `gpu-effects` \u{2014} rebuild with `{BUILD_FLAG_HINT}` to try it.")
}

/// The GPU Effects page.
pub fn page() -> AnyView<AppState> {
    any(column()
        .child(heading("GPU Effects"))
        .child(gap(8.0))
        .child(muted(
            "Five components carry an opt-in true-3D surface behind the \
                 non-default `gpu-effects` cargo feature \u{2014} real perspective \
                 through an offscreen render the engine composites, rather than \
                 the scene's affine transform. Each pair below is the same \
                 component twice: its ordinary 2D path, and the 3D opt-in.",
        ))
        .child(gap(6.0))
        .child(muted(availability_note()))
        .child(gap(28.0))
        .child(tilt_pair())
        .child(wheel_pair())
        .child(carousel_pair())
        .child(folder_pair())
        .child(wallet_pair())
        .child(gap(4.0))
        .child(shader_showcase())
        .cross_axis(CrossAxisAlignment::Start))
}
