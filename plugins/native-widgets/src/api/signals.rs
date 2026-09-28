//! Events-as-signals: wraps a decoded [`EventPayload`] into the plain,
//! parameter-shaped callback an app supplies to a builder's `.on_press`/
//! `.on_toggle`/`.on_change`/`.on_select` — the seam
//! [`crate::runtime::NativeRuntime::set_callback`] invokes on the platform
//! main thread.
//!
//! Wrapping lives here, once per control kind, rather than inline in
//! `builders.rs`'s `Component::build` impls, so each one reads as
//! "encode params, register the callback" without repeating the
//! match-and-forward boilerplate.
//!
//! # Every closure runs on the platform main thread
//!
//! Android's JNI listener callbacks and iOS's target-action both fire on the
//! platform main thread — the same thread the rest of frust runs on — so an
//! app closure that writes an `RwSignal` (`move |v| sig.set(v)`, the blessed
//! idiom) wakes exactly one frust frame — `frust-reactive`'s signal-write
//! wake coalesces any number of writes between rebuilds into one
//! (`docs/ARCHITECTURE.md`'s Signal-driven wake).
//! `Send + Sync` is required only because [`EventCallback`]'s slot is typed
//! that way (shared with a notional future multi-thread event source, and
//! matching every other `Arc<dyn Fn(...) + Send + Sync>` this crate already
//! threads through `crate::runtime::Instance`) — no callback here is ever
//! actually invoked from more than one thread.

use std::sync::Arc;

use crate::events::EventPayload;

/// The callback shape [`crate::runtime::NativeRuntime::set_callback`] stores
/// per slot — one [`EventPayload`] in, nothing out (the callback's whole job
/// is deciding whether/what to write into a signal).
pub(super) type EventCallback = Arc<dyn Fn(EventPayload) + Send + Sync>;

/// Wrap a `Button`'s press handler: fires only on [`EventPayload::Click`] —
/// `Button::on_event` (`crate::controls::button`) never reports anything else.
pub(super) fn on_click(handler: Arc<dyn Fn() + Send + Sync>) -> EventCallback {
    Arc::new(move |payload| {
        if matches!(payload, EventPayload::Click) {
            handler();
        }
    })
}

/// Wrap a `Switch`'s toggle handler: fires only on [`EventPayload::Toggled`],
/// with the reported checked state. `Switch`'s own echo guard
/// (`crate::controls::switch`'s module doc) already drops the event a
/// controlled `update`'s own `Setter::Checked` would otherwise echo back, so
/// every firing here is a genuine user toggle.
pub(super) fn on_toggled(handler: Arc<dyn Fn(bool) + Send + Sync>) -> EventCallback {
    Arc::new(move |payload| {
        if let EventPayload::Toggled(checked) = payload {
            handler(checked);
        }
    })
}

/// Wrap a `Slider`'s change handler: fires only on
/// [`EventPayload::ValueChanged`], with the reported **app-space** value
/// (`crate::controls::slider`'s platform-space `min` mapping already undone
/// by the time it reaches [`EventPayload`]). `from_user` is not exposed here
/// — v1's minimum needs only the value; a future task can widen this if a
/// caller needs to distinguish a drag from a programmatic echo.
pub(super) fn on_value_changed(handler: Arc<dyn Fn(i32) + Send + Sync>) -> EventCallback {
    Arc::new(move |payload| {
        if let EventPayload::ValueChanged { value, .. } = payload {
            handler(value);
        }
    })
}

/// Wrap a segmented control's selection handler: fires only on
/// [`EventPayload::Selected`], with the **requested** segment index — the
/// app confirms it by feeding it back as `selected` (controlled, like
/// [`on_toggled`]). A platform "no segment" report never reaches here
/// (`crate::controls::segmented::decode_event` drops it).
pub(super) fn on_selected(handler: Arc<dyn Fn(usize) + Send + Sync>) -> EventCallback {
    Arc::new(move |payload| {
        if let EventPayload::Selected(index) = payload {
            handler(index);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn on_click_fires_only_for_click() {
        let seen = Arc::new(Mutex::new(0));
        let recorder = Arc::clone(&seen);
        let cb = on_click(Arc::new(move || *recorder.lock().unwrap() += 1));

        cb(EventPayload::Click);
        cb(EventPayload::Toggled(true));
        cb(EventPayload::DragStart);

        assert_eq!(*seen.lock().unwrap(), 1);
    }

    #[test]
    fn on_toggled_forwards_the_checked_state() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        let cb = on_toggled(Arc::new(move |checked| {
            recorder.lock().unwrap().push(checked)
        }));

        cb(EventPayload::Toggled(true));
        cb(EventPayload::Click);
        cb(EventPayload::Toggled(false));

        assert_eq!(*seen.lock().unwrap(), vec![true, false]);
    }

    #[test]
    fn on_value_changed_forwards_the_app_space_value_only() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        let cb = on_value_changed(Arc::new(move |value| recorder.lock().unwrap().push(value)));

        cb(EventPayload::ValueChanged {
            value: 42,
            from_user: true,
        });
        cb(EventPayload::DragEnd);

        assert_eq!(*seen.lock().unwrap(), vec![42]);
    }

    #[test]
    fn on_selected_forwards_the_requested_index_only() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        let cb = on_selected(Arc::new(move |index| recorder.lock().unwrap().push(index)));

        cb(EventPayload::Selected(2));
        cb(EventPayload::Toggled(true));
        cb(EventPayload::Click);
        cb(EventPayload::Selected(0));

        assert_eq!(*seen.lock().unwrap(), vec![2, 0]);
    }
}
