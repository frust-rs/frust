/// The scenario contract shared by all eight benchmark scenarios, mirroring
/// the frust side's `Scenario` trait + registry (task 9E-02): a stable id
/// (also the top-level scenario-marker name), a human label, and a widget
/// builder. [ScenarioHost] brackets the scenario's lifetime with
/// `bench-scenario-start/end <id>` markers so the harness can slice the raw
/// frame series without the scenario itself having to remember to.
library;

import 'package:flutter/material.dart';

import 'perf.dart';

/// One benchmark scenario. `id` is `s1`..`s8` and doubles as the top-level
/// scenario-marker name (byte-identical to the frust side). Scenarios that
/// time sub-operations (S3, S4) emit their own sub-markers via [markScenarioStart]
/// directly.
abstract class Scenario {
  const Scenario();

  /// Stable id, e.g. `s1`. Also the scenario-marker name.
  String get id;

  /// Human-readable label shown when idle / selecting.
  String get label;

  /// Build the scenario's widget subtree. Called once inside a [ScenarioHost].
  Widget build(BuildContext context);
}

/// Wraps a scenario, emitting `bench-scenario-start <id>` on mount and
/// `bench-scenario-end <id>` on dispose — the top-level window every scenario
/// gets for free. Perpetual scenarios (S1) simply never dispose during a run;
/// the harness slices by the start marker plus its fixed run duration.
class ScenarioHost extends StatefulWidget {
  const ScenarioHost({super.key, required this.scenario});

  final Scenario scenario;

  @override
  State<ScenarioHost> createState() => _ScenarioHostState();
}

class _ScenarioHostState extends State<ScenarioHost> {
  @override
  void initState() {
    super.initState();
    markScenarioStart(widget.scenario.id);
  }

  @override
  void dispose() {
    markScenarioEnd(widget.scenario.id);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.scenario.build(context);
}
