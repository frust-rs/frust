//! Script behaviour through the public API: Thai dictionary line breaking and word motion,
//! Vietnamese grapheme steps and space-only breaks, and Japanese / Chinese line-start rules.
//!
//! Only the Thai face covers its test text, so the strict width and zero-`.notdef` checks
//! apply to Thai alone. The Vietnamese, Japanese and Chinese tests assert properties that do not
//! depend on which face shapes the text: line-start byte offsets, break positions, and
//! grapheme and word stops. No test reads a host font on purpose.

use frust_text::{EditOp, EditingStateBytes, TextContext, TextEditor, TextStyle};

const THAI_FACE: &[u8] = include_bytes!("../../../testing/fonts/NotoSansThai-Subset.ttf");
const LATIN_FACE: &[u8] = include_bytes!("../../../testing/fonts/NotoSans-Subset.ttf");
const JP_FACE: &[u8] = include_bytes!("../../../testing/fonts/NotoSansJP-Subset.otf");

/// A context holding the bundled faces.
fn context() -> TextContext {
    let mut cx = TextContext::new();
    for face in [THAI_FACE, LATIN_FACE, JP_FACE] {
        cx.register_fonts(face.to_vec())
            .expect("bundled face registers");
    }
    cx
}

fn style(locale: Option<&str>) -> TextStyle {
    TextStyle::default().with_size(20.0).with_locale(locale)
}

/// An editor over `text` wrapped at `width`, caret at 0.
fn editor(cx: &mut TextContext, text: &str, style: &TextStyle, width: Option<f32>) -> TextEditor {
    let mut ed = TextEditor::new(style);
    ed.set_wrap_width(width, cx);
    ed.apply(EditOp::Insert(text.to_owned()), cx);
    set_caret(&mut ed, cx, 0);
    ed
}

fn set_caret(ed: &mut TextEditor, cx: &mut TextContext, at: usize) {
    let text = ed.text().to_owned();
    ed.apply(
        EditOp::ApplyEditingState(EditingStateBytes {
            text,
            base: at,
            extent: at,
            composing: None,
        }),
        cx,
    );
}

fn caret(ed: &TextEditor) -> usize {
    ed.editing_state_bytes().extent
}

/// Byte offset at which each visual line starts, found by walking Down then Home.
fn line_starts(cx: &mut TextContext, ed: &mut TextEditor) -> Vec<usize> {
    set_caret(ed, cx, 0);
    let mut starts = vec![0];
    for _ in 0..ed.line_count() {
        ed.apply(EditOp::MoveDown { select: false }, cx);
        ed.apply(EditOp::Home { select: false }, cx);
        let at = caret(ed);
        if starts.last() == Some(&at) {
            break;
        }
        starts.push(at);
    }
    starts
}

/// Byte offsets the caret stops at moving right by word from 0 until it stops moving.
fn word_stops(cx: &mut TextContext, ed: &mut TextEditor) -> Vec<usize> {
    set_caret(ed, cx, 0);
    let mut stops = Vec::new();
    for _ in 0..ed.text().len() + 1 {
        let before = caret(ed);
        ed.apply(EditOp::MoveWordRight { select: false }, cx);
        let at = caret(ed);
        if at == before {
            break;
        }
        stops.push(at);
    }
    stops
}

/// Offsets of one grapheme per `MoveRight` from 0.
fn grapheme_steps(cx: &mut TextContext, ed: &mut TextEditor) -> Vec<usize> {
    set_caret(ed, cx, 0);
    let mut offsets = vec![0];
    for _ in 0..ed.text().len() {
        ed.apply(EditOp::MoveRight { select: false }, cx);
        let at = caret(ed);
        if offsets.last() == Some(&at) {
            break;
        }
        offsets.push(at);
    }
    offsets
}

/// The character starting at byte `at`.
fn char_at(text: &str, at: usize) -> char {
    text[at..].chars().next().expect("offset inside the text")
}

#[cfg(feature = "complex-scripts")]
fn is_thai_mark(c: char) -> bool {
    matches!(c, '\u{0E31}' | '\u{0E34}'..='\u{0E3A}' | '\u{0E47}'..='\u{0E4E}')
}

#[cfg(feature = "complex-scripts")]
#[test]
fn thai_unspaced_text_wraps_between_words() {
    let mut cx = context();
    let text = "ภาษาไทยไม่มีการเว้นวรรคระหว่างคำ";
    let st = style(None);
    let natural = cx.layout(text, &st, None).size().width as f32;
    let max = natural / 3.0;
    assert_eq!(
        cx.notdef_glyph_count(),
        0,
        "the Thai face covers the sentence"
    );

    let wrapped = cx.layout(text, &st, Some(max));
    assert!(
        wrapped.size().width <= f64::from(max) + 0.01,
        "wrapped width {} exceeds {max}",
        wrapped.size().width
    );

    let mut ed = editor(&mut cx, text, &st, Some(max));
    assert!(ed.line_count() > 1, "unspaced Thai must wrap");
    let starts = line_starts(&mut cx, &mut ed);
    assert!(starts.len() > 1);
    let mut unwrapped = editor(&mut cx, text, &st, None);
    let stops = word_stops(&mut cx, &mut unwrapped);
    for &start in &starts[1..] {
        assert!(text.is_char_boundary(start));
        let c = char_at(text, start);
        assert!(
            !is_thai_mark(c),
            "line starts on a combining mark at {start}"
        );
        assert!(
            stops.contains(&start),
            "line start {start} is not a word boundary: {stops:?}"
        );
    }
    assert_eq!(cx.notdef_glyph_count(), 0);
}

