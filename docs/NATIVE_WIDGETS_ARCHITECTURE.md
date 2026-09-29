# Frust - NATIVE_WIDGETS Architecture

## Overview

NATIVE_WIDGETS (`plugins/native-widgets`) is a platform plugin rendering real OS controls from pure
Rust: eleven built-in controls in three membership tiers (eight shared by every platform arm, two
Apple-only, one iOS-only — see *Platform Matrix*), arbitrary plugin-authored native view hierarchies
through the public `NativeComponent` trait, and native presentations — host-owned modal UI (an
alert everywhere, a sheet on iOS/iPadOS) requested imperatively and resolved to exactly one outcome,
never a node in the `View` tree. A builder whose control has no arm on the current target renders a
frust-drawn refusal banner, decided at compile time — never an empty slot.

Every control is driven through exactly one generic factory and one generic listener per platform,
in three arms:

- **Android** — the Kotlin pair `FrustNativeControlFactory` (create/update/dispose) and
  `FrustNativeListener` (every listener interface a control needs), shipped as their own Gradle
  library module and driven by four frozen JNI exports. **Mode B**: the differ punches a transparent
  hole in the frust surface and the native view is composited into it.
- **iOS/iPadOS** — a Rust-registered ObjC factory class (`apple/factory.rs`, resolved by the
  embedding through `NSClassFromString`) plus one Rust target class, `FrustNativeControlTarget`
  (`apple/events.rs`), which carries every control's target-action selector and also conforms to
  `UITabBarDelegate` for the tab bar — zero Swift. Also **Mode B**.
- **macOS** — one Rust `frust_plugin::desktop::DesktopViewFactory` (`appkit/factory.rs`, registered
  lazily under a `view_type` string) plus one `define_class!` target class carrying a single action
  selector (`appkit/events.rs`), hosted by `crates/frust-shell-macos`' desktop platform-view host.
  **Mode A**: the frust surface stays opaque and each native view is an AppKit sibling composited
  *above* it — no punch, no Mac Catalyst.

A new control extends the existing dispatch rather than adding per-control platform glue.
`examples/native-widgets-demo` is the plugin's showcase and device-gate vehicle — every control
beside its frust-drawn peer, one page per widget family.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how NATIVE_WIDGETS relates to the other units.

## Platform Matrix

Membership is per arm, not per vendor: `controls/mod.rs`'s `SHARED_KINDS`/`APPLE_KINDS`/
`IOS_ONLY_KINDS` tables are what each arm's `register_controls` registers, and `api/builders.rs`'s
`SEGMENTED_ARM`/`STEPPER_ARM`/`TAB_BAR_ARM` constants are what select a builder's refusal banner.

| Control / surface | Android | iOS/iPadOS | macOS |
|-------------------|---------|------------|-------|
| Button, Label, Switch, Slider, ProgressBar, Image, Spinner, DatePicker (`SHARED_KINDS`) | native | native | native |
| Segmented, Stepper (`APPLE_KINDS`) | refusal banner | native | native |
| TabBar (`IOS_ONLY_KINDS`) | refusal banner | native (a bare `UITabBar`) | refusal banner |
| `NativeComponent` (incl. the `demo-components` `DemoCard`) | native | native | native |
| Alert (`show_native_alert`) | `android.app.AlertDialog` | `UIAlertController` | `NSAlert` window sheet |
| Sheet (`show_native_sheet`) | `PresentError::Unsupported` | page sheet | `PresentError::Unsupported` |

