//! The resolve pipeline: [`Oracle::resolve`] and [`Oracle::default_face`].
//!
//! # Order
//!
//! fontique's (and CSS's) query order — each named family in chain order,
//! then the generics, then whatever the OS cascade substituted — with frust's
//! caches in between.
//!
//! 1. **Each named family, in chain order.** A name the app registry holds is
//!    answered by its app faces only: an app family shadows the system family
//!    of the *same* name, never any other. Any other name is asked of the
//!    backend with a derived request naming only that family (plus the
//!    chain's generic slots, the base a backend falls back to), through the
//!    fallback cache keyed by that request. The first answer that is the asked
//!    family and covers the cluster wins. A covering answer from another
//!    family (the OS cascade substituting inside a family request) is a
//!    *substitute*: demoted (counted, logged once per cache fill), held back so
//!    later families stay reachable, and used only at step 4. At most
//!    [`MAX_FAMILY_PROBES`] system families are asked per resolve. App answers
//!    are never cached.
//! 2. **The generic request**: the chain's generic slots alone (`SystemUi`
//!    when it names none), cached the same way. The backend answers from the
//!    generic family and its OS cascade, then **its generic defaults** answer,
//!    re-synthesised at the request's attributes (a default is built at
//!    regular weight, so its own coordinates and hint are dropped).
//! 3. **App generic fallbacks** (`register_generic_fallback`), appended after
//!    the system's own generic answer exactly as fontique appended them.
//! 4. **The first held-back substitute**, when neither a named family nor the
//!    generic steps cover the cluster.
//!
//! Every candidate must map each of the cluster's characters in its `cmap`
//! (bar the [`coverage_exempt`] controls a shaper consumes through GSUB) and
//! carry a glyph format the request's [`RenderCapabilities`] can draw. A
//! refused face is counted and skipped, never substituted. Misses are cached
//! per key too; a cached substitute is flagged as one, so it expires with the
//! misses when a font directory changes (see [`super::cache`]).

use read_fonts::{
    FontRef, TableProvider,
    types::{NameId, Tag},
};

use super::cache::{CacheKey, CachedAnswer};
use super::registry::AppFace;
use super::{
    DrawFormats, FaceAttrs, FaceFacts, FaceKey, FaceRef, FaceSource, FallbackRequest, Oracle,
    RenderCapabilities, Synthesis, VariationSetting,
};
use crate::style::{FamilyName, FontStyle, GenericSlot};

/// Why a candidate face was not used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rejection {
    /// The bytes could not be read or parsed.
    Unloadable,
    /// The `cmap` misses at least one of the cluster's characters.
    Uncovered,
    /// No glyph format the face carries is drawable.
    Unrenderable,
}

/// Characters a face need not map for coverage: variation selectors and the
/// zero-width joiners, which the shaper consumes (the s-03 keycap case: a
/// `cmap` miss on U+FE0F is expected).
pub(crate) fn coverage_exempt(c: char) -> bool {
    matches!(c, '\u{200C}' | '\u{200D}' | '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}')
}

/// Whether `font`'s `cmap` maps every non-exempt character of `chars`.
fn covers(font: &FontRef<'_>, chars: &str) -> bool {
    let Ok(cmap) = font.cmap() else {
        return false;
    };
    chars
        .chars()
        .filter(|c| !coverage_exempt(*c))
        .all(|c| cmap.map_codepoint(c).is_some_and(|gid| gid.to_u32() != 0))
}

/// Reads a face's glyph formats and style attributes.
pub(crate) fn face_facts(font: &FontRef<'_>) -> FaceFacts {
    let has = |tag: &[u8; 4]| font.table_data(Tag::new(tag)).is_some();
    let formats = DrawFormats {
        outlines: has(b"glyf") || has(b"CFF ") || has(b"CFF2"),
        colr: has(b"COLR"),
        bitmap: has(b"CBDT") || has(b"sbix") || has(b"EBDT"),
        hvgl: has(b"hvgl"),
    };
    FaceFacts {
        formats,
        attrs: face_attrs(font),
    }
}

/// `OS/2` (else `head.macStyle`) weight/slant/width plus the variation axes —
/// fontique 0.11.1's `read_attributes`, reimplemented frust-side.
pub(crate) fn face_attrs(font: &FontRef<'_>) -> FaceAttrs {
    use read_fonts::tables::head::MacStyle;
    use read_fonts::tables::os2::SelectionFlags;

    let mut attrs = FaceAttrs::default();
    if let Ok(os2) = font.os2() {
        attrs.weight = f32::from(os2.us_weight_class());
        attrs.width = width_from_width_class(os2.us_width_class());
        let selection = os2.fs_selection();
        attrs.style = if selection.contains(SelectionFlags::ITALIC) {
            FontStyle::Italic
        } else if selection.contains(SelectionFlags::OBLIQUE) {
            let angle = font.post().ok().map(|p| p.italic_angle().to_f64() as f32);
            FontStyle::Oblique(angle)
        } else {
            FontStyle::Normal
        };
    } else if let Ok(head) = font.head() {
        let style = head.mac_style();
        if style.contains(MacStyle::ITALIC) {
            attrs.style = FontStyle::Italic;
        }
        if style.contains(MacStyle::BOLD) {
            attrs.weight = 700.0;
        }
    }
    if let Ok(axes) = font.fvar().and_then(|fvar| fvar.axes()) {
        for axis in axes {
            match &axis.axis_tag().to_be_bytes() {
                b"wght" => attrs.has_wght = true,
                b"wdth" => attrs.has_wdth = true,
                b"ital" => attrs.has_ital = true,
                b"slnt" => attrs.has_slnt = true,
                _ => {}
            }
        }
    }
    attrs
}

/// `OS/2.usWidthClass` to a width ratio (the OpenType table's mapping).
fn width_from_width_class(class: u16) -> f32 {
    match class {
        0..=1 => 0.5,
        2 => 0.625,
        3 => 0.75,
        4 => 0.875,
        5 => 1.0,
        6 => 1.125,
        7 => 1.25,
        8 => 1.5,
        _ => 2.0,
    }
}

/// The oblique angle used when the request names none, in degrees (fontique's
/// and CSS's 14°).
const DEFAULT_OBLIQUE_DEGREES: f32 = 14.0;

