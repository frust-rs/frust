//! Auth session section: the `frust-auth-session` plugin's device-gate page
//! — `plugins/auth-session`'s whole vertical slice (Android Custom Tabs /
//! Apple `ASWebAuthenticationSession`) exercised from real button presses
//! against the committed static pages under `plugins/auth-session/gate`
//! (see that directory's own README for how to host them and the zero-infra
//! httpbin alternative).
//!
//! # The four buttons
//!
//! - **Callback** starts an [`AuthSession`] against `<base>callback.html?
//!   scheme=frustplay` — `callback.html` immediately redirects to
//!   `frustplay://auth/callback?code=x&state=gate`, so this is the
//!   successful round trip. The status line below prints the callback URL
//!   in full — read through the [`AuthSessionOutcome::Callback`] value,
//!   since the plugin's own `Debug` impls deliberately hide URLs — because
//!   that is what the scripted gate compares against (see [`render`]).
//! - **Cancel test** starts a session against `<base>cookie.html` (any page
//!   works — the tester manually dismisses the in-app browser tab without
//!   the identity provider ever redirecting) to exercise
//!   [`AuthSessionOutcome::Cancelled`].
//! - **Busy** fires two [`AuthSession::start`] calls back-to-back, spawning
//!   the first without awaiting it and immediately awaiting the second —
//!   `AuthSession::start`'s crate doc's *Exactly one live session* section
//!   means the second resolves to [`AuthSessionError::Busy`] the instant
//!   it's called (the `ACTIVE` slot is claimed synchronously inside
//!   `start`, not on first poll), so the status line updates with that
//!   result right away.
//! - **Cookie check** starts a session against `<base>cookie.html` too —
//!   `cookie.html` sets (or reads back) a test cookie and prints it, so a
//!   tester can confirm cookies persist across a non-ephemeral session and
//!   do not across an ephemeral one (the `Ephemeral` toggle below).
//!
//! The `Base URL` field defaults to `https://frust.dev/gate/auth-session/`
//! (editable so a runner can point it at wherever the gate pages are
//! actually hosted, or at the httpbin alternative the gate README
//! documents) and every button appends its own page name to it.

use frust::{
    AnyView, Axis, ButtonStyle, Color, EdgeInsets, FlexChild, FlexView, Get, GetUntracked, Padding,
    RwSignal, Set, SizedBox, Theme, any, button, checkbox, deep_links, inflexible, spawn_local,
    text, text_input, use_context,
};
use frust_auth_session::{AuthSession, AuthSessionError, AuthSessionOutcome, AuthSessionRequest};

use crate::PlaygroundState;

/// Defines a `fn $name() -> RwSignal<$ty>` returning a screen-local signal
/// cached in a `thread_local!`, self-healing across a disposed owner — the
/// established per-module precedent (`url_launcher.rs`, `platform_views.rs`,
/// `native_widgets.rs` each carry the same macro), not shared across files.
macro_rules! local_sig {
    ($name:ident, $ty:ty, $init:expr) => {
        fn $name() -> RwSignal<$ty> {
            thread_local! {
                static SLOT: std::cell::RefCell<Option<RwSignal<$ty>>> =
                    const { std::cell::RefCell::new(None) };
            }
            SLOT.with(|cell| {
                if let Some(sig) = *cell.borrow()
                    && sig.try_get_untracked().is_some()
                {
                    return sig;
                }
                let sig = RwSignal::new($init);
                *cell.borrow_mut() = Some(sig);
                sig
            })
        }
    };
}

// The gate pages' https origin, editable so a runner can point it at
// wherever `plugins/auth-session/gate`'s two pages are actually hosted (see
// that directory's own README), or at the zero-infra httpbin alternative it
// also documents.
local_sig!(
    base_url_sig,
    String,
    "https://frust.dev/gate/auth-session/".to_string()
);

// `AuthSessionRequest::ephemeral` — off by default, matching that field's
// own default meaning (a persisted browsing session).
local_sig!(ephemeral_sig, bool, false);

// The last button press's `Debug`-formatted `AuthSession::start` result —
// empty until the first attempt.
local_sig!(status_sig, String, String::new());

/// The callback-scheme registered on Android
/// (`android/app/src/main/AndroidManifest.xml`) and iOS
/// (`ios/Runner/Info.plist`) — the same `frustplay` scheme
/// `pages/url_launcher.rs`'s round trip already exercises.
const CALLBACK_SCHEME: &str = "frustplay";

/// Live-theme accent-text role (`primary`), falling back to the Material
/// baseline pre-context — the same pattern every other section page uses.
fn accent() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .primary
}

/// A muted caption ink.
fn muted() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .on_surface_variant
}

/// A fixed-width horizontal spacer between two buttons.
fn gap_h(w: f64) -> FlexChild<PlaygroundState> {
    inflexible(SizedBox(Some(w), None))
}

/// Build a request against `<base><path>` using the current [`base_url_sig`]
/// / [`ephemeral_sig`] snapshot — read with `get_untracked` since a request
/// is assembled inside an event handler, not `build` (`docs/CODE_STANDARDS.md`'s
/// Reactivity rule).
fn request_for(path: &str) -> AuthSessionRequest {
    let base = base_url_sig().get_untracked();
    let ephemeral = ephemeral_sig().get_untracked();
    AuthSessionRequest {
        url: format!("{base}{path}"),
        callback_scheme: CALLBACK_SCHEME.to_string(),
        ephemeral,
    }
}

