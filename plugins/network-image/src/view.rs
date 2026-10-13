//! [`network_image`]: a URL image view with placeholder, error and fade-in.
//!
//! # Shape
//!
//! The view hosts one load at a time, keyed by `(url, headers, target_px,
//! loader)`. On mount it first peeks the loader's memory cache: a hit paints
//! the image on the very first frame with no fade, which is what keeps a
//! scrolling list from flickering. A miss starts the load through
//! [`use_task`] inside a private [`Component`] whose owner scopes it, so
//! unmounting the view (or changing its key) aborts the load and its result is
//! never written. The body is an [`EitherOf3`] of placeholder | image | error;
//! changing arm is a teardown + remount.
//!
//! # Fade
//!
//! The image arm sits in [`AnimatedOpacityView`] with an explicit
//! [`Timing::Duration`] of `fade`. When the load lands asynchronously the arm
//! is mounted at opacity `0.0` and retargeted to `1.0` in the same rebuild, so
//! the tween starts on the first paint after the `Ready` state lands and
//! requests frames until it settles. `Duration::ZERO` and memory hits skip it.

use std::cell::RefCell;
use std::convert::Infallible;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, Point, Role, SemanticsCtx, Size, Widget, build_child, rebuild_child,
    route_event_single, teardown_child,
};
use frust::motion::{AnimatedOpacityView, animated_opacity};
use frust::{
    AnyView, AsyncValue, Component, ComponentView, Curve, Either, EitherOf3, GestureDetector,
    GestureDetectorView, Get, Image, ImageFit, ImageSource, SizedBox, Timing, UseTask, View,
    component, use_task,
};

use crate::decode::TargetPx;
use crate::install::{NetworkImages, warn_not_installed};
use crate::loader::{ImageLoader, LoadError, LoadRequest};

/// Default fade-in duration.
const DEFAULT_FADE: Duration = Duration::from_millis(200);

/// Where a [`network_image`] view's load stands.
#[derive(Clone, Debug)]
pub enum LoadState {
    /// The load is in flight; the placeholder shows.
    Loading,
    /// The image is decoded and showing.
    Ready(ImageSource),
    /// The load failed; the error slot shows.
    Failed(LoadError),
}

/// A shared, already-erased slot view, so the body can be rebuilt every frame
/// from the same stored placeholder/error without cloning a view.
struct Slot<S: 'static>(Rc<AnyView<S>>);

impl<S: 'static> Clone for Slot<S> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<S: 'static> Slot<S> {
    fn new<V: View<S>>(view: V) -> Self {
        // erasure: keep heterogeneous placeholder/error slot stored on the view
        Self(Rc::new(AnyView::new(view)))
    }

    fn empty() -> Self {
        Self::new(SizedBox(None, None))
    }
}

impl<S: 'static> View<S> for Slot<S> {
    type Element = <AnyView<S> as View<S>>::Element;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
        self.0.build(ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        self.0.rebuild(&prev.0, element, ctx)
    }

    fn teardown(&self, element: &mut Self::Element, ctx: &mut BuildCtx<'_>) {
        self.0.teardown(element, ctx);
    }
}

/// The three arms: placeholder | faded image | error (tap-to-retry or plain).
type Body<S> = EitherOf3<Slot<S>, AnimatedOpacityView<S>, Either<Slot<S>, GestureDetectorView<S>>>;

/// The private component whose owner scopes one load's [`use_task`]; its
/// state holds the task (and so its `RwSignal`) for exactly the load's life.
struct LoadHost {
    loader: Arc<ImageLoader>,
    request: LoadRequest,
    out: Rc<RefCell<Option<UseTask<LoadState>>>>,
}

impl Component for LoadHost {
    type State = UseTask<LoadState>;

    fn init(&self) -> UseTask<LoadState> {
        let loader = self.loader.clone();
        let request = self.request.clone();
        // `load` registers synchronously, so sibling views share one fetch.
        let task = use_task(move || {
            let load = loader.load(request.clone());
            async move {
                Ok::<_, Infallible>(match load.await {
                    Ok(source) => LoadState::Ready(source),
                    Err(e) => LoadState::Failed(e),
                })
            }
        });
        *self.out.borrow_mut() = Some(task.clone());
        task
    }

    fn build(&self, _task: &mut UseTask<LoadState>) -> impl View<UseTask<LoadState>> {
        SizedBox(Some(0.0), Some(0.0))
    }
}

type HostWidget = <ComponentView<LoadHost> as View<()>>::Element;

