# Changelog

All notable changes to Frust are recorded here, newest first, in the
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) shape. GitHub release notes are copied
from the section for the tag (see [docs/RELEASING.md](docs/RELEASING.md)).

## Unreleased

### Breaking

- `Router::go` and `RouteNavigator::go` now make the stack exactly the matched chain, as their docs
  said and as go_router defines `go`. They used to replace only the top page, so pages below it
  survived. To keep a page below a shell or another page, make it a parent in the route table:
  `Route::new("/", ..).child(shell_route(..))`. A hand-written pop to the root before `go` can go.
  What follows from it:
  - A page whose matched path is unchanged keeps its state and takes the new params, rather than
    rebuilding. A shell page survives any `go` whose chain crosses it.
  - A warm deep link replaces the stack, since `handle_location` is a `go`, and one matching no route
    makes it one error page. An app receiving links no route is for, such as an OAuth callback delivered
    as a deep link, declares a route for them or handles them before `track()`.
  - `go` closes route-less overlays, including a non-dismissable one over a page it keeps, whether
    the `go` came from app code or a link. A security gate must be a route redirect.
  - A parent page in a matched chain publishes its own path in `route_stack`, not the leaf's location.
    A `push` placing a shell from its parent page stacks only the shell.
  - A removed page's `on_result` fires once with an empty result, on the `go`'s own frame.
  - Only the top page animates, with its route's transition or the navigator's default. Removed pages
    stay alive until the transition settles, though depth and back already report the new stack.
  - A `go` applied in a navigator's first build starts no transition, so a cold start shows its page
    at once instead of animating from the root page.
  - When several `go` calls land in one frame, the last one wins and the earlier pages are not built.
- `frust-shell-common`'s non-default `gpu` feature: `install_gpu_handle` and `gpu_handle` are
  removed. A shell now publishes through an RAII `GpuPublisher` into a replaceable, generation-tagged
  slot, read with `with_gpu_handle` and `gpu_handle_generation`. Only in-repo consumers were affected.

### Added

- `frust-database`: `Database::schema_version`, `Database::migrate(&[Migration])` (one transaction per step,
  `PRAGMA user_version` bumped inside it, identical on both engines) and `Database::execute_batch` (many
  statements in one transaction); a new `DatabaseError::Migration` variant.
- A memory-pressure event: `MemoryPressure` (`Moderate`, `Critical`),
  `AppLifecycleListener::on_memory_pressure`, shell-facing `push_memory_pressure` and the diagnostic
  `last_memory_pressure()`, independent of `AppLifecycleState`. iOS reports its memory warning as
  `Critical`; Android maps `onTrimMemory` levels (RUNNING_MODERATE, RUNNING_LOW, BACKGROUND, MODERATE
  to `Moderate`; RUNNING_CRITICAL, COMPLETE to `Critical`) and trims the render caches on `Critical`,
  which was a no-op before; macOS reports a libdispatch memory-pressure source (warn to `Moderate`,
  critical to `Critical`) without trimming. Windows, Linux and web report none.
- `SceneTextureHints` (`filter`: `SceneTextureFilter::{Linear, Nearest, NearestWhenMagnified}`, and
  a caller-asserted `opaque`) for externally bound textures: `EngineRenderer::bind_texture_with` and
  `ExternalFrame::bind_texture_with`, re-exported from `frust-engine`, `frust-render` and `frust::gpu`.
  `Linear` shares the image sampling resolver, and an `opaque` texture joins the depth-writing opaque
  pass like an opaque image. Unhinted textures behave as before.
- Texture bus: `SourceFormat::Bgra8` (premultiplied; swizzled on the CPU on GL only) and sub-rect
  updates through `SourceFrame::with_update_region` / `UpdateRegion` (re-exported as
  `frust::texture::UpdateRegion`), refused with a one-time warning when no same-size, same-format
  texture is shown.
- `DeviceHandle::generation()` (process-wide, monotonic, non-zero, shared by clones) and
  `DeviceHandle::is_lost()` (latched by the device-lost callback, never cleared); `frust::gpu::device_generation()`.
- `frust::gpu::request_optional_features`/`requested_optional_features` (and
  `frust_render::missing_requested_features`): a process-wide, adapter-conditioned union of optional
  wgpu features, honoured by the shells and the headless renderer. A missing feature is left out; none
  can be required, and limits are not opt-in.
- `app!(Root, gpu_features = { .. }, ..)` (facade `gpu` feature) requests optional features before
  the device is built on desktop, web, Android and iOS (`@pre_init` on `android_app!`/`ios_app!`). On
  Android a pre-init device lacking a requested feature is rebuilt, forfeiting that launch overlap.
- Android and iOS publish their live device to `frust::gpu::with_context`, as desktop does.
- Bitmap-strike colour emoji render on the engine: bitmap-first faces (sbix without outlines, CBDT)
  bypass glifo for an engine-side route, drawn by CoreText on iOS and decoded
  from PNG/dupe/flip records elsewhere, each strike charged and refunded its exact atlas slot, strikes
  over 512 px refused everywhere. Faces with outlines draw outlines. BGRA and mask strikes still do not draw.
