//! End-to-end tests for scanning, browsing and direct play.

mod common;

use std::path::Path;
use std::process::Command;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use common::{Harness, json};
use netvideo_server::scanner::ScanStats;

/// A scanner configured with a program that does not exist, so videos are
/// indexed from the filesystem alone.
fn unprobed() -> Harness {
    Harness::with_ffprobe(Path::new("/nonexistent/ffprobe"))
}

fn write(harness: &Harness, rel: &str, bytes: &[u8]) {
    let path = harness.library.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

async fn scan(harness: &Harness) -> ScanStats {
    harness
        .state
        .scan
        .scan_once(&harness.state.db)
        .await
        .unwrap()
        .expect("no scan running")
}

async fn get(harness: &Harness, uri: &str, token: &str) -> (StatusCode, serde_json::Value) {
    let (status, _, body) = harness.request(harness.authed("GET", uri, token)).await;
    let value = if body.is_empty() {
        serde_json::Value::Null
    } else {
        json(&body)
    };
    (status, value)
}

async fn root_id(harness: &Harness, token: &str) -> String {
    let (status, roots) = get(harness, "/api/v1/roots", token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(roots.as_array().unwrap().len(), 1);
    assert_eq!(roots[0]["name"], "library");
    roots[0]["id"].as_str().unwrap().to_string()
}

/// Finds the id of the video named `name` directly under the root.
async fn video_id(harness: &Harness, token: &str, name: &str) -> String {
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

#[tokio::test]
async fn scan_mirrors_the_filesystem() {
    let harness = unprobed();
    write(&harness, "Films/Heat (1995).mkv", b"heat");
    write(&harness, "Films/notes.txt", b"not a video");
    write(&harness, "Shows/S01/e01.mp4", b"e01");
    write(&harness, ".hidden/secret.mkv", b"hidden");
    write(&harness, "clip.webm", b"clip");

    let stats = scan(&harness).await;
    assert_eq!(stats.videos_found, 3);
    assert_eq!(stats.written, 3);
    assert!(!stats.partial);

    let viewer = harness.pair_viewer().await;
    let root = root_id(&harness, &viewer.token).await;
    let (status, page) = get(&harness, &format!("/api/v1/folders/{root}"), &viewer.token).await;
    assert_eq!(status, StatusCode::OK);
    let folders: Vec<_> = page["folders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].clone())
        .collect();
    assert_eq!(folders, ["Films", "Shows"]);
    assert_eq!(page["videos"][0]["name"], "clip.webm");
    assert_eq!(page["videos"].as_array().unwrap().len(), 1);
    assert!(page["next_cursor"].is_null());

    let shows = page["folders"][1]["id"].as_str().unwrap();
    let (_, shows_page) = get(&harness, &format!("/api/v1/folders/{shows}"), &viewer.token).await;
    let season = shows_page["folders"][0]["id"].as_str().unwrap();
    let (_, season_page) = get(
        &harness,
        &format!("/api/v1/folders/{season}"),
        &viewer.token,
    )
    .await;
    assert_eq!(season_page["videos"][0]["name"], "e01.mp4");
    let crumbs: Vec<_> = season_page["path"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].clone())
        .collect();
    assert_eq!(crumbs, ["library", "Shows"]);
    assert_eq!(season_page["folder"]["name"], "S01");
}

#[tokio::test]
async fn folder_pages_are_paginated_across_folders_and_videos() {
    let harness = unprobed();
    for name in ["a", "b"] {
        write(&harness, &format!("{name}/x.mkv"), b"x");
    }
    for name in ["c.mkv", "d.mkv", "e.mkv"] {
        write(&harness, name, b"v");
    }
    scan(&harness).await;
    let admin = harness.pair_admin().await;
    let root = root_id(&harness, &admin.token).await;

    let mut names = Vec::new();
    let mut uri = format!("/api/v1/folders/{root}?limit=2");
    loop {
        let (status, page) = get(&harness, &uri, &admin.token).await;
        assert_eq!(status, StatusCode::OK);
        for entry in page["folders"]
            .as_array()
            .unwrap()
            .iter()
            .chain(page["videos"].as_array().unwrap())
        {
            names.push(entry["name"].as_str().unwrap().to_string());
        }
        match page["next_cursor"].as_str() {
            Some(cursor) => uri = format!("/api/v1/folders/{root}?limit=2&cursor={cursor}"),
            None => break,
        }
    }
    assert_eq!(names, ["a", "b", "c.mkv", "d.mkv", "e.mkv"]);

    for bad in ["limit=0", "limit=1001", "cursor=nope", "surprise=1"] {
        let (status, _) = get(
            &harness,
            &format!("/api/v1/folders/{root}?{bad}"),
            &admin.token,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let (status, _) = get(&harness, "/api/v1/folders/unknown", &admin.token).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn browsing_requires_a_token() {
    let harness = unprobed();
    for uri in [
        "/api/v1/roots",
        "/api/v1/folders/x",
        "/api/v1/videos/x",
        "/api/v1/videos/x/file",
    ] {
        let request = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let (status, _, _) = harness.request(request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
    }
}

#[tokio::test]
async fn video_detail_reports_unprobed_files() {
    let harness = unprobed();
    write(&harness, "movie.mkv", b"0123456789");
    let stats = scan(&harness).await;
    assert_eq!(
        stats.probe_failures, 0,
        "a missing ffprobe is not a per-file failure"
    );
    let viewer = harness.pair_viewer().await;
    let id = video_id(&harness, &viewer.token, "movie.mkv").await;
    let (status, detail) = get(&harness, &format!("/api/v1/videos/{id}"), &viewer.token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["size"], 10);
    assert_eq!(detail["mime"], "video/x-matroska");
    assert_eq!(detail["probed"], false);
    assert!(detail["streams"].as_array().unwrap().is_empty());
    let (status, _) = get(&harness, "/api/v1/videos/unknown", &viewer.token).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn direct_play_serves_ranges_and_etags() {
    let harness = unprobed();
    write(&harness, "movie.mp4", b"0123456789");
    scan(&harness).await;
    let viewer = harness.pair_viewer().await;
    let id = video_id(&harness, &viewer.token, "movie.mp4").await;
    let uri = format!("/api/v1/videos/{id}/file");

    let (status, headers, body) = harness
        .request(harness.authed("GET", &uri, &viewer.token))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&body[..], b"0123456789");
    assert_eq!(headers[header::CONTENT_TYPE], "video/mp4");
    assert_eq!(headers[header::ACCEPT_RANGES], "bytes");
    let etag = headers[header::ETAG].to_str().unwrap().to_string();

    let mut ranged = harness.authed("GET", &uri, &viewer.token);
    ranged
        .headers_mut()
        .insert(header::RANGE, "bytes=2-5".parse().unwrap());
    let (status, headers, body) = harness.request(ranged).await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(&body[..], b"2345");
    assert_eq!(headers[header::CONTENT_RANGE], "bytes 2-5/10");

    let mut beyond = harness.authed("GET", &uri, &viewer.token);
    beyond
        .headers_mut()
        .insert(header::RANGE, "bytes=50-".parse().unwrap());
    let (status, _, _) = harness.request(beyond).await;
    assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);

    let mut cached = harness.authed("GET", &uri, &viewer.token);
    cached
        .headers_mut()
        .insert(header::IF_NONE_MATCH, etag.parse().unwrap());
    let (status, _, body) = harness.request(cached).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    assert!(body.is_empty());
}

#[tokio::test]
async fn stored_paths_cannot_escape_the_library() {
    let harness = unprobed();
    write(&harness, "movie.mkv", b"movie");
    std::fs::write(
        harness.library.parent().unwrap().join("secret.mkv"),
        b"secret",
    )
    .unwrap();
    std::os::unix::fs::symlink(
        harness.library.parent().unwrap().join("secret.mkv"),
        harness.library.join("link.mkv"),
    )
    .unwrap();
    let stats = scan(&harness).await;
    assert_eq!(stats.videos_found, 1, "symlinks are not followed");

    let viewer = harness.pair_viewer().await;
    let id = video_id(&harness, &viewer.token, "movie.mkv").await;
    for evil in ["../secret.mkv", "link.mkv", "/etc/passwd"] {
        harness
            .state
            .db
            .conn()
            .unwrap()
            .execute("UPDATE videos SET rel_path = ?1 WHERE id = ?2", [evil, &id])
            .unwrap();
        let (status, _, body) = harness
            .request(harness.authed("GET", &format!("/api/v1/videos/{id}/file"), &viewer.token))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{evil}");
        assert!(!body.windows(6).any(|w| w == b"secret"));
    }
}

#[tokio::test]
async fn rescans_update_and_prune() {
    let harness = unprobed();
    write(&harness, "keep.mkv", b"keep");
    write(&harness, "gone/old.mkv", b"old");
    scan(&harness).await;
    let viewer = harness.pair_viewer().await;
    let id = video_id(&harness, &viewer.token, "keep.mkv").await;

    std::fs::remove_dir_all(harness.library.join("gone")).unwrap();
    write(&harness, "keep.mkv", b"keep but longer");
    let stats = scan(&harness).await;
    assert_eq!(stats.removed, 1);

    let root = root_id(&harness, &viewer.token).await;
    let (_, page) = get(&harness, &format!("/api/v1/folders/{root}"), &viewer.token).await;
    assert!(
        page["folders"].as_array().unwrap().is_empty(),
        "empty folder pruned"
    );
    assert_eq!(page["videos"][0]["id"], id.as_str(), "ids survive rescans");
    assert_eq!(page["videos"][0]["size"], 15);
}

#[tokio::test]
async fn only_admins_can_trigger_a_scan() {
    let harness = unprobed();
    let viewer = harness.pair_viewer().await;
    let (status, _, _) = harness
        .request(harness.authed("POST", "/api/v1/library/scan", &viewer.token))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let admin = harness.pair_admin().await;
    let (status, _, _) = harness
        .request(harness.authed("POST", "/api/v1/library/scan", &admin.token))
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
}

/// Generates a real one-second clip with ffmpeg and checks ffprobe metadata.
/// Skipped when ffmpeg or ffprobe is not installed.
#[tokio::test]
async fn real_ffprobe_metadata_is_stored() {
    let available = |tool: &str| {
        Command::new(tool)
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success())
    };
    if !available("ffmpeg") || !available("ffprobe") {
        eprintln!("skipping: ffmpeg/ffprobe not installed");
        return;
    }
    let harness = Harness::with_ffprobe(Path::new("ffprobe"));
    let out = harness.library.join("clip.mkv");
    let status = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240:rate=25:duration=1",
        ])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=1"])
        .args([
            "-c:v",
            "mpeg4",
            "-c:a",
            "mp2",
            "-metadata:s:a:0",
            "language=fra",
        ])
        .arg(&out)
        .status()
        .unwrap();
    assert!(status.success());

    let stats = scan(&harness).await;
    assert_eq!((stats.videos_found, stats.probe_failures), (1, 0));
    let viewer = harness.pair_viewer().await;
    let id = video_id(&harness, &viewer.token, "clip.mkv").await;
    let (_, detail) = get(&harness, &format!("/api/v1/videos/{id}"), &viewer.token).await;
    assert_eq!(detail["probed"], true);
    assert_eq!(detail["container"], "matroska,webm");
    let duration = detail["duration_ms"].as_i64().unwrap();
    assert!((900..=1100).contains(&duration), "{duration}");
    let streams = detail["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 2);
    assert_eq!(streams[0]["kind"], "video");
    assert_eq!(streams[0]["codec"], "mpeg4");
    assert_eq!(streams[0]["width"], 320);
    assert_eq!(streams[1]["kind"], "audio");
    assert_eq!(streams[1]["language"], "fra");

    // An unchanged, probed file is not probed or written again.
    assert_eq!(scan(&harness).await.written, 0);
}

#[tokio::test]
async fn a_triggered_scan_holds_the_lock_until_it_runs() {
    let harness = unprobed();
    write(&harness, "movie.mkv", b"movie");
    assert!(harness.state.scan.trigger(harness.state.db.clone()));
    // Nothing has yielded to the spawned task yet, yet the lock is taken.
    assert!(!harness.state.scan.trigger(harness.state.db.clone()));
    let raced = harness
        .state
        .scan
        .scan_once(&harness.state.db)
        .await
        .unwrap();
    assert!(raced.is_none(), "the triggered scan owns the lock");
}

#[tokio::test]
async fn etags_follow_the_file_not_the_last_scan() {
    let harness = unprobed();
    write(&harness, "movie.mkv", b"version-1");
    scan(&harness).await;
    let viewer = harness.pair_viewer().await;
    let id = video_id(&harness, &viewer.token, "movie.mkv").await;
    let uri = format!("/api/v1/videos/{id}/file");
    let (_, headers, _) = harness
        .request(harness.authed("GET", &uri, &viewer.token))
        .await;
    let old_etag = headers[header::ETAG].clone();

    // Same size, new content and mtime, and no rescan.
    let path = harness.library.join("movie.mkv");
    std::fs::write(&path, b"version-2").unwrap();
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(later)
        .unwrap();

    let mut cached = harness.authed("GET", &uri, &viewer.token);
    cached.headers_mut().insert(header::IF_NONE_MATCH, old_etag);
    let (status, _, body) = harness.request(cached).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&body[..], b"version-2");
}
