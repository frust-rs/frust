//! Glyphs of an `hvgl`-only face, drawn from CoreText's outlines.
//!
//! Apple ships some system faces — PingFang, the iOS Han fallback, is the one
//! that matters — with their outlines only in an `hvgl` table and no `glyf`,
//! `CFF ` or `CFF2` table at all. `skrifa` cannot read `hvgl`, so `glifo`
//! finds no outline for any glyph of such a face and silently skips it: the
//! text comes out blank and is not even counted as skipped. CoreText can read
//! it, so a run in such a face never reaches `glifo`. Instead each glyph's
//! outline is asked of CoreText (`CTFontCreatePathForGlyph`) once, kept as a
//! `kurbo::BezPath` in font units, and filled per frame through the engine's
//! own glyph sink — the path every outline glyph takes, under the transform
//! `glifo` builds for an unhinted outline glyph ([`glyph_transform`]).
//!
//! # Which faces
//!
//! [`font_needs_hvgl_provider`] answers per face, from its table directory: an
//! `hvgl` table and none of the three outline tables `skrifa` reads. A face
//! carrying both keeps the ordinary route, since `glifo` can draw it. Such a
//! run is classified like any other and, when the atlas takes it, inserted
//! frust-side ([`super::hvgl_atlas`]): this module then supplies the outline
//! each new glyph is rasterized from, and the outline a glyph the atlas
//! refused is filled from. Off that route (`SceneCompiler::set_hvgl_atlas`,
//! or a face or brush the insert cannot take) every glyph is filled per frame
//! as below.
//!
//! # Opening the face in CoreText
//!
//! By the PostScript name the run's handle carries
//! ([`FontHandle::postscript_name`](frust_scene::FontHandle::postscript_name)):
//! the text tier's font oracle already resolved which face a run draws with,
//! so this module does not re-derive the name from the blob. The two routes
//! that would open the bytes themselves do not work for the face this exists
//! for: `CTFontManagerCreateFontDescriptorsFromData` returns no descriptor at
//! all for `PingFangUI.ttc`, and the file-URL route expands named instances, so
//! its indices are not the collection's. A handle with no name cannot be
//! opened, and neither can one whose name CoreText does not honour: its glyphs
//! are counted in
//! [`CompiledFrame::skipped_glyphs`](crate::compile::CompiledFrame::skipped_glyphs),
//! as are all of them on a target without CoreText.
//!
//! # Drawability
//!
//! A run's glyph ids index the face the shaper chose, so they are drawn only
//! from that face. The face CoreText hands back must carry exactly the
//! PostScript name asked for; a substitute (a hidden, dot-prefixed face
//! CoreText cannot resolve by name comes back as another font) is a refusal,
//! cached like a face, warned once with the names escaped and counted
//! ([`HvglOutlines::identity_mismatches`]). A nameless handle is refused the
//! same way ([`HvglOutlines::nameless_faces`]). A hidden handle therefore
//! draws only when CoreText honours its name.
//!
//! A name is not enough on its own: an app can register its own copy of a
//! face under a system face's PostScript name, and a copy whose glyph ids
//! differ from the installed face's would pass by name and draw the installed
//! outlines under the wrong ids. So before a face opened by name is used, the
//! handle's bytes and the CoreText face must agree on their glyph count
//! (`maxp.numGlyphs` against `CTFontGetGlyphCount`), their `unitsPerEm` and
//! their `fvar` axis tags, in order (`FaceShape`), and on the glyph each of
//! a sample of the bytes' `cmap` codepoints maps to (the middle codepoint of
//! `CMAP_SAMPLES` ranges spread over the best Unicode subtable, against
//! `CTFontGetGlyphsForCharacters`; `cmap_disagreement`). A disagreement is a
//! refusal like the others, warned once with the name escaped (a codepoint as
//! `U+XXXX`) and counted ([`HvglOutlines::bytes_mismatches`]). The handle does
//! not say whether its bytes are a registered copy or the installed file the
//! oracle mapped the name to, so every face is checked when it is first
//! opened: two getters, one copy of the installed face's `fvar` table and one
//! character-map lookup, once per open.
//!
//! Refusals are remembered apart from the open faces, in a set of at most
//! `MAX_REFUSED_FACES` (oldest out), so no refusal evicts a live face. One
//! warning and one count go to each face while the set holds it; a refused
//! face that has aged out of the set is counted again if a run asks for it.
//!
//! # Variation instances
//!
//! PingFang's faces are variable (`wght`, plus Apple's `WDTH` and `HGHT`), and
//! a run carries the normalized coordinates its shaper chose
//! ([`FontHandle::normalized_coords`](frust_scene::FontHandle::normalized_coords)).
//! Those are F2Dot14 values after the face's `avar` mapping, one per `fvar`
//! axis; CoreText wants user values (`wght` 600). So each is mapped back
//! through the inverse of the axis's `avar` segment map, then denormalized
//! against the axis's `fvar` minimum, default and maximum (`user_value`),
//! and the face is copied with a `kCTFontVariationAttribute` dictionary naming
//! every axis (axis tag to user value). The copy carries no failure signal, so
//! the instance is read back from it (`CTFontCopyVariationAxes`,
//! `CTFontCopyVariation`) and kept only when every axis holds the value asked
//! for within the axis's precision (`unapplied_axis`). Otherwise the
//! instance is refused — warned once, counted
//! ([`HvglOutlines::variation_mismatches`]), its glyphs unavailable and
//! nothing cached under its key — never drawn from another instance. Each
//! face keeps its few most recently used instances open
//! ([`MAX_INSTANCES_PER_FACE`]). A run with no coordinates — every
//! non-variable face, and a variable one at its default — draws from the face
//! as opened, exactly as before.
//!
//! # The cache
//!
//! Outlines are retained in a least-recently-used cache bounded by entry count
//! ([`MAX_CACHED_OUTLINES`]), keyed by the open face (one per PostScript name, blob
//! id and face index) plus the glyph id and a [`VariationKey`] built
//! from the run's coordinates, so one instance's outline is never served for
//! another's. A glyph CoreText cannot supply is remembered as missing too, so a
//! refusal is paid for once.
//!
//! # Layout
//!
//! This file is the public surface: the draw entry points, [`HvglOutlines`]
//! and its counters, every capacity and [`VariationKey`]. `face` decides which
//! face a run's glyph ids are drawn from (opened by name, the checks of its
//! bytes against CoreText's face, the refusals). `outline` holds what is drawn
//! from it — each face's variation instances and the outline cache — and the
//! one bounded least-recently-used map ([`outline::Lru`]) those two and the
//! open faces are kept in; the refusals stay first in, first out. `ffi` is
//! every CoreText and CoreGraphics call and this module's only `unsafe`:
//! `CtFace`'s `Send` and `Sync` rest on Apple's documented Core Text thread
//! safety, as their SAFETY notes say, and on a target without CoreText it is
//! uninhabited. Its bindings are the `objc2` 0.3 pins shared with
//! `plugins/native-widgets` (PLUGINS_DEVELOPMENT.md).

