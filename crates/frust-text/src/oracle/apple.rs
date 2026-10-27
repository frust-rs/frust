//! Sanctioned unsafe zone (decision D-6): FFI to CoreText, Apple's font API.
//! Every `unsafe` block carries a SAFETY note, and the FFI stays behind safe
//! wrappers that never leak past the `Backend` trait. No other oracle module
//! may contain `unsafe`; the guard tests in `oracle/mod.rs` enforce it.
//!
//! The CoreText oracle backend (macOS, iOS).
//!
//! Compiled only for `target_vendor = "apple"` (the cfg gate is on this module's `mod` line in
//! `oracle/mod.rs`). Per miss: a base `CTFont` for the request (the chain's first family CoreText
//! knows, else the generic slot's; weight/slant/width as `kCTFontTraitsAttribute`), then
//! `CTFontCreateForStringWithLanguage` on the cluster. The answer's PostScript name and
//! `kCTFontURLAttribute` become (file, face index) by a walk of only the file's TTC header, table
//! directories, `name` and `fvar` tables: name ID 6, else the `fvar` named instance whose
//! `postScriptNameID` matches. CoreText's own descriptor order is not the face index.
//!
//! Dot-prefixed (hidden) answers are returned with `hidden` set, never swapped for a sibling face.
//! One exception: a hidden face with only `hvgl` glyph data is re-asked under a public base
//! (Helvetica, same traits and locale), whose public answer replaces it. `CTFontCreateWithName` is
//! never called: on a hidden name it returns Times.
//!
//! [`preferred_languages`] reads the user's language list (`CFLocaleCopyPreferredLanguages`),
//! from which `oracle/mod.rs` picks the Han language of a Han question that carries no locale.

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::hash_map::Entry;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Instant;

use objc2_core_foundation::{
    CFArray, CFDictionary, CFLocale, CFNumber, CFRange, CFRetained, CFString, CFType, CFURL,
};
use objc2_core_text::{
    CTFont, CTFontDescriptor, CTFontSymbolicTraits, CTFontUIFontType, kCTFontFamilyNameAttribute,
    kCTFontSlantTrait, kCTFontSymbolicTrait, kCTFontTraitsAttribute, kCTFontURLAttribute,
    kCTFontWeightTrait, kCTFontWidthTrait,
};
use read_fonts::tables::fvar::Fvar;
use read_fonts::tables::name::Name;
use read_fonts::types::NameId;
use read_fonts::{FontData, FontRead};

use super::{Backend, FaceSource, FallbackRequest, VariationSetting, language_for};
use crate::style::{FamilyName, FontStyle, GenericSlot};

/// Point size of every base font. Only `opsz` depends on it, and `opsz` is dropped.
const BASE_POINT_SIZE: f64 = 17.0;

/// The public base a hidden `hvgl`-only answer is re-asked under.
const PUBLIC_BASE_FAMILY: &str = "Helvetica";

/// CoreText's "nothing installed covers this" face; treated as a miss.
const LAST_RESORT: &str = "LastResort";

/// Where the OS build lives on macOS and iOS alike.
const SYSTEM_VERSION_PLIST: &str = "/System/Library/CoreServices/SystemVersion.plist";

/// Largest `name`/`fvar` table the face walk reads; anything bigger is a corrupt directory.
const MAX_TABLE_BYTES: u32 = 4 << 20;

/// Total bytes one file's face walk may read (headers, directories, tables). PingFangUI.ttc, the
/// largest system collection walked, reads well under 1 MiB; the cap stops a hostile or corrupt
/// TTC header (up to 65535 face offsets) from turning one miss into a bulk read.
const MAX_WALK_BYTES: u64 = 16 << 20;

/// Distinct `name`/`fvar` tables one walk may read (a collection's faces often share them).
const MAX_WALK_TABLES: usize = 256;

/// CSS weight to CoreText's normalized `kCTFontWeightTrait`, at AppKit's `NSFontWeight` stops.
const WEIGHT_STOPS: [(f32, f32); 9] = [
    (100.0, -0.8),
    (200.0, -0.6),
    (300.0, -0.4),
    (400.0, 0.0),
    (500.0, 0.23),
    (600.0, 0.3),
    (700.0, 0.4),
    (800.0, 0.56),
    (900.0, 0.62),
];

/// `kCTFontSlantTrait` is normalized so that 1.0 is 30 degrees.
const SLANT_FULL_DEGREES: f32 = 30.0;

/// The slant asked for italic, or for an oblique with no angle (CSS's 14 degrees).
const DEFAULT_SLANT_DEGREES: f32 = 14.0;

/// Requested weight/slant/width.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Traits {
    weight: f32,
    slant: FontStyle,
    width: f32,
}

impl Traits {
    fn of(req: &FallbackRequest) -> Self {
        Self {
            weight: req.weight.value(),
            slant: req.slant,
            width: req.width,
        }
    }

    const REGULAR: Self = Self {
        weight: 400.0,
        slant: FontStyle::Normal,
        width: 1.0,
    };

    fn normalized_weight(self) -> f64 {
        let w = self.weight.clamp(100.0, 900.0);
        for pair in WEIGHT_STOPS.windows(2) {
            let ((w0, n0), (w1, n1)) = (pair[0], pair[1]);
            if w <= w1 {
                return f64::from(n0 + (n1 - n0) * (w - w0) / (w1 - w0));
            }
        }
        f64::from(WEIGHT_STOPS[WEIGHT_STOPS.len() - 1].1)
    }

    fn slant_degrees(self) -> f32 {
        match self.slant {
            FontStyle::Normal => 0.0,
            FontStyle::Italic => DEFAULT_SLANT_DEGREES,
            FontStyle::Oblique(angle) => angle.unwrap_or(DEFAULT_SLANT_DEGREES),
        }
    }

    /// Width ratio to `kCTFontWidthTrait` (-1 condensed .. 1 expanded): 0.5 and 2.0 are the ends.
    fn normalized_width(self) -> f64 {
        let w = self.width.clamp(0.5, 2.0);
        let n = if w < 1.0 { (w - 1.0) / 0.5 } else { w - 1.0 };
        f64::from(n)
    }

    fn symbolic(self) -> u32 {
        let mut bits = 0;
        if self.slant != FontStyle::Normal {
            bits |= CTFontSymbolicTraits::TraitItalic.0;
        }
        if self.weight >= 600.0 {
            bits |= CTFontSymbolicTraits::TraitBold.0;
        }
        bits
    }

    /// Hashable form for the base-font cache: weight, slant kind, slant angle, width.
    fn key(self) -> (u32, u8, u32, u32) {
        let kind = match self.slant {
            FontStyle::Normal => 0,
            FontStyle::Italic => 1,
            FontStyle::Oblique(_) => 2,
        };
        (
            self.weight.to_bits(),
            kind,
            self.slant_degrees().to_bits(),
            self.width.to_bits(),
        )
    }
}

/// The safe wrappers around every CoreText call this backend makes. Every Create/Copy call returns
/// a +1 reference that objc2 wraps in `CFRetained` (the Create rule: released once, on drop), and
/// every borrowed argument is a live local that outlives its call.
mod ct {
    use super::*;

    /// A CoreText font the backend owns; a clone is a `CFRetain`.
    #[derive(Clone)]
    pub(super) struct Font(CFRetained<CTFont>);

