/// S1 — animation storm. The bubblebench workload (flutter/flutter#180958):
/// radial-gradient bubbles + two shaped text runs each, repainted every frame
/// with per-frame physics. A 1:1 port of the frust side's
/// `benchmarks/frust_bench/src/scenarios/s1_animation.rs` physics + paint
/// (same constants, same phase order, same seed), so both apps render
/// identical content. Bubble **count** is derived per play area (spec v3,
/// [s1BubbleCountFor]) so the field settles instead of staying jam-packed —
/// capped at [s1BubbleCountCap].
///
/// "Perpetual" per PLAN 9.E: the physics steps and the full bubble scene
/// repaints every frame for the entire run window (the harness controls
/// duration) — the settle gate the original interactive bubblebench example
/// (now removed; its workload lives on self-contained in the frust-side
/// scenario referenced above) used is deliberately NOT applied here, so the
/// per-frame gradient-fill + text-shaping cost is sustained. That per-frame
/// repaint cost is the thing under test.
library;

import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/scheduler.dart' show Ticker;

import '../bench/datasets.dart';
import '../bench/rng.dart';

/// The physics simulation — a port of
/// `benchmarks/frust_bench/src/scenarios/s1_animation.rs`. Bubble
/// geometry follows the canonical S1 size-parity spec (see [s1RadiusMinFrac] in
/// `datasets.dart`, the contract home): radius + cluster spread are fractions
/// of `min(playWidth, playHeight)`, the SafeArea-inset play region.
class BubblePhysics {
  static const int seed = s1BubbleSeed;

  static const double _friction = 0.90;
  static const double _centerPull = 0.001;
  static const double _collisionStrength = 0.08;
  static const double _touchRepulsion = 0.9;
  static const double _touchRadius = 200.0;
  static const double _wallBounce = 0.35;

  static const List<String> _symbols = [
    'BTC', 'ETH', 'SOL', 'BNB', 'XRP', 'ADA', 'DOGE', 'AVAX', 'DOT', 'LINK', //
    'MATIC', 'UNI', 'SHIB', 'LTC', 'ATOM', 'TRX', 'XLM', 'ETC', 'FIL', 'NEAR', //
    'APT', 'OP', 'ARB', 'SUI', 'INJ', 'SEI', 'FTM', 'ALGO', 'VET', 'SAND', //
  ];

  final List<Bubble> bubbles = [];
  Offset? touch;
  double _width = 0;
  double _height = 0;
  double _centerX = 0;
  double _centerY = 0;

  void setSize(double width, double height) {
    if (width <= 0 || height <= 0) return;
    _width = width;
    _height = height;
    _centerX = width / 2;
    _centerY = height / 2;
    for (final b in bubbles) {
      b.x = b.x.clamp(b.radius, _width - b.radius);
      b.y = b.y.clamp(b.radius, _height - b.radius);
    }
  }

  /// Seed `count` bubbles clustered around center — mirrors `initialize_bubbles`.
  /// Radius and cluster spread are fractions of `S = min(width, height)` per the
  /// S1 size-parity spec ([s1RadiusMinFrac]); requires [setSize] to have run.
  void initializeBubbles(int count, int seed) {
    bubbles.clear();
    final s = math.min(_width, _height);
    final rng = SplitMix64(seed);
    for (var i = 0; i < count; i++) {
      final performance = rng.nextF64() * 40.0 - 20.0;
      final radius = (s1RadiusMinFrac + rng.nextF64() * s1RadiusSpanFrac) * s;
      final x =
          _centerX + (rng.nextF64() * s1ClusterSpanFrac - s1ClusterSpanFrac / 2) * s;
      final y =
          _centerY + (rng.nextF64() * s1ClusterSpanFrac - s1ClusterSpanFrac / 2) * s;
      bubbles.add(Bubble(
        symbol: _symbols[i % _symbols.length],
        performance: performance,
        x: x,
        y: y,
        radius: radius,
      ));
    }
  }