- `GestureDetector` press callbacks `on_tap_down`, `on_tap_up` (before `on_tap`) and `on_tap_cancel`,
  with `#[non_exhaustive]` `Copy` `TapDownDetails`/`TapUpDetails` (`local_position`) from `frust-ui`.
  Down fires at once; a slop or bounds exit, a stolen or system cancel, or a second Down cancels.
  Exactly one up or cancel follows each down; leaving the bounds cancels but a later `on_tap` fires.
- `complex-scripts` cargo feature on `frust-text` and `frust-ui`, default on: it carries the
  dictionary line and word segmentation (about 3 MB per app). An app opts out with
  `frust-ui = { version = "...", default-features = false }` (or `frust-text` likewise); unspaced
  Thai/Lao/Khmer/Myanmar text then overflows a line instead of wrapping, and Chinese/Japanese word
  motion falls back to per-character stops. Cargo unifies features across the graph, so every
  library in the app's dependency graph must also leave `frust-ui`/`frust-text` default features
  off; frust's own plugins do.
- `TextStyle` `with_<field>` builders for every field (`with_family`, `with_weight`, `with_style`,
  `with_width`, `with_size`, `with_color`, `with_letter_spacing`, `with_line_height`,
  `with_align`, `with_overflow_wrap`, `with_locale`, `with_variation(s)`), chained from
  `TextStyle::new(size, color)` or `Default`.
- `glass_surface(child)` (`GlassSurfaceView`, flat from `frust`): a baseline widget that paints one
  tier of the theme's glass scale behind its child (shadow cast outside the surface, rounded clip, backdrop blur and washes,
  child, specular hairline). `.tier(GlassTier)` picks `Control`, `Bar` (default) or `Chrome` from the
  new `GlassScale::tier(GlassTier) -> &GlassMaterial`; `.corner_radius(f64)` (clamped to a capsule, half the shorter side, like `ShapeScale::resolve`:
  an infinite radius is a capsule, a NaN or negative one square) and `.material(..)`
  (wins over the theme's tier) refine it, and with neither a theme nor a material it paints an
  opaque `#EFEFEF` fill. A material's blur intent is a Gaussian sigma in logical px (CSS
  `backdrop-filter: blur(Npx)` is sigma N), clamped by the engine. Under a scale the sigma follows
  the transform's mean scale; a rotation or skew samples the rotated rectangle's device bounding box.
- `set_max_backdrops_per_frame(n) -> usize` and `max_backdrops_per_frame()` (flat from `frust` and
  `frust-render`; defined in `frust-engine`, with `MAX_BACKDROPS_PER_FRAME` 4 as the default and
  `BACKDROP_CAP_CEILING` 16): the per-frame backdrop-blur cap is app-configurable, process-wide and
  clamped to `1..=16`. A frame recording more backdrops than the cap is still refused whole
  (`EngineError::TooManyBackdrops`), and `SceneCompiler::compile_with_backdrop_cap` compiles against
  an explicit cap.
- Stroke caps, joins and miter limit, and the even-odd fill rule, in `frust-scene` (also in
  `frust::authoring::scene`): `StrokeCap` (`Butt`, `Round`, `Square`), `StrokeJoin` (`Miter`,
  `Round`, `Bevel`), `StrokeStyle { cap, join, miter_limit }` (`ROUND`, `new`, `with_miter_limit`)
  and `FillRule` (`NonZero`, `EvenOdd`), carried by `PathStyle::FillEvenOdd` and
  `PathStyle::StrokeStyled { width, dash, style }` (helpers `PathStyle::fill_rule()` / `stroke()`).
  `SceneBuilder` and `PaintScene` gain `fill_path_with_rule` and `stroke_path_styled`; the
  `PaintScene` defaults fall back to `fill_path` / `stroke_path(_dashed)`. The engine and the CPU
  oracle render both, and a non-finite or below-1 miter limit is refused as `InvalidGeometry`.
- `frust::motion::detent` (flat in `frust::motion`): `Detents`, pure NaN-safe snap-point math
  (`open_progress`, `drag_to`, `release_target`, `settle` returning `DetentRelease` under a
  `DismissPolicy`), and the headless `DetentController` (interruptible drag; a release hands the
  fling velocity to `spring_to_with_velocity`; `with_fling_velocity(f64)` sets the fling speed,
  defaulting to `DEFAULT_FLING_VELOCITY`). `DismissPolicy::SpringBack` snaps back to open; on
  `Dismiss` the controller stops and holds progress, and the caller owns the exit animation.
