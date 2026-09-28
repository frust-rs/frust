# frust-native-widgets

Render **real OS controls** from pure Rust — `native_button("Save")`,
`native_label(...)`, `native_switch(checked)`, `native_slider(...)`,
`native_progress(...)`, `native_image(bytes)`, `native_spinner(animating)` —
composed straight into a frust `View` tree like any other widget. Each
control is backed by a genuine Android `View`
(`Button`/`TextView`/`Switch`/`SeekBar`/`ProgressBar`/`ImageView`/a
circular-style `ProgressBar`), UIKit view (`UIButton`/`UILabel`/`UISwitch`/
`UISlider`/`UIProgressView`/`UIImageView`/`UIActivityIndicatorView`), or
AppKit view on macOS (`NSButton`/`NSTextField`/`NSSwitch`/`NSSlider`/
`NSProgressIndicator`/`NSImageView`/an `NSProgressIndicator` in its
`Spinning` style), hosted as a **platform-view slot**
(`docs/ARCHITECTURE.md`'s Platform-view flow) — frust paints nothing for it,
the OS composites it in place.

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

Seven controls, one Rust API, no Kotlin or Swift to write for any of them:

| Builder | Android view | iOS view | macOS view |
|---|---|---|---|
| `native_button(text)` | `Button` | `UIButton` | `NSButton` |
| `native_label(text)` | `TextView` | `UILabel` | `NSTextField` (label) |
| `native_switch(checked)` | `Switch` | `UISwitch` | `NSSwitch` |
| `native_slider(value, min, max)` | `SeekBar` | `UISlider` | `NSSlider` |
| `native_progress(value, min, max)` | `ProgressBar` | `UIProgressView` | `NSProgressIndicator` |
| `native_image(bytes)` | `ImageView` | `UIImageView` | `NSImageView` |
| `native_spinner(animating)` | `ProgressBar` (circular style) | `UIActivityIndicatorView` | `NSProgressIndicator` (`Spinning` style) |

…plus `native_component`, the generic mounting seam for a control this crate
does not ship: `impl NativeComponent` (with your own typed `Props`) →
`register_component::<C>(KIND)` → `native_component(KIND, c, props)`, mounted
through the same one factory and the same runtime the seven builders use,
with no per-component Kotlin or Swift anywhere in it. See the `api::mount`
module docs.

A component hears its own views the way the seven builders do: it attaches the
platform's one listener to any view it built with
`ComponentCtx::attach_listener(view, ListenerKinds::CLICK)` (or `TOGGLED` /
`VALUE_CHANGED`), its `NativeComponent::on_event` receives each event, and
whatever that answers reaches the app on the mounted view's `.on_event(...)`
hook — the same events-as-signals idiom as `.on_press`:

```rust
native_component(KIND, MyCard, props)
    .interactive()
    .on_event(move |event| if event.is_click() { taps.set(taps.get_untracked() + 1) })
```

> **One limit on that seam — read it before you plan around it.** An app
> crate cannot implement `NativeComponent` today (it needs raw
> `jni`/`objc2-ui-kit` dependencies this crate does not re-export). It is
> spelled out in §5, and does not apply to the seven builders above.

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

**Integrated this plugin before the module existed?** Both class names and all
four export symbols changed — see §7 before you rebuild.

---

## 4. iOS and macOS setup — nothing either

Neither Apple platform needs anything beyond §2's Cargo dependency: no Swift
package reference, no Xcode project edit, no `Info.plist` key, and on macOS
no explicit "install" call either.

### iOS — zero Swift, by design

The factory that would be a Swift class on every other plugin's Apple arm is
instead a Rust `objc2` `define_class!` type, registered straight into the
Objective-C runtime from this crate's own code and resolved by
`NSClassFromString` under the bare runtime name `FrustNativeControlFactory`
(`docs/CODE_STANDARDS.md`'s Naming Conventions: iOS factories carry no
package prefix — the opposite convention from Android's fully-qualified
FQCN). This was proven out in Phase 0's spike 2 and has held for every
control added since.

### macOS — the desktop Mode-A host registers the factory lazily

The desktop preview/bundle needs no app-side wiring either: this crate's
AppKit factory (a Rust `frust_plugin::desktop::DesktopViewFactory`) registers
itself with the shell's platform-view registry the first time any builder in
this crate encodes a slot's params — there is nothing for your app to call.

Two things differ from the mobile arms, both by design:

- **The theme ladder follows the app, not the Mac.** Every control's
  `NSAppearance` is pinned to the active `frust::Theme`'s brightness
  (`DarkAqua`/`Aqua`), re-applied on every update — so a Mac running Dark
  Mode still draws a Light-themed app's controls light, and vice versa.
- **A handful of AppKit gaps are logged, not papered over**: `Fit::Cover`
  has no true fill-and-crop on `NSImageView`, so it letterboxes like
  `Fit::Contain` rather than cropping; a slider emits no drag-start/drag-end
  event (AppKit target-action carries no gesture phase, only the final
  value); `NSSwitch`'s track and `NSProgressIndicator`'s fill have no tint
  API at all, so `thumbTint`/`trackTint`/`progressTint` on `native_switch`/
  `native_progress` — and the spinner's own tint on `native_spinner`, which
  also draws through `NSProgressIndicator` — are silent no-ops logged at
  debug (all three still draw in the system accent colour);
  `native_image`'s tint marks the image as a template and sets
  `NSImageView.contentTintColor` — a silhouette in the tint colour, like Android's SRC_IN and iOS's template rendering — and clearing
  the tint restores the original image; and `native_image` decodes through
  ImageIO, so a shell environment whose `DYLD_LIBRARY_PATH` shadows one of
  ImageIO's private codec dylibs (e.g. Homebrew's `/opt/homebrew/lib` on a
  machine with `libpng`/`libjpeg` installed) leaves every image slot empty
  with one logged warning rather than decoding or crashing.

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
  where the control should be (the host logs an unresolvable factory). A stale
  pre-module hand copy produces the same blank slot — see §7.
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
- **An app crate cannot implement `NativeComponent` today** (§1's
  `native_component` seam only). `create` has to construct real native views,
  which means naming `jni::objects::JObject` on Android and `objc2-ui-kit`'s
  classes on iOS *in your own crate*; this plugin re-exports neither FFI
  crate, and `docs/CODE_STANDARDS.md` sanctions a
  `frust-core`/`kurbo`/`peniko` escape hatch only for an `examples/*` app. So
  the practical audience today is **plugin authors, not app authors** — the
  only implementor in this repo is this crate's own non-default
  `demo-components` composite. Closing the gap (re-exporting a curated
  view-construction surface, or the FFI crates themselves) is a separate,
  unscheduled decision. The seven builders in §1 are unaffected: they are
  ordinary Rust calls needing no FFI dependency of yours.
