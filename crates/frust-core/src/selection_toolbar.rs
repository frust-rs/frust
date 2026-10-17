//! The selection-toolbar seam: what a focused text field publishes about its
//! selection, and who draws the copy/cut/paste/select-all bar over it.
//!
//! A focused field publishes one [`SelectionToolbarRequest`] per paint. Under
//! [`SelectionToolbarPolicy::Native`] a shell reads it from `RenderRoot::selection_toolbar`
//! and presents the platform menu; under [`SelectionToolbarPolicy::Framework`] the
//! field floats the installed builder's view through [`crate::overlay`]. The policy
//! and builder are process-global `Mutex` slots, callable from any thread.

use std::cell::Cell;
use std::sync::{Arc, Mutex};

use kurbo::{Rect, Size};

use crate::event::PassBracket;
use crate::view::AnyView;

thread_local! {
    /// This paint pass's request, last writer wins; pass-scoped, so a pass with
    /// no publish (a blurred field) resolves to `None`.
    static PUBLISHED: Cell<Option<SelectionToolbarRequest>> = const { Cell::new(None) };

    static PASS_OPEN: Cell<bool> = const { Cell::new(false) };
}

/// Which clipboard verbs apply to the current selection: the enabled set, not a
/// menu layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SelectionToolbarActions {
    pub copy: bool,
    pub cut: bool,
    pub paste: bool,
    pub select_all: bool,
}

/// One focused field's selection, its applicable verbs, and whether it wants a
/// menu now. Published every paint while focused, with or without a selection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionToolbarRequest {
    /// The selection's bounds (the caret rect when collapsed), in absolute
    /// logical window space.
    pub anchor: Rect,
    /// A level from the field's own state, so a hardware Cmd+C finds an answer.
    pub actions: SelectionToolbarActions,
    /// The one edge: a gesture asked for the bar and it has not been dismissed.
    pub present_menu: bool,
}

/// Who draws the selection toolbar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SelectionToolbarPolicy {
    /// The field floats a toolbar pod through the overlay portal. The default.
    #[default]
    Framework,
    /// The field only publishes the request; the shell presents the host menu.
    Native,
}

/// Value and lock share one `Mutex`, so a lock claim and a set never interleave.
#[derive(Clone, Copy, Debug)]
struct PolicyState {
    value: SelectionToolbarPolicy,
    locked: bool,
}

static POLICY: Mutex<PolicyState> = Mutex::new(PolicyState {
    value: SelectionToolbarPolicy::Framework,
    locked: false,
});

/// Choose who draws the selection toolbar, process-wide. A no-op once a shell
/// has called [`lock_selection_toolbar_policy`]; a debug build logs each refusal.
pub fn set_selection_toolbar_policy(policy: SelectionToolbarPolicy) {
    let mut state = POLICY.lock().unwrap_or_else(|e| e.into_inner());
    if state.locked {
        #[cfg(debug_assertions)]
        eprintln!(
            "frust-core: the selection-toolbar policy is locked to {:?} by the platform shell; \
             ignoring an app's set_selection_toolbar_policy({policy:?}) call (see \
             frust_core::lock_selection_toolbar_policy)",
            state.value
        );
        return;
    }
    state.value = policy;
}

/// The process-wide toolbar policy; [`SelectionToolbarPolicy::Framework`] until set.
pub fn selection_toolbar_policy() -> SelectionToolbarPolicy {
    POLICY.lock().unwrap_or_else(|e| e.into_inner()).value
}

/// Set and lock the policy a shell's platform requires; only the first claim
/// wins. Returns `false`, leaving the value alone, when already locked.
pub fn lock_selection_toolbar_policy(policy: SelectionToolbarPolicy) -> bool {
    let mut state = POLICY.lock().unwrap_or_else(|e| e.into_inner());
    if state.locked {
        return false;
    }
    state.value = policy;
    state.locked = true;
    true
}

/// Builds the toolbar view from the request and the logical window size. The
/// view is over `()`, so one builder serves fields under any app state.
pub type SelectionToolbarBuilder =
    Arc<dyn Fn(&SelectionToolbarRequest, Size) -> AnyView<()> + Send + Sync>;

/// `None` until something installs a builder; until then a Framework-policy
/// field has nothing to float.
static BUILDER: Mutex<Option<SelectionToolbarBuilder>> = Mutex::new(None);

