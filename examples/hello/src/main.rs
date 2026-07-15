//! The Phase 0 exit-criterion example: render "Hello from ForgeKit" in the
//! desktop preview window (spec §5, §14).

/// Application state — just the greeting to display.
struct AppState {
    greeting: String,
}

/// Map the app state to a view tree. Re-run each frame (spec §5).
///
/// `+ use<>` opts the return type out of capturing the `&mut AppState`
/// lifetime — required in edition 2024, where `-> impl Trait` otherwise
/// captures all in-scope lifetimes. The view borrows nothing from `state`
/// (views are `'static`), so capturing nothing is correct.
fn app_logic(state: &mut AppState) -> impl forgekit::View<AppState> + use<> {
    forgekit::text(state.greeting.clone()).size(32.0)
}

fn main() {
    forgekit::App::new(
        AppState {
            greeting: "Hello from ForgeKit".into(),
        },
        app_logic,
    )
    .run()
    .unwrap();
}
