//! The fallback cache: system answers per cluster class, in memory and on disk.
//!
//! # In memory
//!
//! A bounded LRU ([`MEMORY_CAPACITY`] entries) keyed by [`CacheKey`] — the
//! cluster's *class* plus script, locale, attributes, family chain and
//! renderability. Misses are cached too ([`CachedAnswer::Miss`]): asking the
//! OS again for a cluster it cannot draw is the expensive path. Only system
//! answers enter it — app-font answers are resolved before the cache and never
//! stored (see `selector`).
//!
//! # On disk
//!
//! [`FallbackCache::attach_persistent`] loads a JSON copy (serde_json) and
//! [`FallbackCache::persist`] writes it back atomically. The file carries a
//! [`VersionStamp`] — the backend's OS build id, this crate's version and the
//! size + mtime of every font file an entry names — and **any** mismatch
//! discards the whole file: a partially valid cache is not worth reasoning
//! about. Entries whose face is an in-memory blob are never written.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use super::{
    FaceLocation, FaceSource, FallbackRequest, GenericKey, RenderCapabilities, ScriptTag,
    Synthesis, VariationSetting,
};
use crate::style::{FamilyName, FontStyle};

/// The in-memory LRU bound, in entries.
///
/// Skia's typeface fallback cache uses 4096; one entry per (cluster class,
/// script, locale, attributes, chain) is small (a key plus a path), so the
/// same bound costs well under a megabyte.
pub(crate) const MEMORY_CAPACITY: usize = 4096;

/// The persistent cache's file name under frust-paths' cache directory.
pub(crate) const PERSIST_FILE_NAME: &str = "font-fallback.json";

/// Bumped whenever the serialised shape changes; a mismatch discards the file.
const FORMAT_VERSION: u32 = 2;

/// Serialises every context's [`FallbackCache::persist`] write in the process.
static PERSIST_LOCK: Mutex<()> = Mutex::new(());

/// Hashable mirror of [`FamilyName`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum FamilyKey {
    /// Lower-cased: family names match case-insensitively.
    Named(String),
    Generic(GenericKey),
}

/// Hashable mirror of [`FontStyle`] (an oblique angle as its `f32` bits).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum SlantKey {
    Normal,
    Italic,
    Oblique(Option<u32>),
}

impl From<FontStyle> for SlantKey {
    fn from(style: FontStyle) -> Self {
        match style {
            FontStyle::Normal => Self::Normal,
            FontStyle::Italic => Self::Italic,
            FontStyle::Oblique(angle) => Self::Oblique(angle.map(f32::to_bits)),
        }
    }
}

/// What a fallback answer depends on.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct CacheKey {
    /// The cluster class: its characters, sorted and de-duplicated, so the
    /// same set of characters in any order shares one answer.
    pub(crate) class: String,
    pub(crate) script: ScriptTag,
    pub(crate) locale: Option<String>,
    /// [`crate::FontWeight`] value bits.
    pub(crate) weight: u32,
    pub(crate) slant: SlantKey,
    /// Width ratio bits.
    pub(crate) width: u32,
    pub(crate) families: Vec<FamilyKey>,
    pub(crate) renderability: RenderCapabilities,
}

impl CacheKey {
    pub(crate) fn from_request(req: &FallbackRequest) -> Self {
        let mut chars: Vec<char> = req.chars.chars().collect();
        chars.sort_unstable();
        chars.dedup();
        Self {
            class: chars.into_iter().collect(),
            script: req.script,
            locale: req.locale.clone(),
            weight: req.weight.value().to_bits(),
            slant: req.slant.into(),
            width: req.width.to_bits(),
            families: req
                .families
                .iter()
                .map(|f| match f {
                    FamilyName::Named(name) => FamilyKey::Named(name.to_lowercase()),
                    FamilyName::Generic(g) => FamilyKey::Generic((*g).into()),
                })
                .collect(),
            renderability: req.renderability,
        }
    }
}