    // SAFETY: Apple documents CoreText as thread-safe: "All individual functions in Core Text are
    // thread-safe. Font objects (CTFont, CTFontDescriptor, and associated objects) can be used
    // simultaneously by multiple operations, work queues, or threads." (Core Text Programming
    // Guide, "Core Text Objects Are C-Language Opaque Types"). A `Font` is immutable: it is only
    // read and used as a fallback base, and its `CFRelease` on drop is thread-safe too.
    unsafe impl Send for Font {}

    /// A descriptor naming `family` (when given) and `traits`.
    fn descriptor(family: Option<&str>, traits: Traits) -> CFRetained<CTFontDescriptor> {
        // SAFETY: reading immutable CoreText key constants.
        let (weight_key, slant_key, width_key, symbolic_key, traits_key, family_key) = unsafe {
            (
                kCTFontWeightTrait,
                kCTFontSlantTrait,
                kCTFontWidthTrait,
                kCTFontSymbolicTrait,
                kCTFontTraitsAttribute,
                kCTFontFamilyNameAttribute,
            )
        };
        let weight = CFNumber::new_f64(traits.normalized_weight());
        let slant = CFNumber::new_f64(f64::from(traits.slant_degrees() / SLANT_FULL_DEGREES));
        let width = CFNumber::new_f64(traits.normalized_width());
        let symbolic = CFNumber::new_i64(i64::from(traits.symbolic()));
        let trait_dict = CFDictionary::<CFString, CFType>::from_slices(
            &[weight_key, slant_key, width_key, symbolic_key],
            &[
                weight.as_ref(),
                slant.as_ref(),
                width.as_ref(),
                symbolic.as_ref(),
            ],
        );
        let family_name = family.map(CFString::from_str);
        let mut keys: Vec<&CFString> = vec![traits_key];
        let mut values: Vec<&CFType> = vec![trait_dict.as_ref()];
        if let Some(name) = &family_name {
            keys.push(family_key);
            values.push(name.as_ref());
        }
        let attributes = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
        // SAFETY: the dictionary maps CoreText attribute names to the types they document (a
        // traits dictionary of CFNumbers, a family-name CFString); Create rule.
        unsafe { CTFontDescriptor::with_attributes(attributes.as_opaque()) }
    }

    impl Font {
        /// The system UI font for `language`, at `traits`.
        pub(super) fn system_ui(language: Option<&str>, traits: Traits) -> Option<Self> {
            let language = language.map(CFString::from_str);
            // SAFETY: plain values and an optional live CFString; a null result stays `None`, a
            // non-null one follows the Create rule.
            let ui = unsafe {
                CTFont::new_ui_font_for_language(
                    CTFontUIFontType::System,
                    BASE_POINT_SIZE,
                    language.as_deref(),
                )
            }?;
            if traits == Traits::REGULAR {
                return Some(Self(ui));
            }
            let wanted = descriptor(None, traits);
            // SAFETY: a live font and descriptor; size 0 and a null matrix keep the original's;
            // Create rule.
            let font = unsafe { ui.copy_with_attributes(0.0, core::ptr::null(), Some(&wanted)) };
            Some(Self(font))
        }

        /// The installed `family` at `traits`, or `None` when CoreText matches another family.
        pub(super) fn family(family: &str, traits: Traits) -> Option<Self> {
            let wanted = descriptor(Some(family), traits);
            // SAFETY: a live descriptor; a null matrix is the documented identity argument;
            // Create rule.
            let font = unsafe {
                CTFont::with_font_descriptor(&wanted, BASE_POINT_SIZE, core::ptr::null())
            };
            let font = Self(font);
            font.family_name()
                .eq_ignore_ascii_case(family)
                .then_some(font)
        }

        /// CoreText's face for `text` with this font as the base, under `language`.
        pub(super) fn for_string(&self, text: &str, language: Option<&str>) -> Option<Self> {
            let string = CFString::from_str(text);
            let range = CFRange {
                location: 0,
                length: string.length(),
            };
            if range.length == 0 {
                return None;
            }
            let language = language.map(CFString::from_str);
            // SAFETY: a live font and string, a range inside the string, an optional live
            // language string; Create rule.
            let font = unsafe {
                self.0
                    .for_string_with_language(&string, range, language.as_deref())
            };
            Some(Self(font))
        }

        /// The installed face CoreText opens by `name` (a hidden name it cannot resolve comes
        /// back as another face: callers check `postscript_name`).
        #[cfg(test)]
        pub(super) fn with_name(name: &str) -> Self {
            // SAFETY: a live string; a null matrix is the documented identity argument; Create
            // rule.
            Self(unsafe {
                CTFont::with_name(
                    &CFString::from_str(name),
                    BASE_POINT_SIZE,
                    core::ptr::null(),
                )
            })
        }

        /// CoreText's own glyph ids for `text` (`CTFontGetGlyphsForCharacters`), 0 where it maps
        /// nothing; one per UTF-16 unit, so `text` should be BMP-only.
        #[cfg(test)]
        pub(super) fn glyph_ids(&self, text: &str) -> Vec<u32> {
            let mut units: Vec<u16> = text.encode_utf16().collect();
            let mut glyphs = vec![0_u16; units.len()];
            if units.is_empty() {
                return Vec::new();
            }
            // SAFETY: both buffers hold `units.len()` elements and outlive the call.
            unsafe {
                self.0.glyphs_for_characters(
                    core::ptr::NonNull::new(units.as_mut_ptr()).expect("non-empty"),
                    core::ptr::NonNull::new(glyphs.as_mut_ptr()).expect("non-empty"),
                    units.len() as isize,
                )
            };
            glyphs.into_iter().map(u32::from).collect()
        }

        pub(super) fn postscript_name(&self) -> String {
            // SAFETY: a getter on a live font; the copy follows the Create rule.
            unsafe { self.0.post_script_name() }.to_string()
        }

        pub(super) fn family_name(&self) -> String {
            // SAFETY: a getter on a live font; the copy follows the Create rule.
            unsafe { self.0.family_name() }.to_string()
        }

        /// The font file (`kCTFontURLAttribute`), when the font has one.
        pub(super) fn path(&self) -> Option<PathBuf> {
            // SAFETY: reading an immutable CoreText constant, then a getter on a live font.
            let value = unsafe { self.0.attribute(kCTFontURLAttribute) }?;
            value.downcast::<CFURL>().ok()?.to_file_path()
        }

        /// `CTFontCopyVariation` as (axis tag, user value), in tag order.
        pub(super) fn variation(&self) -> Vec<VariationSetting> {
            // SAFETY: a getter on a live font; a null result (no axes) stays `None`.
            let Some(dict) = (unsafe { self.0.variation() }) else {
                return Vec::new();
            };
            // SAFETY: CTFontCopyVariation documents its dictionary as axis identifier
            // (CFNumber) to axis value (CFNumber); the cast moves the same +1 reference.
            let dict: CFRetained<CFDictionary<CFNumber, CFNumber>> =
                unsafe { CFRetained::cast_unchecked(dict) };
            let (tags, values) = dict.to_vecs();
            let mut out: Vec<VariationSetting> = tags
                .iter()
                .zip(values.iter())
                .filter_map(|(tag, value)| {
                    let tag = u32::try_from(tag.as_i64()?).ok()?;
                    Some(VariationSetting {
                        tag: tag.to_be_bytes(),
                        value: value.as_f64()? as f32,
                    })
                })
                .collect();
            out.sort_by_key(|v| v.tag);
            out
        }
    }
}

use ct::Font;

