//! `frust-glyph`: the Glyph design system, packaged as a design-system plugin.
//!
//! Glyph is a terminal-native, **dark-first**, monospace-led design language —
//! "inspired by Material 3 Expressive, Liquid Glass, and Yaru research ·
//! original tokens". This crate is the whole system: the [`tokens`] module
//! (color schemes, type/shape/elevation/motion/glass scales, the
//! brightness-invariant [`GlyphInk`] extension, the assembled [`baseline`]
//! theme), the bundled monospace faces, the two Glyph [`motion`] patterns, and
//! the widget catalog.
//!
//! The catalog holds two kinds of component. Most are terminal-native ones with
//! no Material or Cupertino equivalent (badges/tags/alerts, loaders + toast, nav
//! chrome, content cards, the terminal block + tooltip, and the command-palette
//! overlay). The rest are ordinary controls whose *authored Glyph design*
//! diverges from what re-theming a baseline widget would produce — the baseline
//! set is token-themed, not re-designed, per design language. [`toggle`] is the
//! first of those: Glyph authors its own switch (a hairline-bordered pill with a
//! constant-diameter springing knob and an accent wash), and the baseline
//! deliberately ships no `Switch` at all for it to re-theme.
//!
//! # Installing it
//!
//! [`install`] is the one-line entry point, and it must run **before the first
//! frame**: a shell reads the default-theme slot and drains the font registry
//! once, at construction. The supported place is `app!`'s `setup` block, which
//! runs before any shell construction:
//!
//! ```no_run
//! use frust::{AnyView, Component, any, text};
//!
//! #[derive(Default)]
//! struct MyApp;
//!
//! impl Component for MyApp {
//!     type State = ();
//!     fn init(&self) -> Self::State {}
//!     fn build(&self, _state: &mut Self::State) -> AnyView<Self::State> {
//!         any(text("glyph"))
//!     }
//! }
//!
//! frust::app!(MyApp, setup = { frust_glyph::install(); });
//! # fn main() {}
//! ```
//!
//! # Namespace
//!
//! Every catalog module is re-exported wholesale from this root, so app code
//! names one flat namespace — `frust_glyph::GlyphBadge`,
//! `frust_glyph::show_glyph_dialog`, `frust_glyph::baseline()` — alongside the
//! `frust::*` baseline widgets a Glyph screen composes with.
//!
//! # Charter
//!
//! - **Token-driven, never design-language-branching.** A Glyph widget
//!   resolves `Theme::from_paint_ctx`/`from_layout_ctx` and reads the Glyph
//!   token tables ([`tokens`]' color schemes, type scale, status palette,
//!   [`GlyphInk`], …) with an unthemed-fallback constant per resolved value —
//!   exactly like every baseline widget. It never matches on `DesignLanguage`;
//!   a Glyph theme is just a `Theme` whose token tables happen to be the Glyph
//!   ones.
//! - **Facade-only.** Widgets are `View`/`Widget` pairs authored against
//!   `frust::authoring` (plus `kurbo`/`peniko` for geometry and color); this
//!   crate names no other framework crate, and nothing reactive.
//!
//! # Fonts
//!
//! The Space Mono and IBM Plex Mono faces the Glyph type scale names are
//! compiled in and registered by [`install`] behind the crate's
//! `bundled-fonts` feature (default on); an app that sets
//! `default-features = false` on this dependency compiles in no font bytes,
//! `install()` registers none, and text falls back to the platform's system
//! faces through fontique (Roboto on Android). Both families are OFL-1.1 and
//! ship with their license text and provenance record
//! (`plugins/glyph/fonts/README.md`). [`font_data`] exposes the raw bytes for
//! a host that wants them directly.
//! Each family's italic face ships too, even though the catalog itself never
//! requests `FontStyle::Italic` — see [`tokens::fonts`]'s module doc for the
//! measurement behind keeping them (dropping them would silently fall back
//! to plain upright text for an app's own italic request, not a synthesized
//! oblique).

pub mod accordion;
pub mod alert;
pub mod appbar;
pub mod avatar;
pub mod badge;
pub mod breadcrumb;
pub mod card;
pub mod command_palette;
pub mod dialog;
pub mod dots;
pub mod empty_state;
pub mod list;
pub mod menu;
pub mod motion;
pub mod navbar;
mod press;
pub mod progress;
pub mod radio;
pub mod reveal;
pub mod segmented;
pub mod sheet;
pub mod side_sheet;
pub mod skeleton;
pub mod stat_card;
pub mod tabs;
pub mod tag;
pub mod term_block;
pub mod toast;
pub mod toggle;
pub mod tokens;
pub mod tooltip;

pub use accordion::*;
pub use alert::*;
pub use appbar::*;
pub use avatar::*;
pub use badge::*;
pub use breadcrumb::*;
pub use card::*;
pub use command_palette::*;
pub use dialog::*;
pub use dots::*;
pub use empty_state::*;
pub use list::*;
pub use menu::*;
pub use navbar::*;
pub use progress::*;
pub use radio::*;
pub use reveal::*;
pub use segmented::*;
pub use sheet::*;
pub use side_sheet::*;
pub use skeleton::*;
pub use stat_card::*;
pub use tabs::*;
pub use tag::*;
pub use term_block::*;
pub use toast::*;
pub use toggle::*;
pub use tooltip::*;

/// The design language itself, flattened to the root alongside the catalog:
/// the assembled [`baseline`] theme, its bundled [`font_data`], the
/// brightness-invariant [`GlyphInk`] extension, and the
/// [`native_typefaces`](tokens::native_typefaces) binding [`baseline`]
/// attaches. The per-scale constructors stay behind [`tokens`].
pub use tokens::{GlyphInk, baseline, font_data};

/// Make the Glyph design system this app's starting point.
///
/// Two process-global pushes, both public `frust` seams:
///
/// 1. `frust::set_default_theme(`[`baseline()`](baseline)`)` — the *base* a
///    shell seeds itself with instead of its built-in `Theme::neutral()`
///    fallback. Deliberately not `set_app_theme`: a seeded default does not pin
///    brightness, so a Glyph app still follows system dark mode.
/// 2. `frust::register_app_fonts` for every bundled Glyph face, so the Glyph
///    type scale's families actually resolve. The faces are compiled in
///    behind the crate's `bundled-fonts` feature (default on) — see the
///    crate docs' *Fonts* section for the `default-features = false`
///    opt-out.
///
/// # Timing: must run before the first frame
///
/// A shell reads the default-theme slot and drains the font registry **once,
/// at construction**, before its first rebuild. A call after that takes effect
/// only on a later `clear_app_theme`-driven reseed, which may never happen — so
/// a late call silently does nothing visible.
///
/// The supported way to get the timing right on all three platforms is
/// `frust::app!`'s setup block, which runs immediately before the root
/// component's `Component::init` and therefore before any shell construction —
/// see the crate docs for the full example. Calling it from `Component::init`
/// itself also happens to be early enough today, but that is not a contract
/// this crate keeps; the setup block is.
///
/// # Thread contract and repeat calls
///
/// Both underlying seams are plain `Mutex`-guarded process-globals callable
/// from any thread. Calling `install` twice is harmless but wasteful: the
/// second `set_default_theme` replaces an identical value, and the font bytes
/// are pushed (and later re-registered, shadowing the same family names) a
/// second time. Call it once.
pub fn install() {
    frust::set_default_theme(baseline());
    for bytes in font_data() {
        frust::register_app_fonts(bytes.to_vec());
    }
}
