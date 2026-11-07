//! [`Tester`]: a retained root driven frame by frame over an app's own state type, under its own
//! reactive owner and a manual clock. See the [module docs](super) for the Flutter mapping.

use std::any::Any;
use std::sync::Arc;
use std::time::{Duration, Instant};

use frust_core::accesskit::{Action, NodeId};
use frust_core::{
    DiscardScene, EventOutcome, FrameTime, InputEvent, InspectNode, PaintOutcome, PaintScene,
    RenderRoot, SemanticsUpdate, View, WindowInsets, WindowMetrics,
};
use frust_reactive::{Owner, ReactiveRuntime, provide_context};
use frust_scene::{Scene, SceneBuilder};
use kurbo::Size;

use crate::Theme;

/// The virtual frame [`Tester::pump_and_settle`] and [`Tester::pump_until`] advance per pump.
const FRAME: Duration = Duration::from_millis(16);
/// Pumps [`Tester::pump_and_settle`] runs before declaring the tree unsettled (9.6 s virtual).
const SETTLE_FRAME_CAP: usize = 600;
/// Real time [`Tester::pump_until`] sleeps between pumps, so background threads make progress.
const UNTIL_POLL: Duration = Duration::from_millis(20);

/// How [`Tester::with_options`] and [`Tester::new_with`] set up the window and the clock.
///
/// The default is no theme, scale `1.0`, no insets and a clock starting at [`FrameTime::ZERO`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct TesterOptions {
    theme: Option<Theme>,
    scale: f64,
    insets: WindowInsets,
    start_time: FrameTime,
}

impl Default for TesterOptions {
    fn default() -> Self {
        Self {
            theme: None,
            scale: 1.0,
            insets: WindowInsets::default(),
            start_time: FrameTime::ZERO,
        }
    }
}

impl TesterOptions {
    /// Install `theme` on the root and provide it as a `Theme` context before the first frame.
    #[must_use]
    pub fn theme(mut self, theme: Theme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// The device-pixel scale the provided [`WindowMetrics`] carries.
    #[must_use]
    pub fn scale(mut self, scale: f64) -> Self {
        self.scale = scale;
        self
    }

    /// The window insets, set on the root and carried by the provided [`WindowMetrics`].
    #[must_use]
    pub fn insets(mut self, insets: WindowInsets) -> Self {
        self.insets = insets;
        self
    }

    /// The clock of the first frame; every pump advances it from here.
    #[must_use]
    pub fn start_time(mut self, start_time: FrameTime) -> Self {
        self.start_time = start_time;
        self
    }
}

/// What one [`Tester::pump`] reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PumpOutcome {
    /// The tree asked for another frame, or a deferred callback still awaits its flush.
    pub needs_frame: bool,
}

/// A headless app driver: a retained root over state `S`, a text context, a manual clock and a
/// child reactive owner, stepped one frame per [`pump`](Self::pump) in the shells' order.
///
/// Every rebuild, event and `provide_context` runs inside the Tester's own owner, a child of the
/// test's ambient owner when one is set and of the runtime root otherwise, so two Testers never
/// share contexts and dropping one disposes the signals and contexts it created. Construct and
/// pump a Tester on one thread: the constructing thread becomes that thread's UI thread, whose
/// local task queue each pump drains. Tests on separate threads each get their own.
pub struct Tester<S: 'static, V: View<S>> {
    root: RenderRoot<S, V>,
    state: S,
    build: Box<dyn FnMut(&mut S) -> V>,
    text_ctx: frust_text::TextContext,
    metrics: WindowMetrics,
    clock: FrameTime,
    runtime: &'static ReactiveRuntime,
    owner: Owner,
}

