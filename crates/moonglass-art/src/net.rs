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
    if resp.content_length().is_some_and(|n| n > MAX_IMAGE_BYTES) {
        return Err(ArtError::TooLarge(url.to_string()));
    }
    let bytes = resp.bytes().await?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(ArtError::TooLarge(url.to_string()));
    }
    std::fs::create_dir_all(dir).map_err(|e| ArtError::io(dir, e))?;
    let path = dir.join(format!("{stem}.{}", image_extension(url)));
    std::fs::write(&path, &bytes).map_err(|e| ArtError::io(&path, e))?;
    Ok(path)
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
