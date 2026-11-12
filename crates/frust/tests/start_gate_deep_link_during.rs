//! A cold-start deep link pushed while the gate loads wins, and later links still navigate.
#![cfg(feature = "test-support")]

use std::convert::Infallible;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use frust::authoring::Size;
use frust::testing::{Tester, TesterOptions, find};
use frust::{
    Component, Route, Router, RouterDeepLinks, StartGate, View, component, navigator,
    push_deep_link, router_with_deep_links, text,
};

const SIZE: Size = Size::new(400.0, 300.0);
const WAIT: Duration = Duration::from_secs(5);

/// The app root hosted inside the gate's ready view: it owns the router and tracks links.
struct AppRoot;

struct AppState {
    links: RouterDeepLinks<AppState>,
}

impl Component for AppRoot {
    type State = AppState;

    fn init(&self) -> AppState {
        let router: Router<AppState> = Router::new(vec![
            Route::new("/home", |_params| text("home page")),
            Route::new("/detail", |_params| text("detail page")),
        ]);
        AppState {
            links: router_with_deep_links(router, "/home"),
        }
    }

    fn build(&self, state: &mut AppState) -> impl View<AppState> {
        state.links.track();
        navigator(state.links.router().controller(), || text("fallback"))
    }
}

fn shows<S: 'static, V: View<S>>(tester: &Tester<S, V>, s: &str) -> bool {
    !tester.find(&find::text(s)).is_empty()
}

fn gate_tester(rx: Arc<Mutex<Option<mpsc::Receiver<()>>>>) -> Tester<(), impl View<()>> {
    Tester::new_with(SIZE, &TesterOptions::default(), move || {
        ((), move |_: &mut ()| {
            let rx = rx.clone();
            StartGate::new(move || {
                let gate = rx.lock().unwrap().take();
                async move {
                    if let Some(gate) = gate {
                        while gate.try_recv().is_err() {
                            frust::time::sleep(Duration::from_millis(5)).await;
                        }
                    }
                    Ok::<_, Infallible>(1_u32)
                }
            })
            .splash(text("splash"))
            .ready(|_: &u32| component(AppRoot))
        })
    })
}

#[test]
fn link_pushed_while_loading_is_the_first_route_and_later_links_navigate() {
    let (tx, rx) = mpsc::channel();
    let mut tester = gate_tester(Arc::new(Mutex::new(Some(rx))));
    tester.pump(Duration::from_millis(16));
    assert!(shows(&tester, "splash"));

    push_deep_link("/detail");
    tester.pump(Duration::from_millis(16));
    assert!(shows(&tester, "splash"));

    tx.send(()).unwrap();
    tester.pump_until("first route", WAIT, |t| {
        shows(t, "detail page") || shows(t, "home page")
    });
    assert!(shows(&tester, "detail page"));
    assert!(!shows(&tester, "home page"));

    push_deep_link("/home");
    tester.pump_until("warm link navigates", WAIT, |t| shows(t, "home page"));
    assert!(!shows(&tester, "detail page"));
}
