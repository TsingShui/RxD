//! Immutable private snapshots; acquired requests retain bytes after handle closure.
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use super::resources::{ByteBudget, Reservation};
use rxd_core::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// Cooperative capture cancellation/deadline checks; cleared before publication.
pub type CaptureCheck = Arc<dyn Fn() -> Result<()> + Send + Sync>;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PathInput {
    pub path: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetInput {
    pub target_id: String,
}

/// Exactly one input selector is accepted; unknown and combined fields fail decoding.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum InputRef {
    Path(PathInput),
    Target(TargetInput),
}

#[derive(Clone, Copy, Debug)]
pub enum CaptureMode {
    File,
    Bundle,
}

#[derive(Clone, Copy, Debug)]
pub struct TargetLimits {
    pub max_total_bytes: usize,
    pub max_depth: usize,
}

impl Default for TargetLimits {
    fn default() -> Self {
        Self {
            max_total_bytes: 1024 * 1024 * 1024,
            max_depth: 256,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    File,
    Directory,
    Bundle,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetOrigin {
    pub container_source_id: String,
    pub entry: String,
    pub parent_target_id: Option<String>,
}

/// Public identities and provenance do not disclose private snapshot paths.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputInfo {
    pub original_path: PathBuf,
    pub relative_path: Option<PathBuf>,
    pub source_id: String,
    pub input_bytes: u64,
    pub dependency: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetInfo {
    pub target_id: String,
    pub source_id: String,
    pub snapshot_id: String,
    pub input_bytes: u64,
    pub captured_bytes: u64,
    pub kind: TargetKind,
    pub inputs: Vec<InputInfo>,
    pub origin: Option<TargetOrigin>,
}

/// Internal IPC DTO. The parent must keep the acquired snapshot alive through reaping.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerInput {
    pub paths: Vec<PathBuf>,
    pub original_paths: Vec<PathBuf>,
    pub files: Vec<InputInfo>,
    pub source_id: String,
    pub snapshot_id: String,
    pub input_bytes: u64,
    pub captured_bytes: u64,
    pub kind: TargetKind,
    pub origin: Option<TargetOrigin>,
}

pub struct CapturedInput {
    input: WorkerInput,
    // Drop the private files before releasing their storage reservation.
    _directory: TempDir,
    _lease: CaptureLease,
    _storage: Arc<TempDir>,
}

impl CapturedInput {
    pub fn worker_input(&self) -> WorkerInput {
        self.input.clone()
    }

    pub fn info(&self, target_id: String) -> TargetInfo {
        TargetInfo {
            target_id,
            source_id: self.input.source_id.clone(),
            snapshot_id: self.input.snapshot_id.clone(),
            input_bytes: self.input.input_bytes,
            captured_bytes: self.input.captured_bytes,
            kind: self.input.kind,
            inputs: self.input.files.clone(),
            origin: self.input.origin.clone(),
        }
    }
}

struct AllowedPath {
    path: PathBuf,
    is_directory: bool,
    #[cfg(unix)]
    directory: File,
}

impl AllowedPath {
    fn new(path: PathBuf) -> Result<Self> {
        if !path.is_absolute() {
            return Err(invalid_query("allowed input paths must be absolute"));
        }
        let path = fs::canonicalize(path)?;
        let metadata = fs::metadata(&path)?;
        if !metadata.is_dir() && !metadata.is_file() {
            return Err(invalid_query(
                "allowed paths must be regular files or directories",
            ));
        }
        let is_directory = metadata.is_dir();
        #[cfg(unix)]
        let directory = open_directory(if is_directory {
            &path
        } else {
            path.parent()
                .ok_or_else(|| invalid_query("allowed file has no parent"))?
        })?;
        Ok(Self {
            path,
            is_directory,
            #[cfg(unix)]
            directory,
        })
    }

    fn includes(&self, path: &Path) -> bool {
        path == self.path || (self.is_directory && path.starts_with(&self.path))
    }

    #[cfg(unix)]
    fn anchor(&self) -> &Path {
        if self.is_directory {
            &self.path
        } else {
            self.path.parent().unwrap()
        }
    }
}

struct CaptureLease {
    memory: Reservation,
    bytes: usize,
    control: Option<CaptureCheck>,
}

impl CaptureLease {
    fn new(usage: ByteBudget, _limits: TargetLimits, control: CaptureCheck) -> Result<Self> {
        control()?;
        Ok(Self {
            memory: usage.reserve(1024)?,
            bytes: 0,
            control: Some(control),
        })
    }
    fn check(&self) -> Result<()> {
        check_capture(self.control.as_ref())
    }
    fn charge(&mut self, bytes: usize) -> Result<()> {
        self.check()?;
        self.memory.grow(bytes)?;
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or_else(|| resource("snapshot size overflows"))?;
        Ok(())
    }
    fn metadata(&mut self, bytes: usize) -> Result<()> {
        self.check()?;
        self.memory.grow(bytes)
    }
}

struct Handle {
    capture: Arc<CapturedInput>,
    _memory: Reservation,
}

#[derive(Default)]
struct Registry {
    handles: HashMap<String, Handle>,
    serial: u64,
}

pub struct PublishedTarget {
    pub info: TargetInfo,
    store: Arc<TargetStore>,
    delivered: bool,
}
impl PublishedTarget {
    pub fn delivered(&mut self) {
        self.delivered = true;
    }
}
impl Drop for PublishedTarget {
    fn drop(&mut self) {
        if !self.delivered {
            let _ = self.store.close(&self.info.target_id);
        }
    }
}

pub struct TargetStore {
    allow_paths: Vec<AllowedPath>,
    limits: TargetLimits,
    storage: Arc<TempDir>,
    storage_canonical: PathBuf,
    usage: ByteBudget,
    registry: Mutex<Registry>,
}

impl TargetStore {
    pub fn new(allow_paths: Vec<PathBuf>, limits: TargetLimits) -> Result<Self> {
        if limits.max_total_bytes == 0 || limits.max_depth == 0 {
            return Err(invalid_query(
                "snapshot byte budget and depth must be positive",
            ));
        }
        // Traversal runs on smaller-stack server threads, so depth must stay bounded.
        if limits.max_depth > 256 {
            return Err(invalid_query(
                "snapshot depth cannot exceed the safe server ceiling of 256",
            ));
        }
        let mut allowed = allow_paths
            .into_iter()
            .map(AllowedPath::new)
            .collect::<Result<Vec<_>>>()?;
        // Prefer the most specific grant when allowed paths overlap.
        allowed.sort_by_key(|grant| std::cmp::Reverse(grant.path.components().count()));
        let storage = Arc::new(
            tempfile::Builder::new()
                .prefix("rxd-mcp-targets-")
                .rand_bytes(24)
                .tempdir()?,
        );
        let storage_canonical = fs::canonicalize(storage.path())?;
        Ok(Self {
            allow_paths: allowed,
            limits,
            storage,
            storage_canonical,
            usage: ByteBudget::new(limits.max_total_bytes),
            registry: Mutex::new(Registry::default()),
        })
    }

    pub fn acquire(
        &self,
        input: &InputRef,
        mode: CaptureMode,
        check: CaptureCheck,
    ) -> Result<Arc<CapturedInput>> {
        check()?;
        match input {
            InputRef::Path(path) => {
                self.capture_paths(std::slice::from_ref(&path.path), mode, check)
            }
            InputRef::Target(target) => {
                let registry = self.registry.lock().map_err(|_| poisoned())?;

                let capture = registry
                    .handles
                    .get(&target.target_id)
                    .map(|handle| Arc::clone(&handle.capture))
                    .ok_or_else(target_invalid)?;
                let compatible = match mode {
                    CaptureMode::File => capture.input.kind == TargetKind::File,
                    CaptureMode::Bundle => capture.input.kind != TargetKind::Directory,
                };
                if !compatible {
                    return Err(invalid_query(
                        "target input scope does not match this backend operation",
                    ));
                }
                Ok(capture)
            }
        }
    }

    pub fn close(&self, target_id: &str) -> Result<()> {
        let mut registry = self.registry.lock().map_err(|_| poisoned())?;
        registry
            .handles
            .remove(target_id)
            .map(|_| ())
            .ok_or_else(target_invalid)
    }

    pub fn child(
        self: &Arc<Self>,
        bytes: &[u8],
        name: &str,
        origin: TargetOrigin,
        check: CaptureCheck,
    ) -> Result<PublishedTarget> {
        let name = safe_name(name)?;
        let mut lease = CaptureLease::new(self.usage.clone(), self.limits, check)?;
        lease.metadata(2048usize.saturating_add(origin.entry.len().saturating_mul(8)))?;
        lease.charge(bytes.len())?;
        let directory = tempfile::Builder::new()
            .prefix("child-")
            .tempdir_in(self.storage.path())?;
        let path = directory.path().join(name);
        let source_id = write_private(&path, bytes, &lease)?;
        lease.control = None;
        let capture = Arc::new(CapturedInput {
            input: WorkerInput {
                paths: vec![path],
                original_paths: vec![PathBuf::from(&origin.entry)],
                files: vec![InputInfo {
                    original_path: PathBuf::from(&origin.entry),
                    relative_path: None,
                    source_id: source_id.clone(),
                    input_bytes: bytes.len() as u64,
                    dependency: false,
                }],
                source_id: source_id.clone(),
                snapshot_id: source_id,
                input_bytes: bytes.len() as u64,
                captured_bytes: bytes.len() as u64,
                kind: TargetKind::File,
                origin: Some(origin),
            },
            _directory: directory,
            _lease: lease,
            _storage: Arc::clone(&self.storage),
        });
        self.publish(capture)
    }

    pub fn publish(self: &Arc<Self>, capture: Arc<CapturedInput>) -> Result<PublishedTarget> {
        let mut registry = self.registry.lock().map_err(|_| poisoned())?;
        let memory = self.usage.reserve(512)?;
        registry.serial = registry
            .serial
            .checked_add(1)
            .ok_or_else(|| resource("target handle serial exhausted"))?;
        let nonce = self
            .storage
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| invalid_input("snapshot storage path must be UTF-8"))?;
        let id = format!("{nonce}-{}", registry.serial);
        let info = capture.info(id.clone());
        registry.handles.insert(
            id,
            Handle {
                capture,
                _memory: memory,
            },
        );
        Ok(PublishedTarget {
            info,
            store: Arc::clone(self),
            delivered: false,
        })
    }

    pub fn capture_paths(
        &self,
        paths: &[PathBuf],
        mode: CaptureMode,
        check: CaptureCheck,
    ) -> Result<Arc<CapturedInput>> {
        check()?;
        if paths.is_empty() || (paths.len() != 1 && !matches!(mode, CaptureMode::Bundle)) {
            return Err(invalid_query(
                "capture requires one path or an explicit input bundle",
            ));
        }
        if paths.iter().any(|path| !path.is_absolute()) {
            return Err(invalid_query("input paths must be absolute"));
        }
        let mut lease = CaptureLease::new(self.usage.clone(), self.limits, check)?;
        let directory = tempfile::Builder::new()
            .prefix("capture-")
            .tempdir_in(self.storage.path())?;
        let mut files = Vec::new();
        let mut captured_paths = Vec::new();
        let mut original_paths = Vec::new();
        let mut seen = HashMap::<PathBuf, (PathBuf, InputInfo)>::new();
        for (index, input) in paths.iter().enumerate() {
            lease.check()?;
            let canonical = self.resolve(input)?;
            let original = input.clone();
            lease.metadata(
                1024usize
                    .saturating_add(original.as_os_str().len().saturating_mul(8))
                    .saturating_add(canonical.as_os_str().len().saturating_mul(8)),
            )?;
            original_paths.push(original.clone());
            if let Some((path, info)) = seen.get(&original) {
                captured_paths.push(path.clone());
                files.push(info.clone());
                continue;
            }
            let file = self.open_allowed(&canonical, false)?;
            let name = original
                .file_name()
                .ok_or_else(|| invalid_input("input needs a file name"))?;
            validate_snapshot_component(name)?;
            let parent = directory.path().join(index.to_string());
            fs::create_dir(&parent)?;
            let destination = parent.join(name);
            let info = copy_file(
                file,
                &destination,
                original.clone(),
                None,
                false,
                &mut lease,
            )?;
            seen.insert(original, (destination.clone(), info.clone()));
            files.push(info);
            captured_paths.push(destination);
        }
        let input_bytes = files
            .iter()
            .try_fold(0u64, |total, file| total.checked_add(file.input_bytes))
            .ok_or_else(|| resource("input size overflows"))?;
        let kind = if paths.len() == 1 {
            TargetKind::File
        } else {
            TargetKind::Bundle
        };
        let source_id = if kind == TargetKind::File {
            files[0].source_id.clone()
        } else {
            manifest_hash(&files)
        };
        lease.check()?;
        lease.control = None;
        let input = WorkerInput {
            paths: captured_paths,
            original_paths,
            snapshot_id: source_id.clone(),
            source_id,
            files,
            input_bytes,
            captured_bytes: lease.bytes as u64,
            kind,
            origin: None,
        };
        Ok(Arc::new(CapturedInput {
            input,
            _directory: directory,
            _lease: lease,
            _storage: Arc::clone(&self.storage),
        }))
    }

    fn resolve(&self, path: &Path) -> Result<PathBuf> {
        let canonical = fs::canonicalize(path)?;
        if canonical.starts_with(&self.storage_canonical) {
            return Err(Error::new(
                "ACCESS_DENIED",
                "managed snapshot storage must be selected through a target handle",
            ));
        }
        if canonical.to_str().is_none() {
            return Err(invalid_input("input paths must be UTF-8"));
        }
        if !self.allow_paths.is_empty()
            && !self
                .allow_paths
                .iter()
                .any(|grant| grant.includes(&canonical))
        {
            return Err(Error::new(
                "ACCESS_DENIED",
                "input resolves outside allowed paths",
            ));
        }
        Ok(canonical)
    }

    fn open_allowed(&self, canonical: &Path, directory: bool) -> Result<File> {
        let unrestricted;
        let grant = if self.allow_paths.is_empty() {
            // Even unrestricted capture traverses from a held filesystem root without following links.
            let root = canonical
                .ancestors()
                .last()
                .ok_or_else(|| invalid_input("absolute input has no filesystem root"))?;
            unrestricted = AllowedPath::new(root.to_owned())?;
            &unrestricted
        } else {
            self.allow_paths
                .iter()
                .find(|grant| grant.includes(canonical) && (!directory || grant.is_directory))
                .ok_or_else(|| {
                    Error::new("ACCESS_DENIED", "input resolves outside allowed paths")
                })?
        };
        #[cfg(unix)]
        let file = open_beneath(
            &grant.directory,
            canonical
                .strip_prefix(grant.anchor())
                .map_err(|_| invalid_input("input is not under its allowed path"))?,
            directory,
        )?;
        #[cfg(not(unix))]
        let file = open_checked(canonical, &grant.path, directory)?;
        let metadata = file.metadata()?;
        if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
            return Err(invalid_input(if directory {
                "expected a local directory"
            } else {
                "expected a regular local file"
            }));
        }
        Ok(file)
    }
}

