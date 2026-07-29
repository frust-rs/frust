/// Deterministic dataset generators shared across scenarios — the concrete,
/// documented parameters that make the two apps' inputs byte-identical (the
/// fairness gate: "same seed, same dataset bytes").
///
/// Each generator is seeded through [SplitMix64], which is a byte-for-byte port
/// of the frust PRNG, so the frust side reproduces the same
/// values by using the same seed + the same algorithm documented here. Where a
/// container encoding cannot be byte-identical across languages (PNG bytes,
/// float text formatting), the parity target is the *pre-encode pixel/logical
/// content* — stated per generator.
library;

import 'dart:math' as math;
import 'dart:typed_data';

import 'rng.dart';

// ---------------------------------------------------------------------------
// S1 — animation storm (bubble geometry parity)
// ---------------------------------------------------------------------------

/// PRNG seed for the S1 bubble field (both apps seed the same [SplitMix64]).
const int s1BubbleSeed = 42;

/// Cap on the derived S1 bubble count (spec v3, see [s1BubbleCountFor]) — the
/// original v1/v2 fixed count, still the ceiling so a wide desktop window
/// doesn't explode the population.
const int s1BubbleCountCap = 60;

/// **S1 size-parity spec (canonical).** Bubble radius and the initial cluster
/// spread are FRACTIONS of `S = min(playWidth, playHeight)` — the *safe-area
/// inset* play region on BOTH apps (Flutter hosts the scenario under
/// `Scaffold > SafeArea`; the frust app wraps its scenario host in `SafeArea`)
/// — rather than absolute logical pixels. Expressing them as fractions makes
/// the bubble-size-to-play-area ratio (and therefore the collision density and
/// settle behavior) identical across the two apps regardless of any residual
/// difference in each shell's reported logical size, which is what makes S1 a
/// valid head-to-head. Before this normalization the two apps packed the same
/// absolute-pixel bubbles into differently-sized play areas (frust rendered
/// edge-to-edge, Flutter inside the SafeArea), so frust's field was far more
/// crammed — the invalid comparison this spec fixes.
///
/// The velocity/physics constants (friction, center pull, collision strength,
/// wall bounce, the settle/zeroing velocity thresholds) are dimensionally
/// consistent and left byte-identical between the two apps, so a given `S`
/// reproduces a byte-identical simulation. The four per-bubble RNG draws
/// (performance, radius, x, y) keep their original order so `seed 42` yields
/// the same sequence on both sides. Fractions are anchored so a phone-sized
/// play area (`S ≈ 380–400` logical px on the reference OnePlus 9) resolves to
/// the original ~20–60 px radii.
const double s1RadiusMinFrac = 0.05;
const double s1RadiusSpanFrac = 0.10;
const double s1ClusterSpanFrac = 0.5;

/// **S1 count-derivation spec v3 (canonical, user-calibrated 2026-07-20).**
/// v2 (above) fixed the bubble *count* at [s1BubbleCountCap] while making
/// radii fractions of the play area — but 60 bubbles at
/// `s1RadiusMinFrac`..`s1RadiusMinFrac + s1RadiusSpanFrac` still sum to more
/// than a phone's play area, so the field stayed jam-packed with constant
/// collisions rather than settling (confirmed visually on-device). v3 instead
/// derives the bubble **count** so the bubbles' total area is
/// ~[s1TargetAreaCoverage] of the play area, capped at [s1BubbleCountCap] so
/// a wide desktop window doesn't explode the count — a settle-capable field.
///
/// The count is computed analytically, not sampled: bubble radius is
/// `S * (s1RadiusMinFrac + u * s1RadiusSpanFrac)` for `u` uniform in
/// `[0, 1)`, so for the linear function `f(u) = a + u*span` (`a` =
/// [s1RadiusMinFrac], `span` = [s1RadiusSpanFrac]), the exact expectation
/// `E[f(u)^2] = a^2 + a*span + span^2/3` (the closed-form integral of
/// `f(u)^2` over `u in [0, 1]`), so `E[bubbleArea] = pi * S^2 * E[f(u)^2]`.
/// Both apps compute this exact formula (see [s1BubbleCountFor] here, mirrored
/// in the frust side's `s1_animation/physics.rs::bubble_count_for`), so an
/// identical play area yields an identical `N` — the per-bubble RNG draw
/// order (performance, radius, x, y) is unchanged, so `N` only truncates the
/// same deterministic sequence rather than reordering it.
const double s1TargetAreaCoverage = 0.5;