#[cfg(feature = "complex-scripts")]
#[test]
fn thai_word_motion_stops_inside_the_run() {
    let mut cx = context();
    let text = "ภาษาไทยไม่มีการเว้นวรรคระหว่างคำ";
    let mut ed = editor(&mut cx, text, &style(None), None);
    let stops = word_stops(&mut cx, &mut ed);
    let inside = stops.iter().filter(|&&s| s > 0 && s < text.len()).count();
    assert!(
        inside >= 2,
        "word motion stops {stops:?} should fall inside the run"
    );
    for &s in &stops {
        assert!(text.is_char_boundary(s));
        if s < text.len() {
            assert!(!is_thai_mark(char_at(text, s)), "stop {s} splits a cluster");
        }
    }
}

#[cfg(not(feature = "complex-scripts"))]
#[test]
fn thai_unspaced_text_overflows_without_complex_scripts() {
    let mut cx = context();
    let text = "ภาษาไทยไม่มีการเว้นวรรคระหว่างคำ";
    let st = style(None);
    let natural = cx.layout(text, &st, None).size().width as f32;
    let max = natural / 3.0;

    let bounded = cx.layout(text, &st, Some(max));
    assert!(
        bounded.size().width > f64::from(max),
        "without dictionary segmentation the run overflows {max}, got {}",
        bounded.size().width
    );
    let ed = editor(&mut cx, text, &st, Some(max));
    assert_eq!(ed.line_count(), 1, "the unbreakable run stays on one line");
}

#[test]
fn vietnamese_breaks_only_at_spaces() {
    let mut cx = context();
    let text = "Ti\u{1EBF}ng Vi\u{1EC7}t c\u{00F3} d\u{1EA5}u";
    let st = style(None);
    for width in (10..=300).step_by(10) {
        let mut ed = editor(&mut cx, text, &st, Some(width as f32));
        for &start in &line_starts(&mut cx, &mut ed)[1..] {
            assert_eq!(
                text.as_bytes()[start - 1],
                b' ',
                "width {width}: line starts at {start}, not after a space"
            );
        }
    }
}

#[test]
fn vietnamese_move_right_steps_one_grapheme() {
    let mut cx = context();
    // Precomposed U+1EBF, a space, then decomposed e + U+0302 + U+0301.
    let text = "\u{1EBF} e\u{0302}\u{0301}";
    let mut ed = editor(&mut cx, text, &style(None), None);
    assert_eq!(grapheme_steps(&mut cx, &mut ed), vec![0, 3, 4, 9]);
}

#[test]
fn vietnamese_word_motion_stops_at_spaces() {
    let mut cx = context();
    let text = "Ti\u{1EBF}ng Vi\u{1EC7}t c\u{00F3} d\u{1EA5}u";
    let mut ed = editor(&mut cx, text, &style(None), None);
    let stops = word_stops(&mut cx, &mut ed);
    assert!(stops.len() >= 3, "{stops:?}");
    for &s in &stops {
        let bytes = text.as_bytes();
        let at_space = s == text.len() || bytes[s] == b' ' || bytes[s - 1] == b' ';
        assert!(at_space, "stop {s} is inside a word");
    }
}

/// Line starts of `text` at every width in a sweep.
fn swept_line_starts(text: &str, locale: &str) -> Vec<(f32, Vec<usize>)> {
    let mut cx = context();
    let st = style(Some(locale));
    (1..=12)
        .map(|n| {
            let width = n as f32 * 10.0;
            let mut ed = editor(&mut cx, text, &st, Some(width));
            (width, line_starts(&mut cx, &mut ed))
        })
        .collect()
}

#[test]
fn japanese_line_starts_obey_kinsoku() {
    let text = "今日は、天気がいい。明日も、晴れ。";
    let mut multi_line = false;
    for (width, starts) in swept_line_starts(text, "ja") {
        multi_line |= starts.len() > 1;
        for &s in &starts[1..] {
            let c = char_at(text, s);
            assert!(
                !matches!(c, '。' | '、'),
                "width {width}: line starts with {c}"
            );
        }
    }
    assert!(multi_line, "the sweep never wrapped");
}

#[test]
fn chinese_breaks_between_ideographs_and_keeps_punctuation_off_line_starts() {
    let text = "我们是朋友，欢迎来到这里。今天天气很好。";
    let mut between_ideographs = false;
    for (width, starts) in swept_line_starts(text, "zh-Hans") {
        for &s in &starts[1..] {
            let c = char_at(text, s);
            assert!(
                !matches!(c, '，' | '。'),
                "width {width}: line starts with {c}"
            );
            let prev = text[..s].chars().next_back().unwrap();
            between_ideographs |= !matches!(prev, '，' | '。') && !matches!(c, '，' | '。');
        }
    }
    assert!(between_ideographs, "no break fell between two ideographs");
}