- **A `NativeComponent`'s events come only from listeners it attached.**
  `ComponentCtx::attach_listener` binds the platform's one listener class to
  the component's own slot id — which the context never hands the component —
  and the runtime's bridge delivers only the event families the slot attached,
  so a view the component attached nothing to still behaves natively (a
  button highlights) but reports nothing. Two children attached for the same
  family are indistinguishable in `on_event`: a click carries its slot, not
  which child fired, so give each child its own family or its own slot. On
  macOS an `NSControl` carries one target/action pair, so it takes exactly one
  family (`CLICK`, `TOGGLED` or `VALUE_CHANGED`), and AppKit reports no drag
  edges. The listener is released when the `ListenerHandle` the component
  keeps in its state drops.

---

## 6. The TUI Add Plugin dialog automates all of this

Everything in §2 and §3 — the Cargo.toml dependency and the
`:frust-native-widgets` Gradle include plus its app-module dependency — is
applied for you, idempotently, by the frust TUI's **Add Plugin** dialog
(select `native-widgets`). This README is the manual contract that dialog
encodes.

---

## 7. Migrating from the v1 hand copy (breaking, one time)

Before this plugin shipped its own Gradle module, its two generic Kotlin
classes lived in the **bare `dev.frust` package** and the documented
integration was to **hand-copy** `FrustNativeControlFactory.kt` and
`FrustNativeListener.kt` into your own app module, under
`app/src/main/kotlin/dev/frust/`. That shape is gone, and the rename it
required is breaking:

| | v1 (hand copy) | now |
|---|---|---|
| Kotlin package | `dev.frust` | `dev.frust.nativewidgets` |
| factory `viewType` | `dev.frust.FrustNativeControlFactory` | `dev.frust.nativewidgets.FrustNativeControlFactory` |
| JNI export symbols | `Java_dev_frust_FrustNative*` | `Java_dev_frust_nativewidgets_FrustNative*` |
| how the Kotlin ships | copied into your app module | this plugin's own Gradle library module (§3) |

**Why.** The bare `dev.frust` package belongs exclusively to the embedding
module (`docs/CODE_STANDARDS.md`'s Plugin Conventions); this plugin held a
time-boxed exception only because a Kotlin package is baked verbatim into a
JNI export's mangled symbol name. Shipping a real Gradle library module — the
shape every other plugin already uses — ended the exception, and moving the
package *had* to move all four export symbols with it.

**What to do**, in this order:

1. **Delete your copies** of `FrustNativeControlFactory.kt` and
   `FrustNativeListener.kt` (plus the `dev/frust/` directory they sat in, if
   nothing else of yours lives there). They are dead weight now, and worse
   than dead — see the symptom below.
2. **Apply §3's module wiring**: the `include(":frust-native-widgets")` +
   `projectDir` lines in `android/settings.gradle.kts` and the
   `implementation(project(":frust-native-widgets"))` line in
   `android/app/build.gradle.kts`. The TUI's Add Plugin dialog makes both edits
   idempotently.
3. Rebuild. Nothing of yours ever referenced either class by name, so there is
   nothing else to change — no manifest edit, and no keep rule to add (the
   module ships its own `consumer-rules.pro` for the new names). A keep rule
   *you* added for the old `dev.frust.FrustNative*` classes can go too; it
   matches nothing once the copies are deleted.

iOS is unaffected: there was never anything to copy there (§4).

**The symptom if you skip this.** Nothing fails at build time — Kotlin happily
compiles an `external fun` whose native symbol does not exist, and no cargo
gate compiles Kotlin at all (§5) — so it lands on device the first time a
native control is created, and it looks like **a blank slot with no obvious
cause**:

- **Copies kept, module not wired:** the host resolves the factory by the new
  FQCN, your app only has the old `dev.frust.` one, so `Class.forName` fails —
  `logcat` (tag `frust`) shows `platform-view factory
  'dev.frust.nativewidgets.FrustNativeControlFactory' failed to resolve` and
  the slot is marked dead. Every native control is an empty hole.
- **Anything that still reaches a stale copy** dies with `UnsatisfiedLinkError:
  No implementation found for … Java_dev_frust_FrustNativeControlFactory_nativeCreateControl`:
  the JVM resolves a `native`/`external` method by mangled name alone, and that
  name is no longer in the `.so`. The host wraps every factory call in `catch
  (Throwable)`, so this too becomes a dead slot plus one `logcat` warning
  rather than a crash — which is exactly why the cause is not obvious from the
  screen.

Both failures are silent by design (a misbehaving platform-view factory must
never take down the frame loop), so `adb logcat -s frust` is the place to
confirm which one you have.
