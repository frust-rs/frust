# Changelog

All notable changes to Frust are recorded here, newest first, in the
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) shape. GitHub release notes are copied
from the section for the tag (see [docs/RELEASING.md](docs/RELEASING.md)).

## Unreleased

### Breaking

- Android `minSdk` is now 29 (Android 10), up from 26, in the embedding, every plugin module, the
  scaffold template and the example apps. An app scaffolded earlier must set `minSdk = 29` in
  `app/build.gradle.kts` (or its manifest) or the manifest merger fails. The text stack needs the
  NDK font API (`AFontMatcher_match`, API 29+) instead of parsing the `fonts.xml` that Android 15
  deprecates.

### Added

- Text-hardening phase 4: macOS oracle tests and the system emoji-fallback test check their real
  precondition and print a counted skip instead of returning early; a bundled COLRv1 emoji subset
  exercises emoji fallback on every host. The Linux T400 golden class grows from 33 to 70 pairs
  (`page-material-home` escalation row now names the re-measured value, no margin). Optional
  macOS/Windows text-oracle CI legs and an Android RELR tag guard job run outside `ci-ok`;
  hot-patch Android `.so` links use `--pack-dyn-relocs=relr --use-android-relr-tags`; hygiene
  clean-ups (parley attributions, `mod shape`, native-widgets 29-floor comments, scaffold RELR
  comment). The `-P 26` NDK gate references in the plugin docs are now `-P 29`.
- `frust::texture::request_frame`: a producer that has a new frame owes the shell one frame, so an
  idle `texture_view` stays live without a ticker. `TextureSink::set_waker` (`SinkWaker`) runs
  after every push; the camera and video-player bridges install `request_frame` there while their
  source is registered. The mobile frame gates and the desktop loop take the owed frame
  (`frust_render::texture_bus::{request_frame, take_frame_owed, set_frame_request_hook}`); the web
  shell does not yet (`texture-bus-no-wake-on-push`).
- The iOS C export `frust_overlay_take_released` (additive, declared in `frust_ffi.h`, emitted by
  `ios_app!`): it hands back the overlay layers whose surface has dropped.
- `frust-shell-common::overlay`: the shared `OverlayTable`, `OverlaySwitch`, `FrameRequest`,
  `assemble_overlay_cuts` and `SkewWindow` that the Android, iOS and macOS overlay hosts now use.
- `frust_native_widgets::native_scaffold`: app chrome the host owns — a tab bar, title, leading
  item, actions and an optional bottom toolbar (iOS `UITabBarController`, Android Material 3 bars
  and floating action buttons, frust-drawn bars elsewhere). It brings `ChromeEvent`, `ActionItem`,
  `MenuItem`, `Leading`, `ScrollState`, `ToolbarSpec` and `EdgeFade`, and the builder methods
  `.toolbar`, `.edge_fade`/`.no_edge_fade` and `.resize_to_avoid_keyboard`.
- `frust-native-widgets` Android arms for `native_segmented` (`MaterialButtonToggleGroup`) and
  `show_native_sheet` (`BottomSheetDialog`).
- iOS embedding: `FrustSceneDelegate.makeWindowRootViewController(hosting:)` and
  `frustViewController`, so a host can place the one frust surface inside its own container.
- `frust-i18n` formatting: `fmt::currency_minor` and `fmt::minor_unit_digits` (exact integer
  minor units, FTL `NUMBER(..., minorUnits: "true")`); zoned formatting over `jiff`'s bundled tz
  database (`datetime_in_zone`, `date_in_zone`, `time_in_zone`, `datetime_at_offset`, FTL
  `DATETIME(timeZone: ...)`, and ISO `Z` or `±HH:MM` suffixes on FTL date strings); `fmt::time_range`,
  `time_range_in_zone`, `date_range` and `datetime_range_in_zone`
  (`i18n-interval-hand-rolled`); `fmt::relative_day` and FTL `RELATIVE_DAY`; `fmt::units` and
  `duration_minutes`; `fmt::list`, `language_name` and `language_endonym` with FTL `LIST`;
  `DateTimeOptions` (calendar, hour cycle) with `_with` variants, and `NumberOptions` with compact
  notation plus the FTL `NUMBER` options (`minimumFractionDigits`, `maximumFractionDigits`,
  `notation`, `compactDisplay`, `useGrouping`). The bundled tz database is recorded as
  `i18n-tzdb-bundled-size`.
- `frust-i18n` optional `persistence` feature: restores the user's chosen locale through
  `frust-shared-preferences`.
- `frust-material` `MaterialStrings`: a theme extension carrying every label and accessibility
  string, read by every widget with explicit setter > theme extension > English precedence, with
  new per-widget setters (`close_label`, `dismiss_label`, `remove_label`, `collapse_label`,
  `expand_label`, `NavigationDrawerView::label`, `back_label`, `clear_label`, `search_hint`); the
  date and time pickers default their strings from it. `SnackbarController::enqueue`,
  `clear_queue` and `queue_len` add a FIFO notice queue beside `show`.
- `frust-material-i18n` (`plugins/material-i18n`): a facade-tier bridge crate with localized
  `MaterialStrings` tables for en, th, vi, ja, es, de and fr (CLDR month and weekday names) and
  `provide_localized_strings`, `localized_theme` and `material_strings_for`. The non-English
  tables are machine-drafted (`material-strings-machine-drafted-locales`).
- The registry lists `frust-material-i18n` as its twenty-third plugin entry in the Add Plugin picker.
- `examples/playground` (locale switcher) and `examples/material3-demo` (language toggle on the
  theme settings page) demonstrate the localized Material strings.

- App-drawn title bars on desktop: `DesktopConfig::with_decorations` (`Decorations::{Full, None,
  NoTitleBar}`, creation-time only) and `with_title_buttons` (`TitleButtons`, macOS placement of the
  traffic lights under `NoTitleBar`). `frust::{WindowCommand, ResizeEdge, push_window_command}`
  ask the desktop shell to move, resize, maximize, minimize, fullscreen or close the window, and the
  `WindowState` context reports maximized (false while fullscreen), fullscreen, focused and
  minimized. Off desktop the commands do nothing (`desktop-window-commands-off-desktop-noop`). The
  Linux, Windows and macOS gates passed; open: `desktop-windows-menu-bar-undecorated-unverified`.
- `window_drag_region`: wraps a title bar or edge strip and turns a primary press its child ignored
  into a window move (double press toggles maximize) or, with `.resize(edge)`, a resize drag.
- `examples/custom-titlebar` draws a title bar with the window controls; the playground has a
  Window row for minimize, maximize-restore, fullscreen and the live `WindowState` readout.
- `frust new` apps declare a `frust-debug` feature (never passed by frust) so `Cargo.lock` pins
  `frust-hotpatch`, `frust-devtools` and `frust-devtools-protocol`; an existing app can opt in with
  the one line `frust-debug = ["frust/perf-trace", "frust/devtools", "frust/hotpatch"]` in
  `[features]` (its own dependency key if it renamed `frust`).
- `TextStyle` variation settings and width, `RenderCapabilities` (what the renderer can draw,
  filtering font-fallback candidates), and `TextEditor::has_preedit`.
- The app default locale, the language of untagged text: `frust_text::set_default_locale`,
  `default_locale` and `default_locale_generation`, re-exported as `frust::authoring::text` and
  written by `frust-i18n`'s active locale. Shells re-lay-out on a change; `Text` and `TextInput`
  key their layout caches on its generation.

### Changed

- The `frust-text` stack is frust's own: a font oracle (CoreText, the NDK font matcher, fontconfig,
  DirectWrite, the web backend), a shaping driver over `parley_engine`, frust's line layout with
  native max-lines and ellipsis, and frust's own editor. `TextEditor` no longer sits on parley's
  `PlainEditor`: backspace over an emoji grapheme removes the whole grapheme, and editors honour
  alignment within the wrap width.
- `DesktopConfig` gained two fields, `decorations` and `title_buttons`. Builder construction is
  unaffected, and the hand-written `Default` behaves as before; struct-literal construction needs
  the new fields.
- Overlay withdraw never blocks the UI thread on Android, iOS or macOS. All three hosts share one
  overlay table; a layer comes back only after its surface dropped. On iOS `frust_overlay_detach`
  returns at once, so a host keeps a detached view hidden until `frust_overlay_take_released`
  returns its layer (the shipped `FrustViewHost` does); macOS removes a withdrawn view when the
  release arrives.
- Android: the overlay lock is no longer held across the main-surface acquire, and
  `surfaceDestroyed` for an overlay does not wait. An overlay out for a frame is released when that
  frame lands, so one frame may present into a window Android already destroyed; the present fails
  and is logged at debug (`android-overlay-withdraw-after-return`).
