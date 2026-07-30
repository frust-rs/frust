//! Compile guard for the catalog-free build.
//!
//! With every framework feature default-on, nothing else in this workspace
//! ever compiles `frust` with `default-features = false` — the whole
//! opt-out this crate's `Cargo.toml` exercises (`glyph`/`material`/
//! `cupertino`/`glyph-fonts` all off). This crate is a root-workspace
//! member so a plain `cargo build -p no-catalogs`/`cargo check -p
//! no-catalogs` type-checks that configuration with nothing to remember.
//!
//! Deliberately tiny — a compile guard, not a showcase. Baseline widgets
//! only (`text`/`Column`/`Button`/`SizedBox`), no catalog widget
//! (`frust::glyph::*`/`frust::material` re-exports/`frust::cupertino`
//! re-exports), no Glyph token, no bundled font, and no
//! `frust::glyph_theme::install()` call — every one of those is unreachable
//! **in an isolated `-p no-catalogs` build** (verified: referencing
//! `frust::glyph::badge` there is E0433, "found an item that was configured
//! out"), so adding any of them here is a compile error by construction, as
//! long as it's checked that way. Every extra widget beyond the four below
//! is one more thing that can break this guard for reasons unrelated to the
//! catalog opt-out, so resist the urge to grow it into a showcase.
//!
//! # Confirmed limitation: `--workspace` does NOT guard this
//!
//! **`cargo build --workspace`/`cargo test --workspace` do not actually
//! exercise the catalog-off configuration**, despite this crate compiling
//! successfully under both. `plugins/native-widgets` (also a root-workspace
//! member) depends on `frust` too, via `frust = { workspace = true, optional
//! = true }` with its own `default = ["frust-api", "glyph-fonts"]` — no
//! `default-features = false` override, and legitimately so: its theme
//! ladder L3 needs `frust-theme`'s bundled Glyph font bytes to resolve a
//! real `Typeface`/`CTFont`. Cargo's feature resolver unifies two normal,
//! same-target-platform dependents of the same package into ONE build using
//! the *union* of every requested feature — verified directly against this
//! repo (`cargo clean -p frust-theme -p frust-widgets -p frust`, then
//! compare rustc invocations):
//!
//! - `cargo build -p no-catalogs` (isolated): `frust`/`frust-widgets`/
//!   `frust-theme` each compile with **zero** `--cfg 'feature="..."'`
//!   flags; `strings` on the resulting `frust_theme` rlib has 0 hits for
//!   "Space Mono".
//! - `cargo build --workspace`: the SAME `frust`/`frust-widgets`/
//!   `frust-theme` package instances compile ONCE, with `--cfg
//!   'feature="cupertino"' --cfg 'feature="default"' --cfg
//!   'feature="glyph"' --cfg 'feature="glyph-fonts"' --cfg
//!   'feature="material"'` all active (driven by `frust-native-widgets`'s
//!   request) — this crate's binary links against that SAME unified rlib,
//!   and `strings` on it finds 6 hits for "Space Mono". A `frust::glyph::*`
//!   reference added here **compiles successfully** under `cargo check
//!   --workspace -p no-catalogs`, even though the identical reference is
//!   E0433 under `cargo check -p no-catalogs` alone.
//!
//! This is a real Cargo feature-unification limit, not a bug in this
//! crate's `Cargo.toml` — there is no stable-Cargo way to keep one
//! workspace member's dependency on a shared package feature-isolated from
//! a sibling member's dependency on the same package at the same
//! (normal-deps, host-target) resolution key. Fixing it would mean either
//! (a) `plugins/native-widgets` giving up its legitimate default-on
//! `glyph-fonts` need, or (b) this crate leaving the root workspace
//! (defeating its own purpose) — neither is this task's call to make.
//!
//! **The reliable gate is therefore the standalone, package-scoped
//! command** (`cargo build -p no-catalogs`/`cargo check -p no-catalogs`, or
//! equivalently `cargo build -p frust --no-default-features`), not the bare
//! `--workspace` invocation — see `docs/DEVELOPMENT.md`'s Test section
//! (flagged for a doc update to add this as an explicit gate step). This
//! crate still stays a workspace member because it (1) still catches gross
//! breakage of the no-catalogs configuration's *source* the moment someone
//! runs the standalone command, and (2) still compiles under `--workspace`
//! today, so it costs nothing to keep as a companion to that documented
//! fallback.

struct AppState {
    count: i32,
}

fn app_logic(state: &mut AppState) -> impl frust::View<AppState> + use<> {
    frust::Column(vec![
        frust::any(frust::text(format!("count: {}", state.count)).size(24.0)),
        frust::any(frust::SizedBox(Some(0.0), Some(12.0))),
        frust::any(frust::Button("increment", |state: &mut AppState| {
            state.count += 1;
        })),
    ])
}

fn main() {
    frust::App::new(AppState { count: 0 }, app_logic)
        .run()
        .unwrap();
}
