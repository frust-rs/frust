//! The fallback cache: system answers per cluster class, in memory and on disk.
//!
//! In memory: a bounded LRU ([`MEMORY_CAPACITY`]) indexed by [`CacheKey::digest`],
//! a stable 64-bit hash of the cluster's *class* plus script, locale,
//! attributes, family chain and renderability. Misses are cached too. Only
//! system answers enter it — app-font answers never do (see `selector`).
//!
//! On disk: a JSON copy bound eagerly ([`FallbackCache::attach_persistent`]) or
//! at the first in-memory miss or flush ([`FallbackCache::attach_lazy`]), keyed
//! by digest only, so no cluster text, locale or family name is written in
//! the clear. The digest is unsalted over a small input space, so it is
//! pseudonymous, not secret: a dictionary pass over code points, locales and
//! weights recovers which keys were looked up. Its
//! header carries a [`VersionStamp`] (OS build id, crate version), the size +
//! mtime of every font file an entry names, and the font directories' mtimes.
//! A version or file mismatch, or a face whose file is not stamped, discards
//! the whole file; a font-directory change drops only the misses. Blob faces
//! are never written.
//!
//! Writes are write-behind ([`WRITE_BEHIND_ENTRIES`] new answers, or the first
//! lookup after [`IDLE_FLUSH_AFTER`] without one), explicit
//! ([`FallbackCache::persist`]) and final (the owner's `Drop`), each through a
//! per-call temp file (PID + random nonce), fsync and rename.

use std::collections::hash_map::RandomState;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::fs::File;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
const FORMAT_VERSION: u32 = 3;

/// New answers since the last write that trigger a write-behind flush.
///
/// A first screen of CJK or emoji text is a burst of tens of misses: 32 bounds
/// what a killed process loses of one burst while keeping a session to a
/// handful of writes.
pub(crate) const WRITE_BEHIND_ENTRIES: usize = 32;

/// How long the cache may sit dirty without a new answer before the next
/// lookup writes it (a trickle of answers that stops is still saved).
pub(crate) const IDLE_FLUSH_AFTER: Duration = Duration::from_secs(2);

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

/// What a fallback answer depends on. Held in memory only: the disk sees
/// its [`Self::digest`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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

    /// The persistent file's key: a process- and toolchain-stable 64-bit
    /// hash of every field.
    ///
    /// Unsalted and over a small input space, so it hides the key from a
    /// casual read, not from a dictionary pass (pseudonymous, not secret).
    /// Not std's `DefaultHasher`, whose algorithm may change between Rust
    /// releases.
    pub(crate) fn digest(&self) -> u64 {
        let mut h = StableHasher::new();
        h.bytes(b"frust-text fallback key");
        h.str(&self.class);
        h.bytes(&self.script.0);
        match &self.locale {
            None => h.bytes(&[0]),
            Some(locale) => {
                h.bytes(&[1]);
                h.str(locale);
            }
        }
        h.u32(self.weight);
        match self.slant {
            SlantKey::Normal => h.bytes(&[0]),
            SlantKey::Italic => h.bytes(&[1]),
            SlantKey::Oblique(None) => h.bytes(&[2]),
            SlantKey::Oblique(Some(bits)) => {
                h.bytes(&[3]);
                h.u32(bits);
            }
        }
        h.u32(self.width);
        h.u64(self.families.len() as u64);
        for family in &self.families {
            match family {
                FamilyKey::Named(name) => {
                    h.bytes(&[0]);
                    h.str(name);
                }
                FamilyKey::Generic(generic) => h.bytes(&[1, generic_id(*generic)]),
            }
        }
        let caps = self.renderability;
        h.bytes(&[
            u8::from(caps.outlines),
            u8::from(caps.colr),
            u8::from(caps.bitmap),
            u8::from(caps.hvgl_via_coretext),
        ]);
        h.finish()
    }
}

/// A fixed id per generic slot for [`CacheKey::digest`] (never reorder).
fn generic_id(generic: GenericKey) -> u8 {
    match generic {
        GenericKey::Monospace => 0,
        GenericKey::SansSerif => 1,
        GenericKey::Serif => 2,
        GenericKey::SystemUi => 3,
        GenericKey::Emoji => 4,
    }
}

/// FNV-1a over length-prefixed fields, finished with the murmur3 `fmix64`
/// avalanche.
struct StableHasher(u64);

