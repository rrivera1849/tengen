//! Finds Tengen's OGS client id at build time, so it never has to be edited
//! into the source. The first one set wins:
//!
//! 1. the `TENGEN_OGS_CLIENT_ID` environment variable;
//! 2. `ogs_client_id = "…"` in `tengen.local.toml` at the repository root,
//!    a file git ignores, for your own builds;
//! 3. `ogs_client_id = "…"` in `tengen.toml`, Tengen's shared registration.

use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let files = [root.join("tengen.local.toml"), root.join("tengen.toml")];
    println!("cargo:rerun-if-env-changed=TENGEN_OGS_CLIENT_ID");
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
    }

    let from_files = files.iter().filter_map(|f| {
        std::fs::read_to_string(f)
            .ok()
            .and_then(|text| setting(&text, "ogs_client_id"))
    });
    let id = std::env::var("TENGEN_OGS_CLIENT_ID")
        .ok()
        .into_iter()
        .chain(from_files)
        .map(|id| id.trim().to_string())
        .find(|id| !id.is_empty());
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
