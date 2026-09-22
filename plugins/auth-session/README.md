# frust-auth-session

An **RFC 8252 "OAuth for native apps"** in-app browser authentication-session
plugin for frust apps — Android Custom Tabs (owed to a follow-on backend
card) / Apple `ASWebAuthenticationSession` (owed to a follow-on backend
card), presented over the request's `https` authorization URL and resolving
to the identity provider's `callback_scheme://…` redirect.

Like every frust **platform plugin**, this crate is added to your app's own
`Cargo.toml` alongside `frust` (the pubspec model) — the `frust` facade does
not re-export it.

**Status:** this crate currently ships its host-testable core only — the
public API, the `oneshot` awaitable future, the one-live-session ("Busy")
guard, and the URL/callback-scheme validators. Its Android and Apple
backends are stubs that reject every session with
`AuthSessionError::Platform("backend not implemented")`; see § 3 below.

---

## 1. Add the dependency

```toml
# app Cargo.toml — [dependencies]
frust-auth-session = { path = "<frust>/plugins/auth-session" }  # crates.io later
```

`<frust>` is the path to your frust checkout — derive it from the `frust = {
path = "…" }` line the scaffold already wrote. No Gradle module of its own is
needed for the Android backend this crate will eventually ship (Custom Tabs
is reached through `androidx.browser.customtabs`, added the same way any
other Android dependency is — the future Android backend card documents the
exact wiring); until then, the Gradle module row `frust plugin add
auth-session` would add is a no-op placeholder.

---

## 2. API and awaiting the future

```rust
use frust_auth_session::{AuthSession, AuthSessionOutcome, AuthSessionRequest};

let req = AuthSessionRequest {
    url: "https://issuer.example/authorize?client_id=…&redirect_uri=myapp://cb".to_string(),
    callback_scheme: "myapp".to_string(),
    ephemeral: false,
};

// Poll from the UI thread's local executor (`frust::spawn_local`) or any
// other executor — resolution is driven by the platform's own main thread,
// not by whichever thread happens to be polling.
match AuthSession::start(req).await {
    Ok(AuthSessionOutcome::Callback(url)) => {
        // Exchange the authorization code in `url`'s query string for
        // tokens — this crate's job ends at handing back the raw callback
        // URL.
    }
    Ok(AuthSessionOutcome::Cancelled) => { /* user dismissed the tab */ }
    Err(err) => { /* AuthSessionError::{InvalidUrl,Busy,PlatformNotInitialized,NoHandler,Platform} */ }
}
```

`AuthSession::start` validates `req.url` (must be an absolute `https` URL)
and `req.callback_scheme` (1-64 bytes, lowercase-alpha first byte,
lowercase-alnum/`+`/`-`/`.` thereafter, not one of the well-known schemes
`http`/`https`/`file`/`javascript`/`data`/`blob`/`intent`/`content`/`about`)
before touching any platform API — a rejected request resolves
`Err(AuthSessionError::InvalidUrl)` on the future's very first poll.

---

## 3. Platform caveats

*(to be completed by the Android and Apple backend cards — both backends are
currently stubs; every session on every target rejects immediately with
`AuthSessionError::Platform("backend not implemented")`.)*

- **Android:** owed — Custom Tabs (`androidx.browser.customtabs`)
  presentation, the redirect-intent contract, and the double-delivery
  interaction with `frust::deep_links()` (§ 5 below) all land with that
  card.
- **iOS / macOS:** owed — `ASWebAuthenticationSession` presentation (a
  `UIWindow` anchor on iOS, an `NSWindow` anchor on macOS),
  `prefersEphemeralWebBrowserSession` wiring for
  [`AuthSessionRequest::ephemeral`], and the completion-block → this crate's
  `oneshot` channel hookup all land with that card.
- **Desktop (Linux, Windows):** permanent — no platform authentication user
  agent exists; every session rejects with `AuthSessionError::NoHandler`.
  Drive the same OAuth flow through
  [`frust-url-launcher`](../url-launcher/README.md)'s ordinary system-browser
  launch instead, with a local loopback HTTP listener (or an equivalent
  out-of-band step) receiving the redirect — RFC 8252's own "loopback IP
  redirection" pattern for platforms with no in-app browser-tab primitive.

---

## 4. One-session contract

At most one `AuthSession` runs at a time, process-wide: starting a second one
while the first is still live resolves immediately to
`AuthSessionError::Busy` rather than queuing or replacing it (an in-app
browser tab is a modal, single-instance UI surface on every backend
platform). The slot frees the moment the live session resolves — by outcome,
by error, or by a backend module failing without ever calling back — so a
stuck session can never wedge every later one behind it.

---

## 5. Deep links vs the future

On Android, the callback URL Custom Tabs redirects back into the app reaches
the platform **twice** once the real Android backend lands: once as the
redirect this crate resolves `AuthSession::start`'s future with, and once
more as an ordinary deep link `frust::deep_links()` also observes (the OS has
no way to know only this crate wants that intent). Treat the future as the
single authoritative source for an auth-session callback, and ignore
`callback_scheme` in your app's own deep-link handling — acting on both
delivers the same authorization code twice.

---

## 6. Device gate

Owed — no device or simulator run has exercised either backend yet (both are
stubs; see § 3). A follow-on task records the Android Custom Tabs and Apple
`ASWebAuthenticationSession` device-gate transcripts once their backend cards
land.
