//! Sanctioned unsafe zone (decision D-6): FFI to fontconfig, reached through `dlopen`.
//! Every `unsafe` block carries a SAFETY note, and the FFI stays behind safe
//! wrappers that never leak past the `Backend` trait. No other oracle module
//! may contain `unsafe`; the guard tests in `oracle/mod.rs` enforce it.
//!
//! The fontconfig oracle backend (Linux, FreeBSD).
//!
//! Compiled only for `target_os = "linux"` or `"freebsd"` (the cfg gate is on this module's `mod`
//! line in `oracle/mod.rs`). `libfontconfig` is opened with `dlopen` at construction, so a host
//! without it simply gets no answers (one logged warning) and the app-font registry covers what
//! it can.
//!
//! A question builds an `FcPattern` (the request's first named family, then the chain's generic
//! families, weight, slant, width, the locale's language, and the cluster's code points as a
//! charset), runs `FcConfigSubstitute` + `FcDefaultSubstitute`, and asks `FcFontSort` for the
//! coverage-ranked candidate set (`trim = true`). Each candidate goes through
//! `FcFontRenderPrepare` and its charset is tested for every code point of the cluster (a
//! candidate without a charset is skipped, never taken as covering); the first full match becomes
//! the [`FaceSource`] (`FC_FILE`, `FC_INDEX`, and `FC_POSTSCRIPT_NAME` when present, so the
//! selector can tell the asked family from a substitute).
//!
//! Synthesis: this backend never reads `FC_EMBOLDEN` / `FC_MATRIX`, so every answer carries
//! `synthesis_hint: None` (the oracle applies its own weight/slant rule); it never mixes the two.
//! A variable font's named instance is a later refinement, so `variations` stays empty.
//!
//! Logging prints the cluster's code-point count and the request's script tag, never its
//! characters.

#![allow(unsafe_code)]

use std::ffi::{CStr, CString, c_char, c_int};
use std::path::PathBuf;
use std::ptr;
use std::time::Instant;

use fontconfig_sys::constants::{
    FC_CHARSET, FC_COLOR, FC_FAMILY, FC_FILE, FC_INDEX, FC_LANG, FC_POSTSCRIPT_NAME, FC_SLANT,
    FC_SLANT_ITALIC, FC_SLANT_OBLIQUE, FC_SLANT_ROMAN, FC_WEIGHT, FC_WIDTH,
};
use fontconfig_sys::{
    Fc, FcChar8, FcCharSet, FcFontSet, FcMatchPattern, FcPattern, FcResult, FcResultMatch,
};

use super::{Backend, FaceSource, FallbackRequest};
use crate::style::{FamilyName, FontStyle, GenericSlot};

/// The shared object fontconfig is opened from.
const SONAME: &str = "libfontconfig.so.1";

/// The `dlopen`ed fontconfig entry points.
struct Library(Fc);

// SAFETY: `Fc` is a library handle plus plain function pointers, with no thread affinity.
// fontconfig's global `FcConfig` is internally reference-counted and safe to query from any
// thread, and this backend only calls it through `&self`/`&mut self` from its single owner.
unsafe impl Send for Library {}

impl Library {
    /// Opens `soname`, logging one warning when it cannot be loaded.
    fn open(soname: &str) -> Option<Self> {
        // SAFETY: `Fc::open` runs the loaded library's initialisers and resolves symbols with
        // the signatures declared by `yeslogic-fontconfig-sys`; `soname` is a plain library
        // name, not caller-controlled text.
        match unsafe { Fc::open(soname) } {
            Ok(fc) => Some(Self(fc)),
            Err(err) => {
                log::warn!("frust-text: fontconfig unavailable ({err}); no system font fallback");
                None
            }
        }
    }

    fn version(&self) -> i32 {
        // SAFETY: `FcGetVersion` takes no arguments and returns a plain integer.
        unsafe { (self.0.FcGetVersion)() }
    }
}

/// An `FcPattern` this wrapper owns and destroys.
struct Pattern<'a> {
    lib: &'a Library,
    raw: *mut FcPattern,
}

