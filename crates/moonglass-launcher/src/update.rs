//! Updates (spec 4.5): Moonglass checks the latest GitHub release of its
//! repository, downloads the release file for this computer, checks it
//! against the SHA-256 digest GitHub publishes, and puts it in place of the
//! program. The new version runs from the next start, which Settings offers
//! right away.
//!
//! A release is a `v<major>.<minor>.<patch>` tag (the release workflow
//! checks it matches the version in `Cargo.toml`). Drafts and pre-releases
//! are never offered: GitHub's "latest release" leaves them out.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use moonglass_platform::{ReleaseAsset, UpdateTarget};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The running version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Larger than any release file; a bigger download is refused.
const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

/// `owner/name` of the repository releases come from.
fn repo() -> &'static str {
    env!("CARGO_PKG_REPOSITORY")
        .trim_end_matches('/')
        .trim_start_matches("https://github.com/")
}

/// A release newer than the running version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// "0.2.0", without the tag's `v`.
    pub version: String,
    /// The release's page on GitHub.
    pub page: String,
    pub asset: Asset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// Hex SHA-256, when GitHub published one.
    pub sha256: Option<String>,
}

/// What a check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    UpToDate,
    /// Nothing has been released yet.
    NoReleases,
    Available(Release),
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    /// "sha256:<hex>"; absent on files uploaded before GitHub added it.
    #[serde(default)]
    digest: Option<String>,
}

/// A version as (major, minor, patch, is a final release). A pre-release
/// suffix (`-beta.1`) sorts before the release it leads to.
type Version = (u64, u64, u64, bool);

pub fn parse_version(s: &str) -> Option<Version> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let (core, pre) = match s.split_once(['-', '+']) {
        Some((c, rest)) => (c, s.as_bytes()[c.len()] == b'-' && !rest.is_empty()),
        None => (s, false),
    };
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let v = (parts.next()??, parts.next()??, parts.next()??, !pre);
    parts.next().is_none().then_some(v)
}

/// Whether `candidate` is newer than `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("Moonglass/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .build()?)
}

/// Asks GitHub for the latest release.
pub async fn check() -> anyhow::Result<Check> {
    let target = moonglass_platform::update_target()?;
    let url = format!("https://api.github.com/repos/{}/releases/latest", repo());
    let resp = client()?
        .get(&url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .context("could not reach GitHub")?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(Check::NoReleases);
    }
    if resp.status() == reqwest::StatusCode::FORBIDDEN
        || resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        bail!("GitHub asked to wait; try again later");
    }
    let release: GhRelease = resp.error_for_status()?.json().await?;
    evaluate(release, CURRENT, target.asset)
}

fn evaluate(release: GhRelease, current: &str, want: ReleaseAsset) -> anyhow::Result<Check> {
    if release.draft || release.prerelease || !is_newer(&release.tag_name, current) {
        return Ok(Check::UpToDate);
    }
    let version = release.tag_name.trim_start_matches(['v', 'V']).to_string();
    let asset = release
        .assets
        .into_iter()
        .find(|a| a.name.ends_with(want.suffix()))
        .ok_or_else(|| anyhow!("version {version} has no download for this computer yet"))?;
    let sha256 = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .map(str::to_ascii_lowercase);
    Ok(Check::Available(Release {
        version,
        page: release.html_url,
        asset: Asset {
            name: asset.name,
            url: asset.browser_download_url,
            size: asset.size,
            sha256,
        },
    }))
}

/// Where a download is kept until it is installed: beside the program, so
/// it can be renamed into place.
fn download_path(target: &UpdateTarget) -> PathBuf {
    let mut name = target.file.as_os_str().to_owned();
    name.push(".download");
    PathBuf::from(name)
}

/// Downloads `release`, checks it, and installs it over the program.
/// `progress` is told the fraction downloaded (0 to 1) now and then.
pub async fn install(release: &Release, progress: impl Fn(f32)) -> anyhow::Result<()> {
    if cfg!(debug_assertions) {
        bail!("development builds are not updated; build or download a release");
    }
    let target = moonglass_platform::update_target()?;
    let dest = download_path(&target);
    let result = download(release, &dest, &progress).await;
    if let Err(e) = result {
        let _ = std::fs::remove_file(&dest);
        return Err(e);
    }
    let installed =
        tokio::task::spawn_blocking(move || moonglass_platform::install_update(&target, &dest))
            .await?;
    installed.map_err(|e| {
        anyhow!("could not replace the program: {e}. Download it from the release page instead")
    })?;
    tracing::info!(version = %release.version, "update installed");
    Ok(())
}

