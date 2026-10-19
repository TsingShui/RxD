//! Local reads reject links and special files; container paths cannot escape their root.
use std::{
    fs::{File, OpenOptions},
    io::Read,
    path::Path,
};

use rxd_core::{Error, Result};

pub fn invalid(message: impl Into<String>) -> Error {
    Error::new("INVALID_INPUT", message)
}

pub fn validate_entry(entry: &str) -> Result<()> {
    if entry.len() > 4096 {
        return Err(Error::new(
            "RESOURCE_LIMIT",
            "container path byte budget exceeded",
        ));
    }
    if entry.is_empty()
        || entry.split('/').any(|component| {
            component.is_empty()
                || matches!(component, "." | "..")
                || component.contains(['\\', ':', '\0'])
        })
    {
        return Err(invalid(
            "entry must be a relative path without empty, dot, parent, drive or backslash components",
        ));
    }
    Ok(())
}

pub fn open(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    #[cfg(not(unix))]
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(invalid("symlink input is not supported"));
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid("expected a regular local file"));
    }
    Ok(file)
}

// On Unix, each component is opened relative to the preceding directory handle.
// A concurrently replaced symlink cannot redirect an unpacked entry outside its root.
#[cfg(unix)]
pub fn open_entry(root: &Path, entry: &str) -> Result<File> {
    use std::{
        ffi::CString,
        os::fd::{AsRawFd, FromRawFd},
        os::unix::fs::OpenOptionsExt,
    };
    validate_entry(entry)?;
    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(root)?;
    let mut components = entry.split('/').peekable();
    while let Some(component) = components.next() {
        let name = CString::new(component).map_err(|_| invalid("NUL in entry path"))?;
        let is_directory = components.peek().is_some();
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if is_directory { libc::O_DIRECTORY } else { 0 };
        // SAFETY: name is NUL-terminated and the directory handle remains live.
        let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: openat returned a fresh descriptor owned exclusively here.
        directory = unsafe { File::from_raw_fd(fd) };
    }
    if !directory.metadata()?.is_file() {
        return Err(invalid("expected a regular container entry"));
    }
    Ok(directory)
}

#[cfg(not(unix))]
pub fn open_entry(root: &Path, entry: &str) -> Result<File> {
    open(&checked_path(root, entry)?)
}

#[cfg(not(unix))]
pub fn checked_path(root: &Path, entry: &str) -> Result<std::path::PathBuf> {
    validate_entry(entry)?;
    let mut path = root.to_owned();
    for component in entry.split('/') {
        path.push(component);
        if std::fs::symlink_metadata(&path)?.file_type().is_symlink() {
            return Err(invalid("symlink container entries are not supported"));
        }
    }
    Ok(path)
}

pub fn read(mut file: File, max_bytes: usize) -> Result<Vec<u8>> {
    if file.metadata()?.len() > max_bytes as u64 {
        return Err(Error::new("RESOURCE_LIMIT", "input byte budget exceeded"));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(Error::new("RESOURCE_LIMIT", "input byte budget exceeded"));
    }
    Ok(bytes)
}