impl<S: 'static, V: View<S>> Tester<S, V> {
    /// Build `state` with `build` in a `size` window under default options and run one frame,
    /// so layout, overlay hits, [`semantics`](Self::semantics) and [`inspect`](Self::inspect)
    /// are valid on return.
    pub fn new<F>(state: S, build: F, size: Size) -> Self
    where
        F: FnMut(&mut S) -> V + 'static,
    {
        Self::new_with(size, &TesterOptions::default(), move || (state, build))
    }

    /// [`new`](Self::new) under `options`.
    pub fn with_options<F>(state: S, build: F, size: Size, options: &TesterOptions) -> Self
    where
        F: FnMut(&mut S) -> V + 'static,
    {
        Self::new_with(size, options, move || (state, build))
    }

    /// Run `setup` inside the Tester's owner, then build its state with its build closure and
    /// run one frame.
    ///
    /// `setup` runs after the [`WindowMetrics`] (and any theme) context is provided and before
    /// the first rebuild, so contexts it provides (repositories, controllers, a router) resolve
    /// from `use_context` inside the build closure and event handlers. This is the recommended
    /// form for an app with injected dependencies; contexts provided on an ambient owner set
    /// before construction also resolve, through the parent walk.
    pub fn new_with<F>(size: Size, options: &TesterOptions, setup: impl FnOnce() -> (S, F)) -> Self
    where
        F: FnMut(&mut S) -> V + 'static,
    {
        let runtime = ReactiveRuntime::init(Arc::new(|| {}));
        let owner = match Owner::current() {
            Some(ambient) => ambient.child(),
            None => runtime.with_owner(Owner::new),
        };
        let metrics = WindowMetrics::new(size, options.scale, options.insets);
        let mut root = RenderRoot::new();
        root.set_insets(options.insets);
        if let Some(theme) = &options.theme {
            root.set_theme(Box::new(theme.clone()));
        }
        let (state, build) = owner.with(|| {
            provide_context(metrics);
            if let Some(theme) = &options.theme {
                provide_context(theme.clone());
            }
            setup()
        });
        let mut tester = Self {
            root,
            state,
            build: Box::new(build),
            text_ctx: frust_text::TextContext::new(),
            metrics,
            clock: options.start_time,
            runtime,
            owner,
        };
        let _outcome = tester.frame(&mut DiscardScene);
        tester
    }

    /// Advance the clock by `dt`, drain the local task queue, then rebuild, lay out and paint
    /// one frame at the new clock.
    ///
    /// `needs_frame` folds the paint's request with a still-pending deferred-callback flush.
    pub fn pump(&mut self, dt: Duration) -> PumpOutcome {
        let dt = u64::try_from(dt.as_nanos()).unwrap_or(u64::MAX);
        self.clock = FrameTime::from_nanos(self.clock.as_nanos().saturating_add(dt));
        self.runtime.pump_local();
        let outcome = self.frame(&mut DiscardScene);
        PumpOutcome {
            needs_frame: outcome.needs_frame || frust_core::has_pending_result_flush(),
        }
    }

    /// Pump 16 ms frames until two consecutive pumps request no frame, and return the count.
    ///
    /// Two, because a deferred callback delivered by the rebuild's housekeeping flush asks for
    /// its redraw on the frame after, so one such fire costs two pumps to settle.
    ///
    /// # Panics
    ///
    /// When 600 frames (9.6 s of virtual time) pass and the tree still requests frames.
    pub fn pump_and_settle(&mut self) -> usize {
        let mut idle_streak = 0;
        for frames in 1..=SETTLE_FRAME_CAP {
            if self.pump(FRAME).needs_frame {
                idle_streak = 0;
            } else {
                idle_streak += 1;
                if idle_streak == 2 {
                    return frames;
                }
            }
        }
        panic!("pump_and_settle: 600 frames (9.6 s) and the tree still requests frames");
    }

    /// Pump 16 ms virtual frames, sleeping 20 ms of real time between them, until `predicate`
    /// holds; return the number of pumps it took.
    ///
    /// Work handed to `spawn_blocking` or another background thread reaches the local queue only
    /// once a real thread finishes, so a virtual-clock [`pump_and_settle`](Self::pump_and_settle)
    /// never waits for it. This is the one place real time enters a Tester. `what` names the
    /// awaited condition in the failure message.
    ///
    /// # Panics
    ///
    /// When `timeout` of real time passes with `predicate` still false.
    pub fn pump_until(
        &mut self,
        what: &str,
        timeout: Duration,
        mut predicate: impl FnMut(&mut Self) -> bool,
    ) -> usize {
        let deadline = Instant::now() + timeout;
        let mut pumps = 0;
        loop {
            if predicate(self) {
                return pumps;
            }
            if Instant::now() >= deadline {
                panic!("pump_until: {what} did not hold within {timeout:?}");
            }
            self.pump(FRAME);
            pumps += 1;
            std::thread::sleep(UNTIL_POLL);
        }
    }

    /// Re-provide `metrics` as the [`WindowMetrics`] context and lay out at its size from the
    /// next pump on, as a shell does on a rotation or scale change.
    pub fn set_metrics(&mut self, metrics: WindowMetrics) {
        self.metrics = metrics;
        self.root.set_insets(metrics.insets);
        self.owner.with(|| provide_context(metrics));
    }

    /// [`set_metrics`](Self::set_metrics) with a new size and the current scale and insets.
    pub fn resize(&mut self, size: Size) {
        let metrics = WindowMetrics::new(size, self.metrics.scale, self.metrics.insets);
        self.set_metrics(metrics);
    }

    /// The window metrics the Tester last provided.
    pub fn metrics(&self) -> WindowMetrics {
        self.metrics
    }

    /// The clock of the last frame.
    pub fn frame_time(&self) -> FrameTime {
        self.clock
    }

    /// The layout size.
    pub fn size(&self) -> Size {
        self.metrics.size
    }

    /// The app state the build closure reads.
    pub fn state(&self) -> &S {
        &self.state
    }

    /// The app state, mutably; the change shows from the next pump.
    pub fn state_mut(&mut self) -> &mut S {
        &mut self.state
    }

    /// Rebuild, lay out and record one paint at the current clock, without advancing it.
    pub fn scene(&mut self) -> Scene {
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            let _outcome = self.frame(&mut builder);
        }
        scene
    }

    /// The accessibility tree as of the last layout.
    pub fn semantics(&self) -> SemanticsUpdate {
        self.root.semantics()
    }

    /// A pre-order snapshot of the retained tree with each node's absolute bounds.
    pub fn inspect(&self) -> Vec<InspectNode> {
        self.root.inspect()
    }

    /// Deliver `event` to the tree under the Tester's owner.
    ///
    /// Events carry no clock: pump between events whose meaning depends on elapsed time (a long
    /// press, a fling). The change an event makes shows from the next pump.
    pub fn send(&mut self, event: &InputEvent) -> EventOutcome {
        let Self {
            root, state, owner, ..
        } = self;
        owner.with(|| root.event(state, event))
    }

    /// Perform an accessibility `action` on semantics node `node` under the Tester's owner.
    pub fn perform_action(&mut self, node: NodeId, action: Action) -> EventOutcome {
        let Self {
            root, state, owner, ..
        } = self;
        owner.with(|| root.perform_accessibility_action(state, node, action))
    }

    /// One frame at the current clock: rebuild under the owner, lay out, paint into `scene`.
    fn frame(&mut self, scene: &mut dyn PaintScene) -> PaintOutcome {
        let Self {
            root,
            state,
            build,
            text_ctx,
            metrics,
            clock,
            owner,
            ..
        } = self;
        owner.with(|| {
            root.rebuild(build, state);
        });
        let _flags = root.take_change_flags();
        root.layout_with_text(metrics.size, text_ctx as &mut dyn Any);
        root.paint(scene, *clock)
    }
}