impl StableHasher {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Self(Self::OFFSET)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    fn u32(&mut self, value: u32) {
        self.bytes(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn str(&mut self, value: &str) {
        self.u64(value.len() as u64);
        self.bytes(value.as_bytes());
    }

    fn finish(self) -> u64 {
        let mut x = self.0;
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
        x ^= x >> 33;
        x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        x ^ (x >> 33)
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
    /// The font directories whose mtimes are stamped (the file stores the
    /// stamps, which carry the paths).
    #[serde(skip)]
    pub(crate) font_dirs: Vec<PathBuf>,
}

impl VersionStamp {
    /// The stamp for this build of frust-text on `os_build_id`, watching this
    /// platform's font directories.
    pub(crate) fn current(os_build_id: String) -> Self {
        Self {
            os_build_id,
            crate_version: env!("CARGO_PKG_VERSION").to_owned(),
            font_dirs: super::font_dirs(),
        }
    }

    /// The same stamp watching `dirs` instead.
    pub(crate) fn with_font_dirs(mut self, dirs: Vec<PathBuf>) -> Self {
        self.font_dirs = dirs;
        self
    }
}

/// A modification time as (seconds, nanoseconds) since the Unix epoch.
fn mtime_of(meta: &std::fs::Metadata) -> Option<(u64, u32)> {
    let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some((mtime.as_secs(), mtime.subsec_nanos()))
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
        let (mtime_secs, mtime_nanos) = mtime_of(&meta)?;
        Some(Self {
            path: path.to_path_buf(),
            size: meta.len(),
            mtime_secs,
            mtime_nanos,
        })
    }

    fn still_valid(&self) -> bool {
        Self::of(&self.path).as_ref() == Some(self)
    }
}

/// A font directory's mtime (`None`: absent or unreadable) when the cache
/// was attached. Installing or removing a font changes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct DirStamp {
    path: PathBuf,
    mtime: Option<(u64, u32)>,
}

impl DirStamp {
    fn all(dirs: &[PathBuf]) -> Vec<Self> {
        dirs.iter()
            .map(|path| Self {
                path: path.clone(),
                mtime: std::fs::metadata(path).ok().as_ref().and_then(mtime_of),
            })
            .collect()
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
    dirs: Vec<DirStamp>,
    files: Vec<FileStamp>,
    /// (key digest, answer), least recently used first, so a reload keeps
    /// the recency order.
    entries: Vec<(u64, PersistedAnswer)>,
}

/// What [`FallbackCache::attach_persistent`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LoadOutcome {
    /// No file (first run, or nothing written yet).
    Absent,
    /// The file matched and this many entries were loaded (expired misses
    /// excluded).
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
    /// Persistent writes that reached the disk.
    pub(crate) writes: usize,
    /// Loaded misses dropped because a font directory changed.
    pub(crate) expired_misses: usize,
}

/// Resolves the persistent file's path on first use; `None` means "not yet"
/// (e.g. Android before the shell installed its directories) and is asked
/// again at the next miss or flush.
pub(crate) struct PathResolver(Box<dyn FnMut() -> Option<PathBuf> + Send>);

impl PathResolver {
    pub(crate) fn new(resolve: impl FnMut() -> Option<PathBuf> + Send + 'static) -> Self {
        Self(Box::new(resolve))
    }

    /// A resolver that always answers `path`.
    pub(crate) fn fixed(path: PathBuf) -> Self {
        Self::new(move || Some(path.clone()))
    }
}

impl std::fmt::Debug for PathResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PathResolver(..)")
    }
}

/// Where the persistent copy lives and what it must match.
#[derive(Debug)]
struct Persistence {
    path: PathBuf,
    stamp: VersionStamp,
    /// The font directories as they were at attach time: the conservative
    /// stamp for every miss this process records.
    dirs: Vec<DirStamp>,
}

/// The cache's binding to its persistent copy.
#[derive(Debug)]
enum Persist {
    Off,
    Pending {
        resolve: PathResolver,
        stamp: VersionStamp,
    },
    Attached(Persistence),
}

/// One in-memory entry. `key` is the plaintext key for answers computed in
/// this process (a digest collision then reads as a miss); `None` for an
/// entry loaded from disk until its first hit.
#[derive(Debug)]
struct Slot {
    key: Option<CacheKey>,
    answer: CachedAnswer,
    tick: u64,
}