mod face;
mod ffi;
mod outline;

use frust_scene::{FontHandle, GlyphRun};
use glifo::DrawSink as _;
use kurbo::{Affine, BezPath};
use peniko::FontData;

use self::face::FaceTable;
use self::outline::{Fetched, OutlineCache, OutlineKey, fetch};
use super::backend::EngineGlyphSink;
use super::{table_directory, table_record};

/// `hvgl`, as a table record's tag reads.
const HVGL_TAG: [u8; 4] = *b"hvgl";
/// The outline tables `skrifa` reads: a face carrying any of them is one
/// `glifo` can draw on its own.
const OUTLINE_TAGS: [[u8; 4]; 3] = [*b"glyf", *b"CFF ", *b"CFF2"];
/// One F2Dot14 unit: a normalized coordinate of 1.0.
const F2DOT14_ONE: f64 = 16384.0;
/// One 16.16 `Fixed` unit.
const FIXED_ONE: f64 = 65536.0;

/// How many glyph outlines the cache keeps.
///
/// Bounded by count rather than bytes because a glyph's cost is roughly
/// constant: PingFang's Han glyphs measured 57-133 path elements each (spike
/// h2-01), and a `kurbo::PathEl` is 56 bytes, so an entry is about 3-7.5 KiB
/// and a full cache about 3-7.5 MiB. 1024 distinct glyphs is a dense phone
/// page of Han text with room to spare; a page past it re-fetches its oldest
/// glyphs from CoreText (about 6 us each on an iPhone 13 mini) rather than
/// growing without bound.
pub(crate) const MAX_CACHED_OUTLINES: usize = 1024;

