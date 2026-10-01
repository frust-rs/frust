# frust-auth-session gate pages

Three static pages the playground app's "Auth" section (`examples/playground/src/pages/auth_session.rs`)
drives a round trip against: two for an [`AuthSession`](../src/lib.rs) custom-scheme session, one for
a desktop [`LoopbackSession`](../src/loopback.rs) session paired with
[`frust-oauth-native`](../../oauth-native/README.md)'s PKCE/`state`/callback builders.

- **`callback.html`** — reads a `?scheme=` query parameter (falling back to `frustplay`, the scheme
  the playground app registers), validates it against a custom-scheme allow-list (see the page's own
  script for the rule), and redirects to `<scheme>://auth/callback?code=x&state=gate`, simulating
  an identity provider's successful authorization redirect. Unsupported schemes render a visible
  error without any link. A fallback link covers a browser that refuses the automatic redirect — Chrome, for one, will not follow a script-initiated custom-scheme navigation without a user gesture and shows a *Continue to <app>?* prompt instead; if that prompt is dismissed, tap the link.
- **`cookie.html`** — sets a `frustauth=1` cookie if none is present yet, then prints the full
  `document.cookie` string (or `no cookie`) in a large `<pre>` block. Useful two ways: as a page a tester
  manually dismisses the in-app browser tab from (any page works for that), and as a way to confirm
  cookies persist across a non-ephemeral session and do not across an ephemeral one.
- **`loopback.html`** — a fake authorization server for the desktop loopback flow, never a real
  identity provider. It reads `redirect_uri`, `state`, `gate_iss`, the optional `gate_mode`, and the
  PKCE `code_challenge`/`code_challenge_method` the playground's `AuthorizationRequest::build` already
  appended. It renders a visible `bad redirect_uri` error (and stops) unless `redirect_uri` matches
  the RFC 8252 §7.3 loopback shape `http://127.0.0.1:<port>/<reserved path>`, and a `bad PKCE
  parameters` error unless `code_challenge_method` is exactly `S256` and `code_challenge` is 43
  characters. Otherwise it dispatches on `gate_mode`: absent (or anything other than `deny`/`stay`)
  redirects to `redirect_uri?code=gatecode&state=<state>&iss=<gate_iss>` (the success path);
  `gate_mode=deny` redirects to `redirect_uri?error=access_denied&error_description=gate+deny&
  state=<state>&iss=<gate_iss>` instead; `gate_mode=stay` never redirects at all, rendering `waiting
  (close this tab to test timeout/cancel)` — the fixture for the app side's listener-timeout and
  cancel exit paths. Every redirect also carries a fallback link, for the same Chrome
  script-initiated-navigation prompt `callback.html`'s own section above documents (loopback
  redirects are plain `http`, not a custom scheme, so most browsers follow them automatically in
  practice, but the link costs nothing to keep).

## Hosting

Every page works from any `https` origin — [`AuthSession::start`](../src/lib.rs) and
[`LoopbackSession::start`](../src/loopback.rs) both require an absolute `https` authorization URL,
nothing about the page content is origin-specific. Two options:

1. **A real https origin you control** — e.g. serve this directory under `https://frust.dev/gate/auth-session/`
   (or any other host) and point the playground app's `Base URL` field at that origin. This is the
   default the app ships with.
2. **Zero-infra: httpbin.org** — no hosting needed at all, using `httpbin.org`'s own redirect/cookie
   endpoints in place of the two files above:
   - Callback: `https://httpbin.org/redirect-to?url=frustplay%3A%2F%2Fauth%2Fcallback%3Fcode%3Dx%26state%3Dgate`
   - Cookie set: `https://httpbin.org/cookies/set?frustauth=1`
   - Cookie read-back: `https://httpbin.org/cookies`

   Paste the relevant URL directly into the app's `Base URL` field (with an empty trailing path) plus the
   button's own filename, or just swap the button's target for the full httpbin URL when running the gate
   by hand.

## Manual gate script

Run these four steps from the playground app's "Auth" section (`Base URL` pointed at a hosted copy of
this directory, or the httpbin equivalents above):

