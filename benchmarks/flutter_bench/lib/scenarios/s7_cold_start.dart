/// S7 — cold start + idle. On the first frame, report Flutter's own in-app
/// first-frame span (`main` → first rasterized frame); then hold a fully static
/// UI for 60s so the harness can measure idle CPU + sustained memory. The idle
/// window is bracketed by the `s7-idle` marker pair.
///
/// True cold start (process spawn → `main`) is only measurable externally via
/// `am start -W`, which the harness records separately (documented in PROTOCOL).
/// This scenario contributes the framework-internal first-frame number and the
/// idle window; the static UI issues no per-frame work, so a thin runtime goes
/// frame-gate idle (the claim under test).
library;

import 'package:flutter/material.dart';

import '../bench/perf.dart';

class ColdStartIdleView extends StatefulWidget {
  const ColdStartIdleView({super.key});

  @override
  State<ColdStartIdleView> createState() => _ColdStartIdleViewState();
}

class _ColdStartIdleViewState extends State<ColdStartIdleView> {
  String _status = 'measuring first frame…';

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _onFirstFrame());
  }

  Future<void> _onFirstFrame() async {
    final firstFrameMs = benchUptime.elapsedMilliseconds;
    emitStartupSpan(firstFrameMs: firstFrameMs);
    if (!mounted) return;
    setState(() => _status = 'first frame at ${firstFrameMs}ms — idling 60s');

    markScenarioStart('s7-idle');
    await Future<void>.delayed(const Duration(seconds: 60));
    markScenarioEnd('s7-idle');
    if (!mounted) return;
    setState(() => _status = 'idle window complete');
  }

  @override
  Widget build(BuildContext context) {
    // Deliberately static: no animation, no timers driving rebuilds, so the
    // runtime can idle between frames.
    return Center(
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          const FlutterLogo(size: 96),
          const SizedBox(height: 24),
          Text(_status),
        ],
      ),
    );
  }
}