// Directory discovery uses the held directory handle, including its entry enumeration.
// Filesystem symlinks and special children retain the CLI's skip semantics.
fn copy_file(
    mut source: File,
    destination: &Path,
    original_path: PathBuf,
    relative_path: Option<PathBuf>,
    dependency: bool,
    lease: &mut CaptureLease,
) -> Result<InputInfo> {
    let before = source.metadata()?;
    if !before.is_file() {
        return Err(invalid_input("expected a regular local file"));
    }
    lease.metadata(
        2048usize
            .saturating_add(original_path.as_os_str().len().saturating_mul(8))
            .saturating_add(
                relative_path
                    .as_ref()
                    .map_or(0, |p| p.as_os_str().len().saturating_mul(8)),
            ),
    )?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        lease.check()?;
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        lease.charge(count)?;
        output.write_all(&buffer[..count])?;
        hasher.update(&buffer[..count]);
        size = size
            .checked_add(count as u64)
            .ok_or_else(|| resource("input size overflows"))?;
    }
    let after = source.metadata()?;
    if size != before.len()
        || after.len() != before.len()
        || after.modified().ok() != before.modified().ok()
    {
        return Err(Error::new(
            "INPUT_CHANGED",
            "input changed while its snapshot was captured",
        ));
    }
    drop(output);
    make_readonly(destination)?;
    Ok(InputInfo {
        original_path,
        relative_path,
        source_id: format!("{:x}", hasher.finalize()),
        input_bytes: size,
        dependency,
    })
}

