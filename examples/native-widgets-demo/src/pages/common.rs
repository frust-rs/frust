//! Helpers every tab body and section shares: the text roles (accent
//! heading, muted caption), spacing, the padded showcase [`block`], the
//! side-by-side [`pair_row`] (REAL control left, frust-drawn right, one cell
//! size), the page-local signal macro, the embedded demo image, the
//! documented at-rest slot counts per tab and per section ([`AT_REST_SLOTS`],
//! [`SECTION_AT_REST_SLOTS`]) with the [`page_header`] that states them, the
//! scrolling [`tab_body`], and [`AnchorProbe`] — the window-rect helper an
//! action sheet anchors on.
//!
//! Every color resolves from the live [`Theme`] (Glyph baseline fallback), so
//! each page follows the brightness toggle.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, Point, Rect, SemanticsCtx, Size, View, Widget, build_child,
    rebuild_child, route_event_single, teardown_child,
};
use frust::{
    Align, Alignment, AnyView, Axis, ButtonStyle, Color, CrossAxisAlignment, EdgeInsets, Either,
    FlexChild, FlexView, Get, GetUntracked, ImageSource, Padding, Set, SizedBox, Theme, any,
    button, inflexible, safe_area, scroll_view, text, use_context,
};
use frust_native_widgets::{AnchorRect, live_slot_count};

use super::sandwich::SANDWICH_AT_REST_SLOTS;
use crate::{NativeWidgetsDemoState, SECTION_LABELS, Tab};

/// Shorthand for the one app state every page is generic over.
pub type S = NativeWidgetsDemoState;

// ---------------------------------------------------------------------------
// Page-local signals
// ---------------------------------------------------------------------------

/// Defines a `fn $name() -> RwSignal<$ty>` returning a page-local signal
/// cached in a `thread_local!`, self-healing across a disposed owner (a
/// headless test tears its owner down between runs; the next read mints a
/// fresh signal instead of touching a disposed one). Page state no other page
/// reads lives here rather than in [`NativeWidgetsDemoState`], which holds
/// only what the shell itself needs.
macro_rules! local_sig {
    ($name:ident, $ty:ty, $init:expr) => {
        fn $name() -> frust::RwSignal<$ty> {
            use frust::GetUntracked as _;
            thread_local! {
                static SLOT: std::cell::RefCell<Option<frust::RwSignal<$ty>>> =
                    const { std::cell::RefCell::new(None) };
            }
            SLOT.with(|cell| {
                if let Some(sig) = *cell.borrow()
                    && sig.try_get_untracked().is_some()
                {
                    return sig;
                }
                let sig = frust::RwSignal::new($init);
                *cell.borrow_mut() = Some(sig);
                sig
            })
        }
    };
}
pub(crate) use local_sig;

/// Bump a `u32` counter signal by one — from an event handler only (a native
/// listener or a frust `on_*` callback), never from `build`.
pub fn bump(sig: frust::RwSignal<u32>) {
    sig.set(sig.get_untracked().saturating_add(1));
}

// ---------------------------------------------------------------------------
// Text roles and spacing
// ---------------------------------------------------------------------------

/// The live theme, Glyph baseline before any context exists.
pub fn theme() -> Theme {
    use_context::<Theme>().unwrap_or_else(frust_glyph::baseline)
}

/// Accent-text role (`primary`).
pub fn accent() -> Color {
    theme().scheme().primary
}

/// Muted caption ink (`on_surface_variant`).
pub fn muted() -> Color {
    theme().scheme().on_surface_variant
}

/// A demo heading in the accent role.
pub fn label(s: impl Into<String>) -> impl View<S> {
    text(s).size(13.0).color(accent())
}

/// A muted per-demo caption.
pub fn caption(s: impl Into<String>) -> impl View<S> {
    text(s).size(11.0).color(muted())
}

/// A readout line in the body-text role — the "visible frust state" half of
/// every round trip, so it reads as data rather than commentary.
pub fn readout(s: impl Into<String>) -> impl View<S> {
    text(s).size(12.0).color(theme().scheme().on_surface)
}

/// A fixed-height vertical spacer.
pub fn gap(h: f64) -> FlexChild<S> {
    inflexible(SizedBox(None, Some(h)))
}

/// A fixed-width horizontal spacer.
pub fn gap_h(w: f64) -> FlexChild<S> {
    inflexible(SizedBox(Some(w), None))
}

