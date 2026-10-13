//! Persistent raw-bytes cache: sha256-named files, a JSON index, LRU byte cap.
//!
//! The URL is only ever hashed; it is never written to disk.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const INDEX_FILE: &str = "index.json";
const DATA_EXT: &str = "img";
const FLUSH_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct Entry {
    size: u64,
    last_access_unix_ms: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    entries: HashMap<String, Entry>,
}

/// Disk cache occupancy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskStats {
    /// Cached files.
    pub entries: usize,
    /// Bytes across cached files.
    pub total_bytes: u64,
    /// The configured byte cap.
    pub cap_bytes: u64,
}

struct State {
    entries: HashMap<String, Entry>,
    total: u64,
    dirty: bool,
    last_flush: Instant,
    /// Highest access stamp handed out; stamps are strictly increasing so
    /// LRU order is exact even within one millisecond.
    clock: u64,
}

impl State {
    fn stamp(&mut self) -> u64 {
        let now = unix_ms(SystemTime::now());
        self.clock = now.max(self.clock + 1);
        self.clock
    }
}

/// An LRU-capped directory of cached response bodies. `Send + Sync`.
pub struct DiskCache {
    dir: PathBuf,
    cap: u64,
    state: Mutex<State>,
}

fn unix_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn hex_key(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn data_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.{DATA_EXT}"))
}

impl DiskCache {
    /// Open (creating) `dir`, reconciling the index with the files present:
    /// missing files are dropped, unindexed files adopted with their mtime,
    /// stale temp files removed. A corrupt index is rebuilt from the files.
    pub fn open(dir: PathBuf, cap_bytes: u64) -> io::Result<Self> {
        fs::create_dir_all(&dir)?;
        let indexed: Index = fs::read(dir.join(INDEX_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut entries = HashMap::new();
        let mut clock = 0;
        for item in fs::read_dir(&dir)? {
            let Ok(item) = item else { continue };
            let path = item.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.contains(".tmp.") {
                let _ = fs::remove_file(&path);
                continue;
            }
            let Some(key) = name.strip_suffix(&format!(".{DATA_EXT}")) else {
                continue;
            };
            let Ok(meta) = item.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let last = indexed.entries.get(key).map_or_else(
                || meta.modified().map(unix_ms).unwrap_or(0),
                |e| e.last_access_unix_ms,
            );
            clock = clock.max(last);
            entries.insert(
                key.to_string(),
                Entry {
                    size: meta.len(),
                    last_access_unix_ms: last,
                },
            );
        }
        let total = entries.values().map(|e| e.size).sum();
        let cache = Self {
            dir,
            cap: cap_bytes,
            state: Mutex::new(State {
                entries,
                total,
                dirty: true,
                last_flush: Instant::now(),
                clock,
            }),
        };
        {
            let mut st = cache.lock();
            cache.evict(&mut st, None);
            let _ = cache.flush(&mut st);
        }
        Ok(cache)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn flush(&self, st: &mut State) -> io::Result<()> {
        let index = Index {
            entries: st.entries.clone(),
        };
        let bytes = serde_json::to_vec(&index).map_err(io::Error::other)?;
        frust_paths::atomic_write(&self.dir.join(INDEX_FILE), &bytes)?;
        st.dirty = false;
        st.last_flush = Instant::now();
        Ok(())
    }

    fn flush_if_due(&self, st: &mut State) {
        if st.dirty && st.last_flush.elapsed() >= FLUSH_INTERVAL {
            let _ = self.flush(st);
        }
    }

    /// Evict least-recently-used files until under the cap, sparing `keep`.
    /// Flushes the index when anything was evicted.
    fn evict(&self, st: &mut State, keep: Option<&str>) {
        let mut evicted = false;
        while st.total > self.cap {
            let victim = st
                .entries
                .iter()
                .filter(|(k, _)| Some(k.as_str()) != keep)
                .min_by_key(|(_, e)| e.last_access_unix_ms)
                .map(|(k, _)| k.clone());
            let Some(key) = victim else { break };
            if let Some(e) = st.entries.remove(&key) {
                st.total = st.total.saturating_sub(e.size);
            }
            let _ = fs::remove_file(data_path(&self.dir, &key));
            evicted = true;
        }
        if evicted {
            st.dirty = true;
            let _ = self.flush(st);
        }
    }

    /// The cached body for `url`, refreshing its recency.
    pub fn get(&self, url: &str) -> Option<Vec<u8>> {
        let key = hex_key(url);
        if !self.lock().entries.contains_key(&key) {
            return None;
        }
        let read = fs::read(data_path(&self.dir, &key));
        let mut st = self.lock();
        match read {
            Ok(bytes) => {
                let stamp = st.stamp();
                if let Some(e) = st.entries.get_mut(&key) {
                    e.last_access_unix_ms = stamp;
                    st.dirty = true;
                }
                self.flush_if_due(&mut st);
                Some(bytes)
            }
            Err(_) => {
                if let Some(e) = st.entries.remove(&key) {
                    st.total = st.total.saturating_sub(e.size);
                    st.dirty = true;
                }
                None
            }
        }
    }

    /// Store `bytes` for `url`, evicting LRU files to stay under the cap.
    /// Bodies larger than the cap are not stored.
    pub fn put(&self, url: &str, bytes: &[u8]) -> io::Result<()> {
        let size = bytes.len() as u64;
        if size > self.cap {
            return Ok(());
        }
        let key = hex_key(url);
        let mut st = self.lock();
        frust_paths::atomic_write(&data_path(&self.dir, &key), bytes)?;
        let stamp = st.stamp();
        if let Some(old) = st.entries.insert(
            key.clone(),
            Entry {
                size,
                last_access_unix_ms: stamp,
            },
        ) {
            st.total = st.total.saturating_sub(old.size);
        }
        st.total += size;
        st.dirty = true;
        self.evict(&mut st, Some(&key));
        self.flush_if_due(&mut st);
        Ok(())
    }

    /// Remove every cached file.
    pub fn clear(&self) -> io::Result<()> {
        let mut st = self.lock();
        let keys: Vec<String> = st.entries.drain().map(|(k, _)| k).collect();
        st.total = 0;
        for key in keys {
            let _ = fs::remove_file(data_path(&self.dir, &key));
        }
        st.dirty = true;
        self.flush(&mut st)
    }

    /// Current occupancy.
    pub fn stats(&self) -> DiskStats {
        let st = self.lock();
        DiskStats {
            entries: st.entries.len(),
            total_bytes: st.total,
            cap_bytes: self.cap,
        }
    }
}

impl Drop for DiskCache {
    fn drop(&mut self) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.dirty {
            let _ = self.flush(&mut st);
        }
    }
}
