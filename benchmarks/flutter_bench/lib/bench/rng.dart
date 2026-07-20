/// SplitMix64 — a byte-for-byte port of the frust side's PRNG
/// (`examples/bubblebench/src/physics.rs`), so a given seed produces the SAME
/// value sequence on both sides. This is the fairness gate's foundation:
/// identical seeds → identical datasets (bubble layouts, image bytes, JSON
/// payloads) across the two apps.
///
/// Dart native `int` is a fixed 64-bit two's-complement integer that wraps on
/// overflow, matching Rust's `u64` wrapping arithmetic bit-for-bit; `>>>` is
/// the logical (zero-filling) right shift the algorithm requires. (This class
/// is native-only by design — it relies on 64-bit ints and is never run on
/// web, where `int` is a double.)
library;

/// Deterministic 64-bit PRNG. Construct with a seed; each call advances state.
class SplitMix64 {
  int _state;

  SplitMix64(int seed) : _state = seed;

  /// The next 64-bit value — identical to the frust `SplitMix64::next_u64`.
  int nextU64() {
    _state = _state + 0x9E3779B97F4A7C15;
    var z = _state;
    z = (z ^ (z >>> 30)) * 0xBF58476D1CE4E5B9;
    z = (z ^ (z >>> 27)) * 0x94D049BB133111EB;
    return z ^ (z >>> 31);
  }

  /// Uniform in `[0, 1)` from the top 53 bits — identical to `next_f64`.
  double nextF64() => (nextU64() >>> 11) / (1 << 53);

  /// A byte in `[0, 256)` — convenience for the deterministic dataset
  /// generators (image bytes, payload fill).
  int nextByte() => nextU64() & 0xFF;

  /// An `int` in `[0, bound)`.
  int nextIntBelow(int bound) => (nextF64() * bound).floor();
}
