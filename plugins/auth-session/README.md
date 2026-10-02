# frust-auth-session

An **RFC 8252 "OAuth for native apps"** in-app browser authentication-session
plugin for frust apps — Android Custom Tabs / Apple
`ASWebAuthenticationSession` — presented over the request's `https`
authorization URL and resolving to the identity provider's
`callback_scheme://…` redirect.

Like every frust **platform plugin**, this crate is added to your app's own
`Cargo.toml` alongside `frust` (the pubspec model) — the `frust` facade does
not re-export it.

**Status:** this crate ships real Android (Chrome Custom Tabs) and Apple
(`ASWebAuthenticationSession`) backends alongside its host-testable core —
the public API, the `oneshot` awaitable future, the one-live-session
("Busy") guard, and the URL/callback-scheme validators. See § 3 below for
each platform's caveats.

**Platform support:** Android (Chrome Custom Tabs), iOS and macOS (`ASWebAuthenticationSession`), and `LoopbackSession` on Linux, Windows and macOS. On other targets `AuthSession::start` rejects with `AuthSessionError::NoHandler`.

More about Frust: <https://frust.dev> and <https://github.com/frust-rs/frust>.

---

## 1. Add the dependency

```toml
# app Cargo.toml — [dependencies]
frust-auth-session = { path = "<frust>/plugins/auth-session" }  # crates.io later
```

`<frust>` is the path to your frust checkout — derive it from the `frust = {
path = "…" }` line the scaffold already wrote. The Android backend needs
this plugin's own Gradle library module linked too (Chrome Custom Tabs,
reached through `androidx.browser.customtabs`) — run
`frust plugin add auth-session` to wire the Cargo dependency above and the
Gradle module include together, or add the module by hand per
[`platform/android/build.gradle.kts`](platform/android/build.gradle.kts)'s
own header comment (a `settings.gradle.kts` module include plus an
`implementation(project(":frust-auth-session"))` line in `android/app/`,
the same shape `plugins/secure-storage/platform/android` uses).

On **iOS** the app target must also link `AuthenticationServices.framework`
— `frust plugin add auth-session` does this (its `IosFramework` contribution
appends `-framework AuthenticationServices` to every `OTHER_LDFLAGS` list in
`ios/Runner.xcodeproj/project.pbxproj`); an app wired by hand adds the same
flag itself. This crate's Rust code references the framework's
`ASWebAuthenticationSessionErrorDomain` symbol, and a Rust staticlib cannot
carry a framework link into Xcode's link step, so without the flag the app
fails to link with `Undefined symbols for architecture arm64:
"_ASWebAuthenticationSessionErrorDomain"`. A macOS binary linked by cargo
needs nothing extra — the objc2 binding's own link attribute applies there.

---

## 2. API and awaiting the future

```rust
use frust_auth_session::{AuthSession, AuthSessionOutcome, AuthSessionRequest};

let req = AuthSessionRequest {
    url: "https://issuer.example/authorize?client_id=…&redirect_uri=com.example.app:/oauth2redirect".to_string(),
    callback_scheme: "com.example.app".to_string(),
    ephemeral: false,
};

// Poll from the UI thread's local executor (`frust::spawn_local`) or any
// other executor — resolution is driven by the platform's own main thread,
// not by whichever thread happens to be polling.
match AuthSession::start(req).await {
    // `{:?}` on an outcome prints `Callback { url_len: N }`, never the URL —
    // read the String through the value, as here.
    Ok(AuthSessionOutcome::Callback(url)) => {
        // Parse the callback URL and verify the state parameter against your
        // session state (PKCE or equivalent is also required — see § 3.4).
        // Then extract the authorization code from `url`'s query string and
        // exchange it for tokens — this crate's job ends at handing back the
        // raw callback URL.
        let url_str = format!("{}", url);  // read the String through match/pattern
        // ... verify state_param from url_str matches your session ...
        // ... verify PKCE challenge ...
    }
    Ok(AuthSessionOutcome::Cancelled) => { /* user dismissed the tab */ }
    Err(err) => { /* AuthSessionError::{InvalidUrl,Busy,PlatformNotInitialized,NoHandler,TimedOut,Platform} */ }
}
```

`AuthSession::start` validates `req.url` (must be an absolute `https` URL)
and `req.callback_scheme` (1-64 bytes, lowercase-alpha first byte,
lowercase-alnum/`+`/`-`/`.` thereafter, not one of the well-known schemes
`http`/`https`/`file`/`javascript`/`data`/`blob`/`intent`/`content`/`about`)
before touching any platform API — a rejected request resolves
`Err(AuthSessionError::InvalidUrl)` on the future's very first poll.
The validator accepts dotted reverse-DNS schemes such as `io.example.console` (pinned by the test `reverse_dns_callback_scheme_is_accepted`), and `callback_scheme` is the scheme only, not the full redirect URI.

