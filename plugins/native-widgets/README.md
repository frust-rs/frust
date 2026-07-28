# frust-native-widgets

Render **real OS controls** from pure Rust — `native_button("Save")`,
`native_label(...)`, `native_switch(checked)`, `native_slider(...)`,
`native_progress(...)`, `native_image(bytes)` — composed straight into a
frust `View` tree like any other widget. Each control is backed by a genuine
Android `View` (`Button`/`TextView`/`Switch`/`SeekBar`/`ProgressBar`/`ImageView`)
or UIKit view (`UIButton`/`UILabel`/`UISwitch`/`UISlider`/`UIProgressView`/
`UIImageView`), hosted as a **platform-view slot** (`docs/ARCHITECTURE.md`'s
Platform-view flow) — frust paints nothing for it, the OS composites it in
place.

Like every frust **platform plugin**, this crate is added to your app's own
`Cargo.toml` alongside `frust` (the pubspec model) — the `frust` facade does
not re-export it.

> **Templates stay clean.** A generated frust project ships **no**
> native-widgets code, permissions, or plist keys. You add exactly the lines
> below by hand — or let the frust TUI's **Add Plugin** dialog apply them for
> you (it automates every step in this document). On Android the plugin's
> Kotlin ships as its own Gradle library module rather than as files copied
> into your app, so there is nothing to keep in sync by hand; on iOS there is
> nothing to add at all beyond the dependency.

---

## 1. What you get

Six controls, one Rust API, no Kotlin or Swift to write for any of them:

| Builder | Android view | iOS view |
|---|---|---|
| `native_button(text)` | `Button` | `UIButton` |
| `native_label(text)` | `TextView` | `UILabel` |
| `native_switch(checked)` | `Switch` | `UISwitch` |
| `native_slider(value, min, max)` | `SeekBar` | `UISlider` |
| `native_progress(value, min, max)` | `ProgressBar` | `UIProgressView` |
| `native_image(bytes)` | `ImageView` | `UIImageView` |

…plus `native_component`, the generic mounting seam for a control this crate
does not ship: `impl NativeComponent` (with your own typed `Props`) →
`register_component::<C>(KIND)` → `native_component(KIND, c, props)`, mounted
through the same one factory and the same runtime the six builders use, with
no per-component Kotlin or Swift anywhere in it. See the `api::mount` module
docs.

> **Two limits on that seam — read them before you plan around it.** You
> cannot implement `NativeComponent` from an app crate today (it needs raw
> `jni`/`objc2-ui-kit` dependencies this crate does not re-export), and a
> component's native view is **display-only** — no event ever reaches your
> `on_event`. Both are spelled out in §5, and neither applies to the six
> builders above.

