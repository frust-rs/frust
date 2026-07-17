//! Desktop preview entry point (spec §12.9 dev loop / §5.5 canonical app shape).
//!
//! `forgekit::app!` (in `lib.rs`) generates the hidden `__forgekit_main` this
//! one line calls — see `templates/app/src/main.rs.tmpl` for the identical,
//! canonical shape a `forgekit create`-scaffolded app never hand-edits.

// On Android the app is driven by the JNI bridge (`forgekit::app!` in
// `lib.rs`), not this desktop preview binary — and `__forgekit_main` is
// compiled out for the Android target. Gate the desktop `main` accordingly,
// mirroring the scaffolded template.
#[cfg(not(target_os = "android"))]
fn main() {
    navdemo::__forgekit_main();
}

#[cfg(target_os = "android")]
fn main() {}