impl<'a> Pattern<'a> {
    fn new(lib: &'a Library) -> Option<Self> {
        // SAFETY: `FcPatternCreate` allocates a pattern the caller owns (null on OOM).
        let raw = unsafe { (lib.0.FcPatternCreate)() };
        (!raw.is_null()).then_some(Self { lib, raw })
    }

    /// Takes ownership of a pattern returned by fontconfig (e.g. `FcFontRenderPrepare`).
    fn adopt(lib: &'a Library, raw: *mut FcPattern) -> Option<Self> {
        (!raw.is_null()).then_some(Self { lib, raw })
    }

    fn add_string(&mut self, object: &CStr, value: &str) {
        let Ok(value) = CString::new(value) else {
            return;
        };
        // SAFETY: `self.raw` is a live pattern; `object` and `value` are NUL-terminated and
        // outlive the call, and fontconfig copies the string.
        unsafe {
            (self.lib.0.FcPatternAddString)(
                self.raw,
                object.as_ptr(),
                value.as_ptr().cast::<FcChar8>(),
            );
        }
    }

    fn add_integer(&mut self, object: &CStr, value: i32) {
        // SAFETY: `self.raw` is a live pattern and `object` is NUL-terminated.
        unsafe { (self.lib.0.FcPatternAddInteger)(self.raw, object.as_ptr(), value) };
    }

    fn add_bool(&mut self, object: &CStr, value: bool) {
        // SAFETY: `self.raw` is a live pattern and `object` is NUL-terminated.
        unsafe { (self.lib.0.FcPatternAddBool)(self.raw, object.as_ptr(), c_int::from(value)) };
    }

    fn add_double(&mut self, object: &CStr, value: f64) {
        // SAFETY: `self.raw` is a live pattern and `object` is NUL-terminated.
        unsafe { (self.lib.0.FcPatternAddDouble)(self.raw, object.as_ptr(), value) };
    }

    fn add_charset(&mut self, chars: &[u32]) {
        // SAFETY: `FcCharSetCreate` returns an owned set (null on OOM, checked). It is
        // populated, handed to `FcPatternAddCharSet` (which takes its own reference), and then
        // destroyed here exactly once, so no reference outlives the set's last owner.
        unsafe {
            let set: *mut FcCharSet = (self.lib.0.FcCharSetCreate)();
            if set.is_null() {
                return;
            }
            for &c in chars {
                (self.lib.0.FcCharSetAddChar)(set, c);
            }
            (self.lib.0.FcPatternAddCharSet)(self.raw, FC_CHARSET.as_ptr(), set);
            (self.lib.0.FcCharSetDestroy)(set);
        }
    }

    /// Applies the configuration's rules and fontconfig's defaults.
    fn substitute(&mut self) {
        // SAFETY: a null config selects the current (lazily initialised) configuration; `raw`
        // is a live pattern.
        unsafe {
            (self.lib.0.FcConfigSubstitute)(ptr::null_mut(), self.raw, FcMatchPattern);
            (self.lib.0.FcDefaultSubstitute)(self.raw);
        }
    }

    /// A string property, copied out of the pattern.
    fn string(&self, object: &CStr) -> Option<String> {
        let mut out: *mut FcChar8 = ptr::null_mut();
        // SAFETY: `raw` is live and `object` NUL-terminated; on a match `out` points at a
        // NUL-terminated string owned by the pattern, copied before the pattern can change.
        unsafe {
            let result: FcResult = (self.lib.0.FcPatternGetString)(
                self.raw,
                object.as_ptr(),
                0,
                ptr::addr_of_mut!(out),
            );
            if result != FcResultMatch || out.is_null() {
                return None;
            }
            Some(
                CStr::from_ptr(out.cast::<c_char>())
                    .to_string_lossy()
                    .into_owned(),
            )
        }
    }

