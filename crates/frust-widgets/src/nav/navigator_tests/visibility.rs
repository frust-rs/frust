//! Page visibility: `PushOptions::on_visibility`/`PageVisibility` reporting
//! and `cull_covered_builds`' build-skip/retained-state contract.

use super::super::*;
use super::support::*;
use crate::test_support::RecordingScene;
use frust_core::{FrameTime, RenderRoot};
use std::cell::Cell;

// --- Page visibility. ---

type VisLog = Rc<RefCell<Vec<(&'static str, PageVisibility)>>>;

#[test]
fn on_visibility_reports_current_covered_current_and_never_repeats() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let seen: VisLog = Rc::new(RefCell::new(Vec::new()));
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        let seen = seen.clone();
        move |_: &mut ()| {
            let seen = seen.clone();
            navigator(&ctrl, || sized_page(100.0, 100.0))
                .on_root_visibility(move |v| seen.borrow_mut().push(("root", v)))
        }
    };
    let mut state = ();

    // The root page's opening observation fires on the navigator's build.
    root.rebuild(&mut app, &mut state);
    assert_eq!(*seen.borrow(), vec![("root", PageVisibility::Current)]);

    // Idle frames repeat nothing.
    root.rebuild(&mut app, &mut state);
    root.rebuild(&mut app, &mut state);
    assert_eq!(seen.borrow().len(), 1, "a value is never reported twice");

    // A TRANSPARENT push (a dialog) leaves the root painted → `Visible`.
    {
        let seen = seen.clone();
        controller.push_with_options(
            || sized_page(50.0, 50.0),
            PushOptions::transparent()
                .on_visibility(move |v| seen.borrow_mut().push(("dialog", v))),
        );
    }
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        *seen.borrow(),
        vec![
            ("root", PageVisibility::Current),
            ("root", PageVisibility::Visible),
            ("dialog", PageVisibility::Current),
        ],
    );

    // An OPAQUE push over both covers the root and demotes the dialog.
    {
        let seen = seen.clone();
        controller.push_with_options(
            || sized_page(100.0, 60.0),
            PushOptions::opaque().on_visibility(move |v| seen.borrow_mut().push(("b", v))),
        );
    }
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        seen.borrow()[3..],
        [
            ("root", PageVisibility::Covered),
            ("dialog", PageVisibility::Covered),
            ("b", PageVisibility::Current),
        ],
    );

    // Popping B reveals both again, in stack order.
    controller.pop();
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        seen.borrow()[6..],
        [
            ("root", PageVisibility::Visible),
            ("dialog", PageVisibility::Current),
        ],
    );

    // Popping the dialog restores the root to `Current`.
    controller.pop();
    root.rebuild(&mut app, &mut state);
    assert_eq!(seen.borrow()[8..], [("root", PageVisibility::Current)]);
    assert_eq!(seen.borrow().len(), 9, "no extra observations anywhere");
}

/// Count how many times the ROOT page's builder runs, with the covered-build
/// cull either on or off. Returns `(counter, root, app)` ready to drive.
#[allow(clippy::type_complexity)]
fn cull_harness(
    controller: &NavigatorController<()>,
    cull: bool,
) -> (
    Rc<Cell<u32>>,
    RenderRoot<(), NavigatorView<()>>,
    impl FnMut(&mut ()) -> NavigatorView<()>,
) {
    let builds = Rc::new(Cell::new(0u32));
    let root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let app = {
        let ctrl = controller.clone();
        let builds = builds.clone();
        move |_: &mut ()| {
            let builds = builds.clone();
            navigator(&ctrl, move || {
                builds.set(builds.get() + 1);
                sized_page(100.0, 100.0)
            })
            .cull_covered_builds(cull)
        }
    };
    (builds, root, app)
}

#[test]
fn cull_covered_builds_freezes_a_covered_page_and_resumes_on_the_reveal_frame() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let (builds, mut root, mut app) = cull_harness(&controller, true);
    let mut state = ();

    root.rebuild(&mut app, &mut state); // build seeds the root page
    root.rebuild(&mut app, &mut state);
    let before_cover = builds.get();

    // The covering push: the root learns it is `Covered` BEFORE the reconcile
    // loop, but still gets exactly one final rebuild on this frame (so a page
    // staging teardown UI on cover can render it).
    controller.push(|| sized_page(100.0, 60.0));
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        builds.get(),
        before_cover + 1,
        "the frame a page becomes covered still rebuilds it"
    );

    // From here the builder is frozen.
    let frozen = builds.get();
    root.rebuild(&mut app, &mut state);
    root.rebuild(&mut app, &mut state);
    root.rebuild(&mut app, &mut state);
    assert_eq!(builds.get(), frozen, "a covered page stops rebuilding");

    // The pop is applied before the reconcile loop, so the revealed page
    // rebuilds on the SAME frame — nothing has to wake it. `enqueue` also
    // raises the pending-flush mark on this `pop()` (see the type's doc),
    // which this same `root.rebuild` drains into one extra build +
    // view-diff pass once the ops are already applied and the page is
    // already revealed — so the now-uncovered root page rebuilds twice
    // within this one call, not once: the revealing pass itself, plus the
    // pending-flush convergence pass right behind it.
    controller.pop();
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        builds.get(),
        frozen + 2,
        "a revealed page rebuilds in the pass that revealed it, plus once \
         more in the same call's pending-flush convergence pass"
    );
    // No op queued this time, so no mark to converge — a single pass.
    root.rebuild(&mut app, &mut state);
    assert_eq!(builds.get(), frozen + 3, "and keeps rebuilding after that");
}

#[test]
fn cull_covered_builds_defaults_off() {
    let controller: NavigatorController<()> = NavigatorController::new();
    // Same harness, `false` — which is also what plain `navigator(...)` gives:
    // the shipped every-page-every-frame behaviour is unchanged.
    let (builds, mut root, mut app) = cull_harness(&controller, false);
    let mut state = ();

    root.rebuild(&mut app, &mut state);
    controller.push(|| sized_page(100.0, 60.0));
    root.rebuild(&mut app, &mut state);
    let covered_at = builds.get();
    root.rebuild(&mut app, &mut state);
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        builds.get(),
        covered_at + 2,
        "by default a covered page still reconciles every frame"
    );

    // And the default really is `false` on a bare navigator.
    let bare: NavigatorView<()> = navigator(&controller, || sized_page(1.0, 1.0));
    assert!(!bare.cull_covered_builds);
}

#[test]
fn a_covered_page_keeps_its_widget_state_under_the_build_cull() {
    // The "no cleanup on cover" contract, pinned against the opt-in cull:
    // freezing a page's builder must not dispose it.
    let controller: NavigatorController<()> = NavigatorController::new();
    let observed = Rc::new(Cell::new(0u32));
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        let obs = observed.clone();
        move |_: &mut ()| {
            let obs = obs.clone();
            navigator(&ctrl, move || counter_page(&obs)).cull_covered_builds(true)
        }
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);

    // Tap the root page → its retained counter is 1.
    root.event(&mut state, &down(5.0, 5.0));
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
    assert_eq!(observed.get(), 1);

    // Cover it for several frames, then reveal: the retained pod (and thus the
    // count) survived — a covering push never runs `on_cleanup`.
    controller.push(|| sized_page(100.0, 60.0));
    for _ in 0..4 {
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
    }
    controller.pop();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
    assert_eq!(
        observed.get(),
        1,
        "the covered page's widget state survived the build cull"
    );
}
