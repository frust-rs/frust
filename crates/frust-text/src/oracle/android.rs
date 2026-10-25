//! Sanctioned unsafe zone (decision D-6): FFI to the NDK font matcher (`AFontMatcher`, API 29).
//! Every `unsafe` block carries a SAFETY note, and the FFI stays behind safe
//! wrappers that never leak past the `Backend` trait. No other oracle module
//! may contain `unsafe`; the guard tests in `oracle/mod.rs` enforce it.
//!
//! The Android NDK oracle backend (`AFontMatcher`, API 29).
//!
//! Compiled only for `target_os = "android"` (the cfg gate is on this module's `mod` line in
//! `oracle/mod.rs`). With minSdk 29 the NDK font API is the oracle: no `fonts.xml` parsing and no
//! startup enumeration (s-03: enumeration costs 233 ms, a match 2-7 microseconds).
//!
//! One `AFontMatcher` is kept per (weight, italic, family variant, locales) key and reused. A
//! cluster's UTF-16 goes to the matcher under the chain's first generic family; the matched
//! file path and collection index become the [`FaceSource`]. The matcher knows only the generic
//! families, so named (app) families are never asked of it. The matched weight/italic become a
//! synthesis hint: a face with a `wght` axis (variable Roboto) takes the requested weight, a
//! face lighter than requested without one (CJK bold, mono bold) is faux bold, and an upright
//! face asked for italic takes `ital` or a faux oblique.
//!
//! The only `unsafe` here is the `__system_property_get` call behind [`system_property`], and
//! the `Send` assertion on the matcher wrapper.

#![allow(unsafe_code)]

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_int};
use std::time::Instant;

use ndk::font::{FamilyVariant, FontMatcher, FontWeight};

use super::{Backend, FaceSource, FallbackRequest, Synthesis, VariationSetting};
use crate::style::{FontStyle, GenericSlot};

/// Android's `PROP_VALUE_MAX`: the longest property value, including the NUL.
const PROP_VALUE_MAX: usize = 92;

/// Faux-oblique angle (degrees) for an upright face asked for italic.
const FAUX_OBLIQUE_DEGREES: f32 = 14.0;

unsafe extern "C" {
    /// bionic's `__system_property_get(name, value)`: copies the NUL-terminated value (at most
    /// `PROP_VALUE_MAX` bytes) into `value`, returns its length (0 when unset).
    fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
}

/// Reads a system property, `None` when unset or empty. The safe wrapper around the FFI call.
fn system_property(name: &CStr) -> Option<String> {
    let mut buf = [0 as c_char; PROP_VALUE_MAX];
    // SAFETY: `name` is a valid NUL-terminated string for the call's duration, and `buf` is
    // writable for `PROP_VALUE_MAX` bytes, the most `__system_property_get` ever writes
    // (including the NUL).
    let len = unsafe { __system_property_get(name.as_ptr(), buf.as_mut_ptr()) };
    if len <= 0 {
        return None;
    }
    // SAFETY: on a positive return the callee wrote a NUL-terminated string into `buf`.
    let value = unsafe { CStr::from_ptr(buf.as_ptr()) };
    Some(value.to_string_lossy().into_owned())
}

/// An `AFontMatcher` the backend owns exclusively.
struct Matcher(FontMatcher);

// SAFETY: the matcher is a heap object with no thread affinity; it is only ever used through
// `&mut` from the one owner (the backend is not `Sync`-shared), so moving it between threads
// never produces concurrent access.
unsafe impl Send for Matcher {}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct MatcherKey {
    weight: u16,
    italic: bool,
    /// `FamilyVariant` as its NDK discriminant; only `Default` is used (s-03: `compact` and
    /// `elegant` returned identical files), but the key keeps the dimension.
    variant: u32,
    locale: Option<String>,
}

/// Answers fallback questions through the NDK font matcher.
pub(crate) struct NdkFontBackend {
    matchers: HashMap<MatcherKey, Matcher>,
}

impl std::fmt::Debug for NdkFontBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NdkFontBackend")
            .field("matchers", &self.matchers.len())
            .finish()
    }
}

impl Default for NdkFontBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// The matched face, before it becomes a [`FaceSource`].
struct Matched {
    source: FaceSource,
    file: String,
    run_utf16: u32,
}

impl NdkFontBackend {
    pub(crate) fn new() -> Self {
        Self {
            matchers: HashMap::new(),
        }
    }

    fn matcher(&mut self, key: &MatcherKey) -> &mut FontMatcher {
        &mut self
            .matchers
            .entry(key.clone())
            .or_insert_with(|| {
                let mut m = FontMatcher::new();
                let weight = FontWeight::new(
                    key.weight
                        .clamp(FontWeight::MIN.to_u16(), FontWeight::MAX.to_u16()),
                )
                .unwrap_or(FontWeight::NORMAL);
                m.set_style(weight, key.italic);
                m.set_family_variant(FamilyVariant::Default);
                if let Some(locale) = key.locale.as_deref().and_then(|l| CString::new(l).ok()) {
                    m.set_locales(&locale);
                }
                Matcher(m)
            })
            .0
    }

