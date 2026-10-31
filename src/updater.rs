//! Checks GitHub for a newer Snipshot release, and downloads and installs it.
//!
//! How an update is installed depends on how this copy was installed:
//! a standalone binary (release tarball) is replaced in place, the .deb is
//! installed through apt (asking for the admin password), and the Flatpak is
//! reinstalled through the host's `flatpak` command.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use eframe::egui;
use sha2::{Digest, Sha256};

/// The GitHub repository releases are published to, from `Cargo.toml`.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const CURRENT: &str = env!("CARGO_PKG_VERSION");
const CHECKSUMS: &str = "SHA256SUMS";

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateStatus {
    UpToDate,
    Available { version: String, url: String, assets: Vec<Asset> },
    Failed(String),
}

/// A file attached to a GitHub release.
#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
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
    let assets = release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            Some(Asset {
                name: a["name"].as_str()?.to_owned(),
                url: a["browser_download_url"].as_str()?.to_owned(),
                size: a["size"].as_u64().unwrap_or(0),
            })
        })
        .collect();

    Ok(if is_newer(tag, CURRENT) {
        UpdateStatus::Available { version: tag.trim_start_matches('v').to_owned(), url, assets }
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

/// How this copy of Snipshot was installed, which decides how to update it.
#[derive(Clone, Debug, PartialEq)]
pub enum Install {
    /// A standalone binary that we replace in place (the release tarball).
    Binary(PathBuf),
    /// The .deb package in /usr.
    Deb,
    /// The Flatpak, installed per user or system-wide.
    Flatpak { app_id: String, user: bool },
}

impl Install {
    pub fn detect() -> Result<Self, String> {
        if std::env::consts::ARCH != "x86_64" {
            return Err("Updates are only published for x86_64".to_owned());
        }
        if let Ok(app_id) = std::env::var("FLATPAK_ID") {
            // System-wide Flatpaks are deployed under /var/lib/flatpak.
            let info = fs::read_to_string("/.flatpak-info").unwrap_or_default();
            let user = !info.lines().any(|l| l.starts_with("app-path=/var/lib/flatpak/"));
            return Ok(Self::Flatpak { app_id, user });
        }
        let exe = std::env::current_exe().map_err(|e| format!("Can't find the Snipshot program: {e}"))?;
        if exe.components().any(|c| c.as_os_str() == "target") {
            return Err("This copy was built from source. Update it with git pull and cargo build.".to_owned());
        }
        if exe.starts_with("/usr/") {
            return Ok(Self::Deb);
        }
        Ok(Self::Binary(exe))
    }

    /// The release file to download for this kind of install.
    fn asset_suffix(&self) -> &'static str {
        match self {
            Self::Binary(_) => "-x86_64-linux.tar.gz",
            Self::Deb => "_amd64.deb",
            Self::Flatpak { .. } => "-x86_64.flatpak",
        }
    }

    /// Whether installing will ask for the admin password.
    pub fn asks_for_password(&self) -> bool {
        matches!(self, Self::Deb) || matches!(self, Self::Flatpak { user: false, .. })
    }

    /// Starts the updated copy of Snipshot. The caller then closes this one.
    pub fn relaunch(&self) -> Result<(), String> {
        let result = match self {
            Self::Binary(exe) => Command::new(exe).spawn(),
            Self::Deb => Command::new("/usr/bin/snipshot").spawn(),
            Self::Flatpak { app_id, .. } => {
                Command::new("flatpak-spawn").args(["--host", "flatpak", "run", app_id]).spawn()
            }
        };
        result.map(drop).map_err(|e| format!("Couldn't restart Snipshot: {e}"))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Progress {
    /// Fraction of the download done.
    Downloading(f32),
    Installing,
    Done,
    Failed(String),
}

/// Downloads, verifies and installs the update in the background, reporting
/// progress on the returned channel.
pub fn spawn_install(ctx: &egui::Context, install: Install, assets: Vec<Asset>) -> Receiver<Progress> {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let report = |p: Progress| {
            let _ = tx.send(p);
            ctx.request_repaint();
        };
        match download_and_install(&install, &assets, &report) {
            Ok(()) => report(Progress::Done),
            Err(e) => report(Progress::Failed(e)),
        }
    });
    rx
}

fn download_and_install(install: &Install, assets: &[Asset], report: &dyn Fn(Progress)) -> Result<(), String> {
    let suffix = install.asset_suffix();
    let asset = assets
        .iter()
        .find(|a| a.name.ends_with(suffix))
        .ok_or_else(|| format!("The release has no {suffix} file"))?;
    let sums = assets
        .iter()
        .find(|a| a.name == CHECKSUMS)
        .ok_or("The release has no checksums to verify the download with")?;

    let dir = dirs::cache_dir().ok_or("No cache folder to download to")?.join("snipshot").join("updates");
    fs::create_dir_all(&dir).map_err(|e| format!("Can't create {}: {e}", dir.display()))?;
    let file = dir.join(&asset.name);

    report(Progress::Downloading(0.0));
    let result = (|| {
        let digest = download(asset, &file, report)?;
        let expected = expected_checksum(sums, &asset.name)?;
        if digest != expected {
            return Err("The download is damaged (its checksum doesn't match). Try again.".to_owned());
        }
        report(Progress::Installing);
        match install {
            Install::Binary(exe) => replace_binary(&file, exe),
            Install::Deb => run_installer(Command::new("pkexec").args(["apt-get", "install", "-y", "--allow-downgrades"]).arg(&file)),
            Install::Flatpak { user, .. } => run_installer(
                Command::new("flatpak-spawn")
                    .args(["--host", "flatpak", "install", "--reinstall", "-y", "--noninteractive"])
                    .arg(if *user { "--user" } else { "--system" })
                    .arg(&file),
            ),
        }
    })();
    let _ = fs::remove_file(&file);
    result
}

fn download_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_per_read(Some(Duration::from_secs(60)))
        .build()
        .into()
}