/// How many faces stay open in CoreText at once. A frame draws a handful of
/// fallback faces; one evicted is simply opened again by name.
const MAX_OPEN_FACES: usize = 16;

/// How many refused faces are remembered at once, oldest out. Kept apart from
/// the open faces, so no refusal evicts a live one.
const MAX_REFUSED_FACES: usize = 256;

/// How many variation instances of one face stay open at once. A screen uses
/// a few weights of a family (regular, medium, semibold); one evicted is
/// copied from the face again.
pub(crate) const MAX_INSTANCES_PER_FACE: usize = 8;

/// Which variation instance of a face an outline belongs to, derived from a
/// run's normalized coordinates.
///
/// Up to three coordinates — every face this module exists for: PingFang has
/// `WDTH`, `wght` and `HGHT` — are packed exactly, with their count, so two
/// distinct instances never share a key. A face with more axes keys on a
/// 64-bit hash of its coordinates, tagged by the top bit so it cannot collide
/// with a packed key. Coordinates that are all zero name the default instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) struct VariationKey(u64);

impl VariationKey {
    /// The face's default instance — no variation coordinates.
    pub(crate) const DEFAULT: Self = Self(0);

    /// The coordinates packed exactly when there are at most this many.
    const PACKED_AXES: usize = 3;

    /// The key for `coords`, normalized F2Dot14 values in `fvar` axis order.
    pub(crate) fn of(coords: &[i16]) -> Self {
        let coords = match coords.iter().rposition(|coord| *coord != 0) {
            Some(last) => &coords[..=last],
            None => return Self::DEFAULT,
        };
        if coords.len() <= Self::PACKED_AXES {
            let packed = coords.iter().fold(0_u64, |packed, coord| {
                (packed << 16) | u64::from(coord.cast_unsigned())
            });
            // The count (1..=3) sits above the 48 coordinate bits, so the key
            // is never zero and never has the hash tag bit.
            return Self(((coords.len() as u64) << 48) | packed);
        }
        // FNV-1a over the coordinates' bytes.
        let hash = coords
            .iter()
            .flat_map(|coord| coord.to_be_bytes())
            .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
            });
        Self(hash | (1 << 63))
    }
}

/// What [`HvglOutlines::outline`] answers for one glyph.
#[derive(Debug)]
pub(crate) enum GlyphOutline<'a> {
    /// The glyph's outline in font units (y up), and the face's units-per-em.
    Path {
        /// The outline.
        path: &'a BezPath,
        /// The face's `head.unitsPerEm`, which scales the outline to a size.
        units_per_em: f64,
    },
    /// A glyph with no ink: nothing to draw, nothing skipped.
    Empty,
    /// The glyph cannot be drawn — its face was refused, or CoreText has no
    /// outline for it. Counted as skipped.
    Unavailable,
}

