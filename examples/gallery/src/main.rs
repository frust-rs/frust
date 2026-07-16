//! Desktop entry point for the gallery demo (spec §5, §12.9).
//!
//! `forgekit::app!(GalleryApp)` in `lib.rs` generates the hidden
//! `__forgekit_main` this calls — the one-line `main.rs` split every
//! `forgekit::app!`-based app uses (never hand-edited; `forgekit create`
//! scaffolds this exact shape).

fn main() {
    gallery::__forgekit_main();
}
