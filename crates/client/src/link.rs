//! Pairing links, as scanned from `netvideo-server pair --qr`.
//!
//! `netvideo://pair?v=1&url=<server URL>&code=<6 digits>&key=<fingerprint>`

use crate::config::normalize_url;
use crate::error::{ClientError, Result};

/// What a pairing QR code tells the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingLink {
    /// The server's normalized base URL.
    pub url: String,
    /// The one-time pairing code.
    pub code: String,
    /// The server key fingerprint the paired server must match.
    pub key_fingerprint: String,
}

impl PairingLink {
    /// Parses a scanned link, rejecting anything that is not exactly the
    /// format above: a QR code is untrusted input.
    pub fn parse(text: &str) -> Result<Self> {
        let invalid = || ClientError::Url("not a netvideo pairing code".into());
        let query = text
            .trim()
            .strip_prefix("netvideo://pair?")
            .ok_or_else(invalid)?;
        let (mut version, mut url, mut code, mut key) = (None, None, None, None);
        for pair in query.split('&') {
            let (name, value) = pair.split_once('=').ok_or_else(invalid)?;
            let slot = match name {
                "v" => &mut version,
                "url" => &mut url,
                "code" => &mut code,
                "key" => &mut key,
                _ => continue,
            };
            if slot.replace(value).is_some() {
                return Err(invalid());
            }
        }
        if version != Some("1") {
            return Err(ClientError::Url(
                "this pairing code needs a newer netvideo app".into(),
            ));
        }
        let code = code.filter(|c| c.len() == 6 && c.bytes().all(|b| b.is_ascii_digit()));
        let key = key
            .filter(|k| k.len() == 64 && k.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
        let (Some(url), Some(code), Some(key)) = (url, code, key) else {
            return Err(invalid());
        };
        Ok(Self {
            url: normalize_url(&decode(url).ok_or_else(invalid)?)?,
            code: code.to_owned(),
            key_fingerprint: key.to_owned(),
        })
    }
}

/// Decodes `%XX` escapes; `None` for a malformed escape or invalid UTF-8.
fn decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn link(query: &str) -> String {
        format!("netvideo://pair?{query}")
    }

    #[test]
    fn parses_what_the_server_prints() {
        let text = link(&format!(
            "v=1&url=https%3A%2F%2Fvideo.example%3A8443%2Fnv&code=123456&key={KEY}"
        ));
        let parsed = PairingLink::parse(&text).unwrap();
        assert_eq!(parsed.url, "https://video.example:8443/nv");
        assert_eq!(parsed.code, "123456");
        assert_eq!(parsed.key_fingerprint, KEY);
    }

    #[test]
    fn rejects_malformed_links() {
        let good = format!("url=https%3A%2F%2Fa.example&code=123456&key={KEY}");
        for bad in [
            "https://a.example".to_owned(),
            link(&good),
            link(&format!("v=2&{good}")),
            link(&format!("v=1&{good}&code=654321")),
            link(&format!(
                "v=1&url=https%3A%2F%2Fa.example&code=12345&key={KEY}"
            )),
            link("v=1&url=https%3A%2F%2Fa.example&code=123456&key=ABC"),
            link(&format!(
                "v=1&url=ftp%3A%2F%2Fa.example&code=123456&key={KEY}"
            )),
            link(&format!("v=1&url=%ZZ&code=123456&key={KEY}")),
        ] {
            assert!(PairingLink::parse(&bad).is_err(), "{bad}");
        }
    }
}
