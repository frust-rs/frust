//! **Controls** — the six base builders (`native_button`/`native_label`/
//! `native_switch`/`native_slider`/`native_progress`/`native_image`), each
//! beside its frust-drawn counterpart. Android mounts `android.widget` views,
//! iOS UIKit views, macOS AppKit views; each pair's caption names the class
//! per platform.
//!
//! # Side by side: the page's whole argument
//!
//! Every pair is the REAL platform control on the left and frust's own themed
//! widget on the right, in a cell of **exactly the same size**, under **the
//! same [`Theme`]**, bound to **the same signal**. A person holding the device
//! sees a real `Switch`'s thumb travel or a real `UISlider`'s feel against the
//! drawn equivalent rather than being told about it. Because both columns
//! read and write the same signals ([`confirm_switch`]/[`confirm_slider`]
//! serve both), dragging the drawn slider also moves the native slider and
//! the native progress bar (frust → native), and the readout block closes
//! the other direction (native event → signal → frust `Text`).
//!
//! # Write-back affordance (the rejecting round trip)
//!
//! With `REJECT write-back` on, both confirmers write the app's OWN unchanged
//! value back during the change handler instead of accepting the request —
//! the re-entrancy scenario where a platform setter called from inside its
//! own change handler can re-enter it. `Switch events` must advance by exactly
//! one per toggle; two would be a re-entrant echo.
//!
//! A plain "echo the same value" never reaches the platform: the app value did
//! not change, so the builder's params are byte-identical, the platform-view
//! differ emits no update, and the control's plan never sees
//! `observed != app value` — the control keeps the user's optimistic flip
//! while the app believes it refused. So while rejecting, and only then, each
//! control's `content_description` carries a monotonically increasing refusal
//! count ([`switch_description`]/[`slider_description`]): a real app-owned
//! prop that moves the wire, which is what lets the write-back setter run.
//! Accept mode's params are untouched.
//!
//! `content_description` is the accessibility label, so this is a **labeled,
//! test-only affordance**: `checked`/`value` are the values under test,
//! `enabled` breaks the interaction mid-gesture, the tints are what the theme
//! toggle on this page already drives, and a slider's `min`/`max` visibly
//! moves the thumb — the label is the only prop that disturbs nothing else
//! shown here. A real app must never put telemetry in an accessibility label.
//!
//! The drawn column rejects too, and snaps back within the frame (its
//! `rebuild` re-applies the app value), while the native control is driven
//! back across the wire a frame later — the instructive contrast.
//!
//! # Six native slots at rest
//!
//! This page mounts exactly six `platform_view` slots — its entry in
//! [`AT_REST_SLOTS`](super::common::AT_REST_SLOTS), the number the header's
//! live readout settles to. An extra native demo belongs behind a toggle or
//! on another page.
//!
//! # Theme ladder
//!
//! [`theme_toggle_demo`] mirrors the app bar's brightness toggle: the builders
//! read `use_context::<Theme>()` every rebuild and fold the resolved tokens
//! (colors, radius, text size) into their params, so a `set_app_theme` write
//! re-themes every mounted native view through the ordinary update path, no
//! remount. The platform-default chrome an Android control resolves against
//! its construction-time night-qualified `Context` stays pinned to the
//! brightness it was created under (a documented approximation); on macOS
//! every control's `NSAppearance` follows the app theme, not the Mac.

use frust::{
    AnyView, Brightness, Get, GetUntracked, Image, ImageFit, Set, SizedBox, any, button, checkbox,
    inflexible, slider, text,
};
use frust_glyph::{progress, toggle};
use frust_native_widgets::{
    NativeImageFit, native_button, native_image, native_label, native_progress, native_slider,
    native_switch,
};

use super::common::{
    CellFit, NativeClasses, PAIR_CELL_W, S, block, bump, caption, demo_image, demo_image_bytes,
    gap, label, local_sig, page_column, page_header, pair_row, readout,
};

/// This page's index in [`SECTION_LABELS`](crate::SECTION_LABELS).
const SECTION: usize = 0;

local_sig!(tap_count_sig, u32, 0);
local_sig!(switch_checked_sig, bool, false);
local_sig!(slider_value_sig, i32, 30);
// Native EVENT counters, distinct from the values above (which a refusal
// deliberately leaves untouched): one interaction must advance exactly one.
local_sig!(switch_events_sig, u32, 0);
local_sig!(slider_events_sig, u32, 0);
// The drawn button's own counter — `tap_count_sig` counts native presses only.
local_sig!(drawn_tap_count_sig, u32, 0);
local_sig!(reject_writeback_sig, bool, false);
local_sig!(switch_refused_sig, u32, 0);
local_sig!(slider_refused_sig, u32, 0);

