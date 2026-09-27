# Frust - PLUGINS Architecture

## Overview

PLUGINS is the leaf plugin tier sitting beside the `frust` facade. `frust-plugin` is the shared
substrate, in two halves with opposite directions: the Android platform-handle slot
(JavaVM/Context) a shell writes and a JNI-reaching plugin reads, and the desktop view-factory
registry a plugin writes and a desktop shell reads. On top of it sit nine independent
OS-capability plugins — `shared-preferences`, `secure-storage`, `camera`, `clipboard`, `haptics`,
`iap`, `video-player`, `url-launcher`, and `auth-session` — each exposing one platform-independent
public API behind a per-platform backend. `url-launcher` opens an absolute `http`/`https` URL in the
platform's default external browser, fire-and-forget, over Android `ACTION_VIEW`, iOS
`openURL:options:completionHandler:`, or a desktop opener — the launch half of an RFC 8252 OAuth
round trip, whose completion returns through `frust::deep_links()`. `iap` is in-app purchases
and subscriptions over OpenIAP 3.0.1: Play
Billing via the `openiap-google` Kotlin host on Android, StoreKit 2 via the `FrustIap` Swift glue on
iOS, both sides speaking a JSON-string wire protocol, with purchase outcomes delivered on a
registered event listener rather than as a call's return value. `video-player` plays a local file,
a bundled asset, or an `http(s)` URL — progressive download or HLS — over Media3 ExoPlayer on
Android and AVPlayer on iOS *and* macOS, publishing its picture into a native platform-view slot
rather than painting a frame itself.

