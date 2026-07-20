/// S6 — text shaping stress. A fixed multilingual corpus (Latin, CJK, RTL
/// Arabic/Hebrew, Devanagari) re-laid-out every frame under a width animation:
/// animating the text column's width forces a full reshape/reflow each frame,
/// stressing the text pipeline (Parley on the frust side, the engine's shaper
/// on the Flutter side). Same corpus, same width timeline both apps.
library;

import 'package:flutter/material.dart';

/// The embedded corpus — identical bytes on both sides (the fairness gate).
/// Mixed scripts, long paragraphs, so shaping + bidi + line-breaking all run.
const String _corpus =
    'The quick brown fox jumps over the lazy dog while contemplating the '
    'nature of typography, line breaking, and the subtle art of glyph '
    'positioning across scripts and writing systems.\n\n'
    '请注意，这段文字混合了多种语言与书写系统，用于压力测试文本整形与换行算法的性能表现，'
    '包括中文、日文以及韩文的排版。日本語のテキストもここに含まれています。'
    '한국어 텍스트도 여기에 포함됩니다。\n\n'
    'هذا نص عربي يُكتب من اليمين إلى اليسار لاختبار خوارزميات الاتجاه الثنائي '
    'وتشكيل الحروف المتصلة في محرك عرض النصوص. '
    'טקסט עברי נכתב אף הוא מימין לשמאל לבדיקת אלגוריתמי הכיווניות.\n\n'
    'यह देवनागरी लिपि में लिखा गया एक अनुच्छेद है जो जटिल संयुक्ताक्षरों और '
    'मात्राओं के साथ पाठ आकार देने की प्रक्रिया का परीक्षण करता है।';

class TextStressView extends StatefulWidget {
  const TextStressView({super.key});

  @override
  State<TextStressView> createState() => _TextStressViewState();
}

class _TextStressViewState extends State<TextStressView>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller;
  late final Animation<double> _width;

  @override
  void initState() {
    super.initState();
    _controller = AnimationController(
      vsync: this,
      duration: const Duration(seconds: 3),
    )..repeat(reverse: true);
    // Fraction of the available width, oscillating 0.4 → 1.0 — each value forces
    // a reflow of the whole corpus.
    _width = Tween<double>(begin: 0.4, end: 1.0).animate(
      CurvedAnimation(parent: _controller, curve: Curves.easeInOut),
    );
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        return Center(
          child: AnimatedBuilder(
            animation: _width,
            builder: (context, child) {
              return SizedBox(
                width: constraints.maxWidth * _width.value,
                child: child,
              );
            },
            child: const SingleChildScrollView(
              padding: EdgeInsets.all(16),
              child: Text(
                _corpus,
                style: TextStyle(fontSize: 18, height: 1.4),
              ),
            ),
          ),
        );
      },
    );
  }
}
