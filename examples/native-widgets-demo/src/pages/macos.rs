//! **macOS** — the AppKit arm, stated and shown. The other pages already mount
//! AppKit views when this app runs as a macOS desktop build; this page is the
//! one place that says what each builder becomes there, which families
//! refuse, and which documented AppKit gaps a gate should expect.
//!
//! On macOS the class map below is live (every other page's left column is
//! those views). On iOS/Android/a desktop preview it reads as a reference,
//! and the one live demonstration — the `Cover`/`Contain` image pair — shows
//! *this* platform's behavior for contrast: iOS and Android crop `Cover`,
//! macOS letterboxes it (`native-widgets-macos-image-cover-letterboxes`).
//!
//! Two native slots at rest: the image pair.

use frust::{AnyView, any, inflexible};
use frust_native_widgets::{NativeImageFit, native_image};

use super::common::{
    CellFit, PAIR_CELL_W, PAIR_GAP, S, block, caption, cell, demo_image_bytes, gap, gap_h, label,
    page_column, page_header, readout, row,
};

/// This page's index in [`SECTION_LABELS`](crate::SECTION_LABELS).
const SECTION: usize = 1;

/// Whether this build is the AppKit arm.
const ON_MACOS: bool = cfg!(target_os = "macos");

/// The image pair's cell width — wider than tall, so a letterboxed square
/// image shows visible side bands.
const IMAGE_W: f64 = PAIR_CELL_W;
/// The image pair's height: shorter than wide, so `Cover` must crop (or, on
/// macOS, letterbox) the square demo image.
const IMAGE_H: f64 = 64.0;

/// Every builder and what it mounts on macOS, in the plugin README's order.
const APPKIT_MAP: [(&str, &str); 11] = [
    ("native_button", "NSButton"),
    ("native_label", "NSTextField (label)"),
    ("native_switch", "NSSwitch"),
    ("native_slider", "NSSlider"),
    ("native_progress", "NSProgressIndicator"),
    ("native_image", "NSImageView"),
    ("native_spinner", "NSProgressIndicator (Spinning style)"),
    ("native_date_picker", "NSDatePicker (year/month/day)"),
    ("native_segmented", "NSSegmentedControl (select-one)"),
    ("native_stepper", "NSStepper"),
    (
        "native_tab_bar",
        "none - a frust-drawn refusal banner (no bottom-tab idiom)",
    ),
];

/// The documented AppKit gaps a macOS gate should expect, each with its
/// `docs/LIMITATIONS.md` id or plugin README section.
const APPKIT_GAPS: [&str; 7] = [
    "Image Cover letterboxes instead of cropping \
     (native-widgets-macos-image-cover-letterboxes) - see the live pair below.",
    "A slider reports values only - no drag-start/drag-end edges \
     (native-widgets-macos-slider-no-drag-edges).",
    "NSSwitch track, NSProgressIndicator fill, the spinner, NSStepper and NSDatePicker have \
     no tint API - those theme tints are logged no-ops; they draw in the system accent color.",
    "Every control's NSAppearance follows the app's Theme brightness, not the Mac's \
     appearance setting.",
    "A shell whose DYLD_LIBRARY_PATH shadows an ImageIO codec leaves image slots empty with \
     one warning (native-widgets-macos-imageio-dyld-shadow) - launch with it unset.",
    "Alerts are an NSAlert window sheet on the key window: Return picks the first action, \
     Escape the Cancel-role one; no click-away, so Cancelled never occurs; ActionSheet style \
     and the anchor are ignored.",
    "Native sheets are Unsupported on macOS (an NSPopover arm is follow-up work) - the Sheet \
     page's buttons report it.",
];

/// See the page-fn contract in [`crate::pages`] and the [module docs](self).
pub fn page(_state: &S) -> AnyView<S> {
    let status = if ON_MACOS {
        "You are on the AppKit arm: every other page's left column is these AppKit views."
    } else {
        "This build is not macOS: the map below is a reference for the AppKit arm; the image \
         pair shows this platform's own behavior for contrast."
    };

    let mut map_rows = vec![
        inflexible(label("What each builder mounts on macOS")),
        gap(4.0),
        inflexible(caption(status)),
        gap(6.0),
    ];
    for (builder, class) in APPKIT_MAP {
        map_rows.push(inflexible(readout(format!("{builder} \u{2192} {class}"))));
    }

    let mut gap_rows = vec![inflexible(label("Documented AppKit gaps")), gap(6.0)];
    for note in APPKIT_GAPS {
        gap_rows.push(inflexible(caption(format!("\u{2022} {note}"))));
        gap_rows.push(gap(4.0));
    }

    let image_pair = block(vec![
        inflexible(label("Cover vs Contain (live, native both sides)")),
        gap(4.0),
        inflexible(caption(if ON_MACOS {
            "Expected here: BOTH letterbox - AppKit has no fill-and-crop scaling."
        } else {
            "Expected here: Cover (left) fills and crops; Contain (right) letterboxes. On macOS \
             both letterbox."
        })),
        gap(6.0),
        inflexible(row(vec![
            inflexible(cell(
                CellFit::Stretch,
                IMAGE_W,
                IMAGE_H,
                any(native_image(demo_image_bytes())
                    .fit(NativeImageFit::Cover)
                    .content_description("Demo image, Cover fit")
                    .size(IMAGE_W, IMAGE_H)),
            )),
            gap_h(PAIR_GAP),
            inflexible(cell(
                CellFit::Stretch,
                IMAGE_W,
                IMAGE_H,
                any(native_image(demo_image_bytes())
                    .fit(NativeImageFit::Contain)
                    .content_description("Demo image, Contain fit")
                    .size(IMAGE_W, IMAGE_H)),
            )),
        ])),
        gap(4.0),
        inflexible(caption("Left: Cover. Right: Contain.")),
    ]);

    page_column(vec![
        page_header(SECTION),
        block(map_rows),
        block(gap_rows),
        image_pair,
    ])
}
