//! Auto-dismiss toasts (the status-bar layer): transient
//! info/success/warn/error notices that stack (capped, drop-oldest) and expire
//! on the frame tick — no timers outside the tick loop.
//!
//! Everything here is plain data + pure transitions: [`Toasts::push`] adds one
//! (capped at [`Toasts::MAX`], dropping the oldest), and [`Toasts::tick`]
//! ages every live toast one tick and drops the expired ones. The runner's
//! frame interval drives `tick` only while [`AppState::animating`] reports live
//! toasts (see `crate::runner`), so an idle workbench takes no clock reads and
//! never redraws for an empty toast stack (the dirty-frame skip). `crate::ui`
//! renders the stack; it never mutates the model.
//!
//! [`AppState::animating`]: super::state::AppState::animating

/// The severity of a toast, keyed to a theme token by `crate::ui`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    /// Neutral information (accent/muted).
    Info,
    /// A successful action (green).
    Success,
    /// A non-fatal warning (amber).
    Warn,
    /// An error (red).
    Error,
}

/// One transient notice: its severity, text, and remaining life in ticks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    /// The severity (drives the glyph/color).
    pub kind: ToastKind,
    /// The notice text.
    pub text: String,
    /// Remaining life, in frame ticks; reaches 0 and the toast is dropped.
    pub ttl: u32,
}

/// The bounded, drop-oldest toast stack. Newest is last.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Toasts {
    /// Live toasts, oldest first.
    pub items: Vec<Toast>,
}

impl Toasts {
    /// The most toasts shown at once; a push past this drops the oldest.
    pub const MAX: usize = 3;

    /// Default life in ticks. The runner's tick cadence is 50 ms
    /// (`crate::runner::TICK`), so ~80 ticks ≈ 4 s — long enough to read a
    /// short notice, short enough not to linger.
    pub const DEFAULT_TTL: u32 = 80;

    /// Push a toast onto the stack (newest last), dropping the oldest while over
    /// [`Self::MAX`].
    pub fn push(&mut self, kind: ToastKind, text: impl Into<String>) {
        self.items.push(Toast {
            kind,
            text: text.into(),
            ttl: Self::DEFAULT_TTL,
        });
        while self.items.len() > Self::MAX {
            self.items.remove(0);
        }
    }

    /// Age every toast one tick and drop the expired ones. Returns whether the
    /// visible set changed (a toast disappeared) — the only case the frame must
    /// redraw for, since a toast's content is static between spawn and expiry.
    pub fn tick(&mut self) -> bool {
        for t in &mut self.items {
            t.ttl = t.ttl.saturating_sub(1);
        }
        let before = self.items.len();
        self.items.retain(|t| t.ttl > 0);
        self.items.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_caps_at_max_dropping_the_oldest() {
        let mut toasts = Toasts::default();
        for i in 0..(Toasts::MAX + 2) {
            toasts.push(ToastKind::Info, format!("toast {i}"));
        }
        assert_eq!(toasts.items.len(), Toasts::MAX);
        // The two oldest were dropped; the newest survives at the end.
        assert_eq!(toasts.items.last().unwrap().text, "toast 4");
        assert_eq!(toasts.items.first().unwrap().text, "toast 2");
    }

    #[test]
    fn tick_ages_and_drops_only_reporting_change_on_removal() {
        let mut toasts = Toasts::default();
        toasts.push(ToastKind::Success, "hi");
        toasts.items[0].ttl = 2;
        // First tick: 2 -> 1, nothing removed.
        assert!(!toasts.tick());
        assert_eq!(toasts.items.len(), 1);
        // Second tick: 1 -> 0, removed, change reported.
        assert!(toasts.tick());
        assert!(toasts.items.is_empty());
        // A tick on an empty stack is a no-op with no change.
        assert!(!toasts.tick());
    }
}
