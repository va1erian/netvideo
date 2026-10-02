//! Records crossing the uniffi boundary, mirroring `netvideo_client::types`.

use netvideo_client::types as client;

/// A folder as listed by the browse API.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FolderRef {
    /// Opaque folder id.
    pub id: String,
    /// Folder name.
    pub name: String,
}

/// This device's playback progress on a video.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct Progress {
    /// Resume position in milliseconds.
    pub position_ms: i64,
    /// Whether the video is marked as watched.
    pub watched: bool,
    /// When the progress was last saved (Unix seconds).
    pub updated_at: i64,
}

/// A video as listed in a folder.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
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
    pub progress: Option<Progress>,
}

/// One page of a folder: subfolders first, then videos, each by name.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
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
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
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
    pub is_default: bool,
    /// Whether the stream is marked forced.
    pub is_forced: bool,
}

/// Full details of one video.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
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
    pub streams: Vec<StreamInfo>,
    /// This device's progress, when it has any.
    pub progress: Option<Progress>,
}

impl From<client::FolderRef> for FolderRef {
    fn from(folder: client::FolderRef) -> Self {
        Self {
            id: folder.id,
            name: folder.name,
        }
    }
}

impl From<client::Progress> for Progress {
    fn from(progress: client::Progress) -> Self {
        Self {
            position_ms: progress.position_ms,
            watched: progress.watched,
            updated_at: progress.updated_at,
        }
    }
}

impl From<client::VideoSummary> for VideoSummary {
    fn from(video: client::VideoSummary) -> Self {
        Self {
            id: video.id,
            name: video.name,
            size: video.size,
            duration_ms: video.duration_ms,
            video_codec: video.video_codec,
            width: video.width,
            height: video.height,
            progress: video.progress.map(Progress::from),
        }
    }
}

impl From<client::FolderPage> for FolderPage {
    fn from(page: client::FolderPage) -> Self {
        Self {
            folder: page.folder.into(),
            path: page.path.into_iter().map(FolderRef::from).collect(),
            folders: page.folders.into_iter().map(FolderRef::from).collect(),
            videos: page.videos.into_iter().map(VideoSummary::from).collect(),
            next_cursor: page.next_cursor,
        }
    }
}

impl From<client::StreamInfo> for StreamInfo {
    fn from(stream: client::StreamInfo) -> Self {
        Self {
            index: stream.index,
            kind: stream.kind,
            codec: stream.codec,
            profile: stream.profile,
            width: stream.width,
            height: stream.height,
            fps: stream.fps,
            channels: stream.channels,
            language: stream.language,
            title: stream.title,
            is_default: stream.is_default,
            is_forced: stream.is_forced,
        }
    }
}

impl From<client::VideoDetail> for VideoDetail {
    fn from(video: client::VideoDetail) -> Self {
        Self {
            id: video.id,
            name: video.name,
            folder_id: video.folder_id,
            size: video.size,
            mime: video.mime,
            probed: video.probed,
            container: video.container,
            duration_ms: video.duration_ms,
            bitrate: video.bitrate,
            streams: video.streams.into_iter().map(StreamInfo::from).collect(),
            progress: video.progress.map(Progress::from),
        }
    }
}
