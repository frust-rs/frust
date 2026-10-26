//! Test fixtures: a scripted [`Backend`] over in-memory faces, a builder for
//! tiny but well-formed font files, and a scratch directory.
//!
//! The faces carry exactly the tables the oracle reads — `cmap` (format 12),
//! `OS/2`, `name`, optional `fvar`, and a placeholder glyph-data table (`glyf`,
//! or `hvgl` for an hvgl-only face) — so coverage, attributes, naming and the
//! renderability filter run over real parsing with fully controlled answers,
//! on every host.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use peniko::Blob;

use super::selector::coverage_exempt;
use super::{Backend, FaceSource, FallbackRequest, GenericKey};
use crate::style::{FamilyName, GenericSlot};

/// What the backend was asked, shared with the test after the backend moved
/// into its oracle.
#[derive(Clone, Debug, Default)]
pub(crate) struct Calls(Arc<Mutex<CallLog>>);

#[derive(Debug, Default)]
struct CallLog {
    requests: Vec<FallbackRequest>,
}

impl Calls {
    /// How many times [`Backend::resolve`] ran.
    pub(crate) fn resolves(&self) -> usize {
        self.0.lock().unwrap().requests.len()
    }

    /// The last request [`Backend::resolve`] saw.
    pub(crate) fn last_request(&self) -> Option<FallbackRequest> {
        self.0.lock().unwrap().requests.last().cloned()
    }

    /// Every request [`Backend::resolve`] saw, in order.
    pub(crate) fn requests(&self) -> Vec<FallbackRequest> {
        self.0.lock().unwrap().requests.clone()
    }

    /// The named family each request led with (`None` for a generic-only
    /// request), in order.
    pub(crate) fn named_families(&self) -> Vec<Option<String>> {
        self.requests()
            .iter()
            .map(|req| first_named(req).map(str::to_owned))
            .collect()
    }
}

/// A backend answering from a scripted coverage map.
///
/// `resolve` answers like the platform backends do: only the request's
/// *first* named family is consulted — a [`Self::family`] entry of that name
/// whose scripted characters cover the cluster, else a [`Self::cascade_for`]
/// entry of that name (the OS cascading from that family to another) — and
/// otherwise the first [`Self::cover`] entry that does (the OS cascade,
/// whatever family that face is). Walking later families is the oracle's job. The script may claim more
/// than the face's `cmap` holds — that is how a test exercises the oracle's own
/// coverage check.
#[derive(Debug)]
pub(crate) struct SyntheticBackend {
    os_build_id: String,
    families: Vec<(String, String, FaceSource)>,
    cascades: Vec<(String, String, FaceSource)>,
    coverage: Vec<(String, FaceSource)>,
    defaults: Vec<(GenericKey, FaceSource)>,
    calls: Calls,
}

impl SyntheticBackend {
    pub(crate) fn new(os_build_id: &str) -> Self {
        Self {
            os_build_id: os_build_id.to_owned(),
            families: Vec::new(),
            cascades: Vec::new(),
            coverage: Vec::new(),
            defaults: Vec::new(),
            calls: Calls::default(),
        }
    }

    /// The call log handle.
    pub(crate) fn calls(&self) -> Calls {
        self.calls.clone()
    }

    /// Scripts `blob` (face 0) as the system fallback for `chars`.
    pub(crate) fn cover(&mut self, blob: &Blob<u8>, chars: &str) {
        self.cover_with(FaceSource::blob(blob.clone(), 0), chars);
    }

    /// Scripts `source` as the system fallback for `chars`.
    pub(crate) fn cover_with(&mut self, source: FaceSource, chars: &str) {
        self.coverage.push((chars.to_owned(), source));
    }

    /// Scripts `blob` as the installed family `name`, covering `chars`.
    pub(crate) fn family(&mut self, name: &str, blob: &Blob<u8>, chars: &str) {
        self.families.push((
            name.to_lowercase(),
            chars.to_owned(),
            FaceSource::blob(blob.clone(), 0),
        ));
    }

    /// Scripts `blob` as what a request naming family `name` cascades to for
    /// `chars` (CoreText cascading from an installed Menlo that lacks the
    /// cluster): a face of another family, answered only to that request.
    pub(crate) fn cascade_for(&mut self, name: &str, blob: &Blob<u8>, chars: &str) {
        self.cascades.push((
            name.to_lowercase(),
            chars.to_owned(),
            FaceSource::blob(blob.clone(), 0),
        ));
    }

    /// Scripts `blob` as the system default for `generic`.
    pub(crate) fn default_for(&mut self, generic: GenericSlot, blob: &Blob<u8>) {
        self.default_with(generic, FaceSource::blob(blob.clone(), 0));
    }

    /// Scripts `source` (coordinates and hint included) as the system default
    /// for `generic`.
    pub(crate) fn default_with(&mut self, generic: GenericSlot, source: FaceSource) {
        self.defaults.push((generic.into(), source));
    }
}

/// The request's first named family.
fn first_named(req: &FallbackRequest) -> Option<&str> {
    req.families.iter().find_map(|f| match f {
        FamilyName::Named(name) => Some(name.as_str()),
        FamilyName::Generic(_) => None,
    })
}