Desktop Linux/Windows and web have no arm at all: no native view is created, the Apple-only and
iOS-only builders render their banner, and both presentation kinds answer
`PresentError::Unsupported`. The refusals and their rationale (no framework segmented control or
stepper on Android and no AndroidX/Material dependency assumed; no bottom-tab-bar idiom on macOS)
are [LIMITATIONS.md](LIMITATIONS.md)'s `native-widgets-segmented-stepper-apple-only`.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `runtime` / `registry` | Internal type-erased dispatch engine and retained-instance registry driving the create/update/dispose/event lifecycle. Its `NativeCtx`/`NativeView` pair is real on each platform arm and a recording stand-in on a host with none, so the shared dispatch/diff/lifecycle contract is exercised by host tests on Linux/Windows/web only (each arm swaps in its own real types; the gate commands are [DEVELOPMENT.md](DEVELOPMENT.md)'s) |
| `controls/` | The eleven built-in controls (`button`, `label`, `switch`, `slider`, `progress`, `image`, `spinner`, `date_picker`, `segmented`, `stepper`, `tab_bar`), each one file: a platform-agnostic, host-tested `Props`/`decode`/`plan` half plus one `mod platform` per arm it supports. `mod.rs` owns the three kind tables (append-only), the flat-params wire keys and the setter cost tiers |
| `events.rs` | The typed event vocabulary (`EventPayload`) and the primitive `(kind, detail)` codec every arm's listener or target packs into; the `EVENT_KIND_*` constants (1-8) are mirrored verbatim by `FrustNativeListener.kt`'s `KIND_*` |
| `component.rs` | Public `NativeComponent` trait, `ComponentCtx`, `ListenerKinds`/`ListenerHandle`, `register_component`, and the crate-private `Bridge<C>` routing a third-party component through the same runtime as the built-in controls |
| `api/` | App-facing layer behind the default-on `frust-api` feature: `builders.rs` (the eleven builders, the refusal banner, the translucency fallback, and `native_tab_bar`'s inset-aware `TabBarSlot` — 49pt plus the window's bottom safe-area inset), `mount.rs` (`native_component`), `present.rs` (`show_native_alert`/`_into`, `show_native_sheet`/`_into`), `signals.rs` (events-as-signals) and `theme.rs` (theme folding) |
| `present/mod.rs` | The presentation substrate, platform-independent: `AlertSpec`/`SheetSpec` and their outcome types, `PresentError`, the `Presentation<T>` future and its generation-guarded one-shot channel (`oneshot.rs`), the process-wide Busy slot (`ActiveSlot`), and the `AlertHost`/`SheetHost` seams selected per OS by `cfg` — including `UnsupportedSheet`, the sheet arm of every non-iOS target |
| `present/apple_host.rs` | Shared by every Apple arm: `on_main` (the main-queue hop), `presenting_anchor` (host discovery — the topmost view controller on iOS, the key/main/first-visible non-sheet window on macOS), the `LIVE` generation-tagged guard, and the `LivePresentation::dismiss(then)` seam that takes down a presentation displaced by a newer request before the newer one presents (`displaced_action` is its pure decision) |
| `present/apple_alert.rs` / `present/apple_sheet.rs` | The iOS/iPadOS arms: a `UIAlertController` (an iPad action sheet anchored as a popover), and a plugin-owned controller presented as a page sheet under `UISheetPresentationController` |
| `present/appkit_alert.rs` | The macOS alert arm: an `NSAlert` presented as a window sheet |
| `present/android_host.rs` + `present/android_alert.rs` | The Android alert arm: the Kotlin `FrustNativePresenter` object (tracking the resumed `Activity` via the manifest-declared `FrustNativePresenterInitProvider`) building a framework `android.app.AlertDialog`, answered through its one `nativeOnOutcome` export |
| `present/unsupported.rs` | The alert arm of every target with no native modal UI (desktop Linux, Windows, web) |
| `android/` (Kotlin + JNI) | The Kotlin factory/listener pair and the Rust side of their JNI exports |
| `apple/` (objc2) | The iOS factory class, `FrustNativeControlTarget`, and the iOS halves of theme ladder L1/L2 |
| `appkit/` (objc2) | The macOS `DesktopViewFactory` (`ensure_registered`), its target class, and the macOS halves of theme ladder L1/L2 |
| `coretext.rs` | Theme ladder L3's shared CoreText half (embedded bytes to `CTFontDescriptor`, latch-on-first-publish caching), compiled for both Apple arms so neither can drift from the other's typeface behaviour |
| `demo.rs` | `DemoCard`, one composite `NativeComponent` (a parent, a title and two buttons in one slot) behind the non-default `demo-components` feature — the plugin's end-to-end proof of define → register → mount → attach |

