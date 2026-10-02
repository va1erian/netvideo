//! Unit tests for configuration parsing, overrides and validation.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

use super::*;

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    move |key: &str| map.get(key).cloned()
}

const MINIMAL: &str = r#"
[library]
paths = ["/media/videos"]
"#;

#[test]
fn minimal_toml_parses_with_defaults() {
    let config = Config::from_toml(MINIMAL).expect("parse");
    assert_eq!(config.server.port, 8080);
    assert_eq!(config.security.token_ttl_hours, 168);
    assert_eq!(config.library.scan_interval_secs, 3600);
    config.validate().expect("valid");
}

#[test]
fn full_toml_round_trips_through_serialization() {
    let text = r#"
[server]
host = "127.0.0.1"
port = 9000
data_dir = "/var/lib/netvideo"
trusted_proxies = ["172.16.0.0/12", "127.0.0.1"]

[security]
tls_cert = "/etc/cert.pem"
tls_key = "/etc/key.pem"
token_ttl_hours = 24
max_pairing_attempts_per_min = 5
pairing_code_ttl_secs = 300
max_body_bytes = 4096

[library]
paths = ["/media/videos", "/media/films"]
scan_interval_secs = 600
ffprobe_path = "/usr/bin/ffprobe"
"#;
    let config = Config::from_toml(text).expect("parse");
    config.validate().expect("valid");
    assert!(config.tls_enabled());
    let encoded = toml::to_string(&config).expect("serialize");
    let back = Config::from_toml(&encoded).expect("reparse");
    assert_eq!(config, back);
}

#[test]
fn unknown_fields_are_rejected() {
    let text = r#"
[security]
token_ttl_hours = 1
surprise = true
[library]
paths = ["/x"]
"#;
    assert!(Config::from_toml(text).is_err());
}

#[test]
fn env_overrides_are_applied() {
    let mut config = Config::from_toml(MINIMAL).expect("parse");
    config
        .apply_env_with(env(&[
            ("NETVIDEO_PORT", "7000"),
            ("NETVIDEO_HOST", "127.0.0.1"),
            ("NETVIDEO_TRUSTED_PROXIES", "10.0.0.0/8, 192.168.1.5"),
            ("NETVIDEO_TOKEN_TTL_HOURS", "12"),
            ("NETVIDEO_LIBRARY_PATHS", "/a;/b"),
            ("NETVIDEO_SCAN_INTERVAL", "0"),
        ]))
        .expect("env");
    assert_eq!(config.server.port, 7000);
    assert_eq!(config.server.host, "127.0.0.1");
    assert_eq!(config.server.trusted_proxies.len(), 2);
    assert_eq!(config.security.token_ttl_hours, 12);
    assert_eq!(config.library.paths.len(), 2);
    assert_eq!(config.library.scan_interval_secs, 0);
    config.validate().expect("valid");
}

#[test]
fn bad_env_value_is_rejected() {
    let mut config = Config::from_toml(MINIMAL).expect("parse");
    let error = config
        .apply_env_with(env(&[("NETVIDEO_PORT", "not-a-port")]))
        .expect_err("bad port");
    assert!(matches!(error, ServerError::Config(_)));
}

#[test]
fn missing_library_paths_are_rejected() {
    let config = Config::default();
    assert!(config.validate().is_err());
}

#[test]
fn port_zero_is_rejected() {
    let mut config = Config::from_toml(MINIMAL).expect("parse");
    config.server.port = 0;
    assert!(config.validate().is_err());
}

#[test]
fn tls_half_configuration_is_rejected() {
    let mut config = Config::from_toml(MINIMAL).expect("parse");
    config.security.tls_cert = "/only/cert.pem".into();
    assert!(config.validate().is_err());
}

#[test]
fn zero_token_ttl_is_rejected() {
    let mut config = Config::from_toml(MINIMAL).expect("parse");
    config.security.token_ttl_hours = 0;
    assert!(config.validate().is_err());
}

#[test]
fn out_of_range_security_values_are_rejected() {
    let mut config = Config::from_toml(MINIMAL).expect("parse");
    config.security.token_ttl_hours = MAX_TOKEN_TTL_HOURS + 1;
    assert!(config.validate().is_err());

    let mut config = Config::from_toml(MINIMAL).expect("parse");
    config.security.pairing_code_ttl_secs = MAX_PAIRING_CODE_TTL_SECS + 1;
    assert!(config.validate().is_err());

    let mut config = Config::from_toml(MINIMAL).expect("parse");
    config.security.max_body_bytes = MAX_BODY_BYTES + 1;
    assert!(config.validate().is_err());
}

#[test]
fn trusted_proxies_accept_cidr_and_single_ips() {
    let mut config = Config::from_toml(MINIMAL).expect("parse");
    config.server.trusted_proxies = vec!["172.16.0.0/12".into(), "127.0.0.1".into()];
    let nets = config.trusted_proxy_nets().expect("nets");
    assert!(config.is_trusted_proxy(&nets, IpAddr::V4(Ipv4Addr::new(172, 16, 0, 9))));
    assert!(config.is_trusted_proxy(&nets, IpAddr::V4(Ipv4Addr::LOCALHOST)));
    assert!(!config.is_trusted_proxy(&nets, IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
}

#[test]
fn invalid_trusted_proxy_is_rejected() {
    let mut config = Config::from_toml(MINIMAL).expect("parse");
    config.server.trusted_proxies = vec!["not-an-ip".into()];
    assert!(config.validate().is_err());
}

#[test]
fn hostnames_are_rejected_as_listen_hosts() {
    let mut config = Config::from_toml(MINIMAL).expect("parse");
    config.server.host = "localhost".into();
    assert!(config.validate().is_err());
    config.server.host = "::".into();
    config.validate().expect("IPv6 literal is valid");
}
