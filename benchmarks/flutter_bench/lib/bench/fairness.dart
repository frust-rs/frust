/// Fairness gates the paired benchmark's credibility depends on.
///
/// - **Android high refresh**: Flutter does NOT opt into >60Hz display modes by
///   default; a fair comparison against a frust app that runs at the panel's
///   native rate requires explicitly requesting the high-refresh mode. This is
///   the idiomatic community approach (`flutter_displaymode`), applied here.
/// - **iOS `CADisableMinimumFrameDurationOnPhone`**: set in `ios/Runner/
///   Info.plist` (a build-time key, not code) so ProMotion iPhones are allowed
///   to drive past 60Hz — the iOS analog of the Android opt-in.
///
/// Identical seeds / dataset bytes / input timelines are enforced per-scenario
/// (see `rng.dart` and each scenario's generator), not here.
library;

import 'package:flutter/foundation.dart';
import 'package:flutter_displaymode/flutter_displaymode.dart';

/// Request the highest-refresh display mode on Android. A no-op on other
/// platforms (iOS uses the Info.plist key; desktop follows the OS). Best-effort:
/// failures (emulator, unsupported panel) are swallowed so a bench run still
/// proceeds at the default rate rather than crashing.
Future<void> requestHighRefreshRate() async {
  if (kIsWeb || defaultTargetPlatform != TargetPlatform.android) return;
  try {
    await FlutterDisplayMode.setHighRefreshRate();
  } catch (_) {
    // Unsupported / emulator: fall through at the default refresh rate.
  }
}