- The overlay switch takes the first non-blank source — runtime `FRUST_OVERLAYS`, compile-time
  `FRUST_OVERLAYS`, then the same two for the `FRUST_OVERLAY_SPIKE` alias — so a runtime
  `FRUST_OVERLAY_SPIKE=0` switches off an alias value baked in at compile time.
- iOS overlay views host their `CAMetalLayer` as a sublayer sized on the main thread, and the
  render thread leaves `drawableSize` alone.
- `frust-native-widgets`' Android module depends on Material Components 1.14.0 (`api`), and a
  consuming app needs a Material3-derived theme (`native-chrome-android-material3-theme-required`).
- iOS tab selection under `native_scaffold` is native: a tap selects at once and the bar converges
  to the app's spec after a ~250 ms settle, so apps must confirm in `on_tab_select`.
- `examples/native-widgets-demo` is rewritten as four native-chrome tabs (Home, Controls, Dialogs,
  Lab); the `section/<label>` deep links still work and `tab/<slug>` opens a tab.
- In the TUI, a Debug/Profile desktop session that cannot spawn `cargo run` reports it in the
  session tab instead of a toast.

### Removed

- The `parley` dependency, and the Hiragino Han stopgap for iOS.

### Fixed

- Bold on variable system faces (Android Roboto, macOS SF) and CJK faux bold now render at the
  requested weight: the engine draws glyphs at the face's variation instance and synthesized
  embolden, and the glyph atlas keys by instance. Faux italic on a face with no italic stays
  upright by default.
- macOS overlays follow the window's backing scale: a move between a 2x and a 1x display
  republishes the overlay layers, which stay sharp at each scale.
- `frust-url-launcher`: `UrlLauncherError::NoHandler` reads "no handler for this URL on this
  device" instead of naming only `http`/`https`, since `open_maps_native` returns it for native
  map schemes on desktop.
- iOS Han text draws in PingFang at the requested weight, Japanese kana draw in Hiragino, and
  untagged kanji follow the app locale. `frust-text` logs a warning once per font when visible text
  shapes to `.notdef`, and `frust_text::notdef_glyph_count()` reports the process total.
- The first Debug/Profile or hot build after a frust release no longer fails with `failed to select
  a version for the requirement frust-hotpatch/frust-devtools ...` when the machine's crates.io
  index cache predates the release: the CLI, TUI and MCP server run a cargo-resolve preflight
  (`frust_drive::lock_refresh`) that refreshes those index entries (manifest untouched) and
  otherwise prints the exact remedy.

## 0.6.3 — 2026-10-20

A patch release (the 0.6.x line continues) that carries source changes for code that matches
exhaustively on `InputEvent`, `OverlayEventKind` or `NamedKey`, builds `OverlayEntry`/`OverlayHit`
literals, or compares `DesktopConfig` with `Eq` (the Changed list marks them); `NamedKey` is
`#[non_exhaustive]` from this release, so its one wildcard arm is the last such change it asks for.

The engine's per-frame and startup work shrank: per-frame uploads go through a reused staging ring,
glyph replay stops re-rasterising slowly moving text, the render thread keeps the UI thread's
priority, Android's per-frame polls collapse into one JNI crossing, and the first frame no longer
waits for the whole pipeline warm-up or for shader modules it does not draw. Desktop apps gain
keyboard shortcuts with a per-paint registry and a command-palette-ready hint list, Tab traversal
with a keyboard-only focus ring, function keys, native file dialogs (`frust-file-dialogs`), window
size limits and opt-in window-state persistence, and an opt-in baseline scrollbar. Measured rows,
misses included, are in docs/PERFORMANCE_BASELINES.md and benchmarks/RESULTS.md.

### Performance

- Engine uploads and glyph replay. The engine's per-frame uploads (coverage, encoded paints,
  instances, viewport and page uniforms) are written into a reused, mapped staging ring and copied
  inside the frame's own encoder instead of one queue write each (`frust_gpu::staging`;
  `FRUST_ENGINE_NO_STAGING_RING=1` or `EngineRenderer::set_staging_ring(false)` restores queue
  writes). Bind groups naming a pooled or retained page are kept across frames. The resident
  glyph-run cache now keeps every subpixel sibling of the glyphs on screen, so slowly moving text
  stops re-rasterising. Inline-path protocol against 0.6.2, S1: `upload_us` Pixel 4a 571 to 216,
  OnePlus 9 358 to 121; `replay_us` 1283 to 0.3 and 1184 to 0.3; enc total 4523 to 2797 and 4103 to
  3350. The `frust-perf enc` line appends `staged_bytes`, `staging_busy`, `staging_too_large`,
  `staging_buffers` and `bind_groups`.
- Thread scheduling and Android polls. The mobile render thread joins a frame thread class that
  keeps the UI thread's inherited priority instead of Android's fixed nice -4, and blocking-pool
  threads drop to a background class (ten steps below the UI thread on Android and Linux,
  `QOS_CLASS_UTILITY` on Apple) on their first job (`frust_reactive::thread_policy`;
  `FRUST_NO_THREAD_TUNING=1` restores the old shape). Kotlin's per-frame polls are gated by one
  `nativeFrameSync` crossing (`FRUST_NO_FRAME_SYNC=1` polls everything): per-frame JNI cost on an
  idle page 166 to 36 us (Pixel 4a) and 140 to 30 us (OnePlus 9); S1, S4 and S5 frame times
  unchanged within noise. An Android performance-hint (ADPF) session over the UI and render threads
  is available with `FRUST_ADPF=1`, default off: it measured no better as a default
  (`android-adpf-opt-in-measured` in docs/LIMITATIONS.md).
- First frame and pipeline cache. The warm-up list opens with the programs a first frame draws and
  ends with the surface layer composite, and the first frame after each surface install takes only
  what it needs from it (`FRUST_FIRST_FRAME_WARM_UP=steal`, the default; `await` and `finish` are
  diagnostics). Android's split render thread persists the pipeline cache once the warm-up is done
  and again on surface destroy. Split path, warm launches: `first_encode_done` to
  `first_frame_presented` median Pixel 4a 194 to 24 ms (pipeline wait 183.5 to 12.5), OnePlus 9 74
  to 11 ms (66 to 3); the persisted blob grows from 328 B (0 pipelines) to 117,143 B (18) and from
  314 B to 116,758 B; S1 steady state unchanged. Method and every row:
  [docs/PERFORMANCE_BASELINES.md](docs/PERFORMANCE_BASELINES.md) § Engine close-out per device.
