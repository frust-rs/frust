//! A `ScrollController` write wakes an idle desktop loop: the pending-result flag it raises is
//! only read on a frame that already runs, so the write also asks for one. It asks on the
//! empty-to-queued edge only, so a burst of writes (or frames that apply the queue) posts a
//! single wake and never one per frame.
//!
//! The repaint hook is process-global, so this file holds exactly one test.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use frust_core::{
    AnyView, DiscardScene, FrameTime, RenderRoot, any, repaint_generation, set_repaint_hook,
    take_repaint_owed,
};
use frust_text::TextContext;
use frust_widgets::{Column, ScrollController, SizedBox, scroll_view};
use kurbo::Size;

const WINDOW: Size = Size::new(300.0, 200.0);

#[test]
fn a_controller_write_requests_one_repaint_per_queued_batch() {
    let wakes = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&wakes);
    let previous = set_repaint_hook(Some(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
    })));

    let controller = ScrollController::new();
    let handle = controller.clone();
    let mut logic = move |_: &mut ()| -> AnyView<()> {
        any(scroll_view(Column(
            (0..20)
                .map(|_| any(SizedBox::<()>(None, Some(50.0))))
                .collect::<Vec<_>>(),
        ))
        .controller(handle.clone()))
    };
    let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
    let mut tcx = TextContext::new();
    let mut ms = 0.0;
    let mut frame = |root: &mut RenderRoot<(), AnyView<()>>| {
        root.rebuild(&mut logic, &mut ());
        root.layout_with_text(WINDOW, &mut tcx);
        ms += 16.0;
        root.paint(
            &mut DiscardScene,
            FrameTime::from_nanos((ms * 1_000_000.0) as u64),
        );
    };
    for _ in 0..3 {
        frame(&mut root);
    }
    assert!(controller.is_attached());
    take_repaint_owed();
    wakes.store(0, Ordering::SeqCst);
    let generation = repaint_generation();

    // A write from outside any frame requests exactly one repaint.
    controller.jump_to(40.0);
    assert_eq!(wakes.load(Ordering::SeqCst), 1, "the write woke the loop");
    assert_eq!(repaint_generation(), generation + 1);

    // More writes before the queue drains add nothing: no request per write.
    controller.jump_to(80.0);
    assert_eq!(
        repaint_generation(),
        generation + 1,
        "a queued write is silent"
    );

    // The frames apply the queue and request nothing of their own, however many run.
    take_repaint_owed();
    for _ in 0..5 {
        frame(&mut root);
    }
    assert_eq!(controller.offset(), 80.0, "the write was applied");
    assert_eq!(
        repaint_generation(),
        generation + 1,
        "applying the queue asks for no frame"
    );
    assert_eq!(wakes.load(Ordering::SeqCst), 1);

    // A later write from outside a frame wakes the loop again.
    controller.jump_to(0.0);
    assert_eq!(wakes.load(Ordering::SeqCst), 2);

    set_repaint_hook(previous);
}