/// The bounded LRU plus its optional persistent copy.
#[derive(Debug)]
pub(crate) struct FallbackCache {
    entries: HashMap<u64, Slot>,
    /// Recency order: tick → digest, oldest first.
    order: BTreeMap<u64, u64>,
    tick: u64,
    capacity: usize,
    persist: Persist,
    /// Whether the persistent copy is behind memory.
    dirty: bool,
    /// Answers added since the last write.
    unwritten: usize,
    /// When the cache last became (or stayed) dirty; `None` without persistence.
    last_change: Option<Instant>,
    write_behind: usize,
    idle_after: Duration,
    load_time: Duration,
    stats: CacheStats,
}

impl FallbackCache {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: BTreeMap::new(),
            tick: 0,
            capacity: capacity.max(1),
            persist: Persist::Off,
            dirty: false,
            unwritten: 0,
            last_change: None,
            write_behind: WRITE_BEHIND_ENTRIES,
            idle_after: IDLE_FLUSH_AFTER,
            load_time: Duration::ZERO,
            stats: CacheStats::default(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn stats(&self) -> CacheStats {
        self.stats
    }

    /// Whether the persistent copy is behind memory.
    pub(crate) fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Whether a persistent copy is bound or pending (not opted out).
    pub(crate) fn is_persistent(&self) -> bool {
        !matches!(self.persist, Persist::Off)
    }

    /// The persistent file's path, once resolved.
    pub(crate) fn attached_path(&self) -> Option<&Path> {
        match &self.persist {
            Persist::Attached(p) => Some(&p.path),
            Persist::Off | Persist::Pending { .. } => None,
        }
    }

    /// How long reading the persistent copy took (zero until it is attached).
    pub(crate) fn load_time(&self) -> Duration {
        self.load_time
    }

    /// Overrides the write-behind triggers ([`WRITE_BEHIND_ENTRIES`],
    /// [`IDLE_FLUSH_AFTER`]).
    pub(crate) fn set_write_behind(&mut self, entries: usize, idle_after: Duration) {
        self.write_behind = entries.max(1);
        self.idle_after = idle_after;
    }

    /// The answer for `key`, marking it most recently used. An in-memory miss
    /// first attaches a pending persistent copy and looks again.
    pub(crate) fn get(&mut self, key: &CacheKey) -> Option<CachedAnswer> {
        let digest = key.digest();
        let mut found = self.lookup(digest, key);
        if found.is_none() && self.try_attach() {
            found = self.lookup(digest, key);
        }
        if found.is_some() {
            self.stats.hits += 1;
        } else {
            self.stats.misses += 1;
        }
        self.flush_if_idle();
        found
    }

    fn lookup(&mut self, digest: u64, key: &CacheKey) -> Option<CachedAnswer> {
        self.tick += 1;
        let tick = self.tick;
        let slot = self.entries.get_mut(&digest)?;
        match &slot.key {
            Some(known) if known != key => return None,
            Some(_) => {}
            None => slot.key = Some(key.clone()),
        }
        let old = std::mem::replace(&mut slot.tick, tick);
        let answer = slot.answer.clone();
        self.order.remove(&old);
        self.order.insert(tick, digest);
        Some(answer)
    }

    /// Records `answer` for `key`, evicting the least recently used entry when
    /// over capacity, and writes behind once enough answers accumulated.
    pub(crate) fn insert(&mut self, key: CacheKey, answer: CachedAnswer) {
        self.tick += 1;
        let tick = self.tick;
        let digest = key.digest();
        let slot = Slot {
            key: Some(key),
            answer,
            tick,
        };
        if let Some(old) = self.entries.insert(digest, slot) {
            self.order.remove(&old.tick);
        }
        self.order.insert(tick, digest);
        self.evict_to_capacity();
        self.dirty = true;
        self.unwritten += 1;
        if self.is_persistent() {
            self.last_change = Some(Instant::now());
            if self.unwritten >= self.write_behind {
                self.flush_quietly("write-behind");
            }
        }
    }

    fn evict_to_capacity(&mut self) {
        while self.entries.len() > self.capacity {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            self.entries.remove(&oldest);
            self.stats.evictions += 1;
        }
    }

    /// Adds loaded entries as *older* than everything already in memory (an
    /// answer computed this process wins over its on-disk copy).
    fn merge_older(&mut self, loaded: Vec<(u64, CachedAnswer)>) {
        let existing: Vec<u64> = self.order.values().copied().collect();
        let mut order = BTreeMap::new();
        let mut tick = 0;
        for (digest, answer) in loaded {
            if self.entries.contains_key(&digest) {
                continue;
            }
            tick += 1;
            let slot = Slot {
                key: None,
                answer,
                tick,
            };
            self.entries.insert(digest, slot);
            order.insert(tick, digest);
        }
        for digest in existing {
            tick += 1;
            if let Some(slot) = self.entries.get_mut(&digest) {
                slot.tick = tick;
            }
            order.insert(tick, digest);
        }
        self.order = order;
        self.tick = tick;
        self.evict_to_capacity();
    }

    /// Binds the cache to a persistent file now and loads it when it matches
    /// `stamp` and every font file it names is unchanged; otherwise the file is
    /// ignored (and overwritten by the next [`Self::persist`]).
    pub(crate) fn attach_persistent(&mut self, path: PathBuf, stamp: VersionStamp) -> LoadOutcome {
        let started = Instant::now();
        let dirs = DirStamp::all(&stamp.font_dirs);
        let outcome = self.load(&path, &stamp, &dirs);
        self.load_time = started.elapsed();
        match &outcome {
            LoadOutcome::Invalidated(reason) => {
                log::debug!(
                    "frust-text: font fallback cache {} discarded: {reason}",
                    path.display()
                );
                // Rewrite on the next flush even if nothing new is added.
                self.dirty = true;
                self.last_change = Some(Instant::now());
            }
            LoadOutcome::Absent | LoadOutcome::Loaded(_) => {}
        }
        self.persist = Persist::Attached(Persistence { path, stamp, dirs });
        outcome
    }

    /// Binds the cache to a persistent file whose path `resolve` produces at
    /// the first in-memory miss or flush.
    pub(crate) fn attach_lazy(&mut self, resolve: PathResolver, stamp: VersionStamp) {
        self.persist = Persist::Pending { resolve, stamp };
    }

    /// Attaches a pending persistent copy if its path now resolves; whether
    /// this call attached it.
    fn try_attach(&mut self) -> bool {
        match std::mem::replace(&mut self.persist, Persist::Off) {
            Persist::Pending { mut resolve, stamp } => match (resolve.0)() {
                Some(path) => {
                    self.attach_persistent(path, stamp);
                    true
                }
                None => {
                    self.persist = Persist::Pending { resolve, stamp };
                    false
                }
            },
            other => {
                self.persist = other;
                false
            }
        }
    }

    fn load(&mut self, path: &Path, stamp: &VersionStamp, dirs: &[DirStamp]) -> LoadOutcome {
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
        let stamped: HashSet<&Path> = persisted.files.iter().map(|f| f.path.as_path()).collect();
        let all_stamped = persisted.entries.iter().all(|(_, answer)| match answer {
            PersistedAnswer::Face { path, .. } => stamped.contains(path.as_path()),
            PersistedAnswer::Miss => true,
        });
        if !all_stamped {
            return LoadOutcome::Invalidated("unstamped face path");
        }
        let expire_misses = persisted.dirs != dirs;
        let mut loaded = Vec::with_capacity(persisted.entries.len());
        for (digest, answer) in persisted.entries {
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
                PersistedAnswer::Miss if expire_misses => {
                    self.stats.expired_misses += 1;
                    continue;
                }
                PersistedAnswer::Miss => CachedAnswer::Miss,
            };
            loaded.push((digest, answer));
        }
        let count = loaded.len();
        self.merge_older(loaded);
        if expire_misses {
            // Rewrite with the new directory stamps and without the misses.
            self.dirty = true;
            self.last_change = Some(Instant::now());
        }
        LoadOutcome::Loaded(count)
    }