/// Where the element's [`LoadState`] comes from.
enum Source {
    /// Known at mount: a memory hit, or no loader installed.
    Fixed(LoadState),
    /// A running load, owned by its host component (boxed: the widget is large).
    Task {
        host_view: ComponentView<LoadHost>,
        host: Box<HostWidget>,
        task: UseTask<LoadState>,
    },
}

impl Source {
    /// The current state; a tracked read for a running load.
    fn current(&self) -> LoadState {
        match self {
            Source::Fixed(state) => state.clone(),
            Source::Task { task, .. } => match task.signal().get() {
                AsyncValue::Ready(state) => state,
                _ => LoadState::Loading,
            },
        }
    }

    fn task(&self) -> Option<&UseTask<LoadState>> {
        match self {
            Source::Fixed(_) => None,
            Source::Task { task, .. } => Some(task),
        }
    }

    /// Tear the host down: disposes its owner, aborting the load.
    fn stop(&mut self, ctx: &mut BuildCtx<'_>) {
        if let Source::Task {
            host_view, host, ..
        } = self
        {
            View::<()>::teardown(host_view, host, ctx);
        }
    }
}

/// A network image view. Build with [`network_image`].
pub struct NetworkImageView<S: 'static> {
    url: String,
    headers: Vec<(String, String)>,
    placeholder: Slot<S>,
    error: Slot<S>,
    fit: ImageFit,
    target: Option<TargetPx>,
    fade: Duration,
    retry_on_tap: bool,
    loader: Option<Arc<ImageLoader>>,
    label: Option<String>,
}

/// An image loaded from `url` (absolute `http`/`https`). Shows an empty box
/// while loading and on failure until [`placeholder`](NetworkImageView::placeholder)
/// / [`error`](NetworkImageView::error) say otherwise; once loaded it behaves
/// like `Image(source)`.
///
/// ```no_run
/// use std::time::Duration;
///
/// use frust::{Component, ImageFit, View, text};
/// use frust_network_image::network_image;
///
/// #[derive(Default)]
/// pub struct Cover;
///
/// impl Component for Cover {
///     type State = ();
///
///     fn init(&self) {}
///
///     fn build(&self, _state: &mut ()) -> impl View<()> {
///         network_image("https://example.com/cover.jpg")
///             .placeholder(text("Loading"))
///             .error(text("Image unavailable"))
///             .fit(ImageFit::Cover)
///             .target_px(800, 600)
///             .fade(Duration::from_millis(250))
///             .semantic_label("Album cover")
///     }
/// }
/// ```
pub fn network_image<S: 'static>(url: impl Into<String>) -> NetworkImageView<S> {
    NetworkImageView {
        url: url.into(),
        headers: Vec::new(),
        placeholder: Slot::empty(),
        error: Slot::empty(),
        fit: ImageFit::default(),
        target: None,
        fade: DEFAULT_FADE,
        retry_on_tap: false,
        loader: None,
        label: None,
    }
}

/// PascalCase alias for [`network_image`] (mirrors `Image`).
#[allow(non_snake_case)]
pub fn NetworkImage<S: 'static>(url: impl Into<String>) -> NetworkImageView<S> {
    network_image(url)
}

