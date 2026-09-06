/// S2 — long-list scroll. A 10k-row `ListView.builder` (text + generated
/// thumbnail + icons), driven by a deterministic scripted scroll timeline so
/// the harness measures frames, not gestures. Idiomatic Flutter: `ListView.
/// builder` virtualization, `const` where possible, `RepaintBoundary` per row.
///
/// The scripted timeline (fixed, seed-free — it is the same wall-clock schedule
/// both apps replay): a steady downward scroll, then a fast fling to the end,
/// then a steady scroll back — matching the frust side's in-app scroll driver.
library;

import 'package:flutter/material.dart';

const int _rowCount = 10000;

/// Row thumbnail palette — deterministic per row index (index → hue), so both
/// apps show identical content without shipping image assets.
Color _thumbColor(int index) {
  final hue = (index * 137) % 360; // golden-angle stepping for visible variety
  return HSVColor.fromAHSV(1.0, hue.toDouble(), 0.55, 0.85).toColor();
}

const List<IconData> _trailingIcons = [
  Icons.star,
  Icons.favorite,
  Icons.bookmark,
  Icons.flag,
  Icons.check_circle,
];

class LongListView extends StatefulWidget {
  const LongListView({super.key});

  @override
  State<LongListView> createState() => _LongListViewState();
}

class _LongListViewState extends State<LongListView> {
  final ScrollController _controller = ScrollController();

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _runScript());
  }

  /// The scripted scroll timeline — deterministic durations/curves both apps
  /// share. Guards every step against disposal (a 30s harness window may end
  /// mid-script).
  Future<void> _runScript() async {
    if (!_controller.hasClients) return;
    final max = _controller.position.maxScrollExtent;
    // 1) steady scroll a quarter of the way.
    await _animateTo(max * 0.25,
        const Duration(seconds: 4), Curves.linear);
    // 2) fast fling to the end.
    await _animateTo(max, const Duration(milliseconds: 900), Curves.easeOut);
    // 3) steady scroll back to the top.
    await _animateTo(0, const Duration(seconds: 6), Curves.linear);
  }

  Future<void> _animateTo(double offset, Duration duration, Curve curve) async {
    if (!mounted || !_controller.hasClients) return;
    await _controller.animateTo(offset, duration: duration, curve: curve);
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return ListView.builder(
      controller: _controller,
      itemCount: _rowCount,
      itemExtent: 72,
      itemBuilder: (context, index) => _ListRow(index: index),
    );
  }
}

class _ListRow extends StatelessWidget {
  const _ListRow({required this.index});

  final int index;

  @override
  Widget build(BuildContext context) {
    return RepaintBoundary(
      child: ListTile(
        leading: Container(
          width: 48,
          height: 48,
          decoration: BoxDecoration(
            color: _thumbColor(index),
            borderRadius: BorderRadius.circular(8),
          ),
          child: Center(
            child: Text('$index',
                style: const TextStyle(
                    color: Colors.white, fontWeight: FontWeight.bold)),
          ),
        ),
        title: Text('Row $index — long-list scroll benchmark',
            maxLines: 1, overflow: TextOverflow.ellipsis),
        subtitle: Text('Deterministic content for row number $index',
            maxLines: 1, overflow: TextOverflow.ellipsis),
        trailing: Icon(_trailingIcons[index % _trailingIcons.length]),
      ),
    );
  }
}
