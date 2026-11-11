# Changelog

All notable changes to Frust are recorded here, newest first, in the
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) shape. GitHub release notes are copied
from the section for the tag (see [docs/RELEASING.md](docs/RELEASING.md)).

## Unreleased

### Breaking

- `accesskit::Tree` (re-exported through frust) is now `TreeInfo`; the old name is a deprecated alias.
- `frust-i18n` typed keys (`locales!`'s `keys` module): a message with two or more `$variable`s now
  takes one generated `<PascalKey>Args` struct with a named field per variable, instead of
  positional arguments, so two values of the same type cannot be swapped at a call site. A message
  with zero or one variable is unchanged. Migration: for `editor-notice = Working: { $working },
  full: { $full }`, replace `keys::editor_notice(&resolver, 50, 60)` (variables in name order,
  `full` then `working`) with
  `keys::editor_notice(&resolver, keys::EditorNoticeArgs { working: 60, full: 50 })`.
- An event handler now runs under its nearest enclosing component's owner (it reads that component's
  context and its ancestors'). Owner-scoped work armed in a handler (a signal created there, `Timer`,
  `on_cleanup`, `spawn_local_scoped`) is disposed with the component, not on rebuild, so a signal kept
  somewhere that outlives the component is disposed with it. Migration: arm a timer or scoped future
  meant to outlive a page under a longer-lived owner, one the app captured higher up (for example
  `Owner::current()` in a parent component's `init`) and enters with `owner.with(..)`. Likewise
  `provide_context` called from a handler inside a component now lands on that component's owner
  (visible to that component's subtree only, gone when it is disposed), where before it landed on the
  app's root owner; to provide app-wide from a handler, enter a longer-lived owner first.

### Added

- `frust::Owner`: the reactive owner, re-exported so an app can hold a child owner
  (`Owner::current()` then `.child()`) and arm timers under it with `owner.with(..)`. Keep ONE child
  owner in state and call `cleanup()` on it before re-arming under it, because each new child adds an
  entry to its parent that stays until the parent is cleaned up. See the debounce example on
  `Timer::new`.
- `FolderGrant::entry(relative_path)` in `frust-file-dialogs`: reopen one file of a granted folder by
  its stored relative path without listing the folder. It returns the entry, or `None` for a missing
  file or a directory. On Android each path segment costs one directory listing, so keep the entry
  from a listing when reading many files.
- `ListView::item_extent_of(|index| px)` and `extent_generation(u64)`: declare every row's extent up
  front, for a list whose row heights the app already knows. The content extent, every row's position
  and a `jump_to` to any offset are exact from the first frame. The list keeps a cached prefix sum
  (O(1) total, O(log N) offset-to-row lookup), rebuilt only when the item count or the generation
  changes, so bump the generation when extents change under an unchanged count. It is exclusive with
  `estimated_item_extent`; the builders' `item_extent` argument is unchanged and is overridden. A keyed
  exact list keeps its anchor row in place across count and generation changes above the viewport.
- `ListView::on_visible_range(|state, first, last| ..)`: the inclusive first and last indexes of the
  rows intersecting the viewport, with the cache buffer excluded, in all three extent modes. It runs
  one event late like `on_near_start`/`on_near_end`, and only when the range changes.
- `SliverGridDelegate::offset_of_index(index, cross_extent)`: a grid item's row offset in the grid's
  own coordinates. Add the extent of the slivers above the grid and pass the sum to `jump_to` or
  `animate_to`. `scroll_to_item` is documented as honoured only by a keyed `ListView`; a
  `CustomScrollView` drops item commands.
- The playground's Scroll page gains a 100,000-row exact-extent list with jump-to-row and a
  visible-range readout.
- `frust-photo-library` `decode` feature: `PhotoLibrary::decode_file(path, &DecodeOptions)` and
  `decode_asset(&AssetId, &DecodeOptions, &ExportOptions)` (with `library`) decode an image at full
  resolution, capped to `DecodeOptions::max_dimension`, into a `DecodedImage` of straight-alpha RGBA8
  with its `ImageColorSpace` and optional `ExifData`. iOS and macOS decode through ImageIO (HEIC
  included) and Android through `ImageDecoder`; Linux and Windows return `Unsupported`, and a device
  with no decoder for the format returns `Unsupported`. The call blocks, refuses the mobile UI thread
  and checks its `CancelToken` between stages. Without the feature both calls return
  `FeatureDisabled`.
- The plugin registry's `photo-library` entry gains a `decode` option (`frust plugin add photo-library
  --feature decode`, or the TUI's Add Plugin dialog): it enables the Cargo feature and links ImageIO
  on iOS, with no permission or plist string.
- `frust_i18n::fmt::device_time_zone()` and `device_offset_seconds(unix_seconds)` (the `formatting`
  feature): the device's current IANA zone name and its UTC offset at an instant, read from the
  platform on every call through the `iana-time-zone` crate. Both return `None` when the platform
  reports no zone or one the bundled database does not know.
- `frust plugin add <id> [--feature <f>]...`: adds a plugin to the project in the current
  directory with the same edits as the TUI's Add Plugin dialog. It is idempotent, and an unknown
  id or feature lists the valid ones.
- `wrap()` (`WrapView`, `WrapWidget`, `WrapAlignment`, `WrapCrossAlignment`, re-exported from
  `frust`): a layout widget whose children flow along a main axis and break into runs, like
  Flutter's `Wrap`, with `spacing`, `run_spacing` and alignment setters.
- `frust-photo-library` (`library` feature): `Asset` gains `modified_unix_ms`, `is_favorite`, `burst_id`,
  `file_name` and `content_type`. `file_name` and `content_type` are filled only when the query sets the
  new `AssetQuery::with_details()`, because on iOS they cost one resource lookup per asset. `is_favorite`
  is `None` below Android API 30, `burst_id` is always `None` on Android, and `content_type` is a MIME type
  on Android and a uniform type identifier on iOS.
- `PhotoLibrary::asset_metadata(&AssetId, &MetadataOptions)` and `MetadataOptions` (`allow_network`,
  `cancel`): one library asset's `ExifData` with only the `library` feature. On iOS the call reads
  the whole local original into one buffer that ImageIO reads in place, a read with no data for 30 s
  times out, an original over 256 MiB is refused with `InvalidRequest`, and an iCloud-only original
  is `NetworkRequired` unless `allow_network` is set. On Android GPS fields need
  `ACCESS_MEDIA_LOCATION`. Desktop returns `Unsupported`.
- `PhotoLibrary::thumbnail_shared(&AssetId, &ThumbnailRequest, &CancelToken)`: returns an
  `Arc<Thumbnail>`. The cache never copies pixels and a hit shares the cached allocation, but
  building an `ImageSource` from them copies them once; a miss can be cancelled on iOS and Android
  with `PhotoError::Cancelled`, caching nothing. `thumbnail` keeps its signature and shares the same
  cache.
- `frust-photo-library` (`library` feature) change feed: `PhotoLibrary::current_change_token()` returns
  an opaque `ChangeToken` (persist it with `as_bytes`, restore it with `from_bytes`), and
  `PhotoLibrary::changes_since(&ChangeToken)` answers a `LibraryChanges`: a `Delta` of `inserted`,
  `updated` and `deleted` ids plus the `next` token to store, or a `FullRescan` with a fresh `next`
  when the platform cannot say what changed. `PhotoLibrary::on_library_changed(callback)` returns a
  `ChangeSubscription` whose drop unregisters it; the callback only wakes the app (no data, any thread,
  possibly coalesced or spurious), so answer it by calling `changes_since` off the UI thread. iOS reads
  Photos' persistent change history (iOS 16+) and merges its batches into one delta; Android compares the
  `MediaStore` generations (API 30+) and the token's id set. A `FullRescan` answers iOS 15, Android API 29,
  a token from another device or platform, an expired token (iOS), a changed `MediaStore` volume version
  (Android) and bytes that do not decode. On Android an id that is visible but absent from the token's id
  set counts as inserted. A `Delta` evicts every id it names (inserted, updated and deleted) from the thumbnail cache, and a
  `FullRescan` clears it, before `changes_since` returns. Desktop and web return `Unsupported`.
- `frust-photo-library` cursor paging: `PhotoLibrary::assets_after(&AssetQuery, Option<&AssetCursor>)`
  returns an `AssetCursorPage` (`items`, and `next` until the last page) that stays stable while the
  library changes between pages. `AssetQuery`, `AssetPage` and `assets` are unchanged. Android pages by
  keyset on the `assets` order, with no row count and no seek; iOS pins one `PHFetchResult` snapshot per
  cursor session, so changes are invisible until a new session starts from `None`, and four sessions are
  kept (an evicted session's cursor answers `InvalidRequest`). A cursor lives in memory only and is bound
  to the album and filter that made it.
- `PhotoLibrary::cloud_identifiers(&[AssetId])` (iOS 15+): the iCloud identifier of each asset, one
  entry per id in order, `None` where there is none. Unlike an `AssetId` it names the asset on every
  device of the same iCloud Photos library. Every other platform returns `Unsupported`.
- `AssetId` documents its stability: best-effort the same across launches on one device, so an app may
  store it; not portable to another device or platform, and a restore or reinstall may change it.
- The playground's Photos page gains a Changes panel and a cursor-paging toggle.

### Changed

- accesskit 0.25, `accesskit_winit` 0.34, `accesskit_android` 0.9, `accesskit_ios` 0.2.2.
- The plugin registry's `photo-library` `library` option now also links `ImageIO` on iOS, which
  `asset_metadata` reads through; a project that also enables `decode` links it once.

### Fixed

- An Android app no longer aborts when an accessibility client detaches or the host view is
  detached and the UI then changes (for example `uiautomator dump`, then a navigation or a
  rotation). On a OnePlus 9 (Android 15) the build before the bump aborted 10 of 10 and this one
  0 of 10; a Pixel 4a (Android 16) also ran 10 of 10 clean. TalkBack speech, focus order and typing
  under TalkBack were not verified by hand.

## 0.6.6 — 2026-11-11

A patch release (the 0.6.x line continues) with three breaks, listed under Breaking: `Router::go`
and `RouteNavigator::go` now make the stack exactly the matched chain, `frust-shell-common`'s
non-default `gpu` feature replaces `install_gpu_handle` and `gpu_handle` with `GpuPublisher`,
`with_gpu_handle` and `gpu_handle_generation`, and the `frust-shell-ios` `ffi_glue` functions that
dereference a caller's pointers are now `pub unsafe fn`, with `init` also taking a `pre_init`
closure. Narrow source breaks are marked in Changed with their migrations: new `MainAxisAlignment`
and `PathStyle` variants, `PageParity::SpillPool` with `MAX_LIVE_PAGES` raised to 5,
`#[non_exhaustive]` `TextStyle`, the new `InspectNode::detail` field, the `pre_init` parameter on
Android's `native_init`, and added variants or fields on `Capability`, `Method`, `RenderCommand`,
`RenderEvent` and `CompiledFrame`. Behaviour changes to published APIs are marked there too:
`AnimationController::reverse()` at 0.0, `PaintCtx::frame_time()`, `frust::gpu::with_context`, the
facade's default `complex-scripts` feature (about 3 MB per app), the OS text scale on default-size
`Text`, `frust_paths::atomic_write` and `data_dir`, and `frust-i18n` locale negotiation. The release
adds four plugin crates (`frust-background-task`, `frust-device-info`, `frust-permissions` and
`frust-photo-library`), file and photo access on Android and iOS, `frust::time`, the
`frust::testing::Tester` driver, `frust::motion::presence` and snap-point detents, `glass_surface`,
stroke caps and joins, rich text spans, and font features and decorations. Apple's hvgl-only system
fonts (PingFang) now draw from the glyph atlas.

### Breaking

- `Router::go` and `RouteNavigator::go` now make the stack exactly the matched chain, as their docs
  said and as go_router defines `go`. They used to replace only the top page, so pages below it
  survived. Migration: to keep a page below a shell or another page, make it a parent in the route
  table (`Route::new("/", ..).child(shell_route(..))`); a hand-written pop to the root before `go`
  can go. What follows from it:
  - A page whose matched path is unchanged keeps its state and takes the new params, and a shell
    page survives any `go` whose chain crosses it. A parent page in a matched chain publishes its
    own path in `route_stack`, and a `push` placing a shell from its parent page stacks only the
    shell.
  - A warm deep link replaces the stack (`handle_location` is a `go`), and one matching no route
    makes it a single error page. An app receiving links no route is for, such as an OAuth callback,
    declares a route for them or handles them before `track()`.
  - `go` closes route-less overlays, including a non-dismissable one over a page it keeps; a
    security gate must be a route redirect. A removed page's `on_result` fires once with an empty
    result, on the `go`'s own frame.
  - Only the top page animates, with its route's transition or the navigator's default. Removed
    pages stay alive until it settles, though depth and back already report the new stack. A `go`
    applied in a navigator's first build starts no transition, and when several `go` calls land in
    one frame the last wins and the earlier pages are not built.
- `frust-shell-common`'s non-default `gpu` feature: `install_gpu_handle` and `gpu_handle` are
  removed. A shell now publishes through an RAII `GpuPublisher` into a replaceable,
  generation-tagged slot. Migration: hold a `GpuPublisher` for the device owner's lifetime and call
  `publish(generation, make)`; read with `with_gpu_handle::<T, _>(f)` (the handle is current only
  for the call) and watch `gpu_handle_generation()` to detect a replacement.
- `frust-shell-ios`: 29 of the 32 `ffi_glue` functions, and the matching `ios_app!` exports, are now
  `pub unsafe fn` with a Safety contract each, because they dereference a pointer the caller passes;
  `system_ui_state`, `set_surface_mode` and `set_present_sync` stay safe. `ffi_glue::init` also
  gains a required `pre_init: impl FnOnce()` parameter before `make_app`; it runs first, inside the
  function's panic guard and before the GPU device exists, and a panic in it makes `init` return
  null. C symbols and signatures are unchanged, so Swift callers are unaffected, and so are apps
  using `ios_app!`, which passes the closure. Migration: a Rust caller of those functions wraps the
  call in `unsafe` and honours the contract on each, and passes `|| {}` to `init` or a closure that
  runs early setup such as `frust::gpu::request_optional_features(..)`.

### Added

- `frust-background-task` (new plugin crate): add it from the `frust` TUI's Add Plugin dialog, then
  call `frust_background_task::install()` in the app's setup. It backs `frust::BackgroundTask` with
  an iOS background-task assertion or an Android `shortService` foreground service
  (`FOREGROUND_SERVICE`, a low-priority notification, no `POST_NOTIFICATIONS` request). A refused
  start is the no-op guard with `is_granted()` false; a watchdog of about 150 s bounds the service
  and `expired()` signals the app to cut work short. Apps targeting Android 14+ must make a Play
  Console declaration, or opt out with `tools:node="remove"`. Checked on a Pixel 5 (Android 14) and
  an iPhone SE.
- `frust::BackgroundTask` (`begin`, `expired`, `is_granted`, `end`; dropping it ends the task
  exactly once, from any thread): a no-op on every platform until the plugin provides a backend,
  which it does only on iOS and Android, and a `begin` with no backend logs one warning on those
  two. The plugin installs only when no backend is set. The sanctioned pattern: a
  `Timer::flush_on_background` callback calls `begin` on the UI thread, then
  `spawn_blocking(write + drop(task))` (not on web, where `spawn_blocking` panics).
- `frust::time::{sleep, Timer, Duration}`: `sleep(duration)` is an awaitable wall-clock delay (a
  real browser `setTimeout` on web, cleared when dropped or cancelled; durations above `i32::MAX` ms
  clamp and a background tab may throttle it). `Timer::new(duration, callback)` runs a callback
  once, with `cancel`, `is_active`, `fire_now` and `detach`; it is cancelled when dropped and with
  its owner, and the callback runs under the owner that armed it. A timer fires on the next desktop
  loop turn or mobile display tick while resumed; nothing ticks regularly while backgrounded, so a
  due timer may fire on the first tick after resume or in a shell's background pump.
  `frust::spawn_local_scoped` is `spawn_local` bound to the calling owner.
- `Timer::flush_on_background()` runs a timer's callback synchronously at the first of the app going
  inactive or hidden (also on focus loss, once). Only synchronous work in it is guaranteed to
  complete before suspension; work it spawns is not, and no background-task assertion is provided.
  Native debug builds log a warning when a callback runs over 8 ms or a drain over 16 ms, because
  the OS lifecycle callback is blocked meanwhile; release and web builds do not measure.
- `MemoryPressure` (`Moderate`, `Critical`): `AppLifecycleListener::on_memory_pressure`, the
  shell-facing `push_memory_pressure` and the diagnostic `last_memory_pressure()`, independent of
  `AppLifecycleState`. iOS reports its memory warning as `Critical`; Android maps `onTrimMemory`
  levels (`RUNNING_MODERATE`, `RUNNING_LOW`, `BACKGROUND`, `MODERATE` to `Moderate`,
  `RUNNING_CRITICAL` and `COMPLETE` to `Critical`); macOS reports a libdispatch source; Windows,
  Linux and web report none.
- On `Critical` pressure iOS and Android trim the render caches (`SurfaceRenderer::trim_memory` /
  `EngineRenderer::trim_memory` hold retained pages to the plain 32 MiB budget and clamp atlas
  growth, one-way until the renderer is rebuilt); macOS only reports. iOS reaches this through the
  additive C export `frust_memory_warning` (`frust_ffi.h`, called by `FrustViewController`).
  `frust_render::set_reported_device_memory` reports device memory where there is no
  `/proc/meminfo`, and under `perf-trace` iOS logs
  `frust-perf ios-mem phys_footprint_kib=<n>|unknown` about once a second.
  `frust build ios --define FRUST_FORCE_MOBILE_TIER=1` forces the plain tier.
- `frust::paths::{data_dir, cache_dir, atomic_write}`: the platform data and cache directories and
  the write-to-temp-and-rename helper, re-exported from `frust-paths` so an app depending on the
  facade alone can place caches, sidecars and export staging; the module docs say what each platform
  backs up.
- `frust-photo-library` (new plugin crate): `PhotoLibrary::pick(&PickOptions)` opens the system
  photo picker on Android and iOS (no permission or Info.plist string needed) and a native file
  dialog on desktop, with a media filter, a limit, throttled `on_progress` reports (`PickProgress`)
  and a `CancelToken`. Mobile picks are copies in an app cache, reclaimed with `clear_pick_cache`
  (`PickedItem::is_copy`); `pick` blocks, refuses the UI thread on mobile and allows one pick at a
  time (`PhotoError::Busy`), and cancelling removes partial copies (`PhotoError::Cancelled`). The
  `photo-library` registry entry adds the Gradle module and the `PhotosUI` link.
- `frust-photo-library` optional `library` feature (Android media read permissions,
  `NSPhotoLibraryUsageDescription`, the `Photos` link): browse with `PhotoLibrary::albums`, `assets`
  (newest-first pages with an `AssetQuery`) and `thumbnail` (straight-alpha RGBA8, `Fill` or `Fit`,
  a byte-budgeted cache with `set_thumbnail_cache_budget` and `clear_thumbnail_cache`), plus
  `manage_limited_selection`. Browsing is read-only and never takes the pick guard; iOS picks carry
  `asset_id` under the feature.
- `PhotoLibrary::export_original(&AssetId, &ExportOptions)` (`library`) writes an asset's untouched
  original or edited bytes to a file, with `allow_network`, progress and a `CancelToken`; the
  default destination is left alone by `clear_pick_cache`. `PhotoLibrary::save(&SaveRequest)` (the
  `save` feature) files an image or video into Photos or `MediaStore`, optionally into an album.
  `PhotoLibrary::scan_folder(&FolderGrant, &FolderScan)` pages a granted folder's images and videos,
  newest first. New `PhotoError` variants: `NetworkRequired`, `MissingUsageDescription`,
  `InvalidRequest`. Stored location is kept by Android browse and export with Media location granted
  and by the iOS picker, and zeroed by the Android Photo Picker
  (`photo-library-android-location-redacted`).
- `frust-permissions` (new plugin crate): one `status` / `request` / `open_settings` API over
  Android runtime permissions and Apple authorization for photos (read/write or add-only), camera,
  microphone and media location (the location stored inside photos, never device position). Each
  permission is an opt-in cargo feature, and the registry entry (`photos`, `photos-add`, `camera`,
  `microphone`, `media-location`) adds the matching manifest permissions, Info.plist strings,
  framework links and macOS entitlements. Linux, Windows and web answer `Unsupported`.
- `frust-device-info` (new plugin crate): `DeviceMemory::query()` reports total RAM, the Android
  low-RAM flag, system-available memory (Android, Linux, Windows) and the iOS per-process headroom,
  so an app can size caches on Android 14+, where no foreground trim-memory level is delivered any
  more.
- `frust-file-dialogs` on Android (Storage Access Framework) and iOS
  (`UIDocumentPickerViewController`): `open_file` and `open_files` now work, returning app-private
  copies; `save_file` still answers `Unsupported` there, so use the new `FileDialog::export_file`
  (`ExportOutcome`). Also new: `pick_folder` with persistable `FolderGrant`s (`token`, `restore`,
  `local_path`, `list`, `release`; `FolderEntry`; `ListOptions`) and `FileDialog::cancel` /
  `on_progress` with `CancelToken` and `FileProgress`. UI-thread calls are refused with `UiThread`;
  web is unchanged. `FileDialogError` gains `UiThread`, `NoPresenter`, `PlatformNotInitialized`,
  `AccessLost`, `Io`, `Platform` and `Cancelled`.
- `Share::files` and `Share::files_anchored` share one to 64 files: the Android chooser (through a
  `FileProvider` in the plugin's new Gradle module), the iOS activity sheet and a macOS sharing
  picker; Linux and Windows answer `ShareError::Unsupported`. On Android `Ok` means the share is
  enqueued, with files copied on a background thread and any failure logged under tag `FrustShare`.
  An existing app re-runs Add Plugin for `share` to include the module; text and URL sharing are
  unchanged.
- `frust-database`: `Database::schema_version`, `Database::execute_batch` (many statements in one
  transaction) and `Database::migrate(&[Migration])`, one transaction per step with
  `PRAGMA user_version` bumped inside it, identical on both engines. A stored schema newer than the
  last migration the app knows is an error, `DatabaseError::Migration` (the new variant, also
  returned for an invalid list), so a downgraded binary cannot read a schema it does not know.
- `SharedPreferences::in_memory()`: a public, infallible, process-local store that is never
  persisted; clones share the map.
- `frust::testing::Tester` (facade `test-support` feature): a state-generic headless driver under
  its own child reactive owner. `pump` / `pump_and_settle` / `pump_until`; `tap` / `press` /
  `long_press` / `drag` / `timed_drag`; `start_gesture` / `start_touch` with `move_by` / `move_to` /
  `up` / `up_at` / `cancel`; `find::{text, label, role, by_type}`, `get_semantics` / `bounds` /
  `perform_action`; `scene` / `semantics` / `inspect` / `send`; `set_metrics` / `resize`. Built with
  `new`, `with_options` (`TesterOptions`) or `new_with`, with no process-wide lock, so Tester tests
  run in parallel.
- `Tester::pump(Duration)` drives a per-thread virtual clock: `frust::time` sleeps and `Timer`s fire
  deterministically, stepping through each deadline in order (a chain or re-armed timer inside one
  pump fires at its own virtual time). `pump_until` remains the real-time wait, and any future
  polled off the Tester's thread stays on wall time, including `frust::spawn` and `use_task` fetch
  futures. The hook is `frust-reactive`'s `test-support` feature (forwarded by the facade's):
  `time::test_support::{install_virtual_clock, advance, now}`.
- `Tester::{text_runs, text_overflows, assert_no_text_overflow}` and `TextRun`: list every text run
  with its geometry and flags, and fail a test when one is cut or sticks out. Containment stops at a
  clipping ancestor: content a scroll, list or page viewport hides is `TextRun::hidden` or
  `clipped`, never an overflow. `frust::testing` re-exports `InspectDetail` and `TextInspect` next
  to `InspectNode`, and `frust::authoring` re-exports `TextWidget`.
- `frust::testing::{semantics_of, scene_of, scene_of_with}` (`SceneOptions`): one-shot helpers for
  an out-of-tree design system, enabled by the `frust-ui` `test-support` feature from
  `[dev-dependencies]`, which also makes `PaintCtx::for_test` available. `test-support` on
  `frust-text` and `frust-widgets` exposes the shared `LocaleSlotGuard` over the default-locale
  slot.
- `GestureDetector` tap callbacks: `on_tap_down`, `on_tap_up` (before `on_tap`) and `on_tap_cancel`,
  with `#[non_exhaustive]` `Copy` `TapDownDetails` / `TapUpDetails` (`local_position`) from
  `frust-ui`. Down fires at once; a slop or bounds exit, a stolen or system cancel, or a second Down
  cancels, and exactly one up or cancel follows each down (leaving the bounds cancels but a later
  `on_tap` fires).
- `GestureDetector::on_double_tap` and `DoubleTapDetails { local_position }`: two taps within 300 ms
  and 100 px of each other (each within the 18 px slop), timed on the raw input clock. With it wired
  the single `on_tap` waits out the window and is delivered afterwards, and a second press that
  fails drops the first tap; detectors without it are unchanged.
- `GestureDetector` drag callbacks
  `on_{vertical_drag,horizontal_drag,pan}_{start,update,end,cancel}` with `#[non_exhaustive]`,
  local-only `DragStartDetails` / `DragUpdateDetails` / `DragEndDetails`. Axis recognizers start
  after 18 px along their axis, pan after 36 px; the first to cross wins and a same-axis scroll view
  takes the drag first. Release velocity is on the raw input clock, and `cancel` arrives on the next
  state-bearing pass, not inside the `Cancel` event.
  `drag_start_behavior(DragStartBehavior::{Start, Down})` reports the start where the slop was
  crossed (the default) or at the press point.
- A second pointer contact landing while a `GestureDetector` press is pending fires `on_tap_cancel`
  with no long press, tap or double tap, whenever a contact-capturing widget (`pinch_detector`,
  `pan_zoom`) sits below the detector. A bare detector never hears the second finger
  (`gesture-second-contact-needs-a-contact-captor`).
- Rich text spans: `frust-text` `TextSpan` and `TextContext::layout_rich` / `layout_rich_bounded`
  lay out differently styled pieces as one paragraph, and `frust-widgets` `rich_text(spans)` and
  `span(text)` add per-span size, colour, weight, family, italic, letter spacing, locale, font
  features, decorations and themed roles, resolving theme and the OS text scale like `TextView`. A
  truncated line paints its ellipsis within the width unless the ellipsis alone is wider.
- `TextStyle` `with_<field>` builders for every field, chained from `TextStyle::new(size, color)` or
  `Default`.
- `OverflowWrap` (`Normal`, `Anywhere`; `frust::authoring::text`), `TextStyle::with_overflow_wrap`
  and `TextView::overflow_wrap` set per-paragraph wrapping and join the shape-cache key;
  `TextView::locale(..)` tags a paragraph without marking color or family explicit, so it keeps
  theme defaults and the OS text-scale rule.
- OpenType font features: `TextStyle::with_feature(tag, value)` / `with_features(..)` (a later
  setting of a tag wins) and `TextView::font_feature(s)` / `tabular_figures()`. Text decorations:
  `TextDecoration` on `TextStyle::with_decoration`, `TextView::underline()` / `line_through()` /
  `decoration(..)` and `TextLayout::decoration_rects`, painted as plain fills. `TextInput` draws
  none (see LIMITATIONS).
- `MonospaceFamily` theme extension (`Theme::monospace_family()`, attached by the shadcn, beUI and
  Glyph themes) with `TextView::themed_mono_family()`, and `TextInputView::themed_family(role)` /
  `themed_mono_family()` so a theme swap re-faces typed text; the catalog text fields use them.
- `TextLayout::{line_count, max_width, did_exceed_max_lines, is_ellipsized, overflows_width}` report
  how a laid-out block fit its limits, and `TextLayout` is now `Sync`.
  `TextContext::notdef_glyph_count()` counts `.notdef` glyphs per context across retained layouts,
  the shape cache and the editor.
- `TextContext::font_cache_stats()`
  (`FontCacheStats { write_failures, written_generation, dirty, attached, persistent }`) and
  `BackgroundFlush::went_to_background_outcome()` (`BackgroundFlushOutcome`, per-context results;
  `went_to_background` still returns the flushed count), re-exported through `frust_text`,
  `frust_shell_common` and `frust::authoring::text`. A report whose every flush failed leaves the
  once-per-transition latch unarmed so the next hide retries.
- `frust::authoring::text::LocaleStamp` (`frust_text::LocaleStamp`): `for_style` / `is_current`
  stamp a cached layout with the default-locale generation; read it before shaping and store it with
  the layout. `frust-widgets` `Text` / `TextInput` and the glyph, material, beUI, shadcn and
  native-widgets layout caches follow that rule.
- `frust-theme` OS text scale: `TextScale`, `TypeScale::scaled`, `Theme::with_text_scale`,
  `Theme::text_scale`, `Theme::authored_type_scale`, `TextScale::clamped` and `scale_style`. The
  additive iOS C export `frust_set_text_scale` (`frust_ffi.h`) lets a Swift embedder that hosts
  frust outside `FrustViewController` push Dynamic Type, which the shipped controller does itself.
- `glass_surface(child)` (`GlassSurfaceView`, flat from `frust`) paints one tier of the theme's
  glass scale behind its child: an outside shadow, rounded clip, backdrop blur and washes, specular
  hairline. `.tier(GlassTier)` picks `Control`, `Bar` (default) or `Chrome` from `GlassScale::tier`;
  `.corner_radius(f64)` clamps to a capsule, and `.material(..)` wins over the theme's tier. Without
  either it paints opaque `#EFEFEF`. A material's blur is a Gaussian sigma in logical px (CSS
  `blur(Npx)` is sigma N), clamped by the engine.
- `set_max_backdrops_per_frame(n) -> usize` and `max_backdrops_per_frame()` (flat from `frust` and
  `frust-render`): the per-frame backdrop-blur cap is app-configurable, process-wide and clamped to
  `1..=16` (default `MAX_BACKDROPS_PER_FRAME` 4, ceiling `BACKDROP_CAP_CEILING`). A frame over the
  cap is still refused whole (`EngineError::TooManyBackdrops`), and
  `SceneCompiler::compile_with_backdrop_cap` compiles against an explicit cap.
- Stroke caps, joins and miter limit, and the even-odd fill rule, in `frust-scene` and
  `frust::authoring::scene`: `StrokeCap`, `StrokeJoin`, `StrokeStyle { cap, join, miter_limit }`,
  `FillRule`, and `fill_path_with_rule` / `stroke_path_styled` on `SceneBuilder` and `PaintScene`
  (defaults fall back to `fill_path` / `stroke_path`). A non-finite or below-1 miter limit is
  refused as `InvalidGeometry`. A dashed closed subpath with a skipped stretch joins its seam under
  non-round styles, and the culling reach of a mitred stroke caps the miter factor so a huge limit
  stays finite. The new `PathStyle` variants are a narrow break, below.
- Stroked icons: `IconShape` (`Path` / `Circle` / `Rect`), `IconPaint` (`Stroke` default, `Fill`),
  `IconElement` (`stroke` / `fill`, `with_opacity`, `with_dash`, `with_stroke_width`) and
  `StrokedIconSource`, converted to `IconData` by `From<StrokedIconSource>` or
  `IconData::from_elements` (`from_styled_elements` with `IconElementStyle` for runtime-built
  icons). `IconData::resolve()` returns one fillable outline for fill-only slots and `is_stroked()`
  reports one; `IconView::stroke_width(f64)` overrides the width.
- Framework haptics in `frust-core` (flat from `frust` and `frust::authoring`): `#[non_exhaustive]`
  `HapticFeedback` (`Selection`, `Light`, `Medium`, `Heavy`, `Success`, `Warning`, `Error`),
  `HapticSink`, the last-wins `set_haptic_sink`, `haptic_sink_installed()` and `perform_haptic()` (a
  no-op without a sink); the `frust-haptics` README shows the wiring. `MaterialHaptics` forwards to
  the sink when no Material hook is set (`SliderTick` maps to `Selection`, `None` to nothing).
- Animation time scale: `set_animation_time_scale(f64) -> bool` (`MIN_ANIMATION_TIME_SCALE` 0.01 ..=
  `MAX_ANIMATION_TIME_SCALE` 100.0), `animation_time_scale()` and the raw clock
  `PaintCtx::input_time()`, read by gesture timers, caret blink, velocity timestamps, drag hold,
  auto-scroll, the catalog tooltips and the material and shadcn press latches. Devtools adds the
  authenticated `set_animation_time_scale` method, the `animation_time_scale` capability and
  `DevtoolsClient::set_animation_time_scale`; the protocol stays v1, and a scale a client set resets
  to 1.0 when it disconnects.
- `frust::motion::detent` (flat in `frust::motion`): `Detents`, pure NaN-safe snap-point math
  (`open_progress`, `drag_to`, `release_target`, `settle` returning `DetentRelease` under a
  `DismissPolicy`), and the headless `DetentController`, whose release hands the fling velocity to
  `spring_to_with_velocity` (`with_fling_velocity`, default `DEFAULT_FLING_VELOCITY`). `SpringBack`
  snaps back to open; on `Dismiss` the controller stops and holds progress and the caller owns the
  exit animation.
- `frust::motion::presence`: `Presence` and `presence(open, child)` with `.timing(enter, exit)` and
  `.on_exited`. The child stays mounted: always built and laid out, painted only while visible,
  input only while open, semantics only while visible. Reversing a duration ramp is value-continuous
  and a spring value- and velocity-continuous; `on_exited` is delivered on the next frame's
  Housekeeping event, once and only after an exit has painted, so it is a cleanup cue rather than a
  correctness gate. Under `reduce_motion` it runs a linear reduced fade. A settled child paints
  without an alpha layer.
- `AnimationController::velocity()` and the velocity-carrying retarget
  `spring_to(target, SpringDesc)` / `spring_to_with_velocity(target, velocity, SpringDesc)` in
  `frust-core`, so a fling or release velocity carries into the spring. `frust-theme`
  `MotionExtras { extended: 800.0, reduced_fade: 120.0 }` (ms; read with `MotionExtras::of(theme)` /
  `reduced_fade_duration()`; flat as `frust::MotionExtras`) and
  `resolve_spec_with(spec, scheme, reduced_fade)`.
- `column()` / `row()` `.gap(f64)`: a fixed space between consecutive children (negative or NaN is
  `0`).
- `Route::transition(TransitionSpec)` and `Router::push_with(location, PushOptions)`: precedence
  push options, then route, then navigator default, all collapsing under reduce motion.
  `PushOptions::barrier(Color)` and `barrier_dismissible(bool)` add a route-owned modal scrim
  following the route's eased progress; a press the page leaves unhandled dismisses through its
  `BackPolicy` unless it lands on content.
- `Widget::hit_test_self` (provided, default `true`): `false` makes a widget transparent to pointer
  hits beside its children; layout wrappers such as `Padding`, `SafeArea` and `Align`, and a
  `Container` that paints nothing, return `false`.
- `Widget::inspect_detail`, the `#[non_exhaustive]` `InspectDetail` enum and `TextInspect` let a
  widget attach typed detail to its inspect node. The text and rich-text widgets report their last
  layout through it, `EitherWidget`, `EitherOf3Widget`…`EitherOf8Widget` and `ShortcutsWidget`
  forward it, and `InspectDetail::Clip` marks the scroll, list, page, viewport, pan-zoom, navigator
  and clipping sliver widgets.
- `frust::Layer` (with `Layer::IDENTITY`) and `frust::resolve_layers`, beside `TransitionDriver`,
  `make_driver` and `resolve_spec`, so `PageTransition::Custom` is authorable out of tree.
- `SceneTextureHints` for externally bound textures (`filter`:
  `SceneTextureFilter::{Linear, Nearest, NearestWhenMagnified}`, and a caller-asserted `opaque`):
  `EngineRenderer::bind_texture_with` and `ExternalFrame::bind_texture_with`, re-exported from
  `frust-engine`, `frust-render` and `frust::gpu`. An `opaque` texture joins the depth-writing
  opaque pass; unhinted textures behave as before.
- Texture bus: `SourceFormat::Bgra8` (premultiplied) and sub-rect updates through
  `SourceFrame::with_update_region` / `UpdateRegion` (re-exported as
  `frust::texture::UpdateRegion`), refused with a one-time warning when no same-size, same-format
  texture is shown. The bus detects a GPU device change by generation and recovers region producers
  on the new device.
- GPU device identity: `DeviceHandle::generation()` (process-wide, monotonic, non-zero, shared by
  clones), `DeviceHandle::is_lost()` (latched by the device-lost callback) and
  `frust::gpu::device_generation()`. Android and iOS publish their live device to
  `frust::gpu::with_context`, as desktop does.
- `frust::gpu::request_optional_features` / `requested_optional_features` (and
  `frust_render::missing_requested_features`): a process-wide, adapter-conditioned union of optional
  wgpu features honoured by the shells and the headless renderer; a missing feature is left out.
  `app!(Root, gpu_features = { .. }, ..)` (facade `gpu` feature; `@pre_init` on `android_app!` /
  `ios_app!`) requests them before the device is built, and on Android a pre-init device lacking one
  is rebuilt.
- External passes: `ExternalFrame::drain_id()` and `device_generation()` identify the surface drain
  and device a pass records for. `HeadlessRenderer::render_with_passes` and
  `render_draining_registry` render passes offscreen (`EngineOracle` has the same two), with
  `ExternalFrame::frame_index` counting that renderer's own renders from 0 on the explicit path.
  `frust::gpu::headless` re-exports the headless types and golden env-var names so an app can
  golden-test its own pass. `frust_render::ExternalPassDrain` (doc-hidden, shell-facing) is the
  per-engine drain state a custom shell passes to `run_external_passes`.
- Refused frames: `EngineRenderer::prepare` (every plan-stage refusal) and `encode_prepared` /
  `encode_prepared_traced`, `RegionCopyMode::of_usage`, `SurfaceRenderer::engine_refusals()`
  (`EngineRefusalReport { count, first_reason }`) and `frust_gpu::lifecycle::MAX_TRANSIENT_SKIPS`
  (3). The `FRUST_RENDER_INJECT_REFUSAL` knob (`plan:N` / `record:N`) refuses every Nth frame, in
  release builds too.
- `EngineRenderer::set_synthetic_oblique` / `synthetic_oblique` and `set_glyph_run_cache` /
  `glyph_run_cache` forward those switches on the renderer, matching `SceneCompiler`.
  `SceneCompiler::glyph_resident_texels()` and `glyph_texel_budget()` report what the glyph atlas
  charges against its budget.
- Bitmap-strike colour emoji render on the engine: bitmap-first faces (sbix without outlines, CBDT)
  take an engine-side route, drawn by CoreText on iOS and decoded from PNG/dupe/flip records
  elsewhere, each strike charged and refunded its exact atlas slot. Strikes over 512 px are refused;
  faces with outlines draw outlines; BGRA and mask strikes still do not draw.
- Apple's hvgl-only system faces (PingFang) draw from the glyph atlas instead of as outlines, by
  default (`SceneCompiler::set_hvgl_atlas` / `EngineRenderer::set_hvgl_atlas` switch the route,
  `OutlineReason::Hvgl` is the fallback), checked on an iPhone 13 mini (iOS 27.2). `CompiledFrame`
  gains `hvgl_atlas_draws` and `hvgl_outline_fallbacks`, and the `frust-perf enc` line appends the
  route's counts. Faux bold and faux oblique are not applied on this route
  (`text-hvgl-synthesis-not-applied`), and two macOS system faces have no hinting instance, so a
  hinted faux-bold request on them is nearly invisible (`text-hinted-faux-bold-unhintable-face`).
- hvgl faces are verified against CoreText: each variation copy is read back and each requested axis
  classified as applied, contradicted (refusing the instance, `hvgl_variation_mismatches`) or
  unreported (drawing CoreText's copy, `hvgl_variation_unverified()`), and opening a face compares a
  `cmap` sample (`hvgl_bytes_mismatches`). `SceneCompiler::hvgl_outline_hits` /
  `hvgl_outline_misses`, the per-frame `CompiledFrame::hvgl_unavailable_glyphs` and the cumulative
  counter round out the diagnostics. Each kind warns once per open face; off Apple every
  hvgl-classified glyph counts as unavailable.
- Material `zh-Hans` strings and `frust_i18n::Locale::likely_script`.
- The playground gains Timers and guarded write buttons on its Network page, Picker, Browse,
  Originals and Folders panels, Export file, Pick folder and Cancel on its Desktop page, a Photos
  section and a "GPU pass" section.

### Changed

- **Breaking (narrow):** `MainAxisAlignment` gains `Center`, `End`, `SpaceBetween`, `SpaceAround`
  and `SpaceEvenly`, distributing leftover space only on a bounded main axis with no flexible child.
  Migration: an exhaustive `match` over it adds the arms or a wildcard.
- **Breaking (narrow):** `PathStyle` gains `FillEvenOdd` and `StrokeStyled { width, dash, style }`
  (rendered by the engine and the CPU oracle). Migration: an exhaustive `match` over `PathStyle`
  outside frust adds the two arms, or reads through `PathStyle::fill_rule()` / `stroke()` instead of
  matching.
- **Breaking (narrow):** `frust_engine::schedule::PageParity` gains `SpillPool(SpillSlot)` and
  `MAX_LIVE_PAGES` goes from 3 to 5: three `MAX_SPILL_PAGES` slots serve a fan nested inside a fan
  under a held ancestor and every tree whose isolated layers hold at most two isolated children,
  costing up to +128 MiB of intermediates only on frames that use them. Migration: an exhaustive
  `match` needs a wildcard arm or the new `PageParity::is_spill()` / `spill_slot()`
  (`PageParity::spill(n)` builds one, `None` past the limit).
- **Breaking (narrow):** `TextStyle` is now `#[non_exhaustive]` and gains the public fields
  `overflow_wrap`, `features` and `decoration`. Migration: build it with
  `TextStyle::new(size, color)` or `Default` plus the `with_*` builders; struct literals and
  exhaustive destructures no longer compile outside `frust-text`.
- **Breaking (narrow):** `InspectNode` gains the public field `detail`. Migration: a hand-written
  struct literal adds `detail: None`.
- **Breaking (narrow):** tooling and shell-facing types gained members:
  `frust_devtools_protocol::Capability` and `Method` (`AnimationTimeScale`,
  `SetAnimationTimeScale`), `frust_shell_common::RenderCommand` and `RenderEvent` (`TrimMemory`),
  and `frust_engine::CompiledFrame` (five `hvgl_*` fields). Migration: an exhaustive `match` adds
  the arms or a wildcard; a `CompiledFrame` struct literal or pattern adds the fields or `..`.
- **Breaking (narrow):** `frust_shell_android::jni_glue::native_init` gains a required
  `pre_init: impl FnOnce()` parameter between `cache_dir` and `make_app`; it stays a safe function.
  It runs first, inside the panic guard and before the shell adopts the GPU device built at library
  load, and a panic in it makes `nativeInit` return 0. `android_app!` users are unaffected, since
  the macro passes the closure. Migration: a hand-written embedder passes `|| {}`, or a closure that
  runs early setup such as `frust::gpu::request_optional_features(..)`; if the pre-built device
  lacks a requested feature its adapter offers, the shell rebuilds it, a synchronous device build
  at that launch.
- **Behaviour change:** `AnimationController::reverse()` at exactly 0.0 settles `Dismissed` where it
  settled `Completed`. `forward()` always settles `Completed` and `reverse()` always `Dismissed`,
  whatever the start value (it can lie outside 0..=1 after `spring_to`); `animate_to` settles by
  direction of travel.
- **Behaviour change:** `PaintCtx::frame_time()` returns the scaled animation clock (the shell clock
  under `set_animation_time_scale`). Identical at the default scale of 1.0; code timing input should
  read `input_time()`.
- **Behaviour change:** `frust::gpu::with_context` keeps its signature but answers the current
  device, which may change between calls. Do not keep device or queue clones across frames, and
  rebuild device-owned resources when `device_generation()` changes.
- **Behaviour change:** the facade's default features go from none to `complex-scripts` (also on
  `frust-text`), which carries dictionary line and word segmentation at about 3 MB per app
  (`docs/PERFORMANCE_BASELINES.md`). Unspaced Thai, Lao, Khmer and Myanmar text wraps at dictionary
  word boundaries and a Chinese/Japanese dictionary drives CJ word motion. Opt out by adding
  `default-features = false` to the facade dependency, which a scaffolded app declares as
  `frust = { package = "frust-ui", ... }` (or to `frust-text` likewise): unspaced text then
  overflows and CJ word motion falls back to per-character stops. Cargo unifies features, so every
  library in the graph must also leave these defaults off; frust's own plugins do.
- **Behaviour change:** default-sized `Text` and `TextInput` follow the OS text scale on Android and
  iOS; explicit sizes do not. Scaffolded Android manifests gain `fontScale` in
  `android:configChanges`, and an existing app should add it to keep state across a font-size
  change.
- **Behaviour change:** `frust_paths::atomic_write` (also `frust::paths::atomic_write`) uses a
  per-call temp file opened with `create_new`, so concurrent writers and same-stem files no longer
  collide and a pre-placed symlink is never followed, and it returns `Unsupported` on the web
  instead of panicking. `data_dir` and `cache_dir` return `None` for an empty or relative `HOME`,
  `APPDATA` or `LOCALAPPDATA`.
- **Behaviour change:** `frust-i18n` negotiation is script-aware, so `zh-TW`, `zh-HK` and `zh-MO` no
  longer resolve to a `zh-Hans` bundle, and a language-only switch keeps its script.
- Untagged text shapes (`locl`) and line-breaks in the app default locale. ja/zh spans (style
  locale, else the default) use Normal line-break strictness, so small kana and the prolonged sound
  mark may start a line while kinsoku still keeps `。`, `、` and `，` off line starts; an explicit
  `TextStyle` locale always wins.
- The font-cache flush runs at the lifecycle `Hidden` edge instead of on pause or focus loss: iOS in
  `frust_did_enter_background` (no longer at `willResignActive`), Android in `native_on_stop` (no
  longer at `onPause`), and desktop when every window is minimized, occluded or suspended (no longer
  on focus loss), with a flush at exit, once per exit, including menu Quit. Web flushes nothing,
  having no persistent cache.
- `SurfaceRenderer::encode` now does the engine's planning, so a frame the engine refuses is skipped
  before the swapchain is acquired, and `encode_us` carries the plan CPU time `submit_us` used to.
- Glyph admission is sticky: a run whose every glyph is already resident is served over the texel
  budget, only a run that would insert is refused, and LRU eviction drains the excess. On iOS the
  `mobile_large` tier starts at 3 GiB of `hw.memsize` (4 GiB elsewhere), so 4 GB iPhones reach it,
  raising the glyph atlas from 16 to 64 MiB and the retained-page budget from 32 to 64 MiB.
- Faux-bold embolden is computed per side from the size-quantized basis at the route key, the
  lowering and the resident plan, so the amount keyed and the amount drawn agree;
  `FontHandle::with_synthesis` clamps skew to ±30 degrees and embolden to at most 0.5 em.
- The per-frame backdrop cap counts only backdrops whose blur region reaches the frame target, so
  one scrolled or translated clear of the frame (a `ScrollView`'s warm paint margin) no longer
  counts; the up-front count can still over-count inside clipped or retained regions.
- The `.notdef` warning names only the face, the script and the cluster count, never user text, and
  fires once per (face, script).
- The Material and shadcn overlay modals compute snap progress through `Detents`, unchanged for
  finite snap points; their fling-velocity constants now equal `DEFAULT_FLING_VELOCITY`.
- Every reduce-motion collapse (navigator including the interactive edge-swipe settle, switcher,
  `AnimatedOpacity`, `card_switcher`) runs for `MotionExtras::reduced_fade` (120 ms by default; 0
  snaps with no frame request) instead of a fixed 120 ms, and the `network_image` fade-in collapses
  to it under reduce-motion.
- An `ExternalPass` registration's `record` is never run concurrently, even with two live surfaces.
- The Android logger's level follows the `debug.frust.log` system property
  (`adb shell setprop debug.frust.log debug`; the `FRUST_LOG` vocabulary, cleared on reboot) instead
  of being fixed at `Info`; unset it stays `Info` in every build.

### Deprecated

- `frust_text::notdef_glyph_count()` is deprecated in favour of `TextContext::notdef_glyph_count()`.
  It still returns the process-wide total, now counted once per line table when its memo is filled
  rather than once per paint.

### Fixed

- A frame the engine refuses no longer freezes Android/Vulkan apps: a dropped un-presented swapchain
  texture leaks its image on wgpu-hal 30.0.1
  ([gfx-rs/wgpu#5723](https://github.com/gfx-rs/wgpu/issues/5723)), so frust no longer drops an
  acquired texture while its swapchain lives. The main surface presents a backstop frame, a refused
  overlay keeps its texture un-presented so its previous frame stays on screen, and the swapchain
  reconfigures after `MAX_TRANSIENT_SKIPS` consecutive acquire timeouts
  (`engine-scheduler-skip-on-escalation`, `wgpu-vulkan-discard-texture-noop`).
- An interactive edge-swipe pop honours `reduce_motion`: it collapses to `ReducedCrossfade` on its
  first paint, the drag drives the alpha, the release settles on the reduced timing, and a
  `PageTransition::Custom` fn is not called.
- The motion-token conversions behind reduce-motion, navigation transitions and the switcher no
  longer panic paint: each uses `Duration::try_from_secs_f64` and falls back on a negative,
  non-finite or too-large value.
- The shadcn modal drops non-finite snap points, as Material already did, which fixes a possible
  drag panic, and a non-finite settle target snaps to the open detent.
- A second surface no longer keeps a stale external-pass binding after `unregister_external_pass`:
  each surface's drain reconciles against the registry and unbinds in its own engine.
- Font-cache files: stale temp files are swept on both load paths (this process's after 10 minutes,
  another process's after 24 hours), every live copy of a path writes through one writer so an older
  snapshot never overwrites a newer one (file format v7 unchanged), and the first failed flush a
  hook sees logs at warn.
- The web shell's page-lifecycle listeners are owned by a guard that removes them on drop, and the
  hide flush is skipped while the web tier has no persistent font cache.
- DirectWrite resolution honours generic families for a generic-only request: `Monospace` resolves
  to Consolas and `Serif` to Times New Roman, where every generic used to resolve to the Segoe UI
  fallback.
- A `Checkbox` / `Radio` with an empty label is exactly the control size tall (20 px) instead of
  taking the host UI font's line height.
- `frust-i18n`: the language-id cache is bounded (256 entries) and the currency formatter cache is
  bounded (64 entries), both approximate LRU.

### Known issues

- An Android app aborts when an accessibility client is active (TalkBack, or a tool that queries the
  accessibility tree such as `uiautomator dump`) and the window then changes. This predates 0.6.6.
  The fix is an upstream change to `accesskit_android` plus a pin bump, and there is no workaround in
  app code. Fixed after 0.6.6: see Unreleased.

## 0.6.5 — 2026-11-02

A patch release (the 0.6.x line continues) whose one source-affecting change is the removal of the
public fn `frust_engine::gpu::targets::atlas_page_fits` (the Changed list gives the migration).
Everything else is additive, a fix, or a deprecation.

It adds the paged container `page_view` with `PageController` and `PageSnap` physics, the live-card
switcher pattern `motion::card_switcher` with its Glyph skin plus Glyph `fab` and `page_dots`, the
pinch vocabulary through the facade, and reduce-motion handling in `AnimatedScale` and
`AnimatedOpacity`. The render-side `TierCaps` name is now `EngineCaps`; the old name is a deprecated
alias that is removed in 0.7.

### Added

- frust facade re-exports the pinch vocabulary (`pinch_detector`, `PinchDetectorView`,
  `PinchDetectorWidget`, `PinchRecognizer`, `PINCH_SLOP`) at the crate root, and the scale/contact
  event types (`ScaleEvent`, `ScalePhase`, `PointerId`, `PointerSource`) both at the crate root and
  under `frust::authoring`. The pinch names are not re-exported under `frust::authoring`.
- `frust_glyph::fab`, a Glyph `FloatingActionButton`-shaped action button that fits
  `scaffold(..).fab(..)`. It has `.mini(..)`, `.tooltip(..)` (the accessible name) and `.disabled(..)`
  variants and rests on the new `floating()` elevation token, which sits between elevation levels
  3 and 4; a standalone `card_stage()` token covers the switcher stage.
- `frust_glyph::page_dots`, a page indicator for `page_view` that takes a page count and the
  current index.
- `frust_glyph::card_switcher` with `switcher_bar` and `switcher_chip`, the Glyph skin over the
  live-card switcher pattern, including an Android back affordance (`SwitcherBack`).
- `frust::motion::card_switcher` (`CardSwitcherView`, `peer_card` / `PeerCard`), a live-card
  switcher pattern. The page shrinks to a card by scaling about the stage centre, with hit testing
  following the transformed card, then takes a rounded clip, a shadow and a scrim, and blocks input
  beneath it. A peer strip snaps
  between cards, and an expanding phase returns to the full page. Under
  `theme.motion.reduce_motion` the transitions collapse.
- `frust::PageSnap`, a port of Flutter's `PageScrollPhysics` on the `ScrollPhysics` seam
  (`PageSnap::fraction(..)` / `PageSnap::extent(..)`, chainable with `Bouncing` and the other
  physics).
- `frust::page_view` with `PageView` and `PageController`, a paged container.
- `frust-shell-common`'s `build_tree_update` and `tree_update_from_parts`: the one AccessKit tree
  assembly the desktop, iOS and Android shells share.

### Changed

- **Breaking (narrow):** `frust_engine::gpu::targets::atlas_page_fits` is removed, because
  frust-engine no longer calls it. Migration: compare the page size with the adapter limit
  yourself, `page.0 > 0 && page.1 > 0 && page.0 <= caps.max_texture_dimension_2d && page.1 <=
  caps.max_texture_dimension_2d` (`caps: &frust_gpu::TierCaps`); `max_texture_size(caps)` in the same
  module gives the engine's own bound.

### Deprecated

- `frust_render::TierCaps` is renamed `EngineCaps`. The old name stays as a deprecated type alias
  through 0.6 and is removed in 0.7. Migration: `use frust_render::EngineCaps;`.
  `frust_gpu::TierCaps`, the adapter-probed type, is unchanged.

### Fixed

- `frust::motion::AnimatedScale` and `AnimatedOpacity` now honour `theme.motion.reduce_motion`:
  `AnimatedOpacity` fades over 120 ms linear and `AnimatedScale` snaps to its target on the first
  paint, each overriding any explicit timing, matching the navigator and `PatternSwitcher`.

- `frust-camera`'s barcode decoder no longer drops a QR code whose payload is not UTF-8 (a
  byte-mode code carrying binary data or Latin-1 text): it is reported with the exact bytes in
  `Barcode::raw_bytes` and a lossy `raw_value`. `DetectionPolicy::NoDuplicates` tells such payloads
  apart by their bytes.
- frust-engine: a dashed stroke whose dash count is unbounded (a tiny period over a huge path) now
  compiles in bounded time. Only the stretch near the viewport is dashed, with the phase carried
  across the skipped length. Runs whose device dash period is below half a pixel, or that pass the
  per-element budget of 65,536 periods, are stroked solid. Oversize strokes keep their outline
  within 0.25 px under rotated and anisotropic transforms (#141).
- Android accessibility: the TalkBack focus rectangles now come from the shared AccessKit tree
  assembly, whose root carries the frame density so node bounds are published in physical pixels,
  and the tree is re-published when the density changes (#142).

## 0.6.4 — 2026-10-28

A patch release (the 0.6.x line continues) whose source-affecting changes are three: Android `minSdk`
is 29 (Breaking), `TextStyle` gained the public fields `width`, `variations` and `locale`, and
`DesktopConfig` gained `decorations` and `title_buttons` (the Changed list gives each struct-literal
migration). Everything else is additive or internal.

The text stack is frust's own (a font oracle, a shaping driver over `parley_engine`, frust's line
layout and editor), the app has a Flutter-parity lifecycle and a `frust-connectivity` plugin, desktop
apps can draw their own title bar, `frust-i18n` moves to CLDR 49 with a localized Material strings
bridge, and the wasm32 browser boot no longer panics on a clock read.

### Breaking

- Android `minSdk` is now 29 (Android 10), up from 26, in the embedding, every plugin module, the
  scaffold template and the example apps. An app scaffolded earlier must set `minSdk = 29` in
  `app/build.gradle.kts` (or its manifest) or the manifest merger fails. The text stack needs the
  NDK font API (`AFontMatcher_match`, API 29+) instead of parsing the `fonts.xml` that Android 15
  deprecates.

### Added

- The Flutter-parity app lifecycle: `frust::AppLifecycleState { Detached, Resumed, Inactive, Hidden,
  Paused }`, `frust::AppLifecycleListener` (`on_show`, `on_resume`, `on_hide`, `on_inactive`,
  `on_pause`, `on_restart`, `on_detach`, `on_state_change`, `on_exit_requested`) with the intermediate
  steps generated as in Flutter, `frust::AppExitResponse` and `frust::app_lifecycle()`. The Android
  shell pushes the state from new `onStart`/`onStop`/`onWindowFocusChanged`/`onDestroy` hooks, the iOS
  shell from new `didEnterBackground`/`willEnterForeground` observers (additive C exports
  `frust_did_enter_background` and `frust_will_enter_foreground` in `frust_ffi.h`), and the desktop and
  web shells from their window state. Desktop window close and the macOS and Windows menu Quit ask
  `on_exit_requested` first. The macOS app-menu Quit is now a frust item (`frust.app.quit`) instead of
  AppKit's predefined `terminate:`, and it asks the same way.
- The new `frust-connectivity` plugin crate: `Connectivity::set_listener` and `Connectivity::current`
  report the default network as `ConnectivityEvent::{Available { path, initial }, Lost}`, with
  `NetworkPath`, `NetworkId` and `Transport`. Android uses the default-network callback (its
  `ACCESS_NETWORK_STATE` permission comes from the plugin's manifest), iOS and macOS use
  Network.framework, and other platforms are a no-op. The TUI Add Plugin picker lists `connectivity`.
- `frust-i18n` on CLDR 49: `th`, `th-TH` and `en-TH` keep the Buddhist-era default (pin Gregorian
  per call with `DateTimeOptions { calendar: Some(Calendar::Gregorian), .. }`); JPY renders `JP¥`
  in fr-FR/es-ES, `¥` in en-US and `￥` in ja-JP; Vietnamese hours read `tiếng`. `fmt::bridge::Cache`
  is one bounded LRU shared with the currency formatters; a failed currency-fraction data load warns
  once and `fraction_fallbacks()` counts every fallback to two digits; a test fails naming any baked
  currency whose rounding is not R1; English `km`/`m` at Long width stay short
  (`i18n-long-unit-width-short`).
- Text-hardening phase 4: macOS oracle tests and the system emoji-fallback test check their real
  precondition and print a counted skip instead of returning early; a bundled COLRv1 emoji subset
  exercises emoji fallback on every host. The Linux T400 golden class grows from 33 to 70 pairs
  (`page-material-home`'s escalation row now names the re-measured 183, no margin). Optional
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
- The registry lists `frust-material-i18n` as its twenty-fourth plugin entry in the Add Plugin picker.
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
- `frust-engine`: `SceneCompiler::set_synthetic_oblique`, a runtime switch for faux oblique, off by
  default (an italic request on a face with no italic stays upright). `FontHandle` carries the
  shaped synthesis (embolden em fraction and skew degrees) that `frust-text` sets from the shaped
  face.
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
- `DesktopConfig` gained two public fields, `decorations` and `title_buttons`. Builder construction
  is unaffected, and the hand-written `Default` behaves as before. Migration: a struct literal adds
  `..DesktopConfig::default()` (or the builders `with_decorations` and `with_title_buttons`).
- `TextStyle` (re-exported through `frust::authoring::text`) gained three public fields: `width:
  f32`, `variations: Vec<([u8; 4], f32)>` and `locale: Option<String>`. Migration: a struct
  literal adds `..TextStyle::default()` (or starts from `TextStyle::new(size, color)` and chains
  `with_width`, `with_variation`, `with_variations` and `with_locale`); an exhaustive destructure
  adds the fields.
- Native ellipsis cuts at grapheme boundaries, and an ellipsized last line refills from the text
  before the cut (a URL that wrapped early no longer ends at `https://…` far short of the bound).
- `TextEditor` floors the selection to a char boundary after a whole-text replacement and drops a
  stale word-selection base; `TextInput` defers a rebuild caused by a locale change while an IME
  composition is active.
- The app template's `.cargo/config.toml` links the Android targets with
  `--pack-dyn-relocs=relr` and `--use-android-relr-tags` (was `=android`). An existing app may copy
  the rows; the Android-private tags let API 28/29 bionic apply the relative relocations.
- The text shaping engine is `parley_engine` 0.12 with `fontique` 0.12 (no system font scan; the
  oracle owns the OS backends), pinned exactly in the root `Cargo.toml`, with ICU4X held on 2.4
  and `icu_experimental` 0.7.0, replacing `parley` 0.11.
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

- The wasm32 browser boot no longer panics with "time not implemented on this platform":
  `frust-text`, `frust-render`, `frust-engine`, `frust-gpu` and `frust-shell-common` read time
  through `web_time::Instant` (a plain re-export of std, so zero-cost on native). New gates:
  `scripts/ci/wasm-time-check.sh` (a clippy tripwire against std clock reads in the wasm32 browser
  graph) and `scripts/ci/web-smoke.sh` (a headless-Chrome boot of the web gallery; the `ci.yml` job
  `web-smoke`).

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
- Text hardening, phase 2: a `TextInput` IME `Commit` leaves the caret at the end of the committed
  text; the glyph, material, beui, shadcn and native-widgets plugin widgets re-face cached
  `TextLayout`s on a runtime locale switch (generation-stamped, explicit-locale styles exempt);
  faces registered through `register_fonts` carry a PostScript name and hidden flag, and the `hvgl`
  route refuses a face whose bytes disagree with the CoreText face (`bytes_mismatches`); the font
  oracle pins one Han preference per resolve, stamps the Linux cache with the process locale,
  counts and reaps failed or stale cache writes and loads the persisted copy once per process
  (debug builds of an unconfigured context honour `FRUST_TEXT_FONT_CACHE`, `off` or an absolute
  directory); the shells flush the font cache on background transitions (`frust-shell-common::locale_default`).
  Retires the `text-runtime-locale-switch-plugin-layouts` and `text-registered-hvgl-face-skipped`
  limitations.

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
