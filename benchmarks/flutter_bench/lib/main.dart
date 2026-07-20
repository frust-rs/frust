/// flutter_bench — the Flutter side of the Frust vs Flutter paired benchmark
/// suite (Phase 9.E). All eight scenarios (S1–S8) run behind one scenario
/// contract; raw per-frame timings + scenario markers stream to the platform
/// log for the shared harness to slice. See `benchmarks/PROTOCOL.md` for the
/// methodology and `lib/bench/perf.dart` for the exact trace formats.
///
/// Scenario selection: `flutter run --dart-define=SCENARIO=s3` or a
/// `flutterbench://s3` deep link. With neither, a picker is shown.
library;

import 'package:flutter/material.dart';
import 'package:flutter/scheduler.dart';

import 'bench/fairness.dart';
import 'bench/perf.dart';
import 'bench/registry.dart';
import 'bench/scenario.dart';

Future<void> main() async {
  benchUptime.start();
  WidgetsFlutterBinding.ensureInitialized();

  // Fairness gate: opt into the panel's high refresh rate on Android (Flutter
  // does not by default). iOS uses the Info.plist CADisableMinimumFrameDuration
  // key; desktop follows the OS.
  await requestHighRefreshRate();

  // Raw per-frame capture on from process start, so nothing is missed.
  installFrameCapture();

  final initialRoute =
      SchedulerBinding.instance.platformDispatcher.defaultRouteName;
  final scenario = scenarioById(resolveScenarioId(initialRoute));

  runApp(BenchApp(initialScenario: scenario));
}

class BenchApp extends StatelessWidget {
  const BenchApp({super.key, this.initialScenario});

  final Scenario? initialScenario;

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'flutter_bench',
      debugShowCheckedModeBanner: false,
      theme: ThemeData.dark(useMaterial3: true),
      home: BenchHome(initialScenario: initialScenario),
    );
  }
}

/// Hosts the selected scenario, or a picker when none was requested. Also
/// listens for runtime deep links (`didPushRouteInformation`) so a
/// `flutterbench://s5` link delivered while running switches scenarios.
class BenchHome extends StatefulWidget {
  const BenchHome({super.key, this.initialScenario});

  final Scenario? initialScenario;

  @override
  State<BenchHome> createState() => _BenchHomeState();
}

class _BenchHomeState extends State<BenchHome> with WidgetsBindingObserver {
  Scenario? _scenario;

  @override
  void initState() {
    super.initState();
    _scenario = widget.initialScenario;
    WidgetsBinding.instance.addObserver(this);
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  @override
  Future<bool> didPushRouteInformation(RouteInformation info) async {
    final id = scenarioIdFromRoute(info.uri.toString());
    final next = scenarioById(id);
    if (next != null) {
      setState(() => _scenario = next);
      return true;
    }
    return false;
  }

  void _select(Scenario scenario) => setState(() => _scenario = scenario);

  @override
  Widget build(BuildContext context) {
    final scenario = _scenario;
    if (scenario == null) {
      return _ScenarioPicker(onSelect: _select);
    }
    // Keyed so switching scenarios rebuilds the host from scratch (fresh
    // start/end markers, fresh scenario state).
    return Scaffold(
      body: SafeArea(
        child: ScenarioHost(
          key: ValueKey(scenario.id),
          scenario: scenario,
        ),
      ),
    );
  }
}

class _ScenarioPicker extends StatelessWidget {
  const _ScenarioPicker({required this.onSelect});

  final ValueChanged<Scenario> onSelect;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('flutter_bench — pick a scenario')),
      body: ListView(
        children: [
          for (final s in allScenarios)
            ListTile(
              leading: Text(s.id.toUpperCase()),
              title: Text(s.label),
              onTap: () => onSelect(s),
            ),
        ],
      ),
    );
  }
}
