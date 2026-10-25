//! The app-font registry: fonts an app bundles, and faces a host registers as
//! generic-family fallbacks.
//!
//! # Process-wide records, per-context parses
//!
//! [`APP_FONTS`] and [`GENERIC_FALLBACKS`] are the process-wide, append-only
//! records every `TextContext` agrees on; each context's
//! [`AppFontRegistry`] keeps a watermark into both and parses what it has not
//! seen yet (read-fonts: family name, attributes; the `cmap` is read at
//! resolve time). A [`RegistryMirror`] lets the context forward the same blobs
//! to a second font collection while one still exists beside the oracle.
//!
//! App faces are consulted before anything the system offers, and a family
//! the registry provides is removed from the request the system is asked
//! (see `selector`), so an app face *shadows* a same-named system family.

use std::sync::Mutex;

use peniko::Blob;
use read_fonts::tables::name::Name;
use read_fonts::types::NameId;
use read_fonts::{FileRef, TableProvider};

use super::selector::face_facts;
use super::{FaceFacts, GenericKey};
use crate::style::GenericSlot;

/// The process-wide record of every font blob an app registration accepted,
/// in registration order and **never drained**.
///
/// # Why this lives here
///
/// App fonts enter the framework through `frust::register_app_fonts`, whose
/// pending-byte slot (`frust_shell_common::font_registry`) each shell drains —
/// destructively — into the one shell-owned `TextContext`. That context is the
/// one `LayoutCtx::text_context` threads to widgets during layout, so every
/// widget shaping through it (`Text`) already sees app fonts. The gap is the
/// widget that legitimately owns a *private* context: `TextInput` applies edits
/// synchronously during the event pass, where no context is threaded (see
/// `frust_widgets::textinput`'s module docs), and a private context built after
/// the shell drained the pending slot would otherwise carry no app font at all.
///
/// `frust-widgets` cannot read the shell's slot — `frust-shell-common` sits
/// *above* it (`docs/ARCHITECTURE.md`'s layer dependencies) — so the durable
/// record lives at the one layer the shell drain and every widget both already
/// depend on. Registering into any context records the blob here; every other
/// context picks it up through [`AppFontRegistry::sync`], which a new context
/// runs for free.
///
/// Append-only by design: the payloads must survive to seed contexts created
/// *later*, which is exactly what a drain-once slot cannot do. Blobs are
/// `Arc`-backed, so seeding a fresh context costs a refcount bump plus a
/// parse, never a copy of the font bytes.
static APP_FONTS: Mutex<Vec<Blob<u8>>> = Mutex::new(Vec::new());

/// One `register_generic_fallback` call's payload: the raw font bytes plus
/// the generic slot(s) it should be appended to.
#[derive(Clone, Debug)]
pub(crate) struct PendingGenericFallback {
    pub(crate) blob: Blob<u8>,
    pub(crate) generics: Vec<GenericSlot>,
}

/// The process-wide record of every `register_generic_fallback` call accepted
/// so far, in registration order and **never drained** — the generic-family
/// counterpart of [`APP_FONTS`].
static GENERIC_FALLBACKS: Mutex<Vec<PendingGenericFallback>> = Mutex::new(Vec::new());

/// Records `data` as a fallback face for each slot in `generics`, for every
/// context (existing ones on their next sync). No-op when `generics` is empty.
pub(crate) fn record_generic_fallback(data: Vec<u8>, generics: &[GenericSlot]) {
    if generics.is_empty() {
        return;
    }
    let mut slot = GENERIC_FALLBACKS.lock().unwrap_or_else(|e| e.into_inner());
    slot.push(PendingGenericFallback {
        blob: Blob::from(data),
        generics: generics.to_vec(),
    });
}

/// How many generic fallbacks the process has recorded (tests).
#[cfg(test)]
pub(crate) fn generic_fallback_count() -> usize {
    GENERIC_FALLBACKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .len()
}

/// One registered family, as reported back to the registering caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FamilySummary {
    pub(crate) name: String,
    pub(crate) face_count: usize,
}

/// The registered bytes held no parseable face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NoFacesFound;

