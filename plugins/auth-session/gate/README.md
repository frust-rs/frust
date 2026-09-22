# frust-auth-session gate pages

Two static pages the playground app's "Auth" section (`examples/playground/src/pages/auth_session.rs`)
drives an [`AuthSession`](../src/lib.rs) round trip against:

- **`callback.html`** — reads a `?scheme=` query parameter (falling back to `frustplay`, the scheme
  the playground app registers), validates it against a custom-scheme allow-list (see the page's own
  script for the rule), and redirects to `<scheme>://auth/callback?code=x&state=gate`, simulating
  an identity provider's successful authorization redirect. Unsupported schemes render a visible
  error without any link. A fallback link covers a browser that refuses the automatic redirect — Chrome, for one, will not follow a script-initiated custom-scheme navigation without a user gesture and shows a *Continue to <app>?* prompt instead; if that prompt is dismissed, tap the link.
- **`cookie.html`** — sets a `frustauth=1` cookie if none is present yet, then prints the full
  `document.cookie` string (or `no cookie`) in a large `<pre>` block. Useful two ways: as a page a tester
  manually dismisses the in-app browser tab from (any page works for that), and as a way to confirm
  cookies persist across a non-ephemeral session and do not across an ephemeral one.

## Hosting

Either page works from any `https` origin — [`AuthSession::start`](../src/lib.rs) requires an absolute
`https` URL, nothing about the page content is origin-specific. Two options:

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
