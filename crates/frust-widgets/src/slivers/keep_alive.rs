//! `keep_alive`: keeps a lazy list's row alive when it scrolls out of the
//! list's window — Flutter's `KeepAlive` / `AutomaticKeepAlive`
//! (`widgets/automatic_keep_alive.dart`), answered by
//! [`sliver_list`](crate::sliver_list).
//!
//! # The request
//!
//! A row asks to be kept through a [`KeepAliveHandle`]: one acquired while the
//! list builds or rebuilds that row ([`KeepAliveHandle::acquire`]) binds to
//! the row, whatever depth of its subtree acquired it, and the row is kept
//! while any handle bound to it holds a request ([`KeepAliveHandle::set`]).
//! [`keep_alive`] is the declarative form — a transparent wrapper holding one
//! handle, requesting while [`KeepAliveView::enabled`] (the default) — and a
//! custom widget that owns a resource (a player, a socket, a long animation)
//! acquires its own handle in [`View::build`] instead. Dropping a handle, or
//! tearing down the widget holding it, withdraws its request.
//!
//! The binding is a scope that only [`sliver_list`](crate::sliver_list) opens
//! around each row it builds or rebuilds. A `ListView`,
//! `sliver_fixed_extent_list` or `sliver_grid` opens none, so a request inside
//! one of their rows, nested in a `sliver_list` row, binds to the nearest
//! enclosing `sliver_list` row and pins that whole row. Outside every
//! `sliver_list` there is no scope, the handle is `None`, and `keep_alive` is
//! inert.
//!
//! Parked rows are uncapped: memory grows with the rows scrolled past while
//! they stay kept alive.
//!
//! # What the list does with it
//!
//! A row leaving the list's window while its request holds — and whose key the
//! data still holds — is parked in a side map keyed by its [`ChildKey`]
//! instead of being torn down: not laid out, painted, hit tested or visited,
//! its pod and every bit of retained state untouched. When
//! its key re-enters the window the parked pod is re-adopted and rebuilt to the
//! current view. A gesture the row had captured is cancelled as it is parked
//! (nothing would ever route its end to it); a focus link it holds stays
//! recorded but receives nothing while parked. A parked row whose key leaves
//! the data is torn down, and the list tears every parked row down with
//! itself.
//!
//! [`ChildKey`]: crate::ChildKey

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, Widget, any,
};
use kurbo::{Point, Size};

/// How many keep-alive requests one row's handles hold.
type Requests = Rc<Cell<u32>>;

thread_local! {
    /// The rows currently being built or rebuilt, innermost last — the scopes
    /// a [`KeepAliveHandle`] binds to. UI-thread-affine like the build pass.
    static ROW_SCOPES: RefCell<Vec<Requests>> = const { RefCell::new(Vec::new()) };
}

/// Pops [`ROW_SCOPES`] on drop, so an unwinding build cannot leave a stale
/// scope behind.
struct ScopeGuard;

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        ROW_SCOPES.with(|scopes| {
            scopes.borrow_mut().pop();
        });
    }
}

/// Run `f` with `requests` as the innermost row scope.
fn with_row_scope<R>(requests: &Requests, f: impl FnOnce() -> R) -> R {
    ROW_SCOPES.with(|scopes| scopes.borrow_mut().push(Rc::clone(requests)));
    let _guard = ScopeGuard;
    f()
}

/// A keep-alive request bound to the lazy-list row it was acquired in. See
/// the [module docs](self).
pub struct KeepAliveHandle {
    requests: Requests,
    held: bool,
}

impl KeepAliveHandle {
    /// A handle bound to the lazy-list row being built or rebuilt right now —
    /// call it from [`View::build`] (or `rebuild`). `None` outside every lazy
    /// list's row, where keep-alive means nothing. The handle starts holding no
    /// request.
    pub fn acquire() -> Option<Self> {
        ROW_SCOPES
            .with(|scopes| scopes.borrow().last().cloned())
            .map(|requests| Self {
                requests,
                held: false,
            })
    }

    /// Hold (`true`) or withdraw (`false`) this handle's request. Idempotent:
    /// a handle counts once however often it is set.
    pub fn set(&mut self, keep_alive: bool) {
        if keep_alive == self.held {
            return;
        }
        self.held = keep_alive;
        let count = self.requests.get();
        self.requests.set(if keep_alive {
            count.saturating_add(1)
        } else {
            count.saturating_sub(1)
        });
    }

    /// Whether this handle currently holds a request.
    pub fn is_held(&self) -> bool {
        self.held
    }
}

impl Drop for KeepAliveHandle {
    fn drop(&mut self) {
        self.set(false);
    }
}