/// One cached system answer.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CachedAnswer {
    Face(FaceSource),
    /// Nothing on the system could draw the cluster.
    Miss,
}

/// What a persistent file must match to be trusted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct VersionStamp {
    pub(crate) os_build_id: String,
    pub(crate) crate_version: String,
}

impl VersionStamp {
    /// The stamp for this build of frust-text on `os_build_id`.
    pub(crate) fn current(os_build_id: String) -> Self {
        Self {
            os_build_id,
            crate_version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

/// A font file's size and modification time when an entry was written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct FileStamp {
    path: PathBuf,
    size: u64,
    mtime_secs: u64,
    mtime_nanos: u32,
}

impl FileStamp {
    fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
        Some(Self {
            path: path.to_path_buf(),
            size: meta.len(),
            mtime_secs: mtime.as_secs(),
            mtime_nanos: mtime.subsec_nanos(),
        })
    }

    fn still_valid(&self) -> bool {
        Self::of(&self.path).as_ref() == Some(self)
    }
}

/// A serialisable [`CachedAnswer`] (path faces only).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
enum PersistedAnswer {
    Face {
        path: PathBuf,
        index: u32,
        variations: Vec<VariationSetting>,
        synthesis_hint: Option<Synthesis>,
        postscript_name: Option<String>,
        hidden: bool,
        scale: Option<f32>,
    },
    Miss,
}

/// The persistent file's shape.
#[derive(Debug, Serialize, Deserialize)]
struct PersistedCache {
    format: u32,
    stamp: VersionStamp,
    files: Vec<FileStamp>,
    /// Least recently used first, so a reload keeps the recency order.
    entries: Vec<(CacheKey, PersistedAnswer)>,
}

/// What [`FallbackCache::attach_persistent`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LoadOutcome {
    /// No file (first run, or nothing written yet).
    Absent,
    /// The file matched and this many entries were loaded.
    Loaded(usize),
    /// The file was discarded wholesale; the reason is for logs and tests.
    Invalidated(&'static str),
}

/// Counters for tests and traces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CacheStats {
    pub(crate) hits: usize,
    pub(crate) misses: usize,
    pub(crate) evictions: usize,
}

/// Where the persistent copy lives and what it must match.
#[derive(Debug)]
struct Persistence {
    path: PathBuf,
    stamp: VersionStamp,
}

/// The bounded LRU plus its optional persistent copy.
#[derive(Debug)]
pub(crate) struct FallbackCache {
    entries: HashMap<CacheKey, (CachedAnswer, u64)>,
    /// Recency order: tick → key, oldest first.
    order: BTreeMap<u64, CacheKey>,
    tick: u64,
    capacity: usize,
    persistence: Option<Persistence>,
    /// Whether an entry was added since the last load or write.
    dirty: bool,
    stats: CacheStats,
}