- Device open. The engine's shader modules are registered at install and created on first use, the
  first frame's four side by side on a short-lived pool, instead of all twelve at install: device
  open (`adapter_ready`) is back to September's figures on the three phones measured, OnePlus 9 111
  to 32 ms, Pixel 5 189 to 40, iPhone SE 82 to 12; `first_frame_presented` 124 to 67 and 214 to 108
  ms on Android (the iPhone's first frame follows the system-font join). On Android the first frame
  now waits for part of the creation (OnePlus 9 11 to 32 ms, Pixel 5 14 to 54 after encode);
  `android-first-frame-carries-shader-module-creation` in docs/LIMITATIONS.md.

### Changed

- `frust-core` internals. `app.rs` and `event.rs` are now the `app/` and `event/` directory modules,
  with no public path changed; `RenderRoot::event` is decomposed into named steps; the crate's
  comments are cut to the contracts they state (at most 25 % of any region); and the
  `comment_density_budget` test in `frust-ui` keeps them there.
- Desktop window sizing. `DesktopConfig` derives `PartialEq` but no longer `Eq` (its new size
  fields are `f64`-backed). An unset or invalid `FRUST_WINDOW_SIZE` now leaves the size to the saved
  window state, then `DesktopConfig::initial_size`, then 800×600; a set `FRUST_WINDOW_MAXIMIZED`
  other than `1`/`true` un-maximizes a saved maximized window. The one-time window log line prints
  `unset` for an unset knob.
- Keyboard input (breaking for exhaustive matches). `InputEvent::Shortcut` and
  `OverlayEventKind::{Shortcut, Key}` are new variants, so a downstream exhaustive `match` on either
  enum needs an arm; `NamedKey` is now `#[non_exhaustive]` (and gains `F1`–`F24`, under Added), so
  an exhaustive `match` on it needs a wildcard arm once — a variant added later no longer breaks it;
  `OverlayEntry` and `OverlayHit` gained a `keys` field, so a struct literal needs
  it (`OverlayKeys::Pass` keeps the old behaviour). The new `Widget::is_focusable` and
  `Widget::visit_focus_children` are defaulted. `TextInput` now declines a chord it does not decode
  instead of consuming it, so app shortcuts reach the registry while a field has focus; it still
  decodes Ctrl/Meta+C/X/V/A and the legacy Insert/Delete chords.
- Shortcut matching and hints. A symbol chord (a character key that is neither alphanumeric nor
  whitespace, such as `?` or `+`) ignores Shift: `Chord::new` clears it, `Chord::matches` ignores
  it, and `"Shift+?"` parses as `"?"`; letters, digits, Space and named keys still compare exactly.
  Shortcut hints are per root: `RenderRoot` owns the previous paint's snapshot and seeds its build
  and paint contexts, a component's inner build included; the thread-local `LAST_HINTS` is gone, and
  a context no root seeded reads empty.
- Focus behaviour. The baseline `Button`, `Checkbox`, `Radio` and `Slider` take focus on a primary
  mouse press (pointer-sourced, so `KeyboardOnly` paints no ring) and the next Tab continues from
  there; a touch tap activates without claiming focus (an unclaimed Down still blurs a plain
  sibling field; a dialog or sheet re-claiming focus on its chrome keeps it). A focused
  `TextInput`'s bare Escape blurs it and is declined, so a modal around it closes on the first
  Escape — an app chord bound to a bare Escape now fires on the press that leaves a field. A paint
  that registers an `OverlayKeys::Modal` surface ends a main-tree focus session (nothing restores it
  on close; a light-dismiss menu does not end it). `OverlaySlot` visits its floated pod for Tab traversal, so a non-modal
  pod's stops follow its owner's. Every shadcn control uses `KeyEvent::is_activation`, so none
  activates on an auto-repeat or a modified Space/Enter any more.
- The baseline scrollbar paces slow fades: a fade whose per-frame step is under one 8-bit opacity
  level holds until the next visible level instead of asking for every frame.
- Desktop window state. A run with a valid `FRUST_WINDOW_SIZE` or any `FRUST_WINDOW_MAXIMIZED` set
  reads and applies the saved state but never writes it (a one-time `window persist=` log line says
  which). The un-maximized geometry is seeded from the creation geometry and tracked on every
  un-maximized change, so a window maximized before its first save keeps its normal size;
  fullscreen saves nothing and is never restored; the first failed write per process warns, later
  ones log at debug.
- `frust-file-dialogs`: the desktop and fallback backends are separate modules behind one desktop
  predicate (no API change); its docs add threading and zenity `|` multi-select notes, and the
  `rfd` cost is recorded in docs/PERFORMANCE_BASELINES.md (+262,632 B stripped, +58.2 s clean
  release build on the reference box; desktop only).

### Added

- `frust_gpu::staging` (`StagingRing`, `StagingFrame`, `StagingStats`) and
  `PipelineCache::{warm_up_done, wait_warm_up}`; `EngineRenderer::{set_staging_ring, staging_ring,
  last_frame_staged_bytes, staging_stats, bind_groups_created, warm_up_done, wait_warm_up}`;
  `SurfaceRenderer::{pipeline_warm_up_done, compiled_pipelines}`.
- `frust_reactive::thread_policy` (`ThreadClass`, `set_current_thread_class`,
  `runtime_policy_summary`, `thread_tuning_enabled`); `frust-reactive` now depends on `libc`.
- Android: the additive `nativeFrameSync` and `nativePerfTrace` JNI exports, and a `frust-perf jni`
  line per 120 frames on a `perf-trace` build.
- Startup sub-step stamps on the `frust-perf startup` line, printed after `adapter_ready` as
  durations in ms: `engine_surface_ms` (with `device_create_ms`, a part of it, on its own),
  `cache_seed_ms`, `caps_probe_ms`, `shader_modules_ms`, `warm_up_start_ms`, `engine_rest_ms` and
  `install_rest_ms` split the renderer's first install, `font_system_ms` and `font_app_sync_ms`
  split `TextContext::new`, and iOS adds `render_thread_start_ms` (the `perf` module of
  `frust-shell-common` documents each). Behind them: `EngineRenderer::construction_timings`
  (`ConstructionTimings`), `SurfaceRenderer::take_install_timings` (`InstallTimings`) and
  `TextContext::new_with_timings`. Measurement only; no behaviour or order of work changed.
- `frust-file-dialogs` (`plugins/file-dialogs`): native open, open-multiple and save-as dialogs on
  Linux (xdg-desktop-portal, `zenity` fallback), macOS and Windows through `rfd` 0.17; blocking
  calls meant for `frust::spawn_blocking`; Android, iOS and web report `Unsupported`. Listed in the
  Add Plugin picker as the registry's twenty-second plugin.
- `DesktopConfig::{with_initial_size, with_min_size, with_max_size, with_persist_window_state}`:
  initial and min/max window sizes, and opt-in persistence of window size, position and maximized
  state under the data directory, fitted to the connected monitors on restore (see
  docs/SHELLS_DEVELOPMENT.md § Desktop window geometry).
- An opt-in baseline scrollbar: `ScrollView::scrollbar` and `ListView::scrollbar` take a
  `ScrollbarStyle` (`ScrollbarShow::{Never, WhileScrolling, Hover, Always}`), re-exported from the
  facade; an overlay bar with hover widening, an idle fade, thumb drag and track paging.
- Keyboard shortcuts: `Chord` (`Chord::parse("Mod+K")`, `Mod` is Command on macOS/iOS and Ctrl
  elsewhere; `Chord::display`), `ShortcutDecl`/`ShortcutId`/`ShortcutScope`/`ShortcutHint`, and a
  per-paint registry declared through `PaintCtx::register_shortcut`; a key the focus path declines
  is matched topmost-overlay, then `Local`, then `AppGlobal`. App code binds chords with the
  `shortcuts(child).on(..)`/`.global(..)`/`.bind(..)` wrapper (`ShortcutsView`), and
  `RenderRoot::shortcut_hints`, `BuildCtx::shortcut_hints` and `PaintCtx::shortcut_hints` list the
  previous frame's declarations for a palette.
- Modal key scoping and declined-key forwarding: `OverlayKeys::{Pass, Modal}` on a floated surface
  (`OverlaySlot::set_keys`/`take_escape`, `OverlayPortalView::keys`/`on_escape`) masks main-tree
  shortcuts and takes a declined Escape; `EventCtx::forward_declined_keys_to` and
  `consume_overlay_key` hand an editable's declined keys to its owner's floated pod.
- Tab / Shift+Tab focus traversal at the root over `Widget::is_focusable` stops, confined to an open
  modal surface, and `FocusRingPolicy::{Never, KeyboardOnly, Always}` with
  `RenderRoot::set_focus_ring_policy` and `PaintCtx::is_focused_for_ring` for a keyboard-only focus
  ring. The baseline `Button`, `Checkbox`, `Radio`, `Slider` and `TextInput` are Tab stops; the
  first three activate on Space/Enter and `Slider` steps on the arrow keys and jumps on Home/End.
  shadcn's button draws its focus ring for keyboard focus only.
- Function keys: `NamedKey::F1`–`F24` (new variants; `NamedKey` is `#[non_exhaustive]` from this
  release, so a downstream exhaustive `match` needs a wildcard arm once — see Changed) with
  `NamedKey::function_number`/`NamedKey::function`; `Chord::parse`
  accepts `F1`–`F24` and `Chord::display` labels them (`F2`, `⌘F2`). The desktop and web shells
  deliver them; `F25` and up are still dropped, and `ChordParseError::UnsupportedKey` is no longer
  produced.
- `KeyEvent::is_activation` (a bare, non-repeat Space or Enter), and
  `RenderRoot::set_tab_traversal`/`tab_traversal`: off, a declined bare Tab or Shift+Tab stays
  unhandled for the host's own tab order.
- Facade: `Chord`, `FocusRingPolicy`, `OverlayKeys`, `ShortcutHint`, `ShortcutScope`, `shortcuts`
  and `ShortcutsView` at the top level, and the full shortcut vocabulary under `frust::authoring`.
- The playground's Shortcuts section: a Mod+K (or `?`) command palette listing the live hints, a
  Mod+B pane, an F2 chord, 1/2/3 tab chords, a row of focusable controls, a readout naming each
  key's consumer, and a precedence-matrix test suite.

### Fixed

- A desktop wheel over a nested `ListView` or `ScrollView` reaches the innermost scroll surface
  under the pointer instead of scrolling the enclosing page: every scroll surface now offers the
  unmodified `Scroll` to its content first (`ScrollSurface::route_wheel`), a notch with no travel on
  the surface's axis or at its pushed edge answers `Ignored` and passes outward, and `ScrollView`,
  `ListView` and the sliver viewport share that entry (docs/WIDGETS_ARCHITECTURE.md § scrolling).
- Tab traversal keeps focus when no grant lands: the session, `focus_epoch` and the keyboard-focus
  flag stay untouched and Tab is reported unhandled. A focusable root widget is now a stop.
- The keyboard-focus flag behind `FocusRingPolicy::KeyboardOnly` clears on any claim honoured
  outside traversal and on any session release, not only on a pointer press.
- A hidden child no longer answers shortcuts: `AnimatedOpacity` settled at opacity 0 forwards no
  `Shortcut` or `Key` (the navigator, `IndexedStack`, pattern switcher, `Stack` and `Either` were
  checked and already forwarded only to what they paint).
- A pointer press that moves focus from a text field to a control publishing no IME state clears
  the IME surface at once, so the soft keyboard goes down when a button is tapped.

## 0.6.2 — 2026-10-16

A patch release (the 0.6.x line continues) that carries source changes for code that implements
`PaintScene`, builds or matches `frust_scene::Command`, or constructs `frust-gpu` pipeline descriptors
directly (the Changed list marks them), and two behaviour changes every widget author should read:
layout skipping is on by default and `pattern_switcher` tears its outgoing child down at the switch.

The render model gained the pieces a mobile-first framework needs to keep frames cheap and to mix
native content with its own paint. Layout now skips pods whose constraints are unchanged and can
audit itself (`FRUST_LAYOUT_AUDIT`); slivers (`custom_scroll_view`, `sliver_list`,
`sliver_persistent_header`) give scrolling a lazy, composable shape; retained layers
(`repaint_boundary`, `AnimatedOpacity::cached`) keep rasterized pages across frames, with path
clips and backdrop blur as engine primitives. Partial repaint was measured and rejected, and was
removed. Platform views can sit between scene slots through an overlay host on Android, iOS and
macOS (`FRUST_OVERLAYS`, default off), and video and camera frames can enter the scene as textures
through `frust::texture` and `texture_view`, with zero-copy import on Apple and, behind
`FRUST_ANDROID_HWBUFFER_IMPORT` (default off), on Android. Every device leg has run: iPhone SE, iOS
Simulator and macOS for the Apple half, a OnePlus 9 for the Android overlays, the hardware-buffer import and the
Phase 3 retained-layer numbers. Gates passed: workspace tests, clippy, the five engine golden arms byte-unchanged, and both
Android app Kotlin compiles. How the model maps onto Flutter's is in
[docs/learning/17-render-model-vs-flutter.md](docs/learning/17-render-model-vs-flutter.md); the
limits are in [docs/LIMITATIONS.md](docs/LIMITATIONS.md).

### Added

- Backdrop blur: `frust_scene::Command::BackdropBlur`, `SceneBuilder::backdrop_blur` and
  `PaintScene::backdrop_blur(origin, size, radius)` (a no-op by default) blur whatever is already
  painted beneath a rectangle, masked by any open clip. The engine cuts the frame there, copies the
  region into a page, blurs it and draws the panel from it; at most four per frame
  (`EngineError::TooManyBackdrops`), none inside a translucent layer, never retained. A swapchain
  frame carrying one renders via an intermediate plus one present copy (`CopyPresentPass`).
- Path clips: `frust_scene::Command::PushClipPath`, `SceneBuilder::push_clip_path` and
  `PaintScene::push_clip_path(origin, path)` clip to an arbitrary outline (nonzero winding), popped
  by the shared `pop_clip`. The engine lowers it to a coverage mask, never a layer page, so a
  retained body under one paints inline; a recorder without its own override clips to the path's
  bounding box.
- Retained layers. `repaint_boundary(child)` (with `.alpha(f32)`) and `AnimatedOpacity::cached(bool)`
  keep a subtree's rasterized page across frames and composite it while nothing under it changes;
  navigator pages and `pattern_switcher` children carry a boundary, so transitions composite
  retained pages while a page at rest paints plain. The engine owns the pages in a budgeted LRU store
  outside the intermediate pool (`FRUST_ENGINE_RETAINED_BUDGET_MB=<n>` overrides the tier budget); a
  page it cannot serve refuses the frame and the shell repaints without pages. Kill switches:
  `FRUST_NO_RETAINED_LAYERS` (widgets) and `FRUST_ENGINE_NO_LAYERS` (engine, now consulted).
  A page is drawn only when the published ledger covers the paint serial it was recorded under;
  re-recording a key whose new recording does not reach its page (refused, nested or unrendered)
  evicts the old page; a hover-link change or focus-epoch advance re-records every boundary once.
  `PaintScene::current_transform()` (default `None`) reports the transform a command would record
  under. Limits are in docs/LIMITATIONS.md (`retained-pages-budget-refusals`).
- Per-paint-class strip programs: `strip.wgsl` is specialised per `PaintClass` (solid + coverage,
  gradient, image, image-filtered, blurred-rect, layer-composite, and the whole Uber program), the
  variants created lazily against one explicit strip pipeline layout. `FRUST_ENGINE_UBER_PROGRAM=1`
  draws every run with Uber again; `FRUST_ENGINE_RUN_SPLIT_MIN=<n>` (default 16) sets the shortest
  run worth a pipeline switch. `EngineRenderer::set_uber_program` / `set_run_split_min` are the
  programmatic counterparts.
- One-pass surface opening: the clear, the opaque strips and the first drawing surface round record
  as a single render pass. `FRUST_ENGINE_NO_PASS_MERGE=1` records them apart again;
  `EngineRenderer::set_pass_merge` is its counterpart and `last_frame_passes` lists a frame's
  `FramePass`es.
- Atlas tier rule: an adapter-family name table places a mobile GPU on the mobile budget whatever
  its capability flags say, and a mobile family with at least 4 GiB of `MemTotal` (Linux/Android)
  earns the new `mobile_large` tier, 2048x2048x4 (1024x1024x16 when the wide shape is refused).
  The tier log line gains `rule=` and `memory_mib=`. An eviction clear that a pending upload
  covers exactly is no longer issued.
- Hardware image sampler program family, on by default: bilinear atlas images are read with one
  filtered fetch instead of four `textureLoad` taps. `FRUST_ENGINE_IMAGE_SAMPLER=0` selects the
  manual family; `EngineRenderer::with_image_sampler` chooses per renderer.
- `frust-gpu`: `RenderPipelineDesc` gains an optional explicit `layout`, the sixth field of the
  pipeline key.
- `EventCtx::request_layout` / `needs_layout`, `ChildPod` layout-skip state,
  `RenderRoot::{set_layout_skip, layout_skip}` and `WidgetTree::inspect_layout_dirty`.
- Layout-audit mode: `RenderRoot::set_layout_audit` or `FRUST_LAYOUT_AUDIT` (any value but `0`)
  compares each skipped pass with a forced one and logs `layout-audit:` warnings;
  `layout_audit_mismatches()` counts them. Shells log `layout skip=.. audit=..` at startup.
- `frust-network-image` (`plugins/network-image`): a `network_image(url)` view with placeholder,
  error and fade-in slots, an `ImageFetcher` seam (default `ReqwestFetcher`: rustls on the ring
  provider with bundled `webpki-roots`), a byte-budgeted memory cache, an LRU disk cache under the
  app cache directory, single-flight fetches, and an opt-in `webp` feature. Install with
  `NetworkImages::install()` in the `app!` setup block. Its Android compile gate needs
  `cargo ndk` (ring compiles C).
- `frust-network-image`: `DecodeLimits::max_alloc_bytes` is now the budget for the header-predictable
  decode-and-fit working set (resize working set and a progressive JPEG's coefficient store,
  counted from its header, included; default 512 MiB); over-budget images fail with
  `DecodeError::OverBudget`.
- The TUI's Add Plugin dialog (`a` from either screen) offers the new plugin; the registry's 0.6.2
  count is twenty-one, reached with the `share` and `qr` entries below.
- The playground's Images section: six `network_image` views, a local-fixtures loopback toggle, a
  cache stats row and a Clear caches button.
- `IndexedStack` / `indexed_stack` in `frust-widgets`, re-exported by the facade: it keeps every
  child mounted, and only the selected child is painted, routed input and reachable for back, so
  per-tab navigator stacks route Android back to the selected tab.
- `frust::BackFallback` and `frust::back_fallback(&fallback, child)`: a non-navigator back
  registrant ranked after every navigator (for example back-to-home-tab at a tab root). It is live
  while its wrapper is mounted.
- The playground's Tabs section: per-tab navigators under an `IndexedStack` with a home-tab
  `back_fallback`, a device-gate surface for Android back routing.
- `frust-share` (`plugins/share`): `Share::text` / `text_anchored` / `url` open the Android chooser or
  the iOS activity sheet (an iPad popover anchored to a `ShareAnchor` rect, or centred); desktop
  copies to the clipboard through `frust-clipboard` (`ShareOutcome::CopiedToClipboard`). Device
  gates have not run yet.
- `frust-qr` (`plugins/qr`): a `qr_code(payload)` view painted on a canvas with `.ecc`,
  `.quiet_zone`, `.colors`, `.size` and `.semantic_label`, plus `encode` / `QrMatrix`. The TUI's
  Add Plugin dialog (`a` from either screen) offers `share` and `qr` (the registry now has
  twenty-one entries).
- `frust-url-launcher` Maps helpers: `Maps::directions` / `directions_to` / `show` (Apple Maps on
  iOS/macOS, Google Maps elsewhere) and `UrlLauncher::open_maps_native` for `geo:` / `maps:` /
  `comgooglemaps:` / `google.navigation:` URIs (`NoHandler` on desktop).
- `frust-secure-storage`: a debug-only, desktop-only plaintext `insecure_file` fallback for hosts
  without a Secret Service, selected by `StoreOptions::with_insecure_file_fallback()` or
  `FRUST_SECURE_STORAGE_INSECURE_FILE=1`; release builds refuse it.
- The playground's URL page gains Share, QR and Maps groups.
- Sliver scrolling: `custom_scroll_view` (`CustomScrollView`, `.axis(ScrollAxis)`, `.cache_extent`,
  `.controller`) composes slivers under a viewport with a cache window and correction-based anchoring,
  with `sliver_box`, `sliver_padding`, `sliver_fill_remaining`, `sliver_fixed_extent_list` / `_keyed`,
  `sliver_list`, `sliver_grid` / `_keyed` (`SliverGridDelegate`), `sliver_persistent_header`
  (pinned and floating) and `keep_alive` / `KeepAliveHandle`. Transparent wrappers (`Either`,
  `EitherOf3..8`, `hero`, `shield`, `auto_scroll_zone`, `GestureDetector`) forward slivers.
- The `SliverWidget` seam in `frust-core`: `Widget::as_sliver`, `ChildPod::layout_sliver_child`,
  `SliverConstraints` / `SliverGeometry`, re-exported by `frust::authoring` with `ScrollAxis`
  (also flat in the facade).
- Material `sliver_app_bar` works as a sliver inside a `custom_scroll_view` (pinned by default,
  `.floating(bool)` opt-in); a set `collapse` / `scroll_offset` prop keeps the box form. The
  playground gains a Slivers page.
- `ScrollSurfaceCore` and the `ScrollSurface` trait: one shared scroll core under `ScrollView`,
  `ListView` and the viewport.
- Native-view z-order and overlays: `Command::is_drawing`, `PlatformViewFrame::command_index`,
  `ViewCommand::Update.z_index` and the `ViewCommand::{Overlay, DisposeOverlay}` commands, emitted
  by `PlatformViewState::ingest_scene(frames, shields, commands, viewport)` (plain `ingest` never
  emits overlays). Z-shields are now input-only.
- Segmented rendering: `frust_render::overlay::OverlaySurface` and `SurfaceRenderer::encode_segmented`,
  `submit_segmented` and `submit_segmented_deferred` draw one compiled frame as a main segment plus
  one segment per cut with a single submit.
- Android overlay host behind `FRUST_OVERLAYS` (default off; frust paint above a native view on an
  opaque surface; `media` selects the `setZOrderMediaOverlay` arm). Two additive JSON ops,
  `overlay` and `disposeOverlay`, and a debuggable-build intent extra `dev.frust.translucentSurface`
  to force the surface mode for one launch. The native-widgets demo gains a `sandwich` page.
  Interleaved content covers later slots (`android-overlays-cover-later-slots`).
- Texture bus: `frust::texture` (`register_texture_source`, `NativeImporter`), the leaf
  `frust_plugin::texture::TextureSink` mailbox and the `texture_view` widget put plugin-produced
  frames in the scene as ordinary paint.
- Android hardware-buffer import behind `FRUST_ANDROID_HWBUFFER_IMPORT` (or
  `debug.frust.hwbuffer_import`; default off): `frust_gpu::import::android::import_hardware_buffer`
  and `frust_render::native_import::install`.
- `frust-video-player`: `VideoOutput::Texture` with `video_texture_view`, `texture_sink` and
  `texture_output`. `frust-camera`: `CameraSession::preview_texture` and `preview_texture_output`,
  and a default-on `frust-api` feature. Both keep the native view as the default; the playground
  gains Texture toggles.
- Apple overlay hosts behind `FRUST_OVERLAYS` (default off): iOS `FrustOverlayView` per cut (Mode A
  only; additive `frust_overlay_mode`/`frust_overlay_attach`/`frust_overlay_detach` exports) and
  macOS `FrustOverlaySurfaceView`, presenting main then overlays from one segmented submit. Each
  overlay sits directly above its own slot, so interleaved content stays under a later native
  control. Limits: `apple-overlay-limits`.
- Apple zero-copy import: `frust_gpu::import::apple::MetalImporter` wraps a 32BGRA IOSurface-backed
  `CVPixelBuffer` as a texture (NV12 stays on the CPU path), installed by
  `frust_render::native_import::install` on iOS and macOS.
- Apple texture producers: `VideoOutput::Texture` (an `AVPlayerItemVideoOutput`) and the camera's
  `preview_texture` (a second 32BGRA data output) hand frames on zero-copy when the importer is
  installed.

### Changed

- Android video/camera texture output logs the path frames take (AHardwareBuffer zero-copy or CPU
  copy) once per session at Info.
- `NativeHandle` is move-only (no `Clone`/`Copy`) and `FramePayload`/`SourcePayload` are no longer
  `Clone`, so a handle cannot be copied. `TextureFrame` and `SourceFrame` keep their payload and
  release callback private (`new`, `payload()`, `meta()`, `has_release()`), so outside `unsafe` a
  handle cannot leave the frame that carries its release; the `unsafe` `into_parts`
  (`FrameMeta`/`SourceMeta`) makes its caller run the returned callback exactly once. Release
  callbacks run under `catch_unwind` in panic=unwind builds (dev and test); the shipped release
  profile sets `panic = "abort"`, where a panicking callback aborts and callbacks must not panic.
- `NativeHandle` (in `frust-render` and `frust-plugin`) keeps its fields private: build one
  through the unsafe `NativeHandle::new(kind, ptr)` and read it with `kind()`/`ptr()`. Texture
  frames run their release callback on drop, so it runs exactly once on every path.
- BREAKING: `frust_scene::Command::PushSnapshot` / `PopSnapshot` are replaced by `PushRetained
  { key, rect, alpha, scale, transform }` / `PopRetained`, plus the new `Command::DrawRetained`;
  `PaintScene::push_snapshot` / `pop_snapshot` are removed in favour of `push_retained` /
  `pop_retained` / `draw_retained` (rename the calls). A renderer that does not retain paints a
  `PushRetained` body inline exactly as the old snapshot bracket did and is never sent
  `DrawRetained`; a renderer that publishes a ledger must serve every key it lists or refuse the
  frame. `PaintCtx::retained_available` takes a fourth `recorded_serial` parameter.
- `PaintOutcome::paint_serial` reports the monotonic serial `RenderRoot::paint` minted for the pass.
- Host paint cost with retained layers on versus off (release, i5-12600 Linux, 300 frames, mean
  us; `relayout_cost_bench`): cached opacity 0.11 vs 49.00, navigator push/pop 32.63 vs 111.52,
  navigator page at rest 48.53 vs 48.54. Device legs (Pixel 5, iPhone SE) are owed. A backdrop
  frame on the direct present arm pays one present copy, about 0.239 ms at 1080x2400 and 0.189 ms
  at 1920x1080 (T400).
- Measured `gpu_main` p50 on the S1 scene, before and after the FAST Phase 1 engine work: Pixel 4a
  32.70 to 12.75 ms, Pixel 4 11.15 to 8.12 ms. Method and the other scenes are in
  [docs/PERFORMANCE_BASELINES.md](docs/PERFORMANCE_BASELINES.md) § Engine close-out per device.
- Layout is skipped per pod by default: a pod laid out before returns its recorded size unless it
  is dirty, its constraints, the window insets or the root layout epoch changed (theme, insets,
  window size, a dirty floated overlay). Widgets must signal layout-affecting changes through
  `LAYOUT` from rebuild, paint `request_layout`/`request_frame`, or a consumed event. Unoptimized
  bench (`relayout_cost_bench`), skip off to on, mean layout us: static tree 76.3 to 0.52,
  scroll-only 75.2 to 1.46, one changed leaf 160.1 to 93.6 (one-leaf vs no-change ratio 2.10x to
  179.9x); paint unchanged. Set `FRUST_NO_LAYOUT_SKIP` to opt out.
- `ScrollView`'s rebuild reports `LAYOUT` while a `ScrollController` command is queued, as
  `ListView` already did.
- `pattern_switcher` tears its outgoing child down when the switch starts, not when the exit
  animation ends; the outgoing child still paints its exit. A child that owns resources (a native
  widget, a subscription, a timer) releases them at the switch.
- `NavigatorController::back_interest()` is false for a navigator that is not mounted.
- A back press that no registrant claims at delivery is dropped instead of being handed to the
  navigator whose wire consumed it.
- Codemod: the accumulator `// erasure: keep` marker now also covers an untyped `let`, one seeded
  with a direct (unwrapped) `any(..)` element (`let mut body = vec![any(..)]`) or started as
  exactly `Vec::new()`, `Vec::with_capacity(..)` or `vec![]` and later pushed or inserted a direct
  `any(..)` in the same fn, so idiomatic accumulators no longer need a `: Vec<AnyView<_>>`
  annotation for rule U.
- The Android overlay spike's JNI symbols and single-overlay path are removed; the overlay host
  uses `nativeOverlayMode`, `nativeOverlaySurfaceChanged` and `nativeOverlaySurfaceDestroyed`.

### Fixed

- Android overlay host: a new, resized or withdrawn overlay surface requests a frame, so an idle app
  no longer keeps frust content under a native view until an unrelated repaint.
- Overlay cuts: content the overlay engine cannot draw (retained pages, external-texture frames,
  backdrop blurs) stays on the main surface under the native view instead of vanishing; plain
  hosting emits no rank-only updates; a cut no longer swallows a slot's hole-punch.
- `frust-shell-ios`: the AccessKit tree is published in the view's physical pixels through a root
  transform equal to the display scale, so VoiceOver frames and hit-tests land on the drawn
  elements on 2x/3x devices (they were half-scale before).
- A navigator inside a `pattern_switcher` child that had been switched away kept claiming Android
  back, so back was swallowed on every other page and could not exit the app.
- `ScrollController::animate_to` now flushes `on_scroll` (and `ListView` near-edge callbacks) on
  every frame the tween moves and the frame it lands on, with no input event needed.
- A `scroll_to_item` request that moves nothing (unknown key, or `Nearest` on a fully visible row)
  now supersedes an in-flight controller tween instead of leaving it running.
- `VideoFit::Cover` no longer spills outside its box on Android in texture mode, where the frames
  are scene content and crop inside the box, and the native player view now clips on Apple too.
- iOS: with present-sync armed, the 6 wgpu validation errors per launch are gone, and the
  present-sync black screen on a phone and the Simulator Mode B black screen no longer reproduce.
- The playground's camera texture preview no longer freezes when its frame requester is culled
  below the fold; the ticker now sits under the picture (`texture-bus-no-wake-on-push`).

## 0.6.1 — 2026-10-12

A patch release that also carries two narrow source changes for code that calls the authoring child
helpers directly (an explicit `build_child::<State>` turbofish, a `&&AnyView` argument) and the
tooling-crate API changes listed under Changed.

### Added

- `EitherOf3` .. `EitherOf8` (variants `A`..`H`) in `frust-widgets`, re-exported by the facade and
  the prelude: a `match` with three to eight arms returns one typed view instead of erasing each
  arm, with `Either`'s swap semantics (a change of arm is a teardown and remount) (3a75095f).
- `BuildCtx::mark_element_replaced` and `BuildCtx::track_element_replaced` in `frust-core`: a view
  whose `rebuild` replaced its element's widget while keeping the element type (an enum view
  switching arms) reports it, and the nearest enclosing reconciler's tracking scope treats the
  rebuild like a concrete-type swap.