    /// Flushes when dirty and no answer arrived for the idle interval.
    fn flush_if_idle(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(last) = self.last_change
            && last.elapsed() >= self.idle_after
        {
            self.flush_quietly("idle");
        }
    }

    /// A background-trigger flush: failures are logged, and a flush that
    /// could not write (an error, or a path not yet resolvable) re-arms both
    /// triggers rather than retrying on every call.
    fn flush_quietly(&mut self, trigger: &str) {
        if let Err(err) = self.persist() {
            log::debug!("frust-text: font fallback cache {trigger} write failed: {err}");
        }
        if self.dirty {
            self.unwritten = 0;
            self.last_change = Some(Instant::now());
        }
    }

    /// Writes the persistent copy if one is bound (resolving a pending path
    /// first) and anything changed since it was loaded or last written. A
    /// no-op otherwise; an unresolved path leaves the cache dirty.
    pub(crate) fn persist(&mut self) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        self.try_attach();
        let Persist::Attached(persistence) = &self.persist else {
            return Ok(());
        };
        let mut files: Vec<FileStamp> = Vec::new();
        let mut entries = Vec::with_capacity(self.order.len());
        for digest in self.order.values() {
            let Some(slot) = self.entries.get(digest) else {
                continue;
            };
            let persisted = match &slot.answer {
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
            entries.push((*digest, persisted));
        }
        let persisted = PersistedCache {
            format: FORMAT_VERSION,
            stamp: persistence.stamp.clone(),
            dirs: persistence.dirs.clone(),
            files,
            entries,
        };
        let json = serde_json::to_vec(&persisted).map_err(io::Error::other)?;
        write_atomically(&persistence.path, &json)?;
        self.dirty = false;
        self.unwritten = 0;
        self.last_change = None;
        self.stats.writes += 1;
        Ok(())
    }
}

