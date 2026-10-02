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
use tokio::io::AsyncReadExt;

use crate::db::models::{ProbeInfo, StreamInfo, StreamKind};

/// Wall-clock limit for one probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

/// Largest accepted ffprobe output, in bytes.
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

/// Demuxers ffprobe may use: exactly the containers `formats::video_mime`
/// accepts. Playlist and concat demuxers, which can reference other files,
/// are excluded.
const FORMAT_WHITELIST: &str = "matroska,mov,avi,mpegts,mpeg,mpegvideo,flv,ogg,asf";

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
    /// ffprobe could not start for a passing reason (EMFILE, EAGAIN); the
    /// file is retried on the next scan.
    #[error("ffprobe could not start: {0}")]
    Transient(String),
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
        "-format_whitelist",
        FORMAT_WHITELIST,
        "-i",
    ]
    .into_iter()
    .map(std::ffi::OsString::from)
    .chain(std::iter::once(input))
    .collect()
}

/// Whether `program` runs at all. A broken install (say, a missing shared
/// library) fails every probe, which must not mark every file as unreadable.
pub async fn healthy(program: &Path) -> bool {
    let mut command = tokio::process::Command::new(program);
    command.arg("-version");
    run_capped(command, Duration::from_secs(10)).await.is_ok()
}

/// Probes `path` with the ffprobe executable at `program`.
pub async fn probe(program: &Path, path: &Path) -> Result<ProbeInfo, ProbeError> {
    let mut command = tokio::process::Command::new(program);
    command.args(probe_args(path));
    parse(&run_capped(command, PROBE_TIMEOUT).await?)
}

/// Runs `command` and returns its stdout, failing when it times out, exits
/// unsuccessfully or prints more than [`MAX_OUTPUT_BYTES`]. The child is
/// killed on every early return.
pub(crate) async fn run_capped(
    mut command: tokio::process::Command,
    timeout: Duration,
) -> Result<Vec<u8>, ProbeError> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| match error.kind() {
            // Only a missing or forbidden program means ffprobe is absent; a
            // transient failure (EMFILE, EAGAIN) is one failed probe.
            std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => {
                ProbeError::Unavailable(error)
            }
            _ => ProbeError::Transient(error.to_string()),
        })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProbeError::Failed("no stdout".into()))?;
    let run = async {
        // Read at most one byte past the cap, so oversized output is detected
        // without ever buffering it.
        let mut output = Vec::new();
        stdout
            .take(MAX_OUTPUT_BYTES as u64 + 1)
            .read_to_end(&mut output)
            .await?;
        if output.len() > MAX_OUTPUT_BYTES {
            return Ok((None, output));
        }
        Ok::<_, std::io::Error>((Some(child.wait().await?), output))
    };
    let (status, output) = tokio::time::timeout(timeout, run)
        .await
        .map_err(|_| ProbeError::Failed("timed out".into()))?
        .map_err(|error| ProbeError::Failed(error.to_string()))?;
    // `None` means the cap was hit; dropping the child kills it.
    let status = status.ok_or_else(|| ProbeError::Failed("output too large".into()))?;
    if !status.success() {
        return Err(ProbeError::Failed(status.to_string()));
    }
    Ok(output)
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