/// A second font collection that receives the same registrations — the
/// context's bridge while the oracle runs beside an older collection.
pub(crate) trait RegistryMirror {
    /// Registers an app-font blob; returns the families it registered.
    fn app_font(&mut self, blob: &Blob<u8>) -> Vec<FamilySummary>;
    /// Registers a generic fallback; returns whether any face registered.
    fn generic_fallback(&mut self, blob: &Blob<u8>, generics: &[GenericSlot]) -> bool;
}

/// One parsed app face.
#[derive(Clone, Debug)]
pub(crate) struct AppFace {
    pub(crate) blob: Blob<u8>,
    pub(crate) index: u32,
    /// The face's family name (typographic family, else family; English, else
    /// the first record — fontique's naming rule, so names match).
    pub(crate) family: String,
    pub(crate) facts: FaceFacts,
}

/// The `name` table string for `id`: the US-English record, else a
/// language-0 record, else the first record (fontique's `english_or_first`).
fn english_or_first(name: &Name<'_>, id: NameId) -> Option<String> {
    let mut best: Option<(i32, usize)> = None;
    for (i, record) in name.name_record().iter().enumerate() {
        if record.name_id() != id {
            continue;
        }
        let rank = match (i, record.language_id()) {
            (_, 0x0409) => {
                best = Some((i32::MAX, i));
                break;
            }
            (_, 0) => 2,
            (0, _) => 1,
            _ => continue,
        };
        if best.is_none_or(|(r, _)| rank > r) {
            best = Some((rank, i));
        }
    }
    let (_, i) = best?;
    let record = name.name_record().get(i)?;
    let string = record.string(name.string_data()).ok()?;
    Some(string.chars().collect())
}

/// Every face in `blob` (one, or each face of a collection) that has a family
/// name.
fn parse_faces(blob: &Blob<u8>) -> Vec<AppFace> {
    let Ok(file) = FileRef::new(blob.data()) else {
        return Vec::new();
    };
    let mut faces = Vec::new();
    for (index, font) in file.fonts().enumerate() {
        let Ok(font) = font else {
            continue;
        };
        let Ok(name) = font.name() else {
            continue;
        };
        let family = english_or_first(&name, NameId::TYPOGRAPHIC_FAMILY_NAME)
            .or_else(|| english_or_first(&name, NameId::FAMILY_NAME))
            .filter(|n| !n.is_empty());
        let (Some(family), Ok(index)) = (family, u32::try_from(index)) else {
            continue;
        };
        faces.push(AppFace {
            blob: blob.clone(),
            index,
            family,
            facts: face_facts(&font),
        });
    }
    faces
}

/// Groups `faces` into per-family counts, in first-appearance order.
fn summarise(faces: &[AppFace]) -> Vec<FamilySummary> {
    let mut out: Vec<FamilySummary> = Vec::new();
    for face in faces {
        match out.iter_mut().find(|f| f.name == face.family) {
            Some(summary) => summary.face_count += 1,
            None => out.push(FamilySummary {
                name: face.family.clone(),
                face_count: 1,
            }),
        }
    }
    out
}

/// One context's view of the app fonts and generic fallbacks.
#[derive(Debug, Default)]
pub(crate) struct AppFontRegistry {
    faces: Vec<AppFace>,
    generic: Vec<(GenericKey, AppFace)>,
    /// Watermark into [`APP_FONTS`].
    app_fonts_applied: usize,
    /// Watermark into [`GENERIC_FALLBACKS`].
    generic_fallbacks_applied: usize,
}

impl AppFontRegistry {
    /// Parses `blob` into this registry only (no process-wide record);
    /// returns the families it added.
    pub(crate) fn insert_blob(&mut self, blob: &Blob<u8>) -> Vec<FamilySummary> {
        let faces = parse_faces(blob);
        let summary = summarise(&faces);
        self.faces.extend(faces);
        summary
    }

    /// Adds `blob`'s faces as fallbacks for each of `generics`, in this
    /// registry only; returns whether any face parsed.
    pub(crate) fn insert_generic(&mut self, blob: &Blob<u8>, generics: &[GenericSlot]) -> bool {
        let faces = parse_faces(blob);
        for &generic in generics {
            for face in &faces {
                self.generic.push((generic.into(), face.clone()));
            }
        }
        !faces.is_empty()
    }