    fn integer(&self, object: &CStr) -> Option<i32> {
        let mut out: c_int = 0;
        // SAFETY: `raw` is live, `object` NUL-terminated, `out` is a valid `c_int` slot.
        let result = unsafe {
            (self.lib.0.FcPatternGetInteger)(self.raw, object.as_ptr(), 0, ptr::addr_of_mut!(out))
        };
        (result == FcResultMatch).then_some(out)
    }

    /// Whether the pattern's charset covers every one of `chars` (true for an empty slice).
    /// `None` when the pattern has no charset.
    fn covers(&self, chars: &[u32]) -> Option<bool> {
        let mut set: *mut FcCharSet = ptr::null_mut();
        // SAFETY: `raw` is live. On a match `set` points at a charset owned by the pattern; it
        // is only used inside this call, while `self` (and so the pattern) is borrowed.
        unsafe {
            let result = (self.lib.0.FcPatternGetCharSet)(
                self.raw,
                FC_CHARSET.as_ptr(),
                0,
                ptr::addr_of_mut!(set),
            );
            if result != FcResultMatch || set.is_null() {
                return None;
            }
            Some(
                chars
                    .iter()
                    .all(|&c| (self.lib.0.FcCharSetHasChar)(set, c) != 0),
            )
        }
    }
}

impl Drop for Pattern<'_> {
    fn drop(&mut self) {
        // SAFETY: `raw` is an owned, non-null pattern that is destroyed exactly once, here.
        unsafe { (self.lib.0.FcPatternDestroy)(self.raw) };
    }
}

/// The `FcFontSort` result, destroyed on drop. Its patterns belong to the set.
struct SortedSet<'a> {
    lib: &'a Library,
    raw: *mut FcFontSet,
}

impl<'a> SortedSet<'a> {
    fn sort(lib: &'a Library, pattern: &Pattern<'_>) -> Option<Self> {
        let mut result: FcResult = 0;
        // SAFETY: a null config selects the current configuration, `pattern.raw` is live,
        // `trim = 1`, the optional charset out-parameter is null, and `result` is a valid slot.
        let raw = unsafe {
            (lib.0.FcFontSort)(
                ptr::null_mut(),
                pattern.raw,
                1,
                ptr::null_mut(),
                ptr::addr_of_mut!(result),
            )
        };
        (!raw.is_null()).then_some(Self { lib, raw })
    }

    fn len(&self) -> usize {
        // SAFETY: `raw` is a live set returned by `FcFontSort`.
        usize::try_from(unsafe { (*self.raw).nfont }).unwrap_or(0)
    }

    /// `FcFontRenderPrepare` of candidate `i` against `pattern`.
    fn prepare(&self, pattern: &Pattern<'a>, i: usize) -> Option<Pattern<'a>> {
        if i >= self.len() {
            return None;
        }
        // SAFETY: `i < nfont`, so `fonts.add(i)` is inside the array the set owns; the font
        // pattern it points at lives until the set is destroyed, after this call. The returned
        // pattern is a fresh copy the caller owns (adopted into a `Pattern`).
        unsafe {
            let font = *(*self.raw).fonts.add(i);
            if font.is_null() {
                return None;
            }
            let prepared = (self.lib.0.FcFontRenderPrepare)(ptr::null_mut(), pattern.raw, font);
            Pattern::adopt(self.lib, prepared)
        }
    }
}

impl Drop for SortedSet<'_> {
    fn drop(&mut self) {
        // SAFETY: `raw` came from `FcFontSort` and is destroyed exactly once, here, with the
        // matching destroy function; no borrowed pattern outlives `self`.
        unsafe { (self.lib.0.FcFontSetDestroy)(self.raw) };
    }
}

/// What to ask fontconfig.
struct Query<'a> {
    families: Vec<&'a str>,
    /// Code points the answer must cover (empty for a plain default-family lookup).
    chars: Vec<u32>,
    weight: f32,
    slant: FontStyle,
    width: f32,
    lang: Option<String>,
    /// Prefer a colour face (the chain names the emoji generic).
    color: bool,
}

