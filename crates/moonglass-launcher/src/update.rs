//! Updates (spec 4.5): Moonglass checks the latest GitHub release of its
//! repository, downloads the release file for this computer, checks it
//! against the SHA-256 digest GitHub publishes, and puts it in place of the
//! program. The new version runs from the next start, which Settings offers
//! right away.
//!
//! A release is tagged `v` + the version in `Cargo.toml` (`v0.1.0`,
//! `v0.2.0-beta.1`); the release workflow makes the tag. Versions compare
//! as semantic versions. A beta (any version with a `-` suffix) is offered
//! only to a beta: someone on a final release gets final releases only.
//! Drafts are never offered.

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

/// A semantic version: `1.2.3`, `1.2.3-beta.2`. Build metadata (`+...`)
/// is ignored, as semver says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    core: (u64, u64, u64),
    /// Empty for a final release.
    pre: Vec<PreIdent>,
}

/// One dot-separated part of a pre-release suffix. Numbers compare as
/// numbers and sort before words (`beta.2` < `beta.10`, `1` < `beta`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PreIdent {
    Num(u64),
    Text(String),
}

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        let s = s.split('+').next().unwrap_or(s);
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) if !p.is_empty() => (c, Some(p)),
            Some(_) => return None,
            None => (s, None),
        };
        let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
        let core = (parts.next()??, parts.next()??, parts.next()??);
        if parts.next().is_some() {
            return None;
        }
        let pre = match pre {
            None => Vec::new(),
            Some(p) => p
                .split('.')
                .map(|i| match i.parse::<u64>() {
                    _ if i.is_empty() => None,
                    Ok(n) => Some(PreIdent::Num(n)),
                    Err(_) => Some(PreIdent::Text(i.to_ascii_lowercase())),
                })
                .collect::<Option<_>>()?,
        };
        Some(Self { core, pre })
    }

    /// A beta, release candidate or the like.
    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        self.core.cmp(&other.core).then_with(|| {
            // A pre-release comes before the release it leads to.
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Whether `candidate` is newer than `current`.
#[cfg(test)]
fn is_newer(candidate: &str, current: &str) -> bool {
    match (Version::parse(candidate), Version::parse(current)) {
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

/// Asks GitHub for the newest release this version should be offered.
pub async fn check() -> anyhow::Result<Check> {
    let target = moonglass_platform::update_target()?;
    // The most recent releases, newest first, betas included ("latest"
    // would leave betas out).
    let url = format!(
        "https://api.github.com/repos/{}/releases?per_page=30",
        repo()
    );
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
    let releases: Vec<GhRelease> = resp.error_for_status()?.json().await?;
    evaluate(releases, CURRENT, target.asset)
}

fn evaluate(releases: Vec<GhRelease>, current: &str, want: ReleaseAsset) -> anyhow::Result<Check> {
    let current = Version::parse(current).ok_or_else(|| anyhow!("bad version {current:?}"))?;
    let published: Vec<(Version, GhRelease)> = releases
        .into_iter()
        .filter(|r| !r.draft)
        .filter_map(|r| Some((Version::parse(&r.tag_name)?, r)))
        .collect();
    if published.is_empty() {
        return Ok(Check::NoReleases);
    }
    let Some((newest, release)) = published
        .into_iter()
        .filter(|(v, r)| current.is_prerelease() || !(v.is_prerelease() || r.prerelease))
        .max_by(|a, b| a.0.cmp(&b.0))
    else {
        return Ok(Check::UpToDate);
    };
    if newest <= current {
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
        let v = |s| Version::parse(s).unwrap();
        assert!(v("v0.1.0-beta.1").is_prerelease());
        assert!(!v("1.2.3+build.5").is_prerelease());
        assert_eq!(Version::parse("1.2"), None);
        assert_eq!(Version::parse("1.2.3.4"), None);
        assert_eq!(Version::parse("1.2.3-"), None);
        assert_eq!(Version::parse("nightly"), None);
        // In semver order.
        let order = [
            "0.1.0-alpha",
            "0.1.0-beta.1",
            "0.1.0-beta.2",
            "0.1.0-beta.10",
            "0.1.0-rc.1",
            "0.1.0",
            "0.1.1-beta.1",
            "0.1.1",
            "0.10.0",
            "1.0.0",
        ];
        for pair in order.windows(2) {
            assert!(is_newer(pair[1], pair[0]), "{} > {}", pair[1], pair[0]);
            assert!(!is_newer(pair[0], pair[1]), "{} < {}", pair[0], pair[1]);
        }
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.1.0+x", "0.1.0"));
    }

    fn release(tag: &str, names: &[&str]) -> GhRelease {
        GhRelease {
            tag_name: tag.into(),
            html_url: "https://github.com/o/r/releases/tag/x".into(),
            draft: false,
            prerelease: tag.contains('-'),
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

    const WIN: &[&str] = &["moonglass-x-windows-x86_64.exe"];

    fn offered(releases: Vec<GhRelease>, current: &str) -> Option<String> {
        match evaluate(releases, current, ReleaseAsset::WindowsExe).unwrap() {
            Check::Available(r) => Some(r.version),
            _ => None,
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
            let Check::Available(r) =
                evaluate(vec![release("v0.2.0", &names)], "0.1.0", want).unwrap()
            else {
                panic!("expected an update");
            };
            assert_eq!(r.version, "0.2.0");
            assert_eq!(r.asset.name, name);
            assert_eq!(r.asset.sha256.as_deref(), Some("abcd"));
        }
    }

    #[test]
    fn betas_go_to_betas_only() {
        let all = || {
            vec![
                release("v0.1.0-beta.3", WIN),
                release("v0.1.0-beta.2", WIN),
                release("v0.0.9", WIN),
            ]
        };
        assert_eq!(
            offered(all(), "0.1.0-beta.1").as_deref(),
            Some("0.1.0-beta.3")
        );
        assert_eq!(offered(all(), "0.1.0-beta.3"), None);
        // A final release never moves to a beta.
        assert_eq!(offered(all(), "0.0.8").as_deref(), Some("0.0.9"));
        assert_eq!(offered(all(), "0.0.9"), None);
        // A beta moves on to the final release.
        let mut with_final = all();
        with_final.push(release("v0.1.0", WIN));
        assert_eq!(
            offered(with_final, "0.1.0-beta.3").as_deref(),
            Some("0.1.0")
        );
    }

    #[test]
    fn nothing_released_or_missing_file() {
        let none = |r| evaluate(r, "0.1.0", ReleaseAsset::WindowsExe).unwrap();
        assert_eq!(none(vec![]), Check::NoReleases);
        let mut draft = release("v0.2.0", WIN);
        draft.draft = true;
        assert_eq!(none(vec![draft]), Check::NoReleases);
        let linux = evaluate(
            vec![release("v0.3.0", WIN)],
            "0.2.0",
            ReleaseAsset::LinuxAppImage,
        );
        assert!(linux.is_err());
    }

    #[test]
    fn repository_slug() {
        assert_eq!(repo().split('/').count(), 2);
    }
}
