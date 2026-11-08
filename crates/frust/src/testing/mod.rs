//! Headless drivers for app and widget tests.
//!
//! [`Tester`] owns a retained root over the app's own state type, a text context, a manual clock
//! and a child reactive owner, and steps frames in the shells' order: drain the local task queue,
//! rebuild under the owner, lay out, paint. Flutter's `WidgetTester` maps onto it like this:
//!
//! | Flutter | Frust |
//! |---------|-------|
//! | `pumpWidget` | [`Tester::new`] / [`Tester::new_with`] (runs the first frame) |
//! | `pump(duration)` | [`Tester::pump`] |
//! | `pumpAndSettle` | [`Tester::pump_and_settle`] |
//! | `runAsync` + polling | [`Tester::pump_until`] (the one real-time wait) |
//! | `binding.setSurfaceSize` | [`Tester::resize`] / [`Tester::set_metrics`] |
//! | `tap` / `press` / `longPress` | [`Tester::tap`] / [`Tester::press`] / [`Tester::long_press`] |
//! | `drag` / `timedDrag` | [`Tester::drag`] / [`Tester::timed_drag`] |
//! | `startGesture` / `createGesture` | [`Tester::start_gesture`] / [`Tester::start_touch`] |
//! | `TestGesture.moveBy` / `moveTo` | [`Tester::move_by`] / [`Tester::move_to`] |
//! | `TestGesture.up` / `cancel` | [`Tester::up`] / [`Tester::cancel`] |
//! | `find.text` / `find.byType` | [`find::text`] / [`find::by_type`] |
//! | `getSemantics` | [`Tester::get_semantics`] |
//!
//! Unlike Flutter's, [`Tester::tap`] does not pump: the change it makes shows from the next pump.
//! A second touch contact reaches only a widget that opted into the gesture's other contacts;
//! see [`gesture`].
//!
//! Time enters only through paint: there is no fake-async zone and no timer queue. A widget reads
//! the clock from `PaintCtx::frame_time`, and a pump advances it by exactly the duration given.
//!
//! The one-shot helpers suit a single read of a stateless view: [`semantics_of`] returns the
//! accessibility tree after one layout, and [`scene_of`] / [`scene_of_with`] return one recorded
//! [`Scene`]. They need no reactive runtime. The feature also makes
//! [`PaintCtx::for_test`](crate::authoring::PaintCtx::for_test) available for driving a widget's
//! `paint` directly against a chosen clock.

pub mod find;
pub mod gesture;
mod tester;

pub use find::{ActionTarget, Finder, Match};
pub use gesture::{Gesture, Target};
pub use tester::{PumpOutcome, Tester, TesterOptions};

/// What [`Tester::inspect`] returns: one retained-tree node with its absolute bounds.
pub use frust_core::InspectNode;
/// Run a closure with the animation time scale overridden on this thread only.
pub use frust_core::anim::with_thread_animation_time_scale;
/// The reactive owner type, for a test that sets an ambient owner before constructing a
/// [`Tester`] (`let owner = Owner::new(); owner.set();`).
pub use frust_reactive::Owner;

use kurbo::Size;

use crate::{Theme, View};
use frust_core::{FrameTime, PaintScene, SemanticsUpdate};
use frust_scene::{Scene, SceneBuilder};

/// Build the view `build` returns, lay it out in a `size` window and return its semantics tree.
///
/// The tree always holds the window root node; a widget contributes its own nodes through
/// `Widget::semantics`. The layout pass threads a real text context, so text-measuring widgets
/// lay out as they do in an app.
pub fn semantics_of<V, F>(mut build: F, size: Size) -> SemanticsUpdate
where
    F: FnMut(&mut ()) -> V,
    V: View<()>,
{
    let mut root: frust_core::RenderRoot<(), V> = frust_core::RenderRoot::new();
    root.rebuild(&mut build, &mut ());
    let mut text_ctx = frust_text::TextContext::new();
    root.layout_with_text(size, &mut text_ctx as &mut dyn std::any::Any);
    root.semantics()
}

/// How [`scene_of_with`] builds and paints a view.
///
/// The default is no theme, no warm frames and a final paint at [`FrameTime::ZERO`], which is
/// what [`scene_of`] does.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct SceneOptions {
    theme: Option<Theme>,
    frame_time: FrameTime,
    warm_frames: u32,
}

