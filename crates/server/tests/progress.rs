//! End-to-end tests for per-device playback progress.

mod common;

use axum::http::StatusCode;
use common::Harness;
use common::library::*;

async fn put_progress(
    harness: &Harness,
    token: &str,
    video: &str,
    body: serde_json::Value,
) -> StatusCode {
    let uri = format!("/api/v1/videos/{video}/progress");
    let request = harness.authed_json("PUT", &uri, token, &body);
    harness.request(request).await.0
}

#[tokio::test]
async fn progress_is_saved_per_device() {
    let harness = unprobed();
    write(&harness, "movie.mkv", b"movie");
    scan(&harness).await;
    let phone = harness.pair_viewer().await;
    let tablet = harness.pair_viewer().await;
    let video = video_id(&harness, &phone.token, "movie.mkv").await;

    let body = serde_json::json!({ "position_ms": 61_500 });
    let status = put_progress(&harness, &phone.token, &video, body).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, detail) = get(&harness, &format!("/api/v1/videos/{video}"), &phone.token).await;
    assert_eq!(detail["progress"]["position_ms"], 61_500);
    assert_eq!(detail["progress"]["watched"], false);
    let (_, other) = get(&harness, &format!("/api/v1/videos/{video}"), &tablet.token).await;
    assert!(other["progress"].is_null(), "progress is per device");

    let body = serde_json::json!({ "position_ms": 0, "watched": true });
    put_progress(&harness, &phone.token, &video, body).await;
    let root = root_id(&harness, &phone.token).await;
    let (_, page) = get(&harness, &format!("/api/v1/folders/{root}"), &phone.token).await;
    assert_eq!(page["videos"][0]["progress"]["watched"], true);
    let (_, page) = get(&harness, &format!("/api/v1/folders/{root}"), &tablet.token).await;
    assert!(page["videos"][0]["progress"].is_null());
}

#[tokio::test]
async fn bad_progress_updates_are_rejected() {
    let harness = unprobed();
    write(&harness, "movie.mkv", b"movie");
    scan(&harness).await;
    let viewer = harness.pair_viewer().await;
    let video = video_id(&harness, &viewer.token, "movie.mkv").await;

    let unknown = serde_json::json!({ "position_ms": 1 });
    let status = put_progress(&harness, &viewer.token, "missing", unknown).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    for body in [
        serde_json::json!({ "position_ms": -1 }),
        serde_json::json!({ "position_ms": 1_000_000_000_000_i64 }),
    ] {
        let status = put_progress(&harness, &viewer.token, &video, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let extra = serde_json::json!({ "position_ms": 1, "device": "other" });
    let status = put_progress(&harness, &viewer.token, &video, extra).await;
    assert!(status.is_client_error());
}

#[tokio::test]
async fn progress_goes_with_its_video() {
    let harness = unprobed();
    write(&harness, "movie.mkv", b"movie");
    write(&harness, "other.mkv", b"other");
    scan(&harness).await;
    let viewer = harness.pair_viewer().await;
    let video = video_id(&harness, &viewer.token, "movie.mkv").await;
    let body = serde_json::json!({ "position_ms": 5_000 });
    put_progress(&harness, &viewer.token, &video, body).await;

    std::fs::remove_file(harness.library.join("movie.mkv")).unwrap();
    scan(&harness).await;
    let progress = harness
        .state
        .db
        .progress(&viewer.device_id, &video)
        .unwrap();
    assert!(progress.is_none(), "deleted with the video");
}
