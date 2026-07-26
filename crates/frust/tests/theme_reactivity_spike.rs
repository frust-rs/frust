//! THROWAWAY — native-widgets Phase 0 spike 4a probe (not for merge).
//!
//! Question (PLAN.md Phase 0.4a): does a `use_context::<Theme>()`-derived
//! builder param rebuild on `set_app_theme`? The L2 live re-theme story for
//! native widgets depends on it: the plugin's `api` layer reads the active
//! `Theme` during rebuild and applies concrete tokens via direct FFI setters,
//! so a forced theme change must (a) reach the context slot before the next
//! rebuild and (b) actually re-run the component's `build`.
//!
//! This probe emulates the exact shell frame flow (the same sequence
//! `frust-shell-android`'s frame callback and the desktop `RedrawRequested`
//! arm run): `frust::set_app_theme` → `ThemeOverrideWatcher::poll` →
//! `provide_context` re-provide under the root owner → component rebuild.
//! The component's `Owner` is a CHILD of the root owner created BEFORE the
//! re-provide — the risky half: context resolution must walk to the root
//! owner's replaced value at read time, not a captured snapshot.

use std::sync::{Arc, Mutex};

use frust::{Brightness, Theme, provide_context, use_context};
use frust_core::component::{Component, component};
use frust_core::layout::BoxConstraints;
use frust_core::view::{AnyView, BuildCtx, ChangeFlags, View, any};
use frust_core::widget::{LayoutCtx, PaintCtx, PaintScene, Widget};
use frust_reactive::Owner;
use frust_shell_common::ThemeOverrideWatcher;
use kurbo::Size;

/// Trivial leaf to fill the component subtree (mirrors frust-core's own
/// component test fixtures).
struct Empty;
struct EmptyWidget;
impl Widget for EmptyWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(10.0, 10.0))
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
}
impl View<()> for Empty {
    type Element = EmptyWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> EmptyWidget {
        EmptyWidget
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut EmptyWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

/// The probe: a component whose build derives a param from
/// `use_context::<Theme>()` — exactly what a native-widget builder will do —
/// and records every value it observed.
struct ThemeProbe {
    seen: Arc<Mutex<Vec<Option<Brightness>>>>,
}
impl Component for ThemeProbe {
    type State = ();
    fn init(&self) -> Self::State {}
    fn build(&self, _state: &mut Self::State) -> AnyView<Self::State> {
        let brightness = use_context::<Theme>().map(|t| t.brightness);
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(brightness);
        any(Empty)
    }
}

#[test]
fn use_context_theme_param_rebuilds_on_set_app_theme() {
    // The shell's root owner (never disposed for the process lifetime).
    let root = Owner::new();

    // Shell seed: platform-derived light theme, provided before frame 1 —
    // the same call `apply_theme`/`push_theme` makes on every shell.
    let mut seeded = Theme::glyph_baseline();
    seeded.brightness = Brightness::Light;
    root.with(|| provide_context(seeded));

    let seen = Arc::new(Mutex::new(Vec::new()));
    let view = component(ThemeProbe { seen: seen.clone() });

    // Frame 1: build under the root owner (the component's own Owner becomes
    // a child of it here).
    let mut next_id = 0u64;
    let mut element = root.with(|| {
        let mut ctx = BuildCtx::new(&mut next_id);
        View::<()>::build(&view, &mut ctx)
    });

    // App code forces a theme mid-session (callable from any thread).
    let mut forced = Theme::glyph_baseline();
    forced.brightness = Brightness::Dark;
    frust::set_app_theme(forced);

    // Frame 2, shell side: poll the override slot (every shell does this at
    // the top of its frame; on mobile a delivery also sets the frame gate's
    // `theme_or_appearance_changed` input so the frame is never skipped),
    // then re-provide under the root owner, then rebuild.
    let mut watcher = ThemeOverrideWatcher::new();
    let delivered = watcher
        .poll()
        .expect("set_app_theme must be observable on the next poll")
        .expect("a set (not a clear) must deliver the forced theme");
    assert_eq!(delivered.brightness, Brightness::Dark);
    root.with(|| provide_context(delivered));
    root.with(|| {
        let mut ctx = BuildCtx::new(&mut next_id);
        View::<()>::rebuild(&view, &view, &mut element, &mut ctx)
    });

    // The builder param derived from `use_context::<Theme>()` observed the
    // seeded value on frame 1 and the forced value on frame 2.
    let seen = seen.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(
        *seen,
        vec![Some(Brightness::Light), Some(Brightness::Dark)],
        "a use_context::<Theme>()-derived builder param must observe \
         set_app_theme's forced theme on the very next rebuild"
    );
}

/// The fallback question the runbook poses (explicit theme-generation signal)
/// is only needed if the mechanism above fails — this companion pins the
/// inverse property that makes the mechanism sufficient: a rebuild ALWAYS
/// re-runs `Component::build` (no memoization), so context freshness is the
/// only load-bearing half.
#[test]
fn component_build_reruns_on_every_rebuild_without_state_change() {
    let root = Owner::new();
    root.with(|| provide_context(Theme::glyph_baseline()));

    let seen = Arc::new(Mutex::new(Vec::new()));
    let view = component(ThemeProbe { seen: seen.clone() });

    let mut next_id = 0u64;
    let mut element = root.with(|| {
        let mut ctx = BuildCtx::new(&mut next_id);
        View::<()>::build(&view, &mut ctx)
    });
    for _ in 0..3 {
        root.with(|| {
            let mut ctx = BuildCtx::new(&mut next_id);
            View::<()>::rebuild(&view, &view, &mut element, &mut ctx)
        });
    }

    let count = seen.lock().unwrap_or_else(|e| e.into_inner()).len();
    assert_eq!(
        count, 4,
        "build must run on the initial build + all 3 rebuilds"
    );
}
