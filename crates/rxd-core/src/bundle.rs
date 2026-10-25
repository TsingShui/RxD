//! Verified, atomic extraction of compressed native runtimes into a per-user cache.
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Deserialize)]
pub struct Manifest {
    pub bundle_sha256: String,
    #[serde(default)]
    pub executable: Option<String>,
    pub files: Vec<File>,
}

#[derive(Deserialize)]
pub struct File {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

pub fn sha256(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(crate::identity::lower_hex(&digest.finalize()))
}

pub fn regular(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
}

fn cache_parent(namespace: &str) -> Result<PathBuf> {
    let base = if let Some(path) = env::var_os("RXD_CACHE_DIR").filter(|p| !p.is_empty()) {
        PathBuf::from(path)
    } else if let Some(path) = env::var_os("XDG_CACHE_HOME").filter(|p| !p.is_empty()) {
        PathBuf::from(path)
    } else if cfg!(target_os = "windows") {
        PathBuf::from(env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is not set")?)
    } else {
        let home = PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?);
        home.join(if cfg!(target_os = "macos") {
            "Library/Caches"
        } else {
            ".cache"
        })
    };
    let parent = base.join(namespace);
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&parent)?;
    let metadata = fs::symlink_metadata(&parent)?;
    if !metadata.is_dir() {
        return Err("cache parent must be a directory, not a symlink".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
            return Err(
                "cache parent must belong to this user and not be writable by others".into(),
            );
        }
    }
    Ok(parent)
}

fn validate_cache(cache: &Path, manifest: &Manifest) -> Result<()> {
    if !fs::symlink_metadata(cache)?.is_dir()
        || fs::read_to_string(cache.join(".complete"))? != manifest.bundle_sha256
    {
        return Err("incomplete cache; remove its version directory and retry".into());
    }
    if let Some(name) = &manifest.executable {
        let executable = cache.join(name);
        let expected = manifest
            .files
            .iter()
            .find(|f| f.path == *name)
            .ok_or("manifest has no executable")?;
        if !regular(&executable) || sha256(&executable)? != expected.sha256 {
            return Err("cached executable failed integrity verification; remove its version directory and retry".into());
        }
    }
    Ok(())
}

pub fn materialize(
    namespace: &str,
    data: &[u8],
    target: &str,
    manifest: &Manifest,
) -> Result<PathBuf> {
    let os_matches = (cfg!(target_os = "macos") && target.contains("apple-darwin"))
        || (cfg!(target_os = "linux") && target.contains("linux"))
        || (cfg!(target_os = "android") && target.contains("linux-android"))
        || (cfg!(target_os = "windows") && target.contains("windows"));
    if !target.starts_with(env::consts::ARCH) || !os_matches {
        return Err("embedded runtime target does not match rxd".into());
    }
    let parent = cache_parent(namespace)?;
    let cache = parent.join(&manifest.bundle_sha256);
    if fs::symlink_metadata(&cache).is_ok() {
        validate_cache(&cache, manifest)?;
        return Ok(cache);
    }
    if crate::identity::lower_hex(&Sha256::digest(data)) != manifest.bundle_sha256 {
        return Err("embedded archive failed integrity verification".into());
    }
    let temporary = tempfile::Builder::new()
        .prefix(".extract-")
        .tempdir_in(&parent)?;
    let decoder = flate2::read::GzDecoder::new(data);
    let mut archive = tar::Archive::new(decoder);
    let mut count = 0;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if !entry.header().entry_type().is_file()
            || !path.components().all(|c| matches!(c, Component::Normal(_)))
        {
            return Err("bundle contains a non-regular file or invalid path".into());
        }
        let expected = manifest
            .files
            .iter()
            .find(|f| Path::new(&f.path) == path)
            .ok_or("unexpected file in bundle")?;
        if entry.size() != expected.size {
            return Err("bundle file size mismatch".into());
        }
        let destination = temporary.path().join(path);
        fs::create_dir_all(destination.parent().ok_or("invalid bundle path")?)?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)?;
        // Verify bytes while writing; publish only after every size and hash passes.
        let mut digest = Sha256::new();
        let mut copied = 0u64;
        let mut buffer = [0u8; 65536];
        loop {
            let n = entry.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            output.write_all(&buffer[..n])?;
            digest.update(&buffer[..n]);
            copied = copied
                .checked_add(n as u64)
                .ok_or("bundle file size overflow")?;
        }
        output.flush()?;
        if copied != expected.size {
            return Err("bundle copied file size mismatch".into());
        }
        if crate::identity::lower_hex(&digest.finalize()) != expected.sha256 {
            return Err("bundle file checksum mismatch".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                &destination,
                fs::Permissions::from_mode(
                    if manifest.executable.as_deref() == Some(expected.path.as_str()) {
                        0o700
                    } else {
                        0o600
                    },
                ),
            )?;
        }
        count += 1;
    }
    if count != manifest.files.len() {
        return Err("bundle is missing files".into());
    }
    fs::write(temporary.path().join(".complete"), &manifest.bundle_sha256)?;
    match fs::rename(temporary.path(), &cache) {
        // Private bytes are verified; a concurrent cached winner still needs validation.
        Ok(()) => {}
        Err(error) if cache.exists() => validate_cache(&cache, manifest)
            .map_err(|validation| format!("cache publication race ({error}): {validation}"))?,
        Err(error) => return Err(error.into()),
    }
    Ok(cache)
}