/// The user's preferred languages, most preferred first (`CFLocaleCopyPreferredLanguages`): the
/// System Settings list on macOS, the device's (or the app's own) list on iOS. Read once per
/// process by `oracle/mod.rs`; no subprocess, no preferences file.
pub(super) fn preferred_languages() -> Vec<String> {
    let Some(languages) = CFLocale::preferred_languages() else {
        return Vec::new();
    };
    // SAFETY: CFLocaleCopyPreferredLanguages documents a CFArray of CFStrings, so every element
    // is a CoreFoundation object and viewing the array as one of `CFType` is sound; the cast
    // moves the same +1 reference, and each element is still type-checked by `downcast` below.
    let languages: CFRetained<CFArray<CFType>> = unsafe { CFRetained::cast_unchecked(languages) };
    languages
        .iter()
        .filter_map(|language| language.downcast::<CFString>().ok())
        .map(|language| language.to_string())
        .collect()
}

/// One face of a font file, as the tables-only walk read it.
#[derive(Clone, Debug, Default)]
struct FileFace {
    /// Every name ID 6 string of the face.
    postscript_names: Vec<String>,
    /// The `fvar` named instances that carry a PostScript name.
    instances: Vec<(String, Vec<VariationSetting>)>,
    /// Glyph data only in `hvgl`: no `glyf`, `CFF ` or `CFF2`.
    hvgl_only: bool,
}

/// A big-endian `u32` at `at`, if `bytes` holds it.
fn be_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn be_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

/// What one [`walk_file`] may still read.
#[derive(Clone, Copy, Debug)]
struct WalkBudget {
    bytes: u64,
    tables: usize,
    /// A read was refused for want of budget.
    exhausted: bool,
}

impl WalkBudget {
    const DEFAULT: Self = Self::new(MAX_WALK_BYTES, MAX_WALK_TABLES);

    const fn new(bytes: u64, tables: usize) -> Self {
        Self {
            bytes,
            tables,
            exhausted: false,
        }
    }

    /// Charges `bytes` (and a table when `table`); `false`, marking the budget exhausted, when
    /// it cannot pay.
    fn charge(&mut self, bytes: u64, table: bool) -> bool {
        let tables = if table {
            self.tables.checked_sub(1)
        } else {
            Some(self.tables)
        };
        match (self.bytes.checked_sub(bytes), tables) {
            (Some(left), Some(tables)) => {
                (self.bytes, self.tables) = (left, tables);
                true
            }
            _ => {
                self.exhausted = true;
                false
            }
        }
    }
}

/// `len` bytes at `offset`, charged to `budget`; `None` past the file's end or the budget.
fn read_at(file: &mut File, budget: &mut WalkBudget, offset: u64, len: usize) -> Option<Vec<u8>> {
    if !budget.charge(len as u64, false) {
        return None;
    }
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = vec![0; len];
    file.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// Every string `name` holds for `id`.
fn name_strings(name: &Name<'_>, id: NameId) -> Vec<String> {
    let data = name.string_data();
    let mut out: Vec<String> = Vec::new();
    for record in name.name_record().iter().filter(|r| r.name_id() == id) {
        if let Ok(string) = record.string(data) {
            let string: String = string.chars().collect();
            if !out.contains(&string) {
                out.push(string);
            }
        }
    }
    out
}

/// Reads the faces of the font file at `path` from its TTC header, table directories and
/// `name`/`fvar` tables only — never the glyph data — within [`WalkBudget::DEFAULT`].
fn walk_file(path: &Path) -> Option<Vec<FileFace>> {
    let mut budget = WalkBudget::DEFAULT;
    let faces = walk_file_within(path, &mut budget);
    if budget.exhausted {
        log::debug!(
            "frust-text: face walk of {} exceeded its budget",
            path.display()
        );
    }
    faces
}

/// [`walk_file`] under an explicit budget; `None` when the file is unreadable, malformed, or
/// needs more than `budget`.
fn walk_file_within(path: &Path, budget: &mut WalkBudget) -> Option<Vec<FileFace>> {
    let mut file = File::open(path).ok()?;
    let header = read_at(&mut file, budget, 0, 12)?;
    let offsets: Vec<u32> = if header.get(..4) == Some(b"ttcf".as_slice()) {
        let count = be_u32(&header, 8)?.min(u32::from(u16::MAX));
        let list = read_at(&mut file, budget, 12, count as usize * 4)?;
        (0..count as usize)
            .filter_map(|i| be_u32(&list, i * 4))
            .collect()
    } else {
        vec![0]
    };
    let mut tables: HashMap<(u32, u32), Vec<u8>> = HashMap::new();
    let mut faces = Vec::with_capacity(offsets.len());
    for offset in offsets {
        let directory = read_at(&mut file, budget, u64::from(offset), 12)?;
        let count = usize::from(be_u16(&directory, 4)?);
        let records = read_at(&mut file, budget, u64::from(offset) + 12, count * 16)?;
        let mut face = FileFace {
            hvgl_only: true,
            ..FileFace::default()
        };
        let (mut has_hvgl, mut name_at, mut fvar_at) = (false, None, None);
        for record in records.as_chunks::<16>().0 {
            let at = (be_u32(record, 8)?, be_u32(record, 12)?);
            match &record[..4] {
                b"glyf" | b"CFF " | b"CFF2" => face.hvgl_only = false,
                b"hvgl" => has_hvgl = true,
                b"name" => name_at = Some(at),
                b"fvar" => fvar_at = Some(at),
                _ => {}
            }
        }
        face.hvgl_only &= has_hvgl;
        let mut table = |at: Option<(u32, u32)>| -> Result<Option<Vec<u8>>, ()> {
            let Some(at) = at.filter(|(_, len)| *len <= MAX_TABLE_BYTES) else {
                return Ok(None);
            };
            let bytes = match tables.entry(at) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    // An exhausted budget ends the walk; an unreadable table is a face without names.
                    if !budget.charge(0, true) {
                        return Err(());
                    }
                    match read_at(&mut file, budget, u64::from(at.0), at.1 as usize) {
                        Some(bytes) => entry.insert(bytes),
                        None if budget.exhausted => return Err(()),
                        None => return Ok(None),
                    }
                }
            };
            Ok(Some(bytes.clone()))
        };
        let name_bytes = table(name_at).ok()?;
        let fvar_bytes = table(fvar_at).ok()?;
        let Some(name) = name_bytes
            .as_deref()
            .and_then(|b| Name::read(FontData::new(b)).ok())
        else {
            faces.push(face);
            continue;
        };
        face.postscript_names = name_strings(&name, NameId::POSTSCRIPT_NAME);
        if let Some(fvar) = fvar_bytes
            .as_deref()
            .and_then(|b| Fvar::read(FontData::new(b)).ok())
            && let (Ok(axes), Ok(instances)) = (fvar.axes(), fvar.instances())
        {
            for instance in instances.iter().flatten() {
                let Some(id) = instance.post_script_name_id else {
                    continue;
                };
                let coords = axes
                    .iter()
                    .zip(instance.coordinates)
                    .map(|(axis, value)| VariationSetting {
                        tag: axis.axis_tag().to_be_bytes(),
                        value: value.get().to_f64() as f32,
                    })
                    .collect::<Vec<_>>();
                for ps in name_strings(&name, id) {
                    face.instances.push((ps, coords.clone()));
                }
            }
        }
        faces.push(face);
    }
    Some(faces)
}

