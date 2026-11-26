# frust-auth-session

An **RFC 8252 "OAuth for native apps"** in-app browser authentication-session
plugin for frust apps — Android Auth Tab / Custom Tabs and Apple
`ASWebAuthenticationSession` — presented over the request's `https`
authorization URL and resolving to the identity provider's redirect to the
request's callback: a custom scheme (`myapp://…`) or a claimed `https` host
and path.

Like every frust **platform plugin**, this crate is added to your app's own
`Cargo.toml` alongside `frust` (the pubspec model) — the `frust` facade does
not re-export it.

**Status:** this crate ships real Android (Auth Tab, Custom Tabs fallback) and Apple
(`ASWebAuthenticationSession`) backends alongside its host-testable core —
the public API, the `oneshot` awaitable future, the one-live-session
("Busy") guard, and the URL/callback-scheme validators. See § 3 below for
each platform's caveats.

**Platform support:** Android (Auth Tab where the browser offers it, Chrome Custom Tabs otherwise), iOS and macOS (`ASWebAuthenticationSession`), and `LoopbackSession` on Linux, Windows and macOS. On other targets `AuthSession::start` rejects with `AuthSessionError::NoHandler`.

More about Frust: <https://frust.dev> and <https://github.com/frust-rs/frust>.

---

## 1. Add the dependency

```toml
# app Cargo.toml — [dependencies]
frust-auth-session = "0.6"
```

The Android backend needs this plugin's own Gradle library module linked too
(Auth Tab and Custom Tabs, reached through `androidx.browser`; the module also
pins `androidx.activity:activity` 1.9.0 for the Auth Tab result launcher). The frust
TUI's **Add Plugin** action (`a`) wires the Cargo dependency above and the
Gradle module include together. By hand: include the module in
`android/settings.gradle.kts` the way a generated project includes
`:frust-embedding` — `project(":frust-auth-session").projectDir =
frustLocalDir("frust.plugin.frust-auth-session.dir")`, the key written into
`android/local.properties` by `frust run`/`frust build` — and add
`implementation(project(":frust-auth-session"))` to `android/app/`'s
dependencies (see [`platform/android/build.gradle.kts`](platform/android/build.gradle.kts)'s
header comment).

On **iOS** the app target must also link `AuthenticationServices.framework`
— the TUI's Add Plugin does this (its `IosFramework` contribution
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

let req = AuthSessionRequest::custom_scheme(
    "https://issuer.example/authorize?client_id=…&redirect_uri=com.example.app:/oauth2redirect",
    "com.example.app",
);
// Claimed https redirect instead (see "Redirect forms"):
// AuthSessionRequest::https(url, "login.example.com", "/oauth/callback")
// Optional: `.ephemeral(true)` for a private session with no persisted cookies.

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
    Err(err) => { /* AuthSessionError::{InvalidUrl,Busy,PlatformNotInitialized,NoHandler,HttpsCallbackUnsupported,TimedOut,Platform} */ }
}
```

`AuthSessionRequest` is `#[non_exhaustive]`: build it with
`AuthSessionRequest::custom_scheme(url, scheme)` or
`AuthSessionRequest::https(url, host, path)`, then `.ephemeral(bool)`. The
request's `callback` field is an `AuthCallback`: `CustomScheme(String)` or
`Https { host, path }`. **Migration** from the old struct, whose public
field that held the scheme is gone (a source break):

```rust
// Before
AuthSessionRequest { url, callback_scheme: "com.example.app".into(), ephemeral: false }
// After
AuthSessionRequest::custom_scheme(url, "com.example.app")
```

`AuthSession::start` validates `req.url` (must be an absolute `https` URL)
and the callback before touching any platform API — a rejected request
resolves `Err(AuthSessionError::InvalidUrl)` on the future's very first poll.
A custom scheme is 1-64 bytes, lowercase-alpha first byte,
lowercase-alnum/`+`/`-`/`.` thereafter, not one of the well-known schemes
`http`/`https`/`file`/`javascript`/`data`/`blob`/`intent`/`content`/`about`;
it is the scheme only, not the full redirect URI, and dotted reverse-DNS
schemes such as `io.example.console` are accepted. An `https` callback is a
bare DNS-name host (no scheme, port or userinfo) and a path starting with `/`
of RFC 3986 path characters with no `.`/`..` segment; a delivered URL must
match the host (case-insensitively) and the path (byte for byte), and its
query and fragment come back untouched. Where the backend cannot catch an
`https` callback, `start` resolves `Err(AuthSessionError::HttpsCallbackUnsupported)`
before any UI appears.

