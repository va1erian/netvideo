//! End-to-end tests for scan and playback edge cases: unmounted roots,
//! files ffprobe cannot read, HTTP conditional requests and stream limits.

mod common;

use std::path::Path;

use axum::http::{HeaderValue, StatusCode, header};
use common::Harness;
use common::library::*;
use netvideo_server::api::stream_limit::PER_DEVICE_STREAMS;
use tower::ServiceExt;

#[tokio::test]
async fn an_empty_root_keeps_its_rows() {
    let harness = unprobed();
    write(&harness, "Films/heat.mkv", b"heat");
    scan(&harness).await;
    let viewer = harness.pair_viewer().await;
    let root = root_id(&harness, &viewer.token).await;

    // An unmounted share looks exactly like this: an empty directory.
    std::fs::remove_dir_all(harness.library.join("Films")).unwrap();
    let stats = scan(&harness).await;
    assert_eq!(stats.removed, 0);
    assert!(stats.partial);
    let (_, page) = get(&harness, &format!("/api/v1/folders/{root}"), &viewer.token).await;
    assert_eq!(page["folders"][0]["name"], "Films");
}

#[tokio::test]
async fn files_ffprobe_fails_on_are_not_retried_until_they_change() {
    // `true` passes the health check, then prints nothing for a file: a
    // probe failure, not a missing ffprobe.
    let harness = Harness::with_ffprobe(Path::new("true"));
    write(&harness, "broken.mkv", b"broken");
    let first = scan(&harness).await;
    assert_eq!((first.written, first.probe_failures), (1, 1));
    let second = scan(&harness).await;
    assert_eq!((second.written, second.probe_failures), (0, 0));

    write(&harness, "broken.mkv", b"broken, but longer");
    let third = scan(&harness).await;
    assert_eq!((third.written, third.probe_failures), (1, 1));
}

#[tokio::test]
async fn a_broken_ffprobe_marks_nothing_as_failed() {
    // `false` cannot even print its version, like an install missing a
    // shared library: files are indexed unprobed and retried later.
    let harness = Harness::with_ffprobe(Path::new("false"));
    write(&harness, "movie.mkv", b"movie");
    let stats = scan(&harness).await;
    assert_eq!((stats.written, stats.probe_failures), (1, 0));
}

#[tokio::test]
async fn an_unmounted_subfolder_keeps_its_rows() {
    let harness = unprobed();
    write(&harness, "Films/Action/heat.mkv", b"heat");
    write(&harness, "Shows/e01.mkv", b"e01");
    scan(&harness).await;
    let viewer = harness.pair_viewer().await;

    // `Films` is a mount point whose share went away: the folder is there,
    // but empty. `Shows` still has content, so the root is not empty.
    std::fs::remove_dir_all(harness.library.join("Films/Action")).unwrap();
    let stats = scan(&harness).await;
    assert_eq!(stats.removed, 0);
    assert!(stats.partial);
    let root = root_id(&harness, &viewer.token).await;
    let (_, page) = get(&harness, &format!("/api/v1/folders/{root}"), &viewer.token).await;
    let films = page["folders"][0]["id"].as_str().unwrap();
    let (_, films_page) = get(&harness, &format!("/api/v1/folders/{films}"), &viewer.token).await;
    assert_eq!(
        films_page["folders"][0]["name"], "Action",
        "intermediate folder kept"
    );

    // A folder deleted outright is pruned as usual.
    std::fs::remove_dir_all(harness.library.join("Shows")).unwrap();
    assert_eq!(scan(&harness).await.removed, 1);
}

#[tokio::test]
async fn deleting_a_video_beside_its_sidecars_prunes_it() {
    let harness = unprobed();
    write(&harness, "Films/Heat/Heat.mkv", b"heat");
    write(&harness, "Films/Heat/Heat.srt", b"subtitles");
    write(&harness, "Films/Heat/.poster.jpg", b"hidden poster");
    write(&harness, "Films/Other/x.mkv", b"x");
    scan(&harness).await;

    // The folder still holds files, so the video was deleted, not unmounted.
    std::fs::remove_file(harness.library.join("Films/Heat/Heat.mkv")).unwrap();
    let stats = scan(&harness).await;
    assert_eq!(stats.removed, 1);
    assert!(!stats.partial);
}