/// One padded showcase block of rows.
pub fn block(children: Vec<FlexChild<S>>) -> FlexChild<S> {
    inflexible(Padding(
        EdgeInsets::all(12.0),
        FlexView::new(Axis::Vertical, children),
    ))
}

/// A horizontal row of children, vertically centered.
pub fn row(children: Vec<FlexChild<S>>) -> impl View<S> {
    FlexView::new(Axis::Horizontal, children).cross_axis(CrossAxisAlignment::Center)
}

/// A small secondary chip button — the page-local toggles and triggers.
pub fn chip(title: impl Into<String>, handler: impl Fn(&mut S) + 'static) -> impl View<S> {
    button(title, handler).style(ButtonStyle::Secondary).small()
}

/// The page's outer column: 16 px of padding around `children`. The shell
/// already wraps every page in a scroll view, so a page never nests its own.
// erasure: keep every page fn tails through here into the eight-arm section dispatch
pub fn page_column(children: Vec<FlexChild<S>>) -> AnyView<S> {
    any(Padding(
        EdgeInsets::all(16.0),
        FlexView::new(Axis::Vertical, children),
    ))
}

// ---------------------------------------------------------------------------
// Platform naming
// ---------------------------------------------------------------------------

/// The platform this build targets, as a reader would name it.
pub const PLATFORM: &str = if cfg!(target_os = "android") {
    "Android"
} else if cfg!(target_os = "ios") {
    "iOS/iPadOS"
} else if cfg!(target_os = "macos") {
    "macOS"
} else {
    "desktop preview (no native host)"
};

/// The native class backing one builder on each arm. [`Self::caption`] is the
/// one-line per-platform caption every pair row carries.
#[derive(Clone, Copy)]
pub struct NativeClasses {
    /// The `android.widget` class, or a refusal note.
    pub android: &'static str,
    /// The UIKit class, or a refusal note.
    pub ios: &'static str,
    /// The AppKit class, or a refusal note.
    pub macos: &'static str,
}

impl NativeClasses {
    /// The class this build mounts.
    pub fn here(self) -> &'static str {
        if cfg!(target_os = "android") {
            self.android
        } else if cfg!(target_os = "ios") {
            self.ios
        } else if cfg!(target_os = "macos") {
            self.macos
        } else {
            "nothing (no native host on this target)"
        }
    }

    /// `Android X · iOS Y · macOS Z — here: …`.
    pub fn caption(self) -> String {
        format!(
            "Android {} \u{b7} iOS {} \u{b7} macOS {} \u{2014} here: {}",
            self.android,
            self.ios,
            self.macos,
            self.here()
        )
    }
}

// ---------------------------------------------------------------------------
// At-rest slot counts + the page header
// ---------------------------------------------------------------------------

/// The one slot `native_segmented` adds where it has a native arm
/// (iOS/macOS/Android); a refusal banner elsewhere.
const SEGMENTED_SLOTS: usize = if cfg!(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android"
)) {
    1
} else {
    0
};

/// The one slot `native_stepper` adds on iOS/macOS; a banner elsewhere.
const STEPPER_SLOTS: usize = if cfg!(any(target_os = "ios", target_os = "macos")) {
    1
} else {
    0
};

/// The one slot `native_tab_bar` adds on iOS/iPadOS; a banner elsewhere.
const TAB_BAR_SLOTS: usize = if cfg!(target_os = "ios") { 1 } else { 0 };

/// The documented native `platform_view` slots each section publishes at
/// rest (every toggle at its default), by [`SECTION_LABELS`] index, for this
/// target. A presentation is never a slot (Alerts, Sheet: 0); New counts the
/// spinner and date picker plus segmented and stepper where native; the
/// Composite card (1 slot) and the Stress cycler (+6) and grid (+50) sit
/// behind default-off toggles. A Mode B surface whose translucency the
/// platform refused renders placeholders, which publish no slot.
pub const SECTION_AT_REST_SLOTS: [usize; SECTION_LABELS.len()] = [
    6,
    2,
    2 + SEGMENTED_SLOTS + STEPPER_SLOTS,
    0,
    TAB_BAR_SLOTS,
    0,
    0,
    0,
];