/// The synthesis that makes a face with `attrs` look like the requested
/// weight/slant/width — fontique 0.11.1's `FontInfo::synthesis` rule:
///
/// - width differs and the face has `wdth` → set `wdth` (percent);
/// - weight differs: a `wght` axis takes the requested weight, otherwise a
///   *heavier* request is faux bold (a lighter one is left alone);
/// - an upright face asked for italic takes `ital=1`, else `slnt`, else a faux
///   oblique; asked for oblique it takes `slnt`, else `ital=1`, else the skew.
///
/// The `slnt` value keeps fontique's positive-degrees convention for parity
/// with the goldens it produced (OpenType's `slnt` leans right for negative
/// values; the shaping adapter owns any sign correction).
pub(crate) fn synthesize(
    attrs: &FaceAttrs,
    weight: f32,
    slant: FontStyle,
    width: f32,
) -> Synthesis {
    let mut synth = Synthesis::default();
    let mut set = |tag: &[u8; 4], value: f32| {
        synth.variations.push(VariationSetting { tag: *tag, value });
    };
    if attrs.has_wdth && attrs.width != width {
        set(b"wdth", width * 100.0);
    }
    let mut embolden = false;
    if attrs.weight != weight {
        if attrs.has_wght {
            set(b"wght", weight);
        } else if weight > attrs.weight {
            embolden = true;
        }
    }
    let mut skew = None;
    if attrs.style == FontStyle::Normal {
        match slant {
            FontStyle::Normal => {}
            FontStyle::Italic => {
                if attrs.has_ital {
                    set(b"ital", 1.0);
                } else if attrs.has_slnt {
                    set(b"slnt", DEFAULT_OBLIQUE_DEGREES);
                } else {
                    skew = Some(DEFAULT_OBLIQUE_DEGREES);
                }
            }
            FontStyle::Oblique(angle) => {
                let degrees = angle.unwrap_or(DEFAULT_OBLIQUE_DEGREES);
                if attrs.has_slnt {
                    set(b"slnt", degrees);
                } else if attrs.has_ital && degrees > 0.0 {
                    set(b"ital", 1.0);
                } else {
                    skew = Some(degrees);
                }
            }
        }
    }
    synth.embolden = embolden;
    synth.skew = skew;
    synth
}

/// The final synthesis for a backend answer: the backend's hint when it gave
/// one (it knows the match it made), else the frust rule; the backend's own
/// instance coordinates go first, and a synthesised axis never overrides one.
fn synthesis_for(source: &FaceSource, attrs: &FaceAttrs, req: &FallbackRequest) -> Synthesis {
    let mut synth = match &source.synthesis_hint {
        Some(hint) => hint.clone(),
        None => synthesize(attrs, req.weight.value(), req.slant, req.width),
    };
    if !source.variations.is_empty() {
        let mut merged = source.variations.clone();
        merged.extend(
            synth
                .variations
                .iter()
                .filter(|v| !source.variations.iter().any(|s| s.tag == v.tag))
                .copied(),
        );
        synth.variations = merged;
    }
    synth
}

/// How far `attrs` is from the request (lower is closer): slant mismatch
/// dominates, then weight distance, then width distance.
fn match_distance(attrs: &FaceAttrs, req: &FallbackRequest) -> f32 {
    let upright = |s: FontStyle| matches!(s, FontStyle::Normal);
    let slant_penalty = if upright(attrs.style) == upright(req.slant) {
        0.0
    } else {
        10_000.0
    };
    slant_penalty
        + (attrs.weight - req.weight.value()).abs()
        + (attrs.width - req.width).abs() * 100.0
}

/// The variation axes a request's attributes decide.
const STYLE_AXES: [[u8; 4]; 4] = [*b"wght", *b"wdth", *b"ital", *b"slnt"];

/// A generic default re-targeted at the request: `default_family` answers at
/// regular attributes, so its style coordinates and synthesis hint describe a
/// regular match and are dropped for [`synthesize`] to recompute (other
/// coordinates, `opsz` included, are kept).
fn at_request_attributes(mut source: FaceSource) -> FaceSource {
    source.synthesis_hint = None;
    source.variations.retain(|v| !STYLE_AXES.contains(&v.tag));
    source
}

/// Whether `face` belongs to family `name`: its PostScript name, or a family,
/// typographic-family, WWS-family or PostScript `name` record in any language,
/// equals `name` case-insensitively.
fn is_family(face: &FaceRef, name: &str) -> bool {
    let wanted = name.to_lowercase();
    if face
        .postscript_name
        .as_deref()
        .is_some_and(|ps| ps.to_lowercase() == wanted)
    {
        return true;
    }
    let Ok(font) = FontRef::from_index(face.blob.data(), face.index) else {
        return false;
    };
    let Ok(table) = font.name() else {
        return false;
    };
    let ids = [
        NameId::FAMILY_NAME,
        NameId::TYPOGRAPHIC_FAMILY_NAME,
        NameId::WWS_FAMILY_NAME,
        NameId::POSTSCRIPT_NAME,
    ];
    table
        .name_record()
        .iter()
        .filter(|record| ids.contains(&record.name_id()))
        .filter_map(|record| record.string(table.string_data()).ok())
        .any(|string| string.chars().collect::<String>().to_lowercase() == wanted)
}

/// The request the backend is asked for one named family: that family, then
/// the chain's generic slots (the base a backend falls back to when it lacks
/// the family), so no later named family is in it.
fn family_request(req: &FallbackRequest, name: &str) -> FallbackRequest {
    let mut family = req.clone();
    family.families = std::iter::once(FamilyName::Named(name.to_owned()))
        .chain(
            req.families
                .iter()
                .filter(|f| matches!(f, FamilyName::Generic(_)))
                .cloned(),
        )
        .collect();
    family
}

/// The request for the chain's generic slots alone (`SystemUi` when it names
/// none): every chain with the same generics shares its cache entry.
fn generic_request(req: &FallbackRequest) -> FallbackRequest {
    let mut generic = req.clone();
    generic.families = req
        .generics()
        .into_iter()
        .map(FamilyName::Generic)
        .collect();
    generic
}

/// A named family's system answer.
enum FamilyAnswer {
    /// A face of the asked family.
    Family(FaceRef),
    /// A covering face of another family: the OS cascade substituting.
    Substitute(FaceRef),
}

/// A backend question, asked on a cache miss.
type Ask = fn(&mut Oracle, &FallbackRequest) -> Option<(FaceSource, FaceRef)>;

/// The most system families one resolve asks the backend for (step 1); app
/// families do not count. Every named system family is one probe whether the
/// cache or the backend answers it, so a chain resolves the same cold and
/// warm. A chain naming more is cut there (counted in
/// `OracleStats::probe_cap_hits`); its generics still answer. CSS stacks
/// rarely name more than a handful of families, and each probe on a cold
/// cache is an OS font query.
pub(crate) const MAX_FAMILY_PROBES: usize = 8;

impl Oracle {
    /// Resolves one cluster to a face, or `None` when nothing installed or
    /// registered can draw it. See the module docs for the order.
    pub(crate) fn resolve(&mut self, req: &FallbackRequest) -> Option<FaceRef> {
        // 1. Each named family, in chain order.
        let names: Vec<String> = req
            .families
            .iter()
            .filter_map(|f| match f {
                FamilyName::Named(name) => Some(name.clone()),
                FamilyName::Generic(_) => None,
            })
            .collect();
        let mut substitute = None;
        let mut probes = 0;
        let mut capped = false;
        for name in &names {
            if self.registry.has_family(name) {
                if let Some(face) = self.resolve_app_family(name, req) {
                    self.stats.app_hits += 1;
                    return Some(face);
                }
                // Shadowed: the system family of this name is never asked.
                continue;
            }
            if probes == MAX_FAMILY_PROBES {
                // Later app families stay reachable; no more system ones.
                capped = true;
                continue;
            }
            probes += 1;
            match self.resolve_system_family(name, req) {
                Some(FamilyAnswer::Family(face)) => return Some(face),
                Some(FamilyAnswer::Substitute(face)) => {
                    substitute.get_or_insert(face);
                }
                None => {}
            }
        }
        if capped {
            self.stats.probe_cap_hits += 1;
            log::debug!(
                "frust-text: a font chain names more than {MAX_FAMILY_PROBES} system families; \
                 the later ones were not asked"
            );
        }

        // 2. The generic request: the backend, then its generic defaults.
        let generic_req = generic_request(req);
        if let Some((face, _)) = self.through_cache(&generic_req, Self::ask_generic, None) {
            return Some(face);
        }

        // 3. App generic fallbacks.
        for generic in req.generics() {
            if let Some(face) = self.resolve_app_generic(generic, req) {
                self.stats.generic_hits += 1;
                return Some(face);
            }
        }

        // 4. A substitute, when nothing above covered the cluster.
        if substitute.is_some() {
            self.stats.substitute_hits += 1;
            return substitute;
        }
        self.stats.unresolved += 1;
        None
    }