  /// One frame's step, in the repro's exact phase order.
  void update() {
    _applyCenterGravity();
    _applyCollisions();
    _applyTouchRepulsion();
    _applyFriction();
    _applyWallConstraints();
    for (final b in bubbles) {
      b.x += b.vx;
      b.y += b.vy;
    }
    for (final b in bubbles) {
      if (b.vx.abs() < 0.05) b.vx = 0;
      if (b.vy.abs() < 0.05) b.vy = 0;
    }
  }

  void _applyCenterGravity() {
    for (final b in bubbles) {
      b.vx += (_centerX - b.x) * _centerPull;
      b.vy += (_centerY - b.y) * _centerPull;
    }
  }

  void _applyCollisions() {
    for (var i = 0; i < bubbles.length; i++) {
      for (var j = i + 1; j < bubbles.length; j++) {
        final a = bubbles[i];
        final b = bubbles[j];
        final dx = b.x - a.x;
        final dy = b.y - a.y;
        final distance = math.sqrt(dx * dx + dy * dy);
        final minDistance = a.radius + b.radius;
        if (distance < minDistance && distance > 0) {
          final nx = dx / distance;
          final ny = dy / distance;
          final force = (minDistance - distance) * _collisionStrength;
          a.vx -= nx * force;
          a.vy -= ny * force;
          b.vx += nx * force;
          b.vy += ny * force;
        }
      }
    }
  }

  void _applyTouchRepulsion() {
    final t = touch;
    if (t == null) return;
    for (final b in bubbles) {
      final dx = b.x - t.dx;
      final dy = b.y - t.dy;
      final distance = math.sqrt(dx * dx + dy * dy);
      if (distance < _touchRadius && distance > 0) {
        final force = (1.0 - distance / _touchRadius) * _touchRepulsion;
        b.vx += (dx / distance) * force;
        b.vy += (dy / distance) * force;
      }
    }
  }

  void _applyFriction() {
    for (final b in bubbles) {
      b.vx *= _friction;
      b.vy *= _friction;
    }
  }

  void _applyWallConstraints() {
    for (final b in bubbles) {
      if (b.x - b.radius < 0) {
        b.x = b.radius;
        b.vx = b.vx.abs() * _wallBounce;
      }
      if (b.x + b.radius > _width) {
        b.x = _width - b.radius;
        b.vx = -b.vx.abs() * _wallBounce;
      }
      if (b.y - b.radius < 0) {
        b.y = b.radius;
        b.vy = b.vy.abs() * _wallBounce;
      }
      if (b.y + b.radius > _height) {
        b.y = _height - b.radius;
        b.vy = -b.vy.abs() * _wallBounce;
      }
    }
  }
}

/// One animated bubble.
class Bubble {
  Bubble({
    required this.symbol,
    required this.performance,
    required this.x,
    required this.y,
    required this.radius,
  });

  final String symbol;
  final double performance;
  double x;
  double y;
  double vx = 0;
  double vy = 0;
  final double radius;
}

/// The S1 scenario widget: a full-window custom-painted bubble field driven by
/// a per-frame [Ticker].
class AnimationStormView extends StatefulWidget {
  const AnimationStormView({super.key});

  @override
  State<AnimationStormView> createState() => _AnimationStormViewState();
}

