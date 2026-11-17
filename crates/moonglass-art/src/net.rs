//! Image downloads shared by the network artwork sources.

use std::path::{Path, PathBuf};

use crate::ArtError;

/// Larger than any cover or hero image; a response beyond it is refused.
const MAX_IMAGE_BYTES: u64 = 40 * 1024 * 1024;

pub(crate) fn client() -> Result<reqwest::Client, ArtError> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("Moonglass/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(20))
        .build()?)
}

/// Percent-encodes text for use as one URL path segment.
pub(crate) fn encode_path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// File extension for a downloaded image, from its URL.
fn image_extension(url: &str) -> &str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit('.')
        .next()
        .filter(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp"
            )
        })
        .unwrap_or("png")
}

/// Saves the image at `url` as `<dir>/<stem>.<ext>`. No credentials are
/// sent: image hosts are not the API.
pub(crate) async fn download(
    http: &reqwest::Client,
    url: &str,
    dir: &Path,
    stem: &str,
) -> Result<PathBuf, ArtError> {
    let resp = http.get(url).send().await?.error_for_status()?;
    let path = dir.join(format!("{stem}.{}", image_extension(url)));
    save(resp, url, &path).await?;
    Ok(path)
}

/// Like [`download`], for hosts where a missing image is an ordinary
/// answer: `Ok(None)` on 404, or when the response is not an image (some
/// hosts answer a miss with an HTML page). Saved as `<dir>/<stem>.<ext>`.
pub(crate) async fn download_if_found(
    http: &reqwest::Client,
    url: &str,
    dir: &Path,
    stem: &str,
    ext: &str,
) -> Result<Option<PathBuf>, ArtError> {
    let resp = http.get(url).send().await?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let resp = resp.error_for_status()?;
    let is_image = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.starts_with("image/"));
    if !is_image {
        return Ok(None);
    }
    let path = dir.join(format!("{stem}.{ext}"));
    save(resp, url, &path).await?;
    Ok(Some(path))
}

/// Writes a response's body to `path`, refusing one over the size limit.
async fn save(resp: reqwest::Response, url: &str, path: &Path) -> Result<(), ArtError> {
    if resp.content_length().is_some_and(|n| n > MAX_IMAGE_BYTES) {
        return Err(ArtError::TooLarge(url.to_string()));
    }
    let bytes = resp.bytes().await?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(ArtError::TooLarge(url.to_string()));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| ArtError::io(dir, e))?;
    }
    std::fs::write(path, &bytes).map_err(|e| ArtError::io(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions() {
        assert_eq!(image_extension("https://x/a/cover.JPG"), "JPG");
        assert_eq!(image_extension("https://x/a/cover.webp?w=600"), "webp");
        assert_eq!(image_extension("https://x/a/cover"), "png");
    }
}
