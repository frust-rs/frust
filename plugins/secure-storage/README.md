# frust-secure-storage

A platform-independent, **secure** key-value store for frust apps — iOS/macOS
Keychain, Android Keystore (AES-256-GCM), Linux/Windows via the `keyring` stack
— with an **optional per-store biometric gate** (Face ID / Touch ID on Apple,
`BiometricPrompt` on Android). It is the secure sibling of
`frust-shared-preferences`: same sync, typed, `#[cfg]`-routed backend model,
but every value is encrypted at rest and a store may require the user to
authenticate before it can be read or written.

Like every frust **platform plugin**, this crate is added to your app's own
`Cargo.toml` alongside `frust` (the pubspec model) — the `frust` facade does
not re-export it.

> **Templates stay clean.** A generated frust project ships **no**
> secure-storage code, permissions, or plist keys. You add exactly the lines
> below by hand — or let the frust TUI's **Add Plugin** dialog apply them for
> you (it automates every step in this document). Nothing here is scaffolded,
> so an app that never uses the biometric gate never carries an unused
> `NSFaceIDUsageDescription` or `USE_BIOMETRIC` permission. On Android the
> plugin's platform code ships as its own Gradle library module rather than as
> files copied into your app, so there is nothing to keep in sync by hand.

---

## 1. Add the dependency (always)

The **only** required step for plain (non-gated) secure storage is the
dependency line:

```toml
# app Cargo.toml — [dependencies]
frust-secure-storage = { path = "<frust>/plugins/secure-storage" }  # crates.io later
```

`<frust>` is the path to your frust checkout — derive it from the `frust = {
path = "…" }` line the scaffold already wrote. That's it: with just this line,
plain stores work on every platform.

```rust
use frust_secure_storage::SecureStorage;

let store = SecureStorage::open("credentials")?;
store.set("api_token", "s3cr3t")?;
let token = store.get("api_token")?;        // Some("s3cr3t")
```

**Storage-only apps stop here.** No manifest, plist, permission, or Kotlin
file is needed.

---

## 2. Biometric gate — extra setup, only when you use it

A gated store is opened with `AuthPolicy::Required(AuthOptions { … })`:

```rust
use frust_secure_storage::{
    AuthOptions, AuthPolicy, PromptSpec, SecureStorage, StoreOptions,
};

let store = SecureStorage::open_with(
    "credentials",
    StoreOptions {
        auth: AuthPolicy::Required(AuthOptions {
            allow_device_credential: false,   // biometrics only (no PIN fallback)
            invalidate_on_enrollment: true,   // safe default: re-enrollment voids the key
            validity: None,                   // prompt on every gated call
            prompt: PromptSpec {
                title: "Unlock credentials".into(),
                subtitle: "Authenticate to continue".into(),   // Android only
                negative_button: "Cancel".into(),              // Android only
            },
        }),
        ..Default::default()
    },
)?;
```

Every `get`/`set` on a gated store then **blocks on the system biometric
prompt**. That requires the platform additions below.

### 2a. Android biometric setup

**One addition: include this plugin's Android library module.** The framework
`AuthenticationCallback` is an abstract class JNI cannot subclass, so the
installed app must carry a small Kotlin helper — but you never copy a file. The
helper ships inside `plugins/secure-storage/platform/android`, a
`com.android.library` module (`namespace = "dev.frust.securestorage"`) that also
declares the `USE_BIOMETRIC` permission in its own manifest (the merger folds it
into your app) and the R8 keep rule in its own `consumer-rules.pro` (your
`proguard-rules.pro` stays untouched). Nothing to copy means nothing to drift.

The TUI Add Plugin dialog (`frust tui` → Add Plugin → secure-storage →
`biometric-gate`) makes both edits below for you, idempotently. By hand:

1. `android/settings.gradle.kts` — include the module by path, and redirect its
   build directory so two apps can share one frust checkout:

   ```kotlin
   include(":frust-secure-storage")
   project(":frust-secure-storage").projectDir =
       file("<frust checkout>/plugins/secure-storage/platform/android")

   gradle.lifecycle.beforeProject {
       if (path == ":frust-secure-storage") {
           layout.buildDirectory.set(rootDir.resolve("build/frust-secure-storage"))
       }
   }
   ```

   The path is derived the same way `gradle.properties`' `frust.embedding.dir`
   is — one machine-specific line, replaced by a Maven coordinate once the
   modules publish.