/// Render a session outcome for the status label. The plugin's `Debug` for
/// [`AuthSessionOutcome::Callback`] deliberately hides the URL (it prints
/// `Callback { url_len: N }`), but this page is the device gate and the gate
/// compares the callback URL verbatim, so the URL is read through the value
/// and printed in full here; every other outcome keeps its `Debug` form.
fn render(result: &Result<AuthSessionOutcome, AuthSessionError>) -> String {
    match result {
        Ok(AuthSessionOutcome::Callback(url)) => format!("Ok(Callback({url:?}))"),
        other => format!("{other:?}"),
    }
}

/// Start a session against `<base><path>`, writing `"{label} -> <outcome>"`
/// (see [`render`]) into [`status_sig`] once it resolves — the handler every
/// button except `Busy` uses (see this module doc's *The four buttons*).
fn fire(label: &'static str, path: &'static str) -> impl Fn(&mut PlaygroundState) + 'static {
    move |_state: &mut PlaygroundState| {
        let req = request_for(path);
        spawn_local(async move {
            let result = AuthSession::start(req).await;
            status_sig().set(format!("{label} -> {}", render(&result)));
        });
    }
}

/// `Busy`: fire two `start` calls back-to-back, spawning the first without
/// awaiting it (so a real backend still gets to present its session UI) and
/// awaiting the second — which this crate doc's *Exactly one live session*
/// section guarantees resolves to [`AuthSessionError::Busy`](frust_auth_session::AuthSessionError::Busy)
/// immediately, since the `ACTIVE` slot is claimed synchronously inside
/// `start` itself rather than on first poll.
fn fire_busy(_state: &mut PlaygroundState) {
    let first = AuthSession::start(request_for("callback.html?scheme=frustplay"));
    let second = AuthSession::start(request_for("callback.html?scheme=frustplay"));
    spawn_local(async move {
        let _ = first.await;
    });
    spawn_local(async move {
        let result = second.await;
        status_sig().set(format!("Busy -> {}", render(&result)));
    });
}

/// See the page-fn contract in [`crate::pages`]. Reads no
/// [`PlaygroundState`] signal — the call status, base URL and ephemeral
/// toggle live in this module's own screen-local signals, and the deep-link
/// readout comes straight from [`frust::deep_links`].
pub fn page(_state: &PlaygroundState) -> AnyView<PlaygroundState> {
    let base_url = base_url_sig().get();
    let ephemeral = ephemeral_sig().get();
    let status = status_sig().get();
    // Tracked read: subscribes this rebuild to `latest`, so the Android
    // double-delivery this module doc's crate-doc cross-reference describes
    // (the same callback URL also lands as an ordinary deep link) repaints
    // live — the same "track the signal you read" contract `url_launcher.rs`
    // observes.
    let latest = deep_links().latest.get();
    let latest_line = match latest {
        Some(link) => format!("latest deep link: {}", link.url),
        None => "latest deep link: none".to_string(),
    };
    let status_line = if status.is_empty() {
        "status: (no attempt yet)".to_string()
    } else {
        format!("status: {status}")
    };
    let supported_line = format!("is_supported: {}", AuthSession::is_supported());

    let children: Vec<AnyView<PlaygroundState>> = vec![
        any(text("Auth session").size(13.0).color(accent())),
        any(text(
            "frust-auth-session's AuthSession::start, exercised from real button presses \
                 against the committed gate pages (plugins/auth-session/gate). Callback proves \
                 the successful round trip; Cancel test and Cookie check both open cookie.html \
                 for a manually-dismissed session; Busy proves the process-wide single-session \
                 guard.",
        )
        .size(11.0)
        .color(muted())),
        any(SizedBox(None, Some(12.0))),
        any(text_input(base_url, |_state: &mut PlaygroundState, value| {
            base_url_sig().set(value);
        })
        .placeholder("https://frust.dev/gate/auth-session/")),
        any(SizedBox(None, Some(8.0))),
        any(checkbox(
            ephemeral,
            "Ephemeral",
            |_state: &mut PlaygroundState, requested: bool| {
                ephemeral_sig().set(requested);
            },
        )),
        any(SizedBox(None, Some(12.0))),
        any(FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(any(button(
                    "Callback",
                    fire("Callback", "callback.html?scheme=frustplay"),
                )
                .style(ButtonStyle::Primary)
                .small())),
                gap_h(8.0),
                inflexible(any(button(
                    "Cancel test",
                    fire("Cancel test", "cookie.html"),
                )
                .style(ButtonStyle::Secondary)
                .small())),
            ],
        )),
        any(SizedBox(None, Some(8.0))),
        any(FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(any(button("Busy", fire_busy)
                    .style(ButtonStyle::Secondary)
                    .small())),
                gap_h(8.0),
                inflexible(any(button(
                    "Cookie check",
                    fire("Cookie check", "cookie.html"),
                )
                .style(ButtonStyle::Secondary)
                .small())),
            ],
        )),
        any(SizedBox(None, Some(8.0))),
        any(text(status_line).size(12.0)),
        any(SizedBox(None, Some(12.0))),
        any(text(latest_line).size(12.0).color(accent())),
        any(SizedBox(None, Some(4.0))),
        any(text(supported_line).size(11.0).color(muted())),
    ];

    any(Padding(
        EdgeInsets::all(16.0),
        FlexView::new(
            Axis::Vertical,
            children.into_iter().map(inflexible).collect(),
        ),
    ))
}