    /// Matches `text` under the generic `family` and builds the face answer.
    fn match_text(
        &mut self,
        family: &CStr,
        text: &str,
        weight: f32,
        italic: bool,
        locale: Option<&str>,
    ) -> Option<Matched> {
        let utf16: Vec<u16> = text.encode_utf16().collect();
        if utf16.is_empty() {
            return None;
        }
        let key = MatcherKey {
            weight: weight.round().clamp(1.0, 1000.0) as u16,
            italic,
            variant: FamilyVariant::Default.into(),
            locale: locale.map(str::to_owned),
        };
        let mut run = 0u32;
        let font = self
            .matcher(&key)
            .match_font(family, &utf16, Some(&mut run));
        let path = font.path().to_path_buf();
        let index = font.collection_index() as u32;
        let actual_weight = f32::from(font.weight().to_u16());
        let axes: Vec<VariationSetting> = (0..font.axis_count())
            .map(|i| VariationSetting {
                tag: font.axis_tag_at(i).to_be_bytes(),
                value: font.axis_value_at(i),
            })
            .collect();
        let file = path.to_string_lossy().into_owned();

        let has = |tag: &[u8; 4]| axes.iter().any(|a| &a.tag == tag);
        let mut synth = Synthesis::default();
        if has(b"wght") {
            // Variable face: the caller applies the requested weight on the axis.
            synth.variations.push(VariationSetting {
                tag: *b"wght",
                value: weight,
            });
        } else if weight >= 600.0 && actual_weight + 100.0 <= weight {
            // No bold file for this script (CJK) or family (mono): faux bold.
            synth.embolden = true;
        }
        if italic {
            if has(b"ital") {
                synth.variations.push(VariationSetting {
                    tag: *b"ital",
                    value: 1.0,
                });
            } else if !file_is_italic(&file) {
                synth.skew = Some(FAUX_OBLIQUE_DEGREES);
            }
        }
        log::debug!(
            "ndk font match: {:?} -> {}#{} (w{} italic={} run={} axes={:?}) hint={:?}",
            text,
            file,
            index,
            actual_weight,
            font.is_italic(),
            run,
            axes.iter()
                .map(|a| (String::from_utf8_lossy(&a.tag).into_owned(), a.value))
                .collect::<Vec<_>>(),
            synth,
        );
        // The NDK reports no PostScript name without parsing the file's name table, so
        // `postscript_name` stays `None`; `hidden` is false and `scale` is DirectWrite-only.
        // The emoji colour format needs no field: the selector classifies COLR/CBDT/sbix
        // from the face's own tables (`face_facts`), so `emoji_format` stays a log aid.
        let mut source = FaceSource::path(path, index);
        if synth != Synthesis::default() {
            source.synthesis_hint = Some(synth);
        }
        Some(Matched {
            source,
            file,
            run_utf16: run,
        })
    }
}

/// Whether the matched file is itself an italic design (`NotoSerif-Italic.ttf`).
fn file_is_italic(file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file);
    name.contains("Italic")
}

/// The color-glyph table format of an emoji file, for the render-capability check (D5):
/// the flags font is CBDT bitmaps, the main emoji font COLRv1.
fn emoji_format(file: &str) -> Option<&'static str> {
    let name = file.rsplit('/').next().unwrap_or(file);
    if !name.contains("Emoji") {
        None
    } else if name.contains("Flags") {
        Some("CBDT")
    } else {
        Some("COLR")
    }
}

/// The NDK generic-family name a slot maps to.
fn generic_name(slot: GenericSlot) -> &'static CStr {
    match slot {
        GenericSlot::Monospace => c"monospace",
        GenericSlot::Serif => c"serif",
        GenericSlot::SansSerif | GenericSlot::SystemUi | GenericSlot::Emoji => c"sans-serif",
    }
}

impl Backend for NdkFontBackend {
    fn resolve(&mut self, req: &FallbackRequest) -> Option<FaceSource> {
        let slot = req
            .generics()
            .first()
            .copied()
            .unwrap_or(GenericSlot::SystemUi);
        let italic = !matches!(req.slant, FontStyle::Normal);
        let started = Instant::now();
        let matched = self.match_text(
            generic_name(slot),
            &req.chars,
            req.weight.value(),
            italic,
            req.locale.as_deref(),
        )?;
        log::debug!(
            "ndk font miss {:?}: {} us, {}#{}{}{}",
            req.chars,
            started.elapsed().as_micros(),
            matched.file,
            matched.source.index,
            emoji_format(&matched.file)
                .map(|f| format!(" emoji={f}"))
                .unwrap_or_default(),
            if (matched.run_utf16 as usize) < req.chars.encode_utf16().count() {
                format!(
                    " (run {} of {} utf16)",
                    matched.run_utf16,
                    req.chars.encode_utf16().count()
                )
            } else {
                String::new()
            },
        );
        Some(matched.source)
    }

    fn default_family(&self, generic: GenericSlot) -> Option<FaceSource> {
        // `&self`: a throwaway backend keeps the matcher cache out of a read-only question.
        let sample = if generic == GenericSlot::Emoji {
            "\u{1F600}"
        } else {
            "H"
        };
        let matched =
            NdkFontBackend::new().match_text(generic_name(generic), sample, 400.0, false, None)?;
        Some(matched.source)
    }

    fn os_build_id(&self) -> String {
        system_property(c"ro.build.fingerprint")
            .map(|fp| format!("android:{fp}"))
            .unwrap_or_else(|| String::from("android-unknown"))
    }
}