    /// The default face for a generic slot: the system's, else the first app
    /// generic fallback registered for it. No coverage check (there is no
    /// cluster), but the renderability filter still applies.
    pub(crate) fn default_face(&mut self, generic: GenericSlot) -> Option<FaceRef> {
        let caps = self.capabilities;
        if let Some(source) = self.backend.default_family(generic) {
            match self.check(&source, None, &caps) {
                Ok((blob, facts)) => {
                    let synthesis = match &source.synthesis_hint {
                        Some(hint) => hint.clone(),
                        None => Synthesis {
                            variations: source.variations.clone(),
                            ..Synthesis::default()
                        },
                    };
                    let _ = facts;
                    return Some(FaceRef {
                        blob,
                        index: source.index,
                        synthesis,
                        postscript_name: source.postscript_name.clone(),
                        hidden: source.hidden,
                    });
                }
                Err(rejection) => self.count(rejection),
            }
        }
        let faces: Vec<AppFace> = self.registry.generic_faces(generic).cloned().collect();
        faces
            .into_iter()
            .find(|face| caps.can_draw(&face.facts.formats))
            .map(|face| FaceRef {
                blob: face.blob,
                index: face.index,
                synthesis: Synthesis::default(),
                postscript_name: face.postscript_name,
                hidden: face.hidden,
            })
    }

    /// `req`'s system answer through the fallback cache, keyed by `req`;
    /// `ask` runs on a miss and its outcome is cached, misses included. For a
    /// named-family request (`family`), a fresh answer that is not that family
    /// is cached as a substitute; the flag comes back with the face (`true`:
    /// a substitute), so a cache hit never re-reads the face's names.
    fn through_cache(
        &mut self,
        req: &FallbackRequest,
        ask: Ask,
        family: Option<&str>,
    ) -> Option<(FaceRef, bool)> {
        let key = CacheKey::from_request(req);
        match self.cache.get_entry(&key) {
            Some((CachedAnswer::Face(source), substitute)) => {
                self.stats.cache_hits += 1;
                self.load_cached(&source, req)
                    .map(|face| (face, substitute))
            }
            Some((CachedAnswer::Miss, _)) => {
                self.stats.cache_hits += 1;
                None
            }
            None => {
                self.stats.cache_misses += 1;
                let Some((source, face)) = ask(self, req) else {
                    self.cache.insert(key, CachedAnswer::Miss);
                    return None;
                };
                let substitute = family.is_some_and(|name| !is_family(&face, name));
                if substitute {
                    log::debug!(
                        "frust-text: family {:?} answered by another family ({}): demoted to a \
                         substitute behind the generic step",
                        family.unwrap_or_default(),
                        face.postscript_name.as_deref().unwrap_or("unnamed face"),
                    );
                }
                self.cache
                    .insert_entry(key, CachedAnswer::Face(source), substitute);
                Some((face, substitute))
            }
        }
    }

    /// Step 1 for a name the app registry lacks: the backend's answer for that
    /// family alone, sorted into the family itself or a cascade substitute.
    fn resolve_system_family(&mut self, name: &str, req: &FallbackRequest) -> Option<FamilyAnswer> {
        let family_req = family_request(req, name);
        let (face, substitute) = self.through_cache(&family_req, Self::ask_family, Some(name))?;
        Some(if substitute {
            self.stats.substitutes += 1;
            FamilyAnswer::Substitute(face)
        } else {
            FamilyAnswer::Family(face)
        })
    }

    /// Step 1 for a name the app registry holds: its faces in
    /// attribute-distance order.
    fn resolve_app_family(&mut self, name: &str, req: &FallbackRequest) -> Option<FaceRef> {
        let mut faces: Vec<AppFace> = self.registry.faces_named(name).cloned().collect();
        faces.sort_by(|a, b| {
            match_distance(&a.facts.attrs, req).total_cmp(&match_distance(&b.facts.attrs, req))
        });
        faces
            .iter()
            .find_map(|face| self.accept_app_face(face, req))
    }

    /// Step 3 for one generic slot.
    fn resolve_app_generic(
        &mut self,
        generic: GenericSlot,
        req: &FallbackRequest,
    ) -> Option<FaceRef> {
        let faces: Vec<AppFace> = self.registry.generic_faces(generic).cloned().collect();
        faces
            .iter()
            .find_map(|face| self.accept_app_face(face, req))
    }

    /// An app face, if it covers and can be drawn.
    fn accept_app_face(&mut self, face: &AppFace, req: &FallbackRequest) -> Option<FaceRef> {
        if !req.renderability.can_draw(&face.facts.formats) {
            self.stats.refused += 1;
            return None;
        }
        let covered = FontRef::from_index(face.blob.data(), face.index)
            .is_ok_and(|font| covers(&font, &req.chars));
        if !covered {
            self.stats.uncovered += 1;
            return None;
        }
        Some(FaceRef {
            blob: face.blob.clone(),
            index: face.index,
            synthesis: synthesize(&face.facts.attrs, req.weight.value(), req.slant, req.width),
            postscript_name: face.postscript_name.clone(),
            hidden: face.hidden,
        })
    }

    /// A single-family request's backend answer (the accepted source, for the
    /// cache, with the face it resolved to).
    fn ask_family(&mut self, req: &FallbackRequest) -> Option<(FaceSource, FaceRef)> {
        self.stats.backend_calls += 1;
        let source = self.backend.resolve(req)?;
        let face = self.accept(&source, req)?;
        Some((source, face))
    }

    /// Step 2 on a cache miss: the backend's answer for the generic request,
    /// then its generic defaults at the request's attributes.
    fn ask_generic(&mut self, req: &FallbackRequest) -> Option<(FaceSource, FaceRef)> {
        if let Some(answer) = self.ask_family(req) {
            return Some(answer);
        }
        for generic in req.generics() {
            if let Some(source) = self
                .backend
                .default_family(generic)
                .map(at_request_attributes)
                && let Some(face) = self.accept(&source, req)
            {
                self.stats.generic_hits += 1;
                return Some((source, face));
            }
        }
        None
    }

