//! Bounded release checks and verified, same-directory executable replacement.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use yoke_core::store::Store;

#[cfg(test)]
mod tests;
mod transport;

const REPOSITORY: &str = "mrx-arafat/yoke";
const LATEST_URL: &str = "https://api.github.com/repos/mrx-arafat/yoke/releases/latest";
pub const LATEST_VERSION_KEY: &str = "update.latest_version";
const METADATA_LIMIT: usize = 1024 * 1024;
const BINARY_LIMIT: usize = 64 * 1024 * 1024;
const CHECKSUM_LIMIT: usize = 256;

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: usize,
    state: String,
}

/// Check GitHub in at most one bounded request, recording only validated metadata.
pub fn check_latest(store: &Store) -> Result<String> {
    check_with(store, &mut transport::check)
}

/// Check for a release or atomically install a newer release for this build target.
pub fn run(check_only: bool, store: &Store) -> Result<()> {
    // Release builds carry the same generated notices as the archive, including
    // when the updater distributes only the raw executable.
    #[cfg(yoke_release_notices)]
    {
        std::hint::black_box(include_bytes!(env!("YOKE_RELEASE_NOTICES")));
    }
    if check_only {
        println!("latest_version: {}", check_latest(store)?);
        return Ok(());
    }
    let release = read_release(&mut transport::download)?;
    let current = version(env!("CARGO_PKG_VERSION"))?;
    if version(&release.tag_name)? <= current {
        println!("update: already current ({})", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let executable = std::env::current_exe().context("locating current executable")?;
    install_release(&release, &executable, target()?, &mut transport::download)?;
    // The rename is the commit point. Do not report a later bookkeeping failure
    // as an update failure after the executable has already been replaced.
    println!("updated_version: {}", release.tag_name);
    Ok(())
}

fn check_with(
    store: &Store,
    fetch: &mut impl FnMut(&str, usize) -> Result<Vec<u8>>,
) -> Result<String> {
    let release = read_release(fetch)?;
    store.set_kv(LATEST_VERSION_KEY, &release.tag_name)?;
    Ok(release.tag_name)
}

fn read_release(fetch: &mut impl FnMut(&str, usize) -> Result<Vec<u8>>) -> Result<Release> {
    let bytes = fetch(LATEST_URL, METADATA_LIMIT)?;
    ensure!(
        bytes.len() <= METADATA_LIMIT,
        "release metadata exceeds size limit"
    );
    let release: Release =
        serde_json::from_slice(&bytes).context("invalid GitHub release metadata")?;
    version(&release.tag_name)?;
    ensure!(
        !release.draft && !release.prerelease,
        "latest release is not stable"
    );
    for asset in &release.assets {
        ensure!(
            !asset.name.is_empty()
                && asset.name.len() <= 128
                && !asset.name.contains("..")
                && asset
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
            "invalid release asset name"
        );
        ensure!(
            asset.browser_download_url
                == format!(
                    "https://github.com/{REPOSITORY}/releases/download/{}/{}",
                    release.tag_name, asset.name
                ),
            "unexpected release asset URL"
        );
    }
    Ok(release)
}

pub(crate) fn version(tag: &str) -> Result<(u64, u64, u64)> {
    let value = tag.strip_prefix('v').unwrap_or(tag);
    ensure!(value.len() <= 64, "invalid release version");
    let parts: Vec<_> = value.split('.').collect();
    ensure!(
        parts.len() == 3,
        "expected stable major.minor.patch version"
    );
    let mut numbers = [0; 3];
    for (index, part) in parts.iter().enumerate() {
        ensure!(
            !part.is_empty()
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0')),
            "invalid release version"
        );
        numbers[index] = part
            .parse()
            .context("release version component too large")?;
    }
    Ok((numbers[0], numbers[1], numbers[2]))
}

fn target() -> Result<&'static str> {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("aarch64", "macos") => Ok("aarch64-apple-darwin"),
        ("x86_64", "macos") => Ok("x86_64-apple-darwin"),
        ("x86_64", "linux") if cfg!(target_env = "gnu") => Ok("x86_64-unknown-linux-gnu"),
        ("aarch64", "linux") if cfg!(target_env = "gnu") => Ok("aarch64-unknown-linux-gnu"),
        _ => bail!("self-update is unsupported for this build target"),
    }
}