/// A temp-file name beside `path` that no other writer, in this process or
/// another, picks: PID plus a random nonce.
fn temp_name(path: &Path) -> OsString {
    static CALLS: AtomicU64 = AtomicU64::new(0);
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(CALLS.fetch_add(1, Ordering::Relaxed));
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    hasher.write_u128(now);
    let mut name = OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(format!(
        ".{}.{:016x}.tmp",
        std::process::id(),
        hasher.finish()
    ));
    name
}

/// Writes `bytes` to `path` so a reader only ever sees a complete file: a
/// unique temp file, fsync, rename over `path` (atomic; with racing writers
/// the last rename wins and every candidate is whole). The temp file is
/// removed when any step fails.
fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temp = parent.join(temp_name(path));
    let written = File::create_new(&temp).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    let result = written.and_then(|()| rename_with_retry(&temp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    } else {
        sync_dir(parent);
    }
    result
}

/// `rename` over the destination, retried on Windows: when two writers
/// replace the same file at once, the loser's `MoveFileEx` answers "access
/// denied" (or a sharing violation) for a moment even though nothing holds
/// the file. Unix renames atomically and never retries.
fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    const ATTEMPTS: u32 = 8;
    let mut attempt = 0;
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if cfg!(windows) && attempt + 1 < ATTEMPTS && is_transient_rename_error(&e) => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(1 << attempt.min(4)));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Windows `ERROR_ACCESS_DENIED` (5) or `ERROR_SHARING_VIOLATION` (32) on a
/// rename: both transient while another process or thread replaces or
/// opens the destination.
fn is_transient_rename_error(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(32)
}