/// The documented native slots each tab publishes at rest with its whole
/// body laid out visible (a clipped body publishes fewer), by [`Tab::index`]:
/// the sum of its sections' [`SECTION_AT_REST_SLOTS`] (plus
/// [`SANDWICH_AT_REST_SLOTS`] on Lab). The chrome is never a slot.
///
/// **Hidden tabs.** The tab stack lays out every tab but paints only the
/// selected one, and a slot is published only while painted: from the first
/// frame after a switch the published set is the new tab's alone (measured
/// headless on macOS over consecutive frames: Controls 10, Lab 4, Home 0).
/// The host then disposes the unpublished slots, at its own pace — so the
/// live count may lag a switch — and re-creates them when the tab returns.
pub const AT_REST_SLOTS: [usize; Tab::COUNT] = [
    0,
    SECTION_AT_REST_SLOTS[0] + SECTION_AT_REST_SLOTS[2] + SECTION_AT_REST_SLOTS[6],
    SECTION_AT_REST_SLOTS[3] + SECTION_AT_REST_SLOTS[5],
    SECTION_AT_REST_SLOTS[1]
        + SECTION_AT_REST_SLOTS[4]
        + SECTION_AT_REST_SLOTS[7]
        + SANDWICH_AT_REST_SLOTS,
];

/// What a header, readout or slot count is for: a whole tab, or one section
/// by its index into [`SECTION_LABELS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageKey {
    /// A tab body.
    Tab(Tab),
    /// One section page.
    Section(usize),
}

impl From<Tab> for PageKey {
    fn from(tab: Tab) -> Self {
        PageKey::Tab(tab)
    }
}

impl From<usize> for PageKey {
    fn from(section: usize) -> Self {
        PageKey::Section(section)
    }
}

impl PageKey {
    /// The heading a [`page_header`] shows.
    fn title(self) -> &'static str {
        match self {
            PageKey::Tab(tab) => tab.title(),
            PageKey::Section(section) => SECTION_LABELS.get(section).copied().unwrap_or("?"),
        }
    }

    /// The one-line summary a [`page_header`] shows.
    fn summary(self) -> &'static str {
        match self {
            PageKey::Tab(tab) => super::TAB_SUMMARIES[tab.index()],
            PageKey::Section(section) => {
                super::SECTION_SUMMARIES.get(section).copied().unwrap_or("")
            }
        }
    }
}

/// The documented at-rest count for a tab ([`AT_REST_SLOTS`]) or a section
/// ([`SECTION_AT_REST_SLOTS`]), `0` for an out-of-range section.
pub fn at_rest_slots(key: impl Into<PageKey>) -> usize {
    match key.into() {
        PageKey::Tab(tab) => AT_REST_SLOTS[tab.index()],
        PageKey::Section(section) => SECTION_AT_REST_SLOTS.get(section).copied().unwrap_or(0),
    }
}

/// The value [`live_readout`] expects: [`at_rest_slots`] on a target with a
/// native host (Android/iOS/macOS), `0` elsewhere — a Linux/Windows preview
/// has no runtime, so [`live_slot_count`] always reads `0` there. The
/// structural counts stay as documented either way.
fn expected_live_slots(key: PageKey) -> usize {
    if cfg!(any(
        target_os = "android",
        target_os = "ios",
        target_os = "macos"
    )) {
        at_rest_slots(key)
    } else {
        0
    }
}

local_sig!(live_refresh_sig, u32, 0);

/// The signal every [`live_readout`] subscribes to: bump it (from an event
/// handler, on any thread) to re-sample [`live_slot_count`] — the chrome's
/// "Re-read live slots" menu item does.
pub fn live_refresh() -> frust::RwSignal<u32> {
    live_refresh_sig()
}

/// The live-slot readout line plus its "Re-read" chip.
///
/// [`live_slot_count`] is sampled when the page rebuilds, and a new slot is
/// created by the host after the paint that published it — so a page's first
/// frame can still count the previous page's slots. "Re-read" forces a
/// rebuild to sample it again. Diagnostics only: `live_slot_count` carries no
/// compatibility promise.
pub fn live_readout(key: impl Into<PageKey>) -> Vec<FlexChild<S>> {
    let key = key.into();
    let _ = live_refresh_sig().get();
    vec![
        inflexible(readout(format!(
            "Live native slots (process): {} \u{2014} expected at rest here: {}",
            live_slot_count(),
            expected_live_slots(key)
        ))),
        gap(4.0),
        inflexible(chip("Re-read live count", |_: &mut S| {
            bump(live_refresh_sig())
        })),
    ]
}

/// A tab's or section's opening block: its title as a heading, its summary,
/// the platform this build runs on, and the live-slot readout against the
/// documented at-rest count.
pub fn page_header(key: impl Into<PageKey>) -> FlexChild<S> {
    let key = key.into();
    let mut rows = vec![
        inflexible(text(key.title()).size(22.0).color(accent())),
        gap(6.0),
        inflexible(caption(key.summary())),
        gap(6.0),
        inflexible(caption(format!("This build: {PLATFORM}."))),
        gap(4.0),
    ];
    rows.extend(live_readout(key));
    block(rows)
}

