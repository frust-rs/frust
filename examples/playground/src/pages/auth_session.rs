//! Auth session section: the `frust-auth-session` plugin's device-gate page
//! — `plugins/auth-session`'s whole vertical slice (Android Custom Tabs /
//! Apple `ASWebAuthenticationSession`, plus the desktop
//! [`LoopbackSession`]) exercised from real button presses against the
//! committed static pages under `plugins/auth-session/gate` (see that
//! directory's own README for how to host them and the zero-infra httpbin
//! alternative).
//!
//! # The four custom-scheme buttons
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
//!
//! # Desktop loopback
//!
//! Below the four buttons above, a second block exercises
//! [`LoopbackSession`] (RFC 8252 §7.3) plus `frust-oauth-native`'s PKCE/
//! `state`/callback-validation/token-grant-body builders — together the
//! desktop gate vehicle this crate pair ships for Linux, Windows and macOS
//! — against the same gate directory's `loopback.html`, a fake
//! authorization server driven entirely by query parameters and button
//! presses, never a real identity provider. Shown only when
//! [`LoopbackSession::is_supported`]; every other target shows one line
//! instead. Seven buttons:
//!
//! - **Loopback login** binds a session, builds the authorization URL
//!   against `<base>loopback.html?gate_iss=<issuer>` with fresh PKCE/
//!   `state`, starts it, validates the callback with
//!   [`IssuerCheck::Required`] and builds (never sends) the token-grant
//!   body — the whole round trip with no real HTTP anywhere.
//! - **Loopback deny** is the same flow with `gate_mode=deny` appended to
//!   the endpoint, so the gate page's redirect carries
//!   `error=access_denied` instead of a code — a typed
//!   `Authorization { error: AccessDenied, .. }` from
//!   [`parse_callback`].
//! - **Loopback bad iss** runs the plain success flow but validates the
//!   callback against `<issuer>/wrong` — a negative control for
//!   `parse_callback`'s `IssuerMismatch`.
//! - **Loopback timeout (10 s)** binds with a 10-second
//!   [`LoopbackOptions::timeout`] and `gate_mode=stay`, so the gate page
//!   never redirects — the listener's own [`AuthSessionError::TimedOut`]
//!   exit path.
//! - **Loopback cancel** binds with `gate_mode=stay` too and stashes the
//!   session's [`LoopbackCancel`] in a screen-local slot; the separate
//!   **Cancel loopback** button reads that slot and calls
//!   [`LoopbackCancel::cancel`], resolving the still-waiting session
//!   `Ok(Cancelled)`.
//! - **Loopback busy** binds one session, keeps it alive while binding a
//!   second — [`LoopbackSession::bind`] claims the shared slot
//!   synchronously, so the second bind's [`AuthSessionError::Busy`] needs
//!   no `await` at all. The first session is dropped (freeing the slot)
//!   once the handler returns.
//!
//! A status line below the loopback buttons reports the bound session's
//! port only (never the full redirect URI, though neither is secret) —
//! `loopback port: none` until the first bind.

use std::time::Duration;

use frust::{
    AnyView, Axis, ButtonStyle, Color, EdgeInsets, FlexChild, FlexView, Get, GetUntracked, Padding,
    RwSignal, Set, SizedBox, Theme, View, any, button, checkbox, deep_links, inflexible, row,
    spawn_local, text, text_input, use_context,
};
use frust_auth_session::{
    AuthSession, AuthSessionError, AuthSessionOutcome, AuthSessionRequest,
    DEFAULT_LOOPBACK_TIMEOUT, LoopbackCancel, LoopbackOptions, LoopbackSession,
};
use frust_oauth_native::{
    AuthorizationRequest, CallbackExpectations, IssuerCheck, PkceVerifier, State,
    authorization_code_grant, parse_callback,
};

use crate::PlaygroundState;

/// Defines a `fn $name() -> RwSignal<$ty>` returning a screen-local signal
/// cached in a `thread_local!`, self-healing across a disposed owner — the
/// established per-module precedent (`url_launcher.rs`, `platform_views.rs`,
/// the demo app's `composite.rs` page each carry the same macro), not shared
/// across files.
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
// wherever `plugins/auth-session/gate`'s pages are actually hosted (see
// that directory's own README), or at the zero-infra httpbin alternative it
// also documents. Also the source of the desktop loopback block's
// `issuer` — this field's value with its trailing `/` trimmed.
local_sig!(
    base_url_sig,
    String,
    "https://frust.dev/gate/auth-session/".to_string()
);

// `AuthSessionRequest::ephemeral` — off by default, matching that field's
// own default meaning (a persisted browsing session).
local_sig!(ephemeral_sig, bool, false);

