/// S3 — table ops. The js-framework-benchmark subset run as a scripted,
/// per-op-timed sequence: create 1k, create 10k, update every 10th of the 10k,
/// swap two rows, clear. Each op is bracketed by its own sub-marker
/// (`s3-create1k`, `s3-create10k`, `s3-update`, `s3-swap`, `s3-clear`) so the
/// harness times build/diff/reconcile throughput per op — matching the frust
/// side's per-op sub-markers (task 9E-04).
///
/// The `s3-create10k` step is included so the "every 10th of 10k" update has a
/// 10k dataset to act on (the canonical jsfb sequence); both apps run the same
/// steps in the same order.
///
/// **Continuous cycling (see `datasets.dart`'s [s3SettleGapMs] doc).** The
/// sequence repeats — after `clear` closes and its settle gap elapses, the
/// script restarts from `create1k` — for the whole time this widget stays
/// mounted, rather than running once and going idle.
library;

import 'dart:async';

import 'package:flutter/material.dart';

import '../bench/datasets.dart';
import '../bench/perf.dart';

/// One table row — the classic jsfb `{id, label}`.
class _Row {
  _Row(this.id, this.label);
  final int id;
  String label;
}

const List<String> _adjectives = [
  'pretty', 'large', 'big', 'small', 'tall', 'short', 'long', 'handsome', //
  'plain', 'quaint', 'clean', 'elegant', 'easy', 'angry', 'crazy', 'helpful', //
];
const List<String> _colours = [
  'red', 'yellow', 'blue', 'green', 'pink', 'brown', 'purple', 'white', //
];
const List<String> _nouns = [
  'table', 'chair', 'house', 'bbq', 'desk', 'car', 'pony', 'cookie', //
  'sandwich', 'burger', 'pizza', 'mouse', 'keyboard', //
];

class TableOpsView extends StatefulWidget {
  const TableOpsView({super.key});

  @override
  State<TableOpsView> createState() => _TableOpsViewState();
}

class _TableOpsViewState extends State<TableOpsView> {
  final List<_Row> _rows = [];
  int _nextId = 1;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _runScript());
  }

  /// Run the op sequence, stamping each op's sub-marker and waiting for the
  /// resulting frame to settle before the next op so per-op windows don't
  /// overlap — then, once `clear` closes and its settle gap elapses, restart
  /// from `create1k` and repeat for as long as this widget stays mounted (see
  /// the module doc's continuous-cycling contract).
  Future<void> _runScript() async {
    while (mounted) {
      await _op('s3-create1k', () => _append(1000));
      if (!mounted) return;
      await _op('s3-create10k', () {
        _rows.clear();
        _nextId = 1;
        _append(10000);
      });
      if (!mounted) return;
      await _op('s3-update', _updateEveryTenth);
      if (!mounted) return;
      await _op('s3-swap', _swapRows);
      if (!mounted) return;
      await _op('s3-clear', _rows.clear);
      // `_op`'s trailing settle gap (below) already provides the post-clear
      // pause before the next cycle's `create1k` — the loop simply restarts.
    }
  }

  Future<void> _op(String marker, VoidCallback mutate) async {
    if (!mounted) return;
    markScenarioStart(marker);
    setState(mutate);
    await _nextFrame();
    markScenarioEnd(marker);
    // Small settle gap so the op windows are cleanly separable in the trace
    // (also serves as the post-clear, pre-next-cycle pause — see the module
    // doc's continuous-cycling contract).
    await Future<void>.delayed(const Duration(milliseconds: s3SettleGapMs));
  }

  Future<void> _nextFrame() {
    final completer = Completer<void>();
    WidgetsBinding.instance.addPostFrameCallback((_) => completer.complete());
    return completer.future;
  }

  void _append(int count) {
    for (var i = 0; i < count; i++) {
      final id = _nextId++;
      _rows.add(_Row(id, _label(id)));
    }
  }

  String _label(int id) =>
      '${_adjectives[id % _adjectives.length]} '
      '${_colours[id % _colours.length]} '
      '${_nouns[id % _nouns.length]}';

  void _updateEveryTenth() {
    for (var i = 0; i < _rows.length; i += 10) {
      _rows[i].label = '${_rows[i].label} !!!';
    }
  }

  void _swapRows() {
    if (_rows.length < 999) return;
    final tmp = _rows[1];
    _rows[1] = _rows[998];
    _rows[998] = tmp;
  }

  @override
  Widget build(BuildContext context) {
    return ListView.builder(
      itemCount: _rows.length,
      itemExtent: 40,
      itemBuilder: (context, index) {
        final row = _rows[index];
        return _TableRow(id: row.id, label: row.label);
      },
    );
  }
}

class _TableRow extends StatelessWidget {
  const _TableRow({required this.id, required this.label});

  final int id;
  final String label;

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      height: 40,
      child: Row(
        children: [
          SizedBox(
            width: 64,
            child: Text('$id', textAlign: TextAlign.right),
          ),
          const SizedBox(width: 16),
          Expanded(child: Text(label)),
        ],
      ),
    );
  }
}
