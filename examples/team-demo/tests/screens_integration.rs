//! Real-UI integration coverage for the settings/notes/profile screens (task
//! 07, groups (b)/(c)/(d)): synthetic pointer taps and keystrokes driven
//! through the *mounted* [`ShellApp`] tree (the same headless `RenderRoot`
//! harness `tests/team.rs` established), reaching each screen's private
//! controller only through its real, wired-up widgets — `SettingsScreen`,
//! `NotesScreen`, and `ProfileScreen` (and their controllers) all live in
//! private modules (`mod screens`/`mod notes_domain`/`mod profile_domain`/
//! `mod settings_domain` — see the crate's placeholder-contract docs), so an
//! external `tests/*.rs` crate cannot import them directly the way
//! `tests/team.rs` reaches `TeamController` through the crate's public `TeamSpy`
//! seam. These tests locate the real widgets by their painted rounded-rect
//! chrome (mirroring `tests/team.rs`'s own Dismiss-button-location trick) and
//! observe effects through glyph/rounded-rect counts, exactly like every other
//! test in this crate that has no direct controller handle.
//!
//! # `(b)` — SettingsController's `set_app_theme` effect: an accepted gap
//!
//! The task's acceptance criterion (b) asks this suite to "assert via the
//! theme context/override state" that a language/brightness change fires
//! `set_app_theme`. That state is `forgekit_shell_common::theme_override`'s
//! process-global slot, readable only through
//! `forgekit_shell_common::{theme_override_active, ThemeOverrideWatcher}` —
//! neither is re-exported by the `forgekit` facade (only the two *setters*,
//! `set_app_theme`/`clear_app_theme`, are), and `forgekit-shell-common` is not
//! a dependency of this crate. Adding it would mean editing
//! `examples/team-demo/Cargo.toml`, which this task's Files Modified list
//! explicitly excludes (task 08 owns that manifest in the same wave — see
//! `TASKS.md`'s File Overlap Analysis). [`follow_system_brightness_drives_the_real_controller_pipeline_without_panicking`]
//! below is therefore the strongest reachable proof under those constraints:
//! it drives the *real* button → `spawn_local(controller.apply(..))` →
//! `SetTheme::execute` → `set_app_theme`/`clear_app_theme` pipeline end to end
//! (through a real tap, a real pump of the local task queue, and a real
//! subsequent rebuild/paint) and proves it completes and leaves the app
//! stable — without being able to read the override slot's resulting value.
//! A follow-up that either exposes a `#[cfg(test)]`-only settings spy (the
//! `TeamSpy` pattern) or adds `forgekit-shell-common` as a dev-dependency
//! would close this gap; see the completion summary for this task.

use forgekit::{AnyView, Component};
use forgekit_core::{NamedKey, PointerPhase, RenderRoot};
use forgekit_text::TextContext;

use team_demo::{ShellApp, ShellState};

mod support;
use support::{center, char_key, frame, named_key, pointer, pump_frames, setup, tap};

type Root = RenderRoot<ShellState, AnyView<ShellState>>;

/// Mounts the shell under the Material 3 baseline and navigates it to `route`,
/// returning the mounted root/state/logic/text-context plus the first painted
/// scene at that route (a real layout+paint pass, matching `tests/team.rs`'s
/// `frame` helper).
fn mount_at(
    route: &str,
) -> (
    Root,
    ShellState,
    impl FnMut(&mut ShellState) -> AnyView<ShellState> + use<>,
    TextContext,
    support::RecScene,
) {
    forgekit::provide_context(forgekit::Theme::m3_baseline());

    let mut root: Root = RenderRoot::new();
    let mut state = ShellApp.init();
    let mut logic = |s: &mut ShellState| ShellApp.build(s);
    let mut tcx = TextContext::new();

    let _ = frame(&mut root, &mut logic, &mut state, &mut tcx); // resolves "/"
    state.nav.router().go(route);
    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);

    (root, state, logic, tcx, scene)
}