fn asset<'a>(release: &'a Release, name: &str, limit: usize) -> Result<&'a Asset> {
    let mut matches = release.assets.iter().filter(|asset| asset.name == name);
    let asset = matches
        .next()
        .with_context(|| format!("release is missing {name}"))?;
    ensure!(matches.next().is_none(), "duplicate release asset {name}");
    ensure!(asset.state == "uploaded", "release asset is not uploaded");
    ensure!(
        asset.size > 0 && asset.size <= limit,
        "invalid asset size for {name}"
    );
    let expected = format!(
        "https://github.com/{REPOSITORY}/releases/download/{}/{name}",
        release.tag_name
    );
    ensure!(
        asset.browser_download_url == expected,
        "unexpected release asset URL"
    );
    Ok(asset)
}

fn install_release(
    release: &Release,
    executable: &Path,
    target: &str,
    fetch: &mut impl FnMut(&str, usize) -> Result<Vec<u8>>,
) -> Result<()> {
    version(&release.tag_name)?;
    ensure!(
        matches!(
            target,
            "aarch64-apple-darwin"
                | "x86_64-apple-darwin"
                | "x86_64-unknown-linux-gnu"
                | "aarch64-unknown-linux-gnu"
        ),
        "unsupported release target"
    );
    let name = format!("yoke-{target}");
    let binary = asset(release, &name, BINARY_LIMIT)?;
    let checksum = asset(release, &format!("{name}.sha256"), CHECKSUM_LIMIT)?;
    let checksum_bytes = fetch(&checksum.browser_download_url, CHECKSUM_LIMIT)?;
    ensure!(
        checksum_bytes.len() == checksum.size,
        "checksum download size mismatch"
    );
    let digest = parse_checksum(&checksum_bytes, &name)?;
    let bytes = fetch(&binary.browser_download_url, BINARY_LIMIT)?;
    ensure!(bytes.len() == binary.size, "binary download size mismatch");
    ensure!(
        format!("{:x}", Sha256::digest(&bytes)) == digest,
        "release checksum mismatch"
    );
    replace(executable, &bytes)
}

fn parse_checksum(bytes: &[u8], name: &str) -> Result<String> {
    ensure!(bytes.len() <= CHECKSUM_LIMIT, "checksum exceeds size limit");
    let text = std::str::from_utf8(bytes).context("checksum is not UTF-8")?;
    let fields: Vec<_> = text.split_whitespace().collect();
    ensure!(
        fields.len() == 2 && fields[1] == name,
        "invalid checksum filename or record"
    );
    ensure!(
        fields[0].len() == 64 && fields[0].bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid sha256"
    );
    Ok(fields[0].to_ascii_lowercase())
}

fn replace(executable: &Path, bytes: &[u8]) -> Result<()> {
    let original = fs::symlink_metadata(executable).context("reading installed executable")?;
    ensure!(
        original.is_file(),
        "executable must be a regular file, not a symlink"
    );
    let parent = executable
        .parent()
        .context("executable has no parent directory")?;
    let mut staged =
        tempfile::NamedTempFile::new_in(parent).context("staging update beside executable")?;
    staged
        .write_all(bytes)
        .context("writing staged executable")?;
    // Drop setuid/setgid and other special bits, preserving ordinary access bits.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        staged
            .as_file()
            .set_permissions(fs::Permissions::from_mode(
                original.permissions().mode() & 0o777,
            ))?;
    }
    staged
        .as_file()
        .sync_all()
        .context("syncing staged executable")?;
    staged
        .persist(executable)
        .map_err(|error| error.error)
        .context("atomically replacing executable")?;
    Ok(())
}