`clean-signals-frust` is a facade-tier plugin gluing the external `clean_signals`
clean-architecture core into Frust's `Component`/reactive model; it is a standalone workspace
excluded from the root Cargo graph pending a crates.io publication of its dependency. Two further
crates reach no real OS capability at all: `plugins/database` (`frust-database`), the tier's first
such, a synchronous embedded SQL API over a swappable SQLite/Turso engine seam; and `plugins/i18n`
(`frust-i18n` plus its `frust-i18n-macros` companion — the tier's first proc-macro crate), a
Fluent Project + ICU4X internationalization plugin reaching the OS only for a locale read.

`plugins/glyph`, `plugins/material`, `plugins/cupertino`, `plugins/shadcn`, and `plugins/beui`
(`frust-glyph`/`frust-material`/`frust-cupertino`/`frust-shadcn`/`frust-beui`) are the tier's
**design-system plugins**: five widget catalogs (Glyph, Material 3, Cupertino, shadcn/ui, beUI),
each an ordinary sibling crate an app depends on beside `frust` rather than a cargo feature on it.
`frust-shadcn` and `frust-beui` are the tier's **external-origin** catalogs — ports of third-party
web registries (shadcn/ui v4, beUI v2) rather than in-tree extractions of what used to be
feature-gated `frust-widgets` modules — and so the tier's proof case that the external
design-system contract holds for a catalog authored outside this repo. All five reach no OS
capability at all and share nothing with the OS-capability plugins above beyond sitting in the same
tier; see *Design-System Plugins* below.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how PLUGINS relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `crates/frust-plugin` | Android platform-handle substrate (JavaVM/Context) plus a scoped JNI-attach helper, and the process-global desktop view-factory registry a plugin publishes a native-view factory into and a desktop shell resolves a `view_type` through. The registry half is std-only and compiles on every target; the Android half is inert off Android, and there is no Apple-specific surface at all |
| `plugins/shared-preferences` | Synchronous KV store routed to NSUserDefaults, Android SharedPreferences, or a JSON file backend |
| `plugins/secure-storage` | Synchronous, named-store secure string storage with an optional biometric gate, routed to Keychain, Android Keystore, or keyring-core |
| `plugins/camera` | Camera capability (permission, still capture, image stream, barcode/QR decode, torch) over CameraX (Android) or AVFoundation (Apple); preview surfaces as a native platform-view slot |
| `plugins/clipboard` | Synchronous plain-text clipboard over Android `ClipboardManager` (10+ returns nothing to an unfocused reader), iOS `UIPasteboard` (14+ shows a one-time paste banner), or desktop `arboard` (X11 clipboard content dies with the owning process unless a clipboard manager adopts it) |
| `plugins/haptics` | Fire-and-forget haptic effects over Android `Vibrator`/`VibrationEffect` or iOS `UI*FeedbackGenerator`; desktop is unavailable by design (no first-class API to route to) |
| `plugins/iap` | In-app purchases and subscriptions (products, purchases, restore, deep-link to subscription management) over Play Billing (`openiap-google`) or StoreKit 2 (`FrustIap` Swift glue), both speaking OpenIAP 3.0.1; desktop is a v1 deferral, not a capability gap |
| `plugins/video-player` | Video playback (file, bundled-asset, `http(s)` and HLS sources; play/pause/seek/rate/volume/loop; lifecycle state and events) over Media3 ExoPlayer on Android or AVPlayer on iOS and macOS; the picture is a native platform-view slot, never a frame this crate paints |
| `plugins/url-launcher` | Fire-and-forget launch of an absolute http/https URL in the platform's default external browser over Android `ACTION_VIEW` (application-`Context` + `NEW_TASK`), iOS `openURL:options:completionHandler:` (async main-queue bounce), or desktop `xdg-open` / `open` / `ShellExecuteW`; the launch half of an RFC 8252 OAuth round trip — completion returns through `frust::deep_links()` |
| `plugins/auth-session` | OAuth authorization round trip in the platform auth user agent — `ASWebAuthenticationSession` (iOS/macOS; the callback URL returns in-process, no custom-scheme intent) or Chrome Custom Tabs (Android; a Gradle module whose Kotlin host launches from the resumed Activity and reads the callback Intent on resume) — resolving an awaitable `Callback(url)`/`Cancelled` with a one-session (Busy) guard and an ephemeral mode; Linux/Windows report `NoHandler` |
| `plugins/clean-signals-frust` | Facade-tier glue crate binding the `clean_signals` clean-architecture core into Frust's `Component`/reactive model |
| `plugins/database` | Synchronous embedded SQL database (`Database`/`Value`/`Engine`) over a swappable-engine seam — bundled SQLite via `rusqlite` (default) or an optional Turso engine (`engine-turso`); no OS integration |
| `plugins/i18n` | Fluent Project + ICU4X internationalization/localization: compile-time bundle loading (`locales!`, via the companion `frust-i18n-macros` proc-macro crate), locale-aware message resolution, system-locale detection, and (`formatting` feature) ICU4X number/date/currency formatting |
| `plugins/glyph` | The Glyph design-system plugin (`frust-glyph`): terminal-native, dark-first, monospace-led widget catalog — including its own switch-class control (`toggle`), since baseline `frust-widgets` deliberately ships no `Switch` — plus its bundled OFL monospace fonts |
| `plugins/material` | The Material 3 (+Expressive) design-system plugin (`frust-material`): a 43-role token system (34 baseline `ColorScheme` roles + a 9-role `MaterialTokens` extension) with runtime HCT seed-color generation (`from_seed`), a feature-point `RoundedPolygon`/`Morph` shape engine backing a 35-shape catalog (`shapes::` is the crate's only morph engine — the legacy `shape_morph` module was retired), a unified interaction core with a pluggable haptics hook, 88 generated Material Icons vector constants, bundled Roboto Flex/Mono fonts, the M3E core-control catalog at reference parity — buttons (5 variants × 5 sizes, gradient decoration, overflow strategies), icon buttons, toggle button + button groups + segmented buttons, FAB, selection controls (checkbox/radio/switch), chips, sliders (incl. range), and a text field wrapping the baseline editable — plus its own `overlay` hosting seam (anchored + modal hosts, a sibling port of `frust-shadcn`'s; see *Design-System Plugins* below) and the containment/overlay/feedback tier built on it: menus (incl. submenu) and dropdowns, tooltips and a snackbar host, dialogs/bottom sheets/side sheets, a selection host, a search bar/view, cards and list families (incl. expandable/dismissible), dividers and badges, a carousel, progress/loading/refresh indicators, and date/time pickers — and the navigation/structure tier: a 4-constructor app bar family (top/search/bottom/sliver, the last collapsible), primary/secondary tabs, a two-spring liquid-indicator navigation bar/rail/drawer family (the bar self-insets the **bottom** window inset only, matching Flutter's Material 3 NavigationBar — left/right cutout/landscape insets are the caller's: wrap with `safe_area(bar).top(false).bottom(false)` for a docked bar that must respect cutouts, or `.safe_area(false)` for a bar embedded mid-screen, as huddle and material3-demo both do — painting `surface_container` through the consumed edge; default size `NavBarSize::Medium`, 80dp), floating/docked toolbars (scroll-hide + FAB 80→56 morph — see `material-fab-fixed-tier-icon-centering` in [LIMITATIONS.md](LIMITATIONS.md)), a shape-morphing FAB menu, and a two-pod split button (popup + bottom-sheet menu routes). Scroll-linked chrome (app-bar collapse, toolbar hide) has no widget-owned scroll handle: the app feeds `ScrollView::on_scroll` into the widget's own controlled collapse/hide prop — the pattern any future scroll-linked chrome follows |
| `plugins/cupertino` | The Cupertino (iOS-styled) design-system plugin (`frust-cupertino`), including its own "Liquid Glass" `GlassScale` recipe |
| `plugins/shadcn` | The shadcn/ui design-system plugin (`frust-shadcn`), the tier's first external-origin catalog: a port of shadcn/ui v4 (55 components, incl. an app-shell `sidebar`, a `message`/`message_scroller` chat pair, and a `questionnaire` step-sequence form), its own token system (vendored `neutral` base plus six sibling palettes folded onto the baseline `ColorScheme`, a `ShadcnTokens` extension for the roles it has no baseline analogue for), an `overlay` hosting seam for its anchored and modal panel families — every modal component animates its exit, and every anchored panel takes a prop-driven `.open(bool)` for the same (see *Design-System Plugins* below) — and its own bundled Inter + JetBrains Mono variable fonts |
| `plugins/beui` | The beUI design-system plugin (`frust-beui`), the tier's second external-origin catalog: a port of beUI v2 (81 upstream registry slugs — 42 `components`, 17 `agents`, 22 `blocks`, module-family-prefixed rather than flat re-exported), its own `BeuiTokens` extension over an oklch-transcribed palette, three glass recipes, and catalog motion tokens (`SPRING_*`/`EASE_*`), an `overlay` hosting seam mirroring shadcn's (see *Design-System Plugins* below), a dedicated per-character text-cell substrate (`motion::chars`) and stagger/pointer/scroll-effect drivers making motion the catalog's distinguishing layer, its own bundled Geist + Geist Mono variable fonts (`plugins/beui/fonts`, OFL), and (behind the non-default `gpu-effects` feature) the `gpu_fx` perspective-quad substrate five components opt into (see *Design-System Plugins* below) |

## Desktop Backend Status

Every plugin above targets Android/iOS first; desktop coverage is uneven by design, not omission —
this is the ground truth an app author needs before assuming a plugin "just works" in a desktop
preview or a `frust build macos|windows|linux`. See
[CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md)'s Data Flow for how a plugin's desktop-lane
`Contribution`s reach an assembled bundle.

| Plugin | Desktop status | Detail |
|--------|-----------------|--------|
| `shared-preferences` | generic-desktop, macOS-native on macOS | NSUserDefaults (macOS, sharing the Apple arm with iOS) / a JSON file backend (Linux, Windows) |
| `secure-storage` | generic-desktop, macOS-native on macOS | Keychain (macOS, sharing the Apple arm with iOS) / `keyring-core` + secret-service or Credential Manager (Linux, Windows) |
| `camera` | macOS-native only | AVFoundation (macOS, sharing the Apple arm with iOS); no backend at all on Linux/Windows |
| `clipboard` | generic-desktop | `arboard` on macOS, Linux, and Windows alike — macOS shares this arm rather than its own Apple `UIPasteboard` one (UIKit-only) |
| `haptics` | unavailable-by-design | no first-class OS API to route to, on macOS, Linux, or Windows |
| `iap` | deferred (v1) | dependency-free, always-erroring stub on macOS, Linux, and Windows — mobile-first scope, not a capability gap (`iap-desktop-unavailable-v1` in [LIMITATIONS.md](LIMITATIONS.md)) |
| `video-player` | macOS-native (Mode A) via the desktop platform-view host | AVPlayer (macOS, sharing the Apple arm with iOS), its `NSView` hosted above the window's content view by the desktop shell; Windows and Linux have no backend at all (`video-web-windows-linux-unavailable-v1` in [LIMITATIONS.md](LIMITATIONS.md)) |
| `url-launcher` | generic-desktop | `xdg-open` (Linux), `open` (macOS), `ShellExecuteW` (Windows); desktop receives no deep links, so an OAuth round trip needs a typed-code fallback |
| `auth-session` | macOS-native only | `ASWebAuthenticationSession` with an AppKit key-window anchor; Linux/Windows have no auth user agent (`auth-session-linux-windows-unavailable-v1` in [LIMITATIONS.md](LIMITATIONS.md)) |
| `clean-signals-frust` | platform-free | facade-tier glue with no OS integration to split by platform at all |
| `database` | platform-free | file IO via `rusqlite`/`turso`; no OS integration, so no platform split |
| `i18n` | platform-free | reaches the OS only for a `sys_locale` read; no backend split |
| `glyph` / `material` / `cupertino` / `shadcn` / `beui` | platform-free | pure widget/token crates over `frust::authoring`; no OS integration of any kind, desktop included |
| `native-widgets` (NATIVE_WIDGETS unit) | unavailable | no desktop backend of any kind — Android/iOS only (see [NATIVE_WIDGETS_ARCHITECTURE.md](NATIVE_WIDGETS_ARCHITECTURE.md)) |

## Layer Dependencies

Each of `shared-preferences`, `secure-storage`, `camera`, `clipboard`, `haptics` and `iap`
depends on `frust-plugin` and, where it needs a data directory, `frust-paths`,
plus its own target-gated FFI crates: `jni`/`ndk-context` on Android; `objc2` and the matching
`objc2-*` crates (foundation, security, local-authentication, av-foundation, ui-kit, etc.) on
Apple; `keyring-core` plus a secret-service/Credential-Manager backend for `secure-storage`'s
desktop/Linux/Windows path; `arboard` for `clipboard`'s desktop text backend (macOS shares this
arm rather than its Apple one, since `UIPasteboard` is UIKit-only). `iap` narrows this to `jni` on
Android and `objc2`/`objc2-foundation`/`block2` on iOS, with **no** desktop dependency at all — its
desktop arm is a dependency-free, always-erroring stub rather than a real backend. Its iOS package
also makes it the second plugin (after `camera`) to ship its own Swift package, and the first whose
package declares an **external** SwiftPM dependency (`OpenIAP`, exact-pinned) rather than only the
local `FrustEmbedding` one every other plugin package depends on.
`clean-signals-frust` instead depends on the `frust` facade crate — its sole framework dependency —
to bind a `clean_signals` controller into a `Component`'s reactive `Owner`. `database` depends on
neither `frust-plugin` nor any target-gated FFI crate — its own dependencies are `frust-paths` (data
directory), `log`, and its two swappable SQLite engines, `rusqlite` (default, C via `cc`) and the
optional pure-Rust `turso`; both reach storage through plain file IO, so the crate needs no
platform-handle substrate at all. `i18n` is the second plugin (after `native-widgets`) built on the single-crate
platform-plugin-plus-facade-glue shape (`docs/PLUGINS_CODE_STANDARDS.md`'s Plugin Conventions): a
default-on `frust-api` feature gates its sole `frust` facade dependency, so `cargo check -p
frust-i18n --no-default-features` mechanically re-verifies the platform-plugin charter line — no
`frust` crate anywhere in the tree — the way every split plugin's two-crate boundary enforces
structurally instead. Its platform track is `frust-plugin` plus `jni`, Android-target-gated rather
than unconditional — a documented deviation from every Android-reaching plugin's norm above, since
only its Android detection backend needs the platform handle — plus `objc2`/`objc2-foundation` on
Apple and `sys-locale` on desktop (macOS routes through the `sys-locale` arm, not the Apple one).
Its message engine depends on `fluent-bundle`/`fluent-langneg`/`unic-langid`; the `formatting`
feature layers ICU4X's `icu_decimal`/`icu_datetime`/`icu_plurals`/`icu_experimental` plus their
`icu_locale_core`/`tinystr`/`icu_provider` support crates underneath. Like `database`, it needs no
`frust-paths` — it persists nothing itself.

`video-player` likewise needs no `frust-paths`, depending on `frust-plugin` plus FFI crates alone:
`jni` on Android; on `target_vendor = "apple"` the `objc2` family serving one AVFoundation session
on iOS and macOS alike, narrowed per OS by `objc2-ui-kit` plus `objc2-avf-audio` on iOS (the
hosting `UIView` and the playback audio-session category) and `objc2-app-kit` on macOS (the hosting
`NSView`). Its app-facing half rides the same default-on `frust-api` feature `i18n` uses.

This is an architectural charter, not just current practice: a plugin depends on `frust-plugin`
(plus `frust-paths` where needed) and FFI crates only if it reaches the OS through one, never
another `frust-*` framework crate; and the facade never depends on or re-exports a plugin — the
dependency always runs from an app's own manifest into the plugin, never through the facade. The
substrate is directional in both halves — **shells write platform handles and plugins read them;
plugins write desktop view factories and desktop shells read them** — which is what keeps
`frust-plugin` a leaf either way, with neither side ever naming the other.
`database` is the first plugin to need neither `frust-plugin` nor an FFI crate at all — a pure-Rust
plugin whose "platform" is the filesystem — which the charter accommodates rather than exempts: it
still depends on nothing but `frust-paths`, `log`, and its engines, never another framework crate. On
Android its `<data_dir>/databases/<name>.db` resolves under `Context.getFilesDir()` through
frust-paths' shell-installed slot, so `Database::open` needs no `frust-plugin` handle but must run
after `nativeInitPlatform`. Each
plugin's backends are cfg-gated modules (`apple`/`android`/`file`/`desktop`/`unsupported`) behind
one platform-independent public API, with FFI dependencies target-gated rather than unconditional.
The five design-system plugins are a further, distinct shape the charter above accommodates rather
than covers: no `frust-plugin`, no FFI crate, and — unlike every OS-capability plugin — a direct
`frust` facade dependency, since a widget catalog's whole job is building against `frust::authoring`
(see *Design-System Plugins* below).

A few cross-plugin conventions hold as boundary facts rather than mere style: pre-init detection is
a plain atomic flag checked first, so a `NotInitialized` error holds even under `panic=abort`; a JNI
attach is always scoped per call, never held permanently; and a store shared with the OS namespaces
its keys so plugin keys cannot collide with another library's. Plugin Kotlin/Swift host code may
ship warn-level store/host-failure logs in release builds — release-lean governs Rust's own `log`
output only, and payload-content hygiene (truncation/redaction) is a separate concern: `iap`'s
`Purchase`/`PurchaseInput`/`ActiveSubscription` redact their bearer token in `Debug`, and
host-payload excerpts embedded in error strings are capped at `PAYLOAD_EXCERPT_BYTES`.
`frust-reactive`'s `spawn_blocking` is the documented pairing for every plugin's blocking or gated
call (biometric prompts, camera permission/capture, every `iap` store round trip except
`request_purchase`/`set_purchase_listener`) — see [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md).

## Design-System Plugins

`frust-glyph`/`frust-material`/`frust-cupertino` are the three built-in widget catalogs, extracted
from `frust-widgets` into sibling plugin crates; `frust-shadcn` and `frust-beui` are the fourth and
fifth, external-origin ports of third-party web registries (shadcn/ui v4, beUI v2) rather than
in-tree extractions (see [WIDGETS_ARCHITECTURE.md](WIDGETS_ARCHITECTURE.md)'s External
Design-System Contract for the toolkit they all build against). Their shared charter:

- **Production deps: `frust` (`default-features = false`) plus `kurbo`/`peniko` only, never
  another `frust-*` crate.** This is what makes each one a real proof of the external
  design-system contract rather than a special-cased in-tree exception — an app depends on any of
  the five exactly the way it would depend on a third-party catalog.
- **`frust-widgets` (`test-support` feature) and `frust-core` (test-only) are dev-dependencies,
  never production ones.** The sanctioned test-fixture/`RenderRoot` route: a catalog's own tests
  need a real `RenderRoot` to paint against and `frust-widgets`' GPU-free container fixtures, both
  of which the facade deliberately does not re-export to production code.
- **`install()` is the one-line entry point, called from `app!`'s `setup = { .. }` block —
  before shell construction, the one point a shell reads the default-theme slot.** Each `install()`
  calls `frust::set_default_theme(baseline())` (`frust_shadcn::install()` and `frust_beui::install()`
  name their seed `theme()` instead, but the shape is the same); every crate but Cupertino
  additionally calls `frust::register_app_fonts` for its bundled fonts — Glyph's OFL monospace faces
  (Space Mono, IBM Plex Mono, `plugins/glyph/fonts/`), Material's bundled Roboto Flex (OFL-1.1) and
  Roboto Mono (OFL-1.1, `plugins/material/fonts/`), shadcn's bundled Inter Variable and JetBrains
  Mono Variable (OFL-1.1, no Reserved Font Name, `plugins/shadcn/fonts/`), beUI's bundled Geist
  Variable and Geist Mono Variable (OFL-1.1, no Reserved Font Name, `plugins/beui/fonts/`). Glyph,
  Material and shadcn register theirs behind a `bundled-fonts` feature (default on;
  `default-features = false` opts an app out onto the platform's system faces and leaves the bytes
  out of the binary), beUI unconditionally, Cupertino not at all. A call after shell construction
  takes effect only on a later theme reseed, which may never happen.
- **Material's Roboto Flex and shadcn's Inter ship as `wght`-only instances** of their upstream
  variable fonts, regenerated with `scripts/fonts/instance_variable_font.py` (fontTools
  `varLib.instancer`): `frust-text` drives only weight and style on a shaped run, so every other
  axis always rendered at its own default position and pinning it there is visually identical —
  provenance in `plugins/material/FONTS-LICENSE`, `plugins/shadcn/fonts/README.md` and each crate's
  `tokens/fonts.rs`. The other bundled faces are upstream bytes unmodified. Glyph keeps its two
  italic faces for the opposite reason: Frust never applies fontique's synthetic oblique, so an
  italic request with no italic face would render upright with no sign italics were asked for.
- **Every catalog's text takes its font family from the live theme's type-scale role through the
  catalog's own text helper**, with two standing exceptions — monospace text (`TypeScale` has no
  mono role) and a baseline `TextInput` field, whose `effective_style` resolves only color
  (`textinput-no-themed-family` in [LIMITATIONS.md](LIMITATIONS.md); mechanism in
  [WIDGETS_CODE_STANDARDS.md](WIDGETS_CODE_STANDARDS.md)).
