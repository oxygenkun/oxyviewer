//! Manual release check backing the About settings panel.
//!
//! OxyViewer performs no background network I/O. A request is issued only when
//! the user explicitly asks for a release check, and the answer is advisory: the
//! app reports the newest published release and links to it, but never downloads
//! or installs anything from this code path.
//!
//! Every value that reaches the frontend is either a fixed project URL or comes
//! from the GitHub Releases API, so the About panel cannot be used to open an
//! arbitrary URL.

use oxy_domain::{AboutLink, UpdateStatus};
use semver::Version;
use serde::Deserialize;
use std::{io::Read, time::Duration};

pub const REPOSITORY_URL: &str = "https://github.com/oxygenkun/oxyviewer";
pub const RELEASES_URL: &str = "https://github.com/oxygenkun/oxyviewer/releases";
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/oxygenkun/oxyviewer/releases/latest";
const RELEASE_URL_PREFIX: &str = "https://github.com/oxygenkun/oxyviewer/releases/";
const LICENSE_PATH: &str = "/blob/main/LICENSE.md";

/// A release check must not hold the settings panel hostage on a slow network.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// GitHub responses are a few kilobytes; the cap keeps a misbehaving proxy from
/// streaming an unbounded body into memory.
const MAX_RESPONSE_BYTES: u64 = 256 * 1024;

#[derive(Debug, Deserialize)]
struct ReleasePayload {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
}

/// Queries the newest published (non-draft, non-prerelease) release.
pub fn check_for_updates(current_version: &str) -> Result<UpdateStatus, String> {
    let payload = fetch_latest_release()?;
    Ok(evaluate(current_version, &payload))
}

fn fetch_latest_release() -> Result<ReleasePayload, String> {
    let agent = ureq::config::Config::builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .build(),
        )
        .timeout_global(Some(REQUEST_TIMEOUT))
        .build()
        .new_agent();
    let mut response = agent
        .get(LATEST_RELEASE_API)
        .header("User-Agent", "OxyViewer release check")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(request_error)?;
    let mut body = String::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_RESPONSE_BYTES)
        .read_to_string(&mut body)
        .map_err(|error| format!("could not read the release response: {error}"))?;
    serde_json::from_str(&body).map_err(|error| format!("unreadable release response: {error}"))
}

fn request_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::StatusCode(404) => "this project has not published a release yet".to_owned(),
        ureq::Error::StatusCode(403 | 429) => {
            "the release check was rate limited; try again later".to_owned()
        }
        ureq::Error::StatusCode(code) => {
            format!("the release check was rejected with HTTP {code}")
        }
        ureq::Error::Timeout(_) => "the release check timed out".to_owned(),
        _ => format!("release check failed: {error}"),
    }
}

fn evaluate(current_version: &str, release: &ReleasePayload) -> UpdateStatus {
    let current_version = current_version.trim().to_owned();
    let latest_version = release
        .tag_name
        .trim()
        .trim_start_matches(['v', 'V'])
        .to_owned();
    UpdateStatus {
        update_available: is_newer(&current_version, &latest_version),
        current_version,
        latest_version,
        release_url: release.html_url.clone(),
        release_name: release
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned),
        published_at: release.published_at.clone(),
    }
}

fn is_newer(current: &str, latest: &str) -> bool {
    match (Version::parse(current), Version::parse(latest)) {
        (Ok(current), Ok(latest)) => latest > current,
        // Date-stamped or otherwise non-semver tags still surface a notice as
        // long as they differ from the running build.
        _ => !latest.is_empty() && latest != current,
    }
}

/// Resolves one About action to an openable URL.
///
/// Only fixed project destinations and GitHub release pages are accepted, so a
/// compromised or buggy frontend still cannot launch arbitrary URLs.
pub fn about_url(target: AboutLink, release_url: Option<&str>) -> Result<String, String> {
    match target {
        AboutLink::Repository => Ok(REPOSITORY_URL.to_owned()),
        AboutLink::Releases => match release_url {
            None => Ok(RELEASES_URL.to_owned()),
            Some(url) if url.starts_with(RELEASE_URL_PREFIX) => Ok(url.to_owned()),
            Some(_) => Err("the release page must be an OxyViewer release URL".to_owned()),
        },
        AboutLink::License => Ok(format!("{REPOSITORY_URL}{LICENSE_PATH}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str) -> ReleasePayload {
        ReleasePayload {
            tag_name: tag.to_owned(),
            html_url: format!("{RELEASES_URL}/tag/{tag}"),
            name: Some(format!("OxyViewer {tag}")),
            published_at: Some("2026-01-02T03:04:05Z".to_owned()),
        }
    }

    #[test]
    fn newer_release_is_reported_with_the_stripped_tag() {
        let status = evaluate("0.1.1", &release("v0.2.0"));

        assert!(status.update_available);
        assert_eq!(status.current_version, "0.1.1");
        assert_eq!(status.latest_version, "0.2.0");
        assert_eq!(status.release_name.as_deref(), Some("OxyViewer v0.2.0"));
        assert_eq!(status.published_at.as_deref(), Some("2026-01-02T03:04:05Z"));
    }

    #[test]
    fn equal_and_older_releases_report_no_update() {
        assert!(!evaluate("0.1.1", &release("v0.1.1")).update_available);
        assert!(!evaluate("0.1.1", &release("v0.1.0")).update_available);
    }

    #[test]
    fn prerelease_and_patch_order_follow_semver() {
        assert!(evaluate("0.1.1", &release("v0.1.2")).update_available);
        assert!(evaluate("0.1.1-beta.1", &release("v0.1.1")).update_available);
        assert!(!evaluate("0.2.0", &release("v0.2.0-rc.1")).update_available);
    }

    #[test]
    fn non_semver_tags_differing_from_the_build_report_an_update() {
        assert!(evaluate("0.1.1", &release("nightly-2026-01-02")).update_available);
        assert!(!evaluate("nightly-2026-01-02", &release("nightly-2026-01-02")).update_available);
    }

    #[test]
    fn about_links_accept_only_project_destinations() {
        assert_eq!(
            about_url(AboutLink::Repository, None).unwrap(),
            "https://github.com/oxygenkun/oxyviewer"
        );
        assert_eq!(
            about_url(AboutLink::Releases, None).unwrap(),
            "https://github.com/oxygenkun/oxyviewer/releases"
        );
        assert_eq!(
            about_url(
                AboutLink::Releases,
                Some(&format!("{RELEASES_URL}/tag/v0.2.0"))
            )
            .unwrap(),
            "https://github.com/oxygenkun/oxyviewer/releases/tag/v0.2.0"
        );
        assert!(about_url(AboutLink::Releases, Some("https://example.com/evil")).is_err());
        assert_eq!(
            about_url(AboutLink::License, None).unwrap(),
            "https://github.com/oxygenkun/oxyviewer/blob/main/LICENSE.md"
        );
    }
}
