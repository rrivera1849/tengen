//! Where KataGo lives on this Mac, and getting it there.
//!
//! KataGo publishes no Mac binaries, so Tengen's own CI builds KataGo with its
//! Metal backend and publishes it, with a network, as a GitHub release of
//! Tengen (see `.github/workflows/katago-runtime.yml`). A `manifest.json` in
//! that release lists each file with its SHA-256, checked after download.
//!
//! Lookup order:
//! 1. `TENGEN_KATAGO` and `TENGEN_KATAGO_MODEL`, for development.
//! 2. Tengen's own copy in Application Support.
//! 3. A Homebrew `katago`, still using the network from Tengen's folder.

use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::AiError;

pub const MANIFEST_URL: &str =
    "https://github.com/rrivera1849/tengen/releases/download/katago-runtime/manifest.json";

const HOMEBREW: [&str; 2] = ["/opt/homebrew/bin/katago", "/usr/local/bin/katago"];

#[derive(Debug, Clone, PartialEq)]
pub struct Runtime {
    pub binary: PathBuf,
    pub model: PathBuf,
    pub config: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub files: Vec<ManifestFile>,
}

#[derive(Debug, Deserialize)]
pub struct ManifestFile {
    /// `katago` or `model.bin.gz`.
    pub name: String,
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub executable: bool,
}

/// Finds a usable KataGo and network, writing the analysis config next to them.
pub fn locate(dir: &Path) -> Option<Runtime> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from).filter(|p| p.exists());
    let binary = env("TENGEN_KATAGO")
        .or_else(|| Some(dir.join("katago")).filter(|p| p.exists()))
        .or_else(|| HOMEBREW.iter().map(PathBuf::from).find(|p| p.exists()))?;
    let model = env("TENGEN_KATAGO_MODEL").or_else(|| Some(dir.join("model.bin.gz")).filter(|p| p.exists()))?;
    let config = write_config(dir).ok()?;
    Some(Runtime { binary, model, config })
}

/// The analysis settings Tengen uses. Scores are always from Black's side.
pub fn config_text(log_dir: &Path, threads: usize) -> String {
    format!(
        "# Written by Tengen; changes are overwritten.\n\
         logDir = {}\n\
         reportAnalysisWinratesAs = BLACK\n\
         maxVisits = 400\n\
         numAnalysisThreads = 2\n\
         numSearchThreadsPerAnalysisThread = {threads}\n\
         nnMaxBatchSize = 64\n\
         nnCacheSizePowerOfTwo = 21\n\
         nnMutexPoolSizePowerOfTwo = 16\n\
         nnRandomize = true\n",
        log_dir.display()
    )
}

fn write_config(dir: &Path) -> std::io::Result<PathBuf> {
    let logs = dir.join("logs");
    std::fs::create_dir_all(&logs)?;
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 16);
    let path = dir.join("analysis.cfg");
    std::fs::write(&path, config_text(&logs, threads))?;
    Ok(path)
}

/// Downloads KataGo and its network into `dir`, checking every file.
/// `progress(done_bytes, total_bytes)` is called as data arrives; the total
/// may be 0 until the server says how big the files are.
pub async fn install(dir: &Path, mut progress: impl FnMut(u64, u64)) -> Result<Runtime, AiError> {
    let http = reqwest::Client::builder()
        .user_agent(concat!("Tengen/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| AiError::Download(e.to_string()))?;
    let manifest: Manifest = http
        .get(MANIFEST_URL)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| AiError::Download(format!("couldn't read the KataGo manifest: {e}")))?
        .json()
        .await
        .map_err(|e| AiError::Download(format!("the KataGo manifest is malformed: {e}")))?;
    tokio::fs::create_dir_all(dir).await.map_err(|e| AiError::Download(e.to_string()))?;

    // Ask for sizes first so progress covers everything.
    let mut total = 0;
    for f in &manifest.files {
        if let Ok(r) = http.head(&f.url).send().await {
            total += r.content_length().unwrap_or(0);
        }
    }
    let mut done = 0;
    for f in &manifest.files {
        if f.name.contains('/') || f.name.contains("..") {
            return Err(AiError::Download(format!("refusing a file named {}", f.name)));
        }
        let target = dir.join(&f.name);
        let partial = dir.join(format!("{}.part", f.name));
        let resp = http
            .get(&f.url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| AiError::Download(format!("{}: {e}", f.name)))?;
        let mut file = tokio::fs::File::create(&partial).await.map_err(|e| AiError::Download(e.to_string()))?;
        let mut hash = Sha256::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| AiError::Download(format!("{}: {e}", f.name)))?;
            hash.update(&chunk);
            file.write_all(&chunk).await.map_err(|e| AiError::Download(e.to_string()))?;
            done += chunk.len() as u64;
            progress(done, total);
        }
        file.flush().await.map_err(|e| AiError::Download(e.to_string()))?;
        drop(file);
        let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if !digest.eq_ignore_ascii_case(f.sha256.trim()) {
            let _ = tokio::fs::remove_file(&partial).await;
            return Err(AiError::Download(format!("{} didn't match its checksum", f.name)));
        }
        #[cfg(unix)]
        if f.executable {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| AiError::Download(e.to_string()))?;
        }
        tokio::fs::rename(&partial, &target).await.map_err(|e| AiError::Download(e.to_string()))?;
    }
    let _ = &manifest.version;
    locate(dir).ok_or_else(|| AiError::Download("the download finished but KataGo is still missing".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_reports_from_blacks_side() {
        let text = config_text(Path::new("/Users/a b/Library/Application Support/Tengen/katago/logs"), 8);
        assert!(text.contains("reportAnalysisWinratesAs = BLACK"));
        assert!(text.contains("numSearchThreadsPerAnalysisThread = 8"));
    }

    #[test]
    fn reads_a_manifest() {
        let m: Manifest = serde_json::from_str(
            r#"{"version":"1.16.5","files":[{"name":"katago","url":"https://x/katago","sha256":"ab","executable":true},
                {"name":"model.bin.gz","url":"https://x/m","sha256":"cd"}]}"#,
        )
        .unwrap();
        assert!(m.files[0].executable && !m.files[1].executable);
    }
}