- Stroked icons: `IconShape` (`Path`/`Circle`/`Rect`), `IconPaint` (`Stroke` default, `Fill`),
  `IconElement` (const `stroke`/`fill`, with `opacity`, a single `dash` pattern and its own
  `stroke_width` through `with_opacity` / `with_dash` / `with_stroke_width`) and
  `StrokedIconSource { elements, design, stroke_width }`,
  converted to `IconData` by `From<StrokedIconSource>` or `IconData::from_elements`;
  `IconData::is_stroked()` reports one and `IconData::resolve()` returns one fillable outline (each stroke
  expanded with round caps and joins), so fill-only slots (plugin icons) render a stroked icon filled and
  ignore `stroke_width` overrides. `IconView::stroke_width(f64)` overrides the width;
  strokes use round caps and joins, scale with the icon and take the resolved icon colour.
  `IconElementStyle` carries an element's paint and those attributes for runtime-built icons
  (`IconData::from_styled_elements`); `resolve()` bakes an element's dash and width into the
  outline and drops opacity.
- Framework haptic feedback in `frust-core` (flat from `frust` and in `frust::authoring`):
  `#[non_exhaustive] HapticFeedback` (`Selection`, `Light`, `Medium`, `Heavy`, `Success`,
  `Warning`, `Error`), `HapticSink`, the last-wins `set_haptic_sink`, `haptic_sink_installed()`
  and `perform_haptic()` (a no-op without a sink). The `frust-haptics` README shows wiring it as
  the sink.
- Animation time scale: `set_animation_time_scale(f64) -> bool` (accepts
  `MIN_ANIMATION_TIME_SCALE` 0.01 ..= `MAX_ANIMATION_TIME_SCALE` 100.0, rejects anything else) and
  `animation_time_scale()`. `PaintCtx::frame_time()` returns the scaled animation clock and the new
  `PaintCtx::input_time()` the raw shell clock; `frust-widgets` long-press, hold, caret blink,
  double-tap windows, velocity timestamps, drag hold and auto-scroll read `input_time()`.
- Devtools: the `set_animation_time_scale` method (params `AnimationTimeScaleParams { scale }`,
  authenticated), the `animation_time_scale` capability and
  `DevtoolsClient::set_animation_time_scale`. The protocol stays v1: the change is additive and an
  older client decodes the capability as `Unknown`. A scale a client set is reset to 1.0 when that
  client disconnects.
- `frust-ui` `test-support` feature (enable from `[dev-dependencies]`) and
  `frust::testing::semantics_of(build, size) -> SemanticsUpdate`, so an out-of-tree design system
  can test `Widget::semantics`. `scene_of(build, size)` and `scene_of_with(build, size, &SceneOptions)`
  (`SceneOptions::theme` / `frame_time` / `warm_frames`) paint a view into a `Scene` a test can
  inspect; a recorded `BackdropBlur` count is an upper bound on what counts toward the backdrop cap.
  The facade's `test-support` now forwards `frust-core/test-support`, so `PaintCtx::for_test` is
  available too.
- `column()` / `row()` gain `.gap(f64)`, a fixed space between consecutive children (negative or
  NaN is `0`), and `MainAxisAlignment` gains `Center`, `End`, `SpaceBetween`, `SpaceAround` and
  `SpaceEvenly`; they distribute leftover space only on a bounded main axis with no flexible child.
- `Route::transition(TransitionSpec)` gives a route's pages their own transition, and
  `Router::push_with(location, PushOptions)` pushes with per-push options. Precedence: the push
  options, then the route, then the navigator default; all collapse under reduce motion.
  `push_with` stamps the resolved location over any `.route(..)` on the options, and on a resolution
  error pushes the error page and ignores them.
- `PushOptions::barrier(Color)` and `barrier_dismissible(bool)`: a route-owned modal scrim painted
  under the page. Only its alpha animates, following the route's eased progress (instant under
  `TransitionSpec::NONE`); a press and release the page leaves unhandled dismisses through the
  page's `BackPolicy` unless the press lands on content (layout wrappers such as `Padding`,
  `SafeArea` and `Align` are transparent), and taps are ignored while a transition is in flight.
- `Widget::hit_test_self` (provided, default `true`): `false` makes a widget transparent to pointer
  hits beside its children; layout wrappers, and a `Container` that paints nothing, return `false`.
