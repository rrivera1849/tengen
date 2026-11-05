//! A quiet check for a newer Tengen on GitHub, once per launch.

use serde::{Deserialize, Serialize};

/// Where Tengen is published.
pub const REPO: &str = "rrivera1849/tengen";

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub html_url: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Update {
    pub version: String,
    pub url: String,
}

/// `v1.2.3` or `1.2.3` as numbers; anything else (like `katago-runtime`) is None.
pub fn parse_version(tag: &str) -> Option<(u64, u64, u64)> {
    let v = tag.strip_prefix('v').unwrap_or(tag);
    let mut parts = v.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// The newest published release newer than `current`, if there is one.
pub fn newest(current: &str, releases: &[Release]) -> Option<Update> {
    let now = parse_version(current)?;
    releases
        .iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter(|r| r.html_url.starts_with(&release_page_prefix()))
        .filter_map(|r| parse_version(&r.tag_name).map(|v| (v, r)))
        .filter(|(v, _)| *v > now)
        .max_by_key(|(v, _)| *v)
        .map(|((a, b, c), r)| Update {
            version: format!("{a}.{b}.{c}"),
            url: r.html_url.clone(),
        })
}

pub fn release_page_prefix() -> String {
    format!("https://github.com/{REPO}/releases/")
}

/// Asks GitHub for Tengen's releases. Quiet on any failure: this is only a nudge.
pub async fn check() -> Option<Update> {
    let http = reqwest::Client::builder()
        .user_agent(concat!("Tengen/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;
    let releases: Vec<Release> = http
        .get(format!(
            "https://api.github.com/repos/{REPO}/releases?per_page=20"
        ))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    newest(env!("CARGO_PKG_VERSION"), &releases)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(tag: &str) -> Release {
        Release {
            tag_name: tag.into(),
            html_url: format!("https://github.com/{REPO}/releases/tag/{tag}"),
            draft: false,
            prerelease: false,
        }
    }

    #[test]
    fn reads_versions() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.10"), Some((1, 10, 0)));
        assert_eq!(parse_version("katago-runtime"), None);
        assert_eq!(parse_version("v1.2.3.4"), None);
        assert_eq!(parse_version("v1.2.3-beta"), None);
    }

    #[test]
    fn finds_the_newest_newer_release() {
        let all = vec![
            rel("katago-runtime"),
            rel("v1.0.0"),
            rel("v1.2.0"),
            rel("v1.10.1"),
            rel("v0.9.0"),
        ];
        let u = newest("1.2.0", &all).unwrap();
        assert_eq!(u.version, "1.10.1", "compared as numbers, not text");
        assert!(u.url.ends_with("/tag/v1.10.1"));
        assert_eq!(newest("1.10.1", &all), None, "already up to date");
        assert_eq!(newest("2.0.0", &all), None, "ahead of the releases");
    }

    #[test]
    fn skips_drafts_prereleases_and_odd_links() {
        let mut draft = rel("v2.0.0");
        draft.draft = true;
        let mut pre = rel("v1.9.0");
        pre.prerelease = true;
        let mut elsewhere = rel("v1.8.0");
        elsewhere.html_url = "https://example.com/v1.8.0".into();
        assert_eq!(newest("1.0.0", &[draft, pre, elsewhere]), None);
    }
}