### Redirect forms

RFC 8252 § 7.2 prefers claimed `https` redirects over custom schemes where the
platform supports them (a custom scheme can be claimed by any app, § 3.4).

| Redirect | iOS | macOS | Android | Linux | Windows |
|---|---|---|---|---|---|
| Custom scheme | yes (iOS 15+) | yes (macOS 14.4+ for the new initializer, else the older one) | yes: Auth Tab, else Custom Tabs | no (`NoHandler`) | no (`NoHandler`) |
| Claimed `https` | iOS 17.4+; the host must be a domain associated with the app | macOS 14.4+; same | Auth Tab only (Chrome 137+); Digital Asset Links on the host | no | no |
| Loopback (`LoopbackSession`) | no | yes | no | yes | yes |

Device status: the custom scheme is verified on an OnePlus 9 (Android 15,
Chrome 159, through Auth Tab), an iPhone 13 mini (iOS 27.2) and macOS 27.2.1.
The `https` form is **not device-verified end to end on any platform** (no test
domain). Two probes ran. Apple (iPhone iOS 27.2, macOS 27.2.1), a domain the
app is not associated with: no consent alert and no sheet appear and the
future resolves `Ok(Cancelled)` within about two seconds; the system reports
its refusal with the same error code as a user cancel, so the plugin cannot
tell them apart. **A `Cancelled` that arrives at once with no UI on an `https`
request means the domain association is missing or wrong**
(`auth-session-apple-unassociated-https-reads-as-cancelled-v1`; the
association itself has not been run on any device). Android (Chrome 159): the
request is accepted and an Auth Tab opens; nothing is verified until a
redirect reaches the https callback, which was not run. Below iOS 17.4 /
macOS 14.4 an `https` callback is refused
(`auth-session-https-callback-unverified-v1` in
[LIMITATIONS.md](https://github.com/frust-rs/frust/blob/main/docs/LIMITATIONS.md)).

### Cancel

`AuthSession::start_with_cancel(req)` returns the future and an
`AuthSessionCancel` handle (`Clone + Send + Sync`). `cancel()` resolves the
future `Ok(Cancelled)` once and frees the one-session slot at once. The
browser UI: on iOS and macOS the sheet or window is dismissed (a session
started right after waits out a short dismissal delay); on Android the tab
stays open until the user leaves it, on both Auth Tab and Custom Tabs
(`auth-session-android-cancel-leaves-tab-open`). A handle for a request refused
at start is inert.

While the browser is in front the app is in the background and its UI-thread
timers (`frust::time`) do not run (seen on Android), so a cancel on a deadline
must be called from another thread:

```rust
let (session, cancel) = AuthSession::start_with_cancel(req);
std::thread::spawn(move || {
    std::thread::sleep(std::time::Duration::from_secs(5));
    cancel.cancel();
});
let outcome = session.await;
```

### Backgrounding

- **iOS / macOS:** no app-lifecycle event is watched; the future stays pending
  and the sheet is still up on return (iPhone 13 mini: Home, then back, status
  unchanged); it resolves on the redirect, the user's dismissal, or a cancel.
- **Android, Auth Tab:** an unrelated Activity resume, or the launching
  Activity being recreated, does not cancel (OnePlus 9: Home, then reopen: the
  tab is still on top and nothing resolves; closing it gives `Ok(Cancelled)`
  once). It resolves `Cancelled` by itself only when the result can no longer
  reach the app (the Activity finished, or returned without its saved state).
- **Android, Custom Tabs:** nothing resolves while backgrounded; the first
  resume of an app Activity without the callback intent resolves `Cancelled`
  (`auth-session-android-resume-means-cancelled-v1`).
- **Desktop loopback:** the listener keeps waiting, bounded by its timeout.

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

- **iOS / macOS** (`ASWebAuthenticationSession`). Where the OS has
  `ASWebAuthenticationSessionCallback` (iOS 17.4+, macOS 14.4+, a runtime
  check) both callback forms use the new initializer; below that a custom
  scheme uses the older one and an `https` callback is refused with
  `HttpsCallbackUnsupported`. An `https` callback needs the app to be
  associated with the host (`applinks:` and `webcredentials:` entitlements, an
  `apple-app-site-association` file the app owner hosts):
  - iOS apps link `AuthenticationServices.framework` (§ 1 — `frust plugin
    add` wires it; a hand-wired app adds the flag itself).
  - For a custom-scheme request, your app's `Info.plist` must register the
    request's scheme under `CFBundleURLTypes` — the same scheme registration
    any custom-scheme deep link needs.
  - The session intercepts the redirect **in-process**: it never reaches the
    app as a URL open, so `frust_on_deep_link` is **not** invoked for it
    (unlike Android — see § 5). An app's ordinary deep-link handling can
    leave that scheme untouched.
  - This crate's deployment floor is iOS 15 (the workspace floor —
    [DEVELOPMENT.md](https://github.com/frust-rs/frust/blob/main/docs/DEVELOPMENT.md)'s Version-Pin Policy table);
    below iOS 17.4 the deprecated `-initWithURL:callbackURLScheme:completionHandler:`
    initializer serves custom schemes (`auth-session-https-callback-unverified-v1`
    in [LIMITATIONS.md](https://github.com/frust-rs/frust/blob/main/docs/LIMITATIONS.md)).
  - Starting a session before any window exists resolves
    `AuthSessionError::Platform("no window to present the authentication
    session over")` rather than presenting nothing.
  - `AuthSessionRequest::ephemeral` maps straight onto
    `prefersEphemeralWebBrowserSession` and is honoured strictly (not
    best-effort, unlike Android below).
- **Android** (Auth Tab, Custom Tabs fallback): the browser the plugin picks
  decides the path — Auth Tab when it supports it (Chrome 137+), Custom Tabs
  otherwise. With Auth Tab the redirect returns as an activity result: it is
  not also delivered as a deep link, an unrelated return to the app does not
  report `Cancelled`, `AuthCallback::Https` works (the host must serve Digital
  Asset Links), and a session that loses its launcher for good resolves
  `Cancelled`.
  - Needs this plugin's own Gradle module linked (§ 1) — a session on an app
    that skips it resolves `AuthSessionError::Platform` naming `frust plugin
    add auth-session`.
  - The tab launches from the currently resumed `Activity`; with none
    resumed the session resolves `AuthSessionError::Platform("no resumed
    Activity …")` rather than queuing.
  - On the Custom Tabs fallback the redirect is observed through the app's own
    `VIEW`/`BROWSABLE` custom-scheme `<intent-filter>` on resume — the same intent-filter shape
    (and same lack of domain verification) an ordinary deep link uses
    (`auth-session-android-callback-rides-intent-filter` in
    [LIMITATIONS.md](https://github.com/frust-rs/frust/blob/main/docs/LIMITATIONS.md)). The callback URL therefore
    reaches the app **twice** — see § 5. An Auth Tab session needs no such filter.
  - **Provider selection** (`chooseCustomTabsProvider`): the user's default
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
  - **On the Custom Tabs fallback, any Activity resume ends the pending session.** The host reads the
    outcome from the resumed Activity's `Intent`: a callback-scheme `Intent`
    is the redirect (`Ok(Callback)`); anything else — a permission dialog, an
    app switch, a system event — resolves `Ok(Cancelled)`, exactly as a user
    dismissing the tab does, and a still-open Custom Tab is left for the user
    to close (`auth-session-android-resume-means-cancelled-v1` in
    [LIMITATIONS.md](https://github.com/frust-rs/frust/blob/main/docs/LIMITATIONS.md)). Error kind 4 (`Platform`,
    "no resumed Activity") is launch-time only: no tab was ever opened.
  - **On the Custom Tabs path, attribution is by timing, not evidence.** The host stamps whichever
    callback-scheme `Intent` it observes with the generation of the session
    pending *now*; the generation never travels through the browser or the
    redirect. It only lets Rust discard a result for a session that is no
    longer live (dropped future, already resolved) — it cannot tell a
    superseded tab's late redirect from the live session's own
    (`auth-session-android-callback-rides-intent-filter` in
    [LIMITATIONS.md](https://github.com/frust-rs/frust/blob/main/docs/LIMITATIONS.md)). PKCE + `state` (§ 3.4) are
    the caller's defence and are required on every path (Auth Tab removes only the timing attribution: it answers with its own result). Dropping the awaited future releases the Busy slot
    but does not close the tab; to end a session, start it with
    `AuthSession::start_with_cancel` and call the handle's `cancel` (the tab
    still stays open on Android: `auth-session-android-cancel-leaves-tab-open`).
- **Desktop (Linux, Windows, macOS):** use [`LoopbackSession`] instead. On
  Linux and Windows, `AuthSession::start` with a custom scheme still answers
  [`AuthSessionError::NoHandler`] (no platform authentication user agent
  exists; the loopback path has been run against a live browser on both — see §6), and
  [`AuthSession::is_supported()`](https://github.com/frust-rs/frust/blob/main/plugins/auth-session/src/lib.rs) is `false`
  there (`true` on macOS, where `ASWebAuthenticationSession` serves the
  custom-scheme path). Ask `LoopbackSession::is_supported()` for the loopback
  path — `true` on all three. The listener binds on ephemeral loopback
  (`127.0.0.1:<port>` on an ephemeral port; the accept loop polls its shutdown flag every 25 ms, so a
  cancel or timeout closes the socket within about one poll), opens the authorization URL in
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

RFC 8252 § 8.1 documents the risks of custom-scheme redirects: the Android Custom Tabs fallback accepts any Intent carrying
the callback scheme while a session is pending, so any installed app or tapped link can forge a
redirect; iOS/macOS intercept the redirect in-process (in-app only, not as a deep link), but the
redirect itself is unauthenticated — the crate has no way to prove it came from the identity
provider rather than the browser or another app. PKCE and state verification are essential on all
three platforms to prevent authorization code theft.

RFC 8252 § 7.2 prefers claimed `https` redirects: the OS only hands the redirect
to an app the domain owner has associated, which a custom scheme cannot promise.
The check on a delivered `https` URL (scheme, host, path) is a consistency check
on the platform's delivery, not proof of origin; PKCE and `state` still bind.

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
the sheet or the next `start` cancels it). To end a session, start it with
`AuthSession::start_with_cancel` and call the returned handle's `cancel`: the future resolves
`Cancelled` and the slot frees, and the Apple sheet is dismissed, but the Android tab stays open —
`auth-session-android-cancel-leaves-tab-open` in
[LIMITATIONS.md](https://github.com/frust-rs/frust/blob/main/docs/LIMITATIONS.md).

---

## 5. Deep links vs the future

On the Android **Custom Tabs fallback**, the callback URL redirected back into
the app reaches the platform **twice**: once as the redirect this crate resolves
`AuthSession::start`'s future with, and once more as an ordinary deep link
`frust::deep_links()` also observes (the OS has no way to know only this
crate wants that intent). Treat the future as the single authoritative
source for an auth-session callback, and ignore the callback scheme in your
app's own deep-link handling — acting on both delivers the same
authorization code twice. **Where Auth Tab is used the redirect is no longer
also delivered as a deep link** (a compatibility change: an app that relied on
seeing it in `deep_links()` must read it from the future). iOS/macOS has no
double delivery — see § 3.

---

## 6. Tested platforms

The backends have been exercised on real systems, each with a browser-hosted
test page that redirects to the callback:

- **Android** — Xiaomi 12 (Android 16), with Chrome Custom Tabs providers
  (Chrome, Fennec) and a default browser without Custom Tabs support; OnePlus 9
  (Android 15, Chrome 159) through Auth Tab.
- **iOS** — iPhone 17 simulator and a physical iPhone SE; iPhone 13 mini
  (iOS 27.2, the callback-initializer path).
- **macOS** — Apple M4, Safari, via `ASWebAuthenticationSession`; macOS 27.2.1
  run as a bare cargo binary, where the system consent alert names the app
  `(null)` because there is no bundle name.
- **Linux and Windows desktop** — `LoopbackSession` against a live browser.

Cases covered: callback delivery, user cancellation, cancel from the app
(`start_with_cancel`, then a new session at once), leaving the app mid-session
and returning, ephemeral versus persisted browser sessions, and a second
`start` while a session is live (`Busy`). **Not device-verified:** an `https`
callback completing (only the two probes in § 2 ran), Android App Links
verification, and a universal link opened from another app.

## License

Licensed under either of MIT or Apache-2.0 (SPDX: `MIT OR Apache-2.0`), at your
option. See `LICENSE-MIT` and `LICENSE-APACHE` beside this README.