/// A tab's root body: `children` in a column (each brings its own padding,
/// like [`page_column`]), the safe area inside the tab's own scroll view so
/// content starts below floating bars at rest and scrolls under them.
pub fn tab_body(state: &S, tab: Tab, children: Vec<FlexChild<S>>) -> AnyView<S> {
    any(
        scroll_view(safe_area(FlexView::new(Axis::Vertical, children)))
            .controller(state.scroll(tab).clone()),
    )
}

// ---------------------------------------------------------------------------
// Side-by-side pairs
// ---------------------------------------------------------------------------

/// One comparison column's width (logical px). Two of these plus [`PAIR_GAP`]
/// fit an iPhone SE: 375pt less 16 (page) + 12 (block) of padding on each side
/// leaves 319pt, against `140 + 12 + 140 = 292`. The one documented
/// exception is the inline date picker (`new_controls::date_block`): the
/// platform's inline picker is ~300pt wide by itself, so its native cell is
/// 300pt and the row runs `300 + 12 + 140 = 452` through [`pair_row_sized`]'s
/// separate widths; the drawn cell there is a readout the block repeats
/// under the row, so a phone loses nothing off-screen.
pub const PAIR_CELL_W: f64 = 140.0;

/// The gutter between a pair's native and drawn cells.
pub const PAIR_GAP: f64 = 12.0;

/// How a [`pair_row`] cell treats content narrower than the cell.
#[derive(Clone, Copy)]
pub enum CellFit {
    /// Tight constraints — the control fills the cell (a button, a track).
    Stretch,
    /// Loosened constraints, content pinned to the cell's left edge — a
    /// control with a fixed intrinsic size that must not be stretched.
    /// `android.widget.Switch` is the load-bearing case: a `TextView`
    /// subclass drawing its graphic at the far right of an over-wide frame.
    Natural,
}

/// One pair cell: a fixed `w` x `h` box, so both columns are literally the
/// same size and the difference a reader sees is the widget, not the box.
pub fn cell(fit: CellFit, w: f64, h: f64, content: impl View<S>) -> impl View<S> {
    match fit {
        CellFit::Stretch => Either::Left(SizedBox(Some(w), Some(h)).child(content)),
        // `Align` loosens the constraints it hands its child — the only way
        // to keep a natural size inside a tightened box.
        CellFit::Natural => Either::Right(
            SizedBox(Some(w), Some(h)).child(Align(Alignment::new(-1.0, 0.0), content)),
        ),
    }
}

/// One comparison row: `title`, the per-platform native-class caption, a
/// `note`, then the REAL platform control on the left and its frust-drawn
/// counterpart on the right, in cells of exactly the same size.
pub fn pair_row(
    title: &str,
    classes: NativeClasses,
    note: &str,
    fit: CellFit,
    height: f64,
    native: impl View<S>,
    drawn: impl View<S>,
) -> FlexChild<S> {
    pair_row_sized(
        title,
        classes,
        note,
        fit,
        (PAIR_CELL_W, PAIR_CELL_W),
        height,
        native,
        drawn,
    )
}

/// One comparison row with explicit cell widths — `(native, drawn)`, one
/// shared height: `title`, the per-platform native-class caption, a `note`,
/// then the REAL platform control on the left and its frust-drawn
/// counterpart on the right. [`pair_row`] is the equal-width case every
/// ordinary pair uses; pass different widths only when the platform control
/// has an intrinsic size the drawn cell need not match (the inline date
/// picker — see [`PAIR_CELL_W`]), keeping the row's tree shape identical to
/// the equal-width one so a width change never remounts the native control.
#[allow(clippy::too_many_arguments)]
pub fn pair_row_sized(
    title: &str,
    classes: NativeClasses,
    note: &str,
    fit: CellFit,
    (native_width, drawn_width): (f64, f64),
    height: f64,
    native: impl View<S>,
    drawn: impl View<S>,
) -> FlexChild<S> {
    block(vec![
        inflexible(label(title)),
        gap(4.0),
        inflexible(caption(classes.caption())),
        gap(2.0),
        inflexible(caption(note)),
        gap(6.0),
        inflexible(row(vec![
            inflexible(cell(fit, native_width, height, native)),
            gap_h(PAIR_GAP),
            inflexible(cell(fit, drawn_width, height, drawn)),
        ])),
    ])
}

