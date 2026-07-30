//! Runtime proof that this catalog's `frust::app!(CatalogApp, setup = {
//! frust::glyph_theme::install(); })` block actually seeds Glyph — not just
//! that the crate compiles.
//!
//! `app!`'s `setup` block runs `frust::glyph_theme::install()` before any
//! shell reads the default-theme slot (`crates/frust/src/lib.rs`'s
//! `run_with_setup`/`app!` docs). A headless `RenderRoot`-driven test (like
//! `tests/smoke.rs`) never exercises that slot at all — every existing test
//! either `provide_context`s its own `Theme` or lets a page fall back to the
//! hardcoded `Theme::glyph_baseline()` constant, both of which would look
//! identical whether or not `install()` ever ran. The only way to prove the
//! wiring itself did something is to call the exact `setup` payload the
//! macro expands to and read back the process-global slot it writes,
//! `frust_shell_common::default_theme()` — the same slot every real shell
//! (desktop/Android/iOS) reads once at construction (see
//! `docs/ARCHITECTURE.md`'s Theme delivery).
//!
//! This does not (and cannot, headlessly) prove a live shell actually
//! *renders* Glyph — that remains the on-device visual gate `docs/DEVELOPMENT.md`
//! already calls manual/outstanding.

use frust::{DesignLanguage, Theme};

#[test]
fn setup_block_seeds_glyph_as_the_default_theme() {
    // The exact payload `frust::app!(CatalogApp, setup = {
    // frust::glyph_theme::install(); })` expands to (see `crates/frust/src/lib.rs`'s
    // `app!` macro) — called directly here since driving the real macro
    // expansion would require a live shell (window/GPU) this headless test
    // doesn't have.
    frust::glyph_theme::install();

    let seeded = frust_shell_common::default_theme()
        .expect("glyph_theme::install() must seed the default-theme slot");

    assert_eq!(
        seeded.design_language,
        DesignLanguage::Glyph,
        "the seeded default must be the Glyph design language, not the shell's own \
         Theme::neutral() fallback",
    );
    // Glyph is dark-first (`Theme::glyph_baseline()`'s own doc/tests) — a
    // Glyph-only-shaped assertion beyond the design-language tag alone.
    assert_eq!(
        seeded,
        Theme::glyph_baseline(),
        "the seeded default must be byte-identical to Theme::glyph_baseline()",
    );
}