async fn download(release: &Release, dest: &Path, progress: &impl Fn(f32)) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;
    let asset = &release.asset;
    if asset.size > MAX_DOWNLOAD_BYTES {
        bail!("the download is unexpectedly large");
    }
    let mut resp = client()?
        .get(&asset.url)
        .send()
        .await
        .context("could not reach GitHub")?
        .error_for_status()?;
    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| format!("could not write next to the program ({})", dest.display()))?;
    let mut hash = Sha256::new();
    let mut got: u64 = 0;
    let mut shown = -1.0f32;
    while let Some(chunk) = tokio::time::timeout(Duration::from_secs(30), resp.chunk())
        .await
        .map_err(|_| anyhow!("the download stalled"))??
    {
        got += chunk.len() as u64;
        if got > MAX_DOWNLOAD_BYTES {
            bail!("the download is unexpectedly large");
        }
        hash.update(&chunk);
        file.write_all(&chunk).await?;
        let f = if asset.size > 0 {
            (got as f32 / asset.size as f32).min(1.0)
        } else {
            0.0
        };
        if f - shown >= 0.01 {
            shown = f;
            progress(f);
        }
    }
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    if asset.size > 0 && got != asset.size {
        bail!("the download was cut short");
    }
    let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    match &asset.sha256 {
        Some(want) if *want != digest => bail!("the download is damaged (checksum mismatch)"),
        Some(_) => {}
        None => tracing::warn!(asset = %asset.name, "release has no digest; size checked only"),
    }
    Ok(())
}

/// Starts the program again (the new version) once this process exits;
/// the caller quits right after.
pub fn restart() -> anyhow::Result<()> {
    let target = moonglass_platform::update_target()?;
    std::process::Command::new(&target.file)
        .arg(crate::AFTER_UPDATE_ARG)
        .spawn()
        .with_context(|| format!("could not start {}", target.file.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.2.0"), Some((0, 2, 0, true)));
        assert_eq!(parse_version("1.10.3-beta.1"), Some((1, 10, 3, false)));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(is_newer("v1.0.0", "1.0.0-rc.1"));
        assert!(!is_newer("v1.0.0-rc.1", "1.0.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("nightly", "0.1.0"));
    }

    fn release(tag: &str, names: &[&str]) -> GhRelease {
        GhRelease {
            tag_name: tag.into(),
            html_url: "https://github.com/o/r/releases/tag/x".into(),
            draft: false,
            prerelease: false,
            assets: names
                .iter()
                .map(|n| GhAsset {
                    name: (*n).into(),
                    browser_download_url: format!("https://x/{n}"),
                    size: 10,
                    digest: Some("sha256:ABCD".into()),
                })
                .collect(),
        }
    }

    #[test]
    fn picks_this_computers_file() {
        let names = [
            "moonglass-0.2.0-windows-x86_64.exe",
            "moonglass-0.2.0-windows-x86_64.zip",
            "moonglass-0.2.0-linux-x86_64.tar.gz",
            "moonglass-0.2.0-linux-x86_64.AppImage",
        ];
        for (want, name) in [
            (ReleaseAsset::WindowsExe, names[0]),
            (ReleaseAsset::LinuxTarball, names[2]),
            (ReleaseAsset::LinuxAppImage, names[3]),
        ] {
            let Check::Available(r) = evaluate(release("v0.2.0", &names), "0.1.0", want).unwrap()
            else {
                panic!("expected an update");
            };
            assert_eq!(r.version, "0.2.0");
            assert_eq!(r.asset.name, name);
            assert_eq!(r.asset.sha256.as_deref(), Some("abcd"));
        }
    }

    #[test]
    fn same_or_older_or_missing_file() {
        let names = ["moonglass-0.2.0-windows-x86_64.exe"];
        let r = evaluate(release("v0.2.0", &names), "0.2.0", ReleaseAsset::WindowsExe);
        assert_eq!(r.unwrap(), Check::UpToDate);
        let mut pre = release("v0.3.0", &names);
        pre.prerelease = true;
        assert_eq!(
            evaluate(pre, "0.2.0", ReleaseAsset::WindowsExe).unwrap(),
            Check::UpToDate
        );
        assert!(evaluate(
            release("v0.3.0", &names),
            "0.2.0",
            ReleaseAsset::LinuxAppImage
        )
        .is_err());
    }

    #[test]
    fn repository_slug() {
        assert_eq!(repo().split('/').count(), 2);
    }
}