    /// A backend candidate, if it loads, covers and can be drawn.
    fn accept(&mut self, source: &FaceSource, req: &FallbackRequest) -> Option<FaceRef> {
        match self.check(source, Some(&req.chars), &req.renderability) {
            Ok((blob, facts)) => Some(FaceRef {
                blob,
                index: source.index,
                synthesis: synthesis_for(source, &facts.attrs, req),
                postscript_name: source.postscript_name.clone(),
                hidden: source.hidden,
            }),
            Err(rejection) => {
                self.count(rejection);
                None
            }
        }
    }

    /// A cached answer's face. The cached source already passed coverage and
    /// renderability; if its file vanished since, the answer is simply dropped.
    fn load_cached(&mut self, source: &FaceSource, req: &FallbackRequest) -> Option<FaceRef> {
        let (blob, index) = self.store.load(source)?;
        let attrs = self.facts_of(source, &blob)?.attrs;
        Some(FaceRef {
            blob,
            index,
            synthesis: synthesis_for(source, &attrs, req),
            postscript_name: source.postscript_name.clone(),
            hidden: source.hidden,
        })
    }

    /// Loads `source`, then checks coverage of `chars` (when given) and
    /// renderability against `caps`.
    fn check(
        &mut self,
        source: &FaceSource,
        chars: Option<&str>,
        caps: &RenderCapabilities,
    ) -> Result<(peniko::Blob<u8>, FaceFacts), Rejection> {
        let (blob, index) = self.store.load(source).ok_or(Rejection::Unloadable)?;
        let facts = self.facts_of(source, &blob).ok_or(Rejection::Unloadable)?;
        if !caps.can_draw(&facts.formats) {
            return Err(Rejection::Unrenderable);
        }
        if let Some(chars) = chars {
            let font =
                FontRef::from_index(blob.data(), index).map_err(|_| Rejection::Unloadable)?;
            if !covers(&font, chars) {
                return Err(Rejection::Uncovered);
            }
        }
        Ok((blob, facts))
    }

    /// The face's facts, parsed once per [`FaceKey`].
    fn facts_of(&mut self, source: &FaceSource, blob: &peniko::Blob<u8>) -> Option<FaceFacts> {
        *self.facts.entry(FaceKey::of(source)).or_insert_with(|| {
            FontRef::from_index(blob.data(), source.index)
                .ok()
                .map(|font| face_facts(&font))
        })
    }

