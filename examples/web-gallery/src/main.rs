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
//! - `?theme=light` / `?theme=dark`, orthogonal to the above: an embedding
//!   host's explicit request to pin one appearance end-to-end
//!   ([`app::resolve_theme_override_from_query`]), overriding whatever the
//!   host itself reports.
//! - `?embed=1`, orthogonal to both of the above and layered on top of
//!   `?case=`: strips this app's own chrome away entirely, down to exactly
//!   the hosted [`frust_gallery::Case`] — no "‹ Index" header row, no debug
//!   title, no outer `scroll_view`/padding
//!   ([`app::embedded_case_view`] vs. the chromed [`app::case_view`]) — for a
//!   preview host that sizes an iframe from the case's own bare poster
//!   ([`app::resolve_embed_from_query`]). Any value other than the exact
//!   string `"1"` (including absent) leaves the existing chromed `?case=`
//!   page unchanged; this is additive, not a replacement, the identical
//!   contract `?theme=` already established. Resolved once at construction
//!   and never toggled after, so an embedded frame has no path back to the
//!   index page — see [`app::AppState::embed`]'s own doc.
//!
//! Dark/light theme **follow** and resize/DPR handling need no app code at
//! all when `?theme=` is absent: both are the browser shell's own job
//! (`crates/frust-shell-web`'s `follow_platform_brightness`/window-metrics
//! publish, from w1-02/w1-03). `?theme=` is the one exception — this app's
//! own [`frust::set_app_theme`] call, made because an embedding host (a
//! preview iframe, say) may want to force one appearance regardless of what
//! it or the browser reports; see [`app::AppState::new`]'s call site. The
//! index page's `window: ... — Orientation` line ([`frust::WindowMetrics`]
//! read via `use_context`) is this app's one piece of in-UI evidence that a
//! resize/DPR change actually reached the tree, alongside the screenshots
//! recorded in README.md. Reporting this page's own rendered height to an
//! embedding parent frame (the `postMessage({type:"frust:height",...})`
//! contract) is likewise not this crate's own code — see `index.html`'s
//! header comment for where that lives.
//!
//! # Text on `wasm32`: `SystemUi`/`SansSerif` and the bundled design-system faces resolve; the `Monospace`/`Serif`/`Emoji` generics still do not
//!
//! Every text-bearing widget defaults to [`frust_text::FontFamily::SystemUi`]
//! (`TextStyle::default()`) unless a caller sets `.family(...)` explicitly —
//! and, as of this task, **no case in the whole `frust-gallery` registry
//! does** (`grep -rn '\.family(' examples/gallery/src` is empty). On desktop
//! that resolves through fontique's real system-font backend; on
//! `wasm32-unknown-unknown` fontique ships a documented "dummy system font
//! backend" with an empty generic-family map (see
//! `fontique-0.11.1/src/backend/mod.rs`). That was a real defect for
//! `SystemUi` and the generic `NamedWithGeneric([], SansSerif)` stack
//! [`Theme::neutral`]'s own type scale carries too — but it is fixed at the
//! framework level now, not worked around per-app: `crates/frust-shell-web`'s
//! `install_default_fonts` registers a bundled Inter Variable face as the
//! [`frust_text::GenericSlot::SystemUi`]/[`frust_text::GenericSlot::SansSerif`]
//! generic-family fallback, unconditionally, before the shell builds its own
//! `TextContext` — no app code required. See `docs/LIMITATIONS.md`'s
//! `web-generic-family-partial-fallback` entry for the full record:
//! `Monospace`, `Serif`, and `Emoji` remain **unmapped** on this target (a
//! proportional face substituted for `Monospace` would silently regress
//! `TextInput`/code-display layout, and the bundled face set has neither a
//! serif nor an emoji face), so text explicitly requesting one of those
//! three still resolves zero glyphs there — and cases in this registry do
//! request `Monospace`: both Glyph faces are
//! `stack_with_generic([...], GenericSlot::Monospace)`
//! (`plugins/glyph/src/tokens/scales.rs`) and every Glyph type token uses
//! one of them; the `shadcn` and `beui` mono slots carry the same tail.
//!
//! **That half is this binary's own job, and it now does it**
//! ([`app::register_design_system_fonts`], called from
//! [`app::AppState::new`] before `run_app` constructs the shell): the
//! bundled Glyph, shadcn and beUI faces are handed to
//! [`frust::register_app_fonts`] unconditionally, so the *named* half of
//! each of those stacks ("Space Mono"/"IBM Plex Mono", "JetBrains Mono",
//! "Geist Mono") resolves and the unmapped generic tail is never reached.
//! Before that call, all 14 `glyph/*` cases rendered zero glyphs here while
//! their CPU snapshots looked perfectly correct, because the snapshot host
//! has the real faces. They could not inherit them: the registry's Glyph
//! cases are built without `frust_glyph::install`
//! (`examples/gallery/src/glyph.rs`), the only seam that registers them.
//! **Registering a face is not the same as mapping a generic**, and this
//! changes only the former — text that resolves to a bare
//! `Monospace`/`Serif`/`Emoji` generic with no named face in front of it
//! still renders nothing on this target. See
//! [`app::register_design_system_fonts`] for why Material (no `Monospace`
//! slot at all, and no public accessor for its bundled bytes) and Cupertino
//! (a named SF Pro stack on every role, but no bundled font files to
//! register) are not registered and do not need to be.
//!
//! This app's own [`app::LABEL_FAMILY`]/[`app::nav_row`]
//! hand-registration (a *named*-family route through
//! [`frust::register_app_fonts`], predating the shell fix above) is now
//! redundant per `frust-shell-web`'s own doc comment on
//! `install_default_fonts` ("once an app ... calls this seam, that hand
//! registration is redundant and can be dropped in favor of this default")
//! — left in place since dropping it is outside this task's scope, and a
//! named lookup winning over a generic one makes keeping it harmless. A
//! hosted [`frust_gallery::Case`] takes that same `SystemUi`/
//! [`Theme::neutral`] default only when it is a `Design::Base` case:
//! [`app::AppState::new`] installs the case's own design-system theme
//! through `frust::set_default_theme` for every non-`Base` case. Material,
//! shadcn and beUI body text tails `SansSerif` and always rendered;
//! Cupertino sets `FontFamily::stack(["SF Pro Display" | "SF Pro Text",
//! "SF Pro"])` on every type role (`plugins/cupertino/src/tokens.rs`'s
//! `sf_family_for_size`/`apply_type`) — a named stack with **no** generic
//! tail, so it resolves through parley's own fallback once those names miss
//! rather than through a mapped generic slot, and its cases render;
//! Glyph's whole type scale, and the shadcn/beUI mono slots, tail
//! `Monospace` and render through the named faces registered above — see
//! README.md's `wasm32` text section for the full writeup and the
//! browser-measured evidence.
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
        Color, Component, CrossAxisAlignment, EdgeInsets, GestureDetector, Get, Padding, RwSignal,
        TextView, Update, WindowMetrics, any, column, component, container, list_view, row,
        scroll_view, text, text_input, use_context,
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
    /// `SystemUi`/`SansSerif` section for why this app registered a face of
    /// its own before the shell fix landed, and why a registered face is
    /// still not enough to fix every case's own text.
    const LABEL_FAMILY: &str = "Inter Variable";
    const LABEL_FONT_BYTES: &[u8] =
        include_bytes!("../../../plugins/shadcn/fonts/inter/InterVariable.ttf");

    /// Hand every bundled design-system face this binary can reach to
    /// [`frust::register_app_fonts`], once, before the shell exists.
    ///
    /// Without this, all 14 `glyph/*` cases render **zero glyphs** on
    /// `wasm32`, under posters that look perfectly correct. Both Glyph type
    /// faces are `stack_with_generic([..], GenericSlot::Monospace)`
    /// (`plugins/glyph/src/tokens/scales.rs`), the named half of that stack
    /// resolves only against faces someone registered, and the generic tail
    /// is deliberately unmapped on this target
    /// (`crates/frust-shell-web/src/fonts.rs` maps `SystemUi`/`SansSerif`
    /// only — `docs/LIMITATIONS.md`'s `web-generic-family-partial-fallback`).
    /// The registry builds its Glyph cases directly rather than through
    /// `frust_glyph::install` (`examples/gallery/src/glyph.rs` explains why),
    /// so nothing else in this binary ever registers them. The shadcn and
    /// beUI mono slots (`JetBrains Mono`, `Geist Mono`) sit on the identical
    /// tail; their sans body text was always fine.
    ///
    /// # Why these three, and no `install()` call
    ///
    /// `frust_{glyph,shadcn,beui}::install()` would also
    /// `set_default_theme`, fighting [`AppState::new`]'s own per-case
    /// seeding below; `font_data()` is the pure-bytes half of exactly what
    /// `install` registers, so this is that call's font half and nothing
    /// else. Material is absent because its type scale carries no
    /// `Monospace` slot at all — its one stack is `[Roboto Flex] +
    /// SansSerif`, which the shell already maps — and because its bundled
    /// bytes have no public accessor anyway (`plugins/material`'s `tokens`
    /// module is private and its crate root does not re-export
    /// `font_data`, unlike the three below). Cupertino is absent for a
    /// different reason than "no family": it *does* declare one — a named
    /// `FontFamily::stack(["SF Pro Display" | "SF Pro Text", "SF Pro"])` on
    /// every type role (`plugins/cupertino/src/tokens.rs`'s
    /// `sf_family_for_size`, applied by `apply_type` and wired through
    /// `type_scale` into `baseline`) — but with **no** generic tail and, the
    /// decisive part, **no bundled font files at all**: that crate has no
    /// `fonts/` directory, no `include_bytes!` and no `font_data`, so there
    /// is nothing here to register. Its named stack misses and resolves
    /// through parley's own fallback instead, which is why its cases render
    /// without this function's help. Neither plugin can hit the defect this
    /// function fixes; see this crate's `Cargo.toml` for the same note
    /// beside the missing rows.
    ///
    /// # Why unconditionally, rather than per-case
    ///
    /// Registering only the on-screen case's own design would be thriftier
    /// at runtime — ~1.0 MB (Glyph) instead of ~2.5 MB of `Vec<u8>` **heap**
    /// copies per module instance, `register_app_fonts` taking owned bytes;
    /// it saves no payload — but it would be wrong for the way this app is
    /// navigated.
    /// A shell reads the registry at construction; `?case=` is resolved in
    /// the same breath, but the index page's own list writes
    /// [`AppState::case`] at *runtime*, long after that, so a per-case
    /// scheme would have to re-register on every navigation and lean on the
    /// shell's once-per-frame re-drain to recover. That is more machinery
    /// than a demo binary whose entire job is proving text renders should
    /// carry, and it re-opens the exact blank-text failure mode this
    /// function closes. The cost it buys off is bounded, one-time, and heap
    /// only: the *payload* is identical either way, since a per-case `match`
    /// would name all three `font_data()`s and keep every face reachable
    /// just the same.
    ///
    /// # This is not free — measured, not assumed
    ///
    /// All three plugins do bundle unconditionally (`include_bytes!`, no
    /// feature gate), but "bundled" is not "linked": before this call the
    /// only faces surviving into the artifact were the one or two each
    /// plugin's `native_typefaces` binding references (Space Mono Regular,
    /// IBM Plex Mono Regular, Inter, Geist), and the linker dropped the
    /// other seven as unreachable. Turning them on grew the module by
    /// 1,221,759 B after `wasm-opt` (+11.4%) and 560,157 B gzipped
    /// (+12.5%) — 585 B off the 1,222,344 B those seven faces weigh on
    /// disk. README.md's `wasm32` text section carries the full before/after
    /// table; the lever, if that ever proves too expensive, is registering
    /// each family's Regular only, never dropping a family.
    fn register_design_system_fonts() {
        for bytes in frust_glyph::font_data()
            .iter()
            .chain(frust_shadcn::font_data())
            .chain(frust_beui::font_data())
        {
            frust::register_app_fonts(bytes.to_vec());
        }
    }

    /// The app's whole retained state: which case (if any) is showing, the
    /// index page's live filter text, and the zero-input-event ticking
    /// counter's signal handle.
    pub struct AppState {
        /// `Some` while a single case is on screen (from `?case=` or a click
        /// on the index list); `None` shows the index page.
        case: Option<&'static Case>,
        /// `true` when `?embed=1` requested the chrome-free single-case view
        /// (see [`resolve_embed_from_query`]); read once here and never
        /// written again anywhere else in this app — [`view`]'s dispatch on
        /// `(case, embed)` renders [`embedded_case_view`] instead of
        /// [`case_view`] in that case, and `embedded_case_view` never draws
        /// the "back to index" affordance that is the *only* code path
        /// anywhere in this app that ever sets `case` back to `None`. So an
        /// embedded frame has no in-app way back to [`index_view`].
        embed: bool,
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
            // Redundant twice over now: `frust-shell-web`'s
            // `install_default_fonts` maps `SystemUi`/`SansSerif`
            // unconditionally (see this module's header doc), and
            // `register_design_system_fonts` below registers shadcn's own
            // copy of these exact bytes — `LABEL_FONT_BYTES` `include_bytes!`s
            // the same `plugins/shadcn/fonts/inter/InterVariable.ttf` the
            // plugin bundles. Kept anyway: dropping it would make this app's
            // own chrome depend on a plugin's bundle, and re-registering one
            // already-registered family is harmless.
            //
            // It carries NO duplicate payload, contrary to what an earlier
            // pass of this comment claimed: identical `include_bytes!`
            // constants merge, so the two sites are one copy in the
            // artifact. Verified by byte-probing the shipped module —
            // three 4096-byte probes from InterVariable.ttf each match
            // exactly once, the same result the single-site
            // SpaceMono-Regular control gives. Retiring this call is a
            // clarity change worth roughly zero bytes, not the ~0.88 MB win
            // that claim implied.
            //
            // Both calls run before `run_app` constructs the shell,
            // matching the timing contract `frust::register_app_fonts`'s own
            // doc and `frust_shadcn::install` (the precedent this follows)
            // both spell out: "a shell reads ... and drains the font registry
            // once, at construction".
            frust::register_app_fonts(LABEL_FONT_BYTES.to_vec());
            register_design_system_fonts();

            let case = resolve_case_from_query();
            // See [`resolve_embed_from_query`]'s own doc for the exact
            // parsing/fall-back contract; resolved once here, exactly like
            // `case` above, and never re-read.
            let embed = resolve_embed_from_query().unwrap_or(false);
            // `?theme=light|dark` (see [`resolve_theme_override_from_query`])
            // takes precedence over the design-tagged-case seeding below: it
            // is an explicit embedding-host request to pin one appearance
            // end-to-end, exactly `frust::set_app_theme`'s own contract
            // ("never overridden back by a live platform dark-mode flip
            // until `clear_app_theme` runs"). Host-follow with no query
            // parameter is the shell's own job (`Window::theme()`/
            // `prefers-color-scheme` on this target) and needs no app code
            // at all — this app only ever forces brightness when asked to.
            if let Some(variant) = resolve_theme_override_from_query() {
                let design = case.map_or(Design::Base, |case| case.design);
                frust::set_app_theme(frust_gallery::theme(design, variant));
            } else if let Some(case) = case
                && case.design != Design::Base
            {
                // A design-tagged case (`material/button`, `shadcn/button`,
                // ...) wants that design system's own theme underneath it,
                // not the framework's neutral baseline; `Base` cases render
                // correctly either way, so they are left alone rather than
                // pinned. This is `set_default_theme`, not `set_app_theme`:
                // it seeds the base the shell still re-derives light/dark
                // from on every platform brightness change, so a
                // design-tagged case still honours the OS/browser
                // theme-follow milestone instead of freezing one brightness
                // — see `frust::set_default_theme`'s own doc for the
                // precedence order this relies on.
                frust::set_default_theme(frust_gallery::theme(case.design, Variant::Light));
            }

            let ticks = RwSignal::new(0u32);
            start_ticker(ticks);

            Self {
                case,
                embed,
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

    /// Reads `?theme=<variant>` off `window.location.search`: `Some(Variant::
    /// Light)` for `light`, `Some(Variant::Dark)` for `dark`, `None` for a
    /// missing query parameter, an unavailable `window`/`Location`, or any
    /// other value — every one of those leaves brightness on the shell's own
    /// host-follow path (see [`AppState::new`]'s call site), the identical
    /// "fall back rather than fail" contract [`resolve_case_from_query`]
    /// follows for `?case=`.
    fn resolve_theme_override_from_query() -> Option<Variant> {
        let window = web_sys::window()?;
        let search = window.location().search().ok()?;
        let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
        match params.get("theme")?.as_str() {
            "light" => Some(Variant::Light),
            "dark" => Some(Variant::Dark),
            _ => None,
        }
    }

    /// Reads `?embed=1` off `window.location.search`: `Some(true)` for the
    /// exact value `"1"`, `Some(false)` for any other value present, `None`
    /// for a missing query parameter or an unavailable `window`/`Location` —
    /// [`AppState::new`]'s call site collapses `None` and `Some(false)` alike
    /// via `.unwrap_or(false)`, the identical "fall back rather than fail"
    /// contract [`resolve_case_from_query`]/[`resolve_theme_override_from_query`]
    /// both follow for `?case=`/`?theme=`.
    fn resolve_embed_from_query() -> Option<bool> {
        let window = web_sys::window()?;
        let search = window.location().search().ok()?;
        let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
        Some(params.get("embed")?.as_str() == "1")
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
    /// [`text`] call — originally because a bare `text()`/`SystemUi` default
    /// resolved nothing on `wasm32`, and now belt-and-braces over the shell's
    /// own generic fallback (see this module's header doc).
    fn label(content: impl Into<String>) -> TextView {
        text(content).family(FontFamily::named(LABEL_FAMILY))
    }

    /// This app's own hand-rolled tappable row: [`frust_widgets::button`]'s
    /// internal label has no family-override builder, so a `button()` call
    /// could not be pinned to [`LABEL_FAMILY`] the way [`label`] is — which
    /// mattered while `SystemUi` resolved nothing on `wasm32`, and is now
    /// merely a lost override (see this module's header doc). Composes
    /// existing primitives instead ([`GestureDetector`] for the
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

    /// Hosts one case constructor (a plain `fn() -> frust::AnyView<()>`, per
    /// the registry's pure-`View` constraint) inside this app's `AppState`
    /// tree. [`Component`]'s state boundary — "a `ComponentView` implements
    /// `View<Outer>` for **any** outer state, and the subtree it hosts is
    /// diffed against the component's own `State` instead" (see that trait's
    /// module doc) — is exactly the seam a `()`-state case needs to sit
    /// inside an `AppState`-state app without either side knowing about the
    /// other. Which constructor it is asked to host — [`Case::build`] or the
    /// interactive one — is [`case_constructor`]'s decision, not this type's.
    struct CaseHost(fn() -> frust::AnyView<()>);

    impl Component for CaseHost {
        type State = ();

        fn init(&self) -> Self::State {}

        fn build(&self, _state: &mut Self::State) -> impl frust::View<Self::State> {
            (self.0)()
        }
    }

    /// The constructor a hosted case is actually built from: the registry's
    /// interactive one when it declares one for this slug, else the case's own
    /// [`Case::build`].
    ///
    /// This app is a LIVE host, not the recorder, and that is the whole
    /// difference. A `Case::build` is `fn() -> AnyView<()>`, so every input
    /// widget it constructs is handed a `|_: &mut (), _| {}` callback — frust's
    /// widgets are controlled (they report a requested value and wait for the
    /// next rebuild to feed it back), so a click on one changes nothing here,
    /// which is right for a deterministic recorded frame and wrong for a page
    /// a person is meant to operate. `frust_gallery::find_interactive` answers
    /// with a constructor carrying retained `Component` state where one exists.
    ///
    /// Both pages resolve through this one function so the chromed and
    /// embedded routes cannot drift; the `?embed=1` iframe the website's
    /// preview component builds is the one that matters most, since it is what
    /// a reader actually touches.
    fn case_constructor(case: &'static Case) -> fn() -> frust::AnyView<()> {
        frust_gallery::find_interactive(case.slug).unwrap_or(case.build)
    }

    /// The `?case=<slug>` page (the human-browsing default; see
    /// [`embedded_case_view`] for the `?embed=1` sibling that strips this
    /// chrome away): a "back to index" row (mouse/touch/keyboard exercise)
    /// and title over the hosted case, scrollable in case a case's own fixed
    /// frame ([`Case::DEFAULT_SIZE`] or an override) is taller than the
    /// viewport. The hosted case's own text now renders on this target for
    /// the common `SystemUi`/`SansSerif` defaults — see this module's header
    /// doc — though a case naming `Monospace`/`Serif`/`Emoji` explicitly
    /// would still resolve no glyphs there; its shapes, images, layout and
    /// theme colour render regardless, and its interaction handling
    /// (hover/press/focus/keyboard) runs exactly as elsewhere.
    fn case_view(case: &'static Case) -> frust::AnyView<AppState> {
        let header = row()
            .child(nav_row(
                "< Index",
                Color::from_rgb8(0x2a, 0x33, 0x40),
                |state: &mut AppState| {
                    state.case = None;
                },
            ))
            .child(label(format!("{}  [{}]", case.title, case.slug)));
        any(column()
            .child(Padding(EdgeInsets::all(12.0), header))
            .flex(
                1,
                scroll_view(Padding(
                    EdgeInsets::all(16.0),
                    component(CaseHost(case_constructor(case))),
                )),
            )
            .cross_axis(CrossAxisAlignment::Stretch))
    }

    /// The `?case=<slug>&embed=1` page: the hosted case, and *nothing*
    /// else — no header `Row`, no `nav_row`, no debug title, no outer
    /// `scroll_view` or extra `Padding`, unlike [`case_view`]. This is the
    /// whole point of `?embed=1` (see this file's module doc): a preview
    /// host sizes its iframe from the case's own bare poster
    /// ([`Case::DEFAULT_SIZE`], the exact frame `frust-testing`'s CPU oracle
    /// renders the case at with no chrome), so any header/padding here would
    /// show as cropped, squeezed-in content inside that same box instead of
    /// matching it. Structurally inescapable: there is no
    /// `nav_row`/`GestureDetector` anywhere in this function's output, so
    /// nothing it renders can ever set [`AppState::case`] back to `None` —
    /// the only way out of an embedded frame is the host page itself (e.g.
    /// navigating the iframe's own `src`), never a tap inside it.
    fn embedded_case_view(case: &'static Case) -> frust::AnyView<AppState> {
        any(component(CaseHost(case_constructor(case))))
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

        let header = column()
            .child(label("frust-gallery — browser demo"))
            .child(label(format!(
                "{total} cases in the registry ({shown} shown below)"
            )))
            .child(label(format!(
                "signal-driven update, zero input events: ticks = {ticks} \
                 (a background timer writes an RwSignal once a second)"
            )))
            .child(label(metrics_line))
            .child(
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
            );

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

        any(column()
            .child(Padding(EdgeInsets::all(16.0), header))
            .flex(1, Padding(EdgeInsets::symmetric(16.0, 0.0), list))
            .cross_axis(CrossAxisAlignment::Stretch))
    }

    /// `web_app!`'s build closure: dispatches to
    /// [`case_view`]/[`embedded_case_view`]/[`index_view`] on
    /// [`AppState::case`] and [`AppState::embed`]. `embed` only chooses
    /// between the two case pages — a missing/unknown `?case=` still falls
    /// back to the index page exactly as before this task, `embed` or not.
    fn view(state: &mut AppState) -> frust::AnyView<AppState> {
        match state.case {
            Some(case) if state.embed => embedded_case_view(case),
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