### Fixed

- An `Either` / `EitherOf3`..`EitherOf8` arm swap now drops the enclosing child pod's pointer
  capture and focus link (a single-child wrapper, a keyed list, or a component whose body is the
  enum) and, in a positional list, cancels the recorded capture and focus of every later child,
  matching an `AnyView` concrete-type swap; before, a swap mid-gesture kept forwarding `Move`/`Up`
  to the fresh arm and starved its siblings until `Up`.
- The CLI resolves the app's `frust-ui` dependency key, so both `frust-ui = ".."` and
  `frust = { package = "frust-ui", .. }` work for `run`, `build` and hot sessions: every cargo
  argument list, Android `-Pfrust.cargoFeatures`, iOS `FRUST_FEATURES` and hot-patch fat build
  spells features with the app's key, a missing-feature cargo failure on a debug or profile build
  carries a one-line hint naming both forms, and release lanes refuse devtools through either key
  (c801185c, 5fb139c6, fcbb1e91).
- `frust build`'s release lanes also refuse cargo's weak `<key>?/devtools` spelling and the
  `hotpatch` features (`hotpatch`, `<key>/hotpatch`, `<key>?/hotpatch`), which compile the devtools
  listener in through the shells; when cargo cannot report the dependency key, or the app keys the
  facade under a name no feature can spell, the app-alias check fails closed and counts any
  `<dep>/devtools` or `<dep>/hotpatch`, and `<app-package>/<alias>` is checked like `<alias>`
  (601840eb).