- `GestureDetector` drag callbacks: `on_{vertical_drag,horizontal_drag,pan}_{start,update,end,cancel}`
  with the `#[non_exhaustive]`, local-only `DragStartDetails` / `DragUpdateDetails` /
  `DragEndDetails`. Axis recognizers start after 18 px along their axis, pan after 36 px; the
  first to cross wins, and a same-axis scroll view takes the drag first. Release velocity is
  measured on the raw input clock. `cancel` is delivered on the next state-bearing pass (normally
  the next rebuild's housekeeping), not inside the `Cancel` event.
- `SharedPreferences::in_memory()`: a public, infallible, process-local store that is never
  persisted; clones share the map.
- `frust::Layer` (with `Layer::IDENTITY`) and `frust::resolve_layers`, beside `TransitionDriver`,
  `make_driver` and `resolve_spec`, so `PageTransition::Custom` is authorable out of tree.
- `frust::authoring::text::LocaleStamp` (new public API, also `frust_text::LocaleStamp`):
  `for_style(explicit_locale)` / `is_current(explicit_locale)` stamp a cached layout with the
  default-locale generation (constant for a style with its own locale); read it before shaping and
  store it with the layout (464fe55aa). `frust-widgets` `Text`/`TextInput` and the glyph, material,
  beui, shadcn and native-widgets layout caches follow that rule (4c1f2b8ee, d86d95ff1, 0d39ac7a1,
  f897dd659, b0fec455f).
- `test-support` features on `frust-text` and `frust-widgets` expose the shared
  `LocaleSlotGuard` (`frust_text::test_support`, `frust_widgets::test_support`) over the
  default-locale slot; it replaces the plugins' private locale test locks.
- `TextContext::notdef_glyph_count()`: `.notdef` glyphs counted per context across retained
  layouts, the shape-cache path and the editor (17e18cd45). `TextLayout` is now `Sync`.
- `EngineRenderer::prepare` (every plan-stage refusal) and `encode_prepared` / `encode_prepared_traced`
  (record a prepared frame; refuse only `AlphaCapacity` / `PaintCapacity` when the held plan matches, otherwise re-plan and can surface any `prepare` error), plus the public helper
  `RegionCopyMode::of_usage`. `SurfaceRenderer::engine_refusals()` returns the `#[non_exhaustive]`
  `EngineRefusalReport { count, first_reason }`. The `FRUST_RENDER_INJECT_REFUSAL` fault-injection knob
  (`plan:N` / `record:N`, compile-time or runtime, honoured in release builds) refuses every Nth frame
  at that stage. `frust_gpu::lifecycle::MAX_TRANSIENT_SKIPS` (3) bounds a run of acquire
  `Timeout`/`Occluded` skips before the swapchain is reconfigured.
- `EngineRenderer::set_synthetic_oblique` / `synthetic_oblique` and `set_glyph_run_cache` /
  `glyph_run_cache` forward the faux-oblique and resident-glyph-run-cache switches on the renderer,
  matching `SceneCompiler`.
- `frust_bench` gains the `weight_probe` scenario (faux-bold and faux-oblique rows on a static Noto
  Sans subset). On a Pixel 5 (unhinted tier) weight 400 draws 15955 ink px and weight 700 draws 22300
  (+39.8%); the italic rows stay upright with faux oblique off. Three PingFang SC Han rows
  (weights 300/400/600) sit below them: on an iPhone 13 mini (iOS 27.2, `mobile_large`) they draw
  24,812 / 34,416 / 48,085 ink px with no refused, unverified or unavailable warning.
- `skrifa` is a dev-dependency of `frust-engine` (test-only, from the root `[workspace.dependencies]`),
  used by a probe of whether `glifo`'s hinting instance can be built per face.
- hvgl-only faces (Apple's PingFang) now draw from the glyph atlas: `text/hvgl_atlas.rs` inserts
  them frust-side, `OutlineReason::Hvgl` is the fallback, `SceneCompiler::set_hvgl_atlas` /
  `EngineRenderer::set_hvgl_atlas` switch the route (default on), and `CompiledFrame` gains
  `hvgl_atlas_draws` and `hvgl_outline_fallbacks`. The `frust-perf enc` line appends both counts, then
  `hvgl_variation_mismatches` and `hvgl_variation_unverified` (cumulative) and `hvgl_unavailable`
  (per-frame median).
  The route restates `glifo` 0.3.0 `renderer.rs:591`/`:342`/`:521`/`:311`, so
  `tests/hvgl_atlas_parity.rs` is the tripwire on a `glifo` bump (`engine-hvgl-atlas-pin-bound`).
  iPhone 13 mini, S6 at 3x: PingFang 61 atlas draws and 0 outline fallbacks per window, `glyphs_us`
  5,377 -> 183, CPU work per frame 6,146 -> 1,437 us, presented cadence unchanged
  (p2p 16.68 ms; gates ha04-13mini-s6-phase-head, gr02-13mini-presented-cadence); Apple M4
  Metal, a 300-glyph PingFang paragraph compiles in 9.7 us, was 613 (ha04-m4-metal). Merged in f5f360e70,
  a3b5ebd5b.
- `frust_render::set_reported_device_memory`: a shell reports device memory where there is no
  `/proc/meminfo`. `frust-shell-ios` reads `sysctl hw.memsize` and calls it before the first renderer (3bab21765).
- iOS answers `didReceiveMemoryWarning`: `frust_memory_warning` sends `RenderCommand::TrimMemory`
  (fire-and-forget, serviced in every phase), `SurfaceRenderer::trim_memory` and
  `EngineRenderer::trim_memory` hold the retained pages to the plain 32 MiB budget and clamp atlas
  growth to the plain tier's bytes (`ImageResidency::request_growth_ceiling`; the atlas has no
  layer-release primitive), one-way until the renderer is rebuilt. Android and desktop ignore the
  command. Host-tested; on-device delivery was not exercised (rf01-13mini-memory-warning, skipped).
- `frust-perf ios-mem phys_footprint_kib=<n>|unknown`, about 1 Hz on iOS under `perf-trace`, and the
  compile-time `FRUST_FORCE_MOBILE_TIER` define (`frust build ios --define FRUST_FORCE_MOBILE_TIER=1`)
  that forces the plain tier. iPhone 13 mini, S6, one session: `phys_footprint` p50 229.9 MiB plain vs
  252.0 MiB `mobile_large` (+22 MiB), 527/527 vs 137/527 resident glyphs (rf01-13mini-footprint-ab,
  p2-round1-integration-root).
- `frust-testing`'s `dup_identities` gains `hvgl_restatement_is_attested_for_the_locked_glifo`: a
  `glifo` bump fails on ubuntu CI until `HVGL_PARITY_VERIFIED_GLIFO` acknowledges the macOS parity
  re-run (an attestation, not a run). `hvgl_atlas.rs` factors its outline and glyph-rect transforms
  into pure helpers with 11 golden vectors.
- hvgl diagnostics: per-fetch outline-cache counters `SceneCompiler::hvgl_outline_hits` /
  `hvgl_outline_misses` and the `CompiledFrame` fields of the same names (an atlas-route glyph absent
  from the cache is fetched twice, measured then rasterized, so it reads one miss and one hit; an
  inkless glyph is only measured; an atlas-resident glyph never fetches; a fetch at a refused
  variation instance asks CoreText for nothing and counts as neither), and a per-frame
  `CompiledFrame::hvgl_unavailable_glyphs` on both draw routes with the cumulative
  `SceneCompiler::hvgl_unavailable_glyphs`. The first report in a process is a `log::warn` naming the
  face (escaped), the count and `skipped_glyphs`; later ones log at debug (5736ac67a, bc67cdb0b).
- hvgl faces are checked against CoreText rather than trusted. Each variation copy is read back
  (`CTFontCopyVariation`) and each axis asked is classified: applied, contradicted (reported beyond a
  single F2Dot14 step of the axis range, a clamp included) or unreported. Only a contradicted axis
  refuses the instance: counted by the new `SceneCompiler::hvgl_variation_mismatches`, never cached,
  its glyphs unavailable. Unreported axes draw CoreText's copy, unverified, counted by the new public
  `SceneCompiler::hvgl_variation_unverified()`. Each kind warns once per open face. Off Apple every
  glyph of an hvgl-classified run counts in `hvgl_unavailable_glyphs`. Opening a face also compares a 16-range `cmap` sample of
  its bytes with CoreText; a mismatch counts in `hvgl_bytes_mismatches` and the warning names the
  codepoint as `U+XXXX`. `objc2-core-foundation` gains the `CFArray`/`CFDictionary`/`CFNumber`
  features, no version change (265aaa068).
- `TextContext::font_cache_stats()` returns `FontCacheStats { write_failures, written_generation, dirty,
  attached, persistent }`, and `BackgroundFlush::went_to_background_outcome()` returns a `BackgroundFlushOutcome`
  (per-context results; `went_to_background` still returns the flushed count). Both types are
  re-exported (`frust_text`, `frust_shell_common`, `frust::authoring::text`). A report whose every flush
  failed leaves the once-per-transition latch unarmed so the next hide retries; the first failed flush a
  hook sees logs at warn, later ones at debug (1f7cf69bf, d4adce3f3).
- `AnimationController::velocity()` and the velocity-carrying retarget `spring_to(target, SpringDesc)` /
  `spring_to_with_velocity(target, velocity, SpringDesc)` in `frust-core`, so any controller user gets
  velocity continuity (a fling or release velocity carries into the spring).
- `frust-theme` `MotionExtras { extended: 800.0, reduced_fade: 120.0 }` (milliseconds; read with
  `MotionExtras::of(theme)` / `reduced_fade_duration()`; flat as `frust::MotionExtras`) and
  `resolve_spec_with(spec, scheme, reduced_fade)`. `MotionDurations` and `MotionScheme` are unchanged.
- `frust::motion::presence`: `Presence` and `presence(open, child)` with `.timing(enter, exit)` and
  `.on_exited`. The child stays mounted: always built and laid out, painted only while visible,
  input only while open, semantics only while visible. Reversing a duration ramp is value-continuous
  and a spring value- and velocity-continuous; `on_exited` is delivered on the next frame's
  Housekeeping event, and only once an exit has painted. Under `reduce_motion` it runs a linear reduced fade.
- With the default-on `complex-scripts` feature, dictionary line and word segmentation
  (`parley_engine`): unspaced
  Thai/Lao/Khmer/Myanmar text wraps and word-moves at dictionary word boundaries, and the
  Chinese/Japanese word dictionary drives CJ word motion. A line overflows only when a run has no
  break opportunity (one long word, a URL) unless `overflow_wrap` says otherwise.
- Untagged text shapes (`locl`) and line-breaks in the app default locale; ja/zh spans (style
  locale, else the default) get the ja/zh line-break tailoring, so small kana and the prolonged
  sound mark may start a line while kinsoku still keeps 。、， off line starts. An explicit
  `TextStyle` locale always wins.
- `OverflowWrap` (`Normal`, `Anywhere`; `frust::authoring::text`), `TextStyle::with_overflow_wrap`
  and `TextView::overflow_wrap`; paragraph-level and part of the shape-cache key.
- `TextView::locale(..)` tags a paragraph without marking color or family explicit, so it keeps
  theme defaults and the OS text-scale rule.
- A bundled Thai test face (`testing/fonts/NotoSansThai-Subset.ttf`, OFL), registered by
  `frust_testing::fonts::test_fonts()`.
- OpenType font features: `TextStyle::with_feature(tag, value)` / `with_features(..)` (a later
  setting of a tag wins), and `TextView::font_feature(s)` and `tabular_figures()` (`tnum`).
  Features are part of the shape-cache key.
- Text decorations: `TextDecoration` (`frust::authoring::text`) on `TextStyle::with_decoration`,
  and `TextView::underline()`, `line_through()` and `decoration(..)`. `TextLayout::decoration_rects`
  places one rectangle per same-style, same-face line segment from the face's run metrics, and
  `TextView` paints them as plain fills with no new scene command. `TextInput` draws none (see
  LIMITATIONS).
- `MonospaceFamily` theme extension (`Theme::monospace_family()`, falling back to the generic
  monospace family): the shadcn, beUI and Glyph themes attach it, and
  `TextView::themed_mono_family()` resolves it.
- `TextInputView::themed_family(role)` and `themed_mono_family()` take a field's content and
  placeholder family from the active theme at layout (the theme's family wins over a `text_style`
  family). The catalogs' text fields use them, so a theme swap re-faces typed text.
- `ExternalFrame::drain_id()` and `device_generation()` identify the surface drain and device a pass
  records for, so a pass can key per-surface, per-device resources.
- `HeadlessRenderer::render_with_passes` (and `render_draining_registry`, which opts into the
  process-wide registry) render external passes offscreen; `EngineOracle` has the same two methods.
- `frust::gpu::headless` re-exports `HeadlessRenderer`, `HeadlessOptions`, `HeadlessSpec`,
  `HeadlessImage`, `HeadlessMeta` and the golden env-var names, so an app can golden-test its own pass.
- The playground gains a "GPU pass" section that drives an `ExternalPass` through the facade only.
- OS text scale on Android and iOS: `frust-theme` gains `TextScale`, `TypeScale::scaled`,
  `Theme::with_text_scale`, `Theme::text_scale`, `Theme::authored_type_scale`, `TextScale::clamped`
  and `scale_style`; default-sized `Text` and `TextInput` follow it, explicit sizes do not.
- Material `zh-Hans` strings, and `frust_i18n::Locale::likely_script`.

### Changed

- `frust::gpu::with_context` keeps its signature but answers the current device, which may change
  between calls: do not keep device or queue clones across frames, and rebuild device-owned
  resources when `device_generation()` changes.
- `MainAxisAlignment` gained `Center`, `End`, `SpaceBetween`, `SpaceAround` and `SpaceEvenly` (a
  narrow source break): an exhaustive `match` over it must add the arms or a wildcard.
- The per-frame backdrop cap counts only backdrops whose blur region reaches the frame target, so a
  backdrop scrolled or translated clear of the frame (a `ScrollView`'s warm paint margin) no longer
  counts. The cap, its clamp and the whole-frame refusal are unchanged; the up-front count ignores
  retained corrections and clip coverage, so it can still over-count inside clipped or retained
  regions.
- `PathStyle` gained `FillEvenOdd` and `StrokeStyled` (a narrow source break): an exhaustive `match`
  over `PathStyle` outside frust must add the two arms, or read through `PathStyle::fill_rule()` /
  `stroke()` instead of matching.
- The Material and shadcn overlay modals compute open progress, the drag clamp, the release target
  and the settle decision through `Detents`; behaviour is unchanged for finite snap points, and their
  public fling-velocity constants now equal `DEFAULT_FLING_VELOCITY`. The shadcn modal now drops
  non-finite snap points, as Material already did, which fixes a possible drag panic, and a
  non-finite settle target snaps to the open detent.
- Catalog tooltips (material, glyph, shadcn), the material selection and toggle-button long-press latches
  and the shadcn OTP caret blink time input on the raw clock (`input_time()`), so they ignore the
  animation time scale.
- `MaterialHaptics` forwards to the framework haptic sink when no Material hook is set
  (`SliderTick` maps to `Selection`, `None` to nothing).
- The `.notdef` warning names only the face, the script and the cluster count, never user text,
  and fires once per (face, script). `frust_text::notdef_glyph_count()` is deprecated in favour of
  `TextContext::notdef_glyph_count()` (it still returns the process-wide total, now counted once per
  line table when its memo is filled rather than once per paint) (17e18cd45).
- Scheduler: `MAX_SPILL_PAGES` (3) spill slots serve a fan nested inside a fan under a held ancestor
  and every tree whose isolated layers hold at most two isolated children; `MAX_LIVE_PAGES` goes
  from 3 to 5, costing up to +128 MiB of intermediates, only on frames that use the extra slots.
  Migration: `frust_engine::schedule::PageParity` gains the pub variant `SpillPool(SpillSlot)` (a
  patch-release source break), so an exhaustive downstream `match` needs a wildcard arm or the new
  `PageParity::is_spill()` / `spill_slot()` (`PageParity::spill(n)` builds one, `None` past `MAX_SPILL_PAGES`).
- `SurfaceRenderer::encode` now does the engine's planning, so a frame the engine refuses is skipped
  before the swapchain is acquired and `encode_us` carries the plan CPU time that `submit_us` used to.
  Frames refused after acquire present the acquired image on the main surface (the previous frame
  re-converted on the straight-alpha arm, a base-colour clear on the direct arm); a refused overlay keeps
  its acquired texture un-presented, so its previous frame stays, at most one frame
  (`engine-scheduler-skip-on-escalation`, `wgpu-vulkan-discard-texture-noop`).
- `FontHandle::with_synthesis` clamps skew to +/-30 degrees and embolden to at most 0.5 em.
- Glyph admission is sticky: a run whose every glyph is already resident is served over the texel
  budget, only a run that would insert is refused, and LRU eviction still drains the excess. iPhone
  13 mini, S6 at 3x: the atlas held 0 of 527 glyphs and thrashed (24,051 evictions per three runs);
  with sticky admission alone it holds 231, and 527 of 527 with the tier change below (5c4fcad87; gate gr02-13mini-residency).
- On iOS the `mobile_large` tier starts at 3 GiB of `hw.memsize` (4 GiB elsewhere): a 4 GB iPhone
  13 mini reports 3674 MiB and previously stayed on the plain tier. The tier raises the glyph atlas
  from 16 to 64 MiB and the retained-page budget from 32 to 64 MiB.
- Faux-bold embolden is computed per side from the size-quantized basis at the route key, the
  lowering and the resident plan, so the amount keyed and the amount drawn are the same bits.
  Sub-quantum scale jitter can still mint `glifo` cache entries, because the size `glifo` keys is not
  quantized.
- Engine tests: `variable_route` uses calibrated magnitude bands, and the italic paragraph compile-cost
  test counts draws instead of reading a wall clock.
- Probe result: every bundled face yields a `skrifa` hinting instance at every engine size; two macOS
  system faces do not, so a hinted faux-bold request on them is nearly invisible
  (`text-hinted-faux-bold-unhintable-face`). Faux bold and faux oblique are not applied on the hvgl
  route (`text-hvgl-synthesis-not-applied`). See [docs/LIMITATIONS.md](docs/LIMITATIONS.md).
- The single-file hvgl provider is now `text/hvgl/{mod,face,outline,ffi}.rs` (internal): the public surface,
  the open-face checks and refusals, the outline and variation-instance caches, and every CoreText
  call, which is now the only `unsafe` in the module. One generic bounded LRU bounds the open faces,
  each face's instances and the outline cache; refusal sets stay first-in-first-out (c0002cfab,
  1d1418e9b).
- The font-cache flush runs at the lifecycle `Hidden` edge instead of on pause or focus loss. iOS
  flushes in `frust_did_enter_background` (no longer at `willResignActive`), Android in `native_on_stop`
  (no longer at `onPause`), desktop when every window is minimized or occluded, or suspended (no longer
  on focus loss), and desktop gains a flush at exit, once per exit, including menu Quit. Web flushes
  nothing: it has no persistent cache. iPhone 13 mini: one flush per background (14.3 ms first write,
  then 0.1 ms), none for a Control Center pull; Pixel 5: one per `onStop` (12.1 ms, then 0.1 ms), none
  for the share sheet, and the cache survives `am force-stop`; macOS M4: minimize 1 (11.2 ms), occlusion 1,
  Quit 1, focus loss 0 (p4-ios-flush-hidden, p4-android-flush-hidden, p4-desktop-flush-hidden,
  p4-web-no-flush; 64281d470).
- `frust-i18n`: `fraction_fallbacks()` is a test seam, not observability; the 0.6.4 note calling it
  observability is superseded. The language-id cache is bounded (256 entries) and the per-locale currency
  formatter cache has a bound test (64); both documented as approximate LRU. Vietnamese relative
  day `hôm nay` is lowercase (f1956dd44).
- The system emoji-fallback test in `crates/frust-text/tests/emoji.rs` now uses the counted-skip
  precondition (`SKIPPED (n)` plus a `SKIP SUMMARY`): a Linux host with no emoji font reports one
  skip, macOS reports none, and the registered-subset test checks the served face by font-data
  identity. The 0.6.4 note claiming a counted emoji skip was not true until now; the macOS CoreText
  han oracle test also holds `LocaleSlotGuard` (4bddadbd8).
- The advisory `text-oracle-macos` / `text-oracle-windows` CI legs (still outside `ci-ok`) now scope
  on `Cargo.lock`, `Cargo.toml`, `rust-toolchain.toml`, `frust-text`, `frust-engine`, `frust-scene`,
  `frust-paths`, `frust-gpu`, `testing/fonts`, `crates/frust-text/tests/fonts` and `ci.yml`; a leg
  that does not run writes `SKIPPED (scope: <reason>)` to the job summary (05b812682).
- A full `cargo test --workspace` on a Windows x64 MSVC host passes: 18 tests in 8 suites were
  fixed. Eleven assumed Unix (path separators, `\\?\` verbatim prefixes, Java-properties colon
  escaping, JSON-escaped paths, case-insensitive environment names, response-file dialect) and now
  build expectations from the product encoders; the Android hot-patch tests that fake an NDK with
  shell scripts and the `xcodebuild` test are `cfg`-gated to the hosts that support them; the
  `frust-hotpatch` `real_patch_library` test expects the by-design zero slide on Windows
  (2f2832c1c, 651d30a1d).
- Every reduce-motion collapse (navigator, including the interactive edge-swipe settle, switcher,
  `AnimatedOpacity` and `card_switcher`) runs for `MotionExtras::reduced_fade` (120 ms by default; 0
  snaps with no frame request) instead of a fixed 120 ms. The `network_image` fade-in therefore
  collapses to that reduced fade under reduce-motion.
- `TextStyle` gains a public field `overflow_wrap`, and is now `#[non_exhaustive]`: build it with
  `TextStyle::new` / `Default` plus the new `with_*` builders (struct literals no longer compile
  outside `frust-text`). ja/zh text now uses Normal line-break strictness (small kana may start a
  line). Release size grows by about 3 MB per app with the segmentation dictionaries (see
  `docs/PERFORMANCE_BASELINES.md`).
- `TextStyle` gains the public fields `features` and `decoration`; since it is
  `#[non_exhaustive]`, no struct-literal migration is needed outside `frust-text`.
- An `ExternalPass` registration's `record` is never run concurrently, even with two live surfaces.
- `ExternalFrame::frame_index` on an explicit headless render (`render_with_passes`) counts that
  renderer's own renders from 0; surfaces and registry drains keep the process-wide count.
- Scaffolded Android manifests gain `fontScale` in `android:configChanges`; existing apps should add
  it to keep state across a font-size change.
- `frust-material-i18n` depends on `frust-reactive`.
- Docs: `clean-signals-frust` shows localized failure text through the helpers' closures.

### Fixed

- The motion-token conversions behind reduce-motion, navigation transitions, the switcher and presence
  no longer panic paint: each uses `Duration::try_from_secs_f64` and falls back on a negative,
  non-finite or too-large value (`MotionExtras::reduced_fade_duration` 120 ms, navigation transitions
  340 ms enter / 150 ms exit, the switcher base 220 ms, presence timing the neutral token).
- `AnimationController::forward()` always settles `Completed` and `reverse()` always `Dismissed`, whatever
  the start value (it can lie outside 0..=1 after `spring_to`); `animate_to` settles by direction of
  travel. Behaviour change: `reverse()` at exactly 0.0 now settles `Dismissed`, where it settled `Completed`.
- `presence`: a callback attached after an exit settled never fires for that exit, a zero-distance duration
  ramp settles immediately with no frame, open-then-close before any paint fires `on_exited` once, and a
  settled child paints without an alpha layer. An exit settles only when painted, so `on_exited` is a
  cleanup cue, not a correctness gate.
- An interactive edge-swipe pop honours `reduce_motion`: it collapses to `ReducedCrossfade` on its
  first paint, the drag drives the alpha, the release settles on the reduced timing, and a
  `PageTransition::Custom` fn is not called.
- A frame the engine refuses no longer freezes Android/Vulkan apps: a dropped un-presented swapchain
  texture leaks its image on wgpu-hal 30.0.1 ([gfx-rs/wgpu#5723](https://github.com/gfx-rs/wgpu/issues/5723)),
  so frust no longer drops an acquired texture while its swapchain lives (the main surface presents a
  backstop frame, a refused overlay keeps the texture un-presented for its next frame, so its previous frame stays on screen) and reconfigures after `MAX_TRANSIENT_SKIPS`
  consecutive acquire timeouts.
- Font-cache file hygiene: stale temp files are swept under the registry lock on both load paths (this
  process's after 10 minutes, another process's after 24 hours, with no PID liveness check), every live copy
  of a path writes through one writer with a monotonic generation so an older snapshot never overwrites a
  newer one (file format v7 unchanged), and a clean flush of a deferred-path context joins a live shared
  copy (734411802, 2791c5134, f69fccfc0).
- The web shell's page-lifecycle listeners are owned by a guard that removes them on drop (no leaked
  closures), and the hide flush is skipped while the web tier has no persistent font cache (74e0f87dc).
- DirectWrite resolution honours generic families for a generic-only request: `Monospace` resolves
  to Consolas and `Serif` to Times New Roman, where every generic used to resolve to the Segoe UI
  fallback (2f2832c1c).
- A `Checkbox` / `Radio` with an empty label is exactly the control size tall (20 px) instead of
  taking the host UI font's line height; this was a golden mismatch on Windows and on the Linux
  T400 host, and the golden is unchanged (2f2832c1c).
- A second surface no longer keeps a stale external-pass binding after `unregister_external_pass`:
  each surface's drain reconciles against the registry and unbinds in its own engine.
- `frust-i18n` negotiation is script-aware, so `zh-TW`/`HK`/`MO` no longer resolve to a `zh-Hans`
  bundle and a language-only switch keeps its script.

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