- **`frust_glyph::baseline()`, `frust_material::baseline()`, `frust_shadcn::theme()`, and
  `frust_beui::theme()` all attach the `NativeTypefaces` theme extension** — the bundled faces reach
  `frust-native-widgets`' native controls through this attach, not through any
  `DesignLanguage`-keyed special case (see [NATIVE_WIDGETS_ARCHITECTURE.md](NATIVE_WIDGETS_ARCHITECTURE.md)'s
  theme ladder). Cupertino's `baseline()` attaches no font extension.
- **`frust-shadcn` and `frust-material` each carry a cross-component `overlay` seam** — the only
  place either catalog reaches through a full-area top-layer widget pattern standing in for the
  DOM portal shadcn/ui itself relies on. `frust-material`'s is a sibling port of `frust-shadcn`'s
  (ported, never depended on — the charter above forbids one design-system plugin depending on
  another), not a shared module. Both give an `anchored` host (trigger-relative placement in
  window space, light-dismiss, no scrim — popover, the menu/dropdown/select/combobox family, and
  material's tooltip/rich tooltip) and a `modal` host (scrim + centered/edge-pinned panel —
  dialog, alert-dialog, sheet, side/drawer sheet, full-screen search, picker). Every other
  component in either catalog paints inside its own box; only these reach through the seam —
  except `frust-shadcn`'s `tooltip` and `hover_card`, which register their panel with the
  **framework** overlay portal instead (`frust::authoring::OverlaySlot`, the seam
  `frust::overlay_portal` is itself built from), so the render root paints it above the whole main
  tree and hit-tests it ahead of that tree: a tooltip in the `Tooltip` band as
  `OverlayInput::Transparent`, so every press falls through as though it were not there; a hover
  card in `Floating` as `Interactive`, so its content is genuinely pressable. Both still latch
  their open state in a widget and time their delays off the paint clock, a hover having no event
  to open on, and the catalogs' own `anchored` hosts are candidates for becoming thin wrappers
  over that same slot. **One divergence**: `frust-shadcn`'s modal host pops via the simpler
  `NavigatorController::push_transparent_for_result`, with no staged back-press route, while
  `frust-material`'s sits on a `BackPolicy::DismissAnimated` + `PushOptions::dismiss_signal` tier
  — an Android back press stages the *same* reverse-ramp exit every other dismiss gesture takes, a
  back-dismiss tier `frust-shadcn`'s modal host lacks.