- Codemod (`scripts/codemod/frust_any_codemod.py`): the name-list callees and the
  `column`/`row`/`stack` builders resolve by import origin (a strict `frust*` path head), so an
  app fn or an app-imported builder that shares a listed name is skipped with a note, as is a
  `Type::name` callee whose `Type` can only come from a non-frust glob import (412dc49a, 998bc8cf);
  non-private T7 helpers land in a T7-visible bucket and are rewritten only with `--t7-visible`, a
  `return` inside a closure is not an early return, a first-body-line keep marker that rustfmt
  moved is fn-scoped for T7 and rule U (40b6f35f), and a helper whose same-file call is taken by
  `&` is excluded as "taken by reference" (46be57ae).
- Codemod T7 emits a type-only `+ use<..>` whenever the helper (or its impl) has a lifetime
  parameter or an elided reference parameter, in any edition, so a stored borrow of a temporary no
  longer fails E0716; an argument-position `impl Trait` still skips with a note. T7-ref counts a
  use in another file only when it is path-qualified to the helper's module or imported from it,
  `let x = <expr>; any(x)` tails collapse to `<expr>` (no `clippy::let_and_return`) in edition-2024
  crates while an earlier edition skips that helper with a note (tail-expression temporaries
  outlive locals there), and "taken by reference" also covers a same-file call bound by an untyped
  `let` and later borrowed as `&x` or `&mut x` in the same block (bd02b8b7, f3e36a12, 998bc8cf).
