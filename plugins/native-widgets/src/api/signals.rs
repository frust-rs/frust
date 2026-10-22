//! Events-as-signals: wraps a decoded [`EventPayload`] into the plain,
//! parameter-shaped callback an app supplies to a builder's `.on_press`/
//! `.on_toggle`/`.on_change`/`.on_select` (a date picker's `.on_change` and a
//! tab bar's `.on_select`/`.on_reselect` included) — the seam
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

use super::builders::TabId;
use crate::chrome::{ActionId, ChromeEvent, ChromeSink, MenuItemId};
use crate::controls::date_picker::CivilDate;
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

/// Wrap a date picker's change handler: fires only on
/// [`EventPayload::Date`], with the **requested** civil date — the app
/// confirms it by feeding it back as `date` (controlled, like
/// [`on_toggled`]). A report that is not a real date never reaches here
/// (`crate::events::unpack_date` refuses it).
pub(super) fn on_date(handler: Arc<dyn Fn(CivilDate) + Send + Sync>) -> EventCallback {
    Arc::new(move |payload| {
        if let EventPayload::Date(date) = payload {
            handler(date);
        }
    })
}

/// An optional `Fn(TabId)` handler, as a tab bar builder stores it.
pub(super) type TabHandler = Option<Arc<dyn Fn(TabId) + Send + Sync>>;

/// Wrap a tab bar's two handlers: [`EventPayload::Selected`] fires
/// `on_select` and [`EventPayload::Reselected`] fires `on_reselect`, each with
/// the `TabId` of the item at the reported index in `ids` — the ids of the
/// items the builder published alongside this callback (both are rebuilt
/// together every build, so the index and the list agree). An index past
/// `ids` (a report racing an item change) fires nothing; so does every other
/// payload. Selection is **requested**, never applied: the app confirms it by
/// feeding the id back as `selected` (controlled, like [`on_selected`]).
pub(super) fn on_tab_bar(
    ids: Arc<[TabId]>,
    on_select: TabHandler,
    on_reselect: TabHandler,
) -> EventCallback {
    Arc::new(move |payload| {
        let (handler, index) = match payload {
            EventPayload::Selected(index) => (&on_select, index),
            EventPayload::Reselected(index) => (&on_reselect, index),
            _ => return,
        };
        if let (Some(handler), Some(id)) = (handler, ids.get(index)) {
            handler(id.clone());
        }
    })
}

/// An optional `Fn(ActionId)` chrome handler.
pub(super) type ActionHandler = Option<Arc<dyn Fn(ActionId) + Send + Sync>>;

/// The typed handlers a `native_scaffold` routes [`ChromeEvent`]s to, plus the
/// catch-all every event also reaches (the `events_into` signal form).
#[derive(Clone, Default)]
pub(super) struct ChromeHandlers {
    pub(super) on_tab_select: TabHandler,
    pub(super) on_tab_reselect: TabHandler,
    pub(super) on_action: ActionHandler,
    pub(super) on_menu: Option<Arc<dyn Fn(ActionId, MenuItemId) + Send + Sync>>,
    pub(super) on_back: Option<Arc<dyn Fn() + Send + Sync>>,
    pub(super) on_event: Option<Arc<dyn Fn(ChromeEvent) + Send + Sync>>,
}

