//! The selection-toolbar seam: what a text field asks for when it has a
//! selection, and who draws it.
//!
//! # Two routes, one request
//!
//! A "copy / cut / paste / select all" bar over a selection is drawn by the
//! *platform* on iOS (`UIEditMenuInteraction` — the system owns its look, its
//! placement and its animation) and by the *framework* everywhere else. Both
//! routes need the same facts — where the selection is, and which verbs apply —
//! so a field publishes one [`SelectionToolbarRequest`] per paint and the two
//! routes diverge downstream of it:
//!
//! * [`SelectionToolbarPolicy::Native`]: the request surfaces on
//!   [`RenderRoot::selection_toolbar`](crate::app::RenderRoot::selection_toolbar)
//!   with a generation a shell diffs, and the shell asks the platform to present
//!   its own menu.
//! * [`SelectionToolbarPolicy::Framework`]: the field hosts its own toolbar pod
//!   through the overlay portal (see [`crate::overlay`]) — and still publishes
//!   the request, which costs a pointer-sized write and keeps one code path
//!   rather than two.
//!
//! # Process-global slots
//!
//! The policy and the builder are process-global `Mutex` slots, mirroring
//! `frust_shell_common::theme_override`'s contract exactly: callable from any
//! thread (documented, not enforced), read on the UI thread by whoever needs
//! them, and last-writer-wins. They live in `frust-core` rather than beside
//! their nearest neighbours in the shells because
//! [`RenderRoot`](crate::app::RenderRoot) itself is a reader — a slot a shell
//! owned would be unreachable from here without inverting the layer
//! dependencies.
//!
//! The builder is what a design system installs so a text field can float
//! *someone's* toolbar without `frust-core` knowing a single widget type: it
//! returns an [`AnyView`] over `()` (the pod's own state; see [`crate::overlay`]
//! for why an overlay pod is state-independent of the app), so the catalog that
//! installs it decides the whole look.

use std::cell::Cell;
use std::sync::{Arc, Mutex};

use kurbo::{Rect, Size};

use crate::event::PassBracket;
use crate::view::AnyView;

thread_local! {
    /// The selection-toolbar request published during the paint pass currently
    /// running on this thread — written by
    /// [`PaintCtx::publish_selection_toolbar`](crate::widget::PaintCtx::publish_selection_toolbar)
    /// and resolved by [`RenderRoot::paint`](crate::app::RenderRoot::paint).
    ///
    /// Last writer wins, for
    /// [`EventCtx::set_cursor`](crate::event::EventCtx::set_cursor)'s reason: at
    /// most one field holds the focus a selection belongs to, so "who asked"
    /// adds nothing, and no container between the field and the root reads the
    /// value.
    ///
    /// **Pass-scoped**, so a pass in which nobody published resolves to `None`
    /// rather than to whatever the previous pass left — which is what makes a
    /// collapsed selection put the toolbar away with no widget having to retract
    /// anything.
    static PUBLISHED: Cell<Option<SelectionToolbarRequest>> = const { Cell::new(None) };

    /// Whether a selection-toolbar publish pass is open on this thread — owned
    /// by [`SelectionToolbarPass`] alone (see [`PassBracket`]).
    static PASS_OPEN: Cell<bool> = const { Cell::new(false) };
}

/// Which clipboard verbs apply to the current selection — the enabled set, not a
/// menu layout.
///
/// A field computes these from its own state (an empty selection has nothing to
/// copy; a read-only field cannot cut or paste; a field whose whole content is
/// already selected offers no select-all), and whoever draws the menu decides
/// how to present a disabled verb — by omitting it, greying it, or ignoring the
/// distinction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SelectionToolbarActions {
    /// Copy the selection to the host clipboard.
    pub copy: bool,
    /// Cut the selection to the host clipboard.
    pub cut: bool,
    /// Replace the selection with the host clipboard's contents.
    pub paste: bool,
    /// Select the field's whole content.
    pub select_all: bool,
}

/// One field's published "there is a selection here, and these verbs apply".
///
/// Published per paint pass; a pass without one means no field has a selection
/// worth a toolbar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionToolbarRequest {
    /// The selection's bounding rect in **absolute logical window space** — the
    /// same space an [`OverlayEntry::window_rect`](crate::overlay::OverlayEntry::window_rect)
    /// uses, so a framework toolbar can be placed against it directly and a
    /// native menu can be anchored to it after the shell's own scale conversion.
    pub anchor: Rect,
    /// Which verbs apply.
    pub actions: SelectionToolbarActions,
}

/// Who draws the selection toolbar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SelectionToolbarPolicy {
    /// The framework draws it: the field floats a toolbar pod through the
    /// overlay portal ([`crate::overlay`]). The default, and the only route on a
    /// platform with no system edit menu.
    #[default]
    Framework,
    /// The platform draws it: the field publishes the request and nothing else,
    /// and the shell presents the host's own menu.
    Native,
}

