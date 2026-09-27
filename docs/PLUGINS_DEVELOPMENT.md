# Frust - PLUGINS Development

Device gates and version pins owned by the PLUGINS unit (`frust-plugin` plus the
`plugins/` capability crates). Shared prerequisites, build/run commands, the standard verify
gate, the Android/iOS compile gates, and the version-pin *policy* live in
[DEVELOPMENT.md](DEVELOPMENT.md); the unit's design lives in
[PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md), its conventions in
[PLUGINS_CODE_STANDARDS.md](PLUGINS_CODE_STANDARDS.md).

## Shared-preferences manual test (desktop + Android + iOS)

A kill-and-relaunch persistence gate for `frust-shared-preferences`
(`plugins/shared-preferences`), against the scaffolded notes app template (`frust
create`'s default `lib.rs.tmpl`, persisting its notes list + draft):

- **Persistence:** add a note (and/or edit the draft), kill the app, relaunch it, and
  confirm it survived — desktop preview, an installed Android device, and an installed
  iPhone.
- **Old-scaffold graceful error:** a project scaffolded *before* the plugin existed must
  still boot with empty state rather than crash (`PrefsError::PlatformNotInitialized`,
  caught by the template's load path).
- **macOS storage-location caveat:** the unbundled desktop preview (no
  `CFBundleIdentifier`) writes `NSUserDefaults` to the global defaults domain rather
  than an app-specific plist — a storage-location difference, not a behavioral one.

## Secure-storage manual test (desktop + Android + iOS)

A device/emulator gate for `frust-secure-storage`, against an app that depends on the
plugin per **only** `plugins/secure-storage/README.md`:

- **Persistence:** store a value, kill/relaunch the app, confirm it reads back —
  desktop, an installed Android device, and an installed iPhone.
- **Biometric round-trip (physical device only):** open a store with
  `AuthPolicy::Required` following *only* the README's steps; confirm Face ID/Touch ID
  (iOS) or `BiometricPrompt` (Android API 28+) gates each call.
- **Old-scaffold graceful error:** a pre-plugin scaffold must surface a typed
  `PlatformNotInitialized`, never crash.
- **Add Plugin dialog:** clean scaffold builds with zero hand edits.
- **`--overwrite` caveat:** `frust create --overwrite` re-renders the project and drops
  every addition above; recovery is re-running Add Plugin (idempotent).

## Camera manual test (Android + iOS)

A device gate for `frust-camera` (`plugins/camera`), against an app depending on
the plugin per `plugins/camera/README.md`:

- **Permission + preview:** grant, deny, then re-grant via settings; confirm a live Mode B preview inside app chrome in both orientations.
- **Capture + stream:** still capture produces an orientation-correct JPEG; the stream toggle shows a live fps readout ([LIMITATIONS.md](LIMITATIONS.md)'s `cam-bgra-apple-only`).
- **A6 keep-alive:** scroll the preview slot off-screen and back; confirm it resumes without reopening the camera.
- **Torch:** toggle on/off on the back lens from playground's camera page; confirm `torch_available()` is false on the front lens; confirm torch survives starting/stopping the barcode scan strip.
- **Scan:** policy mode detects the dense muxr:// screen-QR once (NoDuplicates), timing mode shows decode ms + attempts/s for MEASUREMENTS.md.
- **Add Plugin dialog:** clean scaffold, both platforms build with zero hand edits.

## IAP manual test (Android + iOS)

A device/emulator gate for `frust-iap` (`plugins/iap`), against an app depending on the
plugin per `plugins/iap/README.md` §1 — the full checklist (Add Plugin scaffold builds
clean both platforms; a StoreKit-Testing smoke round trip needing no store account; a
store-account-gated purchase/finish flow once a Play listing or App Store Connect product
exists) is that README's §6, not repeated here.

## Video-player manual test (Android + iOS + macOS)

A device gate for `frust-video-player` (`plugins/video-player`), against an app depending on
the plugin per `plugins/video-player/README.md`, with `examples/playground`'s **Video** page
as the vehicle (three sources: an https MP4, an HLS playlist, and the bundled
`frust-video-sample.mp4` asset). Nothing below has been run on hardware yet.

- **Sources, all three platforms:** each source loads, plays, pauses, seeks (scrubber), and
  reports position/duration/video-size; a rate and a volume change take effect; loop restarts
  instead of reporting `Ended`.
- **Fit:** `Contain` letterboxes; `Cover` fills. Confirm on Android whether `Cover` actually
  crops — a `SurfaceView`'s compositor layer was observed on device (camera plugin) to ignore an
  ancestor's clip, in which case `Cover` spills over the surrounding chrome
  (act_000001a092461e09gnxOMe0U).
- **Slot lifecycle:** scroll the slot off-screen and back, and rotate — the picture reattaches to
  the same session rather than reopening it; closing the session leaves the slot empty, not stale.
- **Audio (iOS):** playback is audible with the ringer switch silenced, and `mix_with_others`
  ducks alongside another app's audio instead of interrupting it.
- **Background:** playback deliberately continues when the app backgrounds — confirm it does, and
  that an app pausing from its own lifecycle works (act_000001a092460caeP920M89C).
- **macOS (`cargo run` from the repo, plus a `frust build macos` bundle):** the `NSView` appears
  above the frust surface and tracks the slot as the window resizes and the page scrolls; controls
  below the slot keep working while the picture is opaque over anything drawn under it; a
  pointer/scroll gesture over the picture does not reach the frust scroll view beneath; closing the
  window removes the hosted view rather than leaking it. An unbundled `cargo run` has no
  `NSBundle` to resolve an asset name against, so the playground's Asset button falls back to a
  file beside the executable — copy the sample clip there first.
- **Overlay chrome (Android/iOS only):** the state chip over the slot is square-cornered on
  purpose — a rounded fill was found to composite incorrectly at a punched-hole edge
  (act_000001a05c9fb5e4jvgzTl2W, engine-owned). Do not "fix" it to the design system's rounding
  as part of this gate.
- **Add Plugin dialog:** clean scaffold builds with zero hand edits on both mobile platforms.

## URL-launcher manual test (desktop + Android + iOS)

A device/emulator gate for `frust-url-launcher` (`plugins/url-launcher`), against `examples/playground`'s
URL-launcher page (`it.f0x.playground`), which fires `UrlLauncher::open_external` against a fixed test
URL and, for the round-trip cases below, a `frustplay://` return link:

- **Launch, all three targets:** the page's button opens the device/desktop default browser on the
  test URL; the app itself is never blocked while the browser is foregrounding.
- **Return leg (Android):** `adb shell am start -a android.intent.action.VIEW -d frustplay://back?ok=1
  -p it.f0x.playground` after the browser opens; confirm the playground's `latest:` label updates with
  the returned link.
- **Return leg (iOS):** open `frustplay://back?ok=1` from Safari (or `xcrun simctl openurl
  booted frustplay://back?ok=1` on a simulator) after the browser opens; confirm the same
  `latest:` label update.
- **Observe:** the browser foregrounds over the app on every target; the `latest:` label reflects the
  return-leg link on Android/iOS (desktop has no return leg — see
  `url-launcher-desktop-no-deep-link-return` in [LIMITATIONS.md](LIMITATIONS.md)); no crash or hang on
  an invalid URL.
- **Old-scaffold graceful error (Android):** a project scaffolded before the plugin existed must
  report `UrlLauncherError::PlatformNotInitialized` rather than panicking.
- **Add Plugin dialog:** clean scaffold builds with zero hand edits on all three targets.
- **Cross-target clippy:** `cargo clippy -p frust-url-launcher --target aarch64-linux-android --
  -D warnings` and `cargo clippy -p frust-url-launcher --target aarch64-apple-ios-sim -- -D
  warnings`; the Windows arm (`windows-sys 0.61`, pinned in [SHELLS_DEVELOPMENT.md](SHELLS_DEVELOPMENT.md))
  gets `cargo check --target x86_64-pc-windows-gnu -p frust-url-launcher` per
  [DEVELOPMENT.md](DEVELOPMENT.md)'s cross-target Prerequisites — compile-checked only, until a real
  Windows rig runs it (`url-launcher-windows-leg-unrun` in [LIMITATIONS.md](LIMITATIONS.md)).

## Auth-session manual test (Android + iOS + macOS)

A device gate for `frust-auth-session` (`plugins/auth-session`), driven from `examples/playground`'s
Auth session page (`examples/playground/src/pages/auth_session.rs`) against the two committed gate
pages (`plugins/auth-session/gate`) — see that directory's own README for hosting the pages and its
zero-infra httpbin alternative, not repeated here:

- **Callback:** tap `Callback`. Expected: the in-app browser tab (Custom Tabs on Android,
  `ASWebAuthenticationSession` on iOS/macOS) presents `callback.html`, redirects immediately, and
  the status line reads `Callback -> Ok(Callback("frustplay://auth/callback?code=x&state=gate"))`.
- **Cancel:** tap `Cancel test`, then dismiss the presented tab without letting it navigate.
  Expected: `Cancel test -> Ok(Cancelled)`.
- **Busy:** tap `Busy`. Expected: `Busy -> Err(Busy)` — the second of two back-to-back `start`
  calls is rejected while the first is still live; dismiss the tab the first call opened afterwards
  to free the session slot for the next step.
- **Ephemeral:** tap `Cookie check` with `Ephemeral` off, confirm `cookie.html` reads back
  `frustauth=1` on a second run; toggle `Ephemeral` on and repeat, confirming the session sees no
  cookie set by an earlier non-ephemeral run (Apple's `prefersEphemeralWebBrowserSession`;
  Android's `CustomTabsIntent.Builder#setEphemeralBrowsingEnabled` is advisory to the browser).
- **Android double delivery:** the Callback step's `latest deep link:` line also updates on
  Android — the same callback URL additionally arrives as an ordinary deep link
  (`auth-session-android-callback-rides-intent-filter` in [LIMITATIONS.md](LIMITATIONS.md)); this
  is expected, not a bug — treat the future as the single authoritative source.
- **Add Plugin dialog:** clean scaffold builds with zero hand edits on both mobile platforms.
- **Gradle and R8 tripwires:** `(cd examples/playground/android && ./gradlew
  :frust-auth-session:compileDebugKotlin)` compiles the Kotlin host; `(cd examples/playground/android
  && ./gradlew :app:minifyReleaseWithR8)` then `grep dev.frust.authsession.FrustAuthSessionHost
  app/build/outputs/mapping/release/seeds.txt` confirms the JNI-referenced host and its init
  provider survive minification (`consumer-rules.pro`'s keep rules).

`plugins/auth-session/platform/android`'s JVM unit tests (`ProviderSelectionTest`, covering the
Custom Tabs provider-selection policy `FrustAuthSessionHost.chooseCustomTabsProvider` — this
policy regressed twice on-device with no automated coverage before this harness existed) run with
`(cd examples/playground/android && ./gradlew :frust-auth-session:testDebugUnitTest)`; no device,
emulator, or Robolectric needed — plain JVM JUnit 4 against the pure decision function.

## i18n manual test (desktop + Android + iOS)

A device gate for `frust-i18n` (`plugins/i18n`), against an app depending on the plugin per
`plugins/i18n/README.md` §2 — the full checklist (compile-time-validation failures, negotiation
+ fallback, persistence recipe) is that README's §9, not repeated here:

- **Add Plugin dialog:** clean scaffold builds with zero hand edits, both platforms.
- **playground i18n page:** the language switcher re-renders every section across en/de/ja; the
  plural stepper resolves the correct CLDR category per locale (`ja` has no `[one]` arm); the
  currency/date table renders locale-correct output; the system-locale readout matches the
  device's actual setting.
- **No visible tofu.** Interpolated text must render with **no visible tofu boxes** — the
  default FSI/PDI bidi isolation marks (`U+2068`/`U+2069`) are invisible by design; a visible
  box is a finding, not an accepted degrade ([LIMITATIONS.md](LIMITATIONS.md)'s
  `i18n-rtl-unverified`). The escape hatch is `LocaleSet::with_isolating(false)` plus a new
  [LIMITATIONS.md](LIMITATIONS.md) entry, never silently dropping isolation.

Keep this checklist in sync with `plugins/i18n/README.md` §9.

## Material asset regeneration

`frust-material`'s generated/bundled assets are checked in, not built by `build.rs`; regenerate by
hand when the source list changes:

- **Icons:** `python3 plugins/material/scripts/gen_icons.py` regenerates
  `plugins/material/src/icons.rs` from Google's `material-design-icons` SVGs (network fetch,
  cached under the gitignored `target/material-icons-cache/`); deterministic given the script's
  fixed icon list — a re-run with no source changes reproduces byte-identical output.
- **Fonts + NOTICE:** bundled Roboto Flex/Mono bytes live under `plugins/material/fonts/`;
  `plugins/material/NOTICE` and `FONTS-LICENSE` are hand-maintained, not generated — update both
  alongside any font or vendored-module change (see
  [PLUGINS_CODE_STANDARDS.md](PLUGINS_CODE_STANDARDS.md)'s attribution-header convention).

## Version Pins

The pins this unit owns, under [DEVELOPMENT.md](DEVELOPMENT.md)'s Version-Pin Policy (pins are
LAW; re-run the row's tripwire after touching it, and never run a blind `cargo update`):

| Pin | Why | Tripwire |
|---|---|---|
| `ndk-context 0.1` minor | `frust-plugin`'s Android platform-handle slot (written by `nativeInitPlatform`, read by every plugin) | `cargo check --target aarch64-linux-android -p frust-plugin` |
| `objc2 0.6` / `objc2-foundation 0.3` minor | Apple ObjC bridge (`frust-shared-preferences`'s `NSUserDefaults` backend; `frust-secure-storage`'s apple arm also pulls `objc2-foundation` for `NSString`/`NSError`; `frust-camera`'s apple arm pulls the full `objc2-av-foundation`/`objc2-core-media`/`objc2-core-video`/`objc2-quartz-core`/`dispatch2`/`block2` stack, each pinned `0.3`/`0.6` minor — `objc2-av-foundation 0.3.2` itself permits `objc2 >=0.6.2, <0.8.0`, wider than this workspace's own `0.6` caret) | `cargo check --target aarch64-apple-ios-sim -p frust-shared-preferences && cargo check --target aarch64-apple-ios-sim -p frust-camera` |
| `androidx.camera:camera-{core,camera2,lifecycle} 1.6.1` (Gradle, not a Cargo pin) minor | `frust-camera`'s Android CameraX session/preview stack (`plugins/camera/platform/android/build.gradle.kts`); deliberately not `camera-view` (no `PreviewView`) | `(cd examples/playground/android && ./gradlew :frust-camera:compileReleaseKotlin)` |
| `openiap-google 3.0.1` exact (Gradle, `plugins/iap/platform/android/build.gradle.kts`) / `OpenIAP` SPM `revision: "43ecc85bfda0fcd8f56d381e43d9d661afbd4729"` — `3.0.1`'s tag commit (`plugins/iap/platform/ios/FrustIap/Package.swift`) — LOCKSTEP | `frust-iap`'s Play Billing / StoreKit 2 hosts; upstream's `openiap-versions.json` pins spec/google/apple together, so the two sides must never be bumped independently (Play Billing 9.1.0 + kotlinx-coroutines ride transitively/alongside on Android). iOS pins the commit, not the mutable `3.0.1` tag itself — a payments-path dependency gets `clean-signals`-grade rigor (a force-moved tag upstream can't silently swap what a build resolves); bump the revision and its in-file tag-name comment together, in the same lockstep as the Android version | Gradle `:frust-iap:compileReleaseKotlin` + a consuming-app `xcodebuild` (Swift compiles under no cargo gate) + the R8 keep-rule tripwire (`consumer-rules.pro` survives real minification), vehicle `examples/playground/android` (its `:frust-iap` include wires the module — verified compiling under `:app:assembleDebug`): `(cd examples/playground/android && ./gradlew :app:minifyReleaseWithR8)`, then `grep dev.frust.iap.FrustIapHost app/build/outputs/mapping/release/seeds.txt` must match (class + native methods kept, not in `usage.txt`) |
| `objc2-security 0.3` / `objc2-local-authentication 0.3` minor | `frust-secure-storage`'s Apple Keychain backend + biometric gate (`SecAccessControl`/`LAContext`) | the secure-storage mobile compile gates in [DEVELOPMENT.md](DEVELOPMENT.md)'s Test section |
| `objc2-ui-kit` / `objc2-quartz-core` / `objc2-core-text` / `objc2-core-foundation` 0.3 minor | `frust-native-widgets`'s (`plugins/native-widgets`, the NATIVE_WIDGETS unit — this row lives here because the same pins are shared with `frust-clipboard`/`frust-haptics`) UIKit binding, plus its theme-ladder L2 (`objc2-quartz-core`'s `CALayer.cornerRadius`) and L3 (`objc2-core-text`/`objc2-core-foundation` resolving the embedded Glyph font bytes to a `CTFont`) direct pins — each already resolved transitively before this crate named it directly, so no lockfile version change. Also consumed by `frust-clipboard` (iOS `UIPasteboard`), `frust-haptics` (the three `UI*FeedbackGenerator` classes), `frust-video-player` (the iOS `UIView` its Rust factory hosts an `AVPlayerLayer` in), and `frust-url-launcher` (`UIApplication`'s `openURL:options:completionHandler:`, which is the one consumer that also enables this pin's `block2` feature — Cargo unifies features, so every other `objc2-ui-kit` 0.3.2 consumer above compiles those block-based bindings too; no version moved, only a dependency edge was added), and `frust-auth-session` (its iOS presentation anchor walks the same `UIApplication`/`UIWindowScene`/`UIWindow` chain; its macOS anchor rides `objc2-app-kit`'s `NSWindow` instead — see its own `objc2-authentication-services` row below). `frust-shell-macos` rides the same `objc2` line from the other side — its `objc2-app-kit` `NSView` feature already implies `objc2-foundation`'s `NSGeometry` and `objc2-core-foundation` (where `CGRect`/`CGFloat` live), so the pair is named explicitly in that manifest to make a future narrowing of either default set fail loudly instead of silently deleting the geometry types. `cargo tree -i objc2-ui-kit` legitimately shows two versions — `0.2.2` pulled transitively by `accesskit_ios`/`winit`, `0.3.2` consumed by `frust-native-widgets`/`frust-clipboard`/`frust-haptics`/`frust-video-player`/`frust-url-launcher`/`frust-auth-session` — an expected split, not `cargo tree -d` drift; do not force-align them | `cargo check --target aarch64-apple-ios-sim -p frust-native-widgets -p frust-clipboard -p frust-haptics -p frust-video-player -p frust-url-launcher -p frust-auth-session` **and** `cargo check --target aarch64-apple-darwin -p frust-shell-macos` |
| `androidx.media3:media3-exoplayer` / `media3-exoplayer-hls` `1.11.0` exact (Gradle, `plugins/video-player/platform/android/build.gradle.kts`) — LOCKSTEP | `frust-video-player`'s Android ExoPlayer session and its HLS support. Both artifacts move together (mixing two Media3 versions in one app is unsupported upstream), and `media3-exoplayer-hls` is required even though nothing in the module names an HLS type: `DefaultMediaSourceFactory` loads `HlsMediaSource.Factory` reflectively, so a missing artifact fails at playback rather than at build time. `media3-ui` is deliberately absent — `PlayerView`'s fill modes rely on an ancestor clipping a compositor layer, which frust's window arrangement was observed not to honour. These AARs stamp `minCompileSdk = 36`, so the module's `compileSdk = 36` is a hard floor a bump must not lower | `(cd examples/playground/android && ./gradlew :frust-video-player:assembleDebug)` |
| `objc2-avf-audio 0.3` minor (`AVAudioSession` + `AVAudioSessionTypes` features) | `frust-video-player`'s iOS-only audio-session configuration — the playback category (audible with the ringer switch silenced) and the `MixWithOthers` option. Both features are required: the category call is unreachable with either alone. macOS has no `AVAudioSession` at all, so this row is iOS-gated by the platform's own asymmetry | `cargo check --target aarch64-apple-ios-sim -p frust-video-player` |
| `objc2-authentication-services 0.3` minor | `frust-auth-session`'s Apple `ASWebAuthenticationSession` presentation (the RFC 8252 in-app browser tab); its `ASFoundation` feature split transitively pulls `objc2-app-kit`'s `NSResponder`/`NSWindow` on macOS the same way `objc2-ui-kit`'s pulls `UIWindow` on iOS — one session type, two `target_os`-gated anchor dependencies | `cargo check --target aarch64-apple-ios-sim -p frust-auth-session && cargo check --target aarch64-apple-darwin -p frust-auth-session` |
| `androidx.browser:browser 1.10.0` (Gradle, not a Cargo pin) minor | `frust-auth-session`'s Android Chrome Custom Tabs host (`plugins/auth-session/platform/android/build.gradle.kts`); `CustomTabsIntent.Builder#setEphemeralBrowsingEnabled`, confirmed present on this exact version via `javap`, is what makes `AuthSessionRequest::ephemeral` a real advisory flag on Android rather than a silent no-op | `(cd examples/playground/android && ./gradlew :frust-auth-session:compileDebugKotlin)` |
| `keyring-core 1.0` / `zbus-secret-service-keyring-store 1.0` (`rt-async-io-crypto-rust` feature, keeps the plugin tokio-free) / `windows-native-keyring-store 1.1` minor | `frust-secure-storage`'s desktop Linux/Windows backend | `cargo test -p frust-secure-storage` |
| `arboard =3.6.1` exact | `frust-clipboard`'s desktop (macOS/Linux/Windows) text-clipboard backend — **shared across three consumers**: `frust-shell-desktop`'s own clipboard route rides this same row (see [SHELLS_DEVELOPMENT.md](SHELLS_DEVELOPMENT.md)), and so does the TUI unit's `frust-tui::clipboard` system backend (see [TUI_DEVELOPMENT.md](TUI_DEVELOPMENT.md)) — one pin, three consumers, so a bump is all three units' business. Same shape across them: `default-features = false` drops the default `image-data` feature and `wl-clipboard-rs`'s native-Wayland `wayland-data-control` feature is deliberately not enabled. `frust-clipboard` and `frust-shell-desktop` hold arboard differently — a `Clipboard` per call on the caller's thread versus one kept open for the process lifetime on a clipboard worker thread — so both can be live on different threads of the *same* app process at once, which each manifest records as observed to crash arboard's macOS backend; an app using this plugin's API *and* a clipboard-capable widget on desktop is in unsupported territory. The TUI holds its own per-write instance in its own process, never sharing a process with the plugin or the shell, so this two-live-instances hazard is unchanged by its addition as a third consumer | `cargo check -p frust-clipboard` **and** `cargo check -p frust-shell-desktop` **and** `cargo check -p frust-tui` (all three arms, every time) |
| `rusqlite =0.40.1` exact, `default-features = false, features = ["bundled"]` | `frust-database`'s default (`engine-sqlite`) backend; `bundled` compiles SQLite `3.53.2` via `cc` — this crate's own conformance suite is the bump tripwire | `cargo test -p frust-database` |
| `turso =0.7.2` exact, `default-features = false` | `frust-database`'s optional `engine-turso` backend (pre-1.0 rewrite); dropping the default `mimalloc`+`fts` features saves ~4 MB. Needs libclang on the host — [DEVELOPMENT.md](DEVELOPMENT.md)'s Prerequisites. `scripts/size-report.sh` is a whole-artifact snapshot, not a delta tool; `plugins/database/README.md` §6 documents the two-release-build procedure that produced the recorded size delta, reproducible on a pin bump | `cargo test -p frust-database --features engine-turso` |
| `fluent-bundle 0.16` / `fluent-langneg 0.13` / `unic-langid 0.9` / `fluent-syntax 0.12` minor | `frust-i18n`'s Fluent Project message-bundle engine, locale negotiation, BCP-47 locale type, and (via `frust-i18n-macros`) the `locales!` macro's compile-time `.ftl` syntax check — all pre-1.0, same upstream `fluent-rs` repo, same churn risk | `cargo test -p frust-i18n` + `cargo check --target aarch64-linux-android -p frust-i18n && cargo check --target aarch64-apple-ios-sim -p frust-i18n` |
| `icu_decimal`/`icu_datetime`/`icu_plurals 2.2` + `icu_locale_core 2.3` + `tinystr 0.8` + `icu_provider 2.3` (`sync` feature) minor | `frust-i18n`'s optional `formatting` feature — ICU4X number/date/plural formatting. MUST stay unified with the lockfile's parley-driven ICU4X line (`icu_collator`/`icu_normalizer`/`icu_properties`/`icu_segmenter`, pulled transitively by `frust-text`'s shaping stack) — never bumped independently of it, or a duplicate ICU4X major enters the graph. `icu_provider`'s `sync` feature swaps every ICU4X `DataPayload`'s `Rc` for an `Arc` globally (feature unification), an accepted swap that reaches parley's own transitive ICU4X crates in any `formatting` build too, not just this crate's | `cargo test -p frust-i18n` + `cargo check --target aarch64-linux-android -p frust-i18n && cargo check --target aarch64-apple-ios-sim -p frust-i18n`, plus `cargo tree -p frust-i18n --features formatting -d` (no duplicate `icu_*` majors) |
| `icu_experimental =0.5.0` EXACT | `frust-i18n`'s currency + percent formatter surface (`fmt/currency.rs`, `fmt/number.rs`); an explicitly experimental, pre-1.0 ICU4X crate ("all code in this crate is unstable" per its own doc) that major-bumps every ICU4X release — 0.5.0 is the release whose own dependency line still resolves to this workspace's already-present 2.2.x ICU4X components. A currency-formatting dependency gets `clean-signals`-grade rigor: never bump without re-verifying the whole `icu_*` resolve | `cargo test -p frust-i18n` + `cargo check --target aarch64-linux-android -p frust-i18n && cargo check --target aarch64-apple-ios-sim -p frust-i18n` |
| `sys-locale =0.3.2` exact | `frust-i18n`'s desktop (macOS/Linux/Windows) system-locale detection backend — a small, young surface with no minor-version API-stability track record yet | `cargo test -p frust-i18n` + `cargo check --target aarch64-linux-android -p frust-i18n && cargo check --target aarch64-apple-ios-sim -p frust-i18n` |
| `trybuild 1` (dev-only) | `frust-i18n`'s dev-only compile-fail harness for the `locales!` macro's diagnostics (`tests/macro_diagnostics.rs`); never reaches a shipped dependency graph | `cargo test -p frust-i18n --no-default-features --test macro_diagnostics -- --ignored` (regenerate expectations with `TRYBUILD=overwrite`) |
| `junit:junit 4.13.2` exact (Gradle, `testImplementation`, `plugins/auth-session/platform/android/build.gradle.kts`) | `frust-auth-session`'s `ProviderSelectionTest` — JVM-only coverage of the pure `chooseCustomTabsProvider` decision function; dev-only, never reaches a shipped AAR | `(cd examples/playground/android && ./gradlew :frust-auth-session:testDebugUnitTest)` |

## See Also

- [DEVELOPMENT.md](DEVELOPMENT.md) — prerequisites, build/run/test gates, version-pin policy
- [PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md) — the unit's design
- [PLUGINS_CODE_STANDARDS.md](PLUGINS_CODE_STANDARDS.md) — the unit's conventions
- [NATIVE_WIDGETS_ARCHITECTURE.md](NATIVE_WIDGETS_ARCHITECTURE.md) — the native-control plugin