// The last button press's `AuthSession::start` result as `render` prints it
// (the callback URL through the value, since `Debug` hides it) — empty until
// the first attempt. Shared by the loopback buttons below (see
// `render_loopback`), so the whole page carries one status line.
local_sig!(status_sig, String, String::new());

// The `LoopbackCancel` handle for the in-flight "Loopback cancel" session,
// if any — the separate "Cancel loopback" button reads this slot rather
// than holding the handle itself, since the two presses are different event
// handlers. `None` until the first cancel-eligible bind, and calling
// `cancel()` on a handle whose session has already resolved is a documented
// no-op (`LoopbackCancel::cancel`'s own doc), not an error.
local_sig!(loopback_cancel_sig, Option<LoopbackCancel>, None);

// The port of the most recently bound `LoopbackSession`, read back from its
// `redirect_uri()` — `None` until the first loopback bind. Shown in its own
// status line, port only (see this module's doc).
local_sig!(loopback_port_sig, Option<u16>, None);

/// The callback-scheme registered on Android
/// (`android/app/src/main/AndroidManifest.xml`) and iOS
/// (`ios/Runner/Info.plist`) — the same `frustplay` scheme
/// `pages/url_launcher.rs`'s round trip already exercises.
const CALLBACK_SCHEME: &str = "frustplay";

/// The reserved loopback redirect path every "Loopback …" button below
/// binds — a fixed demo value, not something a runner needs to edit.
const LOOPBACK_PATH: &str = "/oauth/callback";

/// The `client_id` every "Loopback …" button sends — this page is not a
/// registered OAuth client of anything real, so a fixed demo value is all
/// the gate page (which never checks it) needs.
const LOOPBACK_CLIENT_ID: &str = "frust-playground";

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
/// button except `Busy` uses (see this module doc's *The four custom-scheme
/// buttons*).
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

/// Percent-encode `value` for a URI query value — RFC 3986 unreserved bytes
/// (`ALPHA DIGIT - . _ ~`) pass through, everything else becomes an
/// uppercase `%XX` per byte. `frust-oauth-native`'s own equivalent
/// (`encode_component`) is crate-private, so this page carries this minimal
/// copy for the one parameter (`gate_iss`) it adds to the authorization
/// endpoint's own query before [`AuthorizationRequest::build`] appends its
/// owned ones.
fn encode_query_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Extract the port from a loopback `redirect_uri()` string
/// (`http://127.0.0.1:<port><path>`), for the port-only status line this
/// module doc describes — never the full redirect URI.
fn port_from_redirect_uri(redirect_uri: &str) -> Option<u16> {
    let rest = redirect_uri.strip_prefix("http://127.0.0.1:")?;
    let end = rest.find('/')?;
    rest[..end].parse().ok()
}

/// Bind a desktop [`LoopbackSession`], build the authorization URL against
/// `plugins/auth-session/gate/loopback.html` (appending `gate_mode` to the
/// endpoint's own query when given), start it, and write
/// `"{label} -> <outcome>"` into [`status_sig`] once it resolves — the
/// handler every "Loopback …" button except `Loopback busy` and `Cancel
/// loopback` uses (see this module doc's *Desktop loopback* section).
///
/// `expect_bad_iss` validates the callback against `<issuer>/wrong` instead
/// of the real issuer — the negative control for `Loopback bad iss`.
/// `stash_cancel` writes the session's [`LoopbackCancel`] into
/// [`loopback_cancel_sig`] before starting it, for the separate `Cancel
/// loopback` button to read.
fn fire_loopback(
    label: &'static str,
    gate_mode: Option<&'static str>,
    timeout: Duration,
    expect_bad_iss: bool,
    stash_cancel: bool,
) -> impl Fn(&mut PlaygroundState) + 'static {
    move |_state: &mut PlaygroundState| {
        let base = base_url_sig().get_untracked();
        let issuer = base.trim_end_matches('/').to_string();

        let session = match LoopbackSession::bind(LoopbackOptions {
            path: LOOPBACK_PATH.to_string(),
            timeout,
        }) {
            Ok(session) => session,
            Err(err) => {
                status_sig().set(format!("{label} -> Err({err:?})"));
                return;
            }
        };
        if stash_cancel {
            loopback_cancel_sig().set(Some(session.cancel_handle()));
        }
        let redirect_uri = session.redirect_uri().to_string();
        loopback_port_sig().set(port_from_redirect_uri(&redirect_uri));

        let verifier = match PkceVerifier::generate() {
            Ok(verifier) => verifier,
            Err(err) => {
                status_sig().set(format!("{label} -> Err(random: {err})"));
                return;
            }
        };
        let state = match State::generate() {
            Ok(state) => state,
            Err(err) => {
                status_sig().set(format!("{label} -> Err(random: {err})"));
                return;
            }
        };

        let mut authorization_endpoint = format!(
            "{base}loopback.html?gate_iss={}",
            encode_query_value(&issuer)
        );
        if let Some(mode) = gate_mode {
            authorization_endpoint.push_str("&gate_mode=");
            authorization_endpoint.push_str(mode);
        }
        let request = AuthorizationRequest {
            authorization_endpoint,
            client_id: LOOPBACK_CLIENT_ID.to_string(),
            redirect_uri: redirect_uri.clone(),
            scope: Some("openid".to_string()),
            resources: Vec::new(),
        };
        let url = match request.build(&state, &verifier.challenge()) {
            Ok(url) => url,
            Err(err) => {
                status_sig().set(format!("{label} -> Err(build: {err})"));
                return;
            }
        };

        let expected_issuer = if expect_bad_iss {
            format!("{issuer}/wrong")
        } else {
            issuer
        };

        spawn_local(async move {
            let outcome = session.start(&url).await;
            let rendered =
                render_loopback(&outcome, &redirect_uri, state, expected_issuer, &verifier);
            status_sig().set(format!("{label} -> {rendered}"));
        });
    }
}

