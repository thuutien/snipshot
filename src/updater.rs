//! Checks GitHub for a newer Snipshot release.

use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use eframe::egui;

/// The GitHub repository releases are published to, from `Cargo.toml`.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateStatus {
    UpToDate,
    Available { version: String, url: String },
    Failed(String),
}

/// Checks in the background; the result arrives on the returned channel.
pub fn spawn_check(ctx: &egui::Context) -> Receiver<UpdateStatus> {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let status = check().unwrap_or_else(UpdateStatus::Failed);
        let _ = tx.send(status);
        ctx.request_repaint();
    });
    rx
}

fn check() -> Result<UpdateStatus, String> {
    let repo = REPOSITORY
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .strip_prefix("https://github.com/")
        .filter(|r| r.split('/').count() == 2)
        .ok_or("No update source is configured")?;

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent
        .get(format!("https://api.github.com/repos/{repo}/releases/latest"))
        .header("User-Agent", format!("snipshot/{CURRENT}"))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("Couldn't reach GitHub: {e}"))?;
    match response.status().as_u16() {
        200 => {}
        404 => return Err("No releases have been published yet".to_owned()),
        code => return Err(format!("GitHub answered with HTTP {code}")),
    }
    let body = response.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let release: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let tag = release["tag_name"].as_str().ok_or("The release has no version tag")?;
    let url = release["html_url"].as_str().unwrap_or(REPOSITORY).to_owned();

    Ok(if is_newer(tag, CURRENT) {
        UpdateStatus::Available { version: tag.trim_start_matches('v').to_owned(), url }
    } else {
        UpdateStatus::UpToDate
    })
}

/// Compares `major.minor.patch` versions, ignoring a leading `v` and any
/// pre-release suffix.
fn is_newer(candidate: &str, current: &str) -> bool {
    fn parse(v: &str) -> Option<(u64, u64, u64)> {
        let core = v.trim().trim_start_matches('v').split(['-', '+']).next()?;
        let mut parts = core.split('.').map(|p| p.parse::<u64>());
        Some((parts.next()?.ok()?, parts.next().unwrap_or(Ok(0)).ok()?, parts.next().unwrap_or(Ok(0)).ok()?))
    }
    matches!((parse(candidate), parse(current)), (Some(a), Some(b)) if a > b)
}

pub fn current_version() -> &'static str {
    CURRENT
}

/// Opens a web page in the default browser.
pub fn open_url(url: &str) {
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(!is_newer("nightly", "0.1.0"));
    }
}