/// Best-effort fsync of a directory so the rename itself is durable.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(handle) = File::open(dir) {
        let _ = handle.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
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

    /// A stamp watching no font directory (tests opt into one explicitly).
    fn stamp(build: &str) -> VersionStamp {
        VersionStamp::current(build.to_owned()).with_font_dirs(Vec::new())
    }

    #[test]
    fn the_cluster_class_ignores_order_and_repeats() {
        assert_eq!(key("ab"), key("ba"));
        assert_eq!(key("aab"), key("ab"));
        assert_ne!(key("ab"), key("abc"));
        assert_eq!(key("ab").digest(), key("ba").digest());
    }

    #[test]
    fn every_attribute_is_part_of_the_key_and_its_digest() {
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
        let mut script = base.clone();
        script.script = ScriptTag(*b"Cyrl");
        for other in [bold, italic, ja, mono, no_bitmaps, script] {
            let other_key = CacheKey::from_request(&other);
            assert_ne!(other_key, k, "{other:?}");
            assert_ne!(other_key.digest(), k.digest(), "{other:?}");
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
    fn the_digest_is_stable_across_processes_and_field_boundaries() {
        // Pinned: the digest is the on-disk key, so it must never depend on
        // the process, the platform, the toolchain or std's hasher.
        let pinned = CacheKey {
            class: "a".into(),
            script: ScriptTag(*b"Latn"),
            locale: Some("en".into()),
            weight: 400f32.to_bits(),
            slant: SlantKey::Normal,
            width: 1f32.to_bits(),
            families: vec![
                FamilyKey::Named("roboto".into()),
                FamilyKey::Generic(GenericKey::SansSerif),
            ],
            renderability: RenderCapabilities {
                outlines: true,
                colr: true,
                bitmap: true,
                hvgl_via_coretext: false,
            },
        };
        assert_eq!(pinned.digest(), 0xf6cb_9913_0951_8f64);
        // Length prefixes keep ("ab", "c") apart from ("a", "bc").
        let mut one = request("x");
        one.families = vec![
            FamilyName::Named("ab".into()),
            FamilyName::Named("c".into()),
        ];
        let mut two = request("x");
        two.families = vec![
            FamilyName::Named("a".into()),
            FamilyName::Named("bc".into()),
        ];
        assert_ne!(
            CacheKey::from_request(&one).digest(),
            CacheKey::from_request(&two).digest()
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

    #[test]
    fn a_digest_collision_with_a_known_key_reads_as_a_miss() {
        let mut cache = FallbackCache::new(4);
        cache.insert(key("a"), CachedAnswer::Miss);
        // Forge a collision: the slot for "a" now claims to hold "b".
        let digest = key("a").digest();
        cache.entries.get_mut(&digest).unwrap().key = Some(key("b"));
        assert_eq!(cache.get(&key("a")), None);
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
    fn hashed_keys_round_trip_and_no_plaintext_reaches_the_disk() {
        let dir = TempDir::new("persist-hashed");
        let font = dir.write("face.ttf", &FaceSpec::new("Face").chars("q").build());
        let file = dir.path().join(PERSIST_FILE_NAME);
        let mut secret = request("\u{4e2d}q");
        secret.locale = Some("x-secretlocale".into());
        secret.families = vec![FamilyName::Named("SecretFamily".into())];
        let secret_key = CacheKey::from_request(&secret);
        let mut miss = request("\u{0436}");
        miss.locale = Some("x-otherlocale".into());
        let miss_key = CacheKey::from_request(&miss);

        let mut cache = FallbackCache::new(16);
        cache.attach_persistent(file.clone(), stamp("b"));
        cache.insert(
            secret_key.clone(),
            CachedAnswer::Face(FaceSource::path(&font, 0)),
        );
        cache.insert(miss_key.clone(), CachedAnswer::Miss);
        cache.persist().unwrap();

        let text = std::fs::read_to_string(&file).unwrap();
        for plaintext in [
            "\u{4e2d}",
            "\\u4e2d",
            "\u{0436}",
            "\\u0436",
            "secretlocale",
            "otherlocale",
            "secretfamily",
            "SecretFamily",
            "\"class\"",
        ] {
            assert!(!text.contains(plaintext), "{plaintext:?} leaked: {text}");
        }
        let mut reread = FallbackCache::new(16);
        assert_eq!(
            reread.attach_persistent(file, stamp("b")),
            LoadOutcome::Loaded(2)
        );
        assert_eq!(
            reread.get(&secret_key),
            Some(CachedAnswer::Face(FaceSource::path(&font, 0)))
        );
        assert_eq!(reread.get(&miss_key), Some(CachedAnswer::Miss));
        assert_eq!(reread.get(&key("q")), None, "another key is not a hit");
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
    fn a_face_whose_file_is_not_stamped_discards_the_cache() {
        let dir = TempDir::new("persist-unstamped");
        let (file, _) = write_cache(&dir, stamp("build-1"));
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        json["files"] = serde_json::Value::Array(Vec::new());
        std::fs::write(&file, serde_json::to_vec(&json).unwrap()).unwrap();
        let mut cache = FallbackCache::new(16);
        assert_eq!(
            cache.attach_persistent(file, stamp("build-1")),
            LoadOutcome::Invalidated("unstamped face path")
        );
        assert_eq!(cache.len(), 0);
        assert!(cache.dirty, "the bad file is rewritten at the next flush");
    }

    #[test]
    fn a_font_directory_change_expires_misses_but_keeps_faces() {
        let dir = TempDir::new("persist-dirs");
        let fonts = dir.path().join("user-fonts");
        let watched = || stamp("b").with_font_dirs(vec![fonts.clone()]);
        let (file, font) = write_cache(&dir, watched());

        // Unchanged directories: the miss survives.
        let mut same = FallbackCache::new(16);
        assert_eq!(
            same.attach_persistent(file.clone(), watched()),
            LoadOutcome::Loaded(2)
        );
        assert_eq!(same.get(&key("ж")), Some(CachedAnswer::Miss));

        // A font directory appears (a user font install): the miss expires.
        std::fs::create_dir_all(&fonts).unwrap();
        let mut changed = FallbackCache::new(16);
        assert_eq!(
            changed.attach_persistent(file.clone(), watched()),
            LoadOutcome::Loaded(1)
        );
        assert_eq!(changed.stats().expired_misses, 1);
        assert_eq!(changed.get(&key("ж")), None, "the miss is asked again");
        assert_eq!(
            changed.get(&key("a")).map(|a| matches!(a, CachedAnswer::Face(s) if s.location == FaceLocation::Path(font.clone()))),
            Some(true),
            "faces are still validated by their own file stamps"
        );
        assert!(changed.dirty, "rewritten with the new directory stamps");
        changed.persist().unwrap();

        let mut after = FallbackCache::new(16);
        assert_eq!(
            after.attach_persistent(file, watched()),
            LoadOutcome::Loaded(1)
        );
        assert_eq!(after.stats().expired_misses, 0, "the new stamps match");
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
    fn the_face_fields_round_trip_and_an_older_format_file_is_discarded() {
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

        // Files written by the previous formats (plaintext keys, no dir stamps)
        // are discarded.
        assert_eq!(FORMAT_VERSION, 3);
        let text = std::fs::read_to_string(&file).unwrap();
        for previous in 1..FORMAT_VERSION {
            let old = text.replacen(
                &format!("\"format\":{FORMAT_VERSION}"),
                &format!("\"format\":{previous}"),
                1,
            );
            assert_ne!(text, old);
            std::fs::write(&file, old).unwrap();
            let mut stale = FallbackCache::new(16);
            assert_eq!(
                stale.attach_persistent(file.clone(), stamp("b")),
                LoadOutcome::Invalidated("format version")
            );
            assert_eq!(stale.len(), 0);
        }
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

    #[test]
    fn write_behind_flushes_after_enough_new_answers() {
        let dir = TempDir::new("write-behind-count");
        let file = dir.path().join(PERSIST_FILE_NAME);
        let mut cache = FallbackCache::new(64);
        cache.attach_persistent(file.clone(), stamp("b"));
        cache.set_write_behind(3, Duration::from_secs(3600));
        cache.insert(key("a"), CachedAnswer::Miss);
        cache.insert(key("b"), CachedAnswer::Miss);
        assert!(!file.exists(), "below the threshold nothing is written");
        cache.insert(key("c"), CachedAnswer::Miss);
        assert!(file.exists(), "the third answer writes behind");
        assert_eq!(cache.stats().writes, 1);
        assert!(!cache.is_dirty());
        cache.insert(key("d"), CachedAnswer::Miss);
        assert_eq!(cache.stats().writes, 1, "the counter restarted");
        assert_eq!(WRITE_BEHIND_ENTRIES, 32);
    }

    #[test]
    fn write_behind_flushes_a_dirty_cache_after_the_idle_interval() {
        let dir = TempDir::new("write-behind-idle");
        let file = dir.path().join(PERSIST_FILE_NAME);
        let mut cache = FallbackCache::new(64);
        cache.attach_persistent(file.clone(), stamp("b"));
        cache.set_write_behind(1000, Duration::from_millis(250));
        cache.insert(key("a"), CachedAnswer::Miss);
        assert_eq!(cache.get(&key("a")), Some(CachedAnswer::Miss));
        assert!(!file.exists(), "not idle yet");
        std::thread::sleep(Duration::from_millis(350));
        assert_eq!(cache.get(&key("a")), Some(CachedAnswer::Miss));
        assert!(file.exists(), "the next lookup after the interval writes");
        assert!(!cache.is_dirty());
    }

    #[test]
    fn a_detached_cache_never_arms_the_triggers() {
        let mut cache = FallbackCache::new(64);
        cache.set_write_behind(1, Duration::ZERO);
        cache.insert(key("a"), CachedAnswer::Miss);
        assert_eq!(cache.get(&key("a")), Some(CachedAnswer::Miss));
        assert_eq!(cache.last_change, None);
        assert_eq!(cache.stats().writes, 0);
    }

    #[test]
    fn a_lazy_path_is_resolved_at_the_first_miss_or_flush() {
        use std::sync::{Arc, Mutex};
        let dir = TempDir::new("lazy-path");
        let (file, _) = write_cache(&dir, stamp("b"));
        let slot: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
        let asks = Arc::new(AtomicU64::new(0));
        let resolver = {
            let (slot, asks) = (Arc::clone(&slot), Arc::clone(&asks));
            PathResolver::new(move || {
                asks.fetch_add(1, Ordering::Relaxed);
                slot.lock().unwrap().clone()
            })
        };
        let mut cache = FallbackCache::new(16);
        cache.attach_lazy(resolver, stamp("b"));
        assert_eq!(asks.load(Ordering::Relaxed), 0, "nothing resolved yet");
        assert!(cache.is_persistent() && cache.attached_path().is_none());

        // Not resolvable yet (Android before the shell's platform init).
        assert_eq!(cache.get(&key("a")), None);
        cache.insert(key("new"), CachedAnswer::Miss);
        cache.persist().unwrap();
        assert!(cache.is_dirty(), "nothing could be written");
        assert_eq!(asks.load(Ordering::Relaxed), 2);

        // Resolvable now: the next miss attaches and loads the file.
        *slot.lock().unwrap() = Some(file.clone());
        assert!(matches!(cache.get(&key("a")), Some(CachedAnswer::Face(_))));
        assert_eq!(cache.attached_path(), Some(file.as_path()));
        assert_eq!(cache.get(&key("new")), Some(CachedAnswer::Miss));
        cache.persist().unwrap();
        let mut reread = FallbackCache::new(16);
        assert_eq!(
            reread.attach_persistent(file, stamp("b")),
            LoadOutcome::Loaded(3),
            "the pre-attach answer merged with the loaded ones"
        );
    }

    #[test]
    fn a_lazy_path_attaches_at_flush_when_nothing_missed() {
        let dir = TempDir::new("lazy-flush");
        let file = dir.path().join(PERSIST_FILE_NAME);
        let mut cache = FallbackCache::new(16);
        cache.attach_lazy(PathResolver::fixed(file.clone()), stamp("b"));
        cache.persist().unwrap();
        assert!(
            cache.attached_path().is_none(),
            "a clean flush resolves nothing"
        );
        cache.insert(key("a"), CachedAnswer::Miss);
        cache.persist().unwrap();
        assert_eq!(cache.attached_path(), Some(file.as_path()));
        assert!(file.exists());
    }

    #[test]
    fn loaded_entries_never_evict_fresher_ones() {
        let dir = TempDir::new("merge-older");
        let file = dir.path().join(PERSIST_FILE_NAME);
        let mut writer = FallbackCache::new(16);
        writer.attach_persistent(file.clone(), stamp("b"));
        for c in ["x", "y", "z"] {
            writer.insert(key(c), CachedAnswer::Miss);
        }
        writer.persist().unwrap();

        let mut cache = FallbackCache::new(3);
        cache.attach_lazy(PathResolver::fixed(file), stamp("b"));
        cache.insert(key("fresh"), CachedAnswer::Miss);
        cache.persist().unwrap();
        assert_eq!(cache.len(), 3);
        assert_eq!(cache.get(&key("fresh")), Some(CachedAnswer::Miss));
        assert_eq!(cache.get(&key("x")), None, "the oldest loaded entry went");
    }

    #[test]
    fn temp_names_are_unique_per_call_and_carry_the_pid() {
        let path = Path::new("/tmp/frust/font-fallback.json");
        let (a, b) = (temp_name(path), temp_name(path));
        assert_ne!(a, b);
        let a = a.to_string_lossy();
        assert!(a.starts_with(".font-fallback.json."), "{a}");
        assert!(a.contains(&format!(".{}.", std::process::id())), "{a}");
        assert!(a.ends_with(".tmp"), "{a}");
    }

    #[test]
    fn racing_writers_leave_one_complete_file_and_no_temp_files() {
        let dir = TempDir::new("racing-writers");
        let file = dir.path().join("cache").join(PERSIST_FILE_NAME);
        const WRITERS: usize = 8;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(WRITERS));
        let handles: Vec<_> = (0..WRITERS)
            .map(|w| {
                let (file, barrier) = (file.clone(), std::sync::Arc::clone(&barrier));
                std::thread::spawn(move || {
                    let mut cache = FallbackCache::new(256);
                    cache.attach_persistent(file, stamp("b"));
                    for i in 0..=w {
                        cache.insert(key(&format!("w{w}-{i}")), CachedAnswer::Miss);
                    }
                    barrier.wait();
                    for _ in 0..5 {
                        cache.dirty = true;
                        cache.persist().expect("every racing write succeeds");
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        let mut reread = FallbackCache::new(256);
        let LoadOutcome::Loaded(n) = reread.attach_persistent(file.clone(), stamp("b")) else {
            panic!("the surviving file is complete and valid");
        };
        assert!((1..=WRITERS).contains(&n), "one writer's whole copy: {n}");
        let leftovers: Vec<_> = std::fs::read_dir(file.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|name| name != PERSIST_FILE_NAME)
            .collect();
        assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");
    }
}