/// Install the builder, replacing any already installed (the app override).
pub fn set_selection_toolbar_builder(builder: SelectionToolbarBuilder) {
    *BUILDER.lock().unwrap_or_else(|e| e.into_inner()) = Some(builder);
}

/// Install the builder only if none is installed, returning whether it took;
/// what a design system calls, so it never undoes an app's override.
pub fn install_selection_toolbar_builder_if_unset(builder: SelectionToolbarBuilder) -> bool {
    let mut slot = BUILDER.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_some() {
        return false;
    }
    *slot = Some(builder);
    true
}

/// A clone of the installed builder, so no caller builds a view under the lock.
pub fn selection_toolbar_builder() -> Option<SelectionToolbarBuilder> {
    BUILDER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(Arc::clone)
}

/// The [`PassBracket`] around one publish pass, the `OverlayPaintPass` shape.
pub(crate) struct SelectionToolbarPass(PassBracket<Option<SelectionToolbarRequest>>);

impl SelectionToolbarPass {
    pub(crate) fn enter() -> Self {
        SelectionToolbarPass(PassBracket::enter(&PUBLISHED, &PASS_OPEN))
    }

    pub(crate) fn take(&self) -> Option<SelectionToolbarRequest> {
        self.0.take()
    }
}

/// Record `request` in this thread's open pass (behind `PaintCtx::publish_selection_toolbar`).
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
            present_menu: true,
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

    /// Test-only escape hatch: release a lock claimed with
    /// [`lock_selection_toolbar_policy`] so one test proving the lock's effect
    /// does not trap every test after it that mutates the process-global
    /// policy the ordinary way. Not `pub`, not `#[cfg(test)]`-exported beyond
    /// this module: unlocking a shell's declared policy from app code would
    /// defeat the whole point of the lock, so this stays a private detail of
    /// this crate's own test suite rather than a casual public API.
    fn unlock_selection_toolbar_policy_for_test() {
        POLICY.lock().unwrap_or_else(|e| e.into_inner()).locked = false;
    }

    #[test]
    fn set_policy_is_unaffected_when_nothing_has_locked_it() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // At rest, nothing has locked the slot — desktop, Android and web
        // never call `lock_selection_toolbar_policy`, so this is their whole
        // world: `set_selection_toolbar_policy` behaves exactly as it always
        // has.
        set_selection_toolbar_policy(SelectionToolbarPolicy::Native);
        assert_eq!(selection_toolbar_policy(), SelectionToolbarPolicy::Native);
        set_selection_toolbar_policy(SelectionToolbarPolicy::Framework);
        assert_eq!(
            selection_toolbar_policy(),
            SelectionToolbarPolicy::Framework
        );
    }

    #[test]
    fn locking_the_policy_sets_the_declared_value() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_selection_toolbar_policy(SelectionToolbarPolicy::Framework);

        assert!(
            lock_selection_toolbar_policy(SelectionToolbarPolicy::Native),
            "the first claim always takes the lock"
        );
        assert_eq!(
            selection_toolbar_policy(),
            SelectionToolbarPolicy::Native,
            "claiming the lock sets the value it declares"
        );

        unlock_selection_toolbar_policy_for_test();
    }

    #[test]
    fn a_locked_policy_refuses_a_later_set_and_a_later_claim() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_selection_toolbar_policy(SelectionToolbarPolicy::Framework);
        assert!(lock_selection_toolbar_policy(
            SelectionToolbarPolicy::Native
        ));

        // An app's later call must not move a locked slot.
        set_selection_toolbar_policy(SelectionToolbarPolicy::Framework);
        assert_eq!(
            selection_toolbar_policy(),
            SelectionToolbarPolicy::Native,
            "a locked slot must not move for an app's later set_selection_toolbar_policy call"
        );

        // Nor may a second shell's claim silently displace the first.
        assert!(
            !lock_selection_toolbar_policy(SelectionToolbarPolicy::Framework),
            "a second lock claim must not displace the first"
        );
        assert_eq!(selection_toolbar_policy(), SelectionToolbarPolicy::Native);

        // Leave the slot as the rest of the process expects to find it.
        unlock_selection_toolbar_policy_for_test();
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
