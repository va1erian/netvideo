//! Persisted row models shared between the database and the REST API.

use serde::{Deserialize, Serialize};

/// A paired client device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// Server-assigned opaque identifier (UUID v4).
    pub id: String,
    /// Human-readable device name supplied at pairing time.
    pub name: String,
    /// PASERK-encoded Ed25519 public key proving possession at refresh.
    pub public_key: String,
    /// Unix timestamp (seconds) when the device paired.
    pub paired_at: i64,
    /// Unix timestamp (seconds) of the device's last authenticated request.
    pub last_seen: Option<i64>,
    /// Whether administrator revocation has invalidated the device.
    pub is_revoked: bool,
    /// Whether the device may manage devices and mint pairing codes.
    pub is_admin: bool,
}

/// Kind of an elementary stream inside a video file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamKind {
    /// A video track (cover art is not counted).
    Video,
    /// An audio track.
    Audio,
    /// A subtitle track.
    Subtitle,
}

impl StreamKind {
    /// The value stored in the `streams.kind` column.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Subtitle => "subtitle",
        }
    }

    /// Parses a stored `streams.kind` value.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "video" => Some(Self::Video),
            "audio" => Some(Self::Audio),
            "subtitle" => Some(Self::Subtitle),
            _ => None,
        }
    }
}

/// One elementary stream, as reported by ffprobe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamInfo {
    /// Stream index inside the container.
    pub index: i64,
    /// Video, audio or subtitle.
    pub kind: StreamKind,
    /// Codec name (`h264`, `hevc`, `aac`, `subrip`, ...).
    pub codec: Option<String>,
    /// Codec profile (`High`, `Main 10`, ...).
    pub profile: Option<String>,
    /// Frame width in pixels (video).
    pub width: Option<i64>,
    /// Frame height in pixels (video).
    pub height: Option<i64>,
    /// Average frame rate (video).
    pub fps: Option<f64>,
    /// Channel count (audio).
    pub channels: Option<i64>,
    /// Language tag (`eng`, `fra`, ...).
    pub language: Option<String>,
    /// Track title.
    pub title: Option<String>,
    /// Whether the track is flagged default.
    pub is_default: bool,
    /// Whether the track is flagged forced.
    pub is_forced: bool,
}

/// Metadata read from a video file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProbeInfo {
    /// ffprobe's container name (`matroska,webm`, `mov,mp4,m4a,3gp,3g2,mj2`).
    pub container: Option<String>,
    /// Duration in milliseconds.
    pub duration_ms: Option<i64>,
    /// Overall bitrate in bits per second.
    pub bitrate: Option<i64>,
    /// Video, audio and subtitle streams.
    pub streams: Vec<StreamInfo>,
}

/// A folder reference: its opaque id and display name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderRef {
    /// Opaque folder id.
    pub id: String,
    /// Folder name (the root's configured directory name for a root).
    pub name: String,
}

/// A video as listed in a folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoSummary {
    /// Opaque video id.
    pub id: String,
    /// File name, including its extension.
    pub name: String,
    /// File size in bytes.
    pub size: i64,
    /// Duration in milliseconds, when probed.
    pub duration_ms: Option<i64>,
    /// Codec of the first video stream, when probed.
    pub video_codec: Option<String>,
    /// Width of the first video stream, when probed.
    pub width: Option<i64>,
    /// Height of the first video stream, when probed.
    pub height: Option<i64>,
    /// The requesting device's playback progress, when it has any.
    pub progress: Option<Progress>,
}

/// One device's playback progress on a video.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// Resume position in milliseconds.
    pub position_ms: i64,
    /// Whether the device marked the video as watched.
    pub watched: bool,
    /// When the progress was last saved (Unix seconds).
    pub updated_at: i64,
}

/// One page of a folder's contents: subfolders first, then videos, each
/// sorted by name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FolderPage {
    /// The folder itself.
    pub folder: FolderRef,
    /// Ancestors from the root down to the parent (empty for a root).
    pub path: Vec<FolderRef>,
    /// Subfolders on this page.
    pub folders: Vec<FolderRef>,
    /// Videos on this page.
    pub videos: Vec<VideoSummary>,
    /// Cursor for the next page, when there is one.
    pub next_cursor: Option<String>,
}

/// Full details of one video.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoDetail {
    /// Opaque video id.
    pub id: String,
    /// File name, including its extension.
    pub name: String,
    /// The folder holding the video.
    pub folder_id: String,
    /// File size in bytes.
    pub size: i64,
    /// Modification time (nanoseconds since the Unix epoch).
    pub mtime_ns: i64,
    /// MIME type served by the direct-play endpoint.
    pub mime: String,
    /// Whether ffprobe metadata is available.
    pub probed: bool,
    /// ffprobe's container name.
    pub container: Option<String>,
    /// Duration in milliseconds.
    pub duration_ms: Option<i64>,
    /// Overall bitrate in bits per second.
    pub bitrate: Option<i64>,
    /// Video, audio and subtitle streams.
    pub streams: Vec<StreamInfo>,
    /// The requesting device's playback progress, when it has any.
    pub progress: Option<Progress>,
}

/// Where a video lives on disk, for the file-serving path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoLocation {
    /// Index of the library root.
    pub root_index: i64,
    /// Path relative to the root, using forward slashes.
    pub rel_path: String,
    /// File size in bytes at the last scan.
    pub size: i64,
    /// Modification time (nanoseconds since the Unix epoch) at the last scan.
    pub mtime_ns: i64,
}
