//! Finds Tengen's OGS client id at build time, so it never has to be edited
//! into the source. In order:
//!
//! 1. the `TENGEN_OGS_CLIENT_ID` environment variable;
//! 2. `ogs_client_id = "…"` in `tengen.local.toml` at the repository root,
//!    a file git ignores (copy `tengen.local.example.toml` to start).

use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let local = root.join("tengen.local.toml");
    println!("cargo:rerun-if-env-changed=TENGEN_OGS_CLIENT_ID");
    println!("cargo:rerun-if-changed={}", local.display());

    let id = std::env::var("TENGEN_OGS_CLIENT_ID")
        .ok()
        .or_else(|| std::fs::read_to_string(&local).ok().and_then(|text| setting(&text, "ogs_client_id")))
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty());
    if let Some(id) = id {
        println!("cargo:rustc-env=TENGEN_OGS_CLIENT_ID_RESOLVED={id}");
    }
}

/// Reads `key = "value"` from a simple TOML file, ignoring comments.
fn setting(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.split('#').next()?.trim();
        let (k, v) = line.split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"').to_string())
    })
}
