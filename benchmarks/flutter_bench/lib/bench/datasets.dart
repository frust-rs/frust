/// Deterministic dataset generators shared across scenarios — the concrete,
/// documented parameters that make the two apps' inputs byte-identical (the
/// PLAN 9.E fairness gate: "same seed, same dataset bytes").
///
/// Each generator is seeded through [SplitMix64], which is a byte-for-byte port
/// of the frust PRNG, so the frust side (tasks 9E-03/04) reproduces the same
/// values by using the same seed + the same algorithm documented here. Where a
/// container encoding cannot be byte-identical across languages (PNG bytes,
/// float text formatting), the parity target is the *pre-encode pixel/logical
/// content* — stated per generator.
library;

import 'dart:typed_data';

import 'rng.dart';

// ---------------------------------------------------------------------------
// S1 — animation storm (bubble geometry parity)
// ---------------------------------------------------------------------------

/// PRNG seed for the S1 bubble field (both apps seed the same [SplitMix64]).
const int s1BubbleSeed = 42;

/// Number of bubbles in the S1 field.
const int s1BubbleCount = 60;

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
