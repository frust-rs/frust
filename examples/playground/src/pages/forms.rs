//! Forms section: the fixtures for the safe-area / keyboard / back device gate.
//!
//! Top to bottom:
//!
//! 1. a status line: the field whose caret last reported, and the depth of the full-width
//!    navigator;
//! 2. two normal fields, A and B, stacked at the top (focus moves between them by tap, or by the
//!    keyboard's Return or Next key if the platform delivers one);
//! 3. an obscured field, C, classified as a password input;
//! 4. a "Push full-width route" button and a navigator that spans the page edge to edge, with no
//!    horizontal padding of its own (the pushed pages show their depth and their own Push);
//! 5. numbered filler rows, so the last field sits where the on-screen keyboard will cover it;
//! 6. field D at the very end of the page, just above the shell's bottom navigation bar once
//!    the page is scrolled to its end.
//!
//! # What the framework cannot do here
//!
//! - **No programmatic focus.** `EventCtx::request_focus` is reachable only inside a widget's own
//!   event handler, and `on_submit` hands the app `&mut State` only, so A's Return cannot move
//!   focus to B. Return keeps its normal behaviour (it fires `on_submit`, which here only records
//!   a submit count); move focus by tapping B.
//! - **No blur callback.** `on_caret_change` fires when a field gains focus and when its caret
//!   moves, but nothing reports a field losing focus, so the status line shows the field whose
//!   caret reported last, not a live "focused now".
//! - **No bottom anchoring.** The shell hosts every page in a vertical scroll view, so D is the
//!   last row of scrolling content rather than pinned to the bottom of the window.
//!
//! # Where the full-width navigator sits
//!
//! The shell hosts the page in a scroll view with no inset of its own, and this page adds no
//! page-level padding, so the navigator spans the scroll view's full width. It is at the
//! screen's left edge in portrait at phone width (the bottom navigation bar is in use). At the
//! wide breakpoint the shell puts a side rail to the left of the page, and the navigator then
//! starts at the rail's right edge. The shell's safe area also insets the page in landscape by
//! the display cutout.

use frust::{
    ButtonStyle, Color, EdgeInsets, Get, NavigatorController, NavigatorView, Padding,
    RouteObserver, RwSignal, Set, SizedBox, Theme, Update, View, button, column, navigator, text,
    text_input, use_context, views,
};

use crate::PlaygroundState;

/// Number of numbered filler rows between the password field and field D.
const FILLER_ROWS: usize = 18;

/// Height of the full-width navigator, in logical pixels: a navigator needs a bounded height
/// inside the shell's scroll view.
const NAV_H: f64 = 160.0;

/// Horizontal inset of ordinary page content (the navigator gets none).
const SIDE: f64 = 16.0;

/// The "Forms" section's state, owned by [`PlaygroundState`]. Every field is a
/// clone-shares-one-handle value.
#[derive(Clone)]
pub struct FormsState {
    /// Field A's contents.
    pub a: RwSignal<String>,
    /// Field B's contents.
    pub b: RwSignal<String>,
    /// Field C's contents (the obscured field).
    pub c: RwSignal<String>,
    /// Field D's contents (the bottom field).
    pub d: RwSignal<String>,
    /// The label of the field whose caret reported last.
    pub focus: RwSignal<&'static str>,
    /// How many times Return fired a field's `on_submit`.
    pub submits: RwSignal<u32>,
    /// The full-width navigator's controller.
    pub nav: NavigatorController<PlaygroundState>,
    /// Feeds the status line's depth.
    pub route: RouteObserver,
}

impl FormsState {
    /// Fresh state: empty fields, no focus report, an empty navigator.
    pub fn new() -> Self {
        FormsState {
            a: RwSignal::new(String::new()),
            b: RwSignal::new(String::new()),
            c: RwSignal::new(String::new()),
            d: RwSignal::new(String::new()),
            focus: RwSignal::new("none"),
            submits: RwSignal::new(0),
            nav: NavigatorController::new(),
            route: RouteObserver::new(),
        }
    }
}

impl Default for FormsState {
    fn default() -> Self {
        Self::new()
    }
}

fn theme() -> Theme {
    use_context::<Theme>().unwrap_or_else(frust_material::baseline)
}

fn accent() -> Color {
    theme().scheme().primary
}

fn muted() -> Color {
    theme().scheme().on_surface_variant
}

/// Inset a content item from the screen edges.
fn inset<V: View<PlaygroundState>>(view: V) -> impl View<PlaygroundState> {
    Padding(EdgeInsets::symmetric(SIDE, 2.0), view)
}

/// A muted caption line.
fn caption(content: &str) -> impl View<PlaygroundState> + use<> {
    inset(text(content.to_string()).size(11.0).color(muted()))
}

/// A label line above a field.
fn label(content: &'static str) -> impl View<PlaygroundState> + use<> {
    inset(text(content).size(13.0).color(accent()))
}