### Desktop loopback (Linux, Windows, macOS)

On desktop, redirect to a loopback HTTP listener instead of a custom scheme (RFC 8252 §7.3):

```rust
use frust_auth_session::{AuthSession, AuthSessionOutcome, AuthSessionRequest, LoopbackSession, LoopbackOptions};
use frust_oauth_native::AuthorizationRequest;

let session = LoopbackSession::bind(LoopbackOptions::new("/oauth/callback"))?;
let redirect_uri = session.redirect_uri().to_string();

// Build the authorization URL with the loopback redirect URI as the target.
let request = AuthorizationRequest {
    authorization_endpoint: "https://as.example/authorize".to_string(),
    client_id: "example-app".to_string(),
    redirect_uri: redirect_uri.clone(),
    scope: Some("openid profile offline_access".to_string()),
    resources: Vec::new(),
};
let url = request.build(&state, &verifier.challenge())?;

// Start the session — it opens the URL in the system browser (via
// frust-url-launcher) and waits for the redirect.
let outcome = session.start(&url).await?;
```

The one-live-session slot (the same one [`AuthSession::start`] claims) is claimed at [`LoopbackSession::bind`] and released when the session drops. Its behaviour follows these rules:

- **Bind address:** `127.0.0.1` on an ephemeral port, never `localhost`, `::1` or `0.0.0.0`. `LoopbackSession::redirect_uri()` gives the full URI (e.g. `http://127.0.0.1:54321/oauth/callback`).
- **Request acceptance:** the listener accepts GET requests on the exact reserved path only. Any other method or path answers `404`; a malformed request (a bad request line, a bad/duplicate/mismatched `Host` header, obsolete line folding) answers `400`. Both keep the listener **waiting** for the callback (stray local clients cannot end the session).
- **Host header:** the listener rejects requests whose `Host` header does not match `127.0.0.1:<port>` (case-insensitive). This is the DNS-rebinding defence — a page that resolves its hostname to `127.0.0.1` sends its own hostname in the `Host` header, not `127.0.0.1`.
- **Fetch Metadata:** a request carrying a `Sec-Fetch-Mode` other than `navigate` or a `Sec-Fetch-Dest` other than `document` answers `404` and the listener keeps waiting — defence in depth against a no-cors cross-origin request (e.g. a port scan) from any web origin open in the browser. Either header's absence leaves the request allowed.
- **Response:** a static page with `Cache-Control: no-store`, `Content-Security-Policy: default-src 'none'`, no script, and no echo of the request bytes.
- **Lifecycle:** the listener accepts exactly one callback, then closes. Cancelling via [`LoopbackCancel::cancel`], dropping the future, or the session timing out all close the listener within about one poll interval (~25 ms), even while a request is being served — the exception is a response write already in progress, bounded by the 2 s connection I/O timeout.
- **Timeout:** the session waits for the redirect up to [`LoopbackOptions::timeout`] (default 300 s, configurable, at most one hour). When it elapses, the session resolves [`AuthSessionError::TimedOut`] and closes.

---

## 3. Platform caveats