/// Per-control cell heights — the box BOTH columns of a row get, so a pair
/// reads as one control shown twice. The playground page's heights, kept so
/// touch targets match every earlier gate.
const PAIR_BUTTON_H: f64 = 48.0;
/// See [`PAIR_BUTTON_H`].
const PAIR_LABEL_H: f64 = 32.0;
/// See [`PAIR_BUTTON_H`].
const PAIR_SWITCH_H: f64 = 40.0;
/// See [`PAIR_BUTTON_H`].
const PAIR_SLIDER_H: f64 = 40.0;
/// See [`PAIR_BUTTON_H`].
const PAIR_PROGRESS_H: f64 = 24.0;
/// The image pair's square edge.
const PAIR_IMAGE: f64 = 96.0;

// ---------------------------------------------------------------------------
// Write-back affordance
// ---------------------------------------------------------------------------

/// Confirm (or refuse) one requested switch value, from EITHER column. Accept
/// is the plain controlled-component confirmation; reject writes the app's
/// own value back and bumps [`switch_refused_sig`], which is what puts the
/// refusal on the wire at all. Untracked reads: this runs in an event
/// handler, never in `build`.
fn confirm_switch(requested: bool) {
    let value = switch_checked_sig();
    if reject_writeback_sig().get_untracked() {
        bump(switch_refused_sig());
        value.set(value.get_untracked());
    } else {
        value.set(requested);
    }
}

/// [`confirm_switch`]'s slider twin over the app-space `0..=100` value.
fn confirm_slider(requested: i32) {
    let value = slider_value_sig();
    if reject_writeback_sig().get_untracked() {
        bump(slider_refused_sig());
        value.set(value.get_untracked());
    } else {
        value.set(requested.clamp(0, 100));
    }
}

/// The native switch's accessibility label, carrying the refusal count only
/// while rejecting (module doc's *Write-back affordance*).
fn switch_description(rejecting: bool, refused: u32) -> String {
    if rejecting {
        format!("Native round-trip switch \u{2014} write-back REJECT, {refused} refused")
    } else {
        "Native round-trip switch".to_string()
    }
}

/// [`switch_description`]'s slider twin.
fn slider_description(rejecting: bool, refused: u32) -> String {
    if rejecting {
        format!("Native round-trip slider \u{2014} write-back REJECT, {refused} refused")
    } else {
        "Native round-trip slider".to_string()
    }
}

// ---------------------------------------------------------------------------
// The six pairs
// ---------------------------------------------------------------------------

/// A native button: each tap bumps [`tap_count_sig`] (`Taps:`).
fn button_native() -> AnyView<S> {
    any(native_button("Tap me")
        .content_description("Native tap counter button")
        .size(PAIR_CELL_W, PAIR_BUTTON_H)
        .on_press(|| bump(tap_count_sig())))
}

/// The drawn button, counting into its OWN signal.
fn button_drawn() -> AnyView<S> {
    any(button("Tap me", |_: &mut S| bump(drawn_tap_count_sig())))
}

/// A display-only native label.
fn label_native() -> AnyView<S> {
    any(native_label("Native Label")
        .content_description("A display-only native label")
        .size(PAIR_CELL_W, PAIR_LABEL_H))
}

/// frust's own shaped text — where the type-scale difference shows.
fn label_drawn() -> AnyView<S> {
    any(text("Drawn Label").size(13.0))
}

/// A controlled native switch: `on_toggle` only reports a *requested* value
/// (`docs/CODE_STANDARDS.md`'s Interaction Semantics), which
/// [`confirm_switch`] confirms or refuses.
fn switch_native(checked: bool, rejecting: bool, refused: u32) -> AnyView<S> {
    any(native_switch(checked)
        .content_description(switch_description(rejecting, refused))
        .size(70.0, PAIR_SWITCH_H)
        .on_toggle(|requested| {
            bump(switch_events_sig());
            confirm_switch(requested);
        }))
}

/// Glyph's toggle on the very same signal.
fn switch_drawn(checked: bool) -> AnyView<S> {
    any(toggle(checked, |_: &mut S, requested: bool| {
        confirm_switch(requested)
    })
    .label("Drawn switch"))
}

/// A controlled native slider on the shared `0..=100` value.
fn slider_native(value: i32, rejecting: bool, refused: u32) -> AnyView<S> {
    any(native_slider(value, 0, 100)
        .content_description(slider_description(rejecting, refused))
        .size(PAIR_CELL_W, PAIR_SLIDER_H)
        .on_change(|requested| {
            bump(slider_events_sig());
            confirm_slider(requested);
        }))
}