impl FallbackCache {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: BTreeMap::new(),
            tick: 0,
            capacity: capacity.max(1),
            persistence: None,
            dirty: false,
            stats: CacheStats::default(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn stats(&self) -> CacheStats {
        self.stats
    }

    /// The answer for `key`, marking it most recently used.
    pub(crate) fn get(&mut self, key: &CacheKey) -> Option<CachedAnswer> {
        let tick = self.next_tick();
        let Some((answer, stamp)) = self.entries.get_mut(key) else {
            self.stats.misses += 1;
            return None;
        };
        self.stats.hits += 1;
        let old = std::mem::replace(stamp, tick);
        let answer = answer.clone();
        if let Some(k) = self.order.remove(&old) {
            self.order.insert(tick, k);
        }
        Some(answer)
    }

    /// Records `answer` for `key`, evicting the least recently used entry when
    /// over capacity.
    pub(crate) fn insert(&mut self, key: CacheKey, answer: CachedAnswer) {
        self.dirty = true;
        self.insert_clean(key, answer);
    }

    fn insert_clean(&mut self, key: CacheKey, answer: CachedAnswer) {
        let tick = self.next_tick();
        if let Some((_, old)) = self.entries.insert(key.clone(), (answer, tick)) {
            self.order.remove(&old);
        }
        self.order.insert(tick, key);
        while self.entries.len() > self.capacity {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            self.entries.remove(&oldest);
            self.stats.evictions += 1;
        }
    }

    fn next_tick(&mut self) -> u64 {
        self.tick += 1;
        self.tick
    }

    /// Binds the cache to a persistent file and loads it when it matches
    /// `stamp` and every font file it names is unchanged; otherwise the file is
    /// ignored (and overwritten by the next [`Self::persist`]).
    pub(crate) fn attach_persistent(&mut self, path: PathBuf, stamp: VersionStamp) -> LoadOutcome {
        let outcome = self.load(&path, &stamp);
        match &outcome {
            LoadOutcome::Invalidated(reason) => {
                log::debug!(
                    "frust-text: font fallback cache {} discarded: {reason}",
                    path.display()
                );
                // Rewrite on the next persist even if nothing new is added.
                self.dirty = true;
            }
            LoadOutcome::Absent | LoadOutcome::Loaded(_) => {}
        }
        self.persistence = Some(Persistence { path, stamp });
        outcome
    }

    fn load(&mut self, path: &Path, stamp: &VersionStamp) -> LoadOutcome {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return LoadOutcome::Absent,
            Err(_) => return LoadOutcome::Invalidated("unreadable"),
        };
        let Ok(persisted) = serde_json::from_slice::<PersistedCache>(&bytes) else {
            return LoadOutcome::Invalidated("unparseable");
        };
        if persisted.format != FORMAT_VERSION {
            return LoadOutcome::Invalidated("format version");
        }
        if persisted.stamp.os_build_id != stamp.os_build_id {
            return LoadOutcome::Invalidated("os build id");
        }
        if persisted.stamp.crate_version != stamp.crate_version {
            return LoadOutcome::Invalidated("crate version");
        }
        if !persisted.files.iter().all(FileStamp::still_valid) {
            return LoadOutcome::Invalidated("font file changed");
        }
        let count = persisted.entries.len();
        for (key, answer) in persisted.entries {
            let answer = match answer {
                PersistedAnswer::Face {
                    path,
                    index,
                    variations,
                    synthesis_hint,
                    postscript_name,
                    hidden,
                    scale,
                } => CachedAnswer::Face(FaceSource {
                    location: FaceLocation::Path(path),
                    index,
                    variations,
                    synthesis_hint,
                    postscript_name,
                    hidden,
                    scale,
                }),
                PersistedAnswer::Miss => CachedAnswer::Miss,
            };
            self.insert_clean(key, answer);
        }
        LoadOutcome::Loaded(count)
    }

