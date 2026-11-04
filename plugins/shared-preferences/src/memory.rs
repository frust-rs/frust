//! The process-local [`Backend`] behind [`SharedPreferences::in_memory`].
//!
//! A plain `Mutex`-guarded map. It owns its storage exclusively, so unlike
//! the OS-shared stores it needs no key prefix, and nothing is ever written
//! to disk or to a platform store. It compiles on every target.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::{Backend, PrefValue, PrefsError};

#[derive(Default)]
pub(crate) struct MemoryStore {
    map: Mutex<HashMap<String, PrefValue>>,
}

impl MemoryStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, PrefValue>> {
        self.map.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Backend for MemoryStore {
    fn get(&self, key: &str) -> Option<PrefValue> {
        self.map().get(key).cloned()
    }

    fn set(&self, key: &str, value: PrefValue) -> Result<(), PrefsError> {
        self.map().insert(key.to_string(), value);
        Ok(())
    }

    fn remove(&self, key: &str) -> Result<(), PrefsError> {
        self.map().remove(key);
        Ok(())
    }

    fn clear(&self) -> Result<(), PrefsError> {
        self.map().clear();
        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        self.map().keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SharedPreferences;

    #[test]
    fn conformance() {
        crate::conformance::run_conformance_suite(&MemoryStore::new());
    }

    #[test]
    fn separate_stores_are_isolated_and_clones_share() {
        let a = SharedPreferences::in_memory();
        let b = SharedPreferences::in_memory();
        a.set_string("k", "v").unwrap();
        assert_eq!(b.get_string("k"), None);
        assert!(!b.contains("k"));

        let a2 = a.clone();
        assert_eq!(a2.get_string("k").as_deref(), Some("v"));
        a2.set_bool("flag", true).unwrap();
        assert_eq!(a.get_bool("flag"), Some(true));
        a.clear().unwrap();
        assert!(a2.keys().is_empty());
    }
}