fn manifest_hash(files: &[InputInfo]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"rxd-mcp-snapshot-v1\0");
    for file in files {
        let relative = file
            .relative_path
            .as_deref()
            .unwrap_or_else(|| Path::new(""));
        let name = relative.to_string_lossy();
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        hash.update(file.source_id.as_bytes());
        hash.update(file.input_bytes.to_le_bytes());
        hash.update([u8::from(file.dependency)]);
    }
    format!("{:x}", hash.finalize())
}

fn safe_name(name: &str) -> Result<&str> {
    if name.is_empty()
        || name.len() > 4096
        || matches!(name, "." | "..")
        || name.contains(['/', '\\', ':', '\0'])
    {
        return Err(invalid_query(
            "child name must be one safe file-name component",
        ));
    }
    validate_snapshot_component(std::ffi::OsStr::new(name))?;
    Ok(name)
}

/// Reject Windows device, stream and normalized aliases before private writes.
#[cfg(windows)]
fn validate_snapshot_component(name: &std::ffi::OsStr) -> Result<()> {
    let name = name
        .to_str()
        .ok_or_else(|| invalid_input("snapshot names must be UTF-8"))?;
    if name.ends_with(['.', ' '])
        || name.chars().any(|character| {
            character < ' '
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
        })
    {
        return Err(invalid_input("unsupported Windows snapshot filename"));
    }
    let base = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    let numbered_device = base
        .strip_prefix("COM")
        .or_else(|| base.strip_prefix("LPT"))
        .is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        });
    if matches!(
        base.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) || numbered_device
    {
        return Err(invalid_input(
            "Windows device names cannot be snapshot filenames",
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn validate_snapshot_component(_name: &std::ffi::OsStr) -> Result<()> {
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8], lease: &CaptureLease) -> Result<String> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut hash = Sha256::new();
    for chunk in bytes.chunks(64 * 1024) {
        lease.check()?;
        file.write_all(chunk)?;
        hash.update(chunk);
    }
    drop(file);
    make_readonly(path)?;
    Ok(format!("{:x}", hash.finalize()))
}

fn check_capture(control: Option<&CaptureCheck>) -> Result<()> {
    match control {
        Some(check) => check(),
        None => Ok(()),
    }
}

fn make_readonly(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o400))?;
    }
    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

