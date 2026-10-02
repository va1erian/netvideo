//! Server response types, mirroring `netvideo-server`'s API.
//!
//! Unknown fields are ignored, so a newer server can add data without
//! breaking older clients.

use serde::{Deserialize, Serialize};

/// A folder as listed by the browse API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderRef {
    /// Opaque folder id.
    pub id: String,
    /// Folder name.
    pub name: String,
}

/// One device's playback progress on a video.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// Resume position in milliseconds.
    pub position_ms: i64,
    /// Whether the video is marked as watched.
    pub watched: bool,
    /// When the progress was last saved (Unix seconds).
    pub updated_at: i64,
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
    /// This device's progress, when it has any.
    #[serde(default)]
    pub progress: Option<Progress>,
}

/// One page of a folder: subfolders first, then videos, each by name.
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

/// One stream (track) of a video.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamInfo {
    /// Stream index in the container.
    pub index: i64,
    /// `video`, `audio` or `subtitle`.
    pub kind: String,
    /// Codec name, as ffprobe reports it.
    pub codec: Option<String>,
    /// Codec profile.
    pub profile: Option<String>,
    /// Width in pixels (video).
    pub width: Option<i64>,
    /// Height in pixels (video).
    pub height: Option<i64>,
    /// Frame rate (video).
    pub fps: Option<f64>,
    /// Channel count (audio).
    pub channels: Option<i64>,
    /// Language tag.
    pub language: Option<String>,
    /// Stream title.
    pub title: Option<String>,
    /// Whether the stream is marked default.
    #[serde(default)]
    pub is_default: bool,
    /// Whether the stream is marked forced.
    #[serde(default)]
    pub is_forced: bool,
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
    #[serde(default)]
    pub streams: Vec<StreamInfo>,
    /// This device's progress, when it has any.
    #[serde(default)]
    pub progress: Option<Progress>,
}

/// Response of `POST /api/v1/auth/pair`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairResponse {
    /// Server-assigned device id.
    pub device_id: String,
    /// Device name as registered.
    pub device_name: String,
    /// First access token.
    pub auth_token: String,
    /// Token expiry (Unix seconds).
    pub expires_at: i64,
    /// The server's PASERK public key.
    #[serde(default)]
    pub server_key: Option<String>,
}

/// Response of `POST /api/v1/auth/refresh`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenResponse {
    /// The renewed access token.
    pub auth_token: String,
    /// Expiry (Unix seconds).
    pub expires_at: i64,
    /// The server's PASERK public key.
    #[serde(default)]
    pub server_key: Option<String>,
}

/// Response of `GET /api/v1/health`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    /// Always `ok` when the server is serving.
    pub status: String,
    /// Server start time (Unix seconds).
    #[serde(default)]
    pub started_at: i64,
}

/// The server's JSON error body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    /// Human-readable error.
    pub error: String,
}
