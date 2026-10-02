//! Video metadata via `ffprobe`, run as a contained child process.
//!
//! The program is spawned with an argument vector (never a shell), with the
//! input passed as a `file:` URL and `-protocol_whitelist file`, so a file
//! name can never be read as an option or a network protocol. Output is parsed
//! into strict types; free-form strings are length-capped.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;

use crate::db::models::{ProbeInfo, StreamInfo, StreamKind};

/// Wall-clock limit for one probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

/// Largest accepted ffprobe output, in bytes.
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

/// Longest stored codec/profile/language/title string, in characters.
const MAX_TEXT_CHARS: usize = 200;

/// Why a probe produced no metadata.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    /// The ffprobe executable could not be started.
    #[error("ffprobe is unavailable: {0}")]
    Unavailable(std::io::Error),
    /// ffprobe ran but failed, timed out or printed something unusable.
    #[error("ffprobe failed: {0}")]
    Failed(String),
}

/// The ffprobe argument vector for `path` (an absolute path).
pub fn probe_args(path: &Path) -> Vec<std::ffi::OsString> {
    let mut input = std::ffi::OsString::from("file:");
    input.push(path.as_os_str());
    [
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
        "-protocol_whitelist",
        "file",
        "-i",
    ]
    .into_iter()
    .map(std::ffi::OsString::from)
    .chain(std::iter::once(input))
    .collect()
}

/// Probes `path` with the ffprobe executable at `program`.
pub async fn probe(program: &Path, path: &Path) -> Result<ProbeInfo, ProbeError> {
    let child = tokio::process::Command::new(program)
        .args(probe_args(path))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(ProbeError::Unavailable)?;
    let output = tokio::time::timeout(PROBE_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| ProbeError::Failed("timed out".into()))?
        .map_err(|error| ProbeError::Failed(error.to_string()))?;
    if !output.status.success() {
        return Err(ProbeError::Failed(format!("exit status {}", output.status)));
    }
    if output.stdout.len() > MAX_OUTPUT_BYTES {
        return Err(ProbeError::Failed("output too large".into()));
    }
    parse(&output.stdout)
}

/// Parses ffprobe's JSON output.
pub fn parse(json: &[u8]) -> Result<ProbeInfo, ProbeError> {
    let raw: RawProbe =
        serde_json::from_slice(json).map_err(|error| ProbeError::Failed(error.to_string()))?;
    let streams = raw.streams.iter().filter_map(stream_info).collect();
    Ok(ProbeInfo {
        container: raw.format.format_name.as_deref().map(cap),
        duration_ms: raw
            .format
            .duration
            .as_deref()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|secs| secs.is_finite() && *secs >= 0.0)
            .map(|secs| (secs * 1000.0).round() as i64),
        bitrate: raw
            .format
            .bit_rate
            .as_deref()
            .and_then(|value| value.parse::<i64>().ok()),
        streams,
    })
}

fn stream_info(raw: &RawStream) -> Option<StreamInfo> {
    let kind = match raw.codec_type.as_deref()? {
        "video" if raw.disposition.attached_pic == 0 => StreamKind::Video,
        "audio" => StreamKind::Audio,
        "subtitle" => StreamKind::Subtitle,
        _ => return None,
    };
    Some(StreamInfo {
        index: raw.index,
        kind,
        codec: raw.codec_name.as_deref().map(cap),
        profile: raw.profile.as_deref().map(cap),
        width: raw.width,
        height: raw.height,
        fps: raw.avg_frame_rate.as_deref().and_then(parse_rate),
        channels: raw.channels,
        language: raw.tags.language.as_deref().map(cap),
        title: raw.tags.title.as_deref().map(cap),
        is_default: raw.disposition.default != 0,
        is_forced: raw.disposition.forced != 0,
    })
}

fn parse_rate(value: &str) -> Option<f64> {
    let (num, den) = value.split_once('/')?;
    let (num, den) = (num.parse::<f64>().ok()?, den.parse::<f64>().ok()?);
    (den > 0.0 && num > 0.0).then(|| num / den)
}

fn cap(value: &str) -> String {
    value.chars().take(MAX_TEXT_CHARS).collect()
}

#[derive(Deserialize)]
struct RawProbe {
    #[serde(default)]
    streams: Vec<RawStream>,
    #[serde(default)]
    format: RawFormat,
}

#[derive(Deserialize, Default)]
struct RawFormat {
    format_name: Option<String>,
    duration: Option<String>,
    bit_rate: Option<String>,
}

#[derive(Deserialize)]
struct RawStream {
    index: i64,
    codec_type: Option<String>,
    codec_name: Option<String>,
    profile: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    avg_frame_rate: Option<String>,
    channels: Option<i64>,
    #[serde(default)]
    disposition: RawDisposition,
    #[serde(default)]
    tags: RawTags,
}

#[derive(Deserialize, Default)]
struct RawDisposition {
    #[serde(default)]
    default: i64,
    #[serde(default)]
    forced: i64,
    #[serde(default)]
    attached_pic: i64,
}

#[derive(Deserialize, Default)]
struct RawTags {
    language: Option<String>,
    title: Option<String>,
}

#[cfg(test)]
mod tests;