/// Keep `child`'s lazy-list row alive outside the list's window (see the
/// [module docs](self)). Transparent everywhere else: `child` is laid out,
/// painted and routed to exactly as if it stood alone.
///
/// ```
/// use frust_widgets::{
///     ChildKey, CustomScrollView, custom_scroll_view, keep_alive, sliver_list, text,
/// };
/// let view: CustomScrollView<()> = custom_scroll_view(sliver_list(
///     100,
///     |i| ChildKey::new(i as u64),
///     |i| keep_alive(text(format!("row {i}"))).enabled(i == 0),
/// ));
/// # let _ = view;
/// ```
pub fn keep_alive<State: 'static, V: View<State>>(child: V) -> KeepAliveView<State> {
    KeepAliveView {
        child: any(child),
        enabled: true,
    }
}

/// A keep-alive wrapper. See [`keep_alive`].
pub struct KeepAliveView<State: 'static> {
    child: AnyView<State>,
    enabled: bool,
}

impl<State: 'static> KeepAliveView<State> {
    /// Whether the wrapper requests keep-alive — `true` unless set. A row whose
    /// request is withdrawn is torn down the next time it leaves the window.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// The retained widget for a [`KeepAliveView`]: the child's pod and the handle
/// bound to its row (`None` outside a lazy list).
pub struct KeepAliveWidget {
    child: ChildPod,
    handle: Option<KeepAliveHandle>,
}

impl<State: 'static> View<State> for KeepAliveView<State> {
    type Element = KeepAliveWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> KeepAliveWidget {
        let mut handle = KeepAliveHandle::acquire();
        if let Some(handle) = handle.as_mut() {
            handle.set(self.enabled);
        }
        KeepAliveWidget {
            child: crate::authoring::build_child(&self.child, ctx),
            handle,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut KeepAliveWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if let Some(handle) = element.handle.as_mut() {
            handle.set(self.enabled);
        }
        crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut KeepAliveWidget, ctx: &mut BuildCtx<'_>) {
        if let Some(handle) = element.handle.as_mut() {
            handle.set(false);
        }
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for KeepAliveWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        crate::authoring::route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    crate::authoring::visit_children!(child);
}

// --- The row scope a lazy list wraps each row in --------------------------------

/// A lazy list's row: `child` built and rebuilt inside the row's keep-alive
/// scope, so a [`KeepAliveHandle`] acquired anywhere in it binds to the row.
pub(crate) struct RowScope<V> {
    child: V,
}

impl<V> RowScope<V> {
    pub(crate) fn new(child: V) -> Self {
        Self { child }
    }
}

/// The retained widget for a [`RowScope`]: the row's pod and the requests its
/// handles hold. A transparent single-child box.
pub(crate) struct RowScopeWidget {
    child: ChildPod,
    requests: Requests,
}

impl<State: 'static, V: View<State>> View<State> for RowScope<V> {
    type Element = RowScopeWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> RowScopeWidget {
        let requests = Requests::default();
        let child = with_row_scope(&requests, || {
            crate::authoring::build_child(&self.child, ctx)
        });
        RowScopeWidget { child, requests }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut RowScopeWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let requests = Rc::clone(&element.requests);
        with_row_scope(&requests, || {
            crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
        })
    }

    fn teardown(&self, element: &mut RowScopeWidget, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for RowScopeWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        crate::authoring::route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    crate::authoring::visit_children!(child);
}

/// Whether the row `pod` holds — a [`RowScope`] built through an `AnyView` or
/// directly — currently asks to be kept alive.
pub(crate) fn row_wants_keep_alive(pod: &ChildPod) -> bool {
    let widget: &dyn Any = pod.widget();
    let scope = match widget.downcast_ref::<Box<dyn Widget>>() {
        Some(boxed) => {
            let inner: &dyn Any = &**boxed;
            inner.downcast_ref::<RowScopeWidget>()
        }
        None => widget.downcast_ref::<RowScopeWidget>(),
    };
    scope.is_some_and(|scope| scope.requests.get() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_handle_binds_to_the_innermost_row_and_counts_once() {
        assert!(KeepAliveHandle::acquire().is_none(), "no row, no handle");
        let outer = Requests::default();
        let inner = Requests::default();
        let (mut a, mut b) = with_row_scope(&outer, || {
            let a = KeepAliveHandle::acquire().expect("in a row");
            let b = with_row_scope(&inner, || KeepAliveHandle::acquire().expect("nested row"));
            (a, b)
        });
        assert!(KeepAliveHandle::acquire().is_none(), "scopes closed");
        a.set(true);
        a.set(true);
        b.set(true);
        assert_eq!((outer.get(), inner.get()), (1, 1));
        a.set(false);
        assert_eq!(outer.get(), 0);
        drop(b);
        assert_eq!(inner.get(), 0, "dropping a handle withdraws its request");
        assert!(!a.is_held());
    }
}