impl<S: 'static, V: View<S>> Drop for Tester<S, V> {
    fn drop(&mut self) {
        // Tear the tree down under the owner first, so widget cleanups still resolve contexts.
        let root = std::mem::replace(&mut self.root, RenderRoot::new());
        self.owner.with(move || drop(root));
        self.owner.cleanup();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    use frust_core::{
        BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, SemanticsUpdate,
        View, Widget, WindowInsets, WindowMetrics,
    };
    use kurbo::Size;

    use super::{FRAME, Tester, TesterOptions};
    use crate::{
        Get, GetUntracked, Orientation, RwSignal, Set, Theme, provide_context, use_context,
    };

    const SIZE: Size = Size::new(200.0, 100.0);

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Marker(u32);

    #[derive(Default)]
    struct Seen {
        metrics: Option<WindowMetrics>,
        marker: Option<Marker>,
        theme: bool,
    }

    fn probe(seen: &mut Seen) -> crate::TextView {
        seen.metrics = use_context::<WindowMetrics>();
        seen.marker = use_context::<Marker>();
        seen.theme = use_context::<Theme>().is_some();
        crate::text("probe")
    }

    fn shows(update: &SemanticsUpdate, text: &str) -> bool {
        update
            .nodes
            .iter()
            .any(|(_, node)| node.value() == Some(text) || node.label() == Some(text))
    }

    /// A leaf that requests a frame from each paint while its countdown is above zero, and
    /// records at layout whether the root carries a theme.
    struct Probe {
        frames_left: Rc<Cell<u32>>,
        themed: Rc<Cell<bool>>,
    }

    impl Probe {
        fn frames(frames_left: u32) -> Self {
            Self {
                frames_left: Rc::new(Cell::new(frames_left)),
                themed: Rc::new(Cell::new(false)),
            }
        }

        fn share(&self) -> Self {
            Self {
                frames_left: Rc::clone(&self.frames_left),
                themed: Rc::clone(&self.themed),
            }
        }
    }

    impl<S: 'static> View<S> for Probe {
        type Element = Probe;

        fn build(&self, _ctx: &mut BuildCtx<'_>) -> Probe {
            self.share()
        }

        fn rebuild(&self, _prev: &Self, _el: &mut Probe, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for Probe {
        fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            self.themed.set(ctx.theme_as::<Theme>().is_some());
            bc.constrain(Size::new(10.0, 10.0))
        }

        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            let left = self.frames_left.get();
            if left > 0 {
                self.frames_left.set(left - 1);
                ctx.request_frame();
            }
        }
    }

    #[test]
    fn testers_on_two_threads_each_read_their_own_window_size() {
        let sizes = [Size::new(300.0, 500.0), Size::new(700.0, 200.0)];
        let barrier = Arc::new(Barrier::new(sizes.len()));
        let handles: Vec<_> = sizes
            .into_iter()
            .map(|size| {
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let mut tester = Tester::new(Seen::default(), probe, size);
                    barrier.wait();
                    for _ in 0..5 {
                        tester.pump(FRAME);
                        std::thread::yield_now();
                    }
                    tester.state().metrics.map(|metrics| metrics.size)
                })
            })
            .collect();
        for (handle, size) in handles.into_iter().zip(sizes) {
            assert_eq!(handle.join().expect("tester thread"), Some(size));
        }
    }

    #[test]
    fn two_testers_in_one_test_read_their_own_metrics() {
        let mut first = Tester::new(Seen::default(), probe, Size::new(100.0, 100.0));
        let options = TesterOptions::default().scale(2.0);
        let mut second =
            Tester::with_options(Seen::default(), probe, Size::new(200.0, 50.0), &options);
        first.pump(Duration::ZERO);
        second.pump(Duration::ZERO);
        let first_seen = first.state().metrics.expect("first metrics");
        let second_seen = second.state().metrics.expect("second metrics");
        assert_eq!(
            (first_seen.size, first_seen.scale),
            (Size::new(100.0, 100.0), 1.0)
        );
        assert_eq!(
            (second_seen.size, second_seen.scale),
            (Size::new(200.0, 50.0), 2.0)
        );
    }

    struct Counter {
        n: u32,
    }

    fn counter_view(counter: &mut Counter) -> crate::TextView {
        crate::text(format!("count {}", counter.n))
    }

    #[test]
    fn state_mutations_thread_through_the_next_rebuild() {
        let mut tester = Tester::new(Counter { n: 0 }, counter_view, SIZE);
        assert!(shows(&tester.semantics(), "count 0"));
        tester.state_mut().n += 1;
        tester.pump(Duration::ZERO);
        assert!(shows(&tester.semantics(), "count 1"));
    }

    #[test]
    fn metrics_scale_insets_and_theme_reach_the_build_closure_and_layout() {
        let insets = WindowInsets::default();
        let options = TesterOptions::default()
            .theme(Theme::neutral())
            .scale(3.0)
            .insets(insets);
        let themed = Probe::frames(0);
        let leaf = themed.share();
        let tester = Tester::with_options(
            Seen::default(),
            move |seen: &mut Seen| {
                let _ = probe(seen);
                leaf.share()
            },
            SIZE,
            &options,
        );
        let seen = tester.state();
        assert_eq!(seen.metrics, Some(WindowMetrics::new(SIZE, 3.0, insets)));
        assert!(seen.theme, "use_context::<Theme>() resolves inside build");
        assert!(themed.themed.get(), "the root theme reaches layout");

        let plain = Tester::new(Seen::default(), probe, SIZE);
        assert!(!plain.state().theme);
    }

    struct Signalled {
        signal: Option<RwSignal<u32>>,
    }

    #[test]
    fn a_signal_created_in_build_and_set_from_state_shows_after_one_pump() {
        let mut tester = Tester::new(
            Signalled { signal: None },
            |state: &mut Signalled| {
                let signal = *state.signal.get_or_insert_with(|| RwSignal::new(0));
                crate::text(format!("signal {}", signal.get()))
            },
            SIZE,
        );
        let signal = tester
            .state_mut()
            .signal
            .expect("created by the first build");
        signal.set(5);
        tester.pump(Duration::ZERO);
        assert!(shows(&tester.semantics(), "signal 5"));

        crate::spawn_local(async move { signal.set(9) });
        tester.pump(Duration::ZERO);
        assert!(
            shows(&tester.semantics(), "signal 9"),
            "pump drains the local queue"
        );
    }

    #[test]
    fn pump_and_settle_runs_until_the_tree_stops_requesting_frames() {
        let frames = 5;
        let probe = Probe::frames(frames);
        let leaf = probe.share();
        let mut tester = Tester::new((), move |_: &mut ()| leaf.share(), SIZE);
        let pumped = tester.pump_and_settle();
        assert!(pumped >= frames as usize, "settled after {pumped} pumps");
        assert_eq!(probe.frames_left.get(), 0);
        assert!(!tester.pump(FRAME).needs_frame);
        assert_eq!(
            tester.frame_time().as_nanos(),
            16_000_000 * (pumped as u64 + 1)
        );
    }

    #[test]
    #[should_panic(
        expected = "pump_and_settle: 600 frames (9.6 s) and the tree still requests frames"
    )]
    fn pump_and_settle_panics_on_a_tree_that_never_settles() {
        let leaf = Probe::frames(u32::MAX);
        let mut tester = Tester::new((), move |_: &mut ()| leaf.share(), SIZE);
        tester.pump_and_settle();
    }

    fn red_box<S: 'static>() -> impl View<S> {
        crate::colored_box()
            .fill(crate::Color::from_rgb8(200, 10, 10))
            .size(40.0, 40.0)
    }

    #[test]
    fn scene_matches_scene_of_and_inspect_has_bounds_after_new() {
        let expected = crate::testing::scene_of(|_: &mut ()| red_box(), SIZE);
        let mut tester = Tester::new((), |_: &mut ()| red_box(), SIZE);
        assert!(
            tester
                .inspect()
                .iter()
                .any(|node| node.bounds.size() == Size::new(40.0, 40.0))
        );
        let time = tester.frame_time();
        let scene = tester.scene();
        assert_eq!(
            tester.frame_time(),
            time,
            "scene() does not advance the clock"
        );
        assert!(!scene.commands().is_empty());
        assert_eq!(
            format!("{:?}", scene.commands()),
            format!("{:?}", expected.commands())
        );
    }

    #[test]
    fn new_with_setup_provides_contexts_the_build_reads() {
        let tester = Tester::new_with(SIZE, &TesterOptions::default(), || {
            provide_context(Marker(3));
            (Seen::default(), probe)
        });
        assert_eq!(tester.state().marker, Some(Marker(3)));
    }

    #[test]
    fn a_context_on_an_ambient_owner_resolves_inside_build() {
        let ambient = super::Owner::new();
        ambient.set();
        provide_context(Marker(7));
        let tester = Tester::new(Seen::default(), probe, SIZE);
        assert_eq!(tester.state().marker, Some(Marker(7)));
        drop(tester);
        ambient.unset_with_forced_cleanup();
    }

    #[test]
    fn a_relaunched_tester_does_not_see_the_dropped_ones_contexts() {
        let first = Tester::new_with(SIZE, &TesterOptions::default(), || {
            provide_context(Marker(1));
            (Seen::default(), probe)
        });
        assert_eq!(first.state().marker, Some(Marker(1)));
        drop(first);
        let second = Tester::new(Seen::default(), probe, SIZE);
        assert_eq!(second.state().marker, None);
        assert_eq!(second.state().metrics.map(|m| m.size), Some(SIZE));
    }

    #[test]
    fn resize_and_set_metrics_reach_build_and_layout() {
        let mut tester = Tester::new(Seen::default(), probe, Size::new(400.0, 800.0));
        let seen = tester.state().metrics.expect("metrics");
        assert_eq!(seen.orientation, Orientation::Portrait);

        tester.resize(Size::new(800.0, 400.0));
        tester.pump(Duration::ZERO);
        let seen = tester.state().metrics.expect("metrics");
        assert_eq!((seen.size, seen.scale), (Size::new(800.0, 400.0), 1.0));
        assert_eq!(seen.orientation, Orientation::Landscape);
        assert_eq!(tester.size(), Size::new(800.0, 400.0));
        let window = tester.semantics();
        let root = window
            .nodes
            .iter()
            .find(|(id, _)| *id == window.root)
            .and_then(|(_, node)| node.bounds())
            .expect("window bounds");
        assert_eq!((root.width(), root.height()), (800.0, 400.0));

        let doubled = WindowMetrics::new(Size::new(800.0, 400.0), 2.0, WindowInsets::default());
        tester.set_metrics(doubled);
        tester.pump(Duration::ZERO);
        assert_eq!(tester.state().metrics.map(|m| m.scale), Some(2.0));
        assert_eq!(tester.metrics(), doubled);
    }

    struct Loaded {
        task: crate::UseTask<u32>,
    }

    fn loaded_view(loaded: &mut Loaded) -> crate::TextView {
        let value = loaded.task.signal().get_untracked().ready().copied();
        crate::text(format!("{value:?}"))
    }

    #[test]
    fn pump_until_waits_in_real_time_for_blocking_work() {
        let mut tester = Tester::new_with(SIZE, &TesterOptions::default(), || {
            let task = crate::use_task(|| async {
                crate::spawn_blocking(|| {
                    std::thread::sleep(Duration::from_millis(50));
                    42_u32
                })
                .await
            });
            (Loaded { task }, loaded_view)
        });
        let pumps = tester.pump_until("value loaded", Duration::from_secs(5), |tester| {
            tester.state().task.signal().get_untracked().ready() == Some(&42)
        });
        assert!(pumps > 0);
        tester.pump(Duration::ZERO);
        assert!(shows(&tester.semantics(), "Some(42)"));
    }

    #[test]
    #[should_panic(expected = "pump_until: never true did not hold within 200ms")]
    fn pump_until_panics_when_the_predicate_never_holds() {
        let mut tester = Tester::new((), |_: &mut ()| crate::text("idle"), SIZE);
        tester.pump_until("never true", Duration::from_millis(200), |_| false);
    }
}