/// Where a PostScript name lives: the face whose name ID 6 is `ps`, else the face with an `fvar`
/// named instance called `ps` (with that instance's coordinates).
fn locate(faces: &[FileFace], ps: &str) -> Option<(u32, Vec<VariationSetting>)> {
    if let Some(index) = faces
        .iter()
        .position(|f| f.postscript_names.iter().any(|n| n == ps))
    {
        return Some((index as u32, Vec::new()));
    }
    faces.iter().enumerate().find_map(|(index, face)| {
        face.instances
            .iter()
            .find(|(name, _)| name == ps)
            .map(|(_, coords)| (index as u32, coords.clone()))
    })
}

/// `coords` without `opsz` (the request carries no size).
fn without_opsz(coords: Vec<VariationSetting>) -> Vec<VariationSetting> {
    coords.into_iter().filter(|v| &v.tag != b"opsz").collect()
}

/// A PostScript name mapped to its face (cached per name).
#[derive(Clone, Debug)]
struct Mapped {
    path: PathBuf,
    index: u32,
    /// The named instance's coordinates; empty for a name ID 6 match.
    instance: Vec<VariationSetting>,
    hidden: bool,
    hvgl_only: bool,
}

/// Which family a base font is built from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum BaseFamily {
    SystemUi,
    Named(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct BaseKey {
    family: BaseFamily,
    traits: (u32, u8, u32, u32),
    /// Only the system UI font is built per language.
    language: Option<String>,
}

/// The concrete families a non-UI generic slot is asked under, in preference order.
fn generic_families(slot: GenericSlot) -> &'static [&'static str] {
    match slot {
        GenericSlot::SansSerif | GenericSlot::SystemUi => &["Helvetica"],
        GenericSlot::Serif => &["Times", "Times New Roman"],
        GenericSlot::Monospace => &["Menlo", "Courier"],
        GenericSlot::Emoji => &["Apple Color Emoji"],
    }
}

/// The backend's caches: base fonts, walked files and mapped PostScript names.
#[derive(Default)]
struct State {
    bases: HashMap<BaseKey, Option<Font>>,
    files: HashMap<PathBuf, Option<Vec<FileFace>>>,
    names: HashMap<String, Option<Mapped>>,
    /// The (script, language) pairs whose hidden hvgl-only answer had no public
    /// re-ask: each is logged once at warn, repeats at debug.
    unreasked_scripts: HashSet<([u8; 4], Option<String>)>,
}

impl State {
    fn base(
        &mut self,
        family: BaseFamily,
        traits: Traits,
        language: Option<&str>,
    ) -> Option<&Font> {
        let language = (family == BaseFamily::SystemUi)
            .then(|| language.map(str::to_owned))
            .flatten();
        let key = BaseKey {
            family,
            traits: traits.key(),
            language,
        };
        self.bases
            .entry(key)
            .or_insert_with_key(|key| match &key.family {
                BaseFamily::SystemUi => Font::system_ui(key.language.as_deref(), traits),
                BaseFamily::Named(name) => Font::family(name, traits),
            })
            .as_ref()
    }

    /// The base for a generic slot: the UI font, or its first installed concrete family.
    fn generic_base(
        &mut self,
        slot: GenericSlot,
        traits: Traits,
        language: Option<&str>,
    ) -> Option<&Font> {
        if slot == GenericSlot::SystemUi {
            return self.base(BaseFamily::SystemUi, traits, language);
        }
        let family = generic_families(slot).iter().find(|f| {
            self.base(BaseFamily::Named((**f).to_owned()), traits, None)
                .is_some()
        })?;
        self.base(BaseFamily::Named((*family).to_owned()), traits, None)
    }

    /// `font`'s face, through the per-name cache.
    fn map(&mut self, font: &Font) -> Option<Mapped> {
        let ps = font.postscript_name();
        if ps == LAST_RESORT {
            return None;
        }
        if let Some(mapped) = self.names.get(&ps) {
            return mapped.clone();
        }
        let mapped = font.path().and_then(|path| {
            let faces = self
                .files
                .entry(path.clone())
                .or_insert_with(|| walk_file(&path));
            let (index, instance) = locate(faces.as_deref()?, &ps)?;
            let hvgl_only = faces.as_deref()?.get(index as usize)?.hvgl_only;
            Some(Mapped {
                path,
                index,
                instance,
                hidden: ps.starts_with('.'),
                hvgl_only,
            })
        });
        if mapped.is_none() {
            log::debug!("frust-text: CoreText face {ps} maps to no face of its file");
        }
        self.names.insert(ps, mapped.clone());
        mapped
    }

    /// The face answer for `font`: its mapped face at `CTFontCopyVariation`'s coordinates (else
    /// the named instance's), `opsz` dropped.
    fn answer(&mut self, font: &Font) -> Option<FaceSource> {
        let mapped = self.map(font)?;
        let live = font.variation();
        let coords = if live.is_empty() {
            mapped.instance
        } else {
            live
        };
        let mut source = FaceSource::path(mapped.path, mapped.index);
        source.variations = without_opsz(coords);
        source.postscript_name = Some(font.postscript_name());
        source.hidden = mapped.hidden;
        Some(source)
    }

    /// The base font the request's family chain names: its first named family CoreText has,
    /// else its first generic slot (`SystemUi` when it names none).
    fn request_base(&mut self, req: &FallbackRequest, traits: Traits) -> Option<&Font> {
        let language = language_for(req.script, req.locale.as_deref());
        let named = req.families.iter().find_map(|f| match f {
            FamilyName::Named(name) => self
                .base(BaseFamily::Named(name.clone()), traits, None)
                .is_some()
                .then(|| name.clone()),
            FamilyName::Generic(_) => None,
        });
        match named {
            Some(name) => self.base(BaseFamily::Named(name), traits, None),
            None => {
                let slot = req
                    .generics()
                    .first()
                    .copied()
                    .unwrap_or(GenericSlot::SystemUi);
                self.generic_base(slot, traits, language)
            }
        }
    }

    /// The whole per-miss resolution, see the module docs.
    fn resolve(&mut self, req: &FallbackRequest) -> Option<Resolution> {
        self.resolve_via(req, PUBLIC_BASE_FAMILY)
    }

    /// [`State::resolve`], re-asking a hidden hvgl-only answer under `public_family`.
    ///
    /// A hidden hvgl-only face cannot be drawn by name, so when the re-ask finds no public
    /// face (or only another hidden one) the answer is a miss, not the hidden face: the
    /// selector moves on to its next candidate.
    fn resolve_via(&mut self, req: &FallbackRequest, public_family: &str) -> Option<Resolution> {
        let traits = Traits::of(req);
        let language = language_for(req.script, req.locale.as_deref());
        let query = query_text(req);
        let first = self
            .request_base(req, traits)?
            .for_string(&query, language)?;
        let mapped = self.map(&first)?;
        if mapped.hidden && mapped.hvgl_only {
            let public = self
                .base(BaseFamily::Named(public_family.to_owned()), traits, None)
                .and_then(|base| base.for_string(&query, language));
            if let Some(public) = public
                && !public.postscript_name().starts_with('.')
                && let Some(source) = self.answer(&public)
            {
                return Some(Resolution {
                    source,
                    reasked_from: Some(first.postscript_name()),
                });
            }
            let first_for_script = self
                .unreasked_scripts
                .insert((req.script.0, language.map(str::to_owned)));
            if first_for_script {
                log::warn!(
                    "frust-text: hidden hvgl-only face {:?} has no public re-ask; a miss, not a \
                     substitute (logged once per script and language)",
                    first.postscript_name(),
                );
            } else {
                log::debug!(
                    "frust-text: hidden hvgl-only face {:?} has no public re-ask; a miss",
                    first.postscript_name(),
                );
            }
            return None;
        }
        Some(Resolution {
            source: self.answer(&first)?,
            reasked_from: None,
        })
    }

