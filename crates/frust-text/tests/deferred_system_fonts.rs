//! Integration tests for the deferred system-font context:
//! [`frust_text::TextContext::new_deferred`] and the adoption of its
//! background-built [`frust_text::SystemCollection`].
//!
//! Every leg runs inside ONE test function, in order. The app-font and
//! generic-fallback records are process-wide and append-only, so the
//! not-deferrable leg (nothing registered) must run before any registration;
//! a second test registering concurrently could flip it.

use std::time::{Duration, Instant};

use frust_text::{
    FamilyName, FontFamily, GenericSlot, SystemCollection, TextContext, TextStyle,
    register_generic_fallback,
};
use kurbo::Point;
use peniko::Color;

/// The registered test font (see `tests/register_fonts.rs` for provenance).
const TUFFY: &[u8] = include_bytes!("fonts/Tuffy-Subset.ttf");

/// The same face renamed "Helvetica" — a second, distinguishable face for the
/// generic-fallback mapping.
const TUFFY_AS_HELVETICA: &[u8] = include_bytes!("fonts/Tuffy-As-Helvetica.ttf");

/// Latin text the Tuffy subset covers.
const LATIN: &str = "Hello Frust 0123";

fn style(family: FontFamily) -> TextStyle {
    TextStyle {
        family,
        ..TextStyle::new(24.0, Color::BLACK)
    }
}

/// A bare generic-family stack.
fn generic(slot: GenericSlot) -> FontFamily {
    FontFamily::NamedWithGeneric(vec![FamilyName::Generic(slot)])
}

/// One shaped run: its face bytes, and per glyph its id and position.
type Run = (Vec<u8>, Vec<(u32, f32, f32)>);

/// Everything shaping decided for `text` (positions carry the advances).
#[derive(Debug, PartialEq)]
struct Shaped {
    runs: Vec<Run>,
    width: f64,
}

fn shape(cx: &mut TextContext, text: &str, sty: &TextStyle) -> Shaped {
    let layout = cx.layout(text, sty, None);
    let runs = layout
        .to_scene_runs(Point::ORIGIN)
        .into_iter()
        .map(|run| {
            let glyphs = run.glyphs.iter().map(|g| (g.id, g.x, g.y)).collect();
            (run.font.font().data.as_ref().to_vec(), glyphs)
        })
        .collect();
    Shaped {
        runs,
        width: layout.size().width,
    }
}

/// The face bytes of every run.
fn faces(shaped: &Shaped) -> Vec<&[u8]> {
    shaped
        .runs
        .iter()
        .map(|(face, _)| face.as_slice())
        .collect()
}

/// Polls [`TextContext::adopt_system_collection`] the way a shell's frame
/// loop does, until it adopts.
fn poll_until_adopted(cx: &mut TextContext, pending: &mut Option<SystemCollection>) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while !cx.adopt_system_collection(pending) {
        assert!(
            Instant::now() < deadline,
            "the system collection never finished building"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn deferred_context_shapes_app_fonts_then_adopts_the_system_collection() {
    not_deferrable_context_adopts_blocking_and_shapes_like_new();
    deferrable_context_keeps_its_glyphs_across_adoption();
}

/// Nothing registered: the gate says "not deferrable", and the blocking
/// adoption is today's context.
fn not_deferrable_context_adopts_blocking_and_shapes_like_new() {
    let (mut deferred, pending) = TextContext::new_deferred();
    assert!(!deferred.has_system_fonts());
    assert!(
        !deferred.is_deferrable(),
        "no app font registered: the context must not be deferrable"
    );

    let mut pending = Some(pending);
    assert!(deferred.adopt_system_collection_blocking(&mut pending));
    assert!(pending.is_none());
    assert!(deferred.has_system_fonts());
    assert!(deferred.system_font_build_time().is_some());
    assert!(
        !deferred.adopt_system_collection_blocking(&mut pending),
        "nothing left to adopt"
    );

    let mut plain = TextContext::new();
    for family in [FontFamily::SystemUi, generic(GenericSlot::Monospace)] {
        let sty = style(family);
        assert_eq!(
            shape(&mut deferred, LATIN, &sty),
            shape(&mut plain, LATIN, &sty),
            "a blocking-adopted context must shape exactly like TextContext::new"
        );
    }
}

/// An app font registered: the systemless window shapes the registered family
/// and the generic slots with it, and adoption changes neither the registered
/// family's glyphs nor an explicitly mapped generic's.
fn deferrable_context_keeps_its_glyphs_across_adoption() {
    let (mut deferred, pending) = TextContext::new_deferred();
    let mut pending = Some(pending);
    let families = deferred
        .register_fonts(TUFFY.to_vec())
        .expect("valid TTF bytes must register");
    assert_eq!(families[0].name, "Tuffy");
    assert!(deferred.is_deferrable());
    assert!(!deferred.has_system_fonts());

    // An explicit generic mapping, recorded process-wide and picked up on sync.
    register_generic_fallback(TUFFY_AS_HELVETICA.to_vec(), &[GenericSlot::Serif]);
    assert!(deferred.sync_app_fonts());

    let named = style(FontFamily::named("Tuffy"));
    let mapped = style(generic(GenericSlot::Serif));
    let before_named = shape(&mut deferred, LATIN, &named);
    let before_mapped = shape(&mut deferred, LATIN, &mapped);
    assert_eq!(faces(&before_named), vec![TUFFY]);
    assert_eq!(faces(&before_mapped), vec![TUFFY_AS_HELVETICA]);
    assert!(
        before_named.runs[0].1.iter().all(|&(id, ..)| id != 0),
        "the registered family must cover the Latin text"
    );

    // The window maps the app family onto the unmapped generic tails too, so a
    // theme style ending in one still resolves instead of shaping nothing.
    for slot in [
        GenericSlot::SystemUi,
        GenericSlot::SansSerif,
        GenericSlot::Monospace,
    ] {
        let shaped = shape(&mut deferred, LATIN, &style(generic(slot)));
        assert_eq!(faces(&shaped), vec![TUFFY], "{slot:?} before adoption");
    }

    // The GO path's limitation: a character no registered face covers has no
    // fallback before adoption.
    let cjk = shape(&mut deferred, "日本", &style(FontFamily::SystemUi));
    assert!(
        cjk.runs
            .iter()
            .flat_map(|(_, glyphs)| glyphs)
            .all(|&(id, ..)| id == 0),
        "nothing covers CJK before adoption: {cjk:?}"
    );

    poll_until_adopted(&mut deferred, &mut pending);
    assert!(pending.is_none());
    assert!(deferred.has_system_fonts());
    assert!(!deferred.is_deferrable());
    assert!(deferred.system_font_build_time().is_some());
    assert!(
        !deferred.adopt_system_collection(&mut pending),
        "adoption reports true exactly once"
    );

    assert_eq!(
        shape(&mut deferred, LATIN, &named),
        before_named,
        "the registered family must keep its glyph ids and advances across adoption"
    );
    assert_eq!(
        shape(&mut deferred, LATIN, &mapped),
        before_mapped,
        "an explicitly mapped generic must keep its face across adoption"
    );

    // The window-only generic mappings are gone: an unmapped generic now
    // resolves the way a full context resolves it.
    let mut plain = TextContext::new();
    let system_ui = style(FontFamily::SystemUi);
    assert_eq!(
        shape(&mut deferred, LATIN, &system_ui),
        shape(&mut plain, LATIN, &system_ui),
        "after adoption SystemUi must resolve like TextContext::new"
    );
}