/// (b) SettingsController, real UI: tapping "Follow system brightness" (the
/// screen's last child, so its rounded-rect chrome is always the max-`y` one
/// — a plain `Button`, unlike the `button_group`/`cupertino_button` selector
/// above it, which paint via `fill_path`/`RoundedPolygon` and so never appear
/// in [`support::RecScene::rounded`] at all) must be handled by a real
/// interactive widget and drive the full controller → use-case →
/// `set_app_theme`/`clear_app_theme` pipeline without panicking or corrupting
/// the app across a subsequent navigation. See the [module docs](self)'s
/// `(b)` section for why this can't additionally assert the override's
/// resulting value.
#[test]
fn follow_system_brightness_drives_the_real_controller_pipeline_without_panicking() {
    let _ambient = setup();
    let (mut root, mut state, mut logic, mut tcx, scene) = mount_at("/settings");

    // The Switch's track + thumb (2 rounded rects) plus the trailing "Follow
    // system brightness" Button (1) — the design-language `button_group`
    // paints no rounded rect (see the module docs).
    assert!(
        scene.rounded.len() >= 3,
        "expected the brightness switch (2 rects) plus the follow-system \
         button (1) on /settings, got {}",
        scene.rounded.len()
    );
    let target = scene
        .rounded
        .iter()
        .copied()
        .max_by(|a, b| a.0.y.partial_cmp(&b.0.y).unwrap())
        .expect("at least one rounded rect on /settings");

    let target_center = center(target);
    root.event(&mut state, &pointer(PointerPhase::Down, target_center));
    let outcome = root.event(&mut state, &pointer(PointerPhase::Up, target_center));
    assert!(
        outcome.handled,
        "the tap must land on and be handled by a real interactive widget"
    );

    // Drain the queued `spawn_local(controller.apply(..))` — the real
    // `SetTheme` use case runs here, calling `set_app_theme`/`clear_app_theme`.
    pump_frames(&mut root, &mut logic, &mut state, &mut tcx, 5);

    // Stability: a further rebuild/paint, and a round trip away and back to
    // /settings, must not panic or leave the app unbuildable.
    let _ = frame(&mut root, &mut logic, &mut state, &mut tcx);
    state.nav.router().go("/");
    let _ = frame(&mut root, &mut logic, &mut state, &mut tcx);
    state.nav.router().go("/settings");
    let after = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert!(
        after.glyph_runs > 0,
        "/settings must still paint content after the round trip"
    );
}

/// (d) NotesController, real UI: submitting a blank draft (the field's
/// starting value) is rejected and raises a Dismiss-able validation banner —
/// no id/controller handle needed since the draft starts empty.
#[test]
fn notes_blank_submit_surfaces_a_validation_banner_that_dismiss_clears() {
    let _ambient = setup();
    let (mut root, mut state, mut logic, mut tcx, baseline) = mount_at("/notes");

    // Only the (empty) `TextInput` field paints a rounded rect before any
    // note or banner exists.
    // A `TextInput` paints two rounded rects per field (a border-colored
    // rect plus an inset background fill — see `textinput.rs`'s `paint`), so
    // the lone notes field alone accounts for 2.
    assert_eq!(
        baseline.rounded.len(),
        2,
        "the notes field's border+fill are the only rounded-rect chrome          before any note/banner"
    );
    let field = baseline.rounded[0];

    tap(&mut root, &mut state, center(field));
    root.event(&mut state, &named_key(NamedKey::Enter));
    pump_frames(&mut root, &mut logic, &mut state, &mut tcx, 5);

    let with_banner = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert_eq!(
        with_banner.rounded.len(),
        3,
        "a blank submit adds the banner's Dismiss button (field's 2 + Dismiss's 1)"
    );
    assert!(
        with_banner.glyph_runs > baseline.glyph_runs,
        "the banner message + Dismiss label paint more glyph runs"
    );

    // The banner sits above the field in `notes.rs`'s child order, so it has
    // the smaller `y` (mirrors `tests/team.rs`'s own banner-location trick).
    let dismiss = with_banner
        .rounded
        .iter()
        .copied()
        .min_by(|a, b| a.0.y.partial_cmp(&b.0.y).unwrap())
        .expect("a rounded rect for the Dismiss button");
    tap(&mut root, &mut state, center(dismiss));
    let after = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert_eq!(
        after.rounded.len(),
        2,
        "Dismiss clears the banner, leaving only the field's border+fill"
    );
}