Every control follows frust's **controlled-component** convention where the
platform allows it: a switch/slider reports the *requested* value through its
`on_...` callback, and the value you pass back down on the next rebuild is
what actually shows — the same contract `Checkbox`/`Slider` follow in
`frust-widgets` (`docs/CODE_STANDARDS.md`'s Interaction Semantics).

A native control's tap/toggle/drag bypasses `frust-core`'s `EventCtx`
entirely — it fires straight from the platform's own listener
(`View.OnClickListener` on Android, target-action on iOS) into a registered
Rust callback. That means none of `frust-core`'s interaction contract
(capture, focus, fire-on-up-inside, `Cancel`-never-mutates-state) applies to
a hosted native control — this is inherent to hosting real platform widgets,
not a gap.

```rust
use frust::RwSignal;
use frust_native_widgets::native_button;

// A native control's callback takes no app-state parameter — it fires
// straight from the platform's own listener, so it closes over a signal
// instead (the events-as-signals idiom every builder follows):
fn save_button(saved: RwSignal<bool>) -> impl frust::View<AppState> {
    native_button("Save")
        .size(160.0, 48.0)
        .on_press(move || saved.set(true))
}
```

---

## 2. Add the dependency (always)

```toml
# app Cargo.toml — [dependencies]
frust-native-widgets = { path = "<frust>/plugins/native-widgets" }  # crates.io later
```

`<frust>` is the path to your frust checkout — derive it from the `frust = {
path = "…" }` line the scaffold already wrote.

This is the **only** step iOS needs — see §4.

---

## 3. Android setup — include the plugin's Gradle module

**One addition: include this plugin's Android library module.** It carries
both of the plugin's Kotlin classes — `dev.frust.nativewidgets`'s
`FrustNativeControlFactory` (the ONE platform-view factory every control
resolves through) and `FrustNativeListener` (the ONE listener class every
control's events funnel through) — plus the two R8 keep rules they need, in
its own `consumer-rules.pro`. Your `AndroidManifest.xml` and
`proguard-rules.pro` stay untouched, and there is no file to copy, so nothing
can drift.

The TUI Add Plugin dialog (`frust tui` → Add Plugin → native-widgets) makes
both edits below for you, idempotently. By hand:

1. `android/settings.gradle.kts` — include the module by path, and redirect
   its build directory so two apps can share one frust checkout:

   ```kotlin
   include(":frust-native-widgets")
   project(":frust-native-widgets").projectDir =
       file("<frust checkout>/plugins/native-widgets/platform/android")

   gradle.lifecycle.beforeProject {
       if (path == ":frust-native-widgets") {
           layout.buildDirectory.set(rootDir.resolve("build/frust-native-widgets"))
       }
   }
   ```

   The path is derived the same way `gradle.properties`' `frust.embedding.dir`
   is — one machine-specific line, replaced by a Maven coordinate once the
   modules publish.

2. `android/app/build.gradle.kts` — depend on it:

   ```kotlin
   dependencies {
       implementation(project(":frust-native-widgets"))
   }
   ```

No permission is required: hosting an `android.widget` view needs none.

**Both class names are hard contracts.**
`dev.frust.nativewidgets.FrustNativeControlFactory` is the platform-view
`viewType` string this crate publishes (the embedding's `FrustViewHost`
resolves it reflectively through the application classloader, which is also
why the package keeps its `dev.frust.` prefix), and both class names are
baked into this plugin's four JNI export symbols
(`Java_dev_frust_nativewidgets_*`). If the module isn't wired in, the host
cannot resolve the factory and every native control renders as an empty slot.

---

## 4. iOS setup — nothing. Zero Swift, by design.

iOS needs **no platform addition at all** beyond §2's Cargo dependency: no
Swift package reference, no Xcode project edit, no `Info.plist` key. The
factory that would be a Swift class on every other plugin's Apple arm is
instead a Rust `objc2` `define_class!` type, registered straight into the
Objective-C runtime from this crate's own code and resolved by
`NSClassFromString` under the bare runtime name `FrustNativeControlFactory`
(`docs/CODE_STANDARDS.md`'s Naming Conventions: iOS factories carry no
package prefix — the opposite convention from Android's fully-qualified
FQCN). This was proven out in Phase 0's spike 2 and has held for every
control added since.

---

## 5. Caveats

- **`frust create --overwrite` destroys these additions.** `--overwrite`
  re-renders the generated project wholesale, silently dropping the Gradle
  include and the module dependency. Both additions are **idempotent** — just
  re-run the Add Plugin dialog (or re-apply §3) to restore them. The Kotlin
  classes themselves live in the plugin's own module, so they survive — but a
  project that no longer includes the module can't reach them.
- **A wrong or missing module wiring fails at runtime, not at build time.**
  Nothing in the Rust build references the Kotlin classes, so a missing
  `include(...)` compiles cleanly and surfaces on device as a blank slot
  where the control should be (the host logs an unresolvable factory).
- **No cargo gate compiles Kotlin.** `cargo check --target
  aarch64-linux-android -p frust-native-widgets` type-checks the Rust half
  only; a typo in the module's `.kt` files surfaces solely in a real Gradle
  build (`docs/DEVELOPMENT.md`). `plugins/native-widgets/tests/
  kotlin_conformance.rs` covers the one thing a source scan can: that the
  Kotlin listener's `KIND_*` constants and value-packing arithmetic still
  agree with `src/events.rs`'s.
- **Events bypass frust's event pipeline** — see §1. A native control is not
  reachable by frust's pointer capture/focus machinery, and a frust widget
  drawn over an interactive slot needs an explicit input `shield(...)`
  (`docs/ARCHITECTURE.md`'s Platform-view flow).
- **You cannot implement `NativeComponent` from an app crate today** (§1's
  `native_component` seam only). `create` has to construct real native views,
  which means naming `jni::objects::JObject` on Android and `objc2-ui-kit`'s
  classes on iOS *in your own crate*; this plugin re-exports neither FFI
  crate, and `docs/CODE_STANDARDS.md` sanctions a
  `frust-core`/`kurbo`/`peniko` escape hatch only for an `examples/*` app. So
  the practical audience today is **plugin authors, not app authors** — the
  only implementor in this repo is this crate's own non-default
  `demo-components` composite. Closing the gap (re-exporting a curated
  view-construction surface, or the FFI crates themselves) is a separate,
  unscheduled decision. The six builders in §1 are unaffected: they are
  ordinary Rust calls needing no FFI dependency of yours.
- **A `NativeComponent` is display-only — its `on_event` never fires.** The
  dispatch half is wired and unit-tested (runtime → bridge → trait method),
  but nothing in production ever attaches a platform listener to a view a
  component built — its root as much as its children — because both listener
  objects are constructed from a slot id `ComponentCtx` never exposes, and
  the mounting builder registers no callback. Overriding `on_event` therefore
  has no effect in this build; it is a deliberately deferred **Phase 4** gap.
  Marking a component `.interactive()` still routes touches to the native
  view, so it behaves natively (a button highlights) — it just reports
  nothing back to Rust. The six built-in controls are unaffected: their
  `on_press`/`on_change` callbacks fire normally (§1).

---

## 6. The TUI Add Plugin dialog automates all of this

Everything in §2 and §3 — the Cargo.toml dependency and the
`:frust-native-widgets` Gradle include plus its app-module dependency — is
applied for you, idempotently, by the frust TUI's **Add Plugin** dialog
(select `native-widgets`). This README is the manual contract that dialog
encodes.
