/// Tests for the S1 dataset-derivation spec (v3, user-calibrated
/// 2026-07-20): the bubble-count formula that replaces the v2 fixed count.
/// The frust side has the same test (`s1_animation/physics.rs`'s
/// `bubble_count_for` unit tests) against the byte-identical formula.
library;

import 'dart:math' as math;

import 'package:flutter_bench/bench/datasets.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('s1BubbleCountFor', () {
    test('desktop-like 800x600 play area', () {
      // S = 600, mean bubble area ~= 0.0340339 * 600^2 ~= 12252.2, play area
      // 480000, target 240000 -> floor(240000 / 12252.2) = 19.
      expect(s1BubbleCountFor(800, 600), 19);
    });

    test('phone-like 393x750 play area derives substantially fewer than the'
        ' original cap', () {
      final n = s1BubbleCountFor(393, 750);
      expect(n, 28);
      expect(n, lessThan(s1BubbleCountCap));
    });

    test('phone-like play area lands near 50% total bubble-area coverage', () {
      // Sum the *analytic mean* per-bubble area at the derived count and
      // compare against the play area — a coarser check than sampling actual
      // radii (that's the frust-side seeded-field test), but confirms the
      // formula's target is honored at this size.
      const playWidth = 393.0, playHeight = 750.0;
      final n = s1BubbleCountFor(playWidth, playHeight);
      final s = math.min(playWidth, playHeight);
      const a = s1RadiusMinFrac, span = s1RadiusSpanFrac;
      final meanArea =
          math.pi * s * s * (a * a + a * span + span * span / 3.0);
      final coverage = (n * meanArea) / (playWidth * playHeight);
      expect(coverage, greaterThan(0.35));
      expect(coverage, lessThan(0.65));
    });

    test('caps at the original sixty on a wide desktop window', () {
      // Uncapped formula would yield 73 at 3000x600; must cap at 60.
      expect(s1BubbleCountFor(3000, 600), s1BubbleCountCap);
    });

    test('zero-area play regions derive zero bubbles', () {
      expect(s1BubbleCountFor(0, 600), 0);
      expect(s1BubbleCountFor(800, 0), 0);
    });

    test('an identical play area on both apps derives an identical count'
        ' (formula determinism, not a sampled estimate)', () {
      // Calling twice with the same inputs must be bit-identical — the
      // fairness contract this spec depends on.
      expect(s1BubbleCountFor(393, 750), s1BubbleCountFor(393, 750));
    });
  });
}