/// Wrap the scaffold's handlers into the sink `crate::chrome::set_sink`
/// stores: each [`ChromeEvent`] variant reaches its own typed handler, and
/// every event also reaches `on_event`. Selection is **requested**, never
/// applied — the app confirms it by feeding `selected` back (controlled).
pub(super) fn on_chrome(handlers: ChromeHandlers) -> ChromeSink {
    Arc::new(move |event| {
        if let Some(all) = &handlers.on_event {
            all(event.clone());
        }
        match event {
            ChromeEvent::TabSelected(id) => {
                if let Some(handler) = &handlers.on_tab_select {
                    handler(id);
                }
            }
            ChromeEvent::TabReselected(id) => {
                if let Some(handler) = &handlers.on_tab_reselect {
                    handler(id);
                }
            }
            ChromeEvent::ActionTapped(id) => {
                if let Some(handler) = &handlers.on_action {
                    handler(id);
                }
            }
            ChromeEvent::MenuItemChosen(action, item) => {
                if let Some(handler) = &handlers.on_menu {
                    handler(action, item);
                }
            }
            ChromeEvent::BackPressed => {
                if let Some(handler) = &handlers.on_back {
                    handler();
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn on_tab_bar_maps_indices_to_ids_and_splits_select_from_reselect() {
        let selected = Arc::new(Mutex::new(Vec::new()));
        let reselected = Arc::new(Mutex::new(Vec::new()));
        let (sel, resel) = (Arc::clone(&selected), Arc::clone(&reselected));
        let ids: Arc<[TabId]> = vec![TabId::new("home"), TabId::new("inbox")].into();
        let cb = on_tab_bar(
            ids,
            Some(Arc::new(move |id: TabId| sel.lock().unwrap().push(id))),
            Some(Arc::new(move |id: TabId| resel.lock().unwrap().push(id))),
        );

        cb(EventPayload::Selected(1));
        cb(EventPayload::Reselected(0));
        cb(EventPayload::Selected(2)); // past the ids: nothing
        cb(EventPayload::Click);

        assert_eq!(*selected.lock().unwrap(), vec![TabId::new("inbox")]);
        assert_eq!(*reselected.lock().unwrap(), vec![TabId::new("home")]);
    }

    #[test]
    fn on_tab_bar_without_a_reselect_handler_ignores_reselects() {
        let selected = Arc::new(Mutex::new(Vec::new()));
        let sel = Arc::clone(&selected);
        let cb = on_tab_bar(
            vec![TabId::new("a")].into(),
            Some(Arc::new(move |id: TabId| sel.lock().unwrap().push(id))),
            None,
        );
        cb(EventPayload::Reselected(0));
        assert!(selected.lock().unwrap().is_empty());
    }

    #[test]
    fn on_chrome_routes_each_variant_to_its_handler_and_every_event_to_the_catch_all() {
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        let push = |tag: &'static str| {
            let log = Arc::clone(&log);
            move |detail: String| log.lock().unwrap().push(format!("{tag}:{detail}"))
        };
        let (select, reselect, action, menu, back, all) = (
            push("select"),
            push("reselect"),
            push("action"),
            push("menu"),
            push("back"),
            push("all"),
        );
        let sink = on_chrome(ChromeHandlers {
            on_tab_select: Some(Arc::new(move |id| select(id.to_string()))),
            on_tab_reselect: Some(Arc::new(move |id| reselect(id.to_string()))),
            on_action: Some(Arc::new(move |id| action(id.to_string()))),
            on_menu: Some(Arc::new(move |a, m| menu(format!("{a}/{m}")))),
            on_back: Some(Arc::new(move || back(String::new()))),
            on_event: Some(Arc::new(move |event| all(format!("{event:?}")))),
        });
        sink(ChromeEvent::TabSelected(TabId::new("a")));
        sink(ChromeEvent::TabReselected(TabId::new("b")));
        sink(ChromeEvent::ActionTapped("add".into()));
        sink(ChromeEvent::MenuItemChosen("more".into(), "share".into()));
        sink(ChromeEvent::BackPressed);
        let log = log.lock().unwrap();
        for expected in [
            "select:a",
            "reselect:b",
            "action:add",
            "menu:more/share",
            "back:",
        ] {
            assert!(log.iter().any(|line| line == expected), "{expected}");
        }
        assert_eq!(log.iter().filter(|l| l.starts_with("all:")).count(), 5);
    }

    #[test]
    fn on_chrome_without_handlers_drops_events() {
        on_chrome(ChromeHandlers::default())(ChromeEvent::BackPressed);
    }

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

    #[test]
    fn on_date_forwards_the_requested_date_only() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        let cb = on_date(Arc::new(move |date| recorder.lock().unwrap().push(date)));

        let picked = CivilDate::new(2026, 10, 3).unwrap();
        cb(EventPayload::Date(picked));
        cb(EventPayload::Selected(1));
        cb(EventPayload::ValueChanged {
            value: 3,
            from_user: true,
        });
        cb(EventPayload::Date(CivilDate::MIN));

        assert_eq!(*seen.lock().unwrap(), vec![picked, CivilDate::MIN]);
    }
}