class _AnimationStormViewState extends State<AnimationStormView>
    with SingleTickerProviderStateMixin {
  final BubblePhysics _physics = BubblePhysics();
  late final Ticker _ticker;
  bool _initialized = false;
  int _frame = 0;

  @override
  void initState() {
    super.initState();
    _ticker = createTicker((_) {
      _physics.update();
      setState(() => _frame++);
    })
      ..start();
  }

  @override
  void dispose() {
    _ticker.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        _physics.setSize(constraints.maxWidth, constraints.maxHeight);
        if (!_initialized &&
            constraints.maxWidth > 0 &&
            constraints.maxHeight > 0) {
          // Count derived per play area (spec v3) — see [s1BubbleCountFor].
          final count = s1BubbleCountFor(
              constraints.maxWidth, constraints.maxHeight);
          _physics.initializeBubbles(count, BubblePhysics.seed);
          _initialized = true;
        }
        return GestureDetector(
          onPanStart: (d) => _physics.touch = d.localPosition,
          onPanUpdate: (d) => _physics.touch = d.localPosition,
          onPanEnd: (_) => _physics.touch = null,
          child: CustomPaint(
            painter: _BubblePainter(_physics, _frame),
            size: Size.infinite,
          ),
        );
      },
    );
  }
}

class _BubblePainter extends CustomPainter {
  _BubblePainter(this.physics, this.frame) : super(repaint: null);

  final BubblePhysics physics;
  final int frame;

  static const Color _background = Color(0xFF0D1421);
  static const Color _green = Color(0xFF66BB6A);
  static const Color _red = Color(0xFFEF5350);
  static const Color _grey = Color(0xFF9E9E9E);

  static Color _bubbleColor(double performance) {
    if (performance > 0) return _green;
    if (performance < 0) return _red;
    return _grey;
  }

  static double _scaledFontSize(double radius) {
    const minR = 18.0, maxR = 60.0, minFs = 8.0, maxFs = 24.0;
    if (radius <= minR) return minFs;
    if (radius >= maxR) return maxFs;
    final normalized = (radius - minR) / (maxR - minR);
    return minFs + math.sqrt(normalized) * (maxFs - minFs);
  }

  @override
  void paint(Canvas canvas, Size size) {
    canvas.drawRect(Offset.zero & size, Paint()..color = _background);

    for (final b in physics.bubbles) {
      final center = Offset(b.x, b.y);
      final r = b.radius;
      final base = _bubbleColor(b.performance);

      // Radial-gradient fill: center offset to (-0.2r, -0.2r), radius 0.95r,
      // 0.7 → 0.3 alpha — the repro's RadialGradient verbatim.
      final fill = Paint()
        ..shader = RadialGradient(
          center: const Alignment(-0.2, -0.2),
          radius: 0.95,
          colors: [
            base.withValues(alpha: 0.7),
            base.withValues(alpha: 0.3),
          ],
        ).createShader(Rect.fromCircle(center: center, radius: r));
      canvas.drawCircle(center, r, fill);

      canvas.drawCircle(
        center,
        r,
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = 2.0
          ..color = base.withValues(alpha: 0.6),
      );

      final fontSize = _scaledFontSize(r);
      final symbolPainter = TextPainter(
        text: TextSpan(
          text: b.symbol,
          style: TextStyle(
            color: Colors.white,
            fontSize: fontSize,
            fontWeight: FontWeight.bold,
          ),
        ),
        textDirection: TextDirection.ltr,
      )..layout();
      // Skip when the symbol outgrows the bubble (repro: width > radius*1.6).
      if (symbolPainter.width > r * 1.6) continue;
      symbolPainter.paint(
        canvas,
        Offset(center.dx - symbolPainter.width / 2,
            center.dy - symbolPainter.height / 2 - r * 0.12),
      );

      final percentPainter = TextPainter(
        text: TextSpan(
          text: '${b.performance >= 0 ? '+' : ''}'
              '${b.performance.toStringAsFixed(1)}%',
          style: TextStyle(
            color: base,
            fontSize: fontSize * 0.7,
            fontWeight: FontWeight.w600,
          ),
        ),
        textDirection: TextDirection.ltr,
      )..layout();
      percentPainter.paint(
        canvas,
        Offset(center.dx - percentPainter.width / 2, center.dy + r * 0.1),
      );
    }
  }

  @override
  bool shouldRepaint(_BubblePainter oldDelegate) => oldDelegate.frame != frame;
}