- Codemod: a local fn shadows a builder only in its own module and descendants, so a `fn row` in a
  nested `#[cfg(test)] mod` no longer blocks T2/T3 in the parent; a reasoned `// erasure: keep`
  marker on an accumulator `let` of `AnyView`s covers that vec's `push`/`insert`/`extend` calls in
  the same fn, and a reasoned own-line marker inside an erasure call's parentheses (the form rustfmt
  produces for a multi-line `any(..)`) covers that call for rule U and the T1-T7 keep exemption
  (1b015a7b).

### Changed

- **API (tooling crates):** `frust-drive`'s
  `desktop_run::desktop_plan` and `desktop_plan_with_features` return
  `Result<DesktopPlan, FacadeKeyError>` instead of falling back to the `frust/` feature spelling,
  and `try_desktop_plan_with_features` is gone; `frust-drive`'s `DesktopBuildError` and
  `frust-tui`'s `LaunchError` gain a `FacadeKey` variant (neither enum is `#[non_exhaustive]`), so
  an unresolvable facade key is reported as itself rather than as a cargo spawn or feature failure
  (d9589c77).
- Migration guidance: use `Either` or `either(..)` only when the two arms have different concrete
  view types. When both arms produce the same type, build it conditionally (compute the differing
  props or children in an `if`, construct once): `Either` reconciles a Left/Right swap as a
  teardown and remount, which drops in-flight gestures and can orphan focus
  ([docs/WIDGETS_CODE_STANDARDS.md](docs/WIDGETS_CODE_STANDARDS.md)).
- `frust::authoring::build_child`, `rebuild_child` and `teardown_child` take any `V: View<State>`,
  so a hand-written view can store `child: V` (single-boxed); `AnyView` children behave as before
  (double-boxed, concrete-type swap detection kept). Two source changes for callers: (a) an
  explicit turbofish `build_child::<State>(..)` becomes `build_child::<State, _>(..)`; (b) a caller
  that passed a `&&AnyView<State>` (for example `&child` where `child: &AnyView<State>` is a
  loop or closure binding) and relied on deref coercion now fails with E0277 (`V` is inferred as
  `&AnyView`) and must pass `child` or `*child` instead (ebc5b245).
- Codemod: a borrow passed to the `build_child` trio no longer excludes a helper from T7, and
  T7-arms lists the arm count and suggests `EitherOf<n>` (b4baeec0).
- Frust's plugins and examples no longer keep `any()` for `ChildPod`-fed helpers or three-to-eight
  arm bodies; the kept cases are overlay-slot tooltip helpers, `Vec<AnyView>` entries,
  runtime-varying content, and page tables of nine or more arms (1a383163, 3281d802, 3d94eef1,
  fc4b5f39, 57d0e6fc, 7abdd14e).

## 0.6.0 — 2026-10-11