1. **Callback.** Tap `Callback`. Expected: the in-app browser tab (Custom Tabs on Android,
   `ASWebAuthenticationSession` on iOS/macOS) presents `callback.html`, redirects immediately, and the
   status line reads `Callback -> Ok(Callback("frustplay://auth/callback?code=x&state=gate"))` (the
   playground prints the URL through the outcome value; the plugin's own `Debug` output would show
   `Callback { url_len: 43 }`). The
   `latest deep link:` line below also updates on Android — the same callback URL additionally arrives as
   an ordinary deep link (`frust-auth-session`'s crate doc's *Android double-delivery* section); this is
   expected, not a bug.
2. **Cancel test.** Tap `Cancel test`, then dismiss the presented tab yourself (swipe it away / tap the
   platform close control) without letting it navigate anywhere. Expected: the status line reads
   `Cancel test -> Ok(Cancelled)`.
3. **Busy.** Tap `Busy`. Expected: the status line reads `Busy -> Err(Busy)` — the second of the two
   back-to-back `AuthSession::start` calls this button fires is rejected because the first is still live
   (`frust-auth-session`'s crate doc's *Exactly one live session* section). Dismiss the tab the first call
   opened afterwards to leave the session slot clear for the next step.
4. **Cookie check.** Tap `Cookie check` once with `Ephemeral` off: `cookie.html` should read back
   `frustauth=1` on a second run (the cookie persisted). Toggle `Ephemeral` on and repeat: the session
   should not see a cookie set by an earlier non-ephemeral run (Apple's
   `prefersEphemeralWebBrowserSession`; Android has no first-class equivalent — see
   [`AuthSessionRequest::ephemeral`](../src/lib.rs)'s own doc for that platform's best-effort mapping, if
   any).

Expected per platform:

- **Android / iOS / macOS** (the only targets with a real backend — `AuthSession::is_supported()` reads
  `true`): all four steps behave as described above.
- **Every other target** (desktop Linux/Windows without an Apple backend): every button resolves to
  `Err(NoHandler)` immediately — no platform authentication user agent exists there
  (`plugins/auth-session/src/unsupported.rs`'s own doc documents the recommended fallback).

## Manual gate script — Desktop loopback

The playground's "Desktop loopback" block (below the four buttons above, shown only when
[`LoopbackSession::is_supported`](../src/loopback.rs) reads `true` — desktop Linux, Windows and macOS)
runs these five steps, `Base URL` pointed at a hosted copy of this directory (the httpbin alternative
above has no stand-in for `loopback.html`, so this block needs a real hosted origin):

1. **Loopback login.** Tap `Loopback login`. Expected: the system browser opens `loopback.html`,
   which redirects immediately back to the loopback listener, and the status line reads `Loopback
   login -> code ok len=8, state ok, iss ok, grant body len=<N>` (`gatecode` is 8 characters; `<N>`
   is `authorization_code_grant`'s form body length — proving the builder, since nothing on this page
   ever sends it over HTTP). The `loopback port:` line below updates to the bound port.
2. **Loopback deny.** Tap `Loopback deny`. Expected: the gate page redirects with
   `error=access_denied`, and the status line reads `Loopback deny -> Err(Authorization { error:
   AccessDenied, description: Some("gate deny"), uri: None })`.
3. **Loopback bad iss.** Tap `Loopback bad iss`. Expected: the gate page's redirect succeeds, but the
   playground validates it against `<issuer>/wrong` on purpose, so the status line reads `Loopback
   bad iss -> Err(IssuerMismatch)` — the negative control proving the issuer check actually runs.
4. **Loopback timeout, then Loopback cancel.** Tap `Loopback timeout (10 s)`: the gate page shows
   `waiting (close this tab to test timeout/cancel)` and never redirects, so after 10 s the status
   line reads `Loopback timeout (10 s) -> Err(TimedOut)`. Next tap `Loopback cancel` (same `waiting`
   page), then tap `Cancel loopback` before it times out: the status line reads `Loopback cancel ->
   Ok(Cancelled)` within about a second.
5. **Loopback busy.** Tap `Loopback busy`. Expected: the status line reads `Loopback busy ->
   Err(Busy)` — the first bind is still holding the shared slot when the second one is attempted
   (`frust-auth-session`'s crate doc's *Exactly one live session* section, which the loopback and
   custom-scheme backends share); the first session is dropped as soon as the handler returns, so a
   later button press sees a clear slot again.

Expected per platform: **Linux, Windows and macOS** (`LoopbackSession::is_supported()` reads `true`)
run all five steps as described; every other target (Android, iOS) shows the single `loopback: not
supported on this target` line instead of the block, and `AuthSession::is_supported()`'s own line
above still answers for the custom-scheme flow there.
