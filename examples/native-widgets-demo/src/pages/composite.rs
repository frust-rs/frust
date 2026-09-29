//! **Composite** — one `NativeComponent` owning a whole native hierarchy
//! frust does not lay out: the plugin's `DemoCard` (a parent view with a
//! title label and two buttons) published as **one** `platform_view` slot.
//!
//! # Events work
//!
//! The card attaches the platform's one listener to its Primary button, its
//! `NativeComponent::on_event` forwards each click, and this page's
//! `.on_event` hook bumps a signal — the builders' events-as-signals idiom.
//! The count comes back to the card as its `presses` prop, so the readout
//! and the Primary caption both advance by exactly one per tap. Dismiss is
//! deliberately left unwired: two children attached for the same family are
//! indistinguishable (`native-widgets-component-same-kind-children-indistinguishable`).
//!
//! # The FFI wall still applies
//!
//! The card ships inside the plugin behind its non-default
//! `demo-components` feature, not here, because implementing
//! `NativeComponent` needs raw `jni`/`objc2-ui-kit`/`objc2-app-kit` view
//! construction in the implementing crate, which this plugin does not
//! re-export — and `docs/CODE_STANDARDS.md` puts an app on `frust` plus
//! plugin crates only. So this page does what an app really can: enable the
//! feature, register the component once, mount it. The claim is stated in
//! [`composite_block`]'s caption and kept in step with
//! `plugins/native-widgets/tests/limitation_conformance.rs`'s list.
//!
//! # Zero slots at rest, one with the card on
//!
//! The toggle is OFF by default, so this page mounts no native slot at rest.
//! Turned on, the header's live count must read exactly ONE more — one slot
//! for the whole card however many native children it has. Reading three
//! more would mean the card's children leaked into frust's slot space.

use frust::{AnyView, Color, Get, GetUntracked, Set, any, inflexible};
use frust_native_widgets::{
    DEMO_CARD_HEIGHT, DEMO_CARD_KIND, DEMO_CARD_WIDTH, DemoCard, DemoCardProps, native_component,
    register_demo_components,
};

use super::common::{
    S, block, bump, caption, chip, gap, label, local_sig, page_column, page_header, readout, theme,
};

/// This page's index in [`SECTION_LABELS`](crate::SECTION_LABELS).
const SECTION: usize = 6;

/// Titles the "Rename" chip cycles through — a frust-side prop change that
/// drives the card's `update` path (and its retained child handles) live.
const TITLES: [&str; 3] = ["Composite card", "Renamed from frust", "Third title"];

local_sig!(visible_sig, bool, false);
local_sig!(presses_sig, u32, 0);
local_sig!(title_sig, usize, 0);

/// Pack a [`Color`] as the ARGB `int` every native color setter takes — the
/// `[a, r, g, b]` big-endian packing the plugin's own wire uses. A
/// component's props are typed Rust values, so folding the theme into them is
/// the app's job; this fold is why the card re-themes off the brightness
/// toggle like the builders do.
fn argb(color: Color) -> i32 {
    let [r, g, b, a] = color.to_rgba8().to_u8_array();
    i32::from_be_bytes([a, r, g, b])
}

/// Register `DemoCard` with this thread's native-widgets runtime, once.
/// Registration is first-wins and warns when refused, so a thread-local latch
/// keeps it to one call; a rebuild always runs on the platform main thread.
fn ensure_demo_registered() {
    thread_local! {
        static REGISTERED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    REGISTERED.with(|done| {
        if !done.get() {
            register_demo_components();
            done.set(true);
        }
    });
}

/// The card itself: ONE `platform_view` slot. `title` and the two theme
/// colors are live props (update path); `presses` closes the event round
/// trip.
fn composite_demo(title: &str, background: Color, ink: Color, presses: u32) -> AnyView<S> {
    any(native_component(
        DEMO_CARD_KIND,
        DemoCard,
        DemoCardProps {
            title: title.to_string(),
            primary: "Primary".to_string(),
            secondary: "Dismiss".to_string(),
            background_argb: argb(background),
            title_argb: argb(ink),
            presses,
        },
    )
    .size(DEMO_CARD_WIDTH, DEMO_CARD_HEIGHT)
    .interactive()
    .semantics_label("Native composite card")
    .on_event(|event| {
        if event.is_click() {
            bump(presses_sig());
        }
    }))
}

/// The explanation, the toggle, the readout, and (when on) the card.
///
/// Always returns exactly two children: the page's column is reconciled by
/// position, so a hidden card is a zero-height spacer rather than nothing —
/// the same stable shape the toggle would need anywhere above other content.
fn composite_block(visible: bool, presses: u32, title: usize) -> Vec<frust::FlexChild<S>> {
    let intro = block(vec![
        inflexible(label(
            "Native composite \u{2014} one component, one slot, three native children",
        )),
        gap(6.0),
        inflexible(caption(
            "Every other page mounts ONE native control per slot. This is the other shape: a \
             single `NativeComponent` that builds its own native view hierarchy (a parent \
             with a label and two buttons) and ships it as ONE `platform_view` slot \u{2014} \
             the platform lays the children out; frust never learns they exist.",
        )),
        gap(6.0),
        inflexible(caption(
            "EVENTS WORK: tap Primary and the click travels native \u{2192} the component's \
             `on_event` \u{2192} this page's `.on_event` hook \u{2192} a signal; the new count \
             comes back as a prop, so the readout below and the Primary caption both advance \
             by exactly 1. Dismiss is left unwired on purpose (same-family children are \
             indistinguishable), so it shows the platform's press feedback only.",
        )),
        gap(6.0),
        inflexible(caption(
            "THE FFI WALL STILL APPLIES: an app crate cannot implement `NativeComponent` (it \
             needs raw jni/objc2-ui-kit dependencies the plugin does not re-export). The card \
             is defined inside the plugin behind its non-default `demo-components` feature; \
             this page enables the feature, registers the component and mounts it.",
        )),
        gap(6.0),
        inflexible(readout(format!("Composite presses: {presses}"))),
        gap(6.0),
        inflexible(chip(
            if visible {
                "Hide native composite (live count -1)"
            } else {
                "Show native composite (live count +1)"
            },
            |_: &mut S| {
                let sig = visible_sig();
                sig.set(!sig.get_untracked());
            },
        )),
        gap(6.0),
        inflexible(chip("Rename the card's title", |_: &mut S| {
            let sig = title_sig();
            sig.set((sig.get_untracked() + 1) % TITLES.len());
        })),
        gap(4.0),
        inflexible(caption(format!(
            "Title prop: \u{201c}{}\u{201d} \u{2014} renaming runs the card's update path; \
             light/dark re-themes it through the same diff as the builders.",
            TITLES[title % TITLES.len()]
        ))),
    ]);

    let second = if visible {
        // Registered lazily: nothing else needs the component, and a hidden
        // card costs this page nothing.
        ensure_demo_registered();
        let active = theme();
        let scheme = active.scheme();
        block(vec![inflexible(composite_demo(
            TITLES[title % TITLES.len()],
            scheme.surface_container,
            scheme.on_surface,
            presses,
        ))])
    } else {
        gap(0.0)
    };
    vec![intro, second]
}

/// See the page-fn contract in [`crate::pages`] and the [module docs](self).
pub fn page(_state: &S) -> AnyView<S> {
    // Tracked: the card's native click writes `presses_sig` from the
    // platform listener, outside any frust event pass.
    let visible = visible_sig().get();
    let presses = presses_sig().get();
    let title = title_sig().get();

    let mut children = vec![page_header(SECTION)];
    children.extend(composite_block(visible, presses, title));
    page_column(children)
}