Type erasure moved from the call site to the API boundary: public builders take generic views and
erase internally, and views are composed with fluent builders, child sequences (`ViewSeq<State, M>`)
and `Either<L, R>` instead of `any()` everywhere; helpers return `impl View<S>` instead of
`AnyView<S>` wherever the result is not stored erased. Debug builds hot-patch a saved edit into the
running app on the desktop, an Android device and the iOS simulator, keeping the process and every
component's State.

### Breaking

- `Component::build` returns `impl View<Self::State>` instead of `AnyView<Self::State>`. An impl
  that keeps `-> AnyView<..>` warns (`refining_impl_trait`) and fails under `-D warnings`.
- `Component` is no longer dyn-compatible: `dyn Component` and `Box<dyn Component>` do not
  compile. Use generics, or store the `AnyView`s a component produces.
- Driver closures must erase the build result with `AnyView::new(root.build(state))`; the opaque
  return captures the `&self` and `&mut State` borrows and cannot escape the closure otherwise.
  `frust::run`, `frust::app!` and the templates already do this.
- Public parameters that were typed `AnyView` are generic (`impl View<S>`, `Option<V>`,
  `impl IntoIterator<Item = impl View<S>>`, `impl Fn(..) -> V`, `ViewSeq<S, M>`), with one exception
  that keeps erasure because it lazily produces views: the deprecated `frust_material::list_view`
  item builder (`impl Fn(usize) -> AnyView<State>`; `frust::list_view` is its generic replacement).
  The material button-group icon family (`button_group_icon_action`, `ButtonGroupAction::icon` and
  `checked_icon`, `ButtonGroupView::overflow_icon`), `SplitButtonView::leading_icon`, beui's
  `ChatConversationView::extra_rows` and clean-signals-frust's `async_view` take generic views too.
  Callers passing `any(..)` (or a closure returning it) still compile, but a bare `None`, an empty
  `vec![]`, an explicit turbofish call, or a `.collect()` straight into a list parameter now needs a
  type: a turbofish caller of a plugin list function writes `::<State, _>`, and one of
  `button_group_icon_action` needs one more `_`.
- `InputEvent` gained a `FileDrop(FileDropEvent)` variant (with the new `FileDropPhase`).
  `InputEvent` is not `#[non_exhaustive]`, so an exhaustive `match` over it needs a `FileDrop(_)`
  arm (E0004) (`crates/frust-core/src/event.rs`). The same release adds `OverlayAnchor::Window`
  (`crates/frust-widgets/src/overlay.rs`), which an exhaustive `match` over `OverlayAnchor` must
  also handle.
- Public exhaustive-enum changes in tooling crates (none is `#[non_exhaustive]`, so an exhaustive
  `match` outside the crate breaks): `frust-devtools-protocol` `Capability` gains `HotPatch` and
  `Method` gains `HotpatchInfo`, `PatchChunk`, `TableChunk` and `ApplyPatch`; `frust-tui`'s
  engine `Message` (+`HotPatchNow`, `HotPatchOutcome`, `HotSessionStarted`,
  `RunConfigToggleAutoApply`, `RunConfigToggleRunHot`; -`RunConfigToggleWatch`), `RegionId`
  (+`RunAutoApplyRow`, `RunHotRow`; -`RunWatchRow`), `RunFocus` (+`AutoApply`, `RunHot`;
  -`Watch`), `Effect` (+`HotPatch`, `HotPatchNow`, `LaunchHotSessions`;
  -`LaunchWatchedSessions`) and `SessionTarget` (+`Simulator`). `frust-shell-desktop`'s
  `ShellUserEvent` (doc-hidden) became `#[non_exhaustive]` and gained `HotPatched` (`hotpatch`
  feature only). Files: `crates/frust-devtools-protocol/src/{messages,method}.rs`,
  `crates/frust-tui/src/engine/{message,update,run_config,session_view}.rs`,
  `crates/frust-shell-desktop/src/app_handler.rs` (`git diff v0.5.2 v0.6.0 -- crates`).

### Known limitations, now documented

These behaviours already held before 0.6.0; this release documents them in `docs/LIMITATIONS.md`
rather than introducing them.

- Wrapper views with `type Element = Box<dyn Widget>` that forward to an inner `AnyView` hide
  inner type swaps from focus bookkeeping (`focus-wrapper-erasure-swap-blind`, the renamed
  `focus-double-erasure-swap-blind` entry that 0.5.1 already carried).
- Keyed flex lists are all-or-nothing, and the check is a `debug_assert` (present since 0.5.1,
  `crates/frust-widgets/src/authoring.rs:598` at v0.5.1): a mixed list silently falls back to
  positional reconciliation in release builds (`keyed-list-all-or-nothing-debug-only`).

### Added

- Hot patching (debug builds): `frust run --watch` (the desktop preview),
  `frust run -d <Android device> --watch` and `frust run -d <booted iOS simulator> --watch` patch
  each settled save into the running app. The process, its PID and every component's State are
  kept; the CLI prints `patched in N ms` and relaunches only on `restart required: <reason>`; a
  compile error leaves the app untouched; `--no-hot` keeps the kill-and-relaunch loop. It works on
  a stock `frust create` app.
- Hot-patch targets: macOS, Linux and Windows x64 (MSVC) desktop, Android arm64-v8a and the arm64
  iOS simulator. Not supported: a physical iOS device, the web, Windows on ARM64 and other Android
  ABIs.
- Hot patching in the TUI: debug desktop, Android-device and iOS-simulator sessions run hot by
  default; `r` hot-patches now, `W` toggles the watcher, `R` is a full rebuild and relaunch, and
  `o` opens the run dialog with 'Run hot' and 'Auto-apply on save' (both on by default).
- Hot-patch restart fallbacks, each printed as `restart required: <reason>` and answered with a
  fresh build and launch: a replayed type's layout change, a component State type change, a
  manifest, build script, cargo config, toolchain or `frust.toml` edit, a path-dependency edit on
  Android or the iOS simulator, a proc macro or an uncaptured package, the patch budget (64
  patches or 96 MiB per process by default, set by `[hotpatch]` in `frust.toml`) and a lost
  apply reply. See `no-hot-reload-restart-is-a-rebuild` in `docs/LIMITATIONS.md`.
- New crate `frust-hotpatch`, the in-app hot-patch runtime (a port of Dioxus subsecond 0.7.10),
  and the opt-in `frust/hotpatch` feature, which a hot run switches on for its own build only.
- `ScrollController` for `ScrollView` and `ListView`: programmatic `jump_to`, physics-driven
  `animate_to`, `scroll_to_item` with settle-and-correct, offset publishing and `on_change`; a
  command ends a live drag. shadcn's `scroll_area` thumb can be dragged.
- Drag and drop: a `DragCoordinator`, `draggable` sources and `drag_target` drop targets with
  keyboard and accessibility parity, auto-scroll zones, `reorderable_list`, and desktop file drop
  (`InputEvent::FileDrop` with `FileDropEvent`/`FileDropPhase`).
- `frust::ViewSeq<State, M>` — the child-sequence trait every list parameter now takes (`M` is an
  inference marker callers never name): implemented for a single view, tuples of 1–12 views (nest
  tuples beyond 12), `Vec<V>`, arrays `[V; N]`, `Option<V>` (`None` drops its slot, like
  `.when_some()`), and iterators wrapped in `views(iter)`; erasure happens once, inside the
  container.
- `frust::Either<L, R>` — two-arm typed view composition with `either(cond, || l, || r)` factory.
  Swapping an arm rebuilds; both arms stay typed.
- `frust::views(iter)` — wraps an iterator as a child sequence. A bare `.collect()` passed to a
  sequence API needs `.collect::<Vec<_>>()`.
- Generic view parameters for the icon and row APIs that still took erased closures:
  `button_group_icon_action`, `ButtonGroupAction::icon`/`checked_icon`,
  `ButtonGroupView::overflow_icon` and `SplitButtonView::leading_icon` take `Fn() -> V` with
  `V: View<State>`; beui's `ChatConversationView::extra_rows` takes `Fn() -> Seq` with
  `Seq: ViewSeq<State, M>`; clean-signals-frust's `async_view` takes loading, data and error
  closures returning any view and erases once inside.
- `AnyView::new` and `any()` are idempotent: erasing an `AnyView` returns it unchanged.
- Fluent builders: `column()` and `row()` with `child`, `flex`, `keyed`, `children`, `push`,
  `when` and `when_some`; `stack()` with `child`, `children`, `when` and `when_some`.
- `scripts/codemod/frust_any_codemod.py` migrates an app to the new idiom, including the opt-in
  rule `--t5` that drops `any()` from homogeneous `vec![..]` list arguments, and rules T6 and T7
  (opt-in flags, enforced by the tripwire): T6 rewrites `vec![..]` arguments of sequence APIs to
  tuples when all elements are erasure calls (1–12 of them); T7 converts helpers ending in one
  erasure call to return `impl View<S>` instead of `AnyView<S>`.