/// Whether `font`'s face keeps its outlines only in an `hvgl` table, so its
/// glyphs must come from this module rather than from `glifo`.
pub(crate) fn font_needs_hvgl_provider(font: &FontData) -> bool {
    let data = font.data.data();
    table_directory(data, font.index).is_some_and(|directory| {
        table_record(data, directory, HVGL_TAG).is_some()
            && OUTLINE_TAGS
                .iter()
                .all(|tag| table_record(data, directory, *tag).is_none())
    })
}

/// The draw transform of one glyph of an `hvgl` run: the run's device
/// transform, the glyph's run-space position, then font units scaled to the
/// run's size with y flipped — the unhinted outline transform `glifo` builds.
pub(crate) fn glyph_transform(
    run: Affine,
    x: f32,
    y: f32,
    font_size: f32,
    units_per_em: f64,
) -> Affine {
    let scale = f64::from(font_size) / units_per_em;
    run * Affine::translate((f64::from(x), f64::from(y))) * Affine::scale_non_uniform(scale, -scale)
}

/// Fill every glyph of `run` from `outlines` into `sink`, under the run's
/// device `transform`, answering how many glyphs could not be drawn.
///
/// The glyphs are drawn at the variation instance the run's handle carries.
pub(crate) fn draw_run(
    run: &GlyphRun,
    transform: Affine,
    sink: &mut EngineGlyphSink<'_>,
    outlines: &mut HvglOutlines,
) -> u32 {
    let mut unavailable = 0_u32;
    for glyph in &run.glyphs {
        if !draw_glyph(run, transform, glyph, sink, outlines) {
            unavailable = unavailable.saturating_add(1);
        }
    }
    unavailable
}

/// Fill one glyph of `run` from `outlines` into `sink` under the run's device
/// `transform`, answering `false` when it could not be drawn (a glyph with no
/// ink draws nothing and answers `true`).
pub(crate) fn draw_glyph(
    run: &GlyphRun,
    transform: Affine,
    glyph: &frust_scene::Glyph,
    sink: &mut EngineGlyphSink<'_>,
    outlines: &mut HvglOutlines,
) -> bool {
    match outlines.outline(&run.font, glyph.id) {
        GlyphOutline::Path { path, units_per_em } => {
            sink.set_transform(glyph_transform(
                transform,
                glyph.x,
                glyph.y,
                run.font_size,
                units_per_em,
            ));
            sink.fill_path(path);
            true
        }
        GlyphOutline::Empty => true,
        GlyphOutline::Unavailable => false,
    }
}

/// The compiler's retained `hvgl` state: the faces open in CoreText and the
/// bounded outline cache.
pub(crate) struct HvglOutlines {
    /// The faces opened (or refused) most recently.
    faces: FaceTable,
    /// The outlines fetched, least recently used evicted first.
    outlines: OutlineCache,
    /// Glyphs the draw routes could not supply.
    unavailable: u64,
    /// Variation instances refused because CoreText's copy did not carry the
    /// user values asked for.
    variation_mismatches: u32,
}

impl Default for HvglOutlines {
    fn default() -> Self {
        Self::with_capacity(MAX_CACHED_OUTLINES)
    }
}

impl core::fmt::Debug for HvglOutlines {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HvglOutlines")
            .field("open_faces", &self.faces.open.len())
            .field("refused_faces", &self.faces.refused.len())
            .field("outlines", &self.outlines.len())
            .field("capacity", &self.outlines.capacity())
            .finish()
    }
}

