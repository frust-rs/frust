/// S4 — heavy-work responsiveness. Parse a deterministic ~50MB JSON payload
/// off the UI thread (via `Isolate.run`, Flutter's idiomatic heavy-work path)
/// while a continuous animation runs; the harness reads the animation's frame
/// percentiles during the parse plus the parse wall time.
///
/// This is the head-to-head for PLAN 9.E's S4 claim: `Isolate.run` COPIES the
/// payload into the worker isolate (the idiom under test), versus the frust
/// side's `spawn_blocking` MOVE. The parse window is bracketed by the
/// `s4-parse` marker pair (its own wall-time series); the animation runs the
/// whole time so its frame series shows any UI-thread impact.
library;

import 'dart:convert';
import 'dart:isolate';

import 'package:flutter/material.dart';

import '../bench/datasets.dart';
import '../bench/perf.dart';

class HeavyWorkView extends StatefulWidget {
  const HeavyWorkView({super.key});

  @override
  State<HeavyWorkView> createState() => _HeavyWorkViewState();
}

class _HeavyWorkViewState extends State<HeavyWorkView>
    with SingleTickerProviderStateMixin {
  late final AnimationController _spin;
  String _status = 'generating payload…';
  int _parsedRecords = 0;

  @override
  void initState() {
    super.initState();
    _spin = AnimationController(
      vsync: this,
      duration: const Duration(seconds: 2),
    )..repeat();
    WidgetsBinding.instance.addPostFrameCallback((_) => _run());
  }

  Future<void> _run() async {
    // Generate off-thread so payload construction (not the measured part) does
    // not itself jank the animation.
    final payload = await Isolate.run(generateS4JsonPayload);
    if (!mounted) return;
    setState(() => _status = 'parsing ${payload.length ~/ (1024 * 1024)}MB…');

    markScenarioStart('s4-parse');
    // Isolate.run copies `payload` into the worker — the copy-vs-move idiom.
    final decoded = await Isolate.run(() => jsonDecode(payload) as List);
    markScenarioEnd('s4-parse');

    if (!mounted) return;
    setState(() {
      _parsedRecords = decoded.length;
      _status = 'parsed $_parsedRecords records';
    });
  }

  @override
  void dispose() {
    _spin.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          RotationTransition(
            turns: _spin,
            child: Container(
              width: 96,
              height: 96,
              decoration: BoxDecoration(
                gradient: const LinearGradient(
                  colors: [Colors.tealAccent, Colors.deepPurpleAccent],
                ),
                borderRadius: BorderRadius.circular(16),
              ),
            ),
          ),
          const SizedBox(height: 32),
          Text(_status),
        ],
      ),
    );
  }
}