- `scripts/ci/erasure-check.sh` guards the repository against redundant erasure (rules T1–T7 in
  strict mode) and runs as a CI hygiene step; a site that must keep `any()` carries a
  `// erasure: keep <why>` comment the tool honours.
- `docs/PERFORMANCE_BASELINES.md` collects the size and build-time measurements.

### Changed

- `frust create`'s default template is a single `src/lib.rs` holding the whole counter app: one
  `Component` whose `build` is one declarative widget tree, helpers as associated functions called
  `Self::name()`, and `frust::app!` default-constructing the root; `src/home_page.rs` is gone. The
  clean-signals template's pages pass generic `async_view` closures with no `any()`.
- `Column`, `Row`, `Stack` fluent builders and all layout/plugin list parameters (`frust-material`,
  `frust-beui`, `frust-shadcn`, `frust-glyph`) accept any child sequence; `Vec<_>` callers compile
  unchanged; iterator callers wrap in `views(..)`.
- Helpers across the examples, benchmarks and the app template return `-> impl View<S>` where the
  result is not stored erased (`-> AnyView` signatures 1,159 → 592 tree-wide in the first pass);
  the sites that must stay erased — stored view tables and page registries, accumulators into
  `Vec<AnyView>`, three-plus-arm bodies, helpers feeding a hand-written view's `ChildPod` — keep
  `AnyView` and carry `// erasure: keep <why>`.
- The example apps, benchmarks and templates carry no redundant erasure: every remaining `any()`
  in `examples/` and `benchmarks/` is marked `// erasure: keep <why>`, and the tripwire enforces it
  (rule U, the codemod's `--unmarked` check).
- Templates, README and rustdoc present erasure at the API boundary and the fluent builders;
  rustdoc examples and the crates.io READMEs are on the 0.6 idiom with 0.6 version pins.
- frust-mcp's `restart_app` description no longer claims Frust has no hot reload.
- frust-engine bounds flattening cost for every finite input: huge viewport-straddling curves are
  split at the viewport before flattening (the pre-flatten viewport split).

### Fixed

- frust-mcp: rmcp 1.8 → 2.2, fixing the Streamable-HTTP session leak.

### Measured

- Hot patch, save to patched frame against a full restart (medians): macOS desktop 1,397 ms vs
  5,218 ms (1b1ef31d); Pixel 5, stock app, 2,883 ms vs 13,458 ms (4c7e25b3); Windows x64 1,468 ms
  vs 10,861 ms (90113508); arm64 iOS simulator 1,694 ms vs 4,395 ms (6b82685e). material3-demo on
  a Pixel 5 patches in 3.5 s (median over four body-only edits), down from 58-72 s (80663b22).
- Cost of erasure at the API boundary (medians of 3 on one host, main 79686b68 → f79b739c; details
  in `docs/PERFORMANCE_BASELINES.md`): stripped arm64 `.so` huddle +0.12 %, material3-demo
  +0.34 %; clean release rebuild frust-gallery +1.6 % (an earlier run at the pull-request head
  measured +3.4 %), material3-demo −2.9 %. Build-time deltas of this size sit inside the host's
  run-to-run noise; the size figures are exact.
- Cost of child sequences and helper return types (i5-12600 Linux, main 721ab1e0 →
  feature/erasure-free-apps b2a41334; see `docs/PERFORMANCE_BASELINES.md`): stripped arm64 `.so`
  (material3-demo) 14,643,024 B → 14,658,576 B (+0.11 %; bar ≤ 2 %); clean release rebuild
  frust-gallery 14.42 s → 13.74 s (−4.7 %; bar ≤ 10 %), material3demo 129.02 s → 125.77 s
  (−2.5 %; bar ≤ 10 %).

### Migration

1. Change every `impl Component` whose `build` returns `AnyView<..>` to `-> impl View<Self::State>`;
   the body may keep `any(..)` for now.
2. Where the app writes its own driver closure, erase the build result there:
   `move |s| AnyView::new(root.build(s))` (`frust::run`, `frust::app!` and the templates already
   do).
3. Run `python3 -I scripts/codemod/frust_any_codemod.py --write --t5 --t6 --t7 <src dirs>`, then
   `cargo fmt` and `cargo check`.
4. Fix what the compiler reports: give a type to any bare `None`, empty `vec![]`, turbofish call or
   `.collect()` it rejects (a `.collect()` passed to a sequence API gets `::<Vec<_>>()`, a turbofish
   caller of a plugin list function writes `::<State, _>`, one of `button_group_icon_action` adds a
   `_`); a caller whose result now lands in a `Vec<AnyView>` or a `match` arm is re-erased with
   `any(..)` (or its list becomes a tuple); two-arm bodies use `either(cond, || a, || b)`, except a
   body whose two arms both consume one moved value, which uses `Either::Left`/`Either::Right` (or a
   `match`) instead of `either(..)`. Only a site that must stay erased (a stored view table, an
   accumulator into `Vec<AnyView>`, a three-plus-arm body, a `ChildPod`-fed helper, a same-head list
   whose elements differ in type) keeps `any()`. Re-run the codemod after each fix until
   `--check --t6 --t7` is quiet.
5. Replace `dyn Component` with generics or stored `AnyView`s; see `docs/LIMITATIONS.md` for the
   two behaviours listed under Known limitations, now documented.
6. Run `scripts/ci/erasure-check.sh`; mark required erasure with `// erasure: keep <why>`.
7. A `// erasure: keep` marker needs a non-empty reason; a bare marker covers nothing under rule U.
   Accepted placements (`docs/WIDGETS_CODE_STANDARDS.md`): on the call's own line, alone on the
   line above it, on the first line of an enclosing `vec![..]`, `let` or `match`, or on the
   enclosing fn signature (the `fn` line, its `->` line, or alone above the `fn`). rustfmt-stable
   forms in 0.6.0: at the end of the `->` line with the `{` on the next line, or at the end of the
   `any(..)` line. Newer placements (a first body line, an accumulator `let`, an in-call own-line
   marker) are listed under Unreleased.
8. An app scaffolded by 0.5.0 (yanked) also follows the 0.5.0 machine-path migration (see
   `scaffold-0.5.0-path-dependency` in `docs/LIMITATIONS.md`).

## 0.5.2 — 2026-10-05

Template, top-app-bar and wording release (`git log v0.5.1..v0.5.2`, 44 commits).

### Changed

- `Image` layout keeps its aspect ratio when the natural size exceeds the bounds (4f42b8bf, merged
  by 3d2287d4).
- The Material top app bar follows the M3 spec: a bare title starts 16dp from the leading edge,
  `AppBarView` gains `container(AppBarContainer)`, `elevation(u8)` and `safe_area(bool)`, and the
  fixed and search bars consume the top and side window insets themselves; a bar that is not at
  the window top opts out with `.safe_area(false)`. `selection_app_bar`'s contextual branch insets
  the same way (903184fa, 95c92de9, 0dce814d, f12c7da9). The examples that wrapped bars in
  `safe_area` were audited to match (2f61d997, 6d88f1e5, 2b8ee475).
- `frust create`'s default template is a Material counter starter built from components, and the
  clean-signals template is a Material counter feature; both use `InversePrimary` with elevation 2
  and self-inset (806c1814, c4d4e5da, c480f7ec). Generated READMEs describe each architecture's
  own layout and their font/theme snippets match the API (f70a92e9, 42a4d372), and the CLI help,
  wizard and scaffold tests no longer carry notes-app wording (d8e725f3).
- The app root closure is named `build` (was `app_logic`) across the shells, `frust-core`, the
  testing harness and the facade docs, which teach all UI in `Component::build` (d9108ee5,
  94689de4, 5346af2a, f2637e9c). The only public effect is renamed type and value parameters:
  `App<State, Logic>` is `App<State, Build>`, and `new_boxed_app`, `new_boxed_app_with`,
  `run_desktop` and `run_desktop_with` name their closure parameter `build`. No public item was
  added or removed, and type-parameter names are not nameable by callers.
- User-visible text no longer describes the framework as pre-release: the generated Gradle
  settings, `.gitignore`, the auth-session runtime error and README, and the TUI wizard no longer
  name a `frust plugin add` command (there never was one; the TUI's Add Plugin action does this),
  and the plugin READMEs and Gradle module headers show the generated
  `frustLocalDir("frust.plugin.<module>.dir")` form read from the gitignored `local.properties`
  instead of `file("<frust checkout>/plugins/...")` includes (d350ec1b, PR #15 / 383444e6).

### Release

- Version bump to 0.5.2, the first release published by `release.yml` through Trusted
  Publishing; the publish step retries a minute apart while crates.io's update rate limit
  applies (a28b2563, PR #16 / 4829391d).
