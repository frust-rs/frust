//! The vision catalog: shape checks offline, one ignored live drift test.

#![cfg(feature = "catalog")]

use std::collections::HashSet;

use frust_models::{ModelSource, ModelStore};
use frust_onnx::catalog::{CatalogSource, VISION_CATALOG, VisionTask, by_key, for_task};

#[test]
fn keys_are_unique_and_rows_are_well_formed() {
    let mut keys = HashSet::new();
    let mut ids = HashSet::new();
    for m in VISION_CATALOG {
        assert!(keys.insert(m.key), "duplicate key {}", m.key);
        assert!(ids.insert((m.id)()), "duplicate id for {}", m.key);
        assert_eq!(m.sha256.len(), 64, "{}", m.key);
        assert!(
            m.sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "{} digest is not lowercase hex",
            m.key
        );
        assert!(m.size_bytes > 0 && m.input.0 > 0 && m.input.1 > 0);
        assert!(!m.licence.is_empty() && !m.display.is_empty());
        assert!(!m.licence.contains("NC"), "{} is non-commercial", m.key);
    }
    assert_eq!(VISION_CATALOG.len(), 3);
}

#[test]
fn lookup_by_key_and_task() {
    assert_eq!(by_key("u2netp").unwrap().task, VisionTask::Segmentation);
    assert!(by_key("missing").is_none());
    let faces: Vec<_> = for_task(VisionTask::FaceDetection).map(|m| m.key).collect();
    assert_eq!(faces, ["yunet"]);
    assert_eq!(for_task(VisionTask::Depth).count(), 1);
}

#[test]
fn download_request_carries_the_pin() {
    for m in VISION_CATALOG {
        let req = m.download_request();
        assert!(req.verified, "{}", m.key);
        assert_eq!(req.licence.as_deref(), Some(m.licence));
        assert_eq!(req.items.len(), 1);
        let item = &req.items[0];
        assert_eq!(item.id, (m.id)());
        assert_eq!(item.size, Some(m.size_bytes));
        assert_eq!(item.sha256.as_deref(), Some(m.sha256));
        assert!(item.url.starts_with("https://"));
        match m.source {
            CatalogSource::Url(url) => {
                assert_eq!(item.url, url);
                assert!(matches!(item.source, ModelSource::Url { .. }));
            }
            CatalogSource::Hub {
                repo,
                revision,
                path,
            } => {
                assert_eq!(revision.len(), 40, "Hub rows pin a commit");
                assert_eq!(
                    item.url,
                    format!("https://huggingface.co/{repo}/resolve/{revision}/{path}")
                );
                assert_eq!(
                    item.source,
                    ModelSource::HuggingFace {
                        repo: repo.to_owned(),
                        revision: revision.to_owned()
                    }
                );
            }
        }
    }
}

/// Drift detector: downloads every row (about 32 MB); the downloader checks size and SHA-256 against the pin
/// before storing, and the manifest must record them.
#[test]
#[ignore = "downloads the catalog files from the network"]
fn live_rows_match_their_pins() {
    let dir = std::env::temp_dir().join(format!("frust-onnx-catalog-{}", std::process::id()));
    let store = ModelStore::with_root(dir.clone()).unwrap();
    let hub = frust_models::HubClient::new().unwrap();
    for m in VISION_CATALOG {
        let handle = frust_models::Downloader::start(
            &hub,
            &store,
            m.download_request(),
            std::sync::Arc::new(|_| {}),
        )
        .unwrap();
        let models = handle.wait().unwrap_or_else(|e| panic!("{}: {e}", m.key));
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].size_bytes, m.size_bytes, "{}", m.key);
        assert_eq!(
            std::fs::metadata(&models[0].path).unwrap().len(),
            m.size_bytes
        );
        let manifest = models[0].manifest.as_ref().expect("manifest is written");
        assert!(manifest.verified, "{}", m.key);
        assert_eq!(manifest.sha256.as_deref(), Some(m.sha256), "{}", m.key);
        assert_eq!(manifest.licence.as_deref(), Some(m.licence));
    }
    let _ = std::fs::remove_dir_all(dir);
}
