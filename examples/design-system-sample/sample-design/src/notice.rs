//! [`sample_notice`]: the design system's **kept-mounted overlay** shape — a
//! panel-wrapped message that fades in and out through `frust::motion::presence`.
//!
//! Beyond the three core authoring shapes, this one proves `presence` is usable
//! from an out-of-tree catalog with `frust` as the only dependency. While the
//! notice is closed its child stays built and laid out but is neither painted,
//! hit-tested, nor reported to accessibility; `on_exited` tells the owner when
//! an exit has settled.

use frust::motion::presence::{PresenceView, presence};
use frust::text;

use crate::panel::sample_panel;

/// Show `message` in a themed [`sample_panel`] while `open`, fading it in and
/// out. Chain `.on_exited(..)` (and `.timing(..)`) on the result.
pub fn sample_notice<State: 'static>(
    open: bool,
    message: impl Into<String>,
) -> PresenceView<State> {
    presence(open, sample_panel(text(message.into())))
}

#[cfg(test)]
mod tests {
    use frust::View;
    use frust::authoring::{BuildCtx, Size};

    use super::*;
    use crate::testing::{layout_widget, paint_widget};

    const MESSAGE: &str = "saved";

    fn has_message(open: bool) -> bool {
        let update = frust::testing::semantics_of(
            |_: &mut ()| sample_notice::<()>(open, MESSAGE),
            Size::new(400.0, 200.0),
        );
        update
            .nodes
            .iter()
            .any(|(_, node)| node.value() == Some(MESSAGE) || node.label() == Some(MESSAGE))
    }

    #[test]
    fn notice_semantics_disappear_when_closed() {
        assert!(has_message(true), "an open notice reports its text");
        assert!(!has_message(false), "a closed notice reports nothing");
    }

    fn painted(open: bool) -> usize {
        let view = sample_notice::<()>(open, MESSAGE);
        let mut counter = 0u64;
        let mut widget = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let size = layout_widget(&mut widget, None, Size::new(400.0, 200.0));
        let scene = paint_widget(&mut widget, size, None);
        scene.rects.len()
            + scene.rounded_rects.len()
            + scene.strokes.len()
            + scene.glyph_colors.len()
    }

    #[test]
    fn closed_notice_records_no_draw() {
        assert_eq!(painted(false), 0);
        assert!(painted(true) > 0, "the open control paints");
    }
}
