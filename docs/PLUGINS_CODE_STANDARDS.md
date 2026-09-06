# Frust - PLUGINS Code Standards

Conventions specific to the two plugin tiers under `plugins/` (the PLUGINS and NATIVE_WIDGETS
units — see [ARCHITECTURE.md](ARCHITECTURE.md)'s Doc Map). Everything shared with the rest of the
workspace — language idioms, error handling, naming, anti-patterns, testing, comment conventions
— lives in [CODE_STANDARDS.md](CODE_STANDARDS.md) and binds here too; this doc records only what
is additional or different for a plugin.

## Plugin Conventions

- **Backends are `#[cfg]`-gated modules** (`apple`/`android`/`file`) behind one
  platform-independent public API; FFI deps are target-gated in the crate's `Cargo.toml`,
  never unconditional.
- **Errors are `thiserror` enums callers match on** (`PrefsError`, `PlatformHandleError`),
  per [CODE_STANDARDS.md](CODE_STANDARDS.md)'s Error Handling rule.
- **A JNI attach is scoped per call, never permanent** — threads don't auto-detach on exit,
  so `attach_permanently` leaks the attachment; use `frust-plugin::android::with_jni_env`'s
  scoped `AttachGuard`.
- **No panics/unwinds near an FFI boundary**, the same rule as shell exports
  ([CODE_STANDARDS.md](CODE_STANDARDS.md)'s Language Idioms).
- **Platform plugins never depend on `frust-*` framework crates** (`frust-plugin` +
  `frust-paths` + FFI crates only); **facade plugins depend on `frust` alone**. The three
  design-system plugins (`frust-glyph`/`frust-material`/`frust-cupertino`) are facade plugins
  that also name `kurbo`/`peniko` directly (mirroring `frust-widgets`' own manifest, since a
  catalog builds custom widgets over `frust::authoring` the same way `frust-widgets` builds its
  baseline set) — `frust-widgets` (`test-support` feature) and `frust-core` (test-only) are
  sanctioned dev-dependencies for the same `RenderRoot`/fixture route `frust-widgets` itself
  uses, never a production one (see [PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md)'s
  Design-System Plugins). A plugin
  needing both splits into a platform-core plus facade-glue crate — **or** ships as one
  crate with a default-on `frust-api` feature gating the optional
  `frust`/`frust-core`/`frust-theme` deps (`frust-native-widgets`'s shape), so `cargo check
  -p <crate> --no-default-features` still resolves to the platform-plugin charter line.
  Splitting would have left an almost-empty platform-core crate here; the charter line is
  what actually matters, and it is mechanically checkable either way (`cargo tree -p <crate>
  --no-default-features -e normal`). `frust-native-widgets` needs `frust-core`/`kurbo`
  directly alongside `frust` (not the facade alone) because its builders hand-implement
  `View<Outer>`/`Widget` in the plugin's own crate — the plugin tier's sanctioned, permanent
  exception to the app-tier facade-first rule ([CODE_STANDARDS.md](CODE_STANDARDS.md)'s State &
  Reactivity Conventions); plugins sit beside the facade, never inside it
  ([ARCHITECTURE.md](ARCHITECTURE.md)'s facade/plugin boundary), and are not expected to
  migrate onto `frust::authoring`.
- **A facade plugin that records its own GPU pass reaches `wgpu` only through `frust::gpu`'s
  re-export, behind an optional, non-default cargo feature** (e.g. `gpu-effects =
  ["frust/gpu"]`) — never a direct `wgpu` edge of its own. The default build stays on the
  facade-plugin charter line above (`frust` alone, plus `kurbo`/`peniko` where sanctioned),
  checkable with `cargo tree -p <crate> -e normal`.
- **A store shared with the OS namespaces its keys `frust.`** (NSUserDefaults, Android
  SharedPreferences) so plugin keys can't collide with other libraries'.
- **`dev.frust` is the embedding module's exclusive package; a plugin's Android Kotlin ships
  as a subpackage inside its own Gradle module (`plugins/<name>/platform/android`, e.g.
  `dev.frust.securestorage`), wired by `Contribution::GradleModule` — never rendered from a
  template or copied into the app.** AGP's `namespace` scopes generated `R`/`BuildConfig`
  per module (a plugin's namespace must differ from `dev.frust`), and Kotlin's `internal` is
  module-scoped (an `internal` embedding type stays invisible to a plugin module even under
  the same source package) — two confirmed side-constraints this rule is built around.
- **A plugin's theme-token folding pins a representative subset, never chases full
  design-token fidelity.** `frust-native-widgets`' theme ladder resolves a fixed, documented
  mapping of `Theme` roles into each control's props — no elevation, motion, glass, or
  per-state (hover/pressed/disabled) variants, which the platform's own drawables already
  provide for free; widening the mapping is additive, never a breaking republish.
- **A platform capability gap is recorded per-platform, never "corrected" onto the platform
  that doesn't have it.** `frust-native-widgets`' theme-ladder L1 (brightness) is
  deliberately asymmetric: Android bakes it at control-construction time
  (`createConfigurationContext` yields a `Context` consumed once, so a live brightness
  toggle needs a rebuilt view), while iOS re-pins `overrideUserInterfaceStyle` on every
  `update` and re-themes live. Mirroring one platform's constraint onto the other is a
  real defect class — match each platform's own capability, not its sibling's.
- **A native listener callback is wrapped into a plain signal-writing closure, not routed
  through frust's event machinery.** `frust-native-widgets`' events-as-signals convention
  (`crate::api::signals`) decodes each control's raw platform callback (Android JNI, iOS
  target-action) into the closure shape a builder's `.on_press`/`.on_toggle`/`.on_change`
  takes; since that callback runs on the platform main thread — the same thread the rest of
  frust runs on — an app closure just writes an `RwSignal` (`move |v| sig.set(v)`) and wakes
  the next frame normally, with no `TrackedScope`/`EventCtx` involved.
- **A generated-project mutation is idempotent, never a blind overwrite.** Adding an OS-side
  contribution (a Cargo dependency, a manifest permission, an Info.plist key, or a Gradle
  module include) checks first and no-ops if already present — the contract
  `frust-drive::plugin::add_plugin` implements (see [CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md)'s
  plugin-add data flow).
- **A call that blocks pairs with `spawn_blocking`, and every blockable path is
  UI-thread-guarded.** A gated/platform-answer call (secure-storage's biometric prompt;
  camera's `request_permission`/`take_picture`) fails fast with a typed error
  (`CameraError::UiThread`) on the platform's UI thread instead of parking there. A call
  that answers without waiting (e.g. an already-decided permission status) is exempt and
  stays callable from anywhere.
- **`frust-material`'s ported/vendored modules carry a 2–4 line attribution header**: upstream
  file or package, version/revision, license, and any porting decisions, e.g.
  `// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments)`. Binding for every
  module under `plugins/material/src/` ported from Dart/upstream source (tokens, shapes, icons); a
  from-scratch module carries none. `plugins/material/NOTICE`'s own convention section and
  attribution entries are the canonical reference.

## See Also

- [CODE_STANDARDS.md](CODE_STANDARDS.md) — the shared conventions every plugin also follows
- [PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md) / [NATIVE_WIDGETS_ARCHITECTURE.md](NATIVE_WIDGETS_ARCHITECTURE.md) — the units' design
- [PLUGINS_DEVELOPMENT.md](PLUGINS_DEVELOPMENT.md) — device gates and version pins
