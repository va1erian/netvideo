//! Pairing links: what `netvideo-server pair --qr` encodes in its QR code.
//!
//! `netvideo://pair?v=1&url=<server URL>&code=<6 digits>&key=<fingerprint>`
//!
//! The key fingerprint lets the client reject any server that does not hold
//! this server's signing key, even if the network lies to it during pairing.
//! `netvideo-client`'s `PairingLink::parse` reads the same format.

use crate::error::{Result, ServerError};

/// Checks a public URL with the same rules clients apply, and returns it
/// without a trailing slash. Run it before minting a code, so a bad URL
/// does not leave a live code nobody sees.
pub fn validate_public_url(url: &str) -> Result<String> {
    let trimmed = url.trim().trim_end_matches('/');
    let lower = trimmed.to_ascii_lowercase();
    let host = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .map(|rest| rest.split('/').next().unwrap_or(""));
    let valid = host.is_some_and(|host| !host.is_empty())
        && !trimmed.contains(['?', '#'])
        && !trimmed.contains(char::is_whitespace);
    if !valid {
        return Err(ServerError::Config(format!(
            "public URL must be http(s)://host[:port][/path], got {url:?}"
        )));
    }
    Ok(trimmed.to_owned())
}

/// Builds the pairing link for `url` (the address clients reach the server
/// at), a fresh pairing `code` and the server key `fingerprint`.
pub fn pairing_link(url: &str, code: &str, fingerprint: &str) -> Result<String> {
    let url = validate_public_url(url)?;
    Ok(format!(
        "netvideo://pair?v=1&url={}&code={code}&key={fingerprint}",
        encode(&url)
    ))
}

/// Renders `text` as a QR code drawn with Unicode half blocks, for a
/// terminal. Light modules are drawn as spaces, so it reads on dark themes.
pub fn render_qr(text: &str) -> Result<String> {
    use qrcode::render::unicode::Dense1x2;
    let code = qrcode::QrCode::new(text.as_bytes())
        .map_err(|error| ServerError::Config(format!("cannot encode QR code: {error}")))?;
    Ok(code
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Light)
        .light_color(Dense1x2::Dark)
        .quiet_zone(true)
        .build())
}

/// Percent-encodes everything but RFC 3986 unreserved characters.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_link_with_an_encoded_url() {
        let link = pairing_link("https://video.example:8443/nv/", "123456", "ab").unwrap();
        assert_eq!(
            link,
            "netvideo://pair?v=1&url=https%3A%2F%2Fvideo.example%3A8443%2Fnv&code=123456&key=ab"
        );
    }

    #[test]
    fn rejects_urls_clients_would_refuse() {
        for bad in [
            "video.example",
            "https://video.example/?x=1",
            "https://a b",
            "https://",
            "https:///path",
        ] {
            assert!(pairing_link(bad, "123456", "ab").is_err(), "{bad}");
        }
    }

    #[test]
    fn renders_a_qr_code() {
        let qr = render_qr("netvideo://pair?v=1").unwrap();
        assert!(qr.lines().count() > 10);
    }
}