/// Answers fallback questions through fontconfig.
pub(crate) struct FontconfigBackend {
    lib: Option<Library>,
    build_id: String,
}

impl std::fmt::Debug for FontconfigBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontconfigBackend")
            .field("loaded", &self.lib.is_some())
            .finish()
    }
}

impl Default for FontconfigBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl FontconfigBackend {
    pub(crate) fn new() -> Self {
        Self::with_soname(SONAME)
    }

    /// A backend over the library named `soname` (tests point this at a bogus name).
    fn with_soname(soname: &str) -> Self {
        let lib = Library::open(soname);
        let build_id = build_id(lib.as_ref().map(Library::version));
        Self { lib, build_id }
    }

    /// Runs `query`; the first candidate covering every code point wins.
    fn answer(&self, query: &Query<'_>) -> Option<FaceSource> {
        let lib = self.lib.as_ref()?;
        let mut pattern = Pattern::new(lib)?;
        for family in &query.families {
            pattern.add_string(FC_FAMILY, family);
        }
        pattern.add_integer(FC_WEIGHT, fc_weight(query.weight));
        pattern.add_integer(FC_SLANT, fc_slant(&query.slant));
        pattern.add_double(FC_WIDTH, f64::from(query.width) * 100.0);
        if query.color {
            pattern.add_bool(FC_COLOR, true);
        }
        if let Some(lang) = &query.lang {
            pattern.add_string(FC_LANG, lang);
        }
        if !query.chars.is_empty() {
            pattern.add_charset(&query.chars);
        }
        pattern.substitute();
        let sorted = SortedSet::sort(lib, &pattern)?;
        for i in 0..sorted.len() {
            let Some(prepared) = sorted.prepare(&pattern, i) else {
                continue;
            };
            // No charset is not coverage: only a candidate whose charset holds every code
            // point is an answer.
            if prepared.covers(&query.chars) != Some(true) {
                continue;
            }
            let Some(file) = prepared.string(FC_FILE) else {
                continue;
            };
            let index = prepared
                .integer(FC_INDEX)
                .and_then(|i| u32::try_from(i).ok())
                .unwrap_or(0);
            let mut source = FaceSource::path(PathBuf::from(file), index);
            source.postscript_name = prepared.string(FC_POSTSCRIPT_NAME);
            return Some(source);
        }
        None
    }
}

impl Backend for FontconfigBackend {
    fn resolve(&mut self, req: &FallbackRequest) -> Option<FaceSource> {
        self.lib.as_ref()?;
        let started = Instant::now();
        let chars: Vec<u32> = req
            .chars
            .chars()
            .filter(|&c| needs_coverage(c))
            .map(u32::from)
            .collect();
        let named = req.families.iter().find_map(|f| match f {
            FamilyName::Named(name) => Some(name.as_str()),
            FamilyName::Generic(_) => None,
        });
        let mut families: Vec<&str> = named.into_iter().collect();
        let generics = req.generics();
        for generic in &generics {
            let generic = *generic;
            let name = generic_name(generic);
            if !families.contains(&name) {
                families.push(name);
            }
        }
        let query = Query {
            families,
            chars,
            weight: req.weight.value(),
            slant: req.slant,
            width: req.width,
            lang: super::language_for(req.script, req.locale.as_deref()).and_then(fc_lang),
            color: generics.contains(&GenericSlot::Emoji),
        };
        let answer = self.answer(&query);
        log::debug!(
            "fontconfig match: {} code points, script {} -> {} ({} us)",
            req.chars.chars().count(),
            String::from_utf8_lossy(&req.script.0),
            if answer.is_some() { "hit" } else { "miss" },
            started.elapsed().as_micros(),
        );
        answer
    }

    fn default_family(&self, generic: GenericSlot) -> Option<FaceSource> {
        self.answer(&Query {
            families: vec![generic_name(generic)],
            chars: Vec::new(),
            weight: 400.0,
            slant: FontStyle::Normal,
            width: 1.0,
            lang: None,
            color: generic == GenericSlot::Emoji,
        })
    }