#[cfg(unix)]
fn open_directory(path: &Path) -> Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    Ok(OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?)
}

#[cfg(unix)]
fn open_beneath(root: &File, relative: &Path, directory: bool) -> Result<File> {
    use std::{
        ffi::CString,
        os::fd::{AsRawFd, FromRawFd},
        os::unix::ffi::OsStrExt,
        path::Component,
    };
    // Reopen root-relative "." for an independent directory cursor.
    // SAFETY: root is live; the static path is NUL-terminated.
    let initial = unsafe {
        libc::openat(
            root.as_raw_fd(),
            c".".as_ptr(),
            libc::O_RDONLY
                | libc::O_CLOEXEC
                | libc::O_NOFOLLOW
                | libc::O_NONBLOCK
                | libc::O_DIRECTORY,
        )
    };
    if initial < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: openat returned an independently owned descriptor.
    let mut opened = unsafe { File::from_raw_fd(initial) };
    let components = relative.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err(invalid_input("input path must have only normal components"));
        };
        let name = CString::new(name.as_bytes()).map_err(|_| invalid_input("NUL in input path"))?;
        let is_directory = directory || index + 1 < components.len();
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if is_directory { libc::O_DIRECTORY } else { 0 };
        // SAFETY: root/opened is live, name is NUL terminated, and the new descriptor is owned.
        let fd = unsafe { libc::openat(opened.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: openat returned a fresh owned descriptor above.
        opened = unsafe { File::from_raw_fd(fd) };
    }
    Ok(opened)
}

