//! `web-gallery`: the browser gallery — the web shell's milestone gate vehicle.
//!
//! An ordinary Frust app — authored through the `frust` facade's
//! [`frust::web_app!`] entry point (w1-05), the same vocabulary a
//! desktop/Android/iOS app author uses — that renders the shared
//! `frust-gallery` case registry (`examples/gallery`: 107 cases across the
//! framework baseline and all five design-system plugins) in the browser.
//! Unlike `examples/web-spike` (which builds a static [`frust_scene::Scene`]
//! by hand, bypassing the facade to probe `frust-gpu`'s seams one at a time),
//! this crate exercises the *whole* app-authoring path for `wasm32`: widgets,
//! the `Component` state boundary, the reactive runtime, and every browser
//! host-signal milestone (pointer/touch/wheel/keyboard input, theme follow,
//! resize + DPR, and a signal-driven repaint with zero input events) in one
//! binary.
//!
//! # Pages
//!
//! - `?case=<slug>` (e.g. `?case=button`, `?case=material/button`) renders
//!   exactly that [`frust_gallery::Case`], full-screen, with a small "back to
//!   index" header.
//! - No `?case=` (or an unknown slug) falls back to the **index page**: a
//!   live-filtered, keyboard-searchable, scrollable list of every case
//!   (mouse/touch/wheel exercise — see [`app::index_view`]), plus a
//!   background ticking counter that proves a `frust::RwSignal` write alone
//!   — no pointer or key event — wakes exactly the frames it needs to (the
//!   "signal-driven update, zero input events" milestone).
//!
//! Dark/light theme follow and resize/DPR handling need no app code at all:
//! both are the browser shell's own job (`crates/frust-shell-web`'s
//! `follow_platform_brightness`/window-metrics publish, from w1-02/w1-03).
//! The index page's `window: ... — Orientation` line
//! ([`frust::WindowMetrics`] read via `use_context`) is this app's one piece
//! of in-UI evidence that a resize/DPR change actually reached the tree,
//! alongside the screenshots recorded in README.md.
//!
//! # A discovered pre-existing limitation: `SystemUi` never resolves on `wasm32`
//!
//! Every text-bearing widget defaults to [`frust_text::FontFamily::SystemUi`]
//! (`TextStyle::default()`) unless a caller sets `.family(...)` explicitly —
//! and, as of this task, **no case in the whole `frust-gallery` registry
//! does** (`grep -rn '\.family(' examples/gallery/src` is empty). On desktop
//! that resolves through fontique's real system-font backend; on
//! `wasm32-unknown-unknown` fontique ships a documented "dummy system font
//! backend" with an empty generic-family map (see
//! `fontique-0.11.1/src/backend/mod.rs`), so `SystemUi` — and a generic
//! `NamedWithGeneric([], SansSerif)` stack, which is what [`Theme::neutral`]'s
//! own type scale carries too — resolves **zero glyphs**, and nothing an app
//! registers through [`frust::register_app_fonts`] changes that (verified
//! empirically here: registering a bundled face and re-rendering with no
//! other change left `SystemUi`-styled text exactly as blank). This app
//! works around it for its *own* authored chrome — see [`app::LABEL_FAMILY`]
//! and [`app::nav_row`] — by never using [`frust_widgets::button`] (whose
//! internal label has no family override) and instead building its own
//! tappable rows with an explicit bundled family. It cannot work around it
//! for a hosted [`frust_gallery::Case`]'s own internal text, which is outside
//! this task's write scope (`crates/frust-text`, `crates/frust-widgets`,
//! `examples/gallery`) — see README.md's "Known limitation" section for the
//! full writeup and the recommended follow-up card.
//!
//! See `README.md` for the build/serve/drive commands and the recorded
//! evidence for every milestone above, on both the WebGPU and the forced
//! WebGL2 backend arm.

// Real entry point. `frust::web_app!` (invoked inside `mod app` below)
// expands to a `#[wasm_bindgen(start)]` function that the browser calls once
// the compiled module is instantiated — `main` itself is never reached in
// that environment, but a `[[bin]]` target still needs one to link. Kept
// empty rather than gated off: this crate exists to run in a browser, not
// natively, so there is nothing native-side worth probing here (contrast
// `examples/web-spike/src/main.rs`, which keeps a real native probe body
// because w0-02's compile check predates any browser bring-up).
fn main() {}

