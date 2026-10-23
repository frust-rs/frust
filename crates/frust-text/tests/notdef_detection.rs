//! `.notdef` detection: visible characters shaped to glyph 0 are counted and
//! warned about once per font. System fonts are OFF in every case so the host's
//! Han fonts can never satisfy a layout.

use std::sync::Mutex;

use frust_text::{layout_to_scene_runs, notdef_glyph_count};
use kurbo::Point;
use parley::fontique::{Blob, Collection, CollectionOptions};
use parley::{FontContext, LayoutContext, StyleProperty};
use peniko::Brush;

const NOTO_SANS: &[u8] = include_bytes!("../../../testing/fonts/NotoSans-Subset.ttf");
const NOTO_JP: &[u8] = include_bytes!("../../../testing/fonts/NotoSansJP-Subset.otf");

struct Capture;
static WARNS: Mutex<Vec<String>> = Mutex::new(Vec::new());

impl log::Log for Capture {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Warn
    }
    fn log(&self, r: &log::Record) {
        if r.level() == log::Level::Warn {
            WARNS.lock().unwrap().push(r.args().to_string());
        }
    }
    fn flush(&self) {}
}

fn lay_out(font_cx: &mut FontContext, family: &str, text: &str) {
    let mut layout_cx: LayoutContext<Brush> = LayoutContext::new();
    let mut builder = layout_cx.ranged_builder(font_cx, text, 1.0, true);
    builder.push_default(StyleProperty::FontSize(16.0));
    builder.push_default(StyleProperty::FontFamily(parley::FontFamily::named(family)));
    let mut layout = builder.build(text);
    layout.break_all_lines(None);
    let _ = layout_to_scene_runs(&layout, Point::ORIGIN);
}

fn systemless(fonts: &[&[u8]]) -> (FontContext, Vec<String>) {
    let mut cx = FontContext {
        collection: Collection::new(CollectionOptions {
            system_fonts: false,
            ..Default::default()
        }),
        source_cache: Default::default(),
    };
    let mut names = Vec::new();
    for f in fonts {
        for (id, _) in cx.collection.register_fonts(Blob::from(f.to_vec()), None) {
            names.push(cx.collection.family_name(id).unwrap().to_string());
        }
    }
    (cx, names)
}

#[test]
fn notdef_counts_and_warns_once() {
    log::set_logger(&Capture).unwrap();
    log::set_max_level(log::LevelFilter::Warn);

    // Uncovered: Latin-only face, two Han characters. The subset covers only
    // " ,Helo\u{e9}" (no a-d), so the Latin around the Han uses those letters.
    let (mut cx, names) = systemless(&[NOTO_SANS]);
    let family = names[0].clone();
    let before = notdef_glyph_count();
    lay_out(&mut cx, &family, "He中文 lo");
    assert_eq!(
        notdef_glyph_count() - before,
        2,
        "two Han glyphs are .notdef"
    );
    assert_eq!(WARNS.lock().unwrap().len(), 1, "first layout warns");
    {
        let w = WARNS.lock().unwrap();
        assert!(
            w[0].contains("中文"),
            "warning names affected chars: {}",
            w[0]
        );
    }

    // Same layout again: counted again, but warned only once per font.
    lay_out(&mut cx, &family, "He中文 lo");
    assert_eq!(notdef_glyph_count() - before, 4);
    assert_eq!(WARNS.lock().unwrap().len(), 1, "warn fires once per font");

    // Whitespace only: no count.
    let mid = notdef_glyph_count();
    lay_out(&mut cx, &family, "He llo");
    assert_eq!(notdef_glyph_count(), mid, "whitespace is not .notdef");

    // Covered: JP face with its own Han glyphs.
    let (mut jp_cx, jp_names) = systemless(&[NOTO_JP]);
    lay_out(&mut jp_cx, &jp_names[0], "日本語");
    assert_eq!(notdef_glyph_count(), mid, "covered Han does not count");
    assert_eq!(WARNS.lock().unwrap().len(), 1);
}