    fn default_family(&mut self, generic: GenericSlot) -> Option<FaceSource> {
        if generic == GenericSlot::SystemUi {
            let ui = self
                .base(BaseFamily::SystemUi, Traits::REGULAR, None)?
                .clone();
            return self.answer(&ui);
        }
        let sample = if generic == GenericSlot::Emoji {
            "\u{1F600}"
        } else {
            "a"
        };
        let found = self
            .generic_base(generic, Traits::REGULAR, None)?
            .for_string(sample, None)?;
        self.answer(&found)
    }
}

/// The text CoreText is asked about. Danda and double danda (U+0964, U+0965) are shared by the
/// Indic scripts and CoreText ignores the language for them (it answers `.SFBangla`), so a
/// danda-only Devanagari cluster is asked with a Devanagari letter before it, which the
/// cascade resolves to the Devanagari face that also covers the danda.
fn query_text(req: &FallbackRequest) -> std::borrow::Cow<'_, str> {
    let danda_only = req
        .chars
        .chars()
        .all(|c| matches!(c, '\u{0964}' | '\u{0965}'));
    if req.script.0 == *b"Deva" && danda_only && !req.chars.is_empty() {
        std::borrow::Cow::Owned(format!("\u{0905}{}", req.chars))
    } else {
        std::borrow::Cow::Borrowed(&req.chars)
    }
}

/// A resolved request, with the hidden name a public re-ask replaced.
struct Resolution {
    source: FaceSource,
    reasked_from: Option<String>,
}

/// `ProductBuildVersion` from a `SystemVersion.plist` (XML) by text search.
fn product_build_version(plist: &str) -> Option<&str> {
    let (_, after) = plist.split_once("<key>ProductBuildVersion</key>")?;
    let value = after.trim_start().strip_prefix("<string>")?;
    let (value, _) = value.split_once("</string>")?;
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

/// Answers fallback questions through CoreText.
#[derive(Default)]
pub(crate) struct CoreTextBackend {
    state: RefCell<State>,
}

impl std::fmt::Debug for CoreTextBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state.borrow();
        f.debug_struct("CoreTextBackend")
            .field("bases", &state.bases.len())
            .field("files", &state.files.len())
            .field("names", &state.names.len())
            .finish()
    }
}

impl CoreTextBackend {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn resolve_traced(&mut self, req: &FallbackRequest) -> Option<Resolution> {
        let started = Instant::now();
        let resolution = self.state.get_mut().resolve(req);
        if let Some(r) = &resolution {
            // Never the characters themselves: log output can leave the device.
            log::debug!(
                "coretext font miss ({} code points, {}): {} us, {:?} #{} {:?}{}",
                req.chars.chars().count(),
                String::from_utf8_lossy(&req.script.0),
                started.elapsed().as_micros(),
                r.source.postscript_name,
                r.source.index,
                r.source.location,
                r.reasked_from
                    .as_ref()
                    .map(|h| format!(" (re-asked from {h})"))
                    .unwrap_or_default(),
            );
        }
        resolution
    }
}

impl Backend for CoreTextBackend {
    fn resolve(&mut self, req: &FallbackRequest) -> Option<FaceSource> {
        self.resolve_traced(req).map(|r| r.source)
    }

    fn default_family(&self, generic: GenericSlot) -> Option<FaceSource> {
        self.state.borrow_mut().default_family(generic)
    }