/// (d) NotesController, real UI: typing a real note, submitting it, then
/// deleting it round-trips the rounded-rect/glyph-run counts back to the
/// pre-add baseline.
#[test]
fn notes_add_then_delete_round_trips_through_the_real_controller() {
    let _ambient = setup();
    let (mut root, mut state, mut logic, mut tcx, baseline) = mount_at("/notes");
    assert_eq!(
        baseline.rounded.len(),
        2,
        "just the field's border+fill, no notes yet"
    );

    let field = baseline.rounded[0];
    tap(&mut root, &mut state, center(field));
    for c in "Buy milk".chars() {
        root.event(&mut state, &char_key(&c.to_string()));
    }
    root.event(&mut state, &named_key(NamedKey::Enter));
    pump_frames(&mut root, &mut logic, &mut state, &mut tcx, 5);

    let with_note = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert_eq!(
        with_note.rounded.len(),
        3,
        "the field's border+fill plus the new row's Delete button"
    );
    assert!(with_note.glyph_runs > baseline.glyph_runs);

    // The submitted row (and its Delete button) is appended after the field,
    // so it has the larger `y`.
    let delete = with_note
        .rounded
        .iter()
        .copied()
        .max_by(|a, b| a.0.y.partial_cmp(&b.0.y).unwrap())
        .expect("a rounded rect for the Delete button");
    tap(&mut root, &mut state, center(delete));
    pump_frames(&mut root, &mut logic, &mut state, &mut tcx, 5);

    let after = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert_eq!(
        after.rounded.len(),
        2,
        "deleting the only note returns to just the field's border+fill"
    );
}

/// (c) ProfileController, real UI (success leg): saving the screen's
/// as-seeded (already-valid) profile must not raise a validation banner. The
/// failure leg (blank name / malformed email → banner) is covered at the
/// domain-unit level in `src/profile_domain.rs`'s own `#[cfg(test)]` module
/// (added in task 06) — reaching it here would need reliably clearing a
/// pre-filled `TextInput` via `Backspace` key events, which this suite avoids
/// given the field's initial-cursor-position assumption that would rest on
/// (see this file's module docs for the analogous `(b)` gap this crate's
/// module-privacy boundary creates).
#[test]
fn profile_save_with_the_seeded_valid_profile_raises_no_banner() {
    let _ambient = setup();
    let (mut root, mut state, mut logic, mut tcx, baseline) = mount_at("/profile");

    // Three `TextInput` fields (name/role/email), each 2 rounded rects
    // (border+fill — see `textinput.rs`'s `paint`), plus the Save button (1),
    // no banner yet.
    assert_eq!(
        baseline.rounded.len(),
        7,
        "three fields' border+fill (6) plus Save (1), no banner before any          save attempt"
    );

    let save = baseline
        .rounded
        .iter()
        .copied()
        .max_by(|a, b| a.0.y.partial_cmp(&b.0.y).unwrap())
        .expect("a rounded rect for the Save button");
    tap(&mut root, &mut state, center(save));
    pump_frames(&mut root, &mut logic, &mut state, &mut tcx, 5);

    let after = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert_eq!(
        after.rounded.len(),
        7,
        "a successful save raises no Dismiss-able banner"
    );
}
