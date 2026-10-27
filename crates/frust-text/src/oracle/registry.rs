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
use read_fonts::{FileRef, FontRef, TableProvider};

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
    /// The face's PostScript name, read the way the Apple oracle reads a
    /// system face's (see [`postscript_name`]); `None` when the face names
    /// none. A renderer that opens a face by name (CoreText, for an
    /// `hvgl`-only face) needs it.
    pub(crate) postscript_name: Option<String>,
    /// The name is system-private: it starts with `.`.
    pub(crate) hidden: bool,
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

/// The PostScript name `font`'s face is known by, from its `name` table.
///
/// The candidates, as the Apple oracle's face walk reads them: the face's
/// name ID 6, then the PostScript name of the `fvar` named instance at the
/// face's default coordinates (the instance the face draws as unvaried — how a
/// collection face that is one of a variable family's named instances is
/// named). The first public (not dot-prefixed) candidate wins, else the first
/// at all: a public name is one CoreText can open the face by. After those,
/// every other named instance's name is a candidate, in `fvar` order: any
/// named instance of the face opens the same outlines in CoreText (the handle's
/// coordinates select the instance, which the `hvgl` provider applies), so
/// the first public one names a face whose name ID 6 and default instance are
/// both hidden.
fn postscript_name(font: &FontRef<'_>, name: &Name<'_>) -> Option<String> {
    let id6 = english_or_first(name, NameId::POSTSCRIPT_NAME);
    let (default_instance, other_instances) = font
        .fvar()
        .ok()
        .and_then(|fvar| {
            let axes = fvar.axes().ok()?;
            let mut default_instance = None;
            let mut others = Vec::new();
            for instance in fvar.instances().ok()?.iter().flatten() {
                let Some(id) = instance.post_script_name_id else {
                    continue;
                };
                let at_default = instance.coordinates.len() == axes.len()
                    && axes
                        .iter()
                        .zip(instance.coordinates)
                        .all(|(axis, value)| value.get() == axis.default_value());
                let Some(found) = english_or_first(name, id) else {
                    continue;
                };
                if at_default && default_instance.is_none() {
                    default_instance = Some(found);
                } else {
                    others.push(found);
                }
            }
            Some((default_instance, others))
        })
        .unwrap_or_default();
    let candidates = [id6, default_instance]
        .into_iter()
        .chain(other_instances.into_iter().map(Some));
    let mut named = candidates.into_iter().flatten().filter(|n| !n.is_empty());
    let first = named.next()?;
    if !first.starts_with('.') {
        return Some(first);
    }
    Some(named.find(|n| !n.starts_with('.')).unwrap_or(first))
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
        let postscript_name = postscript_name(&font, &name);
        let hidden = postscript_name
            .as_deref()
            .is_some_and(|n| n.starts_with('.'));
        faces.push(AppFace {
            blob: blob.clone(),
            index,
            family,
            facts: face_facts(&font),
            postscript_name,
            hidden,
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

    const NOTO_SANS: &[u8] = include_bytes!("../../../../testing/fonts/NotoSans-Subset.ttf");

    /// A single-face sfnt holding `tables` (tag, bytes), table data 4-aligned.
    fn sfnt(tables: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
        let count = u16::try_from(tables.len()).expect("few tables");
        let mut out = vec![0x00, 0x01, 0x00, 0x00];
        out.extend(count.to_be_bytes());
        out.extend([0; 6]);
        let mut offset = 12 + 16 * tables.len();
        let mut data = Vec::new();
        for (tag, bytes) in tables {
            out.extend(tag);
            out.extend([0; 4]);
            out.extend(u32::try_from(offset).expect("small").to_be_bytes());
            out.extend(u32::try_from(bytes.len()).expect("small").to_be_bytes());
            let padded = bytes.len().div_ceil(4) * 4;
            data.extend(bytes);
            data.resize(data.len() + padded - bytes.len(), 0);
            offset += padded;
        }
        out.extend(data);
        out
    }

    /// A `name` table of `(name id, string)` Windows US-English records.
    fn name_table(records: &[(u16, &str)]) -> Vec<u8> {
        let count = u16::try_from(records.len()).expect("few records");
        let mut head = Vec::new();
        let mut strings = Vec::new();
        for field in [0, count, 6 + 12 * count] {
            head.extend(field.to_be_bytes());
        }
        for (id, string) in records {
            let utf16: Vec<u8> = string.encode_utf16().flat_map(u16::to_be_bytes).collect();
            let length = u16::try_from(utf16.len()).expect("short");
            let offset = u16::try_from(strings.len()).expect("short");
            for field in [3, 1, 0x0409, *id, length, offset] {
                head.extend(field.to_be_bytes());
            }
            strings.extend(utf16);
        }
        head.extend(strings);
        head
    }

    /// An `fvar` of one `wght` axis (100..900, default 400) and a named
    /// instance per `(weight, PostScript name id)`.
    fn wght_fvar(instances: &[(i32, u16)]) -> Vec<u8> {
        let count = u16::try_from(instances.len()).expect("few instances");
        let mut out = Vec::new();
        // Version 1.0, axes at 16, reserved 2, one 20-byte axis, `count`
        // instances of 4 + 4 + 2 bytes (the PostScript name id included).
        for field in [1_u16, 0, 16, 2, 1, 20, count, 10] {
            out.extend(field.to_be_bytes());
        }
        out.extend(b"wght");
        for value in [100_i32, 400, 900] {
            out.extend((value << 16).to_be_bytes());
        }
        out.extend([0, 0, 1, 0]);
        for (weight, ps_id) in instances {
            out.extend([0, 2, 0, 0]);
            out.extend((weight << 16).to_be_bytes());
            out.extend(ps_id.to_be_bytes());
        }
        out
    }

    /// The one face `bytes` registers.
    fn only_face(bytes: Vec<u8>) -> AppFace {
        let mut registry = AppFontRegistry::default();
        registry.insert_blob(&Blob::from(bytes));
        assert_eq!(registry.face_count(), 1);
        registry.faces.remove(0)
    }

    #[test]
    fn a_registered_face_reports_its_postscript_name() {
        let face = only_face(NOTO_SANS.to_vec());
        assert_eq!(face.postscript_name.as_deref(), Some("NotoSans-Regular"));
        assert!(!face.hidden, "a public name is not hidden");
        // A face whose `name` table carries no PostScript name has none.
        let nameless = only_face(FaceSpec::new("No PS").chars("a").build());
        assert_eq!(nameless.postscript_name, None);
        assert!(!nameless.hidden);
    }

    /// Only a dot-prefixed name is hidden: the same face with each name ID 6
    /// string's first character patched to `.` flips the flag, nothing else.
    #[test]
    fn only_a_dot_name_is_hidden() {
        let mut bytes = NOTO_SANS.to_vec();
        let patches: Vec<(usize, bool)> = {
            let font = FontRef::new(&bytes).expect("a face");
            let name = font.name().expect("a name table");
            let table = font
                .table_data(read_fonts::types::Tag::new(b"name"))
                .expect("a name table");
            let table_at = table.as_bytes().as_ptr() as usize - bytes.as_ptr() as usize;
            let storage = table_at + usize::from(name.storage_offset());
            name.name_record()
                .iter()
                .filter(|record| record.name_id() == NameId::POSTSCRIPT_NAME)
                .map(|record| {
                    let offset = usize::try_from(record.string_offset().to_u32()).expect("small");
                    let at = storage + offset;
                    // Windows and Unicode records are UTF-16BE; Mac Roman is
                    // one byte per character.
                    (at, record.platform_id() != 1)
                })
                .collect()
        };
        assert!(!patches.is_empty(), "the face carries name ID 6");
        for (at, utf16) in patches {
            if utf16 {
                bytes[at] = 0;
                bytes[at + 1] = b'.';
            } else {
                bytes[at] = b'.';
            }
        }
        let face = only_face(bytes);
        assert_eq!(face.postscript_name.as_deref(), Some(".otoSans-Regular"));
        assert!(face.hidden);
    }

    /// The named instance at the face's default coordinates names a face with
    /// no name ID 6, and a public instance name beats a hidden name ID 6.
    #[test]
    fn a_variable_face_is_named_by_its_default_instance() {
        let face = |records: &[(u16, &str)]| {
            only_face(sfnt(&[
                (*b"fvar", wght_fvar(&[(700, 257), (400, 256)])),
                (*b"glyf", vec![0; 4]),
                (*b"name", name_table(records)),
            ]))
        };
        let names = [(1, "Var"), (256, "Var-Regular"), (257, "Var-Bold")];
        let unnamed = face(&names);
        assert_eq!(unnamed.postscript_name.as_deref(), Some("Var-Regular"));
        assert!(!unnamed.hidden);

        let hidden_id6 = face(&[names[0], (6, ".VarUI-Regular"), names[1], names[2]]);
        assert_eq!(hidden_id6.postscript_name.as_deref(), Some("Var-Regular"));
        assert!(!hidden_id6.hidden);

        let public_id6 = face(&[names[0], (6, "VarFace-Regular"), names[1], names[2]]);
        assert_eq!(
            public_id6.postscript_name.as_deref(),
            Some("VarFace-Regular")
        );

        let all_hidden = face(&[(1, "Var"), (6, ".VarUI-Regular"), (256, ".VarUI-Text")]);
        assert_eq!(
            all_hidden.postscript_name.as_deref(),
            Some(".VarUI-Regular")
        );
        assert!(all_hidden.hidden);
    }

    /// A collection-style face whose name ID 6 and default-instance name are
    /// hidden is named by the first public name among its other instances.
    #[test]
    fn a_hidden_default_is_named_by_the_first_public_instance() {
        let face = only_face(sfnt(&[
            (*b"fvar", wght_fvar(&[(400, 256), (500, 257), (700, 258)])),
            (*b"glyf", vec![0; 4]),
            (
                *b"name",
                name_table(&[
                    (1, "Var"),
                    (6, ".VarUI-Default"),
                    (256, ".VarUI-Regular"),
                    (257, "Var-Medium"),
                    (258, "Var-Bold"),
                ]),
            ),
        ]));
        assert_eq!(face.postscript_name.as_deref(), Some("Var-Medium"));
        assert!(!face.hidden);
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