impl HvglOutlines {
    /// A provider whose outline cache keeps at most `capacity` glyphs (at
    /// least one).
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            faces: FaceTable::default(),
            outlines: OutlineCache::new(capacity),
            unavailable: 0,
            variation_mismatches: 0,
        }
    }

    /// Whether `handle`'s face can be drawn from CoreText: opened (or opened
    /// now) by its PostScript name with no refusal. Never on a target without
    /// CoreText. A refusal is counted and warned once, as on a first draw.
    pub(crate) fn face_drawable(&mut self, handle: &FontHandle) -> bool {
        self.faces.open(handle).is_some()
    }

    /// How many glyph outlines are cached.
    pub(crate) fn cached_outlines(&self) -> usize {
        self.outlines.len()
    }

    /// How many glyph outlines the cache keeps at most.
    pub(crate) fn capacity(&self) -> usize {
        self.outlines.capacity()
    }

    /// How many faces CoreText resolved to a different PostScript name than
    /// the one asked for, since the provider was built. Each was refused.
    pub(crate) fn identity_mismatches(&self) -> u32 {
        self.faces.mismatches
    }

    /// How many faces were refused for carrying no PostScript name, since the
    /// provider was built.
    pub(crate) fn nameless_faces(&self) -> u32 {
        self.faces.nameless
    }

    /// How many faces were refused because their bytes and the CoreText face
    /// their name opened disagree (glyph count, `unitsPerEm` or `fvar` axes),
    /// since the provider was built.
    pub(crate) fn bytes_mismatches(&self) -> u32 {
        self.faces.bytes_mismatches
    }

    /// How many variation instances were refused because the copy CoreText
    /// made of a face did not carry the user values asked for (read back from
    /// the copy), since the provider was built. Each instance counts once per
    /// open face; its glyphs are unavailable rather than drawn from another
    /// instance.
    pub(crate) fn variation_mismatches(&self) -> u32 {
        self.variation_mismatches
    }

    /// How many outline fetches the cache answered without asking CoreText,
    /// since the provider was built (saturating).
    ///
    /// Counted per fetch call, not per distinct glyph: on the atlas route an
    /// absent glyph is fetched twice (measured, then rasterized into the
    /// atlas), so one new inked glyph reads as one miss and one hit while the
    /// frame's new glyphs fit the cache; an inkless glyph is only measured,
    /// and a glyph resident in the atlas never fetches at all.
    pub(crate) fn outline_hits(&self) -> u64 {
        self.outlines.hits
    }

    /// How many outline fetches asked CoreText (a cache miss), since the
    /// provider was built (saturating). A miss costs a path read and build,
    /// plus a font copy for each new variation instance. Counted per fetch
    /// call, as [`Self::outline_hits`] explains.
    pub(crate) fn outline_misses(&self) -> u64 {
        self.outlines.misses
    }

    /// How many glyphs of `hvgl` runs could not be drawn, since the provider
    /// was built (saturating): the face refused, or CoreText had no path.
    /// Added by the draw routes through [`Self::note_unavailable`].
    pub(crate) fn unavailable_glyphs(&self) -> u64 {
        self.unavailable
    }

    /// Add `count` glyphs a draw route could not supply to
    /// [`Self::unavailable_glyphs`].
    pub(crate) fn note_unavailable(&mut self, count: u32) {
        self.unavailable = self.unavailable.saturating_add(u64::from(count));
    }

    /// `glyph_id`'s outline in `handle`'s face at the variation instance its
    /// coordinates name (normalized F2Dot14 values in `fvar` axis order; empty
    /// for the default instance), fetched from CoreText on a cache miss.
    ///
    /// Coordinates on a face with no `fvar` axes name nothing to vary, and
    /// draw from the face as opened. A refused instance answers
    /// [`GlyphOutline::Unavailable`] and caches nothing under its key.
    pub(crate) fn outline(&mut self, handle: &FontHandle, glyph_id: u32) -> GlyphOutline<'_> {
        let Self {
            faces,
            outlines,
            variation_mismatches,
            ..
        } = self;
        let coords = handle.normalized_coords();
        let Some(face) = faces.open(handle) else {
            return GlyphOutline::Unavailable;
        };
        let units_per_em = face.units_per_em;
        let variation = if face.axes.is_empty() {
            VariationKey::DEFAULT
        } else {
            VariationKey::of(coords)
        };
        let key = OutlineKey {
            face: face.id,
            glyph_id,
            variation,
        };
        let fetched = outlines.get_or_fetch(key, || {
            let name = handle.postscript_name().unwrap_or_default();
            face.instance(variation, coords, name, variation_mismatches)
                .map(|instance| fetch(instance, glyph_id))
        });
        match fetched {
            Some(Fetched::Path(path)) => GlyphOutline::Path { path, units_per_em },
            Some(Fetched::Empty) => GlyphOutline::Empty,
            Some(Fetched::Missing) | None => GlyphOutline::Unavailable,
        }
    }
}