/// The process-wide toolbar policy. A plain value rather than a generation-
/// carrying slot like [`BUILDER`]: it is read directly at the point of decision
/// (there is nothing to diff), and the default is the Framework route.
static POLICY: Mutex<SelectionToolbarPolicy> = Mutex::new(SelectionToolbarPolicy::Framework);

/// Choose who draws the selection toolbar, process-wide.
///
/// Callable from any thread; the slot is a plain `Mutex`, and each reader
/// observes it on its own thread when it next asks (see the module docs' thread
/// contract). A shell sets [`SelectionToolbarPolicy::Native`] once at start-up on
/// a platform whose system edit menu it intends to drive.
pub fn set_selection_toolbar_policy(policy: SelectionToolbarPolicy) {
    *POLICY.lock().unwrap_or_else(|e| e.into_inner()) = policy;
}

/// The process-wide toolbar policy, [`SelectionToolbarPolicy::Framework`] until
/// something sets otherwise.
pub fn selection_toolbar_policy() -> SelectionToolbarPolicy {
    *POLICY.lock().unwrap_or_else(|e| e.into_inner())
}

/// Builds the view a framework-drawn selection toolbar floats.
///
/// Called with the request the field published and the logical window size (so
/// the toolbar can size or clamp itself against the window it will float in),
/// and returns the pod's view over `()` — an overlay pod carries no application
/// state, so a toolbar built here is usable from a field hosted under any app
/// state at all.
///
/// `Arc<dyn Fn ... + Send + Sync>` because the slot is process-global: the
/// closure is shared by every root in the process and may be installed from any
/// thread.
pub type SelectionToolbarBuilder =
    Arc<dyn Fn(&SelectionToolbarRequest, Size) -> AnyView<()> + Send + Sync>;

/// The process-wide builder slot; `None` until a design system installs one, in
/// which case a field under the Framework policy floats nothing and the platform
/// route is the only one available.
static BUILDER: Mutex<Option<SelectionToolbarBuilder>> = Mutex::new(None);

/// Install the selection-toolbar builder, **replacing** whatever was there.
///
/// The explicit-override half of the pair: an app that wants its own toolbar
/// over the catalog's calls this. Callable from any thread (see the module docs).
pub fn set_selection_toolbar_builder(builder: SelectionToolbarBuilder) {
    *BUILDER.lock().unwrap_or_else(|e| e.into_inner()) = Some(builder);
}

/// Install the builder only if none is installed, reporting whether it took.
///
/// The cooperative half of the pair, and what a design system's own
/// initialisation should call: two catalogs linked into one binary must not
/// fight over the slot, and an app's explicit
/// [`set_selection_toolbar_builder`] must not be undone by a catalog
/// initialising later.
pub fn install_selection_toolbar_builder_if_unset(builder: SelectionToolbarBuilder) -> bool {
    let mut slot = BUILDER.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_some() {
        return false;
    }
    *slot = Some(builder);
    true
}

/// The installed builder, or `None` when nothing has installed one.
///
/// Returns a clone of the `Arc` rather than lending the slot, so a caller never
/// holds the process-global lock while building a view.
pub fn selection_toolbar_builder() -> Option<SelectionToolbarBuilder> {
    BUILDER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(Arc::clone)
}

/// The open/close bracket around one selection-toolbar publish pass — the
/// [`crate::overlay::OverlayPaintPass`] shape, one channel over.
///
/// [`RenderRoot::paint`](crate::app::RenderRoot::paint) holds one for the length
/// of the pass, so a publish made by a widget painted with no root above it (a
/// leaf unit test) is dropped by the next pass's clear rather than leaking into
/// it.
pub(crate) struct SelectionToolbarPass(PassBracket<Option<SelectionToolbarRequest>>);

impl SelectionToolbarPass {
    /// Open a publish pass, starting from "nobody has published anything".
    pub(crate) fn enter() -> Self {
        SelectionToolbarPass(PassBracket::enter(&PUBLISHED, &PASS_OPEN))
    }

    /// Take what *this* pass published, leaving the slot empty.
    pub(crate) fn take(&self) -> Option<SelectionToolbarRequest> {
        self.0.take()
    }
}