#[cfg(not(unix))]
fn open_checked(path: &Path, root: &Path, directory: bool) -> Result<File> {
    // On Windows inspect the opened handle's resolved path, not a pre-open string prefix.
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || (!directory && !metadata.is_file())
        || (directory && !metadata.is_dir())
    {
        return Err(invalid_input(
            "expected a regular input without filesystem links",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x02200000); // BACKUP_SEMANTICS | OPEN_REPARSE_POINT: validate directories and reject raced leaf links.
    }
    let file = options.open(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::{ffi::OsStringExt, io::AsRawHandle};
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetFinalPathNameByHandleW(
                handle: *mut std::ffi::c_void,
                buffer: *mut u16,
                length: u32,
                flags: u32,
            ) -> u32;
        }
        let mut buffer = vec![0u16; 32768];
        // SAFETY: handle is live, the writable buffer spans length elements.
        let length = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                0,
            )
        };
        if length == 0 || length as usize >= buffer.len() {
            return Err(std::io::Error::last_os_error().into());
        }
        let actual = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length as usize]));
        if !actual.starts_with(root) {
            return Err(Error::new(
                "ACCESS_DENIED",
                "opened input resolves outside allowed paths",
            ));
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (file, root);
        return Err(invalid_input(
            "snapshot file-handle validation is unsupported on this platform",
        ));
    }
    #[cfg(windows)]
    Ok(file)
}

fn resource(message: &str) -> Error {
    Error::new("RESOURCE_LIMIT", message)
}
fn invalid_query(message: &str) -> Error {
    Error::new("INVALID_QUERY", message)
}
fn invalid_input(message: &str) -> Error {
    Error::new("INVALID_INPUT", message)
}
fn target_invalid() -> Error {
    Error::new(
        "TARGET_INVALID",
        "target is unknown, closed, or belongs to another server process",
    )
}
fn poisoned() -> Error {
    Error::new("WORKER_FAILURE", "snapshot bookkeeping lock was poisoned")
}