    fn count(&mut self, rejection: Rejection) {
        match rejection {
            Rejection::Unrenderable => self.stats.refused += 1,
            Rejection::Uncovered => self.stats.uncovered += 1,
            Rejection::Unloadable => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::synthetic::{FaceSpec, SyntheticBackend, TempDir};
    use super::super::{FaceLocation, ScriptTag};
    use super::*;
    use crate::style::{FontFamily, FontWeight};
    use peniko::Blob;

    fn caps() -> RenderCapabilities {
        RenderCapabilities::default()
    }

    fn req(chars: &str, script: &[u8; 4], family: &FontFamily) -> FallbackRequest {
        FallbackRequest::new(chars, ScriptTag(*script), family, caps())
    }

    fn oracle_over(backend: SyntheticBackend) -> Oracle {
        Oracle::new(Box::new(backend), caps(), None)
    }

    #[test]
    fn hello_chinese_resolves_per_cluster_to_the_expected_faces() {
        let latin = Blob::from(FaceSpec::new("Synthetic Sans").chars("Helo ").build());
        let han = Blob::from(FaceSpec::new("Synthetic Han").chars("中文").build());
        let mut backend = SyntheticBackend::new("build-1");
        backend.cover(&latin, "Helo ");
        backend.cover(&han, "中文");
        let mut oracle = oracle_over(backend);
        let system_ui = FontFamily::SystemUi;
        for (i, c) in "Hello 中文".chars().enumerate() {
            let script = if c == ' ' {
                ScriptTag::COMMON
            } else if c.is_ascii() {
                ScriptTag(*b"Latn")
            } else {
                ScriptTag(*b"Hani")
            };
            let request = FallbackRequest::new(c.to_string(), script, &system_ui, caps());
            let face = oracle
                .resolve(&request)
                .unwrap_or_else(|| panic!("cluster {i} ({c:?}) must resolve"));
            let expected = if c.is_ascii() { &latin } else { &han };
            assert_eq!(face.blob.id(), expected.id(), "cluster {i} ({c:?})");
            assert_eq!(face.synthesis, Synthesis::default());
        }
    }

    #[test]
    fn hits_and_misses_are_both_cached() {
        let latin = Blob::from(FaceSpec::new("Synthetic Sans").chars("a").build());
        let mut backend = SyntheticBackend::new("b");
        backend.cover(&latin, "a");
        let calls = backend.calls();
        let mut oracle = oracle_over(backend);
        let family = FontFamily::SystemUi;

        assert!(oracle.resolve(&req("a", b"Latn", &family)).is_some());
        assert!(oracle.resolve(&req("a", b"Latn", &family)).is_some());
        assert_eq!(calls.resolves(), 1, "a hit is served from the cache");

        assert!(oracle.resolve(&req("ж", b"Cyrl", &family)).is_none());
        assert!(oracle.resolve(&req("ж", b"Cyrl", &family)).is_none());
        assert_eq!(calls.resolves(), 2, "a miss is cached too");
        let stats = oracle.stats();
        assert_eq!((stats.cache_hits, stats.cache_misses), (2, 2));
        assert_eq!(stats.unresolved, 2);
        assert_eq!(oracle.cache().len(), 2, "one hit entry, one miss entry");
    }

    #[test]
    fn app_answers_are_never_cached() {
        let app = Blob::from(FaceSpec::new("Brand").chars("a").build());
        let mut oracle = oracle_over(SyntheticBackend::new("b"));
        oracle.registry_mut().insert_blob(&app);
        let chain = FontFamily::named("Brand");
        for _ in 0..3 {
            assert!(oracle.resolve(&req("a", b"Latn", &chain)).is_some());
        }
        assert_eq!(oracle.stats().app_hits, 3);
        assert_eq!(oracle.cache().len(), 0);
    }

    /// A registered face carries its PostScript name and hidden flag onto the
    /// face the selector answers — through a named family and through a
    /// registered generic fallback alike — as a system face does.
    #[test]
    fn a_registered_face_answers_with_its_postscript_name() {
        let noto =
            Blob::from(include_bytes!("../../../../testing/fonts/NotoSans-Subset.ttf").to_vec());
        let mut oracle = oracle_over(SyntheticBackend::new("b"));
        oracle.registry_mut().insert_blob(&noto);
        oracle
            .registry_mut()
            .insert_generic(&noto, &[GenericSlot::Serif]);
        let face = oracle
            .resolve(&req("H", b"Latn", &FontFamily::named("Noto Sans")))
            .expect("the registered family covers H");
        assert_eq!(face.blob.id(), noto.id());
        assert_eq!(face.postscript_name.as_deref(), Some("NotoSans-Regular"));
        assert!(!face.hidden);
        let generic = oracle
            .resolve(&req(
                "H",
                b"Latn",
                &FontFamily::NamedWithGeneric(vec![FamilyName::Generic(GenericSlot::Serif)]),
            ))
            .expect("the registered generic covers H");
        assert_eq!(generic.postscript_name.as_deref(), Some("NotoSans-Regular"));
        let default = oracle
            .default_face(GenericSlot::Serif)
            .expect("the registered generic is the default");
        assert_eq!(default.postscript_name.as_deref(), Some("NotoSans-Regular"));
        assert!(!default.hidden);
    }

    #[test]
    fn the_persistent_cache_answers_the_next_run_without_the_backend() {
        let dir = TempDir::new("oracle-persist");
        let font = dir.write("latin.ttf", &FaceSpec::new("Latin").chars("a").build());
        let file = dir
            .path()
            .join("cache")
            .join(super::super::cache::PERSIST_FILE_NAME);
        let backend = |build: &str| {
            let mut backend = SyntheticBackend::new(build);
            backend.cover_with(FaceSource::path(&font, 0), "a");
            backend
        };
        let family = FontFamily::SystemUi;

        let first = backend("build-1");
        let first_calls = first.calls();
        let mut run1 = Oracle::new(Box::new(first), caps(), Some(file.clone()));
        assert!(run1.resolve(&req("a", b"Latn", &family)).is_some());
        assert!(run1.resolve(&req("ж", b"Cyrl", &family)).is_none());
        assert_eq!(first_calls.resolves(), 2);
        run1.persist().expect("written");
        drop(run1);

        // Same OS build: both answers come from disk.
        let second = backend("build-1");
        let second_calls = second.calls();
        let mut run2 = Oracle::new(Box::new(second), caps(), Some(file.clone()));
        let face = run2.resolve(&req("a", b"Latn", &family)).unwrap();
        assert_eq!(face.blob.data(), std::fs::read(&font).unwrap().as_slice());
        assert!(run2.resolve(&req("ж", b"Cyrl", &family)).is_none());
        assert_eq!(second_calls.resolves(), 0, "the persistent cache answered");
        drop(run2);

        // A new OS build: the backend is asked again.
        let third = backend("build-2");
        let third_calls = third.calls();
        let mut run3 = Oracle::new(Box::new(third), caps(), Some(file));
        assert!(run3.resolve(&req("a", b"Latn", &family)).is_some());
        assert_eq!(third_calls.resolves(), 1);
    }

    #[test]
    fn resolve_order_is_app_then_cache_then_backend_then_generic() {
        let app = Blob::from(FaceSpec::new("Brand").chars("ab").build());
        let system = Blob::from(FaceSpec::new("System").chars("abc").build());
        let generic = Blob::from(FaceSpec::new("Generic").chars("abcd").build());
        let app_generic = Blob::from(FaceSpec::new("App Generic").chars("abcde").build());
        let mut backend = SyntheticBackend::new("b");
        backend.cover(&system, "abc");
        backend.default_for(GenericSlot::SansSerif, &generic);
        let calls = backend.calls();
        let mut oracle = oracle_over(backend);
        oracle.registry_mut().insert_blob(&app);
        oracle
            .registry_mut()
            .insert_generic(&app_generic, &[GenericSlot::SansSerif]);
        let chain = FontFamily::stack_with_generic(["Brand"], GenericSlot::SansSerif);

        // App first: the backend is never asked.
        let face = oracle.resolve(&req("a", b"Latn", &chain)).unwrap();
        assert_eq!(face.blob.id(), app.id());
        assert_eq!(calls.resolves(), 0);

        // Not in the app face: the backend answers, then the cache does.
        let face = oracle.resolve(&req("c", b"Latn", &chain)).unwrap();
        assert_eq!(face.blob.id(), system.id());
        let face = oracle.resolve(&req("c", b"Latn", &chain)).unwrap();
        assert_eq!(face.blob.id(), system.id());
        assert_eq!(calls.resolves(), 1);
        assert_eq!(oracle.stats().cache_hits, 1);

        // The backend misses: its generic default answers.
        let face = oracle.resolve(&req("d", b"Latn", &chain)).unwrap();
        assert_eq!(face.blob.id(), generic.id());

        // The system has nothing: the app generic fallback answers last.
        let face = oracle.resolve(&req("e", b"Latn", &chain)).unwrap();
        assert_eq!(face.blob.id(), app_generic.id());
        let stats = oracle.stats();
        assert_eq!(stats.app_hits, 1);
        assert_eq!(stats.generic_hits, 2);
    }

    #[test]
    fn app_faces_shadow_system_faces_of_the_same_family() {
        let system_helvetica = Blob::from(FaceSpec::new("Helvetica").chars("AB").build());
        let app_helvetica = Blob::from(FaceSpec::new("Helvetica").chars("A").build());
        let mut backend = SyntheticBackend::new("b");
        backend.family("Helvetica", &system_helvetica, "AB");
        let calls = backend.calls();
        let mut oracle = oracle_over(backend);
        let chain = FontFamily::named("Helvetica");

        // Before registration the system family answers and is cached.
        let face = oracle.resolve(&req("A", b"Latn", &chain)).unwrap();
        assert_eq!(face.blob.id(), system_helvetica.id());

        oracle.registry_mut().insert_blob(&app_helvetica);
        let face = oracle.resolve(&req("A", b"Latn", &chain)).unwrap();
        assert_eq!(face.blob.id(), app_helvetica.id(), "the app face wins");

        // A character the app face lacks never reaches the system family of
        // the same name: the backend is asked without it.
        let _ = oracle.resolve(&req("B", b"Latn", &chain));
        let last = calls.last_request().expect("the backend was asked");
        assert!(
            !last
                .families
                .iter()
                .any(|f| matches!(f, FamilyName::Named(n) if n == "Helvetica")),
            "a shadowed family is stripped from the system request: {:?}",
            last.families
        );
    }

    #[test]
    fn a_face_the_engine_cannot_draw_is_refused_and_counted() {
        let hvgl_only = Blob::from(FaceSpec::new("PingFang UI").chars("中").hvgl_only().build());
        let mut backend = SyntheticBackend::new("b");
        backend.cover(&hvgl_only, "中");
        let mut oracle = oracle_over(backend);
        let mut request = req("中", b"Hani", &FontFamily::SystemUi);
        request.renderability.hvgl_via_coretext = false;
        assert!(oracle.resolve(&request).is_none(), "never substituted");
        assert_eq!(oracle.stats().refused, 1);

        // The same face with hvgl drawable (Apple) is accepted.
        let mut backend = SyntheticBackend::new("b");
        backend.cover(&hvgl_only, "中");
        let mut oracle = oracle_over(backend);
        request.renderability.hvgl_via_coretext = true;
        assert!(oracle.resolve(&request).is_some());
        assert_eq!(oracle.stats().refused, 0);
    }

    #[test]
    fn a_backend_answer_missing_a_character_is_uncovered() {
        let partial = Blob::from(FaceSpec::new("Partial").chars("a").build());
        let mut backend = SyntheticBackend::new("b");
        // Scripted to claim "ab" although its cmap only has "a".
        backend.cover(&partial, "ab");
        let mut oracle = oracle_over(backend);
        assert!(
            oracle
                .resolve(&req("ab", b"Latn", &FontFamily::SystemUi))
                .is_none()
        );
        assert_eq!(oracle.stats().uncovered, 1);
        // Variation selectors are exempt: "a" + VS16 is covered.
        assert!(
            oracle
                .resolve(&req("a\u{FE0F}", b"Latn", &FontFamily::SystemUi))
                .is_some()
        );
    }

    #[test]
    fn synthesis_fakes_bold_and_oblique_on_a_static_face() {
        let regular = Blob::from(FaceSpec::new("Static").chars("a").build());
        let mut backend = SyntheticBackend::new("b");
        backend.cover(&regular, "a");
        let mut oracle = oracle_over(backend);
        let mut request = req("a", b"Latn", &FontFamily::SystemUi);
        request.weight = FontWeight::BOLD;
        request.slant = FontStyle::Italic;
        let face = oracle.resolve(&request).unwrap();
        assert!(face.synthesis.embolden, "no wght axis: faux bold");
        assert_eq!(face.synthesis.skew, Some(DEFAULT_OBLIQUE_DEGREES));
        assert!(face.synthesis.variations.is_empty());

        // Lighter than the face: nothing to fake.
        request.weight = FontWeight::LIGHT;
        request.slant = FontStyle::Oblique(Some(8.0));
        let face = oracle.resolve(&request).unwrap();
        assert!(!face.synthesis.embolden);
        assert_eq!(face.synthesis.skew, Some(8.0));
    }

    #[test]
    fn synthesis_uses_axes_on_a_variable_face() {
        let attrs = FaceAttrs {
            has_wght: true,
            has_ital: true,
            has_wdth: true,
            ..FaceAttrs::default()
        };
        let synth = synthesize(&attrs, 700.0, FontStyle::Italic, 0.75);
        assert!(!synth.embolden && synth.skew.is_none());
        assert_eq!(synth.variation(*b"wght"), Some(700.0));
        assert_eq!(synth.variation(*b"ital"), Some(1.0));
        assert_eq!(synth.variation(*b"wdth"), Some(75.0));

        let slnt_only = FaceAttrs {
            has_slnt: true,
            ..FaceAttrs::default()
        };
        let synth = synthesize(&slnt_only, 400.0, FontStyle::Oblique(None), 1.0);
        assert_eq!(synth.variation(*b"slnt"), Some(DEFAULT_OBLIQUE_DEGREES));

        // An italic face asked for italic needs nothing.
        let italic = FaceAttrs {
            style: FontStyle::Italic,
            ..FaceAttrs::default()
        };
        assert_eq!(
            synthesize(&italic, 400.0, FontStyle::Italic, 1.0),
            Synthesis::default()
        );
    }

    #[test]
    fn synthesis_parsed_from_real_tables() {
        let face = FaceSpec::new("Var")
            .chars("a")
            .weight(300)
            .italic()
            .axes(&[*b"wght"])
            .build();
        let font = FontRef::new(&face).unwrap();
        let attrs = face_attrs(&font);
        assert_eq!(attrs.weight, 300.0);
        assert_eq!(attrs.style, FontStyle::Italic);
        assert!(attrs.has_wght && !attrs.has_slnt);
    }

    #[test]
    fn a_backend_synthesis_hint_is_honoured() {
        let cjk = Blob::from(FaceSpec::new("Noto Sans CJK").chars("中").build());
        let hint = Synthesis {
            embolden: true,
            skew: None,
            variations: Vec::new(),
        };
        let mut backend = SyntheticBackend::new("b");
        backend.cover_with(
            FaceSource {
                location: FaceLocation::Blob(cjk.clone()),
                index: 0,
                variations: vec![VariationSetting {
                    tag: *b"opsz",
                    value: 17.0,
                }],
                synthesis_hint: Some(hint),
                postscript_name: None,
                hidden: false,
                scale: None,
            },
            "中",
        );
        let mut oracle = oracle_over(backend);
        // Regular weight requested, yet the backend says faux bold is needed
        // (the s-03 "Han w700 came back w400" case expressed as a hint).
        let face = oracle
            .resolve(&req("中", b"Hani", &FontFamily::SystemUi))
            .unwrap();
        assert!(face.synthesis.embolden, "the hint wins over the frust rule");
        assert_eq!(
            face.synthesis.variation(*b"opsz"),
            Some(17.0),
            "the backend's instance coordinates are kept"
        );
    }

    #[test]
    fn a_hidden_face_passes_through_with_its_flag_and_name() {
        let hidden = Blob::from(FaceSpec::new(".Hidden Sans").chars("a").build());
        let mut backend = SyntheticBackend::new("b");
        backend.cover_with(
            FaceSource {
                postscript_name: Some(".HiddenSans-Regular".into()),
                hidden: true,
                ..FaceSource::blob(hidden.clone(), 0)
            },
            "a",
        );
        let mut oracle = oracle_over(backend);
        let face = oracle
            .resolve(&req("a", b"Latn", &FontFamily::SystemUi))
            .expect("a hidden face is not substituted");
        assert_eq!(face.blob.id(), hidden.id());
        assert!(face.hidden);
        assert_eq!(face.postscript_name.as_deref(), Some(".HiddenSans-Regular"));
        // Cached lookups keep the flag.
        let again = oracle
            .resolve(&req("a", b"Latn", &FontFamily::SystemUi))
            .unwrap();
        assert!(again.hidden);
    }

    #[test]
    fn a_collection_face_index_is_honoured() {
        let dir = TempDir::new("ttc");
        let ttc = FaceSpec::collection(&[
            FaceSpec::new("CJK JP").chars("文"),
            FaceSpec::new("CJK SC").chars("中文"),
        ]);
        let path = dir.write("cjk.ttc", &ttc);
        let mut backend = SyntheticBackend::new("b");
        backend.cover_with(FaceSource::path(&path, 1), "中");
        let mut oracle = oracle_over(backend);
        let face = oracle
            .resolve(&req("中", b"Hani", &FontFamily::SystemUi))
            .unwrap();
        assert_eq!(face.index, 1);
        assert!(
            oracle
                .resolve(&req("中", b"Hani", &FontFamily::SystemUi))
                .is_some()
        );
        assert_eq!(oracle.store().file_reads(), 1, "the file is read once");
    }

    #[test]
    fn default_face_prefers_the_system_then_app_generics() {
        let system = Blob::from(FaceSpec::new("System Mono").chars("a").build());
        let app = Blob::from(FaceSpec::new("App Emoji").chars("a").build());
        let mut backend = SyntheticBackend::new("b");
        backend.default_for(GenericSlot::Monospace, &system);
        let mut oracle = oracle_over(backend);
        oracle
            .registry_mut()
            .insert_generic(&app, &[GenericSlot::Emoji, GenericSlot::Monospace]);
        let mono = oracle.default_face(GenericSlot::Monospace).unwrap();
        assert_eq!(mono.blob.id(), system.id());
        let emoji = oracle.default_face(GenericSlot::Emoji).unwrap();
        assert_eq!(emoji.blob.id(), app.id());
        assert!(oracle.default_face(GenericSlot::Serif).is_none());
    }

    #[test]
    fn an_earlier_system_family_beats_a_later_app_family() {
        let segoe = Blob::from(FaceSpec::new("Segoe UI").chars("a").build());
        let inter = Blob::from(FaceSpec::new("Inter").chars("a").build());
        let mut backend = SyntheticBackend::new("b");
        backend.family("Segoe UI", &segoe, "a");
        let calls = backend.calls();
        let mut oracle = oracle_over(backend);
        oracle.registry_mut().insert_blob(&inter);
        let chain = FontFamily::stack(["Segoe UI", "Inter"]);

        let face = oracle.resolve(&req("a", b"Latn", &chain)).unwrap();
        assert_eq!(face.blob.id(), segoe.id(), "chain order, not app-first");
        assert_eq!(calls.resolves(), 1, "the backend is asked exactly once");
        assert_eq!(calls.named_families(), [Some("Segoe UI".to_owned())]);
        assert_eq!(oracle.stats().app_hits, 0);
    }

    #[test]
    fn an_earlier_app_family_beats_a_later_system_family() {
        let segoe = Blob::from(FaceSpec::new("Segoe UI").chars("a").build());
        let inter = Blob::from(FaceSpec::new("Inter").chars("a").build());
        let mut backend = SyntheticBackend::new("b");
        backend.family("Segoe UI", &segoe, "a");
        let calls = backend.calls();
        let mut oracle = oracle_over(backend);
        oracle.registry_mut().insert_blob(&inter);
        let chain = FontFamily::stack(["Inter", "Segoe UI"]);

        let face = oracle.resolve(&req("a", b"Latn", &chain)).unwrap();
        assert_eq!(face.blob.id(), inter.id());
        assert_eq!(calls.resolves(), 0);
        assert_eq!(oracle.stats().app_hits, 1);
    }

    #[test]
    fn a_later_system_family_answers_when_the_first_fails_coverage() {
        // Scripted to claim "a" although its cmap only has "b".
        let first = Blob::from(FaceSpec::new("First").chars("b").build());
        let second = Blob::from(FaceSpec::new("Second").chars("a").build());
        let mut backend = SyntheticBackend::new("b");
        backend.family("First", &first, "ab");
        backend.family("Second", &second, "a");
        let calls = backend.calls();
        let mut oracle = oracle_over(backend);
        let chain = FontFamily::stack(["First", "Second"]);

        let face = oracle.resolve(&req("a", b"Latn", &chain)).unwrap();
        assert_eq!(
            face.blob.id(),
            second.id(),
            "the second family is reachable"
        );
        assert_eq!(
            calls.named_families(),
            [Some("First".to_owned()), Some("Second".to_owned())],
            "one request per family, each naming only its family"
        );
        assert_eq!(oracle.stats().uncovered, 1);

        // Both per-family answers are cached under their own keys.
        let again = oracle.resolve(&req("a", b"Latn", &chain)).unwrap();
        assert_eq!(again.blob.id(), second.id());
        assert_eq!(calls.resolves(), 2);
        assert_eq!(oracle.cache().len(), 2);
    }

    #[test]
    fn a_cascade_substitute_does_not_hide_a_later_family() {
        let second = Blob::from(FaceSpec::new("Second").chars("a").build());
        let cascade = Blob::from(FaceSpec::new("Cascade").chars("a").build());
        let mut backend = SyntheticBackend::new("b");
        backend.family("Second", &second, "a");
        backend.cover(&cascade, "a");
        let calls = backend.calls();
        let mut oracle = oracle_over(backend);

        // "First" is not installed: its request comes back from the cascade.
        let chain = FontFamily::stack(["First", "Second"]);
        let face = oracle.resolve(&req("a", b"Latn", &chain)).unwrap();
        assert_eq!(face.blob.id(), second.id());

        // With no named family answering, the generic request IS asked before
        // a held-back substitute is used (fontique / CSS order).
        let before = calls.resolves();
        let chain = FontFamily::stack(["First", "Third"]);
        let face = oracle.resolve(&req("a", b"Latn", &chain)).unwrap();
        assert_eq!(face.blob.id(), cascade.id());
        assert_eq!(
            calls.named_families()[before..],
            [Some("Third".to_owned()), None],
            "\"First\" came from the cache, then \"Third\" and the generic request were asked"
        );
        let stats = oracle.stats();
        assert_eq!(stats.substitutes, 3, "First (twice) and Third were demoted");
        assert_eq!(stats.substitute_hits, 0, "the generic request answered");
    }

    #[test]
    fn the_generic_request_beats_a_held_back_substitute() {
        // An installed Menlo lacks Han: the OS cascades from it to Cascade Han.
        let menlo = Blob::from(FaceSpec::new("Menlo").chars("a").build());
        let cascade = Blob::from(FaceSpec::new("Cascade Han").chars("中").build());
        let generic = Blob::from(FaceSpec::new("Generic Han").chars("中").build());
        let mut backend = SyntheticBackend::new("b");
        backend.family("Menlo", &menlo, "a");
        backend.cascade_for("Menlo", &cascade, "中");
        backend.cover(&generic, "中");
        let calls = backend.calls();
        let mut oracle = oracle_over(backend);
        let chain = FontFamily::stack_with_generic(["Menlo"], GenericSlot::Monospace);

        for _ in 0..2 {
            let face = oracle.resolve(&req("中", b"Hani", &chain)).unwrap();
            assert_eq!(face.blob.id(), generic.id(), "generics before the cascade");
        }
        assert_eq!(
            calls.named_families(),
            [Some("Menlo".to_owned()), None],
            "each request asked once, then cached"
        );
        let stats = oracle.stats();
        assert_eq!(stats.substitutes, 2, "demoted on the fill and on the hit");
        assert_eq!(stats.substitute_hits, 0);
        // The family's own answer is not a substitute.
        assert_eq!(
            oracle
                .resolve(&req("a", b"Latn", &chain))
                .unwrap()
                .blob
                .id(),
            menlo.id()
        );
        assert_eq!(oracle.stats().substitutes, 2);
    }

    #[test]
    fn an_app_generic_fallback_beats_a_held_back_substitute() {
        let cascade = Blob::from(FaceSpec::new("Cascade").chars("a").build());
        let app_generic = Blob::from(FaceSpec::new("App Sans").chars("a").build());
        let mut backend = SyntheticBackend::new("b");
        backend.cascade_for("Brand", &cascade, "a");
        let mut oracle = oracle_over(backend);
        oracle
            .registry_mut()
            .insert_generic(&app_generic, &[GenericSlot::SansSerif]);
        let chain = FontFamily::stack_with_generic(["Brand"], GenericSlot::SansSerif);
        let face = oracle.resolve(&req("a", b"Latn", &chain)).unwrap();
        assert_eq!(face.blob.id(), app_generic.id());
        assert_eq!(oracle.stats().substitute_hits, 0);
    }

    #[test]
    fn a_substitute_answers_when_nothing_else_covers() {
        let cascade = Blob::from(FaceSpec::new("Cascade").chars("a").build());
        let mut backend = SyntheticBackend::new("b");
        backend.cascade_for("Brand", &cascade, "a");
        let calls = backend.calls();
        let mut oracle = oracle_over(backend);
        let chain = FontFamily::named("Brand");
        let face = oracle.resolve(&req("a", b"Latn", &chain)).unwrap();
        assert_eq!(face.blob.id(), cascade.id(), "the last resort");
        assert_eq!(calls.named_families(), [Some("Brand".to_owned()), None]);
        let stats = oracle.stats();
        assert_eq!((stats.substitutes, stats.substitute_hits), (1, 1));
        assert_eq!(stats.unresolved, 0);
    }

    #[test]
    fn a_substitute_is_flagged_in_the_cache_and_the_family_is_not() {
        let inter = Blob::from(FaceSpec::new("Inter").chars("a").build());
        let cascade = Blob::from(FaceSpec::new("Cascade").chars("b").build());
        let mut backend = SyntheticBackend::new("b");
        backend.family("Inter", &inter, "a");
        backend.cascade_for("Inter", &cascade, "b");
        let mut oracle = oracle_over(backend);
        let chain = FontFamily::named("Inter");
        let _ = oracle.resolve(&req("a", b"Latn", &chain));
        let _ = oracle.resolve(&req("b", b"Latn", &chain));
        let flag = |oracle: &mut Oracle, chars: &str| {
            let family_req = family_request(&req(chars, b"Latn", &chain), "Inter");
            oracle
                .cache_mut()
                .get_entry(&CacheKey::from_request(&family_req))
                .map(|(_, substitute)| substitute)
        };
        assert_eq!(flag(&mut oracle, "a"), Some(false));
        assert_eq!(flag(&mut oracle, "b"), Some(true));
        // The generic request's answer is never a substitute.
        let generic_key = CacheKey::from_request(&generic_request(&req("b", b"Latn", &chain)));
        assert!(matches!(
            oracle.cache_mut().get_entry(&generic_key),
            Some((_, false))
        ));
    }

    #[test]
    fn named_family_probes_are_capped_per_chain() {
        let late = Blob::from(FaceSpec::new("Late").chars("a").build());
        let app = Blob::from(FaceSpec::new("App Late").chars("a").build());
        let mut backend = SyntheticBackend::new("b");
        backend.family("Late", &late, "a");
        let calls = backend.calls();
        let mut oracle = oracle_over(backend);
        oracle.registry_mut().insert_blob(&app);
        let mut names: Vec<String> = (0..MAX_FAMILY_PROBES)
            .map(|i| format!("Absent {i}"))
            .collect();
        names.push("Late".to_owned());
        let chain = FontFamily::stack(names.clone());

        // Nine system families: the ninth ("Late") is never asked.
        assert!(oracle.resolve(&req("a", b"Latn", &chain)).is_none());
        let named = calls.named_families();
        assert_eq!(named.len(), MAX_FAMILY_PROBES + 1, "{named:?}");
        assert_eq!(named[MAX_FAMILY_PROBES], None, "then the generic request");
        assert!(!named.contains(&Some("Late".to_owned())));
        assert_eq!(oracle.stats().probe_cap_hits, 1);

        // A warm cache cuts at the same place: cold and warm agree.
        assert!(oracle.resolve(&req("a", b"Latn", &chain)).is_none());
        assert_eq!(calls.resolves(), MAX_FAMILY_PROBES + 1, "all cached");
        assert_eq!(oracle.stats().probe_cap_hits, 2);

        // App families are not probes: one past the cap still answers.
        names.push("App Late".to_owned());
        let chain = FontFamily::stack(names);
        let face = oracle.resolve(&req("a", b"Latn", &chain)).unwrap();
        assert_eq!(face.blob.id(), app.id());
        assert_eq!(MAX_FAMILY_PROBES, 8);
    }

    #[test]
    fn app_shadowing_applies_to_the_same_name_only() {
        let system_brand = Blob::from(FaceSpec::new("Brand").chars("ab").build());
        let other = Blob::from(FaceSpec::new("Other").chars("b").build());
        let app_brand = Blob::from(FaceSpec::new("Brand").chars("a").build());
        let mut backend = SyntheticBackend::new("b");
        backend.family("Brand", &system_brand, "ab");
        backend.family("Other", &other, "b");
        let calls = backend.calls();
        let mut oracle = oracle_over(backend);
        oracle.registry_mut().insert_blob(&app_brand);
        let chain = FontFamily::stack(["Brand", "Other"]);

        let face = oracle.resolve(&req("a", b"Latn", &chain)).unwrap();
        assert_eq!(face.blob.id(), app_brand.id());
        let face = oracle.resolve(&req("b", b"Latn", &chain)).unwrap();
        assert_eq!(
            face.blob.id(),
            other.id(),
            "a different name is still asked"
        );
        assert_eq!(
            calls.named_families(),
            [Some("Other".to_owned())],
            "the shadowed system \"Brand\" is never asked"
        );
    }

    #[test]
    fn a_family_request_keeps_the_chain_generics_but_no_other_family() {
        let mono = Blob::from(FaceSpec::new("Mono").chars("a").build());
        let mut backend = SyntheticBackend::new("b");
        backend.cover(&mono, "a");
        let calls = backend.calls();
        let mut oracle = oracle_over(backend);
        let chain = FontFamily::stack_with_generic(["First", "Second"], GenericSlot::Monospace);
        assert!(oracle.resolve(&req("a", b"Latn", &chain)).is_some());
        let first = &calls.requests()[0];
        assert_eq!(
            first.families,
            [
                FamilyName::Named("First".into()),
                FamilyName::Generic(GenericSlot::Monospace)
            ]
        );
    }

    #[test]
    fn a_generic_default_is_resynthesised_at_the_requested_attributes() {
        // A variable default reported at its regular instance (CoreText's
        // `CTFontCopyVariation` wght=400), plus an optical-size coordinate.
        let variable = Blob::from(FaceSpec::new("Var UI").chars("a").axes(&[*b"wght"]).build());
        // A static default with a regular-weight hint (Android matching at 400).
        let static_serif = Blob::from(FaceSpec::new("Static Serif").chars("a").build());
        let mut backend = SyntheticBackend::new("b");
        backend.default_with(
            GenericSlot::SystemUi,
            FaceSource {
                variations: vec![
                    VariationSetting {
                        tag: *b"wght",
                        value: 400.0,
                    },
                    VariationSetting {
                        tag: *b"opsz",
                        value: 17.0,
                    },
                ],
                ..FaceSource::blob(variable.clone(), 0)
            },
        );
        backend.default_with(
            GenericSlot::Serif,
            FaceSource {
                synthesis_hint: Some(Synthesis::default()),
                ..FaceSource::blob(static_serif.clone(), 0)
            },
        );
        let mut oracle = oracle_over(backend);

        let mut bold = req("a", b"Latn", &FontFamily::SystemUi);
        bold.weight = FontWeight::BOLD;
        for _ in 0..2 {
            let face = oracle.resolve(&bold).unwrap();
            assert_eq!(face.blob.id(), variable.id());
            assert_eq!(
                face.synthesis.variation(*b"wght"),
                Some(700.0),
                "bold coords"
            );
            assert_eq!(face.synthesis.variation(*b"opsz"), Some(17.0));
        }

        let serif = FontFamily::stack_with_generic(Vec::<String>::new(), GenericSlot::Serif);
        let mut bold = req("a", b"Latn", &serif);
        bold.weight = FontWeight::BOLD;
        let face = oracle.resolve(&bold).unwrap();
        assert_eq!(face.blob.id(), static_serif.id());
        assert!(face.synthesis.embolden, "faux bold, not the regular hint");
        assert_eq!(oracle.stats().generic_hits, 2);
    }
}
