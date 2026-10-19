use crate::{Diagnostic, ErrorKind, ExtensionArtifacts, Limits, PublicationReceipt, Result, Stage};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path},
};

fn io(error: std::io::Error) -> Diagnostic {
    Diagnostic::new(ErrorKind::Io, Stage::Publication, error.to_string())
}
fn invalid(message: &str) -> Diagnostic {
    Diagnostic::new(ErrorKind::InvalidInput, Stage::Publication, message)
}

/// Revalidates the strict local MV3 artifact structure, not JS syntax/semantics or
/// runtime hookability. Stages complete files privately and publishes by an atomic
/// no-clobber directory rename (Linux renameat2, macOS renameatx_np, Windows MoveFileExW).
/// Existing nonempty directories, files and symlinks are never replaced. An existing
/// empty directory may be removed; a racing new destination causes publication to fail.
/// Parent symlinks (including macOS /tmp) are resolved once. Callers must control the
/// parent directory: this is not a sandbox against concurrent ancestor renaming.
/// Atomic visibility is guaranteed, not power-loss durability.
pub fn publish(
    artifacts: &ExtensionArtifacts,
    destination: &Path,
    limits: Limits,
) -> Result<PublicationReceipt> {
    super::extension::validate(artifacts, limits)?;
    let name = destination
        .file_name()
        .ok_or_else(|| invalid("destination requires a directory name"))?;
    if !matches!(
        destination.components().next_back(),
        Some(Component::Normal(_))
    ) {
        return Err(invalid(
            "destination requires a normal final directory component",
        ));
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = fs::canonicalize(parent).map_err(io)?;
    let resolved = parent.join(name);
    check_destination(&resolved)?;
    let staging = tempfile::Builder::new()
        .prefix(".rxd-hook-gen-")
        .tempdir_in(&parent)
        .map_err(io)?;
    for file in &artifacts.files {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(staging.path().join(&file.name))
            .map_err(io)?;
        output.write_all(&file.contents).map_err(io)?;
        output.sync_all().map_err(io)?;
    }
    // Recheck immediately before removal. remove_dir only removes an empty directory,
    // never a racing nonempty one; atomic no-clobber rename guards the final race.
    if check_destination(&resolved)? {
        fs::remove_dir(&resolved).map_err(io)?;
    }
    rename_noreplace(staging.path(), &resolved).map_err(io)?;
    // TempDir cleanup now sees an absent staging name, never the published destination.
    Ok(PublicationReceipt {
        directory: destination.to_owned(),
        files: artifacts
            .files
            .iter()
            .map(|file| destination.join(&file.name))
            .collect(),
        bytes: artifacts.files.iter().map(|file| file.contents.len()).sum(),
    })
}

fn check_destination(destination: &Path) -> Result<bool> {
    match fs::symlink_metadata(destination) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(invalid(
                    "destination is a file or symlink; refusing replacement",
                ));
            }
            if fs::read_dir(destination)
                .map_err(io)?
                .next()
                .transpose()
                .map_err(io)?
                .is_some()
            {
                return Err(invalid("destination is nonempty; refusing replacement"));
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io(error)),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn rename_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let from = CString::new(from.as_os_str().as_bytes())?;
    let to = CString::new(to.as_os_str().as_bytes())?;
    // Both names are sibling paths and contain no NUL. EXCL/NOREPLACE is essential:
    // std::fs::rename can replace a racing empty directory on Unix.
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "windows")]
fn rename_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    }
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // No MOVEFILE_REPLACE_EXISTING and no cross-volume copy fallback.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn rename_noreplace(_from: &Path, _to: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic no-clobber directory publication is unavailable on this platform",
    ))
}
