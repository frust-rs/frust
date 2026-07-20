/// S5 — image pipeline. A scrolling stream of large images, each decoded off
/// the UI thread via Flutter's async image pipeline (`ui.decodeImageFromPixels`)
/// from the SAME deterministic bytes the frust side uses (see `datasets.dart`'s
/// `generateS5ImageRgba` parity contract). Tests off-thread decode + composite
/// under scroll — PLAN 9.E's `decode_async` claim.
///
/// The pixel source is byte-identical across both apps (same SplitMix64 seed +
/// generator params). We feed raw RGBA straight into the engine's async pixel
/// decode, so no cross-language PNG encoder sits on the fairness path.
library;

import 'dart:async';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';

import '../bench/datasets.dart';

class ImagePipelineView extends StatefulWidget {
  const ImagePipelineView({super.key});

  @override
  State<ImagePipelineView> createState() => _ImagePipelineViewState();
}

class _ImagePipelineViewState extends State<ImagePipelineView> {
  final ScrollController _controller = ScrollController();

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _runScript());
  }

  /// Deterministic scroll timeline (shared with the frust side): steady scroll
  /// down the stream, then back up, so decode happens continuously under motion.
  Future<void> _runScript() async {
    if (!_controller.hasClients) return;
    while (mounted && _controller.hasClients) {
      await _animateTo(_controller.position.maxScrollExtent,
          const Duration(seconds: 10), Curves.linear);
      await _animateTo(0, const Duration(seconds: 10), Curves.linear);
    }
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
    // A long virtualized stream cycling the generated image set.
    return ListView.builder(
      controller: _controller,
      itemCount: s5ImageCount * 8,
      itemExtent: s5ImageSize.toDouble() + 16,
      itemBuilder: (context, index) => _ImageCell(index: index % s5ImageCount),
    );
  }
}

class _ImageCell extends StatefulWidget {
  const _ImageCell({required this.index});

  final int index;

  @override
  State<_ImageCell> createState() => _ImageCellState();
}

class _ImageCellState extends State<_ImageCell> {
  ui.Image? _image;

  @override
  void initState() {
    super.initState();
    _decode();
  }

  Future<void> _decode() async {
    final rgba = generateS5ImageRgba(widget.index);
    final completer = Completer<ui.Image>();
    ui.decodeImageFromPixels(
      rgba,
      s5ImageSize,
      s5ImageSize,
      ui.PixelFormat.rgba8888,
      completer.complete,
    );
    final image = await completer.future;
    if (!mounted) {
      image.dispose();
      return;
    }
    setState(() => _image = image);
  }

  @override
  void dispose() {
    _image?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final image = _image;
    return Padding(
      padding: const EdgeInsets.all(8),
      child: SizedBox(
        width: s5ImageSize.toDouble(),
        height: s5ImageSize.toDouble(),
        child: image == null
            ? const ColoredBox(color: Color(0xFF202020))
            : RawImage(image: image, fit: BoxFit.cover),
      ),
    );
  }
}