/// Everything that actually touches a browser API, target-gated on
/// `wasm32-unknown-unknown` the same way `examples/web-spike/src/main.rs`'s
/// `mod web` is: a native `cargo check` of this crate (not part of this
/// task's verify gate, but harmless to keep working) resolves none of the
/// `web-sys`/`js-sys`/`wasm-bindgen-futures`/`frust-shell-web` rows this
/// module's functions call into.
#[cfg(target_arch = "wasm32")]
mod app {
    use frust::authoring::text::FontFamily;
    use frust::{
        Axis, Color, Column, Component, CrossAxisAlignment, EdgeInsets, FlexView, GestureDetector,
        Get, Padding, Row, RwSignal, TextView, Update, WindowMetrics, any, component, container,
        flexible, inflexible, list_view, scroll_view, text, text_input, use_context,
    };
    use frust_gallery::{Case, Design, Variant};

    /// The bundled face this app registers at startup and every one of its
    /// *own* authored labels names explicitly (see [`label`]/[`nav_row`]) —
    /// `"Inter Variable"`, not `"Inter"`: the variable release's own name
    /// table entry reads that (same fact `plugins/shadcn`'s
    /// `tokens::theme::INTER_FAMILY` documents; not depended on directly here
    /// since it is not re-exported past that crate's `tokens` module).
    /// Reused from `plugins/shadcn/fonts/inter/InterVariable.ttf` rather than
    /// carrying a new font asset of this crate's own — full Latin coverage,
    /// already vendored and licensed in-repo, and not a new write-scope file
    /// (an `include_bytes!` read, not a write). See this file's module-level
    /// "discovered pre-existing limitation" section for why registering a
    /// face at all is necessary on this target, and why it is not enough on
    /// its own to fix every case's own text.
    const LABEL_FAMILY: &str = "Inter Variable";
    const LABEL_FONT_BYTES: &[u8] =
        include_bytes!("../../../plugins/shadcn/fonts/inter/InterVariable.ttf");

    /// The app's whole retained state: which case (if any) is showing, the
    /// index page's live filter text, and the zero-input-event ticking
    /// counter's signal handle.
    pub struct AppState {
        /// `Some` while a single case is on screen (from `?case=` or a click
        /// on the index list); `None` shows the index page.
        case: Option<&'static Case>,
        /// The index page's search box contents — [`text_input`]'s
        /// controlled value (see its own doc: "reports the requested text
        /// through `on_change` and adopts the app-confirmed `value` on the
        /// next rebuild").
        filter: String,
        /// Written once a second by [`start_ticker`]'s background task, read
        /// (and thereby tracked) by [`index_view`] — the proof that a signal
        /// write alone, with no pointer/key event anywhere near it, wakes a
        /// repaint.
        ticks: RwSignal<u32>,
    }

    impl AppState {
        /// Built once, under the reactive runtime's root `Owner`
        /// (`frust::web_app!`'s generated shim runs this inside
        /// `__web_init_state`, itself `rt.with_owner(state_init)` — see
        /// `crates/frust/src/lib.rs`'s `__web_init_state` doc), so
        /// [`RwSignal::new`] here has an owner to register under exactly the
        /// way it would inside a root [`Component::init`].
        fn new() -> Self {
            raise_log_level_from_query();
            // See `LABEL_FONT_BYTES`'s doc: without this, every text-bearing
            // widget on this target — including this app's own chrome —
            // renders zero glyphs (`FontFamily::SystemUi` never resolves on
            // `wasm32`; see this module's header doc). Registered before
            // `run_app` constructs the shell, matching the timing contract
            // `frust::register_app_fonts`'s own doc and `frust_shadcn::install`
            // (the precedent this follows) both spell out: "a shell reads ...
            // and drains the font registry once, at construction".
            frust::register_app_fonts(LABEL_FONT_BYTES.to_vec());

            let case = resolve_case_from_query();
            // A design-tagged case (`material/button`, `shadcn/button`, ...)
            // wants that design system's own theme underneath it, not the
            // framework's neutral baseline; `Base` cases render correctly
            // either way, so they are left alone rather than pinned. This is
            // `set_default_theme`, not `set_app_theme`: it seeds the base the
            // shell still re-derives light/dark from on every platform
            // brightness change, so a design-tagged case still honours the
            // OS/browser theme-follow milestone instead of freezing one
            // brightness — see `frust::set_default_theme`'s own doc for the
            // precedence order this relies on.
            if let Some(case) = case
                && case.design != Design::Base
            {
                frust::set_default_theme(frust_gallery::theme(case.design, Variant::Light));
            }

            let ticks = RwSignal::new(0u32);
            start_ticker(ticks);

            Self {
                case,
                filter: String::new(),
                ticks,
            }
        }
    }

    /// Reads `window.location.search` off `?log=`
    /// ([`frust_shell_web::logging::level_from_query`]) and raises the
    /// console log ceiling above the facade's fixed `Warn` bootstrap default
    /// — e.g. `?log=debug` for a verbose bring-up trace, matching
    /// `examples/web-spike`'s `?log=info` control. A missing or unparseable
    /// `?log=` leaves the facade's `Warn` default in place
    /// ([`frust_shell_web::logging::install`]'s own documented no-op-on-`Err`
    /// behaviour).
    fn raise_log_level_from_query() {
        let search = web_sys::window()
            .and_then(|window| window.location().search().ok())
            .unwrap_or_default();
        // `location.search` includes the leading `?`;
        // `level_from_query` explicitly wants the query string without it
        // (see that function's own doc).
        let query = search.strip_prefix('?').unwrap_or(&search);
        frust_shell_web::logging::install(frust_shell_web::logging::level_from_query(query));
    }

