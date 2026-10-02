//! Library test helpers: files on disk, scans and browse lookups.

use std::path::Path;

use axum::http::StatusCode;
use netvideo_server::scanner::ScanStats;

use super::{Harness, json};

/// A scanner configured with a program that does not exist, so videos are
/// indexed from the filesystem alone.
pub fn unprobed() -> Harness {
    Harness::with_ffprobe(Path::new("/nonexistent/ffprobe"))
}

pub fn write(harness: &Harness, rel: &str, bytes: &[u8]) {
    let path = harness.library.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

pub async fn scan(harness: &Harness) -> ScanStats {
    harness
        .state
        .scan
        .scan_once(&harness.state.db)
        .await
        .unwrap()
        .expect("no scan running")
}

pub async fn get(harness: &Harness, uri: &str, token: &str) -> (StatusCode, serde_json::Value) {
    let (status, _, body) = harness.request(harness.authed("GET", uri, token)).await;
    let value = if body.is_empty() {
        serde_json::Value::Null
    } else {
        json(&body)
    };
    (status, value)
}

pub async fn root_id(harness: &Harness, token: &str) -> String {
    let (status, roots) = get(harness, "/api/v1/roots", token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(roots.as_array().unwrap().len(), 1);
    assert_eq!(roots[0]["name"], "library");
    roots[0]["id"].as_str().unwrap().to_string()
}

/// Finds the id of the video named `name` directly under the root.
pub async fn video_id(harness: &Harness, token: &str, name: &str) -> String {
    let root = root_id(harness, token).await;
    let (_, page) = get(harness, &format!("/api/v1/folders/{root}"), token).await;
    page["videos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|video| video["name"] == name)
        .expect("video listed")["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Sets `rel`'s mtime to a whole second plus `millis`, so tests can make two
/// changes that fall within the same Unix second.
pub fn set_mtime(harness: &Harness, rel: &str, millis: u64) {
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_700_000_000_000 + millis);
    std::fs::File::options()
        .write(true)
        .open(harness.library.join(rel))
        .unwrap()
        .set_modified(time)
        .unwrap();
}