/// A scanned 10-byte video and the URI and ETag of its file.
async fn served_video(harness: &Harness, token: &str) -> (String, HeaderValue) {
    write(harness, "movie.mp4", b"0123456789");
    scan(harness).await;
    let id = video_id(harness, token, "movie.mp4").await;
    let uri = format!("/api/v1/videos/{id}/file");
    let (_, headers, _) = harness.request(harness.authed("GET", &uri, token)).await;
    (uri, headers[header::ETAG].clone())
}

async fn get_with(
    harness: &Harness,
    uri: &str,
    token: &str,
    extra: &[(header::HeaderName, &str)],
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut request = harness.authed("GET", uri, token);
    for (name, value) in extra {
        request
            .headers_mut()
            .insert(name.clone(), value.parse().unwrap());
    }
    let (status, headers, body) = harness.request(request).await;
    (status, headers, body.to_vec())
}

#[tokio::test]
async fn conditional_requests_follow_rfc_9110() {
    let harness = unprobed();
    let viewer = harness.pair_viewer().await;
    let (uri, etag) = served_video(&harness, &viewer.token).await;
    let etag = etag.to_str().unwrap().to_owned();
    let weak = format!("W/{etag}");
    let listed = format!("\"other\", {weak}");

    for matching in [etag.as_str(), weak.as_str(), listed.as_str(), "*"] {
        let (status, headers, body) = get_with(
            &harness,
            &uri,
            &viewer.token,
            &[(header::IF_NONE_MATCH, matching)],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_MODIFIED, "{matching}");
        assert!(body.is_empty());
        assert_eq!(headers[header::CACHE_CONTROL], "private, no-transform");
    }

    // The precondition is evaluated before Range.
    let (status, _, _) = get_with(
        &harness,
        &uri,
        &viewer.token,
        &[(header::IF_NONE_MATCH, &etag), (header::RANGE, "bytes=0-1")],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn unusable_ranges_fall_back_to_the_whole_file() {
    let harness = unprobed();
    let viewer = harness.pair_viewer().await;
    let (uri, etag) = served_video(&harness, &viewer.token).await;
    let etag = etag.to_str().unwrap().to_owned();

    let whole: [&[(header::HeaderName, &str)]; 4] = [
        &[(header::RANGE, "items=0-1")],
        &[(header::RANGE, "bytes=0-1,4-5")],
        &[(header::RANGE, "bytes=abc")],
        // A changed file: the stale If-Range validator voids the Range.
        &[
            (header::RANGE, "bytes=0-1"),
            (header::IF_RANGE, "\"stale\""),
        ],
    ];
    for extra in whole {
        let (status, _, body) = get_with(&harness, &uri, &viewer.token, extra).await;
        assert_eq!(status, StatusCode::OK, "{extra:?}");
        assert_eq!(body, b"0123456789");
    }

    let (status, _, body) = get_with(
        &harness,
        &uri,
        &viewer.token,
        &[(header::RANGE, "bytes=0-1"), (header::IF_RANGE, &etag)],
    )
    .await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body, b"01");
}

#[tokio::test]
async fn one_device_cannot_hold_unlimited_streams() {
    let harness = unprobed();
    let viewer = harness.pair_viewer().await;
    let (uri, _) = served_video(&harness, &viewer.token).await;

    // Responses whose bodies are never read keep their slots.
    let mut open = Vec::new();
    for _ in 0..PER_DEVICE_STREAMS {
        let response = harness
            .app
            .clone()
            .oneshot(harness.authed("GET", &uri, &viewer.token))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        open.push(response);
    }
    let (status, _, _) = harness
        .request(harness.authed("GET", &uri, &viewer.token))
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

    // Other devices are unaffected, and closing a stream frees its slot.
    let other = harness.pair_viewer().await;
    let (status, _, _) = harness
        .request(harness.authed("GET", &uri, &other.token))
        .await;
    assert_eq!(status, StatusCode::OK);
    open.pop();
    let (status, _, _) = harness
        .request(harness.authed("GET", &uri, &viewer.token))
        .await;
    assert_eq!(status, StatusCode::OK);
}