/// Render a loopback session's outcome for the status line: a validated
/// `Callback` reports the authorization code's length plus the token-grant
/// body's length only (proving [`authorization_code_grant`]'s builder —
/// there is no HTTP client anywhere on this page), never the callback URL,
/// the code or the `state` itself. Every other outcome — `Cancelled`,
/// `TimedOut`, a `parse_callback` rejection such as `IssuerMismatch` or a
/// typed `Authorization { error: AccessDenied, .. }` — keeps its `Debug`
/// form, which this crate pair's own redaction rules already keep free of
/// secrets.
fn render_loopback(
    outcome: &Result<AuthSessionOutcome, AuthSessionError>,
    redirect_uri: &str,
    state: State,
    expected_issuer: String,
    verifier: &PkceVerifier,
) -> String {
    let Ok(AuthSessionOutcome::Callback(callback_url)) = outcome else {
        return format!("{outcome:?}");
    };
    let expectations = CallbackExpectations {
        redirect_uri: redirect_uri.to_string(),
        state,
        issuer: IssuerCheck::Required(expected_issuer),
    };
    match parse_callback(callback_url, &expectations) {
        Ok(code) => {
            let code_len = code.as_str().len();
            let form =
                authorization_code_grant(&code, redirect_uri, LOOPBACK_CLIENT_ID, verifier, &[]);
            format!(
                "code ok len={code_len}, state ok, iss ok, grant body len={}",
                form.body.len()
            )
        }
        Err(err) => format!("Err({err:?})"),
    }
}

/// `Loopback busy`: bind once, keep that session alive in scope, then bind
/// a second time — [`LoopbackSession::bind`] claims the crate's shared
/// single-session slot synchronously (this crate doc's *Exactly one live
/// session* section), so the second call's [`AuthSessionError::Busy`] is
/// available with no `await` needed. The first session drops (freeing the
/// slot) when this handler returns.
fn fire_loopback_busy(_state: &mut PlaygroundState) {
    let first = LoopbackSession::bind(LoopbackOptions::new(LOOPBACK_PATH));
    let second = LoopbackSession::bind(LoopbackOptions::new(LOOPBACK_PATH));
    status_sig().set(format!("Loopback busy -> {second:?}"));
    drop(first);
}

/// `Cancel loopback`: cancel whatever session [`loopback_cancel_sig`] is
/// holding. A no-op if nothing is stashed, or if the session it belongs to
/// has already resolved (see [`LoopbackCancel::cancel`]'s own doc).
fn fire_cancel_loopback(_state: &mut PlaygroundState) {
    if let Some(cancel) = loopback_cancel_sig().get_untracked() {
        cancel.cancel();
    }
}

