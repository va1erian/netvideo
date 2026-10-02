//! Unit tests for ffprobe argument building and output parsing.

use super::*;

const SAMPLE: &str = r#"{
  "streams": [
    {"index": 0, "codec_type": "video", "codec_name": "hevc", "profile": "Main 10",
     "width": 3840, "height": 2160, "avg_frame_rate": "24000/1001",
     "disposition": {"default": 1, "forced": 0, "attached_pic": 0}},
    {"index": 1, "codec_type": "audio", "codec_name": "dts", "channels": 6,
     "tags": {"language": "eng", "title": "Surround"},
     "disposition": {"default": 1}},
    {"index": 2, "codec_type": "subtitle", "codec_name": "subrip",
     "tags": {"language": "fra"}, "disposition": {"forced": 1}},
    {"index": 3, "codec_type": "video", "codec_name": "mjpeg",
     "disposition": {"attached_pic": 1}},
    {"index": 4, "codec_type": "attachment", "codec_name": "ttf"}
  ],
  "format": {"format_name": "matroska,webm", "duration": "5400.250", "bit_rate": "12000000"}
}"#;

#[test]
fn parses_streams_and_format() {
    let info = parse(SAMPLE.as_bytes()).expect("parse");
    assert_eq!(info.container.as_deref(), Some("matroska,webm"));
    assert_eq!(info.duration_ms, Some(5_400_250));
    assert_eq!(info.bitrate, Some(12_000_000));
    assert_eq!(
        info.streams.len(),
        3,
        "cover art and attachments are dropped"
    );

    let video = &info.streams[0];
    assert_eq!(video.kind, StreamKind::Video);
    assert_eq!(video.codec.as_deref(), Some("hevc"));
    assert_eq!((video.width, video.height), (Some(3840), Some(2160)));
    assert!((video.fps.unwrap() - 23.976).abs() < 0.001);

    let audio = &info.streams[1];
    assert_eq!(audio.kind, StreamKind::Audio);
    assert_eq!(audio.channels, Some(6));
    assert_eq!(audio.language.as_deref(), Some("eng"));
    assert!(audio.is_default);

    let subtitle = &info.streams[2];
    assert_eq!(subtitle.kind, StreamKind::Subtitle);
    assert!(subtitle.is_forced);
}

#[test]
fn rejects_garbage_and_tolerates_missing_fields() {
    assert!(parse(b"not json").is_err());
    let info = parse(br#"{"format": {"duration": "N/A"}}"#).expect("parse");
    assert!(info.streams.is_empty());
    assert_eq!(info.duration_ms, None);
}

#[test]
fn long_strings_are_capped() {
    let title = "x".repeat(10_000);
    let json = format!(
        r#"{{"streams": [{{"index": 0, "codec_type": "audio", "tags": {{"title": "{title}"}}}}]}}"#
    );
    let info = parse(json.as_bytes()).expect("parse");
    assert_eq!(
        info.streams[0].title.as_ref().unwrap().len(),
        MAX_TEXT_CHARS
    );
}

#[test]
fn input_is_a_file_url_after_the_protocol_whitelist() {
    let args = probe_args(Path::new("/media/-i http:evil.mkv"));
    let args: Vec<_> = args.iter().map(|arg| arg.to_str().unwrap()).collect();
    let whitelist = args
        .iter()
        .position(|arg| *arg == "-protocol_whitelist")
        .unwrap();
    assert_eq!(args[whitelist + 1], "file");
    assert_eq!(args[args.len() - 2], "-i");
    assert_eq!(args[args.len() - 1], "file:/media/-i http:evil.mkv");
}

#[tokio::test]
async fn a_missing_program_is_reported_as_unavailable() {
    let error = probe(Path::new("/nonexistent/ffprobe"), Path::new("/tmp/x.mkv"))
        .await
        .unwrap_err();
    assert!(matches!(error, ProbeError::Unavailable(_)));
}

#[cfg(unix)]
fn shell(script: &str) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("sh");
    command.args(["-c", script]);
    command
}

#[cfg(unix)]
#[tokio::test]
async fn endless_output_is_cut_off_at_the_cap() {
    let error = run_capped(shell("exec yes"), Duration::from_secs(10))
        .await
        .unwrap_err();
    assert!(
        matches!(error, ProbeError::Failed(ref why) if why == "output too large"),
        "{error}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_hung_program_times_out() {
    let error = run_capped(shell("exec sleep 30"), Duration::from_millis(100))
        .await
        .unwrap_err();
    assert!(matches!(error, ProbeError::Failed(ref why) if why == "timed out"));
}

#[cfg(unix)]
#[tokio::test]
async fn a_failing_program_is_a_failed_probe() {
    let error = run_capped(shell("echo '{}'; exit 1"), Duration::from_secs(10))
        .await
        .unwrap_err();
    assert!(matches!(error, ProbeError::Failed(_)));
}