/// Downloads to `path`, returning the SHA-256 of the file as lowercase hex.
fn download(asset: &Asset, path: &Path, report: &dyn Fn(Progress)) -> Result<String, String> {
    let mut response = download_agent()
        .get(&asset.url)
        .header("User-Agent", format!("snipshot/{CURRENT}"))
        .call()
        .map_err(|e| format!("Couldn't download the update: {e}"))?;
    let total = response.body().content_length().unwrap_or(asset.size).max(1);
    let mut reader = response.body_mut().as_reader();
    let mut out = File::create(path).map_err(|e| format!("Can't save the download: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 64 * 1024];
    let (mut done, mut last_report) = (0u64, 0.0f32);
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("The download stopped: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n]).map_err(|e| format!("Can't save the download: {e}"))?;
        done += n as u64;
        let fraction = (done as f32 / total as f32).min(1.0);
        if fraction - last_report >= 0.01 {
            last_report = fraction;
            report(Progress::Downloading(fraction));
        }
    }
    out.flush().map_err(|e| format!("Can't save the download: {e}"))?;
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Looks up `name` in the release's SHA256SUMS file.
fn expected_checksum(sums: &Asset, name: &str) -> Result<String, String> {
    let mut response = download_agent()
        .get(&sums.url)
        .header("User-Agent", format!("snipshot/{CURRENT}"))
        .call()
        .map_err(|e| format!("Couldn't download the checksums: {e}"))?;
    let text = response.body_mut().read_to_string().map_err(|e| e.to_string())?;
    checksum_for(&text, name).ok_or_else(|| format!("{name} isn't listed in {CHECKSUMS}"))
}

fn checksum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then(|| hash.to_ascii_lowercase())
    })
}

/// Swaps in the `snipshot` binary from a release tarball. The running
/// program keeps working: renaming over it doesn't touch the open file.
fn replace_binary(tarball: &Path, exe: &Path) -> Result<(), String> {
    let dir = exe.parent().ok_or("Can't find the folder Snipshot is in")?;
    let staged = dir.join(".snipshot-update");
    let file = File::open(tarball).map_err(|e| e.to_string())?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(BufReader::new(file)));
    let mut found = false;
    for entry in archive.entries().map_err(|e| format!("The download isn't a valid archive: {e}"))? {
        let mut entry = entry.map_err(|e| format!("The download isn't a valid archive: {e}"))?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        // The binary sits at snipshot-<version>-x86_64-linux/snipshot.
        if path.components().count() == 2 && path.file_name().is_some_and(|n| n == "snipshot") {
            let mut out = File::create(&staged)
                .map_err(|e| format!("Can't write to {} ({e}). Install the update by hand.", dir.display()))?;
            std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
            found = true;
            break;
        }
    }
    if !found {
        return Err("The archive doesn't contain the Snipshot program".to_owned());
    }
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    fs::rename(&staged, exe).map_err(|e| {
        let _ = fs::remove_file(&staged);
        format!("Can't replace {}: {e}", exe.display())
    })
}

/// Runs a package manager, turning its exit status and output into an error.
fn run_installer(command: &mut Command) -> Result<(), String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command.output().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("{program} isn't available, so the update can't be installed here"),
        _ => format!("Couldn't run {program}: {e}"),
    })?;
    if output.status.success() {
        return Ok(());
    }
    // pkexec exits with 126 when the password prompt is dismissed.
    if program == "pkexec" && matches!(output.status.code(), Some(126 | 127)) {
        return Err("The installation was cancelled".to_owned());
    }
    let last_line = BufReader::new(&output.stderr[..])
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty())
        .last()
        .unwrap_or_else(|| format!("{program} failed ({})", output.status));
    Err(last_line)
}

#[cfg(test)]
mod tests {
    use super::{checksum_for, is_newer};

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(!is_newer("nightly", "0.1.0"));
    }

    #[test]
    fn finds_checksums() {
        let sums = "ABC123  snipshot_0.2.0_amd64.deb\ndef456 *snipshot-0.2.0-x86_64.flatpak\n";
        assert_eq!(checksum_for(sums, "snipshot_0.2.0_amd64.deb").as_deref(), Some("abc123"));
        assert_eq!(checksum_for(sums, "snipshot-0.2.0-x86_64.flatpak").as_deref(), Some("def456"));
        assert_eq!(checksum_for(sums, "other"), None);
    }
}