/// Record `request` in the pass currently painting on this thread.
///
/// The implementation behind
/// [`PaintCtx::publish_selection_toolbar`](crate::widget::PaintCtx::publish_selection_toolbar),
/// which is the API a field calls; this is the module-private half so the slot
/// stays owned here.
pub(crate) fn publish(request: SelectionToolbarRequest) {
    PUBLISHED.with(|slot| slot.set(Some(request)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// Serialises the tests that mutate the process-global policy/builder slots
    /// — mirrors `frust_shell_common::theme_override`'s `TEST_LOCK` pattern, and
    /// is necessary for the same reason: Rust runs a crate's tests in parallel
    /// threads that share one process, so two tests writing the same static
    /// would see each other's writes.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    fn toolbar_request() -> SelectionToolbarRequest {
        SelectionToolbarRequest {
            anchor: Rect::new(10.0, 20.0, 110.0, 40.0),
            actions: SelectionToolbarActions {
                copy: true,
                cut: true,
                paste: false,
                select_all: true,
            },
        }
    }

    #[test]
    fn policy_defaults_to_framework_and_round_trips() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_selection_toolbar_policy(SelectionToolbarPolicy::Framework);
        assert_eq!(
            selection_toolbar_policy(),
            SelectionToolbarPolicy::Framework
        );
        assert_eq!(
            SelectionToolbarPolicy::default(),
            SelectionToolbarPolicy::Framework,
            "the default route is the framework-drawn one"
        );

        set_selection_toolbar_policy(SelectionToolbarPolicy::Native);
        assert_eq!(selection_toolbar_policy(), SelectionToolbarPolicy::Native);

        // Leave the slot as the rest of the process expects to find it.
        set_selection_toolbar_policy(SelectionToolbarPolicy::Framework);
    }

    #[test]
    fn install_if_unset_yields_to_an_installed_builder_but_set_replaces_it() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Start from a known-empty slot (another test may have installed one).
        *BUILDER.lock().unwrap_or_else(|e| e.into_inner()) = None;
        assert!(
            selection_toolbar_builder().is_none(),
            "nothing is installed at rest"
        );

        let first: SelectionToolbarBuilder = Arc::new(|_req, _size| crate::view::any(FirstView));
        assert!(
            install_selection_toolbar_builder_if_unset(Arc::clone(&first)),
            "the first cooperative install takes the empty slot"
        );

        let second: SelectionToolbarBuilder = Arc::new(|_req, _size| crate::view::any(SecondView));
        assert!(
            !install_selection_toolbar_builder_if_unset(Arc::clone(&second)),
            "a second cooperative install must not displace the first"
        );
        assert!(
            Arc::ptr_eq(
                &selection_toolbar_builder().expect("a builder is installed"),
                &first
            ),
            "the slot still holds the first builder"
        );

        set_selection_toolbar_builder(Arc::clone(&second));
        assert!(
            Arc::ptr_eq(
                &selection_toolbar_builder().expect("a builder is installed"),
                &second
            ),
            "an explicit set replaces whatever stood"
        );

        *BUILDER.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    #[test]
    fn publish_is_pass_scoped_and_last_writer_wins() {
        let pass = SelectionToolbarPass::enter();
        assert_eq!(pass.take(), None, "a fresh pass starts empty");

        publish(toolbar_request());
        let mut second = toolbar_request();
        second.actions.paste = true;
        publish(second);
        assert_eq!(
            pass.take(),
            Some(second),
            "the last publish of the pass is the one resolved"
        );
        assert_eq!(pass.take(), None, "and the drain empties the slot");
        drop(pass);

        // A publish made with no pass open is dropped by the next pass's clear,
        // never leaked into it.
        publish(toolbar_request());
        let next = SelectionToolbarPass::enter();
        assert_eq!(
            next.take(),
            None,
            "a stray publish does not survive into the next pass"
        );
    }

    /// A view double for the builder-slot tests — the builder's return type is
    /// `AnyView<()>`, and these tests only ever compare `Arc` identities, so the
    /// view never has to build anything.
    struct FirstView;
    /// The second double; see [`FirstView`].
    struct SecondView;

    macro_rules! stub_view {
        ($name:ident) => {
            impl crate::view::View<()> for $name {
                type Element = StubWidget;
                fn build(&self, _ctx: &mut crate::view::BuildCtx<'_>) -> StubWidget {
                    StubWidget
                }
                fn rebuild(
                    &self,
                    _prev: &Self,
                    _element: &mut StubWidget,
                    _ctx: &mut crate::view::BuildCtx<'_>,
                ) -> crate::view::ChangeFlags {
                    crate::view::ChangeFlags::NONE
                }
            }
        };
    }
    stub_view!(FirstView);
    stub_view!(SecondView);

    /// The widget both doubles build: a zero-sized leaf that paints nothing.
    struct StubWidget;
    impl crate::widget::Widget for StubWidget {
        fn layout(
            &mut self,
            _ctx: &mut crate::widget::LayoutCtx,
            _bc: &crate::layout::BoxConstraints,
        ) -> Size {
            Size::ZERO
        }
        fn paint(
            &mut self,
            _ctx: &mut crate::widget::PaintCtx,
            _scene: &mut dyn crate::widget::PaintScene,
        ) {
        }
    }
}