    fn os_build_id(&self) -> String {
        self.build_id.clone()
    }
}

/// The fontconfig family name for a generic slot.
fn generic_name(slot: GenericSlot) -> &'static str {
    match slot {
        GenericSlot::Monospace => "monospace",
        GenericSlot::Serif => "serif",
        GenericSlot::Emoji => "emoji",
        GenericSlot::SansSerif | GenericSlot::SystemUi => "sans-serif",
    }
}

/// Whether a code point must be in a candidate's charset. Controls and default-ignorable
/// joiners/selectors are not drawn on their own and are rarely in a font's charset.
fn needs_coverage(c: char) -> bool {
    !(c.is_control()
        || matches!(u32::from(c),
            0x034F | 0x180B..=0x180F | 0x200B..=0x200F | 0x2060..=0x2064
            | 0xFE00..=0xFE0F | 0xE0001 | 0xE0020..=0xE007F | 0xE0100..=0xE01EF))
}

/// OpenType (CSS) weight to fontconfig's scale, interpolating between its named steps.
fn fc_weight(weight: f32) -> i32 {
    const STEPS: [(f32, f32); 11] = [
        (100.0, 0.0),
        (200.0, 40.0),
        (300.0, 50.0),
        (350.0, 75.0),
        (400.0, 80.0),
        (500.0, 100.0),
        (600.0, 180.0),
        (700.0, 200.0),
        (800.0, 205.0),
        (900.0, 210.0),
        (1000.0, 215.0),
    ];
    let w = weight.clamp(STEPS[0].0, STEPS[STEPS.len() - 1].0);
    for pair in STEPS.windows(2) {
        let ((lo, flo), (hi, fhi)) = (pair[0], pair[1]);
        if w <= hi {
            return (flo + (fhi - flo) * (w - lo) / (hi - lo)).round() as i32;
        }
    }
    STEPS[STEPS.len() - 1].1 as i32
}

fn fc_slant(style: &FontStyle) -> i32 {
    match style {
        FontStyle::Normal => FC_SLANT_ROMAN,
        FontStyle::Italic => FC_SLANT_ITALIC,
        FontStyle::Oblique(_) => FC_SLANT_OBLIQUE,
    }
}

/// A BCP 47 locale as a fontconfig language tag (`zh-Hant` -> `zh-tw`, `en_US` -> `en-us`).
fn fc_lang(locale: &str) -> Option<String> {
    let lower = locale.trim().replace('_', "-").to_ascii_lowercase();
    let mut parts = lower.split('-');
    let lang = parts.next().filter(|l| !l.is_empty())?;
    let rest: Vec<&str> = parts.collect();
    if lang == "zh" {
        let traditional = rest
            .iter()
            .any(|p| matches!(*p, "hant" | "tw" | "hk" | "mo"));
        return Some(String::from(if traditional { "zh-tw" } else { "zh-cn" }));
    }
    // A four-letter subtag is a script, not a region; fontconfig tags carry regions only.
    match rest.iter().find(|p| p.len() == 2) {
        Some(region) => Some(format!("{lang}-{region}")),
        None => Some(lang.to_owned()),
    }
}

/// A stable identity of the font stack: the OS release plus fontconfig's version. Never the
/// fontconfig cache mtime (`font_dirs()` stamps those directories already).
fn build_id(fc_version: Option<i32>) -> String {
    let os = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    format!(
        "fontconfig:{}:fc{}",
        os_release_identity(&os),
        fc_version.map_or_else(|| String::from("none"), |v| v.to_string())
    )
}

/// `ID`, `VERSION_ID` and `BUILD_ID` of an os-release file, joined (`-` when absent).
fn os_release_identity(text: &str) -> String {
    let field = |key: &str| {
        text.lines()
            .filter_map(|line| line.trim().strip_prefix(key)?.strip_prefix('='))
            .map(|v| v.trim().trim_matches(['"', '\'']).to_owned())
            .find(|v| !v.is_empty())
    };
    let mut parts = vec![
        field("ID").unwrap_or_else(|| String::from("-")),
        field("VERSION_ID").unwrap_or_else(|| String::from("-")),
    ];
    parts.extend(field("BUILD_ID"));
    parts.join(":")
}