    /// Registers an app font: records it process-wide and parses it here,
    /// mirroring it (and any record entries this context missed) to `mirror`.
    ///
    /// Returns the mirror's family names when it registered any (they are
    /// what the older collection resolves by), else this registry's own.
    pub(crate) fn register(
        &mut self,
        blob: Blob<u8>,
        mirror: &mut dyn RegistryMirror,
    ) -> Result<Vec<FamilySummary>, NoFacesFound> {
        // Held across the registration so the watermark cannot skip a blob
        // another context records concurrently.
        let mut slot = APP_FONTS.lock().unwrap_or_else(|e| e.into_inner());
        // Catch up first: this context may be behind the record, and the
        // watermark below would otherwise declare those blobs applied.
        for missed in &slot[self.app_fonts_applied..] {
            self.insert_blob(missed);
            let _ = mirror.app_font(missed);
        }
        self.app_fonts_applied = slot.len();

        let ours = self.insert_blob(&blob);
        let mirrored = mirror.app_font(&blob);
        if ours.is_empty() && mirrored.is_empty() {
            return Err(NoFacesFound);
        }
        slot.push(blob);
        self.app_fonts_applied = slot.len();
        Ok(if mirrored.is_empty() { ours } else { mirrored })
    }

    /// Applies every app font and generic fallback recorded since this
    /// context last synced; returns whether anything registered. Cheap when
    /// nothing is pending: two locks and two length compares.
    pub(crate) fn sync(&mut self, mirror: &mut dyn RegistryMirror) -> bool {
        let mut applied = {
            let slot = GENERIC_FALLBACKS.lock().unwrap_or_else(|e| e.into_inner());
            self.apply_generic_entries(&slot, mirror)
        };
        let slot = APP_FONTS.lock().unwrap_or_else(|e| e.into_inner());
        if self.app_fonts_applied != slot.len() {
            for blob in &slot[self.app_fonts_applied..] {
                let ours = self.insert_blob(blob);
                let mirrored = mirror.app_font(blob);
                if !ours.is_empty() || !mirrored.is_empty() {
                    applied = true;
                }
            }
            self.app_fonts_applied = slot.len();
        }
        applied
    }

    /// Applies `entries[watermark..]` and advances the generic watermark to
    /// `entries.len()`; `false` (no work) when it is already there.
    ///
    /// Pure over `entries` — the process-wide record in production, a local
    /// list in tests, since a test registering through the real record would
    /// leak its face into every later context of the test binary.
    pub(crate) fn apply_generic_entries(
        &mut self,
        entries: &[PendingGenericFallback],
        mirror: &mut dyn RegistryMirror,
    ) -> bool {
        if self.generic_fallbacks_applied >= entries.len() {
            return false;
        }
        let mut applied = false;
        for entry in &entries[self.generic_fallbacks_applied..] {
            let ours = self.insert_generic(&entry.blob, &entry.generics);
            let mirrored = mirror.generic_fallback(&entry.blob, &entry.generics);
            applied |= ours || mirrored;
        }
        self.generic_fallbacks_applied = entries.len();
        applied
    }

    /// The app faces of family `name` (case-insensitive), registration order.
    pub(crate) fn faces_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a AppFace> {
        self.faces
            .iter()
            .filter(move |face| face.family.eq_ignore_ascii_case(name))
    }

    /// Whether any app face belongs to family `name`.
    pub(crate) fn has_family(&self, name: &str) -> bool {
        self.faces_named(name).next().is_some()
    }

    /// The app generic fallbacks for `generic`, registration order.
    pub(crate) fn generic_faces(&self, generic: GenericSlot) -> impl Iterator<Item = &AppFace> {
        let key = GenericKey::from(generic);
        self.generic
            .iter()
            .filter(move |(k, _)| *k == key)
            .map(|(_, face)| face)
    }

    /// How many app faces this registry holds.
    pub(crate) fn face_count(&self) -> usize {
        self.faces.len()
    }
}

#[cfg(test)]
mod tests {
    use super::super::synthetic::FaceSpec;
    use super::*;
    use crate::style::FontStyle;

