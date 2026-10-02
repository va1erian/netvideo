//! HTTP `Range` header parsing for byte-serving.
//!
//! Only a single range is supported — multiple ranges are rejected rather
//! than silently served as the whole file — and every bound is clamped to the
//! file length so a hostile header can never read outside the file.

/// Why a range request could not be honoured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeError {
    /// The header is syntactically invalid.
    Invalid,
    /// The range is valid but falls outside the file.
    Unsatisfiable,
}

/// Parses a `Range: bytes=...` header against a file of `len` bytes.
///
/// Returns an inclusive `(start, end)` pair.
pub fn parse_range(raw: &str, len: u64) -> Result<(u64, u64), RangeError> {
    let (unit, spec) = raw.trim().split_once('=').ok_or(RangeError::Invalid)?;
    if !unit.eq_ignore_ascii_case("bytes") {
        return Err(RangeError::Invalid);
    }
    let spec = spec.trim();
    if spec.contains(',') {
        return Err(RangeError::Invalid);
    }
    if len == 0 {
        return Err(RangeError::Unsatisfiable);
    }
    let (start_text, end_text) = spec.split_once('-').ok_or(RangeError::Invalid)?;
    let start_text = start_text.trim();
    let end_text = end_text.trim();

    if start_text.is_empty() {
        // Suffix range: last N bytes.
        let suffix: u64 = end_text.parse().map_err(|_| RangeError::Invalid)?;
        if suffix == 0 {
            return Err(RangeError::Unsatisfiable);
        }
        let start = len.saturating_sub(suffix);
        return Ok((start, len - 1));
    }

    let start: u64 = start_text.parse().map_err(|_| RangeError::Invalid)?;
    if start >= len {
        return Err(RangeError::Unsatisfiable);
    }
    let end = if end_text.is_empty() {
        len - 1
    } else {
        let end: u64 = end_text.parse().map_err(|_| RangeError::Invalid)?;
        if end < start {
            return Err(RangeError::Invalid);
        }
        end.min(len - 1)
    };
    Ok((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bounded_range() {
        assert_eq!(parse_range("bytes=0-99", 1000), Ok((0, 99)));
        assert_eq!(parse_range("bytes=100-199", 1000), Ok((100, 199)));
    }

    #[test]
    fn open_ended_range_runs_to_eof() {
        assert_eq!(parse_range("bytes=500-", 1000), Ok((500, 999)));
    }

    #[test]
    fn suffix_range_reads_last_bytes() {
        assert_eq!(parse_range("bytes=-100", 1000), Ok((900, 999)));
        assert_eq!(parse_range("bytes=-5000", 1000), Ok((0, 999)));
    }

    #[test]
    fn end_is_clamped_to_file_length() {
        assert_eq!(parse_range("bytes=990-100000", 1000), Ok((990, 999)));
    }

    #[test]
    fn start_past_eof_is_unsatisfiable() {
        assert_eq!(
            parse_range("bytes=1000-", 1000),
            Err(RangeError::Unsatisfiable)
        );
        assert_eq!(
            parse_range("bytes=1000-2000", 1000),
            Err(RangeError::Unsatisfiable)
        );
    }

    #[test]
    fn zero_suffix_is_unsatisfiable() {
        assert_eq!(
            parse_range("bytes=-0", 1000),
            Err(RangeError::Unsatisfiable)
        );
    }

    #[test]
    fn empty_file_is_unsatisfiable() {
        assert_eq!(parse_range("bytes=0-", 0), Err(RangeError::Unsatisfiable));
    }

    #[test]
    fn unit_is_case_insensitive() {
        assert_eq!(parse_range("Bytes=0-9", 100), Ok((0, 9)));
        assert_eq!(parse_range("BYTES=0-9", 100), Ok((0, 9)));
    }

    #[test]
    fn malformed_headers_are_invalid() {
        assert_eq!(parse_range("items=0-1", 10), Err(RangeError::Invalid));
        assert_eq!(parse_range("bytes=abc-def", 10), Err(RangeError::Invalid));
        assert_eq!(parse_range("bytes=0-1,5-6", 10), Err(RangeError::Invalid));
        assert_eq!(parse_range("bytes=5-1", 10), Err(RangeError::Invalid));
        assert_eq!(parse_range("bytes=0", 10), Err(RangeError::Invalid));
    }
}