/// frust's slider on the same signal — `0.0..=1.0`, so this pair converts.
fn slider_drawn(value: i32) -> AnyView<S> {
    any(slider(
        f64::from(value) / 100.0,
        |_: &mut S, requested: f64| confirm_slider((requested * 100.0).round() as i32),
    ))
}

/// A native progress bar mirroring the slider (native → signal → native).
fn progress_native(value: i32) -> AnyView<S> {
    any(native_progress(value, 0, 100)
        .content_description("Progress mirroring the slider above")
        .size(PAIR_CELL_W, PAIR_PROGRESS_H))
}

/// Glyph's determinate progress bar on the same value.
fn progress_drawn(value: i32) -> AnyView<S> {
    any(progress(f64::from(value) / 100.0))
}

/// A native image, cover-fit (macOS letterboxes instead of cropping —
/// `native-widgets-macos-image-cover-letterboxes`).
fn image_native() -> AnyView<S> {
    any(native_image(demo_image_bytes())
        .fit(NativeImageFit::Cover)
        .content_description("Demo image, rendered by a native image view")
        .size(PAIR_IMAGE, PAIR_IMAGE))
}

/// The SAME bytes through frust's own decoder, same size and fit.
fn image_drawn() -> AnyView<S> {
    any(SizedBox(Some(PAIR_IMAGE), Some(PAIR_IMAGE))
        .child(Image(demo_image()).fit(ImageFit::Cover)))
}

/// The page-local brightness toggle: flips the shell's `brightness` signal and
/// forces the app theme through [`crate::apply_theme`] — the app bar's own
/// mechanism.
fn theme_toggle_demo(brightness: Brightness) -> AnyView<S> {
    let label_text = match brightness {
        // Plain text: Glyph's bundled faces carry no half-circle glyph.
        Brightness::Dark => "Dark \u{2014} tap for Light",
        Brightness::Light => "Light \u{2014} tap for Dark",
    };
    any(button(label_text, |state: &mut S| {
        let next = match state.brightness.get_untracked() {
            Brightness::Dark => Brightness::Light,
            Brightness::Light => Brightness::Dark,
        };
        state.brightness.set(next);
        crate::apply_theme(next);
    }))
}