    /// Reads `?case=<slug>` off `window.location.search` and looks it up in
    /// [`frust_gallery::find`]. `None` for a missing query parameter, an
    /// unavailable `window`/`Location`, or a slug the registry does not
    /// carry — every one of those falls back to the index page rather than
    /// failing the app.
    fn resolve_case_from_query() -> Option<&'static Case> {
        let window = web_sys::window()?;
        let search = window.location().search().ok()?;
        let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
        let slug = params.get("case")?;
        frust_gallery::find(&slug)
    }

    /// Awaits `ms` milliseconds of wall-clock time via the
    /// `setTimeout`-into-`Promise`-into-`JsFuture` idiom —
    /// `std::thread::sleep` does not exist on `wasm32-unknown-unknown` and
    /// nothing in the graph carries a timer future, the identical situation
    /// `examples/web-spike/src/main.rs`'s own `sleep_ms` documents.
    async fn sleep_ms(ms: i32) {
        let promise = js_sys::Promise::new(&mut |resolve, _reject| {
            if let Some(window) = web_sys::window() {
                let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
            }
        });
        let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
    }

    /// Starts the zero-input-event repaint proof: a `!Send` task on the
    /// UI-thread local queue ([`frust::spawn_local`]) that wakes once a
    /// second and writes `ticks` — no pointer or key event anywhere in this
    /// function. [`index_view`] reads `ticks` through [`Get::get`], which
    /// tracks the read, so the write's `notify_dirty` is what wakes the next
    /// repaint (the same `TrackedScope` clean→dirty mechanism
    /// `examples/web-spike`'s `?signal=timer` proof mode exercises directly
    /// against `frust_reactive`; here it runs through the ordinary facade
    /// path an app never has to name).
    fn start_ticker(ticks: RwSignal<u32>) {
        frust::spawn_local(async move {
            loop {
                sleep_ms(1_000).await;
                ticks.update(|value| *value = value.wrapping_add(1));
            }
        });
    }

    /// A [`text`] view pre-styled with [`LABEL_FAMILY`] — every piece of this
    /// app's own authored chrome goes through this rather than a bare
    /// [`text`] call, so it is legible on `wasm32` (see this module's header
    /// doc for why a bare `text()`/`SystemUi` default renders nothing there).
    fn label(content: impl Into<String>) -> TextView {
        text(content).family(FontFamily::named(LABEL_FAMILY))
    }

    /// This app's own hand-rolled tappable row: [`frust_widgets::button`]'s
    /// internal label has no family-override builder, so a `button()` call
    /// would render an invisible (but still clickable) label under the same
    /// `SystemUi` defect [`label`] works around — see this module's header
    /// doc. Composes existing primitives instead ([`GestureDetector`] for the
    /// tap, [`container`] for the fill/radius chrome, [`label`] for the
    /// legible text), all within this crate's own write scope.
    fn nav_row<F: Fn(&mut AppState) + 'static>(
        content: impl Into<String>,
        fill: Color,
        on_tap: F,
    ) -> frust::AnyView<AppState> {
        any(GestureDetector(
            container(Padding(EdgeInsets::symmetric(12.0, 8.0), label(content)))
                .fill(fill)
                .radius(8.0),
        )
        .on_tap(on_tap))
    }

    /// Hosts one [`Case::build`] (a plain `fn() -> frust::AnyView<()>`, per
    /// the registry's pure-`View` constraint) inside this app's `AppState`
    /// tree. [`Component`]'s state boundary — "a `ComponentView` implements
    /// `View<Outer>` for **any** outer state, and the subtree it hosts is
    /// diffed against the component's own `State` instead" (see that trait's
    /// module doc) — is exactly the seam a `()`-state case needs to sit
    /// inside an `AppState`-state app without either side knowing about the
    /// other.
    struct CaseHost(fn() -> frust::AnyView<()>);

    impl Component for CaseHost {
        type State = ();

        fn init(&self) -> Self::State {}

        fn build(&self, _state: &mut Self::State) -> frust::AnyView<Self::State> {
            (self.0)()
        }
    }

    /// The `?case=<slug>` page: a "back to index" row (mouse/touch/keyboard
    /// exercise) and title over the hosted case, scrollable in case a case's
    /// own fixed frame ([`Case::DEFAULT_SIZE`] or an override) is taller than
    /// the viewport. The hosted case's *own* text will not be legible on this
    /// target — see this module's header doc — but its shapes, images,
    /// layout and theme colour still render, and its interaction handling
    /// (hover/press/focus/keyboard) still runs exactly as elsewhere.
    fn case_view(case: &'static Case) -> frust::AnyView<AppState> {
        let header = Row(vec![
            nav_row(
                "< Index",
                Color::from_rgb8(0x2a, 0x33, 0x40),
                |state: &mut AppState| {
                    state.case = None;
                },
            ),
            any(label(format!("{}  [{}]", case.title, case.slug))),
        ]);
        any(FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(any(Padding(EdgeInsets::all(12.0), header))),
                flexible(
                    1,
                    any(scroll_view(Padding(
                        EdgeInsets::all(16.0),
                        component(CaseHost(case.build)),
                    ))),
                ),
            ],
        )
        .cross_axis(CrossAxisAlignment::Stretch))
    }

    /// The fallback/landing page: a live filter box (keyboard text-entry
    /// exercise), a virtualized, wheel/touch-scrollable
    /// ([`frust_widgets::list_view`] carries its own fling/scroll handling —
    /// see that module's doc) list of every matching case (each row a
    /// clickable [`nav_row`] — mouse/touch-tap exercise), the ticking-counter
    /// line ([`start_ticker`]'s zero-input-event proof), and the current
    /// [`WindowMetrics`] as in-UI evidence for the resize/DPR milestone.
    fn index_view(state: &AppState) -> frust::AnyView<AppState> {
        let filter_lower = state.filter.to_lowercase();
        let matches: Vec<&'static Case> = frust_gallery::cases()
            .iter()
            .filter(|case| {
                filter_lower.is_empty()
                    || case.slug.to_lowercase().contains(&filter_lower)
                    || case.title.to_lowercase().contains(&filter_lower)
            })
            .collect();
        let total = frust_gallery::cases().len();
        let shown = matches.len();
        // `Get::get` tracks this read: the ticking background task's write
        // (`start_ticker`) is what marks this build dirty again, never a
        // pointer/key event.
        let ticks = state.ticks.get();
        let metrics_line = use_context::<WindowMetrics>().map_or_else(
            || "window: (metrics not yet published)".to_string(),
            |metrics| {
                format!(
                    "window: {:.0}x{:.0} @ {:.2}x scale — {:?}",
                    metrics.size.width, metrics.size.height, metrics.scale, metrics.orientation
                )
            },
        );

        let header = Column(vec![
            any(label("frust-gallery — browser demo")),
            any(label(format!(
                "{total} cases in the registry ({shown} shown below)"
            ))),
            any(label(format!(
                "signal-driven update, zero input events: ticks = {ticks} \
                 (a background timer writes an RwSignal once a second)"
            ))),
            any(label(metrics_line)),
            any(
                text_input(state.filter.clone(), |state: &mut AppState, value| {
                    state.filter = value;
                })
                .placeholder("filter by slug or title (keyboard input) ...")
                .text_style(frust::authoring::text::TextStyle {
                    family: FontFamily::named(LABEL_FAMILY),
                    ..frust::authoring::text::TextStyle::new(
                        16.0,
                        Color::from_rgb8(0xe6, 0xed, 0xf5),
                    )
                }),
            ),
        ]);

        let list = list_view(shown, 44.0, move |index| {
            let case = matches[index];
            let fill = if index % 2 == 0 {
                Color::from_rgb8(0x22, 0x2a, 0x35)
            } else {
                Color::from_rgb8(0x1a, 0x21, 0x2a)
            };
            nav_row(
                format!("{}  —  {}", case.slug, case.title),
                fill,
                move |state: &mut AppState| {
                    state.case = Some(case);
                },
            )
        });

        any(FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(any(Padding(EdgeInsets::all(16.0), header))),
                flexible(1, any(Padding(EdgeInsets::symmetric(16.0, 0.0), list))),
            ],
        )
        .cross_axis(CrossAxisAlignment::Stretch))
    }

    /// `web_app!`'s `app_logic`: dispatches to [`case_view`]/[`index_view`]
    /// on [`AppState::case`].
    fn view(state: &mut AppState) -> frust::AnyView<AppState> {
        match state.case {
            Some(case) => case_view(case),
            None => index_view(state),
        }
    }

    // The browser counterpart of `android_app!`/`ios_app!`: self-gates to
    // `wasm32` internally (this whole module already is, but the macro's own
    // generated items carry the same `#[cfg]` regardless of caller), expands
    // to a `#[wasm_bindgen(start)]` `__frust_web_start` that installs the
    // panic hook + console sink, brings up the reactive runtime under
    // `AppState::new`, and hands `view` to `frust_shell_web::run_app`.
    frust::web_app!(AppState, AppState::new, view);
}