impl SceneOptions {
    /// Install `theme` on the root before the first layout, so themed widgets resolve it.
    #[must_use]
    pub fn theme(mut self, theme: Theme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// The clock of the final, recorded paint.
    #[must_use]
    pub fn frame_time(mut self, frame_time: FrameTime) -> Self {
        self.frame_time = frame_time;
        self
    }

    /// Run `warm_frames` full rebuild, layout and paint passes before the recorded one.
    ///
    /// Each pass paints into a discarded scene at evenly spaced times from zero up to, but
    /// never reaching, the final [`frame_time`](Self::frame_time), so animations the view
    /// stages are underway when the recorded paint happens.
    #[must_use]
    pub fn warm_frames(mut self, warm_frames: u32) -> Self {
        self.warm_frames = warm_frames;
        self
    }
}

/// Build the view `build` returns, lay it out in a `size` window with no theme and record one
/// paint at [`FrameTime::ZERO`].
///
/// The returned scene holds every command the view painted, so a test can count or inspect
/// them. See [`scene_of_with`] for a theme, a clock and warm frames, and for what a recorded
/// backdrop count means.
pub fn scene_of<V, F>(build: F, size: Size) -> Scene
where
    F: FnMut(&mut ()) -> V,
    V: View<()>,
{
    scene_of_with(build, size, &SceneOptions::default())
}

/// Build the view `build` returns, lay it out in a `size` window and record one paint under
/// `options`.
///
/// Any theme is installed before the first layout. Each warm pass rebuilds, lays out and
/// paints into a discarded scene, then the final paint is recorded at the options' frame time.
///
/// # Counting backdrops
///
/// The scene records every backdrop blur the view paints, including ones far off screen. The
/// engine counts toward `max_backdrops_per_frame()` only the backdrops whose blur region
/// reaches the frame, so a recorded `BackdropBlur` count is an upper bound on what counts
/// toward that cap, and exact for a view that lies entirely on screen. An opaque glass
/// material records no backdrop at all.
pub fn scene_of_with<V, F>(mut build: F, size: Size, options: &SceneOptions) -> Scene
where
    F: FnMut(&mut ()) -> V,
    V: View<()>,
{
    let mut root: frust_core::RenderRoot<(), V> = frust_core::RenderRoot::new();
    if let Some(theme) = &options.theme {
        root.set_theme(Box::new(theme.clone()));
    }
    let mut text_ctx = frust_text::TextContext::new();
    let mut pass = |root: &mut frust_core::RenderRoot<(), V>, scene: &mut dyn PaintScene, at| {
        root.rebuild(&mut build, &mut ());
        root.layout_with_text(size, &mut text_ctx as &mut dyn std::any::Any);
        let _outcome = root.paint(scene, at);
    };
    let count = u64::from(options.warm_frames);
    for index in 0..count {
        let at = FrameTime::from_nanos(options.frame_time.as_nanos().saturating_mul(index) / count);
        pass(&mut root, &mut crate::authoring::DiscardScene, at);
    }
    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        pass(&mut root, &mut builder, options.frame_time);
    }
    scene
}

#[cfg(test)]
mod tests {
    use kurbo::Size;

    #[test]
    fn semantics_of_a_text_view_finds_the_text_node() {
        let update = crate::testing::semantics_of(
            |_: &mut ()| crate::text("hello"),
            Size::new(200.0, 100.0),
        );
        assert!(update.nodes.len() >= 2, "window root plus the text node");
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, node)| node.value() == Some("hello") || node.label() == Some("hello"))
        );
    }

    fn backdrops(scene: &frust_scene::Scene) -> usize {
        scene
            .commands()
            .iter()
            .filter(|c| matches!(c, frust_scene::Command::BackdropBlur { .. }))
            .count()
    }

    fn translucent_theme() -> crate::Theme {
        let lens = crate::GlassMaterial {
            blur_radius_intent: 20.0,
            fills_light: vec![crate::GlassFill::new(1.0, 1.0, 1.0, 0.3)],
            fills_dark: vec![crate::GlassFill::new(0.0, 0.0, 0.0, 0.3)],
            hairline_alpha: 0.5,
            shadow: crate::ShadowSpec {
                y_offset: 4.0,
                blur_std_dev: 8.0,
                color_alpha: 0.2,
            },
        };
        crate::Theme::builder(crate::Theme::neutral())
            .glass(crate::GlassScale {
                chrome: lens.clone(),
                bar: lens.clone(),
                control: lens,
            })
            .build()
    }

    #[test]
    fn scene_of_records_a_command_for_a_coloured_box() {
        let scene = crate::testing::scene_of(
            |_: &mut ()| {
                crate::colored_box()
                    .fill(crate::Color::from_rgb8(200, 10, 10))
                    .size(40.0, 40.0)
            },
            Size::new(200.0, 100.0),
        );
        assert!(!scene.commands().is_empty());
        assert_eq!(backdrops(&scene), 0);
    }

    #[test]
    fn scene_of_with_counts_one_backdrop_for_translucent_glass_and_none_for_opaque() {
        let size = Size::new(200.0, 100.0);
        let view = |_: &mut ()| crate::glass_surface(crate::colored_box().size(40.0, 40.0));
        let translucent = crate::testing::SceneOptions::default().theme(translucent_theme());
        let scene = crate::testing::scene_of_with(view, size, &translucent);
        assert_eq!(backdrops(&scene), 1);

        let opaque = crate::testing::SceneOptions::default().theme(crate::Theme::neutral());
        let scene = crate::testing::scene_of_with(view, size, &opaque);
        assert_eq!(backdrops(&scene), 0);
    }

    #[test]
    fn warm_frames_advance_an_animation_before_the_recorded_paint() {
        let at = |warm: u32| {
            let mut pass = 0u32;
            let options = crate::testing::SceneOptions::default()
                .frame_time(frust_core::FrameTime::from_nanos(100_000_000))
                .warm_frames(warm);
            let scene = crate::testing::scene_of_with(
                move |_: &mut ()| {
                    pass += 1;
                    let target = if pass == 1 { 0.0 } else { 1.0 };
                    crate::motion::AnimatedOpacity(
                        target,
                        crate::colored_box()
                            .fill(crate::Color::from_rgb8(10, 10, 200))
                            .size(40.0, 40.0),
                    )
                },
                Size::new(200.0, 100.0),
                &options,
            );
            format!("{:?}", scene.commands())
        };
        assert_ne!(
            at(0),
            at(4),
            "warm passes move the fade off its first value"
        );
    }

    #[test]
    fn paint_ctx_for_test_resolves_through_the_facade() {
        let ctx = crate::authoring::PaintCtx::for_test(
            kurbo::Point::ZERO,
            Size::new(10.0, 10.0),
            frust_core::FrameTime::from_nanos(5),
        );
        assert_eq!(ctx.frame_time(), frust_core::FrameTime::from_nanos(5));
    }
}
