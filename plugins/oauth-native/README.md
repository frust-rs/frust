# frust-oauth-native

The **protocol half of an RFC 8252 "OAuth 2.0 for Native Apps"** sign-in for
frust apps: PKCE (S256), `state`, the authorization URL, callback validation
(including RFC 9207 `iss`), and the token-grant request bodies and response
parser. It pairs with [`frust-auth-session`](../auth-session/README.md),
which shows the authorization URL in the platform's in-app browser tab and
hands back the raw callback URL.

This is a pure-Rust crate: **no platform code, no HTTP client, no async, no
`frust-*` dependency**. Your app sends the token request with whatever HTTP
client it already uses; this crate builds the body and parses the response.

---

## 1. Add the dependency

```toml
# app Cargo.toml — [dependencies]
frust-oauth-native = { path = "<frust>/plugins/oauth-native" }  # crates.io later
frust-auth-session = { path = "<frust>/plugins/auth-session" }
```

`<frust>` is the path to your frust checkout — derive it from the `frust = {
path = "…" }` line the scaffold already wrote. This crate needs no manifest
permission, Info.plist key or Gradle module; `frust-auth-session` has its
own setup (see its README).

---

## 2. End to end with `frust-auth-session`

### Private-use URI scheme (mobile)

```rust
use frust_auth_session::{AuthSession, AuthSessionOutcome, AuthSessionRequest};
use frust_oauth_native::{
    AuthorizationRequest, CallbackExpectations, IssuerCheck, PkceVerifier, State,
    authorization_code_grant, parse_callback, parse_token_response,
};

const REDIRECT: &str = "com.example.app:/oauth/callback";

// 1. Fresh secrets for this attempt.
let verifier = PkceVerifier::generate()?;
let state = State::generate()?;

// 2. The authorization URL.
let request = AuthorizationRequest {
    authorization_endpoint: "https://as.example/authorize".to_string(),
    client_id: "example-app".to_string(),
    redirect_uri: REDIRECT.to_string(),
    scope: Some("openid profile offline_access".to_string()),
    resources: Vec::new(),
};
let url = request.build(&state, &verifier.challenge())?;

// 3. The in-app browser tab.
let outcome = AuthSession::start(AuthSessionRequest {
    url,
    callback_scheme: "com.example.app".to_string(),
    ephemeral: false,
})
.await?;
let AuthSessionOutcome::Callback(callback_url) = outcome else {
    return Ok(()); // the user cancelled
};

// 4. Validate the callback. `IssuerCheck::from_metadata` reads the server's
//    `issuer` and `authorization_response_iss_parameter_supported`.
let expectations = CallbackExpectations {
    redirect_uri: REDIRECT.to_string(),
    state,
    issuer: IssuerCheck::from_metadata("https://as.example".to_string(), true),
};
let code = parse_callback(&callback_url, &expectations)?;

// 5. The token request — your HTTP client, this crate's body.
let form = authorization_code_grant(&code, REDIRECT, "example-app", &verifier, &[]);
let (status, body): (u16, Vec<u8>) =
    my_http_post("https://as.example/token", form.content_type, &form.body).await?;
let tokens = parse_token_response(status, &body)?;
```

### Loopback redirect (desktop)

On desktop the redirect lands on a loopback listener instead of a custom
scheme (RFC 8252 §7.3). Only the redirect URI's source changes; everything
else is the same as above.

```rust
let session = LoopbackSession::bind(LoopbackOptions::new("/oauth/callback"))?;
let redirect_uri = session.redirect_uri().to_string();

let request = AuthorizationRequest {
    authorization_endpoint: "https://as.example/authorize".to_string(),
    client_id: "example-app".to_string(),
    redirect_uri: redirect_uri.clone(),
    scope: Some("openid profile offline_access".to_string()),
    resources: Vec::new(),
};
let url = request.build(&state, &verifier.challenge())?;

let outcome = session.start(&url).await?;
let AuthSessionOutcome::Callback(callback_url) = outcome else {
    return Ok(()); // the user cancelled
};

let expectations = CallbackExpectations {
    redirect_uri,
    state,
    issuer: IssuerCheck::from_metadata("https://as.example".to_string(), true),
};
let code = parse_callback(&callback_url, &expectations)?;
```


`parse_callback` compares the callback with the redirect URI exactly, except
that the scheme and (for `http`/`https`) the host compare case-insensitively,
so the port the listener bound must match.

### Refreshing

```rust
use frust_oauth_native::refresh_token_grant;

let form = refresh_token_grant(&refresh_token, "example-app", None, &[]);
```

---

## 3. Security notes

- **S256 only.** There is no `plain` PKCE method anywhere in the API. A
  generated verifier is 64 characters (384 bits).
- **`state` is 256 bits** from the OS random source, and the callback
  compares it in constant time.
- **The `iss` policy has no skip.** `IssuerCheck::Required` demands a
  matching `iss`; `IssuerCheck::IfPresent` accepts its absence but still
  rejects a mismatch (RFC 9207 §2.4). Use `from_metadata` so the server's
  advertised support decides.
- **Checks run in a fixed order:** redirect URI → duplicate parameters →
  `iss` → `state` → error response → code. An error response carrying the
  wrong `state` is reported as `StateMismatch`, never as a typed
  authorization error, so a forged error redirect cannot impersonate the
  server.
- **No HTTP transport by design.** The app owns TLS and the HTTP client;
  this crate never sees the network.
- **Secrets stay out of logs.** Every secret-bearing type redacts itself in
  `Debug`, and no error message prints the code, the `state`, the callback
  URL or a token response body.
- **Store refresh tokens in
  [`frust-secure-storage`](../secure-storage/README.md)**, never in plain
  preferences. Keep the verifier and `state` in memory for one attempt only.

---

## 4. Standards

- [RFC 6749](https://www.rfc-editor.org/rfc/rfc6749) — OAuth 2.0 (authorization code grant, error responses, refresh)
- [RFC 7636](https://www.rfc-editor.org/rfc/rfc7636) — PKCE
- [RFC 8252](https://www.rfc-editor.org/rfc/rfc8252) — OAuth 2.0 for Native Apps
- [RFC 8707](https://www.rfc-editor.org/rfc/rfc8707) — Resource Indicators
- [RFC 9207](https://www.rfc-editor.org/rfc/rfc9207) — Authorization Server Issuer Identification
- [RFC 3986](https://www.rfc-editor.org/rfc/rfc3986) — URI syntax (percent-encoding)
