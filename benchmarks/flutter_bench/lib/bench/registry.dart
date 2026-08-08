/// The scenario registry (all eight frame-class scenarios plus the `d*`
/// op-latency scenarios) plus scenario selection — deep link
/// (`flutterbench://<id>`) with a `--dart-define=SCENARIO=<id>` fallback,
/// mirroring the frust side's driver contract (task 9E-02).
library;

import 'package:flutter/material.dart';

import '../scenarios/d1_db_write.dart';
import '../scenarios/d2_db_read.dart';
import '../scenarios/s1_animation.dart';
import '../scenarios/s2_list.dart';
import '../scenarios/s3_table.dart';
import '../scenarios/s4_heavy.dart';
import '../scenarios/s5_image.dart';
import '../scenarios/s6_text.dart';
import '../scenarios/s7_cold_start.dart';
import '../scenarios/s8_prefs.dart';
import 'scenario.dart';

/// A concrete scenario built from a widget factory.
class _WidgetScenario extends Scenario {
  const _WidgetScenario(this.id, this.label, this._builder);

  @override
  final String id;
  @override
  final String label;
  final WidgetBuilder _builder;

  @override
  Widget build(BuildContext context) => _builder(context);
}

/// Every scenario, in id order. Keep this list the single source of truth.
final List<Scenario> allScenarios = [
  const _WidgetScenario('s1', 'Animation storm', _s1),
  const _WidgetScenario('s2', 'Long-list scroll', _s2),
  const _WidgetScenario('s3', 'Table ops', _s3),
  const _WidgetScenario('s4', 'Heavy-work responsiveness', _s4),
  const _WidgetScenario('s5', 'Image pipeline', _s5),
  const _WidgetScenario('s6', 'Text shaping stress', _s6),
  const _WidgetScenario('s7', 'Cold start + idle', _s7),
  const _WidgetScenario('s8', 'Plugin-call overhead', _s8),
  // `d*` — op-latency DB scenarios (PROTOCOL §9), a second scenario-id
  // namespace parallel to s1..s8 (§9.1).
  const _WidgetScenario('d1', 'DB writes', _d1),
  const _WidgetScenario('d2', 'DB reads', _d2),
];

Widget _s1(BuildContext _) => const AnimationStormView();
Widget _s2(BuildContext _) => const LongListView();
Widget _s3(BuildContext _) => const TableOpsView();
Widget _s4(BuildContext _) => const HeavyWorkView();
Widget _s5(BuildContext _) => const ImagePipelineView();
Widget _s6(BuildContext _) => const TextStressView();
Widget _s7(BuildContext _) => const ColdStartIdleView();
Widget _s8(BuildContext _) => const PluginOverheadView();
Widget _d1(BuildContext _) => const DbWriteView();
Widget _d2(BuildContext _) => const DbReadView();

/// Look a scenario up by id, or null if none matches.
Scenario? scenarioById(String? id) {
  if (id == null) return null;
  final normalized = id.trim().toLowerCase();
  for (final s in allScenarios) {
    if (s.id == normalized) return s;
  }
  return null;
}

/// Resolve the requested scenario id from (in priority order) the
/// `SCENARIO` dart-define, then the initial deep-link route. Returns null when
/// neither selects one (the app then shows the picker).
String? resolveScenarioId(String? initialRoute) {
  const fromDefine = String.fromEnvironment('SCENARIO');
  if (fromDefine.isNotEmpty) return fromDefine;
  return scenarioIdFromRoute(initialRoute);
}

/// Parse a scenario id out of a deep-link route/URI. Accepts
/// `flutterbench://s3`, `flutterbench://scenario/s3`, and a bare `/s3` path.
/// Pure and directly unit-testable.
String? scenarioIdFromRoute(String? route) {
  if (route == null || route.isEmpty || route == '/') return null;
  final uri = Uri.tryParse(route);
  if (uri == null) return null;
  // Custom-scheme deep link: `flutterbench://<id>` → host is the id.
  if (uri.scheme == 'flutterbench' && uri.host.isNotEmpty) {
    // `flutterbench://scenario/s3` → host 'scenario', last segment 's3'.
    if (uri.pathSegments.isNotEmpty) return uri.pathSegments.last;
    return uri.host;
  }
  // Plain path route (`/s3` or `/scenario/s3`): take the last non-empty segment.
  if (uri.pathSegments.isNotEmpty) return uri.pathSegments.last;
  return null;
}