#[cfg(test)]
impl FontconfigBackend {
    /// How many installed faces cover code point `c`, by `FcFontList` over a charset pattern —
    /// a precondition independent of [`Self::answer`]'s sort-and-prepare path, so a host test
    /// can tell "no face covers this here" from a backend bug. `None` without fontconfig.
    fn faces_covering(&self, c: u32) -> Option<usize> {
        let lib = self.lib.as_ref()?;
        let mut pattern = Pattern::new(lib)?;
        pattern.add_charset(&[c]);
        listed(lib, &pattern)
    }

    /// How many installed faces belong to `family`, by `FcFontList` over a family pattern (no
    /// `fc-list` subprocess). `None` without fontconfig.
    fn faces_in_family(&self, family: &str) -> Option<usize> {
        let lib = self.lib.as_ref()?;
        let mut pattern = Pattern::new(lib)?;
        pattern.add_string(FC_FAMILY, family);
        listed(lib, &pattern)
    }
}

/// How many installed faces `FcFontList` lists for `pattern`.
#[cfg(test)]
fn listed(lib: &Library, pattern: &Pattern<'_>) -> Option<usize> {
    // SAFETY: `FcObjectSetCreate` returns an owned object set (null on OOM, checked) that is
    // destroyed exactly once below; `FC_FILE` is NUL-terminated and copied by
    // `FcObjectSetAdd`. A null config selects the current configuration and `pattern.raw` is
    // live for the call. The returned font set is owned here: `nfont` is read while it is
    // live and it is destroyed exactly once with the matching destroy function.
    unsafe {
        let objects = (lib.0.FcObjectSetCreate)();
        if objects.is_null() {
            return None;
        }
        (lib.0.FcObjectSetAdd)(objects, FC_FILE.as_ptr());
        let set = (lib.0.FcFontList)(ptr::null_mut(), pattern.raw, objects);
        (lib.0.FcObjectSetDestroy)(objects);
        if set.is_null() {
            return None;
        }
        let count = usize::try_from((*set).nfont).unwrap_or(0);
        (lib.0.FcFontSetDestroy)(set);
        Some(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oracle::{FaceLocation, RenderCapabilities, ScriptTag};
    use crate::style::{FontFamily, FontWeight};

    fn request(chars: &str, families: Vec<FamilyName>) -> FallbackRequest {
        let mut req = FallbackRequest::new(
            chars,
            ScriptTag::COMMON,
            &FontFamily::SystemUi,
            RenderCapabilities::default(),
        );
        req.families = families;
        req
    }

    fn generic(chars: &str) -> FallbackRequest {
        request(chars, vec![FamilyName::Generic(GenericSlot::SansSerif)])
    }

    /// The backend, or `None` (test skipped) when this host has no fontconfig.
    fn host() -> Option<FontconfigBackend> {
        let backend = FontconfigBackend::new();
        if backend.lib.is_none() {
            eprintln!("skipping: libfontconfig not available");
            return None;
        }
        Some(backend)
    }

    fn file_name(source: &FaceSource) -> String {
        match &source.location {
            FaceLocation::Path(p) => p.file_name().unwrap().to_string_lossy().to_lowercase(),
            FaceLocation::Blob(_) => panic!("expected a path"),
        }
    }

    fn family_installed(backend: &FontconfigBackend, family: &str) -> bool {
        backend.faces_in_family(family).is_some_and(|n| n > 0)
    }

    #[test]
    fn latin_resolves_to_a_path_face() {
        let Some(mut b) = host() else { return };
        let answer = b.resolve(&generic("Hello")).expect("latin resolves");
        assert!(matches!(answer.location, FaceLocation::Path(_)));
        assert!(!answer.hidden);
        assert_eq!(answer.scale, None);
        assert!(answer.variations.is_empty());
        assert_eq!(answer.synthesis_hint, None);
        assert!(answer.postscript_name.is_some());
    }

    /// The answer's PostScript name plus every family, full and PostScript `name` record of
    /// its file, lower-cased, for the "looks CJK" check.
    fn face_names(source: &FaceSource) -> Vec<String> {
        use read_fonts::{FontRef, TableProvider, types::NameId};
        let mut names: Vec<String> = source.postscript_name.iter().cloned().collect();
        let FaceLocation::Path(path) = &source.location else {
            panic!("expected a path");
        };
        let bytes = std::fs::read(path).expect("the answered file is readable");
        if let Ok(table) = FontRef::from_index(&bytes, source.index).and_then(|f| f.name()) {
            let ids = [
                NameId::FAMILY_NAME,
                NameId::TYPOGRAPHIC_FAMILY_NAME,
                NameId::FULL_NAME,
                NameId::POSTSCRIPT_NAME,
            ];
            names.extend(
                table
                    .name_record()
                    .iter()
                    .filter(|record| ids.contains(&record.name_id()))
                    .filter_map(|record| record.string(table.string_data()).ok())
                    .map(|string| string.chars().collect::<String>()),
            );
        }
        names.iter().map(|n| n.to_lowercase()).collect()
    }

    /// Name fragments of the Han-covering families a Linux host ships (Noto/Source Han CJK,
    /// Droid Sans Fallback, WenQuanYi, AR PL, IPA, Takao, VL Gothic, Sarasa, ...).
    const CJK_MARKERS: [&str; 22] = [
        "cjk",
        "droid sans fallback",
        "droidsansfallback",
        "wenquanyi",
        "wqy",
        "source han",
        "sourcehan",
        "noto sans sc",
        "noto sans tc",
        "noto sans jp",
        "noto sans kr",
        "noto serif sc",
        "notosanssc",
        "notosansjp",
        "ar pl",
        "uming",
        "ukai",
        "ipa",
        "takao",
        "vl gothic",
        "sarasa",
        "unifont",
    ];

    #[test]
    fn han_resolves_to_a_cjk_face() {
        let Some(mut b) = host() else { return };
        // The precondition comes from `FcFontList`, not from `resolve()`: a backend bug that
        // misses Han must fail here, never skip.
        let covering = b.faces_covering(0x4E2D).expect("FcFontList answers");
        if covering == 0 {
            eprintln!("skipping: no installed face covers U+4E2D");
            return;
        }
        let mut req = generic("\u{4E2D}\u{6587}");
        for locale in ["zh-Hans", "ja"] {
            req.locale = Some(String::from(locale));
            let answer = b
                .resolve(&req)
                .unwrap_or_else(|| panic!("{covering} faces cover U+4E2D, yet {locale} missed"));
            let names = face_names(&answer);
            assert!(
                names
                    .iter()
                    .any(|name| CJK_MARKERS.iter().any(|marker| name.contains(marker))),
                "{locale}: the Han answer does not look CJK: {names:?} ({answer:?})"
            );
        }
    }

    #[test]
    fn a_code_point_no_face_has_lists_nothing() {
        let Some(b) = host() else { return };
        assert_eq!(b.faces_covering(0x10_FFFD), Some(0));
        assert!(b.faces_covering(u32::from('a')).is_some_and(|n| n > 0));
    }

    #[test]
    fn emoji_resolves_to_a_colour_face_when_installed() {
        let Some(mut b) = host() else { return };
        let req = request("\u{1F600}", vec![FamilyName::Generic(GenericSlot::Emoji)]);
        let answer = b.resolve(&req);
        if family_installed(&b, "Noto Color Emoji") {
            let answer = answer.expect("emoji resolves");
            assert!(file_name(&answer).contains("emoji"), "{answer:?}");
        }
    }

    #[test]
    fn uncovered_cluster_is_a_miss() {
        let Some(mut b) = host() else { return };
        // A private-use plane-16 code point no stock font covers.
        assert_eq!(b.resolve(&generic("\u{10FFFD}")), None);
    }

    #[test]
    fn installed_named_family_answers_itself() {
        let Some(mut b) = host() else { return };
        if !family_installed(&b, "Noto Sans CJK JP") {
            return;
        }
        let req = request(
            "Hello",
            vec![
                FamilyName::Named(String::from("Noto Sans CJK JP")),
                FamilyName::Generic(GenericSlot::SansSerif),
            ],
        );
        let answer = b.resolve(&req).expect("named family resolves");
        let ps = answer.postscript_name.expect("postscript name filled");
        assert!(ps.starts_with("NotoSansCJKjp"), "{ps}");
    }

    #[test]
    fn uninstalled_named_family_answers_the_cascade() {
        let Some(mut b) = host() else { return };
        let req = request(
            "Hello",
            vec![
                FamilyName::Named(String::from("Definitely Not An Installed Family")),
                FamilyName::Generic(GenericSlot::SansSerif),
            ],
        );
        let answer = b.resolve(&req).expect("cascade answers");
        assert!(matches!(answer.location, FaceLocation::Path(_)));
    }

    #[test]
    fn bold_and_italic_requests_do_not_panic() {
        let Some(mut b) = host() else { return };
        let mut req = generic("Hello");
        req.weight = FontWeight::BOLD;
        req.slant = FontStyle::Italic;
        assert!(b.resolve(&req).is_some());
    }

    #[test]
    fn default_families_answer() {
        let Some(b) = host() else { return };
        for slot in [
            GenericSlot::SansSerif,
            GenericSlot::Serif,
            GenericSlot::Monospace,
            GenericSlot::SystemUi,
        ] {
            assert!(b.default_family(slot).is_some(), "{slot:?}");
        }
    }

    #[test]
    fn missing_library_degrades_to_none() {
        let mut b = FontconfigBackend::with_soname("libfrust-no-such-fontconfig.so.0");
        assert!(b.lib.is_none());
        assert_eq!(b.resolve(&generic("Hello")), None);
        assert_eq!(b.default_family(GenericSlot::SansSerif), None);
        assert!(b.os_build_id().ends_with(":fcnone"));
    }

    #[test]
    fn build_id_is_stable() {
        let Some(b) = host() else { return };
        assert_eq!(b.os_build_id(), FontconfigBackend::new().os_build_id());
        assert!(b.os_build_id().starts_with("fontconfig:"));
    }

    #[test]
    fn os_release_fields_are_parsed() {
        let text = "NAME=\"X\"\nID=debian\nVERSION_ID=\"13\"\nBUILD_ID=rolling\n";
        assert_eq!(os_release_identity(text), "debian:13:rolling");
        assert_eq!(os_release_identity(""), "-:-");
    }

    #[test]
    fn weights_map_onto_fontconfig_steps() {
        assert_eq!(fc_weight(400.0), 80);
        assert_eq!(fc_weight(700.0), 200);
        assert_eq!(fc_weight(100.0), 0);
        assert_eq!(fc_weight(5000.0), 215);
        assert!(fc_weight(450.0) > 80 && fc_weight(450.0) < 100);
    }

    #[test]
    fn locales_map_onto_fontconfig_languages() {
        assert_eq!(fc_lang("zh-Hant").as_deref(), Some("zh-tw"));
        assert_eq!(fc_lang("zh-Hans-CN").as_deref(), Some("zh-cn"));
        assert_eq!(fc_lang("ja").as_deref(), Some("ja"));
        assert_eq!(fc_lang("en_US").as_deref(), Some("en-us"));
        assert_eq!(fc_lang("sr-Latn").as_deref(), Some("sr"));
        assert_eq!(fc_lang(""), None);
    }

    #[test]
    fn joiners_and_controls_need_no_coverage() {
        assert!(!needs_coverage('\u{200D}'));
        assert!(!needs_coverage('\u{FE0F}'));
        assert!(!needs_coverage('\n'));
        assert!(needs_coverage('a'));
    }
}
