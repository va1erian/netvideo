//! Video file extensions the scanner indexes, and their MIME types.

use std::path::Path;

/// `(extension, MIME type)` for every indexed video container.
const VIDEO_TYPES: &[(&str, &str)] = &[
    ("3gp", "video/3gpp"),
    ("avi", "video/x-msvideo"),
    ("flv", "video/x-flv"),
    ("m2ts", "video/mp2t"),
    ("m4v", "video/mp4"),
    ("mkv", "video/x-matroska"),
    ("mov", "video/quicktime"),
    ("mp4", "video/mp4"),
    ("mpeg", "video/mpeg"),
    ("mpg", "video/mpeg"),
    ("mts", "video/mp2t"),
    ("ogv", "video/ogg"),
    ("ts", "video/mp2t"),
    ("webm", "video/webm"),
    ("wmv", "video/x-ms-wmv"),
];

/// The MIME type of a video file name, or `None` when it is not a video the
/// scanner indexes. Matching is on the extension, case-insensitively.
pub fn video_mime(name: &str) -> Option<&'static str> {
    let ext = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    VIDEO_TYPES
        .iter()
        .find(|(known, _)| *known == ext)
        .map(|(_, mime)| *mime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_extensions_match_case_insensitively() {
        assert_eq!(video_mime("Film.MKV"), Some("video/x-matroska"));
        assert_eq!(video_mime("clip.mp4"), Some("video/mp4"));
        assert_eq!(video_mime("notes.txt"), None);
        assert_eq!(video_mime("no-extension"), None);
    }
}