/// The analytic mean bubble area (`E[bubbleArea]`) for a play region whose
/// `S = min(playWidth, playHeight)` — see [s1TargetAreaCoverage]'s derivation.
double _s1MeanBubbleArea(double s) {
  const a = s1RadiusMinFrac;
  const span = s1RadiusSpanFrac;
  final meanRSquared = a * a + a * span + span * span / 3.0;
  return math.pi * s * s * meanRSquared;
}

/// Derive the S1 bubble count for a `playWidth` x `playHeight` play area (the
/// SafeArea-inset region on both apps), so total bubble area is
/// ~[s1TargetAreaCoverage] of the play area — capped at [s1BubbleCountCap].
/// See the spec above [s1TargetAreaCoverage] for the exact formula and the
/// frust-side mirror.
int s1BubbleCountFor(double playWidth, double playHeight) {
  if (playWidth <= 0 || playHeight <= 0) return 0;
  final s = math.min(playWidth, playHeight);
  final meanArea = _s1MeanBubbleArea(s);
  final playArea = playWidth * playHeight;
  final n = (s1TargetAreaCoverage * playArea / meanArea).floor();
  return n > s1BubbleCountCap ? s1BubbleCountCap : n;
}

// ---------------------------------------------------------------------------
// S3 — table ops (continuous-cycling contract)
// ---------------------------------------------------------------------------

/// **S3 continuous-cycling spec (canonical, user-directed 2026-07-20).**
///
/// Root cause of the prior "table disappears" report: S3's classic jsfb op
/// sequence (`create1k → create10k → update → swap → clear`) ran ONCE, so the
/// table legitimately emptied at the terminal `clear` within the first few
/// seconds and then sat blank for the rest of a 30s capture — a real
/// emptiness, not the "occlusion" bug originally suspected (device data
/// showed healthy per-op layouts throughout).
///
/// Both apps now CYCLE the op sequence continuously for the whole capture
/// window: run the five ops in the existing order, each separated by
/// [s3SettleGapMs]'s fixed settle gap (keeping per-op trace windows cleanly
/// separable, same purpose as every other scripted scenario's settle
/// framing); after `clear` closes, wait the same [s3SettleGapMs] gap and
/// restart from `create1k` — repeating until the scenario itself is torn
/// down (navigating away, or the harness ending the capture). The `s3-*`
/// sub-marker names are unchanged and stamp once per op **per cycle**; see
/// `benchmarks/harness/stats.py`'s scenario-slicing docs for why repeated
/// same-name marker pairs across cycles aggregate into one series rather
/// than breaking.
///
/// No explicit per-cycle RNG reseed is needed: S3 draws no randomness at all
/// (each row's label is a pure function of a monotonically-increasing `id`
/// against the three fixed word banks each scenario file keeps verbatim), and
/// `id` assignment is already cycle-index-free by construction — `create10k`
/// deterministically resets the id counter to 1 at the start of every cycle,
/// identically on both apps, so cycle N reproduces the byte-identical
/// row/label sequence cycle 1 did. Both `flutter_bench`'s `s3_table.dart` and
/// the frust side's `scenarios/s3_table.rs` implement this loop and mirror
/// this one constant.
const int s3SettleGapMs = 300;

// ---------------------------------------------------------------------------
// S5 — image pipeline
// ---------------------------------------------------------------------------

/// Side length in pixels of every generated S5 image (square).
const int s5ImageSize = 256;