- **iOS / macOS** (`ASWebAuthenticationSession`):
  - iOS apps link `AuthenticationServices.framework` (§ 1 — `frust plugin
    add` wires it; a hand-wired app adds the flag itself).
  - Your app's `Info.plist` must register `req.callback_scheme` under
    `CFBundleURLTypes` — the same scheme registration any custom-scheme deep
    link needs.
  - The session intercepts the redirect **in-process**: it never reaches the
    app as a URL open, so `frust_on_deep_link` is **not** invoked for it
    (unlike Android — see § 5). An app's ordinary deep-link handling can
    leave that scheme untouched.
  - This crate's deployment floor is iOS 15 (the workspace floor —
    [DEVELOPMENT.md](https://github.com/frust-rs/frust/blob/main/docs/DEVELOPMENT.md)'s Version-Pin Policy table);
    the deprecated `-initWithURL:callbackURLScheme:completionHandler:`
    initializer is used rather than iOS 17.4's HTTPS-App-Link-capable one
    (`auth-session-ios-https-callback-not-supported-v1` in
    [LIMITATIONS.md](https://github.com/frust-rs/frust/blob/main/docs/LIMITATIONS.md)).
  - Starting a session before any window exists resolves
    `AuthSessionError::Platform("no window to present the authentication
    session over")` rather than presenting nothing.
  - `AuthSessionRequest::ephemeral` maps straight onto
    `prefersEphemeralWebBrowserSession` and is honoured strictly (not
    best-effort, unlike Android below).
- **Android** (Chrome Custom Tabs):
  - Needs this plugin's own Gradle module linked (§ 1) — a session on an app
    that skips it resolves `AuthSessionError::Platform` naming `frust plugin
    add auth-session`.
  - The Custom Tab launches from the currently resumed `Activity`; with none
    resumed the session resolves `AuthSessionError::Platform("no resumed
    Activity …")` rather than queuing.
  - The redirect is observed through the app's own `VIEW`/`BROWSABLE`
    custom-scheme `<intent-filter>` on resume — the same intent-filter shape
    (and same lack of domain verification) an ordinary deep link uses
    (`auth-session-android-callback-rides-intent-filter` in
    [LIMITATIONS.md](https://github.com/frust-rs/frust/blob/main/docs/LIMITATIONS.md)). The callback URL therefore
    reaches the app **twice** — see § 5.
  - **Provider selection** (`pinCustomTabsProvider`): the user's default
    `http://` `VIEW` handler is pinned if it alone answers `CustomTabsService`;
    otherwise the host enumerates every installed `VIEW`/`BROWSABLE` `http://`
    handler with `MATCH_ALL` (default-app filtering would hide every
    non-default browser), intersects that set with a fixed allow-list of
    well-known browsers in preference order (Chrome variants, Firefox / Fenix /
    Fennec F-Droid, Brave, Samsung Internet, Edge, Vivaldi, DuckDuckGo) and
    pins the first one `getPackageName(…, ignoreDefault = true)` accepts —
    never a package outside the allow-list. With no qualifying provider the
    Custom Tab launches unpinned and the OS resolves it like any `VIEW` intent
    (a plain browser window). The module manifest's `<queries>` block is what
    makes those handlers visible under Android 11+ package-visibility rules.
  - **`NoHandler`:** arises only when `CustomTabsIntent.launchUrl` raises
    `ActivityNotFoundException` — no `VIEW` handler for the authorization URL
    exists at all on the device (very rare; see kind 2 in § 5).
  - `ephemeral` maps to `CustomTabsIntent.Builder#setEphemeralBrowsingEnabled`,
    advisory to the browser (a browser that doesn't support it opens a normal
    tab instead of refusing).
  - **Any Activity resume ends the pending session.** The host reads the
    outcome from the resumed Activity's `Intent`: a callback-scheme `Intent`
    is the redirect (`Ok(Callback)`); anything else — a permission dialog, an
    app switch, a system event — resolves `Ok(Cancelled)`, exactly as a user
    dismissing the tab does, and a still-open Custom Tab is left for the user
    to close (`auth-session-android-resume-means-cancelled-v1` in
    [LIMITATIONS.md](https://github.com/frust-rs/frust/blob/main/docs/LIMITATIONS.md)). Error kind 4 (`Platform`,
    "no resumed Activity") is launch-time only: no tab was ever opened.
  - **Attribution is by timing, not evidence.** The host stamps whichever
    callback-scheme `Intent` it observes with the generation of the session
    pending *now*; the generation never travels through the browser or the
    redirect. It only lets Rust discard a result for a session that is no
    longer live (dropped future, already resolved) — it cannot tell a
    superseded tab's late redirect from the live session's own
    (`auth-session-android-callback-rides-intent-filter` in
    [LIMITATIONS.md](https://github.com/frust-rs/frust/blob/main/docs/LIMITATIONS.md)). PKCE + `state` (§ 3.4) are
    the caller's defence. Dropping the awaited future releases the Busy slot
    but does not close the tab (`auth-session-no-cancel-v1`).
- **Desktop (Linux, Windows, macOS):** use [`LoopbackSession`] instead. On
  Linux and Windows, `AuthSession::start` with a custom scheme still answers
  [`AuthSessionError::NoHandler`] (no platform authentication user agent
  exists; the loopback path has been run against a live browser on both — see §6), and
  [`AuthSession::is_supported()`](https://github.com/frust-rs/frust/blob/main/plugins/auth-session/src/lib.rs) is `false`
  there (`true` on macOS, where `ASWebAuthenticationSession` serves the
  custom-scheme path). Ask `LoopbackSession::is_supported()` for the loopback
  path — `true` on all three. The listener binds on ephemeral loopback
  (`127.0.0.1:<port>` on an ephemeral port; `auth-session-loopback-poll-interval-v1` in
  [LIMITATIONS.md](https://github.com/frust-rs/frust/blob/main/docs/LIMITATIONS.md)), opens the authorization URL in
  the system browser (via [`frust-url-launcher`](https://github.com/frust-rs/frust/blob/main/plugins/url-launcher/README.md)),
  and receives the redirect — RFC 8252 §7.3's "loopback IP redirection" pattern
  for platforms with no in-app browser-tab primitive
  (`auth-session-loopback-first-match-wins-v1`, `auth-session-loopback-local-stall-v1` in
  [LIMITATIONS.md](https://github.com/frust-rs/frust/blob/main/docs/LIMITATIONS.md)). See the Desktop loopback subsection
  above for the complete listener contract and example.

---

## 3.4. Security

This crate returns the raw callback URL to the caller — it does not validate or consume the
authorization code or state parameter on any platform. **Callers MUST use PKCE (RFC 7636 with
S256 challenge method) and MUST verify the `state` parameter against their own session state
before exchanging the code for tokens on EVERY platform.** 

RFC 8252 § 8.1 documents the risks of custom-scheme redirects: Android accepts any Intent carrying
the callback scheme while a session is pending, so any installed app or tapped link can forge a
redirect; iOS/macOS intercept the redirect in-process (in-app only, not as a deep link), but the
redirect itself is unauthenticated — the crate has no way to prove it came from the identity
provider rather than the browser or another app. PKCE and state verification are essential on all
three platforms to prevent authorization code theft.

The loopback listener has its own attack surface. Any local process can connect to the loopback
port while the session is live and the authorization response is plain `http`. A same-user local
process can race the real redirect to the reserved path and send its own request — it does not
matter which request arrives first. A well-formed callback proves only that *a* local client sent
a request to the reserved path, never that it came from the browser. **The crate's PKCE and `state`
requirement binds here exactly as for a custom-scheme callback** (RFC 8252 § 8.3): the app's own
state and PKCE check are the sole defence against accepting a forged response.

The loopback listener also checks the `Host` header (must match `127.0.0.1:<port>`) to defend
against DNS rebinding: a web page can resolve its hostname to `127.0.0.1`, but the browser still
sends its own hostname in the `Host` header, not `127.0.0.1`, so the request is rejected. Lastly,
every response is a static page with no echo of request bytes and a `no-store` cache policy.

On Linux and macOS, the authorization URL (its state parameter, PKCE code challenge, and the
redirect listener's port) is handed to `xdg-open` or `open` as a command-line argument and is
readable by other local users via `/proc/<pid>/cmdline` on Linux or `ps` on macOS. The S256 PKCE
challenge does not reveal the verifier, so the authorization code exchange cannot be completed by
a reader, but the `state` parameter itself is not secret from other users on a shared host.

---

## 4. One-session contract

At most one session runs at a time, process-wide: the one-live-session slot is **shared across
both backends** (custom-scheme [`AuthSession`] and loopback [`LoopbackSession`]). Starting a second
one while the first is still live resolves immediately to [`AuthSessionError::Busy`] rather than
queuing or replacing it. A bound [`LoopbackSession`] (even if not yet started) holds the slot, so
both [`AuthSession::start`] and a second [`LoopbackSession::bind`] answer [`Busy`] while one is
live, and vice versa.

The slot frees the moment the live session resolves — by outcome, by error, or by a backend module
failing without ever calling back — so a stuck session can never wedge every later one behind it.
Dropping the awaited future frees the slot too, but frees only this crate's bookkeeping: the
platform UI stays up (Android: until the user closes the tab; iOS/macOS: until the user dismisses
the sheet or the next `start` cancels it) — `auth-session-no-cancel-v1` in
[LIMITATIONS.md](https://github.com/frust-rs/frust/blob/main/docs/LIMITATIONS.md).

---

## 5. Deep links vs the future

On Android, the callback URL Custom Tabs redirects back into the app reaches
the platform **twice**: once as the redirect this crate resolves
`AuthSession::start`'s future with, and once more as an ordinary deep link
`frust::deep_links()` also observes (the OS has no way to know only this
crate wants that intent). Treat the future as the single authoritative
source for an auth-session callback, and ignore `callback_scheme` in your
app's own deep-link handling — acting on both delivers the same
authorization code twice. iOS/macOS has no such double delivery — see § 3.

---

## 6. Tested platforms

The backends have been exercised on real systems, each with a browser-hosted
test page that redirects to the callback:

- **Android** — Xiaomi 12 (Android 16), with Chrome Custom Tabs providers
  (Chrome, Fennec) and a default browser without Custom Tabs support.
- **iOS** — iPhone 17 simulator and a physical iPhone SE.
- **macOS** — Apple M4, Safari, via `ASWebAuthenticationSession`.
- **Linux and Windows desktop** — `LoopbackSession` against a live browser.

Cases covered: callback delivery, user cancellation, ephemeral versus
persisted browser sessions, and a second `start` while a session is live
(`Busy`).

## License

Licensed under either of MIT or Apache-2.0 (SPDX: `MIT OR Apache-2.0`), at your
option. See `LICENSE-MIT` and `LICENSE-APACHE` beside this README.
