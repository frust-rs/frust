//! `NavigatorWidget::semantics` (R23) parity at the root overlay host: what
//! contributes no accessibility node is exactly what receives no input.

use super::super::*;
use super::support::*;
use std::cell::Cell;

/// **R23 parity at the root.** With an overlay up, the app root and its
/// chrome contribute NO accessibility nodes — asserted, not assumed, and
/// asserted *together with* the input reach so the two can only ever agree.
/// A root overlay that dimmed the chrome visually while a screen reader still
/// read it out would re-create the reachable-but-inert chrome bug inside the
/// accessibility tree.
#[test]
fn an_overlay_host_omits_the_app_root_and_its_chrome_from_semantics() {
    let (controller, mut root, mut app, content_hits, chrome_hits) = overlay_host_fixture();
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // With no overlay, the app root IS the routed page: both reaches carry
    // the content and the chrome (the R23 parity guard, at the root).
    let labels = semantic_labels(&root);
    assert!(
        labels.iter().any(|l| l == "app-content") && labels.iter().any(|l| l == "app-chrome"),
        "no overlay: the whole app root is in the accessibility tree ({labels:?})"
    );

    let overlay_hits = Rc::new(Cell::new(0u32));
    controller.push_with_options(
        {
            let overlay_hits = overlay_hits.clone();
            move || host_probe("overlay", &overlay_hits)
        },
        PushOptions::transparent(),
    );
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // The accessibility reach…
    let labels = semantic_labels(&root);
    assert!(
        labels.iter().any(|l| l == "overlay"),
        "the overlay itself is present ({labels:?})"
    );
    assert!(
        !labels.iter().any(|l| l == "app-content") && !labels.iter().any(|l| l == "app-chrome"),
        "R23: the app root and its chrome contribute nothing under an \
         overlay ({labels:?})"
    );

    // …equals the input reach, measured the same way.
    content_hits.set(0);
    chrome_hits.set(0);
    root.event(&mut state, &down(50.0, 50.0));
    assert_eq!(overlay_hits.get(), 1);
    assert_eq!(
        (content_hits.get(), chrome_hits.get()),
        (0, 0),
        "R23 parity: what contributes no node is exactly what receives no input"
    );
}