/// See the page-fn contract in [`crate::pages`] and the [module docs](self).
pub fn page(state: &S) -> AnyView<S> {
    // Tracked reads: a native listener's signal write is not a frust event
    // pass, so it only wakes this page because the page subscribed here.
    let taps = tap_count_sig().get();
    let checked = switch_checked_sig().get();
    let slider_value = slider_value_sig().get();
    let brightness = state.brightness.get();
    let switch_events = switch_events_sig().get();
    let slider_events = slider_events_sig().get();
    let drawn_taps = drawn_tap_count_sig().get();
    let rejecting = reject_writeback_sig().get();
    let switch_refused = switch_refused_sig().get();
    let slider_refused = slider_refused_sig().get();

    let intro = block(vec![
        inflexible(caption(
            "EVERY ROW IS A PAIR: on the LEFT the real platform control, on the RIGHT frust's \
             own drawn widget \u{2014} same cell size, same theme, same signal. Flip either \
             switch, drag either slider: both columns share one value, so a drawn-side change \
             drives the native control too (frust \u{2192} native) and a native change drives \
             the readout (native \u{2192} frust).",
        )),
        gap(4.0),
        inflexible(caption(
            "No native host exists in a Linux/Windows desktop preview, so the left column is \
             empty there.",
        )),
    ]);

    let theme_block = block(vec![
        inflexible(label("Theme ladder (brightness + tokens)")),
        gap(6.0),
        inflexible(caption(
            "Flip light/dark and watch BOTH columns re-theme live \u{2014} colors, corner \
             radius and tints ride the same update path as any other prop, with no remount. \
             Android's platform-default chrome (ripple, resting thumb) stays pinned to the \
             brightness a control was created under; macOS follows the app theme, not the Mac.",
        )),
        gap(6.0),
        inflexible(theme_toggle_demo(brightness)),
    ]);

    let writeback_block = block(vec![
        inflexible(label("Write-back mode (the rejecting round trip)")),
        gap(6.0),
        inflexible(caption(
            "OFF (default) = ACCEPT: the app confirms whatever the control reports. ON = \
             REJECT: during the change handler the app echoes its OWN unchanged value back. \
             Flip it on, then drag the Slider or flip the Switch in EITHER column: the drawn \
             widget snaps back within the frame, the native one is driven back a frame later. \
             \u{201c}Switch events\u{201d} must advance by exactly 1 per toggle \u{2014} 2 would \
             be a re-entrant echo.",
        )),
        gap(6.0),
        inflexible(checkbox(
            rejecting,
            "REJECT write-back",
            |_: &mut S, on: bool| reject_writeback_sig().set(on),
        )),
        gap(4.0),
        inflexible(readout(format!(
            "Write-back: {} \u{2014} refused so far: {switch_refused} switch, {slider_refused} \
             slider",
            if rejecting { "REJECT" } else { "ACCEPT" }
        ))),
        gap(4.0),
        inflexible(caption(
            "TEST-ONLY: while REJECT is on, the refusal count also rides each control's \
             accessibility label (`content_description`), the only prop that moves the wire \
             without disturbing something else this page shows. A screen reader would \
             announce it \u{2014} never do this in a real app.",
        )),
    ]);

    let pairs = [
        pair_row(
            "Button",
            NativeClasses {
                android: "Button",
                ios: "UIButton",
                macos: "NSButton",
            },
            "Native taps feed `Taps:`; the drawn one counts separately.",
            CellFit::Stretch,
            PAIR_BUTTON_H,
            button_native(),
            button_drawn(),
        ),
        pair_row(
            "Label",
            NativeClasses {
                android: "TextView",
                ios: "UILabel",
                macos: "NSTextField (label)",
            },
            "A platform label against frust's own shaped text, same box.",
            CellFit::Stretch,
            PAIR_LABEL_H,
            label_native(),
            label_drawn(),
        ),
        pair_row(
            "Switch",
            NativeClasses {
                android: "Switch",
                ios: "UISwitch",
                macos: "NSSwitch",
            },
            "One shared value. Natural size, not stretched \u{2014} a platform Switch draws \
             its graphic at the right edge of an over-wide frame.",
            CellFit::Natural,
            PAIR_SWITCH_H,
            switch_native(checked, rejecting, switch_refused),
            switch_drawn(checked),
        ),
        pair_row(
            "Slider",
            NativeClasses {
                android: "SeekBar",
                ios: "UISlider",
                macos: "NSSlider",
            },
            "One shared value, `0..=100` native / `0.0..=1.0` drawn. Drag either.",
            CellFit::Stretch,
            PAIR_SLIDER_H,
            slider_native(slider_value, rejecting, slider_refused),
            slider_drawn(slider_value),
        ),
        pair_row(
            "ProgressBar (mirrors the slider)",
            NativeClasses {
                android: "ProgressBar",
                ios: "UIProgressView",
                macos: "NSProgressIndicator",
            },
            "Display-only on both sides \u{2014} one drag drives native \u{2192} signal \
             \u{2192} native and \u{2192} drawn.",
            CellFit::Stretch,
            PAIR_PROGRESS_H,
            progress_native(slider_value),
            progress_drawn(slider_value),
        ),
        pair_row(
            "Image",
            NativeClasses {
                android: "ImageView",
                ios: "UIImageView",
                macos: "NSImageView",
            },
            "The same embedded PNG bytes, cover-fit: platform decoder vs frust's own. macOS \
             letterboxes Cover instead of cropping.",
            CellFit::Natural,
            PAIR_IMAGE,
            image_native(),
            image_drawn(),
        ),
    ];

    // `Taps:`/`Switch:`/`Slider:` are the device gate's machine-readable
    // outputs; their spelling is load-bearing.
    let readout_block = block(vec![
        inflexible(label(
            "Round-trip readout (native event \u{2192} signal \u{2192} frust)",
        )),
        gap(4.0),
        inflexible(readout(format!("Taps: {taps}"))),
        inflexible(readout(format!(
            "Switch: {}",
            if checked { "ON" } else { "OFF" }
        ))),
        inflexible(readout(format!("Slider: {slider_value}"))),
        gap(4.0),
        inflexible(readout(format!("Switch events: {switch_events}"))),
        inflexible(readout(format!("Slider events: {slider_events}"))),
        inflexible(readout(format!("Drawn taps: {drawn_taps}"))),
        gap(4.0),
        inflexible(caption(
            "The three values are the APP's confirmed state; the event counters are raw native \
             listener firings (a drag fires many, a toggle exactly one).",
        )),
    ]);

    let mut children = vec![page_header(SECTION), intro, theme_block, writeback_block];
    children.extend(pairs);
    children.push(readout_block);
    page_column(children)
}