/// Whether the scripted `claimed` characters cover `chars`.
fn claims(claimed: &str, chars: &str) -> bool {
    chars
        .chars()
        .filter(|c| !coverage_exempt(*c))
        .all(|c| claimed.contains(c))
}

impl Backend for SyntheticBackend {
    fn resolve(&mut self, req: &FallbackRequest) -> Option<FaceSource> {
        self.calls.0.lock().unwrap().requests.push(req.clone());
        if let Some(name) = first_named(req).map(str::to_lowercase)
            && let Some((_, _, source)) = self
                .families
                .iter()
                .chain(&self.cascades)
                .find(|(n, claimed, _)| *n == name && claims(claimed, &req.chars))
        {
            return Some(source.clone());
        }
        self.coverage
            .iter()
            .find(|(claimed, _)| claims(claimed, &req.chars))
            .map(|(_, source)| source.clone())
    }

    fn default_family(&self, generic: GenericSlot) -> Option<FaceSource> {
        let key = GenericKey::from(generic);
        self.defaults
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, source)| source.clone())
    }

    fn os_build_id(&self) -> String {
        self.os_build_id.clone()
    }
}

/// A tiny font face to build.
#[derive(Clone, Debug)]
pub(crate) struct FaceSpec {
    family: String,
    chars: Vec<char>,
    weight: u16,
    italic: bool,
    axes: Vec<[u8; 4]>,
    hvgl_only: bool,
}

impl FaceSpec {
    pub(crate) fn new(family: &str) -> Self {
        Self {
            family: family.to_owned(),
            chars: Vec::new(),
            weight: 400,
            italic: false,
            axes: Vec::new(),
            hvgl_only: false,
        }
    }

    /// Characters the `cmap` maps (each to its own non-zero glyph id).
    pub(crate) fn chars(mut self, chars: &str) -> Self {
        self.chars.extend(chars.chars());
        self
    }

    /// `OS/2.usWeightClass`.
    pub(crate) fn weight(mut self, weight: u16) -> Self {
        self.weight = weight;
        self
    }

    /// Sets `OS/2.fsSelection`'s italic bit.
    pub(crate) fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    /// Adds `fvar` axes.
    pub(crate) fn axes(mut self, axes: &[[u8; 4]]) -> Self {
        self.axes.extend_from_slice(axes);
        self
    }

    /// Glyph data in `hvgl` only (no `glyf`), like iOS's PingFangUI.
    pub(crate) fn hvgl_only(mut self) -> Self {
        self.hvgl_only = true;
        self
    }

    /// A single-face font file.
    pub(crate) fn build(&self) -> Vec<u8> {
        let mut out = Vec::new();
        write_font(&mut out, &self.tables());
        out
    }

