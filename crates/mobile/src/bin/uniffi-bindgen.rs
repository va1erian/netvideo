//! Bindings generator entry point.
//!
//! Built only with `--features cli`:
//!
//! ```text
//! cargo run -p netvideo-mobile --features cli --bin uniffi-bindgen -- \
//!     generate --library <libnetvideo_mobile.so> --language kotlin --out-dir <dir>
//! ```

#![forbid(unsafe_code)]

fn main() {
    uniffi::uniffi_bindgen_main()
}