2. `android/app/build.gradle.kts` — depend on it:

   ```kotlin
   dependencies {
       implementation(project(":frust-secure-storage"))
   }
   ```

The class name `dev.frust.securestorage.FrustBiometric` is a hard contract — the
Rust backend looks it up through the application classloader. If the module
isn't wired in, a gated open returns a typed `NotAvailable(HelperMissing)`,
never a crash.

The framework `BiometricPrompt` path requires **API 28+**; on API 26–27 a
gated open returns `NotAvailable(UnsupportedApiLevel)` (frust's minSdk is 26).
No third-party Gradle dependency and no `FragmentActivity` are needed — the
module above pulls in nothing (not even `:frust-embedding`), and the plugin uses
the application `Context` frust already provides.

### 2b. iOS biometric setup

One addition — add to `Info.plist` **only when you use the gate** (an unused
`NSFaceIDUsageDescription` invites App Store review questions):

```xml
<key>NSFaceIDUsageDescription</key>
<string>Unlock your stored credentials.</string>
```

No entitlements are needed for single-app Keychain use. On macOS/iOS the gate
is the Keychain's own `SecAccessControl` + `LAContext` — the system renders
the Face ID/Touch ID dialog during the read; there is no UI code to write.

---

## 3. Never call a gated store on the UI thread

A gated `get`/`set` **blocks** while the system prompt is up. Calling it on the
UI thread freezes the frame loop. Pair it with `frust-reactive`'s
`spawn_blocking` (an app-tier concern — the plugin itself stays framework-free
per the platform-plugin charter):

```rust
// In your app (which already depends on `frust` / `frust-reactive`):
let token = frust_reactive::spawn_blocking(move || {
    let store = SecureStorage::open_with("credentials", opts)?;
    store.get("api_token")           // blocks here on the biometric prompt
}).await??;
```

`can_authenticate()` is a **non-blocking** availability probe (it never shows a
prompt) — safe to call anywhere to decide whether to offer a gated flow.

---

## 4. Caveats

- **`frust create --overwrite` destroys these additions.** `--overwrite`
  re-renders the generated project wholesale, silently dropping the Gradle
  include, the module dependency and the plist key. All additions are
  **idempotent** — just re-run the Add Plugin dialog (or re-apply this
  document) to restore them. (The helper class, permission and keep rule live
  in the plugin's own module, so those survive — but a project that no longer
  includes the module can't reach them.)
- **Android backup/restore data loss.** Auto-backup restores the ciphertext
  XML but never the Keystore key, so restored entries are undecryptable.
  Exclude the store's `frust.ss.<name>` SharedPreferences file from backup
  (backup rules), or accept that gated data does not survive device transfer.
- **Biometric re-enrollment invalidates the key.** With the default
  `invalidate_on_enrollment: true`, adding/removing a fingerprint or face voids
  the store's key; the next read returns `KeyInvalidated`. Recovery is to
  `remove` and `set` the affected entries. Set `invalidate_on_enrollment:
  false` to opt out (weaker posture).
- **Physical device required.** The iOS Simulator cannot render the biometric
  prompt and the Android emulator needs an enrolled fingerprint; the gate is a
  physical-device feature.
- **Apple `set()` overwrite protection is asymmetric by store kind.** A
  **plain** store's overwrite can never downgrade an existing item's
  protection — it updates only the value data, leaving a pre-existing
  gated item's `SecAccessControl` (or a plain item's accessibility) exactly
  as it was. A **gated** store's overwrite instead re-asserts protection on
  every write (Keychain delete + re-add, since `kSecAttrAccessControl`
  can't be mutated in place) — so a key first written while the store was
  plain gets the gate the moment it's rewritten through a gated store. The
  residual limitation: a key that is only ever **read**, never rewritten,
  after switching a store from plain to gated keeps its old, unprotected
  posture indefinitely — the gate only re-asserts on a write. Retroactively
  protecting an existing store means rewriting (`get` + `set`) every key,
  not just reopening it gated.

---

## 5. The TUI Add Plugin dialog automates all of this

Everything in §1 and §2 — the Cargo.toml dep, the `:frust-secure-storage`
Gradle include plus its app-module dependency, and the plist key — is applied for
you, idempotently, by the frust TUI's **Add Plugin** dialog (select
`secure-storage`, tick the *biometric gate* option). This README is the manual
contract that dialog encodes.