    const TUFFY: &[u8] = include_bytes!("../../tests/fonts/Tuffy-Subset.ttf");
    const TUFFY_AS_HELVETICA: &[u8] = include_bytes!("../../tests/fonts/Tuffy-As-Helvetica.ttf");

    /// Records what it was asked to mirror.
    #[derive(Default)]
    struct RecordingMirror {
        app: usize,
        generic: usize,
    }

    impl RegistryMirror for RecordingMirror {
        fn app_font(&mut self, _blob: &Blob<u8>) -> Vec<FamilySummary> {
            self.app += 1;
            Vec::new()
        }
        fn generic_fallback(&mut self, _blob: &Blob<u8>, _generics: &[GenericSlot]) -> bool {
            self.generic += 1;
            false
        }
    }

    #[test]
    fn real_faces_parse_their_family_and_attributes() {
        let mut registry = AppFontRegistry::default();
        let families = registry.insert_blob(&Blob::from(TUFFY.to_vec()));
        assert_eq!(
            families,
            vec![FamilySummary {
                name: "Tuffy".into(),
                face_count: 1
            }]
        );
        let renamed = registry.insert_blob(&Blob::from(TUFFY_AS_HELVETICA.to_vec()));
        assert_eq!(renamed[0].name, "Helvetica");
        let face = registry
            .faces_named("tuffy")
            .next()
            .expect("case-insensitive");
        assert!(face.facts.formats.outlines);
        assert_eq!(face.facts.attrs.style, FontStyle::Normal);
        assert!(registry.has_family("HELVETICA"));
        assert!(!registry.has_family("Roboto"));
    }

    #[test]
    fn synthetic_attributes_and_collections_parse() {
        let mut registry = AppFontRegistry::default();
        let ttc = FaceSpec::collection(&[
            FaceSpec::new("Family A").weight(700),
            FaceSpec::new("Family A").italic(),
            FaceSpec::new("Family B"),
        ]);
        let families = registry.insert_blob(&Blob::from(ttc));
        assert_eq!(
            families,
            vec![
                FamilySummary {
                    name: "Family A".into(),
                    face_count: 2
                },
                FamilySummary {
                    name: "Family B".into(),
                    face_count: 1
                },
            ]
        );
        let a: Vec<_> = registry.faces_named("Family A").collect();
        assert_eq!((a[0].index, a[1].index), (0, 1));
        assert_eq!(a[0].facts.attrs.weight, 700.0);
        assert_eq!(a[1].facts.attrs.style, FontStyle::Italic);
    }

    #[test]
    fn garbage_registers_nothing() {
        let mut registry = AppFontRegistry::default();
        assert!(registry.insert_blob(&Blob::from(vec![1, 2, 3])).is_empty());
        assert!(registry.insert_blob(&Blob::from(Vec::new())).is_empty());
        assert_eq!(registry.face_count(), 0);
    }

    #[test]
    fn generic_entries_apply_once_and_mirror() {
        let entries = vec![PendingGenericFallback {
            blob: Blob::from(FaceSpec::new("Bundled").chars("a").build()),
            generics: vec![GenericSlot::SystemUi, GenericSlot::SansSerif],
        }];
        let mut registry = AppFontRegistry::default();
        let mut mirror = RecordingMirror::default();
        assert!(registry.apply_generic_entries(&entries, &mut mirror));
        assert_eq!(registry.generic_faces(GenericSlot::SystemUi).count(), 1);
        assert_eq!(registry.generic_faces(GenericSlot::SansSerif).count(), 1);
        assert_eq!(registry.generic_faces(GenericSlot::Monospace).count(), 0);
        assert_eq!(mirror.generic, 1);
        assert!(
            !registry.apply_generic_entries(&entries, &mut mirror),
            "nothing new: a repeat apply is a no-op"
        );
        assert_eq!(registry.generic_faces(GenericSlot::SystemUi).count(), 1);
        assert_eq!(mirror.generic, 1);
        assert_eq!(mirror.app, 0);
    }

    #[test]
    fn recording_an_empty_generic_list_is_a_no_op() {
        // Only the empty case: a real slot would stay registered for every
        // later context in this test binary (the record is never drained).
        let before = generic_fallback_count();
        record_generic_fallback(vec![1, 2, 3], &[]);
        assert_eq!(generic_fallback_count(), before);
    }
}