    /// A TTC holding `specs` in order (face index = position).
    pub(crate) fn collection(specs: &[Self]) -> Vec<u8> {
        let count = u32::try_from(specs.len()).expect("few faces");
        let mut out = Vec::new();
        out.extend_from_slice(b"ttcf");
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&count.to_be_bytes());
        let offsets_at = out.len();
        out.resize(offsets_at + 4 * specs.len(), 0);
        for (i, spec) in specs.iter().enumerate() {
            let start = u32::try_from(out.len()).expect("small file");
            out[offsets_at + 4 * i..offsets_at + 4 * i + 4].copy_from_slice(&start.to_be_bytes());
            write_font(&mut out, &spec.tables());
        }
        out
    }

    fn tables(&self) -> Vec<([u8; 4], Vec<u8>)> {
        let mut tables = vec![
            (*b"cmap", self.cmap()),
            (*b"OS/2", self.os2()),
            (*b"name", self.name()),
            (if self.hvgl_only { *b"hvgl" } else { *b"glyf" }, vec![0; 4]),
        ];
        if !self.axes.is_empty() {
            tables.push((*b"fvar", self.fvar()));
        }
        tables.sort_by_key(|(tag, _)| *tag);
        tables
    }

    fn cmap(&self) -> Vec<u8> {
        let mut chars = self.chars.clone();
        chars.sort_unstable();
        chars.dedup();
        let groups = u32::try_from(chars.len()).expect("few chars");
        let mut t = Vec::new();
        t.extend_from_slice(&0u16.to_be_bytes()); // version
        t.extend_from_slice(&1u16.to_be_bytes()); // numTables
        t.extend_from_slice(&3u16.to_be_bytes()); // platform: Windows
        t.extend_from_slice(&10u16.to_be_bytes()); // encoding: UCS-4
        t.extend_from_slice(&12u32.to_be_bytes()); // subtable offset
        t.extend_from_slice(&12u16.to_be_bytes()); // format
        t.extend_from_slice(&0u16.to_be_bytes()); // reserved
        t.extend_from_slice(&(16 + 12 * groups).to_be_bytes()); // length
        t.extend_from_slice(&0u32.to_be_bytes()); // language
        t.extend_from_slice(&groups.to_be_bytes());
        for (gid, c) in (1u32..).zip(chars) {
            let c = u32::from(c);
            t.extend_from_slice(&c.to_be_bytes());
            t.extend_from_slice(&c.to_be_bytes());
            t.extend_from_slice(&gid.to_be_bytes());
        }
        t
    }

    fn os2(&self) -> Vec<u8> {
        let mut t = vec![0u8; 96];
        t[0..2].copy_from_slice(&4u16.to_be_bytes()); // version 4
        t[4..6].copy_from_slice(&self.weight.to_be_bytes()); // usWeightClass
        t[6..8].copy_from_slice(&5u16.to_be_bytes()); // usWidthClass: normal
        let selection: u16 = if self.italic { 0x0001 } else { 0x0040 };
        t[62..64].copy_from_slice(&selection.to_be_bytes()); // fsSelection
        t
    }

    fn name(&self) -> Vec<u8> {
        let utf16: Vec<u8> = self
            .family
            .encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect();
        let len = u16::try_from(utf16.len()).expect("short name");
        let mut t = Vec::new();
        t.extend_from_slice(&0u16.to_be_bytes()); // format
        t.extend_from_slice(&1u16.to_be_bytes()); // count
        t.extend_from_slice(&18u16.to_be_bytes()); // storage offset: 6 + 12
        for field in [3u16, 1, 0x0409, 1, len, 0] {
            t.extend_from_slice(&field.to_be_bytes());
        }
        t.extend_from_slice(&utf16);
        t
    }

    fn fvar(&self) -> Vec<u8> {
        let count = u16::try_from(self.axes.len()).expect("few axes");
        let mut t = Vec::new();
        for field in [1u16, 0, 16, 2, count, 20, 0, 4 + 4 * count] {
            t.extend_from_slice(&field.to_be_bytes());
        }
        for tag in &self.axes {
            let (min, default, max): (i32, i32, i32) = match tag {
                b"wght" => (100, 400, 900),
                b"wdth" => (50, 100, 200),
                b"slnt" => (-15, 0, 0),
                _ => (0, 0, 1),
            };
            t.extend_from_slice(tag);
            for v in [min, default, max] {
                t.extend_from_slice(&(v << 16).to_be_bytes());
            }
            t.extend_from_slice(&0u16.to_be_bytes()); // flags
            t.extend_from_slice(&256u16.to_be_bytes()); // axisNameID
        }
        t
    }
}

/// Appends an sfnt (table directory + 4-byte-aligned tables) at the end of
/// `out`; table offsets are absolute, so this also lays out TTC members.
fn write_font(out: &mut Vec<u8>, tables: &[([u8; 4], Vec<u8>)]) {
    let count = u16::try_from(tables.len()).expect("few tables");
    let dir_start = out.len();
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    out.extend_from_slice(&count.to_be_bytes());
    for field in [0u16, 0, 0] {
        // searchRange/entrySelector/rangeShift: unused by readers here.
        out.extend_from_slice(&field.to_be_bytes());
    }
    let records_at = out.len();
    out.resize(records_at + 16 * tables.len(), 0);
    debug_assert_eq!(records_at - dir_start, 12);
    for (i, (tag, data)) in tables.iter().enumerate() {
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
        let offset = u32::try_from(out.len()).expect("small file");
        let length = u32::try_from(data.len()).expect("small table");
        let record = records_at + 16 * i;
        out[record..record + 4].copy_from_slice(tag);
        out[record + 8..record + 12].copy_from_slice(&offset.to_be_bytes());
        out[record + 12..record + 16].copy_from_slice(&length.to_be_bytes());
        out.extend_from_slice(data);
    }
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

/// A scratch directory under the system temp dir, removed on drop.
#[derive(Debug)]
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-text-oracle-{}-{label}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    /// Writes `bytes` to `name` inside the directory.
    pub(crate) fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).expect("temp write");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use read_fonts::{FileRef, FontRef, TableProvider};

    #[test]
    fn built_faces_parse_with_read_fonts() {
        let face = FaceSpec::new("Probe")
            .chars("ab中")
            .axes(&[*b"wght"])
            .build();
        let font = FontRef::new(&face).expect("a valid sfnt");
        let cmap = font.cmap().expect("cmap");
        assert!(cmap.map_codepoint('a').is_some());
        assert!(cmap.map_codepoint('中').is_some());
        assert!(cmap.map_codepoint('z').is_none());
        assert_eq!(font.os2().unwrap().us_weight_class(), 400);
        assert_eq!(font.fvar().unwrap().axes().unwrap().len(), 1);

        let ttc =
            FaceSpec::collection(&[FaceSpec::new("A").chars("a"), FaceSpec::new("B").chars("b")]);
        let FileRef::Collection(collection) = FileRef::new(&ttc).unwrap() else {
            panic!("a TTC");
        };
        assert_eq!(collection.len(), 2);
        let second = FontRef::from_index(&ttc, 1).unwrap();
        assert!(second.cmap().unwrap().map_codepoint('b').is_some());
        assert!(second.cmap().unwrap().map_codepoint('a').is_none());
    }
}