    /// Writes the persistent copy if one is attached and anything changed
    /// since it was loaded or last written. A no-op otherwise.
    pub(crate) fn persist(&mut self) -> io::Result<()> {
        let Some(persistence) = &self.persistence else {
            return Ok(());
        };
        if !self.dirty {
            return Ok(());
        }
        let mut files: Vec<FileStamp> = Vec::new();
        let mut entries = Vec::with_capacity(self.order.len());
        for key in self.order.values() {
            let Some((answer, _)) = self.entries.get(key) else {
                continue;
            };
            let persisted = match answer {
                CachedAnswer::Miss => PersistedAnswer::Miss,
                CachedAnswer::Face(source) => {
                    let FaceLocation::Path(path) = &source.location else {
                        continue;
                    };
                    if !files.iter().any(|f| &f.path == path) {
                        let Some(stamp) = FileStamp::of(path) else {
                            continue;
                        };
                        files.push(stamp);
                    }
                    PersistedAnswer::Face {
                        path: path.clone(),
                        index: source.index,
                        variations: source.variations.clone(),
                        synthesis_hint: source.synthesis_hint.clone(),
                        postscript_name: source.postscript_name.clone(),
                        hidden: source.hidden,
                        scale: source.scale,
                    }
                }
            };
            entries.push((key.clone(), persisted));
        }
        let persisted = PersistedCache {
            format: FORMAT_VERSION,
            stamp: persistence.stamp.clone(),
            files,
            entries,
        };
        let json = serde_json::to_vec(&persisted).map_err(io::Error::other)?;
        // `atomic_write`'s temp file is keyed on the PID alone, so two contexts
        // persisting at once (a shell's and a widget's) must not overlap.
        let _guard = PERSIST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        frust_paths::atomic_write(&persistence.path, &json)?;
        self.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::synthetic::{FaceSpec, TempDir};
    use super::*;
    use crate::style::{FontFamily, FontWeight, GenericSlot};

    fn request(chars: &str) -> FallbackRequest {
        FallbackRequest::new(
            chars,
            ScriptTag(*b"Latn"),
            &FontFamily::SystemUi,
            RenderCapabilities::default(),
        )
    }

    fn key(chars: &str) -> CacheKey {
        CacheKey::from_request(&request(chars))
    }

    fn stamp(build: &str) -> VersionStamp {
        VersionStamp::current(build.to_owned())
    }

    #[test]
    fn the_cluster_class_ignores_order_and_repeats() {
        assert_eq!(key("ab"), key("ba"));
        assert_eq!(key("aab"), key("ab"));
        assert_ne!(key("ab"), key("abc"));
    }

    #[test]
    fn every_attribute_is_part_of_the_key() {
        let base = request("a");
        let k = CacheKey::from_request(&base);
        let mut bold = base.clone();
        bold.weight = FontWeight::BOLD;
        let mut italic = base.clone();
        italic.slant = FontStyle::Italic;
        let mut ja = base.clone();
        ja.locale = Some("ja".into());
        let mut mono = base.clone();
        mono.families = vec![FamilyName::Generic(GenericSlot::Monospace)];
        let mut no_bitmaps = base.clone();
        no_bitmaps.renderability.bitmap = false;
        for other in [bold, italic, ja, mono, no_bitmaps] {
            assert_ne!(CacheKey::from_request(&other), k, "{other:?}");
        }
        let mut cased = base.clone();
        cased.families = vec![FamilyName::Named("Roboto".into())];
        let mut lower = base;
        lower.families = vec![FamilyName::Named("roboto".into())];
        assert_eq!(
            CacheKey::from_request(&cased),
            CacheKey::from_request(&lower)
        );
    }

    #[test]
    fn the_lru_is_bounded_and_evicts_the_least_recently_used() {
        let mut cache = FallbackCache::new(3);
        for c in ["a", "b", "c"] {
            cache.insert(key(c), CachedAnswer::Miss);
        }
        // Touch "a" so "b" is now the oldest.
        assert_eq!(cache.get(&key("a")), Some(CachedAnswer::Miss));
        cache.insert(key("d"), CachedAnswer::Miss);
        assert_eq!(cache.len(), 3);
        assert_eq!(cache.get(&key("b")), None, "the LRU entry was evicted");
        assert!(cache.get(&key("a")).is_some());
        assert!(cache.get(&key("c")).is_some());
        assert!(cache.get(&key("d")).is_some());
        assert_eq!(cache.stats().evictions, 1);
        for i in 0..100 {
            cache.insert(key(&format!("x{i}")), CachedAnswer::Miss);
        }
        assert_eq!(cache.len(), 3, "never grows past its bound");
        assert_eq!(cache.order.len(), 3);
    }

    #[test]
    fn the_memory_bound_is_skias() {
        assert_eq!(MEMORY_CAPACITY, 4096);
    }

    /// A cache file under `dir` holding one face entry (backed by a real font
    /// file) and one miss, written with `stamp`.
    fn write_cache(dir: &TempDir, stamp: VersionStamp) -> (PathBuf, PathBuf) {
        let font = dir.write("face.ttf", &FaceSpec::new("Face").chars("a").build());
        let file = dir.path().join("cache").join(PERSIST_FILE_NAME);
        let mut cache = FallbackCache::new(16);
        assert_eq!(
            cache.attach_persistent(file.clone(), stamp),
            LoadOutcome::Absent
        );
        cache.insert(
            key("a"),
            CachedAnswer::Face(FaceSource {
                synthesis_hint: Some(Synthesis {
                    embolden: true,
                    ..Synthesis::default()
                }),
                ..FaceSource::path(&font, 0)
            }),
        );
        cache.insert(key("ж"), CachedAnswer::Miss);
        // An in-memory face is never written.
        cache.insert(
            key("b"),
            CachedAnswer::Face(FaceSource::blob(peniko::Blob::from(vec![0u8; 4]), 0)),
        );
        cache.persist().expect("cache written");
        (file, font)
    }

    #[test]
    fn the_persistent_cache_round_trips() {
        let dir = TempDir::new("persist-roundtrip");
        let (file, font) = write_cache(&dir, stamp("build-1"));
        let mut cache = FallbackCache::new(16);
        assert_eq!(
            cache.attach_persistent(file, stamp("build-1")),
            LoadOutcome::Loaded(2)
        );
        let Some(CachedAnswer::Face(source)) = cache.get(&key("a")) else {
            panic!("the face entry survives");
        };
        assert_eq!(source.location, FaceLocation::Path(font));
        assert!(source.synthesis_hint.is_some_and(|h| h.embolden));
        assert_eq!(cache.get(&key("ж")), Some(CachedAnswer::Miss));
        assert_eq!(cache.get(&key("b")), None, "blob faces are not persisted");
        assert!(!cache.dirty, "a clean load does not need rewriting");
    }

    #[test]
    fn the_persistent_cache_is_discarded_on_a_new_os_build() {
        let dir = TempDir::new("persist-os");
        let (file, _) = write_cache(&dir, stamp("build-1"));
        let mut cache = FallbackCache::new(16);
        assert_eq!(
            cache.attach_persistent(file, stamp("build-2")),
            LoadOutcome::Invalidated("os build id")
        );
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn the_persistent_cache_is_discarded_on_a_new_crate_version() {
        let dir = TempDir::new("persist-crate");
        let old = VersionStamp {
            crate_version: "0.0.1-old".into(),
            ..stamp("build-1")
        };
        let (file, _) = write_cache(&dir, old);
        let mut cache = FallbackCache::new(16);
        assert_eq!(
            cache.attach_persistent(file, stamp("build-1")),
            LoadOutcome::Invalidated("crate version")
        );
    }

    #[test]
    fn the_persistent_cache_is_discarded_when_a_font_file_changes_size() {
        let dir = TempDir::new("persist-size");
        let (file, font) = write_cache(&dir, stamp("build-1"));
        let mtime = std::fs::metadata(&font).unwrap().modified().unwrap();
        let mut bytes = std::fs::read(&font).unwrap();
        bytes.extend_from_slice(&[0; 4]);
        std::fs::write(&font, &bytes).unwrap();
        // Same mtime: only the size differs.
        std::fs::File::options()
            .write(true)
            .open(&font)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        let mut cache = FallbackCache::new(16);
        assert_eq!(
            cache.attach_persistent(file, stamp("build-1")),
            LoadOutcome::Invalidated("font file changed")
        );
    }

    #[test]
    fn the_persistent_cache_is_discarded_when_a_font_file_changes_mtime() {
        let dir = TempDir::new("persist-mtime");
        let (file, font) = write_cache(&dir, stamp("build-1"));
        let mtime = std::fs::metadata(&font).unwrap().modified().unwrap();
        std::fs::File::options()
            .write(true)
            .open(&font)
            .unwrap()
            .set_modified(mtime + std::time::Duration::from_secs(5))
            .unwrap();
        let mut cache = FallbackCache::new(16);
        assert_eq!(
            cache.attach_persistent(file, stamp("build-1")),
            LoadOutcome::Invalidated("font file changed")
        );
    }

    #[test]
    fn the_persistent_cache_is_discarded_when_a_font_file_disappears() {
        let dir = TempDir::new("persist-gone");
        let (file, font) = write_cache(&dir, stamp("build-1"));
        std::fs::remove_file(&font).unwrap();
        let mut cache = FallbackCache::new(16);
        assert_eq!(
            cache.attach_persistent(file, stamp("build-1")),
            LoadOutcome::Invalidated("font file changed")
        );
    }

    #[test]
    fn a_corrupt_or_old_format_file_is_discarded_and_rewritten() {
        let dir = TempDir::new("persist-corrupt");
        let file = dir.write("cache.json", b"{not json");
        let mut cache = FallbackCache::new(16);
        assert_eq!(
            cache.attach_persistent(file.clone(), stamp("b")),
            LoadOutcome::Invalidated("unparseable")
        );
        cache.persist().unwrap();
        let mut reread = FallbackCache::new(16);
        assert_eq!(
            reread.attach_persistent(file.clone(), stamp("b")),
            LoadOutcome::Loaded(0)
        );

        let text = std::fs::read_to_string(&file).unwrap();
        let bumped = text.replacen(
            &format!("\"format\":{FORMAT_VERSION}"),
            &format!("\"format\":{}", FORMAT_VERSION + 1),
            1,
        );
        assert_ne!(
            text, bumped,
            "the format field is where the test expects it"
        );
        std::fs::write(&file, bumped).unwrap();
        let mut old = FallbackCache::new(16);
        assert_eq!(
            old.attach_persistent(file, stamp("b")),
            LoadOutcome::Invalidated("format version")
        );
    }

    #[test]
    fn the_new_face_fields_round_trip_and_a_format_one_file_is_discarded() {
        let dir = TempDir::new("persist-fields");
        let font = dir.write("face.ttf", &FaceSpec::new("Face").chars("a").build());
        let file = dir.path().join("cache").join(PERSIST_FILE_NAME);
        let mut cache = FallbackCache::new(16);
        cache.attach_persistent(file.clone(), stamp("b"));
        cache.insert(
            key("a"),
            CachedAnswer::Face(FaceSource {
                postscript_name: Some("Face-Regular".into()),
                hidden: true,
                scale: Some(0.75),
                ..FaceSource::path(&font, 0)
            }),
        );
        cache.persist().unwrap();
        let mut reread = FallbackCache::new(16);
        assert_eq!(
            reread.attach_persistent(file.clone(), stamp("b")),
            LoadOutcome::Loaded(1)
        );
        let Some(CachedAnswer::Face(source)) = reread.get(&key("a")) else {
            panic!("the face survives");
        };
        assert_eq!(source.postscript_name.as_deref(), Some("Face-Regular"));
        assert!(source.hidden);
        assert_eq!(source.scale, Some(0.75));

        // A file written by the previous format (no new fields) is discarded.
        let text = std::fs::read_to_string(&file).unwrap();
        let old = text.replacen(&format!("\"format\":{FORMAT_VERSION}"), "\"format\":1", 1);
        assert_ne!(text, old);
        std::fs::write(&file, old).unwrap();
        let mut stale = FallbackCache::new(16);
        assert_eq!(
            stale.attach_persistent(file, stamp("b")),
            LoadOutcome::Invalidated("format version")
        );
        assert_eq!(stale.len(), 0);
    }

    #[test]
    fn persist_is_a_no_op_without_changes_or_a_file() {
        let mut detached = FallbackCache::new(4);
        detached.insert(key("a"), CachedAnswer::Miss);
        detached
            .persist()
            .expect("nothing attached: nothing written");

        let dir = TempDir::new("persist-noop");
        let file = dir.path().join(PERSIST_FILE_NAME);
        let mut cache = FallbackCache::new(4);
        cache.attach_persistent(file.clone(), stamp("b"));
        cache.persist().unwrap();
        assert!(!file.exists(), "a clean cache writes nothing");
    }
}