/// [`face::truncated`] for the compiler's log lines.
pub(crate) fn truncated_name(name: &str) -> &str {
    face::truncated(name)
}

#[cfg(test)]
mod tests {
    use super::face::tests::{font, head, sfnt};
    use super::*;

    #[test]
    fn only_an_hvgl_face_without_another_outline_table_needs_the_provider() {
        let hvgl_only = font(sfnt(&[(*b"head", head(1000)), (HVGL_TAG, vec![0; 8])]));
        assert!(font_needs_hvgl_provider(&hvgl_only));
        for tag in OUTLINE_TAGS {
            let both = font(sfnt(&[
                (*b"head", head(1000)),
                (HVGL_TAG, vec![0; 8]),
                (tag, vec![0; 8]),
            ]));
            assert!(!font_needs_hvgl_provider(&both), "{tag:?}");
        }
        let plain = font(sfnt(&[(*b"head", head(1000)), (*b"glyf", vec![0; 8])]));
        assert!(!font_needs_hvgl_provider(&plain));
        assert!(!font_needs_hvgl_provider(&font(b"not a font".to_vec())));
    }

    #[test]
    fn unavailable_glyphs_accumulate_and_saturate() {
        let mut outlines = HvglOutlines::with_capacity(2);
        outlines.note_unavailable(3);
        outlines.note_unavailable(4);
        assert_eq!(outlines.unavailable_glyphs(), 7);
        outlines.unavailable = u64::MAX;
        outlines.note_unavailable(1);
        assert_eq!(outlines.unavailable_glyphs(), u64::MAX);
    }

    #[test]
    fn variation_keys_separate_every_instance_and_fold_the_default() {
        assert_eq!(VariationKey::of(&[]), VariationKey::DEFAULT);
        assert_eq!(VariationKey::of(&[0, 0, 0]), VariationKey::DEFAULT);
        // Trailing zeros are default axes: the same instance.
        assert_eq!(VariationKey::of(&[0, 100]), VariationKey::of(&[0, 100, 0]));
        let keys = [
            VariationKey::of(&[100]),
            VariationKey::of(&[0, 100]),
            VariationKey::of(&[0, 0, 100]),
            VariationKey::of(&[-100]),
            VariationKey::of(&[0, -9830, 0]),
            VariationKey::of(&[0, 16384, 0]),
            VariationKey::of(&[1, 2, 3, 4]),
            VariationKey::of(&[1, 2, 3, 5]),
        ];
        for (index, key) in keys.iter().enumerate() {
            assert_ne!(*key, VariationKey::DEFAULT, "{index}");
            for other in &keys[index + 1..] {
                assert_ne!(key, other);
            }
        }
        // Packed keys never carry the hashed keys' tag bit.
        assert_eq!(VariationKey::of(&[-1, -1, -1]).0 >> 63, 0);
        assert_eq!(VariationKey::of(&[1, 2, 3, 4]).0 >> 63, 1);
    }

    #[test]
    fn a_glyph_is_scaled_to_the_run_size_with_y_flipped() {
        let transform = glyph_transform(Affine::translate((10.0, 20.0)), 5.0, 0.0, 20.0, 1000.0);
        let origin = transform * kurbo::Point::new(0.0, 0.0);
        assert_eq!((origin.x, origin.y), (15.0, 20.0));
        let up = transform * kurbo::Point::new(0.0, 1000.0);
        assert!((up.y - 0.0).abs() < 1e-9, "{up:?}");
    }
}