// ---------------------------------------------------------------------------
// The embedded demo image
// ---------------------------------------------------------------------------

/// The PNG every image demo shows (`assets/demo-image.png`, 128x128 — the same
/// bytes `examples/playground`'s image pair used; see `README.md` § Assets for
/// provenance and licence).
const DEMO_IMAGE_PNG: &[u8] = include_bytes!("../../assets/demo-image.png");

/// The demo image's encoded bytes, published once: re-`Arc`ing them every
/// rebuild would mint a fresh `Arc` — a fresh publish revision, so a
/// per-rebuild native re-decode.
pub fn demo_image_bytes() -> Arc<[u8]> {
    static BYTES: OnceLock<Arc<[u8]>> = OnceLock::new();
    Arc::clone(BYTES.get_or_init(|| Arc::from(DEMO_IMAGE_PNG)))
}

/// The same bytes decoded ONCE for frust's own `Image` (an `ImageSource` is a
/// cheap handle over decoded pixels). A corrupt asset degrades to a 1x1
/// transparent stand-in rather than taking the app down.
pub fn demo_image() -> ImageSource {
    static IMAGE: OnceLock<ImageSource> = OnceLock::new();
    IMAGE
        .get_or_init(|| {
            ImageSource::decode(DEMO_IMAGE_PNG)
                .unwrap_or_else(|_| ImageSource::from_rgba8(vec![0, 0, 0, 0], 1, 1))
        })
        .clone()
}

// ---------------------------------------------------------------------------
// AnchorProbe — a widget's window rect, for an action sheet's anchor
// ---------------------------------------------------------------------------

thread_local! {
    /// The last painted window rect of every mounted [`AnchorProbe`], by key.
    /// Plain (non-reactive) storage on purpose: paint must never write a
    /// signal, and nothing renders from this — an event handler reads it.
    static ANCHORS: RefCell<HashMap<&'static str, AnchorRect>> = RefCell::new(HashMap::new());
}

/// Wrap `child` so its painted window rect is recorded under `key`, for
/// [`anchor_rect`] to hand an action sheet as its [`AnchorRect`].
///
/// # How the rect is computed
///
/// `PaintCtx::origin` is the widget's **absolute window-space origin**,
/// accumulated down the paint pass (scroll offsets included) — the same value
/// a `platform_view` slot publishes as its native frame rect. The probe lays
/// its child out at its own origin under the incoming constraints (layout-,
/// event- and semantics-transparent), paints it, then records `origin + size`
/// in logical pixels. frust's logical pixels are iOS points and `AnchorRect`
/// is in window coordinates, so the value needs no conversion. It describes
/// the most recent paint: read it from an event handler (the button the user
/// just pressed was painted where they saw it).
pub fn anchor_probe<V: View<S>>(key: &'static str, child: V) -> AnchorProbe<V> {
    AnchorProbe { key, child }
}

/// The last painted window rect recorded under `key`, if a probe with that key
/// has painted on this thread.
pub fn anchor_rect(key: &str) -> Option<AnchorRect> {
    ANCHORS.with(|anchors| anchors.borrow().get(key).copied())
}

/// Convert a window-space rect into the plugin's [`AnchorRect`].
pub fn anchor_from_rect(rect: Rect) -> AnchorRect {
    AnchorRect {
        x: rect.x0,
        y: rect.y0,
        width: rect.width(),
        height: rect.height(),
    }
}

/// See [`anchor_probe`].
pub struct AnchorProbe<V> {
    key: &'static str,
    child: V,
}

/// The retained widget for an [`AnchorProbe`].
pub struct AnchorProbeWidget {
    key: &'static str,
    child: ChildPod,
}

impl<V: View<S>> View<S> for AnchorProbe<V> {
    type Element = AnchorProbeWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AnchorProbeWidget {
        AnchorProbeWidget {
            key: self.key,
            child: build_child(&self.child, ctx),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AnchorProbeWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.key = self.key;
        rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut AnchorProbeWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for AnchorProbeWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.child.set_origin(Point::ZERO);
        self.child.layout_child(ctx, bc)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.child.paint_child(ctx, scene);
        let rect = Rect::from_origin_size(ctx.origin(), ctx.size());
        ANCHORS.with(|anchors| {
            anchors
                .borrow_mut()
                .insert(self.key, anchor_from_rect(rect));
        });
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        visitor(&self.child);
    }
}