/// A field: `value` mirrored into `signal`, reporting its label on every caret report and counting
/// Return presses.
fn field(
    name: &'static str,
    value: RwSignal<String>,
    focus: RwSignal<&'static str>,
    submits: RwSignal<u32>,
) -> frust::TextInputView<PlaygroundState> {
    text_input(
        value.get(),
        move |_state: &mut PlaygroundState, next: String| value.set(next),
    )
    .placeholder(format!("Field {name}"))
    .on_caret_change(move |_state: &mut PlaygroundState, _info| focus.set(name))
    .on_submit(move |_state: &mut PlaygroundState, _text: String| {
        submits.update(|count| *count += 1);
    })
}

/// A pushed page showing its depth, with its own Push. The page pads itself, so the navigator
/// around it stays flush with the screen edges.
fn detail_page(
    nav: NavigatorController<PlaygroundState>,
    level: usize,
) -> impl View<PlaygroundState> {
    let host = nav.clone();
    Padding(
        EdgeInsets::all(12.0),
        column()
            .child(
                text(format!("Route depth {level}"))
                    .size(13.0)
                    .color(accent()),
            )
            .child(SizedBox(None, Some(8.0)))
            .child(
                button("Push", move |_state: &mut PlaygroundState| {
                    push_route(&host, level + 1);
                })
                .style(ButtonStyle::Primary)
                .small(),
            ),
    )
}

/// Push depth `level` onto `nav` with the default push options, as the Tabs page does.
fn push_route(nav: &NavigatorController<PlaygroundState>, level: usize) {
    let host = nav.clone();
    nav.push(move || detail_page(host.clone(), level));
}

/// The full-width navigator, observed for the status line's depth.
fn full_width_navigator(forms: &FormsState) -> NavigatorView<PlaygroundState> {
    let nav = forms.nav.clone();
    forms.route.observe(navigator(&nav, || {
        Padding(
            EdgeInsets::all(12.0),
            column()
                .child(text("Route depth 1 (root)").size(13.0).color(accent()))
                .child(
                    text("Press Push full-width route, then swipe back from the left edge.")
                        .size(11.0),
                ),
        )
    }))
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &PlaygroundState) -> impl View<PlaygroundState> {
    let forms = &state.forms;
    let status = format!(
        "focus (last caret report): {}   route depth: {}   Return presses: {}",
        forms.focus.get(),
        forms.route.depth(),
        forms.submits.get(),
    );
    let (focus, submits) = (forms.focus, forms.submits);
    let push_nav = forms.nav.clone();

    column()
        .child(SizedBox(None, Some(8.0)))
        .child(inset(text("Forms").size(13.0).color(accent())))
        .child(inset(text(status).size(12.0)))
        .child(SizedBox(None, Some(8.0)))
        .child(label("A and B: two normal fields"))
        .child(inset(field("A", forms.a, focus, submits)))
        .child(inset(field("B", forms.b, focus, submits)))
        .child(caption(
            "Move focus from A to B by tapping B, then try the keyboard's Return or \
             Next key. The framework has no programmatic focus, so Return on A does \
             not move focus to B; it only counts a press.",
        ))
        .child(SizedBox(None, Some(8.0)))
        .child(label("C: the password field"))
        .child(inset(field("C", forms.c, focus, submits).obscured(true)))
        .child(caption(
            "C is obscured and reports itself as a password input to accessibility.",
        ))
        .child(SizedBox(None, Some(8.0)))
        .child(inset(
            button(
                "Push full-width route",
                move |_state: &mut PlaygroundState| {
                    let depth = push_nav.depth();
                    push_route(&push_nav, depth + 1);
                },
            )
            .style(ButtonStyle::Primary)
            .small(),
        ))
        .child(caption(
            "The navigator below has no horizontal padding or inset. It is at the \
             screen's left edge in portrait at phone width; with the wide-width side \
             rail it starts at the rail's right edge, and in landscape the safe area \
             insets it by the display cutout.",
        ))
        .child(SizedBox::<PlaygroundState>(None, Some(NAV_H)).child(full_width_navigator(forms)))
        .child(SizedBox(None, Some(8.0)))
        .child(caption(
            "Filler rows: scroll down to field D, where the keyboard covers it.",
        ))
        .children(views(
            (1..=FILLER_ROWS).map(|n| inset(text(format!("Filler row {n}")).size(13.0))),
        ))
        .child(SizedBox(None, Some(8.0)))
        .child(label("D: the bottom field"))
        .child(inset(field("D", forms.d, focus, submits)))
        .child(caption(
            "D is the last row of the page: it sits just above the bottom navigation bar \
             when scrolled to the end. With the keyboard open the content should shift \
             clear of the keyboard.",
        ))
        .child(SizedBox(None, Some(8.0)))
}