/// See the page-fn contract in [`crate::pages`]. Reads no
/// [`PlaygroundState`] signal — the call status, base URL and ephemeral
/// toggle live in this module's own screen-local signals, and the deep-link
/// readout comes straight from [`frust::deep_links`].
pub fn page(_state: &PlaygroundState) -> impl View<PlaygroundState> {
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
    let supported_line = format!(
        "is_supported: {} loopback: {}",
        AuthSession::is_supported(),
        LoopbackSession::is_supported()
    );
    let loopback_port_line = match loopback_port_sig().get() {
        Some(port) => format!("loopback port: {port}"),
        None => "loopback port: none".to_string(),
    };

    let mut children: Vec<AnyView<PlaygroundState>> = vec![
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
        any(row()
            .child(
                button(
                    "Callback",
                    fire("Callback", "callback.html?scheme=frustplay"),
                )
                .style(ButtonStyle::Primary)
                .small(),
            )
            .push(gap_h(8.0))
            .child(
                button("Cancel test", fire("Cancel test", "cookie.html"))
                    .style(ButtonStyle::Secondary)
                    .small(),
            )),
        any(SizedBox(None, Some(8.0))),
        any(row()
            .child(
                button("Busy", fire_busy)
                    .style(ButtonStyle::Secondary)
                    .small(),
            )
            .push(gap_h(8.0))
            .child(
                button("Cookie check", fire("Cookie check", "cookie.html"))
                    .style(ButtonStyle::Secondary)
                    .small(),
            )),
        any(SizedBox(None, Some(8.0))),
        any(text(status_line).size(12.0)),
        any(SizedBox(None, Some(12.0))),
        any(text(latest_line).size(12.0).color(accent())),
        any(SizedBox(None, Some(4.0))),
        any(text(supported_line).size(11.0).color(muted())),
    ];

    if LoopbackSession::is_supported() {
        children.extend([
            any(SizedBox(None, Some(16.0))),
            any(text("Desktop loopback").size(13.0).color(accent())),
            any(text(
                "frust-auth-session's LoopbackSession (RFC 8252 §7.3) plus frust-oauth-native's \
                 PKCE/state/callback builders, driven against the same gate directory's \
                 loopback.html — a fake authorization server, never a real identity provider. \
                 Login proves the whole round trip (parse_callback plus the token-grant body, \
                 never sent anywhere); Deny and Bad iss are negative controls; Timeout and \
                 Cancel exercise the listener's own exit paths; Busy proves the shared \
                 single-session slot.",
            )
            .size(11.0)
            .color(muted())),
            any(SizedBox(None, Some(8.0))),
            any(row()
                .child(
                    button(
                        "Loopback login",
                        fire_loopback(
                            "Loopback login",
                            None,
                            DEFAULT_LOOPBACK_TIMEOUT,
                            false,
                            false,
                        ),
                    )
                    .style(ButtonStyle::Primary)
                    .small(),
                )
                .push(gap_h(8.0))
                .child(
                    button(
                        "Loopback deny",
                        fire_loopback(
                            "Loopback deny",
                            Some("deny"),
                            DEFAULT_LOOPBACK_TIMEOUT,
                            false,
                            false,
                        ),
                    )
                    .style(ButtonStyle::Secondary)
                    .small(),
                )),
            any(SizedBox(None, Some(8.0))),
            any(row()
                .child(
                    button(
                        "Loopback bad iss",
                        fire_loopback(
                            "Loopback bad iss",
                            None,
                            DEFAULT_LOOPBACK_TIMEOUT,
                            true,
                            false,
                        ),
                    )
                    .style(ButtonStyle::Secondary)
                    .small(),
                )
                .push(gap_h(8.0))
                .child(
                    button(
                        "Loopback timeout (10 s)",
                        fire_loopback(
                            "Loopback timeout (10 s)",
                            Some("stay"),
                            Duration::from_secs(10),
                            false,
                            false,
                        ),
                    )
                    .style(ButtonStyle::Secondary)
                    .small(),
                )),
            any(SizedBox(None, Some(8.0))),
            any(row()
                .child(
                    button(
                        "Loopback cancel",
                        fire_loopback(
                            "Loopback cancel",
                            Some("stay"),
                            DEFAULT_LOOPBACK_TIMEOUT,
                            false,
                            true,
                        ),
                    )
                    .style(ButtonStyle::Secondary)
                    .small(),
                )
                .push(gap_h(8.0))
                .child(
                    button("Cancel loopback", fire_cancel_loopback)
                        .style(ButtonStyle::Secondary)
                        .small(),
                )),
            any(SizedBox(None, Some(8.0))),
            any(row().child(
                button("Loopback busy", fire_loopback_busy)
                    .style(ButtonStyle::Secondary)
                    .small(),
            )),
            any(SizedBox(None, Some(8.0))),
            any(text(loopback_port_line).size(11.0).color(muted())),
        ]);
    } else {
        children.extend([
            any(SizedBox(None, Some(16.0))),
            any(text("Desktop loopback").size(13.0).color(accent())),
            any(text("loopback: not supported on this target")
                .size(11.0)
                .color(muted())),
        ]);
    }

    Padding(
        EdgeInsets::all(16.0),
        FlexView::new(
            Axis::Vertical,
            children.into_iter().map(inflexible).collect(),
        ),
    )
}
