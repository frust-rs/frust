# Lab 12 — Clips, layers, snapshots and the clear punch

**Concept:** Lab 4 compiled plain fills into strips. The display list also carries *brackets* —
`PushClip`/`PushClipRounded`, `PushLayer`, `PushSnapshot`, each closed by a `Pop*` — plus one
command that erases rather than paints, `ClearRect`. This lab shows how `SceneCompiler` lowers all
of them without ever allocating an intermediate texture for a clip: a clip is a rectangle the strip
run gets cut against or a coverage mask the strip generator already understands; only a translucent
layer costs a page of its own (lab 13's scheduler); and a clear is hoisted out of every bracket to
the frame root. Everything here is GPU-free — the tests compile a `Scene` and read the strips back.

## Where it lives

| Thing | File | Symbols |
|---|---|---|
| The clip stack, both lowerings | `crates/frust-engine/src/compile/clip.rs` | `ClipStack` (≈106–130), `Entry::{Scissor, Mask}`, `push_rect`, `push_rounded`, `pop`, `clip_run` |
| Rewriting a strip run under a scissor | same | `Clipped` (≈415–476), `spans` (≈478–519), `emit_split_solid`, `emit_masked` |
| Shared admission rule (fill fast path = clip scissor path) | `crates/frust-engine/src/compile/mod.rs` | `fast_rect`, `is_pixel_aligned` |
| One bracket stack; layer lowering; snapshot emulation | `crates/frust-engine/src/compile/layers.rs` | `GroupStack` (≈96–156), `lower_layer`, `LayerLowering`, `layer_props`, `snapshot_correction`, `SnapshotStack` (≈231–325) |
| The hoisted punch | `crates/frust-engine/src/compile/clear.rs` | `ClearPunch` (≈107–130), `StagedPunch` (≈138–144), `punch_rect` (≈155–166), `device_bounds` (≈175–185) |
| The walk's bracket arms and helpers | `crates/frust-engine/src/compile/mod.rs` | `Command::PushClip` … `Command::PopSnapshot` arms (≈1116–1183), `open_layer`, `close_group`, `close_open_groups`, `generate_punches` |
| What the frame reports | same | `CompiledFrame::{clears, scissor_clips, mask_clips, clip_mask_strips, draws}` |
| Up-front refusal | same | `check_geometry` (≈2234) |
| Tests | `crates/frust-engine/tests/clips.rs` (24) · `crates/frust-engine/tests/layers.rs` (25) | plus unit tests in `clip.rs` (≈584–728), `layers.rs` (≈327–484), `clear.rs` (≈187–241) |

## 1. Clips never become a texture

`frust_scene` has one clip stack, and so does the compiler: `ClipStack` holds one `Entry` per
unpopped push, in one of two shapes.

- **`Entry::Scissor`** — `push_rect` asks `fast_rect(rect, transform)`, the *same* function that
  admits a fill to the direct-coverage fast path: the composed transform must be axis-aligned and
  every transformed edge must land on a whole pixel. If so, the rectangle becomes a `RectU16`
  intersected into the enclosing scissor, and the old scissor is stored in the entry because
  intersection is not invertible. Nothing is rasterised. After each draw generates its strips,
  `clip_run` rewrites *that draw's own run*: `spans` decodes it exactly as the renderer reads it
  (alpha spans plus fill-gap solid spans), `Clipped::of` computes what survives, and a span cut by an
  edge gets fresh coverage bytes with outside pixels zeroed — tile-aligned, because a strip's `x` and
  width are whole tiles. A solid span keeps a solid interior (`emit_split_solid`), so a full-screen
  fill under a scrolling clip does not turn into a full-screen alpha buffer.
- **`Entry::Mask`** — everything else (rounded corners, rotation or skew, a half-pixel edge) is
  rasterised once into a coverage mask by `vello_common::clip::ClipContext`, and later draws are
  generated with that mask as their clip path. Nesting is `ClipContext`'s own: a mask pushed inside
  another is generated against it, so the top of the stack *is* the intersection.

`push_rounded` with all-square radii falls back to `push_rect`, so it can still scissor. `pop` on
an empty stack is ignored (`None => {}`) — an unbalanced widget tree must neither lift a sibling's
clip nor underflow the mask context, which would panic on the frame path.

## 2. One bracket stack for clips, layers and snapshots

`GroupStack` is deliberately one stack, not three. The walk pushes a `Group` for every
`PushClip`/`PushClipRounded`/`PushLayer` (and a translucent `PushSnapshot`), and routes `PopClip` and
`PopLayer` to the same `close_group` — so whichever pop arrives closes the innermost open bracket,
and each `Group` remembers what to undo (`closes_clip`, `closes_layer`). `close_open_groups` shuts
anything a frame left open at its end.

A layer lowers by opacity — `lower_layer(alpha)`: `alpha >= 1.0` is `LayerLowering::Clip`
(source-over at full opacity *is* drawing into the parent, so only its rectangle's clip remains);
anything below — including `0.0` and a non-finite value — is `LayerLowering::Isolated`: `open_layer`
also calls `frame.recorder.push_layer(layer_props(alpha), None)`, a recorded layer the scheduler
renders into a page and composites (lab 13). Either way the rectangle goes through `ClipStack`, never
`LayerProps::clip_path` — `layer_props` leaves that `None`.

`SnapshotStack` is the inline emulation of `PushSnapshot` (the engine has no snapshot cache): the
outermost bracket installs `snapshot_correction(rect, scale, transform)` as an affine the walk
composes as `root * correction` ahead of every inner command's own transform, and a sub-unity
`alpha` opens a nested layer via `open_layer`. Inner brackets' `alpha`/`scale` are ignored; only
their depth is tracked. The correction is `transform * scale_about(scale, rect.center()) *
transform.inverse()` — a conjugation: undo the body's transform, scale about the rect centre in the
body's own space, re-apply the transform — which is why the unit test
`the_correction_conjugates_by_the_body_transform` (≈401–411) finds the body's centre unmoved under
`correction * transform`. A singular transform yields the identity rather than infinities.

## 3. The clear punch is hoisted to the root

`ClearRect` exists for the platform-view hole punch (the producer is `PlatformViewWidget::paint`,
[lab 9](09-native-widgets-pipeline.md)). A punch confined to its group would only erase that group's
content, and a backdrop painted outside the scroll clip would seal the hole again. So the walk's
`Command::ClearRect` arm calls `punch_rect(rect, transform, self.groups.bounds())` — the transform's
bbox intersected with every open bracket's device bounds — and stages a `StagedPunch { device,
depth }` with a painter-order depth of its own. After the walk, `generate_punches` rasterises each
at the root, under no clip, into the shared strip storage *past* every draw's range, and pushes a
`ClearPunch { strip_range, bounds: device_bounds(..), depth }`. `device_bounds` clamps to the `u16`
grid rather than wrapping.

Punches live in `CompiledFrame::clears`, not `draws()`. The renderer issues them as a
destination-out pass (`dst · (1 − src.a)`); a target that disregards alpha skips that pass and reads
the frame exactly as if the clear had never been recorded.

## 4. Bracket geometry is refused up front

Before a strip is generated, `compile` walks every command through `check_geometry`: a clip's rect
(and rounded radii), a layer's rect *and* alpha, a snapshot's rect, alpha *and* scale, and a clear's
rect are checked for finiteness on the same terms as a fill's, because each is lowered (a clip's
rect decides scissor-vs-mask; an alpha decides isolation; a scale composes a transform). The three
`Pop*` carry no geometry and pass. The match is exhaustive over `Command`.

## Experiments

Run each as its own filtered target. Never run `cargo test -p frust-engine` unfiltered — it
includes `tests/proptest_strips.rs`, whose allocations run to tens of GB.

### 12.1 — Scissor versus mask

```bash
cargo test -p frust-engine --test clips      # 24 passed
```

Read `a_rotated_rectangular_clip_falls_back_to_a_mask` beside
`a_pixel_aligned_rectangular_clip_allocates_no_mask_and_no_intermediate`: same clip idea, but a
`rotate_about(0.4, ..)` transform flips `scissor_clips == 1, mask_clips == 0, clip_mask_strips == 0`
into `scissor_clips == 0, mask_clips == 1`. `a_rectangular_clip_off_the_pixel_grid_falls_back_to_a_mask`
shows half a pixel (`x0 = 12.5`) is enough. The pixel-aligned and rotated cases both assert `!frame.recorder.has_layers()` — no intermediate target.

### 12.2 — Measure it yourself (scratch test, do not commit)

Drop a throwaway `scratch_lab12.rs` into the crate's `tests/` directory that builds, on a 64×48 viewport,
a full-viewport `fill_rect` wrapped in `push_transform(t); push_clip(r); … pop_clip(); pop_transform()`,
compiles it with `SceneCompiler::new(64, 48).compile(&scene, Affine::IDENTITY, (64, 48))`, and prints
`frame.strips.strips.len()`, `frame.strips.alphas.len()`, `scissor_clips`, `mask_clips`,
`clip_mask_strips`. Run it with `--test scratch_lab12 -- --nocapture`. Measured on this tree:

| Case | strips | alphas | scissor | mask | mask strips |
|---|---|---|---|---|---|
| no clip | 25 | 384 | 0 | 0 | 0 |
| `r = (12,8,52,40)`, identity | 17 | 0 | 1 | 0 | 0 |
| `r = (13,6,51,39)`, identity | 31 | 544 | 1 | 0 | 0 |
| `r = (12,8,52,40)`, rotate 10° about centre | 19 | 640 | 0 | 1 | 19 |

The unclipped fill's 384 bytes are its left/right edge tiles (vello_common's `rect::render` emits a
coverage strip at each end of every interior row). A tile-aligned scissor drops those tiles and keeps
a pure solid interior — **zero** coverage bytes. A ragged scissor pays coverage only for the tiles its
edges cut. The rotated clip pays a 19-strip mask *and* 640 coverage bytes on the draw. Delete the file.

### 12.3 — Opacity decides isolation

```bash
cargo test -p frust-engine --test layers     # 25 passed
cargo test -p frust-engine --test schedule   # 33 passed
```

In `layers.rs`, `a_layer_at_full_opacity_is_its_clip_and_nothing_more` asserts
`layered.recorder.layers.is_empty()`, coverage identical to a `push_clip` scene, and one round.
At alpha < 1, `three_nested_translucent_layers_take_a_page_each` asserts
`frame.recorder.layers.len() == 3` and four rounds, innermost first (opacities `[0.75, 0.5, 0.25]`).
`a_snapshot_body_lowers_like_the_equivalent_transform_and_layer_pair` proves the snapshot emulation.
Bridge to lab 13: in `schedule.rs`, `a_depth_four_chain_alternates_page_groups_and_ends_at_the_surface`
counts `rounds.len() == 5` for four nested layers — one page per level, plus the surface.

### 12.4 — A clear inside a layer lands at the root

```bash
cargo test -p frust-engine --lib compile::clear   # 5 passed
```

`a_punch_is_confined_to_every_bracket_it_is_hoisted_past` shows the intersection;
`a_punch_no_open_bracket_admits_survives_nowhere` the drop. The whole-compile version is
`a_punch_inside_a_layer_is_hoisted_out_of_it_and_confined_to_it` in `tests/layers.rs`: a `clear_rect`
inside a translucent layer yields `frame.clears.len() == 1`, its coverage bbox clipped to the layer
`(16, 16, 24, 24)`, while the layer still costs its page (2 rounds). And
`a_clear_is_lowered_to_coverage_that_paints_no_draw` shows the frame's draws are unchanged by it.

## What to notice

- The clip *admission* rule and the fill fast path are one function (`fast_rect`); change one and
  you change both.
- "No intermediate texture for a clip" is load-bearing: the only thing that ever gets a page is a
  recorded layer, and only `lower_layer` decides that.
- Every `Pop*` is forgiving and every push is strict: unbalanced pops are ignored at runtime, but
  non-finite bracket geometry refuses the whole frame before anything is generated.
- A punch is a strip run like any draw, parked outside `draws()` so the pass that issues it is
  optional — the design choice that lets an opaque surface ignore it for free.