impl<S: 'static> NetworkImageView<S> {
    /// Extra request headers (e.g. `Authorization`). Part of the load key, not
    /// the cache key.
    pub fn headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.headers = headers;
        self
    }

    /// Shown while the image loads.
    pub fn placeholder(mut self, view: impl View<S>) -> Self {
        self.placeholder = Slot::new(view);
        self
    }

    /// Shown when the load fails.
    pub fn error(mut self, view: impl View<S>) -> Self {
        self.error = Slot::new(view);
        self
    }

    /// How the loaded image fills its box (default [`ImageFit::Contain`]).
    pub fn fit(mut self, fit: ImageFit) -> Self {
        self.fit = fit;
        self
    }

    /// Decode into at most `width` x `height` pixels (aspect ratio kept).
    pub fn target_px(mut self, width: u32, height: u32) -> Self {
        self.target = Some(TargetPx { width, height });
        self
    }

    /// Fade-in duration once a network load lands (default 200 ms;
    /// `Duration::ZERO` disables). Memory-cache hits never fade.
    pub fn fade(mut self, fade: Duration) -> Self {
        self.fade = fade;
        self
    }

    /// Tapping the error slot restarts the load (default false).
    pub fn retry_on_tap(mut self, retry: bool) -> Self {
        self.retry_on_tap = retry;
        self
    }

    /// Load through `loader` instead of [`NetworkImages::loader`].
    pub fn loader(mut self, loader: Arc<ImageLoader>) -> Self {
        self.loader = Some(loader);
        self
    }

    /// The accessible name of the image node.
    pub fn semantic_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Whether `prev` loads something different from `self`.
    fn key_changed(&self, prev: &Self) -> bool {
        let same_loader = match (&self.loader, &prev.loader) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        self.url != prev.url
            || self.headers != prev.headers
            || self.target != prev.target
            || !same_loader
    }

    /// Resolve the loader and either answer from memory or start a load.
    fn start(&self, ctx: &mut BuildCtx<'_>) -> Source {
        let Some(loader) = self.loader.clone().or_else(NetworkImages::loader) else {
            warn_not_installed();
            return Source::Fixed(LoadState::Failed(LoadError::NotInstalled));
        };
        if let Some(source) = loader.peek_memory(&self.url, self.target) {
            return Source::Fixed(LoadState::Ready(source));
        }
        let out = Rc::new(RefCell::new(None));
        let host_view = component(LoadHost {
            loader,
            request: LoadRequest {
                url: self.url.clone(),
                headers: self.headers.clone(),
                target: self.target,
            },
            out: out.clone(),
        });
        let host = Box::new(View::<()>::build(&host_view, ctx));
        let task = out
            .borrow_mut()
            .take()
            .expect("LoadHost::init runs inside ComponentView::build");
        Source::Task {
            host_view,
            host,
            task,
        }
    }

    /// The body for `state`, with the image arm at `opacity`.
    fn body(&self, state: &LoadState, opacity: f64, task: Option<&UseTask<LoadState>>) -> Body<S> {
        match state {
            LoadState::Loading => EitherOf3::A(self.placeholder.clone()),
            LoadState::Ready(source) => EitherOf3::B(
                animated_opacity(opacity, Image(source.clone()).fit(self.fit))
                    .timing(Timing::Duration(self.fade, Curve::EaseOut)),
            ),
            LoadState::Failed(_) => match task.filter(|_| self.retry_on_tap) {
                Some(task) => {
                    let task = task.clone();
                    EitherOf3::C(Either::Right(
                        GestureDetector(self.error.clone()).on_tap(move |_: &mut S| task.restart()),
                    ))
                }
                None => EitherOf3::C(Either::Left(self.error.clone())),
            },
        }
    }
}

/// The retained widget behind a [`NetworkImageView`].
pub struct NetworkImageWidget<S: 'static> {
    source: Source,
    body: Body<S>,
    pod: ChildPod,
    label: Option<String>,
}

impl<S: 'static> View<S> for NetworkImageView<S> {
    type Element = NetworkImageWidget<S>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> NetworkImageWidget<S> {
        let source = self.start(ctx);
        let body = self.body(&source.current(), 1.0, source.task());
        let pod = build_child(&body, ctx);
        NetworkImageWidget {
            source,
            body,
            pod,
            label: self.label.clone(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut NetworkImageWidget<S>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if self.key_changed(prev) {
            element.source.stop(ctx);
            element.source = self.start(ctx);
        }
        let state = element.source.current();
        let task = element.source.task();
        let landed = matches!(state, LoadState::Ready(_))
            && !matches!(element.body, EitherOf3::B(_))
            && task.is_some();
        if landed && !self.fade.is_zero() {
            // Mount the image hidden, then retarget: the tween starts at the
            // next paint.
            let hidden = self.body(&state, 0.0, task);
            flags |= rebuild_child(&element.body, &hidden, &mut element.pod, ctx);
            element.body = hidden;
        }
        let body = self.body(&state, 1.0, task);
        flags |= rebuild_child(&element.body, &body, &mut element.pod, ctx);
        element.body = body;
        if element.label != self.label {
            element.label = self.label.clone();
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut NetworkImageWidget<S>, ctx: &mut BuildCtx<'_>) {
        teardown_child(&element.body, &mut element.pod, ctx);
        element.source.stop(ctx);
    }
}

impl<S: 'static> Widget for NetworkImageWidget<S> {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.pod.layout_child(ctx, bc);
        self.pod.set_origin(Point::ZERO);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.pod.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event_single(&mut self.pod, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Image,
            |node| {
                if let Some(label) = &self.label {
                    node.set_label(label.as_str());
                }
            },
            |ctx| self.pod.semantics_child(ctx),
        );
    }

    frust::authoring::visit_children!(pod);
}