## Layer Dependencies

NATIVE_WIDGETS depends on `frust-plugin` (Android JavaVM/Context handle install, the desktop
view-factory registry), `thiserror`/`log`, `jni` on Android, and the `objc2` family on the Apple
arms: iOS takes `objc2-ui-kit`/`objc2-quartz-core`/`objc2-core-text`/`objc2-core-foundation`,
macOS `objc2-app-kit`/`objc2-quartz-core`/`objc2-core-graphics`/`objc2-core-text`/
`objc2-core-foundation`, and both take `block2`/`dispatch2` for the presentation substrate (every
block-based handler and the main-queue hop). Pin rows and feature lists ride
[PLUGINS_DEVELOPMENT.md](PLUGINS_DEVELOPMENT.md)'s Version Pins. The default-on `frust-api` feature
additionally pulls in `frust`/`frust-core`/`frust-theme`/`kurbo` for the app-facing layer; with it
off the crate is a bare leaf plugin, and `present/` still compiles (it names no framework type).

On Android and iOS the plugin integrates with `frust-shell-common`'s `platform_view` Mode-B differ
and platform-view-factory contract (see [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)); on macOS
it registers a `DesktopViewFactory` that `crates/frust-shell-macos`' Mode-A host resolves by
`view_type` string — a different integration seam, not a Cargo dependency either way.

This is a chartered leaf plugin: `frust-plugin` + FFI only with default features off, pinned by a
no-default-features conformance gate. The one-generic-factory/one-generic-listener shape per
platform is permanent: a new control extends the existing dispatch, and a control that reports
through a delegate protocol (the tab bar) gains that conformance on the existing target class
rather than a second one. Native presentations are the one chartered exception, adding one
presenter per platform rather than riding the control dispatch — on Android a third frozen Kotlin
class, `FrustNativePresenter`, plus one frozen JNI export,
`Java_dev_frust_nativewidgets_FrustNativePresenter_nativeOnOutcome`. Once shipped, JNI export
symbol names and Kotlin package/class names are frozen; each Apple arm gates on its own
`target_os` (`"ios"` for `apple/`, `"macos"` for `appkit/`), never `target_vendor = "apple"` —
UIKit does not exist on macOS, and `coretext.rs` is the one module compiled for both; and the FFI
boundary uses hand-rolled flat-JSON params instead of `serde`, keeping the wire format independent
of any Rust-side type change.

That same FFI boundary caps who can practically extend `NativeComponent`: an app crate cannot
implement it, because doing so means naming raw `jni`/`objc2-ui-kit`/`objc2-app-kit` types this
plugin does not re-export, so its audience is plugin authors who take the FFI dependency directly —
app code stays on the builders.

## Data Flow

- **Create/update/dispose** route through the one platform factory into the internal runtime, which
  diffs props before any platform call (an unchanged rebuild costs zero FFI crossings) and resolves
  disposal by view identity, so late or duplicate calls are no-ops. Each control's `plan` emits a
  setter only for a changed field; the controlled controls (switch, slider, segmented, stepper,
  date picker, tab bar) write the app-confirmed value back when the platform has drifted.