/// How many images the S5 stream cycles through.
const int s5ImageCount = 240;

/// Base seed for the S5 image stream. Image `i` derives its own stream from
/// `s5ImageSeed + i` so each image is distinct yet fully reproducible.
const int s5ImageSeed = 0x00C0FFEE;

/// Generate image `index`'s raw RGBA pixel bytes (`s5ImageSize²` × 4 bytes,
/// row-major, straight alpha). The content is a diagonal gradient (r from x,
/// g from y) plus per-pixel seeded blue-channel noise — cheap to generate,
/// visually non-trivial to decode/composite.
///
/// **Parity contract with 9E-03**: these RGBA bytes are the byte-identical
/// pixel source both apps share. The frust side may wrap them in a PNG for its
/// `decode_image_async` path; the container bytes then differ by encoder (as
/// they must across `image` crate vs the engine), but the decoded pixels are
/// identical. The Flutter side feeds these straight into the engine's async
/// pixel decode (`decodeImageFromPixels`), so no cross-language encoder is on
/// the fairness path at all.
Uint8List generateS5ImageRgba(int index) {
  final size = s5ImageSize;
  final rng = SplitMix64(s5ImageSeed + index);
  final bytes = Uint8List(size * size * 4);
  var o = 0;
  for (var y = 0; y < size; y++) {
    for (var x = 0; x < size; x++) {
      bytes[o++] = (x * 255) ~/ (size - 1); // R: horizontal gradient
      bytes[o++] = (y * 255) ~/ (size - 1); // G: vertical gradient
      bytes[o++] = rng.nextByte(); // B: seeded noise
      bytes[o++] = 0xFF; // A: opaque
    }
  }
  return bytes;
}

// ---------------------------------------------------------------------------
// S4 — heavy-work JSON payload
// ---------------------------------------------------------------------------

/// Seed for the S4 payload generator.
const int s4PayloadSeed = 0x5EED4;

/// Record count for the S4 payload — tuned so the serialized JSON is ~50MB
/// (each record serializes to roughly 500 bytes).
const int s4RecordCount = 100000;

/// Build the deterministic ~50MB JSON payload string parsed by S4.
///
/// Each record is an object with an `id` (int), `name` (12 hex chars),
/// `value` (0..1000 int), `active` (bool), and `tags` (3 ints); the whole
/// thing is a JSON array. See the code below for the exact field order.
///
/// **Parity contract with 9E-04**: the logical structure, record count, field
/// order, and per-field seeded values are the shared contract. Integer-valued
/// fields keep the text byte-identical across languages (no float formatting
/// divergence); the harness measures parse wall time of a payload of this exact
/// size and shape on both sides.
String generateS4JsonPayload() {
  final rng = SplitMix64(s4PayloadSeed);
  final buf = StringBuffer('[');
  for (var i = 0; i < s4RecordCount; i++) {
    if (i > 0) buf.write(',');
    final name = _hex12(rng);
    final value = rng.nextIntBelow(1000);
    final active = (rng.nextU64() & 1) == 1;
    final t0 = rng.nextIntBelow(1000);
    final t1 = rng.nextIntBelow(1000);
    final t2 = rng.nextIntBelow(1000);
    buf
      ..write('{"id":')
      ..write(i)
      ..write(',"name":"')
      ..write(name)
      ..write('","value":')
      ..write(value)
      ..write(',"active":')
      ..write(active)
      ..write(',"tags":[')
      ..write(t0)
      ..write(',')
      ..write(t1)
      ..write(',')
      ..write(t2)
      ..write(']}');
  }
  buf.write(']');
  return buf.toString();
}

const String _hexDigits = '0123456789abcdef';

String _hex12(SplitMix64 rng) {
  final sb = StringBuffer();
  for (var i = 0; i < 12; i++) {
    sb.write(_hexDigits[rng.nextU64() & 0xF]);
  }
  return sb.toString();
}