- **Both plugins' overlay hosts animate their exit, not just their entrance.** Each `modal` host
  stages every dismiss (scrim tap, Escape, close button, drag, and — material only — back) as a
  reverse ramp and fires the app's dismissal only once it settles — material's staged pop is
  identity-guarded against the navigator stack and refuses to fire (ramping the surface back open
  instead) if the stack moved during the ramp; `frust-shadcn`'s drawer
  additionally supports drag-to-close on all four pinned edges with a velocity-flick threshold and
  Base UI-style snap points. Each `anchored` host takes the same shape through a builder-level
  `.open(bool)`: an app that wants an exit ramp must keep the host mounted and toggle `open` rather
  than unmount it, since the framework has no seam for keeping a conditionally-mounted view alive
  past the rebuild that drops it (the *kept-mounted pattern*, documented in each plugin's own
  `overlay/anchored.rs`; see `shadcn-anchored-exit-needs-kept-mounted` in
  [LIMITATIONS.md](LIMITATIONS.md)).
- **`frust-beui` carries a third, independently-shaped `overlay` seam** — its own `anchored`/`modal`
  hosts (not ported from `frust-shadcn`'s), staged by the catalog's own `motion::Presence` driver,
  with the same *which-component-mounts-through-which-host* table discipline (`overlay/mod.rs`'s own
  doc table) and the same bounded-constraints/no-scroll-view mounting contract as shadcn/material's.
  Its `anchored` host takes the identical kept-mounted `.open(bool)` shape — a third
  `shadcn-anchored-exit-needs-kept-mounted` consumer — and its `modal` host's `StagedPop` is an
  advisory one-frame depth guard: it snapshots the navigator's page-stack depth when an exit ramp is
  staged and refuses to fire the pop if that depth moved before the ramp settles — the same
  identity-guard shape `frust-material`'s staged pop already carries; the two hosts carry the
  same guard under the same name, `StagedPop`.
- **`frust-beui`'s three catalogs (`components`/`agents`/`blocks`) mirror upstream beUI's own
  registry split** — 42 `components` (incl. `shader_background`'s five WGSL variants), 17 `agents`,
  22 `blocks`, 81 slugs total; public symbols stay module-prefixed rather than flat re-exported at
  the crate root (`ButtonVariant`, not a bare `Variant`) since the three families cover overlapping
  upstream ground. Motion is this catalog's distinguishing layer over the other four: `motion::chars`
  (per-character shaped text cells), `motion::stagger`/`pointer`/`scroll_fx`, and `motion::presence`
  (the kept-mounted exit staging above) back nearly every component, against Glyph/Material/
  Cupertino's and shadcn's mostly-static layouts. `examples/beui-demo` (desktop gallery, 19 pages) is
  the reference consumer — see [ARCHITECTURE.md](ARCHITECTURE.md)'s Examples table.
- **`frust-beui`'s non-default `gpu-effects` feature (`= ["frust/gpu"]`) adds `gpu_fx`, a
  perspective-quad GPU substrate reaching `wgpu` only through `frust::gpu`'s re-export — the crate
  carries no `wgpu` edge of its own.** `GpuFx::try_acquire` acquires the shell's live device and
  degrades to `None` (no device installed yet, or the crate's own `FRUST_BEUI_NO_GPU_FX` kill
  switch) so every consumer's 2D path stays both the default build's behaviour and the runtime
  fallback. `quad3d` is the one renderer (`QuadFace` solid/gradient/texture faces, optional depth
  test); `card3d`/`cylinder`/`fan` are three geometry families built on it that render a
  component's own already-computed 2D placement rather than a second one; `pool` quantises
  offscreen targets to 256px and reaps them unseen after 120 frames; `schedule::FxPass` is the
  per-component `ExternalPass` the engine drains ahead of its own scene pass every frame (see
  [RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md)'s GPU Seam).
- **Five components take an explicit, never-auto-on opt-in onto that substrate** —
  `TiltCardView::gpu_face`, `WalletCardView::gpu_fan`, `WheelPicker::gpu_drum`,
  `CylinderCarouselView::gpu_cylinder`, `ProjectFolderView::gpu_fan` — each compositing the
  projected face through `PaintScene::draw_scene_texture` alongside its existing 2D layout. The
  boundary holds everywhere it's used: a 3D face is a colour, a gradient, or a caller-owned texture
  the substrate itself renders, never a widget subtree — a component's own child content (text,
  captions, avatars) keeps compositing un-perspectived under `Affine`, on top of the composited
  face — and the composite is always blended, never opaque (see [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md)'s
  `PaintScene` row and `engine-scene-texture-always-blended` in [LIMITATIONS.md](LIMITATIONS.md)).
  `examples/beui-demo`'s GPU Effects page is the reference consumer, pairing every opt-in's 2D and
  3D path side by side.
- **`frust-shadcn`'s `scroll_area` establishes the *deferred-erasure builder* pattern**, for any
  wrapper needing an owned-value builder method (`.physics(...)`-style) to reach its child after
  construction: the child sits un-erased in a `Cell<Option<...>>` until a memoized
  `OnceCell<AnyView<_>>` erases it on first `build`/`rebuild`, since `View`'s own methods take only
  `&self` and cannot move it. A future wrapper component in this position should reach for this
  shape rather than re-inventing it.

## Data Flow

- Android requires an explicit platform-handle init (JavaVM/Context, via `frust_plugin::android`)
  before any plugin can reach the OS, done through scoped, per-call JNI attaches; Apple needs no
  init step since `objc2` reaches the ObjC runtime globally. `camera`, `iap`, `video-player`, and
  `auth-session` each ship their own Android `ContentProvider`-based init provider, installing the
  process's `Context` before `Application.onCreate` runs — four independent users of the same
  bootstrap pattern; `camera`, `iap`, and `auth-session` additionally cache the resumed `Activity`
  via `ActivityLifecycleCallbacks`, since billing/capture/Custom-Tab-launch flows all need one to
  launch a platform sheet, while `video-player` registers no such callback. `url-launcher` needs
  neither: its Android backend runs off the process-lifetime application `Context` the shell
  already installs.
- Each plugin exposes one platform-independent public API (`SharedPreferences`/`SecureStorage`/
  `Camera`/`Clipboard`/`Haptics`/`Iap`) that dispatches to a per-platform backend implementation; a
  shared conformance suite validates all backends uniformly, including a test-only file/desktop
  harness (`haptics` has no desktop backend to conform, being unavailable there by design; `iap`'s
  suite runs only against a `cfg(test)` in-memory fake store, since neither mobile backend can run
  host-side at all).
- Camera preview mounts as a native-sibling compositing slot via `frust::platform_view` — the core
  render pipeline paints nothing for it (see [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)) —
  while the underlying camera session's lifetime is independent of any single preview slot.
- The camera plugin's barcode/QR decoder runs Rust-side, synchronously inside the image-stream
  callback on the plugin-owned thread (never the UI thread) — the same lossy-latest backpressure
  that governs a raw image stream self-regulates decode cost. `rqrr` is the private v1 engine
  behind an internal, swappable decode-engine seam. Torch control is session-level state,
  independent of any stream, and dies with session close.
- `video-player` is the tier's non-blocking counterexample: `VideoPlayer::open` and every control
  method is a fire-and-forget command posted at the platform player from whatever thread asks — no
  `spawn_blocking` pairing and no UI-thread guard anywhere. Outcomes come back the other way on the
  platform **main thread**, into a lock-free snapshot and then the session's single listener: the
  sanctioned alternative to `iap`'s plugin-owned event-delivery thread, available because both
  backends already report on that thread, and the reason the plugin's reactive half writes an
  `RwSignal` straight from a listener. A listener may only write — a control call from inside a
  delivery is refused as `VideoError::Reentrant` rather than re-entering the backend mid-event.
- A video session's lifetime is independent of the slot showing its picture, the same A6 contract
  `camera`'s preview holds: a slot scrolled away and back reattaches to the same player, and only
  closing (or dropping) the session releases it.
- Both Apple view factories are Rust `define_class!` Objective-C classes registering themselves on
  the first `open` — no Swift package and no C export, each reaching its player through a
  crate-private accessor rather than an FFI symbol; on macOS the registration goes into
  `frust-plugin`'s desktop registry, which is where the shell resolves it. Android's factory is
  Kotlin inside the plugin's own Gradle module, resolved by the embedding by class name.
- Blocking or gated calls (secure-storage's biometric gate, camera's permission/capture, every
  `iap` store call except `request_purchase`) fail fast with a typed UI-thread error rather than
  parking when invoked on the platform UI thread; callers re-issue the call via
  `frust_reactive::spawn_blocking`.
- `iap` crosses the Rust↔Kotlin/Swift FFI boundary as JSON strings on both platforms — OpenIAP's
  own serializers on the host side, `serde`/`serde_json` on the Rust side — rather than typed
  per-field calls. A purchase is two-phase: `Iap::request_purchase` only acknowledges that the
  store accepted the request; the actual outcome (`IapEvent::PurchaseUpdated`/`PurchaseError`)
  arrives later on the listener registered via `Iap::set_purchase_listener`, delivered in reported
  order on the plugin's own event-delivery thread — one process-global consumer every store-SDK
  callback is handed to, since both hosts report on the platform main thread (Play Billing's
  `PurchasesUpdatedListener`; OpenIAP's main-actor listener delivery) — never the UI thread and
  never the thread that called `request_purchase`.
- Plugin distribution: `frust-drive`'s plugin registry applies each plugin's OS-side integration
  (Gradle module, Swift package, Info.plist key, manifest permission, Cargo dependency) onto a
  scaffolded app, driven by the `frust` TUI's Add Plugin dialog or manually per plugin README (see
  [CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md) and [TUI_ARCHITECTURE.md](TUI_ARCHITECTURE.md)).
  `haptics` is the registry's first entry to add a manifest permission
  (`android.permission.VIBRATE`) directly rather than folding it inside a plugin's own Gradle
  module, since its Android backend is plain JNI with no Kotlin helper class to carry it.
- `clean-signals-frust` wires a `clean_signals` controller into a Frust `Component`'s reactive
  `Owner`, surfacing failures and async state as `View`s; `Component` builds are coarse-grained,
  fully re-running on any tracked-signal write.
- `database` dispatches `Database::execute`/`query`/`transaction` through a crate-private
  `EngineConn` seam to whichever engine is compiled in; both engines enforce the same interop
  discipline (WAL journal mode, no `mvcc`/cipher/hexkey pragma) so a file either engine writes
  stays readable by the other. Its `turso` backend is the tier's first to own a process-wide OS
  thread running a dedicated tokio runtime: seam calls send their future to that thread over a
  channel and park, rather than `block_on`-ing on the caller's own thread, so there is no
  panic-under-abort path. `DatabaseError::AsyncContext` is returned for the provable subset of
  wrong-context callers (inside a runtime's `block_on` body, not inside a spawned task); like
  every plugin's UI-thread discipline, `database`'s is docs-only (no typed guard) and
  `frust_reactive::spawn_blocking` is the sanctioned call path, never rejected by the guard.
  `Database::transaction` rolls back on every exit but a successful `COMMIT` — closure `Err`, a
  failing `COMMIT`, or a panicking closure, the last via a `Drop`-guard rollback; same-thread
  reentrancy is refused as a typed `Reentrant` error rather than deadlocking, while cross-thread
  contention still queues on the connection mutex.
- `i18n`'s `locales!` proc macro (`frust-i18n-macros`) expands per invoking module into
  `locale_set()`/`engine()`/typed `keys::` functions built over the `Resolve` seam every
  resolver — `Engine::with_chain`, the reactive `I18n` handle — implements; every `.ftl` file is
  parsed at macro-expansion time, so a malformed message is a compile error naming file/line
  rather than a runtime miss.
- `i18n`'s detection (`system_locales`) re-queries the OS on every call — no cache, no
  platform-change event plumbing — so an app that wants to react to a live system-locale change
  polls it itself (e.g. on resume) and re-negotiates through `I18n::set_locale`.
- `i18n`'s reactive `I18n` handle holds the active locale in a tracked `RwSignal`; `set_locale`
  re-negotiates and writes it, waking the shell through the normal signal-write → `FrameWaker`
  path like any other app-state change — a coarse, whole-subscribed-tree rebuild, the same shape
  `clean-signals-frust`'s `Component` builds take above.
- `i18n`'s `formatting` feature registers ICU-backed `NUMBER`/`DATETIME` Fluent functions per
  locale via `LocaleSet::with_locale_function` (one formatter factory call per registered
  locale, keyed by it), reachable both directly (`fmt::decimal`/`currency`/`date`/`time`) and
  from inside an `.ftl` message.

## Key Types

| Type | Purpose |
|------|---------|
| `PlatformHandleError` | Error reported when platform handles aren't yet installed or a JNI attach fails |
| `SharedPreferences` / `PrefsError` | The KV-store handle and its typed error enum |
| `SecureStorage` / `SecureStorageError` / `AuthPolicy` / `Accessibility` | The secure-store handle, its error enum, and the biometric-gate/accessibility policy types |
| `Camera` / `CameraSession` / `CameraError` / `ImageFrame` / `ImagePlane` | Camera entry point, an open session handle, its error enum (including `StreamBusy`, raised when the raw image stream and the barcode stream contend for the session's single stream claim), and the per-frame image-stream payload types |
| `Barcode` / `BarcodeFormat` / `DetectionPolicy` | A decoded barcode/QR result; its symbology enum (`#[non_exhaustive]`, QR-only in v1); the emit-timing policy (`NoDuplicates`/`Throttled`/`Unrestricted`) for `CameraSession::start_barcode_stream` |
| `Clipboard` / `ClipboardError` | Synchronous plain-text clipboard entry point and its error enum |
| `Haptics` / `HapticEffect` / `HapticsError` | Haptic-feedback entry point, its closed effect vocabulary, and its error enum |
| `Iap` / `IapError` / `IapEvent` / `IapErrorCode` / `ListenerHandle` | In-app-purchase entry point (stateless, one store connection per process); its error enum (`Store` = a store refusal, `Platform` = a glue/boundary defect — Android tells them apart via a `synthetic` marker field the host JSON can never legitimately carry, iOS via the throw's type); the two purchase-outcome events (`PurchaseUpdated`/`PurchaseError`) delivered on the registered listener; the store-reported failure code carried inside a `PurchaseError`; the drop-to-unregister handle returned by `set_purchase_listener` |
| `use_controller` / `provide_controller` / `expect_controller` / `use_failure_listener` / `async_view` / `use_interval` | `clean-signals-frust`'s public hooks bridging a `clean_signals` controller into a Frust `Component`'s reactive `Owner` |
| `Database` / `Value` / `Engine` / `DatabaseError` | The embedded-SQL entry point (one serialized connection per handle, `Send + Sync`); the five-SQLite-storage-class param/result value; the compiled-engine selector (`Sqlite`/`Turso`, `#[non_exhaustive]`); the typed error enum (`Storage`, `Sql`, `AsyncContext`, `EngineUnavailable`, `Reentrant`) |
| `I18n` / `Locale` / `I18nError` | The reactive locale handle (`frust-api`) pairing an `frust_i18n::Engine` with a tracked active-locale signal, exposing `locale()` (message locale for bundle resolution) and `format_locale()` (composed message-language + requested-region for ICU formatting); the BCP-47 locale newtype; the crate's one public error enum |
| `frust_i18n::Engine` / `LocaleSet` / `Resolve` | The immutable, `Send + Sync` Fluent bundle core and its builder; the message-resolution trait both `locales!`-generated typed-key functions and the reactive `I18n` handle implement |
| `locales!` | `frust-i18n-macros`' compile-time proc macro loading a locale directory into `locale_set()`/`engine()`/typed `keys` |
| `fmt` (`CivilDate` / `CivilTime` / `DateLength`) | ICU4X-backed decimal/percent/currency/date/time formatting entry points and their date/time value types (`formatting` feature) |
| `VideoPlayer` / `PlayerSession` / `VideoSource` / `PlaybackState` / `PlayerEvent` / `VideoError` / `VideoPlayerHandle` | The video entry point (opens sessions, never blocks); the open session carrying the controls, a lock-free snapshot, the one-listener registration, and the `view_type`/`params_json` its native view attaches through; the file/bundled-asset/URL source enum; the frozen seven-state lifecycle; the listener's event enum; the typed error enum (`NotSupported`, `Reentrant`, `Closed`, and the host-reported failures); and the reactive handle pairing a session with the five tracked signals a `build` reads |
| `UrlLauncher` / `UrlLauncherError` | External-browser launch entry point and its error enum (`InvalidUrl`, `PlatformNotInitialized`, `NoHandler`, `Platform`) |
| `AuthSession` / `AuthSessionRequest` / `AuthSessionOutcome` / `AuthSessionError` | Auth-user-agent entry point, its request (https URL, callback scheme, ephemeral), the two outcomes, and the error enum (`InvalidUrl`, `Busy`, `PlatformNotInitialized`, `NoHandler`, `Platform`) |
| `DesktopViewFactory` / `DesktopViewHandle` | `frust-plugin`'s desktop-registry vocabulary: the main-thread-only create/update-params/dispose trait a plugin registers per `view_type`, and the `!Send`, +1-retained native-view pointer handed to a desktop shell on create and given back on dispose |
| `ButtonDecoration` / `OverflowObserver` | `frust_material`'s button-family extension seams: pluggable gradient decoration and label-overflow-strategy observation |