- **Events**: the platform listener or target fires on the main thread and routes by slot id into
  a typed payload delivered to app callbacks through signals, bypassing the core render/event
  system in [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md) entirely. Eight wire kinds cross that
  boundary, pinned against Kotlin drift by `tests/kotlin_conformance.rs`: `CLICK`, `TOGGLED`,
  `VALUE_CHANGED` (1-3, all three arms); `DRAG_START`/`DRAG_END` (4-5, Android and iOS only — AppKit
  target-action carries no gesture phase, and macOS never synthesizes one); `SELECTION` (6, the
  Apple segmented control and the iOS tab bar); `DATE` (7, all three arms); `RESELECTED` (8, the iOS
  tab bar's tap on the tab the app last confirmed). A Kotlin twin exists for every kind, emitted or
  not, so the two tables stay one append-only table.
- **Tab bar**: the bar never navigates. A tap reports the requested `TabId` through `on_select`
  (or `on_reselect`), the app routes, and the confirmed id comes back as props; which tab counts as
  "showing" is the app's last confirmed selection, not UIKit's highlight.
- **`NativeComponent`** reuses the built-in controls' runtime and factory, so a plugin author's
  native subtree mounts through one slot. A component attaches its platform's one listener class to
  any view it built through `ComponentCtx::attach_listener`, naming a `ListenerKinds` family (click,
  toggled, value changed); on Android that is `FrustNativeListener`, on iOS
  `FrustNativeControlTarget::attach_component`, on macOS the AppKit target's `attach_view`. Each
  attach answers a `ListenerHandle` released on drop or dispose; an attached family's event routes
  by slot id to `NativeComponent::on_event`, and an unattached family is dropped first. An event
  carries only slot id, kind and a primitive detail — never which child fired
  ([LIMITATIONS.md](LIMITATIONS.md)'s `native-widgets-component-same-kind-children-indistinguishable`).
  `DemoCard` proves the path on all three arms, plus a recording host arm on Linux/Windows/web.
- **Mode B (Android/iOS)**: a slot is only visible if the frust surface above it actually resolves
  translucent. The engine never refuses a translucent surface itself (see
  [RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md)), but a platform compositor with no translucent
  alpha mode surfaces as `frust::resolved_surface_mode() == ResolvedSurfaceMode::RefusedTranslucent`,
  on which every builder renders a frust-drawn placeholder instead of an invisible, untappable slot.
- **Mode A (macOS)**: the hosted view is opaque and above the surface, so there is no translucency
  concern — and the consequences run the other way: frust content under a slot is covered, never
  blended, so frust chrome cannot draw over a hosted control; the host ignores a slot's
  `interactive` flag, because AppKit's responder chain delivers pointer, wheel and scroll input
  inside a hosted control's bounds to that control directly
  ([LIMITATIONS.md](LIMITATIONS.md)'s `desktop-platform-view-mode-a-only`).
- **Native presentations** (`present/`): submit → validate the spec → claim the process-wide Busy
  slot and stamp a fresh, never-zero generation → on its main thread (the Apple arms hop there, the
  Android runtime is already confined to it), the platform host discovers its presenting host and
  parks the live presentation under that generation → exactly
  one terminal outcome returns through the generation-guarded one-shot, and any callback for another
  generation is dropped → the slot is released before the caller is woken, so its continuation may
  present again. The slot is shared by alerts and sheets, and there is no queueing: a request while
  any presentation is live answers `Busy` at once. Dropping the `Presentation` future frees the slot
  but not the platform UI; a later request takes that displaced presentation down first, its
  outcome discarded, so two never show stacked (on iOS the newer one presents from the displaced
  one's dismissal completion). A sheet's detent changes stream to `SheetSpec::on_detent` and are
  not outcomes.

  | Platform | Alert dismissal | Alert action mapping | Alert `style`/`anchor` |
  |----------|-----------------|----------------------|------------------------|
  | macOS | Return → the first action; Escape → the Cancel-role action; no click-away, so a bare `Cancelled` never happens | one `NSAlert` button per action, in spec order | ignored — no action-sheet idiom |
  | iOS/iPadOS | `Cancelled` only on an iPad popover's outside tap | UIKit places the Cancel-role action itself | `ActionSheet` on iPad requires `anchor` (`InvalidSpec` without one) |
  | Android | back key / outside tap → `Cancelled`, only when `cancelable` | Cancel → negative, then neutral, then positive; Default/Destructive → positive, then neutral, then negative | ignored — `android.app.AlertDialog` has no action-sheet idiom |

  The iOS sheet resolves `Action(id)` after it has finished dismissing itself, `Dismissed(User)` on
  a swipe-down (only when `dismissible`), `Dismissed(Programmatic)` through its `SheetHandle`, and
  `HostLost` when its scene disconnects or its presenter is torn down. Custom detents need iOS 16
  (older systems substitute the nearest medium/large detent). A page sheet in a regular-width iPad
  window is a centered form sheet that ignores detents
  ([LIMITATIONS.md](LIMITATIONS.md)'s `native-sheet-ipad-regular-width-detents`).
- **Theme ladder**: `Theme` folds into control props every frame, diff-gated, so an unchanged theme
  costs zero FFI calls (see [WIDGETS_ARCHITECTURE.md](WIDGETS_ARCHITECTURE.md) for `Theme` itself).
  L1 is brightness: Android bakes it at construction, while both Apple arms re-pin it on every create
  and update (`overrideUserInterfaceStyle` on iOS, `NSAppearance` over the hosted subtree on macOS,
  so a culled-then-recreated control never diverges from its siblings). L2 maps packed ARGB colours
  per control — `NSSwitch` and `NSProgressIndicator` expose no tintable property and draw in the
  system accent colour, logged and no-op'd rather than faked. L3, the typeface ladder, resolves
  **per slot** (button/body): `NativeTypefaces` — a design system's attached `ThemeExtensions`
  payload — beats the platform's system font, with no design-language shortcut
  (`Theme::design_language` is never read here). Publishing is last-pair-wins and
  diff-gated, but each platform half latches its first published pair until relaunch
  ([LIMITATIONS.md](LIMITATIONS.md)'s `native-typeface-first-publish-latch`); the two Apple halves
  share `coretext.rs`.

## Key Types

| Type | Purpose |
|------|---------|
| `native_button` / `native_label` / `native_switch` / `native_slider` / `native_progress` / `native_image` / `native_spinner` / `native_date_picker` | The eight shared app-facing builders, each composing one `platform_view` slot |
| `native_segmented` / `native_stepper` | The two Apple-only builders (iOS + macOS; a refusal banner elsewhere) |
| `native_tab_bar` + `TabItem` / `TabId` / `TabIcon` | The iOS-only controlled bottom tab bar (a refusal banner elsewhere); an item (id, title, icon, optional selected icon, badge, enabled); the app-chosen stable tab identity it selects and reports, never an index; and an icon as encoded bytes or an SF Symbol name |
| `CivilDate` | Plain year/month/day value `native_date_picker` takes and its `DATE` event reports |
| `NativeComponent` (+ `ComponentCtx`) | Public trait for a plugin author to drive a native view hierarchy from Rust |
| `ListenerKinds` / `ListenerHandle` | Which event families a component attaches to a view it built, and the RAII handle releasing that attach |
| `native_component` / `register_component` | Generic define → register → mount path for a `NativeComponent` |
| `show_native_alert` / `show_native_alert_into` | App-facing alert entry points: the awaitable form, and the events-as-signals form writing the outcome into an app `RwSignal` once |
| `show_native_sheet` / `show_native_sheet_into` | The same two forms for a sheet |
| `AlertSpec` (+ `AlertAction` / `ActionRole` / `AlertStyle` / `AnchorRect`) | A native alert's request: title, message, up to three actions, cancelable, centered-alert-vs-action-sheet style, and the popover anchor an iPad action sheet points from |
| `SheetSpec` (+ `SheetContent` / `SheetAction` / `Detent`) | A native sheet's request: constrained native content (title, message, image, up to three action rows), its detents (`Medium`/`Large`/`Custom(fraction)`), selected detent, grabber, dismissibility and detent listener |
| `AlertOutcome` / `SheetOutcome` / `DismissReason` | The one terminal outcome: an alert's `Action`/`Cancelled`/`Dismissed`/`HostLost`; a sheet's `Action`/`Dismissed(DismissReason)`/`HostLost`, where `DismissReason` is `User` or `Programmatic` |
| `PresentError` | Why a presentation could not be shown or complete: `Busy`/`NoHost`/`Unsupported`/`InvalidSpec`/`Platform` |
| `Presentation<T>` / `PresentationHandle` / `SheetHandle` | The awaitable future every request answers with, the handle naming it for `dismiss`, and a sheet's handle adding `select_detent` |
