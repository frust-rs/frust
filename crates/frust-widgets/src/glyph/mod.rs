//! The Glyph widget catalog: terminal-native, monospace-led components that
//! have no Material or Cupertino equivalent (badges/tags/alerts, loaders +
//! toast, nav chrome, content cards, the terminal block + tooltip, and the
//! command-palette overlay).
//!
//! # Charter
//!
//! - **Token-driven, never design-language-branching.** A Glyph widget
//!   resolves `Theme::from_paint_ctx`/`from_layout_ctx` and reads the Glyph
//!   token tables ([`frust_theme`]'s `ColorScheme::glyph_*`, `TypeScale::glyph`,
//!   `StatusPalette::glyph`, `GlyphInk`, …) with an unthemed-fallback constant
//!   per resolved value — exactly like every other widget in this crate. It
//!   never matches on `DesignLanguage`; a Glyph theme is just a `Theme` whose
//!   token tables happen to be the Glyph ones.
//! - **Reactive-free**, like the rest of `frust-widgets`: `View`/`Widget`
//!   pairs over `frust-core` + `frust-text` + `frust-theme`, no `frust-reactive`.
//!
//! # Catalog scaffold (this module)
//!
//! Every catalog module is **pre-declared here with a wholesale re-export**
//! (`pub use badge::*;`) so a new widget adds its `View`/
//! `Widget`/spec types to its own file and its public items ride along
//! with **no edit to this module list** (the icons-precedent same-file
//! discipline). The crate root
//! (`frust-widgets/src/lib.rs`) exposes this module as `pub mod glyph;` and the
//! `frust` facade re-exports it wholesale (`pub use frust_widgets::glyph;`), so
//! app code names `frust::glyph::*`.
//!
//! **Scaffold-only lint note:** the wholesale `pub use <module>::*;` lines
//! below would re-export nothing if a submodule were an empty doc-only stub,
//! which would fire rustc's `unused_imports` under the workspace's `-D
//! warnings` gate. Rather than drop the
//! re-exports to plain `pub mod` visibility — which would break the flattening
//! contract the facade's wholesale `pub use frust_widgets::glyph;` relies on
//! (a new widget's `pub` items must reach `frust::glyph::*` with no `mod.rs`
//! edit) —
//! this one `#![allow(unused_imports)]` masks the **transient** empty-stub
//! warning. It self-clears per module the moment
//! a first `pub` item is added, and stays harmless thereafter.
#![allow(unused_imports)]

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
pub mod navbar;
pub mod progress;
pub mod segmented;
pub mod skeleton;
pub mod stat_card;
pub mod tabs;
pub mod tag;
pub mod term_block;
pub mod toast;
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
pub use segmented::*;
pub use skeleton::*;
pub use stat_card::*;
pub use tabs::*;
pub use tag::*;
pub use term_block::*;
pub use toast::*;
pub use tooltip::*;