    fn os_build_id(&self) -> String {
        std::fs::read_to_string(SYSTEM_VERSION_PLIST)
            .ok()
            .as_deref()
            .and_then(product_build_version)
            .map(|build| format!("apple:{build}"))
            .unwrap_or_else(|| String::from("apple-unknown"))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use read_fonts::{FontRef, TableProvider};

    use super::*;
    use crate::oracle::{FaceLocation, Oracle, RenderCapabilities, ScriptTag};
    use crate::style::{FontFamily, FontWeight};

    /// Skips announced so far; a test that cannot run bumps it instead of passing silently.
    static SKIPS: AtomicUsize = AtomicUsize::new(0);

    /// How many tests skipped for a missing host precondition.
    fn skips() -> usize {
        SKIPS.load(Ordering::SeqCst)
    }

    /// The first missing precondition of the macOS-installed-face tests, else `None`: this is
    /// macOS, CoreText answers a Han cluster from a `PingFangUI.ttc` collection, and that
    /// collection holds the face named `PingFangSC-Regular`.
    fn missing_precondition() -> Option<String> {
        static CHECK: OnceLock<Option<String>> = OnceLock::new();
        CHECK
            .get_or_init(|| {
                if !cfg!(target_os = "macos") {
                    return Some("not macOS; the expected faces are macOS system fonts".into());
                }
                let mut backend = CoreTextBackend::new();
                let Some(han) = backend.resolve(&request("\u{4e2d}\u{6587}", Some("zh-Hans")))
                else {
                    return Some("CoreText resolves no Han face for zh-Hans".into());
                };
                let FaceLocation::Path(path) = &han.location else {
                    return Some("CoreText answered a blob, not a path".into());
                };
                if path.file_name().is_none_or(|n| n != "PingFangUI.ttc") || !path.exists() {
                    return Some(format!(
                        "the PingFang collection is absent (CoreText answered {})",
                        path.display()
                    ));
                }
                let named = walk_file(path)
                    .is_some_and(|faces| locate(&faces, "PingFangSC-Regular").is_some());
                (!named).then(|| "PingFangUI.ttc has no PingFangSC-Regular face".into())
            })
            .clone()
    }

    /// True when the host has what `test` needs; otherwise prints one counted
    /// `SKIPPED (n)` line naming the missing precondition and returns false.
    fn host_ready(test: &str) -> bool {
        let Some(missing) = missing_precondition() else {
            return true;
        };
        let n = SKIPS.fetch_add(1, Ordering::SeqCst) + 1;
        println!("SKIPPED ({n}): {test}: {missing}");
        false
    }

    fn request(chars: &str, locale: Option<&str>) -> FallbackRequest {
        let mut req = FallbackRequest::new(
            chars,
            ScriptTag::COMMON,
            &FontFamily::SystemUi,
            RenderCapabilities::default(),
        );
        req.locale = locale.map(str::to_owned);
        req
    }

    fn path_of(source: &FaceSource) -> &Path {
        match &source.location {
            FaceLocation::Path(path) => path,
            FaceLocation::Blob(_) => panic!("CoreText answers are paths"),
        }
    }

    fn file_name(source: &FaceSource) -> String {
        path_of(source)
            .file_name()
            .expect("a file name")
            .to_string_lossy()
            .into_owned()
    }

    fn ps(source: &FaceSource) -> &str {
        source
            .postscript_name
            .as_deref()
            .expect("a PostScript name")
    }

    fn has_opsz(source: &FaceSource) -> bool {
        source.variations.iter().any(|v| &v.tag == b"opsz")
    }

    /// Resolves `chars` under `SystemUi`, printing the answer and the miss's time.
    fn resolve(backend: &mut CoreTextBackend, chars: &str, locale: Option<&str>) -> Resolution {
        let started = Instant::now();
        let resolution = backend
            .resolve_traced(&request(chars, locale))
            .unwrap_or_else(|| panic!("{chars:?} {locale:?} resolves"));
        let source = &resolution.source;
        println!(
            "{chars:?} {locale:?} -> {} #{} {} hidden={} coords={:?} re-asked from {:?} ({} us)",
            file_name(source),
            source.index,
            ps(source),
            source.hidden,
            source
                .variations
                .iter()
                .map(|v| (String::from_utf8_lossy(&v.tag).into_owned(), v.value))
                .collect::<Vec<_>>(),
            resolution.reasked_from,
            started.elapsed().as_micros(),
        );
        resolution
    }

    /// `text`'s `cmap` glyph ids in `source`'s face.
    fn glyph_ids(source: &FaceSource, text: &str) -> Vec<u32> {
        let bytes = std::fs::read(path_of(source)).expect("the face's file is readable");
        let font = FontRef::from_index(&bytes, source.index).expect("the face parses");
        let cmap = font.cmap().expect("a cmap");
        text.chars()
            .map(|c| cmap.map_codepoint(c).map_or(0, |g| g.to_u32()))
            .collect()
    }

    #[test]
    fn han_under_system_ui_is_re_asked_to_the_public_pingfang_instance() {
        if !host_ready("han_under_system_ui_is_re_asked_to_the_public_pingfang_instance") {
            return;
        }
        let mut backend = CoreTextBackend::new();
        let han = resolve(&mut backend, "\u{4e2d}\u{6587}", Some("zh-Hans"));
        let reasked = han
            .reasked_from
            .as_deref()
            .expect("the hvgl-only answer was re-asked");
        assert!(reasked.starts_with(".PingFangUIText"), "{reasked}");
        let source = han.source;
        assert_eq!(file_name(&source), "PingFangUI.ttc");
        let faces = walk_file(path_of(&source)).expect("PingFangUI.ttc walks");
        let regular = locate(&faces, "PingFangSC-Regular")
            .map(|(i, _)| i)
            .expect("the collection names PingFangSC-Regular");
        assert_eq!(
            source.index, regular,
            "the face CoreText's answer maps to, found by name in the file"
        );
        assert_eq!(ps(&source), "PingFangSC-Regular");
        assert!(!source.hidden);
        assert!(!has_opsz(&source), "{:?}", source.variations);
        let wght = source.variations.iter().find(|v| &v.tag == b"wght");
        assert_eq!(wght.map(|v| v.value), Some(400.0));
    }

    #[test]
    fn a_hidden_hvgl_only_answer_whose_public_re_ask_fails_is_a_miss() {
        if !host_ready("a_hidden_hvgl_only_answer_whose_public_re_ask_fails_is_a_miss") {
            return;
        }
        let mut backend = CoreTextBackend::new();
        let req = request("\u{4e2d}\u{6587}", Some("zh-Hans"));
        let state = backend.state.get_mut();
        assert!(
            state.resolve_via(&req, "FrustNoSuchFamily").is_none(),
            "the hidden face is not returned when the public re-ask fails"
        );
        let reasked = state
            .resolve_via(&req, PUBLIC_BASE_FAMILY)
            .expect("the normal re-ask still resolves");
        assert!(!reasked.source.hidden);
    }

    #[test]
    fn scripts_map_to_their_system_faces() {
        if !host_ready("scripts_map_to_their_system_faces") {
            return;
        }
        let mut backend = CoreTextBackend::new();

        let latin = resolve(&mut backend, "Hello", None).source;
        assert_eq!(file_name(&latin), "SFNS.ttf");
        assert_eq!(
            (latin.index, ps(&latin), latin.hidden),
            (0, ".SFNS-Regular", true)
        );
        assert!(!has_opsz(&latin));

        let ja = resolve(&mut backend, "\u{3072}\u{3089}\u{304c}\u{306a}", Some("ja")).source;
        assert!(ps(&ja).starts_with(".HiraKakuInterface-"), "{}", ps(&ja));
        assert!(
            file_name(&ja).ends_with(" W4.ttc"),
            "Hiragino Sans W4: {}",
            file_name(&ja)
        );
        assert!(ja.hidden);

        let thai = resolve(&mut backend, "\u{0e44}\u{0e17}\u{0e22}", Some("th-TH")).source;
        assert_eq!(file_name(&thai), "ThonburiUI.ttc");
        assert_eq!((ps(&thai), thai.hidden), (".ThonburiUI-Regular", true));

        let arabic = resolve(
            &mut backend,
            "\u{0627}\u{0644}\u{0639}\u{0631}\u{0628}\u{064a}\u{0629}",
            Some("ar"),
        )
        .source;
        assert_eq!(file_name(&arabic), "SFArabic.ttf");
        assert_eq!((ps(&arabic), arabic.hidden), (".SFArabic-Regular", true));

        let emoji = resolve(&mut backend, "\u{1F600}", None).source;
        assert_eq!(file_name(&emoji), "Apple Color Emoji.ttc");
        assert_eq!(
            (emoji.index, ps(&emoji), emoji.hidden),
            (1, ".AppleColorEmojiUI", true)
        );
    }

    #[test]
    fn a_dot_prefixed_face_always_carries_the_hidden_flag() {
        if !host_ready("a_dot_prefixed_face_always_carries_the_hidden_flag") {
            return;
        }
        let mut backend = CoreTextBackend::new();
        let samples = [
            "Hello",
            "\u{4e2d}\u{6587}",
            "\u{7e41}\u{9ad4}",
            "\u{3072}\u{3089}",
            "\u{0e44}\u{0e17}",
            "\u{0627}\u{0644}",
            "\u{1F600}",
            "\u{d55c}\u{ad6d}",
            "\u{25CC}\u{0301}",
        ];
        let locales = [
            None,
            Some("en"),
            Some("zh-Hans"),
            Some("zh-Hant"),
            Some("ja"),
            Some("ar"),
        ];
        let (mut hidden, mut answers) = (0, 0);
        for text in samples {
            for locale in locales {
                for (weight, slant) in [(400.0, FontStyle::Normal), (700.0, FontStyle::Italic)] {
                    let mut req = request(text, locale);
                    req.weight = FontWeight::new(weight);
                    req.slant = slant;
                    let Some(source) = backend.resolve(&req) else {
                        continue;
                    };
                    answers += 1;
                    let name = ps(&source);
                    assert_eq!(
                        source.hidden,
                        name.starts_with('.'),
                        "{text:?} {locale:?}: {name}"
                    );
                    hidden += usize::from(source.hidden);
                    assert!(!has_opsz(&source), "{name}: {:?}", source.variations);
                }
            }
        }
        println!("{answers} answers, {hidden} hidden");
        assert!(hidden > 0 && hidden < answers);
    }

    #[test]
    fn cmap_glyph_ids_match_coretexts_recorded_ids() {
        if !host_ready("cmap_glyph_ids_match_coretexts_recorded_ids") {
            return;
        }
        // The oracle's `cmap` walk must agree with CoreText's own mapping for the same face.
        // CoreText is asked, not a recorded list: glyph ids differ between macOS releases.
        let mut backend = CoreTextBackend::new();
        let sf = resolve(&mut backend, "Hello", None).source;
        // The SF answer is hidden, and CoreText resolves no hidden name, so its face is the
        // system UI font's answer for the same text, checked to be the same face.
        let sf_ct = ct::Font::system_ui(None, Traits::REGULAR)
            .and_then(|ui| ui.for_string("Hello", None))
            .expect("CoreText answers Latin");
        assert_eq!(sf_ct.postscript_name(), ps(&sf));
        let hello = glyph_ids(&sf, "Hello");
        assert!(hello.iter().all(|&id| id != 0), "{hello:?}");
        assert_eq!(hello, sf_ct.glyph_ids("Hello"));
        assert_eq!(glyph_ids(&sf, " "), sf_ct.glyph_ids(" "));
        let han_text = "\u{4e2d}\u{6587}";
        let pingfang = resolve(&mut backend, han_text, Some("zh-Hans")).source;
        let pingfang_ct = ct::Font::with_name(ps(&pingfang));
        assert_eq!(pingfang_ct.postscript_name(), ps(&pingfang));
        let han = glyph_ids(&pingfang, han_text);
        assert!(han.iter().all(|&id| id != 0), "{han:?}");
        assert_eq!(han, pingfang_ct.glyph_ids(han_text));
    }

    #[test]
    fn the_face_walk_reads_names_and_instances_only() {
        if !host_ready("the_face_walk_reads_names_and_instances_only") {
            return;
        }
        let mut backend = CoreTextBackend::new();
        let pingfang = resolve(&mut backend, "\u{4e2d}\u{6587}", Some("zh-Hans")).source;
        let started = Instant::now();
        let faces = walk_file(path_of(&pingfang)).expect("PingFangUI.ttc walks");
        println!(
            "walked {} faces in {} us",
            faces.len(),
            started.elapsed().as_micros()
        );
        assert!(
            faces.iter().all(|f| f.hvgl_only),
            "every PingFangUI face is hvgl-only"
        );
        let (index, _) = locate(&faces, "PingFangSC-Regular").expect("PingFangSC-Regular");
        let face = &faces[index as usize];
        assert!(
            face.postscript_names
                .iter()
                .any(|n| n == "PingFangSC-Medium"),
            "the regular instance lives in the Medium face"
        );
        assert_eq!(index, pingfang.index, "CoreText's answer is the same face");
        let sfns = walk_file(Path::new("/System/Library/Fonts/SFNS.ttf")).expect("SFNS walks");
        assert_eq!(sfns.len(), 1);
        assert!(!sfns[0].hvgl_only, "SF has its own outlines");
    }

    /// The median and largest of `n` misses, each on a cluster no earlier miss asked.
    fn time_misses(
        backend: &mut CoreTextBackend,
        n: u32,
        cluster: impl Fn(u32) -> FallbackRequest,
    ) -> (u128, u128) {
        let mut times: Vec<Duration> = (0..n)
            .map(|i| {
                let req = cluster(i);
                let started = Instant::now();
                assert!(backend.resolve(&req).is_some(), "{:?}", req.chars);
                started.elapsed()
            })
            .collect();
        times.sort();
        let median = times[times.len() / 2].as_micros();
        (median, times[times.len() - 1].as_micros())
    }

    #[test]
    fn per_miss_time_is_printed() {
        if !host_ready("per_miss_time_is_printed") {
            return;
        }
        let mut backend = CoreTextBackend::new();
        let started = Instant::now();
        let _ = backend.resolve(&request("\u{4e2d}\u{6587}", Some("zh-Hans")));
        let cold = started.elapsed().as_micros();
        println!("first miss (cold base font, cold PingFangUI.ttc walk): {cold} us");
        let han = |i| {
            let c = char::from_u32(0x4e10 + i).expect("a CJK ideograph");
            request(&c.to_string(), Some("zh-Hans"))
        };
        let (median, max) = time_misses(&mut backend, 200, han);
        println!(
            "warm Han miss (two CreateForString, the hvgl re-ask): median {median} us, max {max} us"
        );
        let latin = |i| {
            let c = char::from_u32(0x41 + i % 58).expect("ASCII");
            let d = char::from_u32(0xc0 + i / 58).expect("Latin-1");
            request(&format!("{c}{d}"), None)
        };
        let (median, max) = time_misses(&mut backend, 200, latin);
        println!("warm Latin miss (one CreateForString): median {median} us, max {max} us");
    }

    #[test]
    fn named_families_and_generic_defaults_resolve() {
        if !host_ready("named_families_and_generic_defaults_resolve") {
            return;
        }
        let mut backend = CoreTextBackend::new();
        let mut req = request("abc", None);
        req.families = vec![FamilyName::Named("Menlo".into())];
        assert_eq!(
            backend.resolve(&req).map(|s| ps(&s).to_owned()).as_deref(),
            Some("Menlo-Regular")
        );
        req.families = vec![
            FamilyName::Named("No Such Family 7f3a".into()),
            FamilyName::Generic(GenericSlot::Serif),
        ];
        assert_eq!(
            backend.resolve(&req).map(|s| ps(&s).to_owned()).as_deref(),
            Some("Times-Roman")
        );

        for (slot, file, name, hidden) in [
            (GenericSlot::SystemUi, "SFNS.ttf", ".SFNS-Regular", true),
            (GenericSlot::SansSerif, "Helvetica.ttc", "Helvetica", false),
            (GenericSlot::Serif, "Times.ttc", "Times-Roman", false),
            (GenericSlot::Monospace, "Menlo.ttc", "Menlo-Regular", false),
            (
                GenericSlot::Emoji,
                "Apple Color Emoji.ttc",
                "AppleColorEmoji",
                false,
            ),
        ] {
            let source = backend
                .default_family(slot)
                .unwrap_or_else(|| panic!("{slot:?}"));
            assert_eq!(
                (file_name(&source).as_str(), ps(&source), source.hidden),
                (file, name, hidden)
            );
        }
    }

    #[test]
    fn a_cluster_nothing_covers_is_a_miss() {
        if !host_ready("a_cluster_nothing_covers_is_a_miss") {
            return;
        }
        // A noncharacter: CoreText answers LastResort.
        assert!(
            CoreTextBackend::new()
                .resolve(&request("\u{fdd0}", None))
                .is_none()
        );
    }

    #[test]
    fn the_oracle_accepts_the_hvgl_pingfang_answer() {
        if !host_ready("the_oracle_accepts_the_hvgl_pingfang_answer") {
            return;
        }
        let caps = RenderCapabilities::default();
        let mut oracle = Oracle::new(Box::new(CoreTextBackend::new()), caps, None);
        let mut req = request("\u{4e2d}\u{6587}", Some("zh-Hans"));
        req.script = ScriptTag(*b"Hani");
        let face = oracle
            .resolve(&req)
            .expect("PingFang is drawable through CoreText");
        let expected = CoreTextBackend::new()
            .resolve(&req)
            .expect("the backend answers the same question");
        assert_eq!(
            (face.index, face.postscript_name.as_deref(), face.hidden),
            (expected.index, Some("PingFangSC-Regular"), false)
        );
        assert_eq!(oracle.stats().refused, 0);
    }

    #[test]
    fn the_face_walk_stops_at_its_byte_and_table_budgets() {
        use crate::oracle::synthetic::{FaceSpec, TempDir};
        let dir = TempDir::new("apple-walk-budget");
        let specs: Vec<FaceSpec> = ["A", "B", "C"]
            .into_iter()
            .map(|family| FaceSpec::new(family).chars("a"))
            .collect();
        let ttc = dir.write("three.ttc", &FaceSpec::collection(&specs));
        let len = std::fs::metadata(&ttc).unwrap().len();

        let mut ample = WalkBudget::new(len, 3);
        let faces = walk_file_within(&ttc, &mut ample).expect("within budget");
        assert_eq!(faces.len(), 3);
        assert!(!ample.exhausted);

        let mut two_tables = WalkBudget::new(len, 2);
        assert!(walk_file_within(&ttc, &mut two_tables).is_none());
        assert!(
            two_tables.exhausted,
            "the third distinct name table is refused"
        );

        let mut few_bytes = WalkBudget::new(64, MAX_WALK_TABLES);
        assert!(walk_file_within(&ttc, &mut few_bytes).is_none());
        assert!(few_bytes.exhausted, "the directories alone exceed 64 bytes");

        // A header claiming 65535 faces is cut off by the byte budget, not walked.
        let mut forged = std::fs::read(&ttc).unwrap();
        forged[8..12].copy_from_slice(&u32::from(u16::MAX).to_be_bytes());
        let forged = dir.write("forged.ttc", &forged);
        let mut budget = WalkBudget::new(1024, MAX_WALK_TABLES);
        assert!(walk_file_within(&forged, &mut budget).is_none());
        assert!(budget.exhausted);
        assert!(
            walk_file(&forged).is_none(),
            "a malformed collection maps nothing"
        );
    }

    #[test]
    fn the_default_walk_budget_fits_the_largest_system_collection() {
        if !host_ready("the_default_walk_budget_fits_the_largest_system_collection") {
            return;
        }
        let mut backend = CoreTextBackend::new();
        let pingfang = resolve(&mut backend, "\u{4e2d}\u{6587}", Some("zh-Hans")).source;
        let mut budget = WalkBudget::DEFAULT;
        let faces = walk_file_within(path_of(&pingfang), &mut budget).expect("PingFangUI.ttc");
        let used = MAX_WALK_BYTES - budget.bytes;
        println!(
            "PingFangUI.ttc: {} faces, {used} bytes, {} tables",
            faces.len(),
            MAX_WALK_TABLES - budget.tables
        );
        assert!(!budget.exhausted);
        assert!(used < MAX_WALK_BYTES / 4, "{used} bytes leaves headroom");
    }

    #[test]
    fn the_os_build_comes_from_system_version_plist() {
        let plist = "<dict>\n\t<key>ProductBuildVersion</key>\n\t<string>26A434</string>\n</dict>";
        assert_eq!(product_build_version(plist), Some("26A434"));
        assert_eq!(product_build_version("<dict></dict>"), None);
        assert_eq!(
            product_build_version("<key>ProductBuildVersion</key><string> </string>"),
            None
        );
        let build = CoreTextBackend::new().os_build_id();
        println!("os build: {build}");
        if cfg!(target_os = "macos") {
            assert!(
                build.starts_with("apple:") && build.len() > "apple:".len(),
                "{build}"
            );
        } else {
            assert!(!build.is_empty());
        }
    }

    /// A request with a cluster script, no locale.
    fn scripted(chars: &str, script: &[u8; 4]) -> FallbackRequest {
        let mut req = request(chars, None);
        req.script = ScriptTag(*script);
        req
    }

    #[test]
    fn kana_without_a_locale_resolve_to_hiragino() {
        if !host_ready("kana_without_a_locale_resolve_to_hiragino") {
            return;
        }
        let mut backend = CoreTextBackend::new();
        let kana = backend
            .resolve(&scripted(
                "\u{306e}\u{30c6}\u{30ad}\u{30b9}\u{30c8}",
                b"Hira",
            ))
            .expect("kana resolve");
        let name = ps(&kana);
        println!("kana -> {name}");
        // Hiragino is hidden (`.HiraKakuInterface-W4`) or public (`HiraginoSans-W4`); a Mac
        // without either must still not land on the symbols fallback.
        let hiragino = [
            "Hiragino",
            ".Hiragino",
            "HiraKaku",
            ".HiraKaku",
            ".HiraMaru",
            "HiraMin",
        ]
        .iter()
        .any(|p| name.starts_with(p));
        assert!(
            hiragino || !name.starts_with(".CJKSymbolsFallback"),
            "{name}"
        );
        assert!(hiragino, "kana resolved to {name}, not a Hiragino face");
    }

    #[test]
    fn han_without_a_locale_resolves_to_a_cjk_face() {
        if !host_ready("han_without_a_locale_resolves_to_a_cjk_face") {
            return;
        }
        let mut backend = CoreTextBackend::new();
        let han = backend
            .resolve(&scripted("\u{65e5}\u{672c}\u{8a9e}", b"Hani"))
            .expect("han resolves");
        println!("han -> {}", ps(&han));
        let name = ps(&han);
        assert!(
            [
                "PingFang",
                "Hiragino",
                ".PingFang",
                ".Hiragino",
                "STHeiti",
                "AppleSDGothic"
            ]
            .iter()
            .any(|p| name.starts_with(p)),
            "{name}"
        );
    }

    #[test]
    fn every_preferred_han_language_picks_its_own_face() {
        if !host_ready("every_preferred_han_language_picks_its_own_face") {
            return;
        }
        let mut backend = CoreTextBackend::new();
        // 直 and 骨 differ in glyph shape between the Han regions.
        for (language, prefixes) in [
            ("zh-Hans", &["PingFangSC"][..]),
            ("zh-Hant", &["PingFangTC"][..]),
            ("zh-HK", &["PingFangHK"][..]),
            (
                "ja",
                &["Hiragino", ".Hiragino", "HiraKaku", ".HiraKaku"][..],
            ),
            ("ko", &["AppleSDGothic", ".AppleSDGothic"][..]),
        ] {
            let mut req = scripted("\u{76f4}\u{9aa8}", b"Hani");
            req.locale = Some(language.to_owned());
            let face = backend.resolve(&req).expect("han resolves");
            println!("han under {language} -> {}", ps(&face));
            assert!(
                prefixes.iter().any(|p| ps(&face).starts_with(p)),
                "{language}: {}",
                ps(&face)
            );
        }
    }

    #[test]
    fn the_preferred_language_list_is_read_without_a_subprocess() {
        let started = Instant::now();
        let languages = preferred_languages();
        println!(
            "CFLocaleCopyPreferredLanguages -> {languages:?} ({} us)",
            started.elapsed().as_micros()
        );
        assert!(!languages.is_empty());
        assert!(languages.iter().all(|tag| !tag.is_empty()));
    }

    #[test]
    fn a_danda_in_devanagari_matches_its_neighbours() {
        if !host_ready("a_danda_in_devanagari_matches_its_neighbours") {
            return;
        }
        let mut backend = CoreTextBackend::new();
        let danda = backend
            .resolve(&scripted("\u{0964}", b"Deva"))
            .expect("danda resolves");
        let letters = backend
            .resolve(&scripted(
                "\u{0928}\u{092e}\u{0938}\u{094d}\u{0924}\u{0947}",
                b"Deva",
            ))
            .expect("devanagari resolves");
        println!("danda -> {}, letters -> {}", ps(&danda), ps(&letters));
        assert!(!ps(&danda).contains("Bangla"), "{}", ps(&danda));
        assert_eq!(ps(&danda), ps(&letters));
    }

    /// Reports the skip counter so a fully skipped run is visible in the output. Tests run in
    /// parallel, so the count is what had been announced by now; when the host is ready no test
    /// can have skipped, which is asserted.
    #[test]
    fn zz_skip_summary() {
        let missing = missing_precondition();
        match &missing {
            None => {
                println!("SKIP SUMMARY: 0 skipped, the host has every precondition");
                assert_eq!(skips(), 0, "a ready host never skips");
            }
            Some(why) => println!(
                "SKIP SUMMARY: {} skipped so far; every host-dependent test skips here: {why}",
                skips()
            ),
        }
    }
}
