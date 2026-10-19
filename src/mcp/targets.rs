//! Process-local immutable input snapshots and bounded target lifetimes.
//!
//! A target owns private copies, not an open descriptor to a caller's mutable file.
//! Acquiring an Arc retains storage and quota after the handle is closed or expires.
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use rxd_core::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// Cooperative checks supplied by the transport-independent request scheduler.
/// Guards are released before a completed capture becomes a durable target.
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
    JavaScriptDirectory,
    Asar,
    Bundle,
}

#[derive(Clone, Copy, Debug)]
pub struct TargetLimits {
    pub max_targets: usize,
    pub max_live_captures: usize,
    pub max_total_bytes: usize,
    pub max_capture_bytes: usize,
    pub max_files: usize,
    pub max_depth: usize,
    pub max_directory_entries: usize,
    pub ttl: Duration,
}

impl Default for TargetLimits {
    fn default() -> Self {
        Self {
            max_targets: 64,
            max_live_captures: 128,
            max_total_bytes: 1024 * 1024 * 1024,
            max_capture_bytes: 256 * 1024 * 1024,
            max_files: 10_000,
            max_depth: 64,
            max_directory_entries: 100_000,
            ttl: Duration::from_secs(600),
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
    pub asar_dependencies_captured: bool,
}

/// Snapshot plan data from the isolated ASAR metadata worker, never caller-provided.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AsarDependency {
    pub entry: String,
    pub size: u64,
}

pub struct CapturedInput {
    input: WorkerInput,
    requires_asar_plan: bool,
    canonical_paths: Vec<PathBuf>,
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

struct Root {
    path: PathBuf,
    #[cfg(unix)]
    directory: File,
}

#[derive(Default)]
struct Usage {
    captures: usize,
    bytes: usize,
}

struct CaptureLease {
    usage: Arc<Mutex<Usage>>,
    bytes: usize,
    limits: TargetLimits,
    control: Option<CaptureCheck>,
}

impl CaptureLease {
    fn new(usage: Arc<Mutex<Usage>>, limits: TargetLimits, control: CaptureCheck) -> Result<Self> {
        control()?;
        {
            let mut current = usage.lock().map_err(|_| poisoned())?;
            if current.captures >= limits.max_live_captures {
                return Err(resource("live snapshot count budget exceeded"));
            }
            current.captures += 1;
        }
        Ok(Self {
            usage,
            bytes: 0,
            limits,
            control: Some(control),
        })
    }

    fn check(&self) -> Result<()> {
        check_capture(self.control.as_ref())
    }

    fn charge(&mut self, bytes: usize) -> Result<()> {
        self.check()?;
        if bytes > self.limits.max_capture_bytes.saturating_sub(self.bytes) {
            return Err(resource("snapshot capture byte budget exceeded"));
        }
        let mut current = self.usage.lock().map_err(|_| poisoned())?;
        if bytes > self.limits.max_total_bytes.saturating_sub(current.bytes) {
            return Err(resource("total snapshot byte budget exceeded"));
        }
        current.bytes += bytes;
        self.bytes += bytes;
        Ok(())
    }
}

impl Drop for CaptureLease {
    fn drop(&mut self) {
        let mut current = self.usage.lock().unwrap_or_else(|error| error.into_inner());
        current.bytes = current.bytes.saturating_sub(self.bytes);
        current.captures = current.captures.saturating_sub(1);
    }
}

struct Handle {
    capture: Arc<CapturedInput>,
    expires: Instant,
}

#[derive(Default)]
struct Registry {
    handles: HashMap<String, Handle>,
    serial: u64,
}

pub struct TargetStore {
    base: PathBuf,
    roots: Vec<Root>,
    limits: TargetLimits,
    storage: Arc<TempDir>,
    storage_canonical: PathBuf,
    usage: Arc<Mutex<Usage>>,
    registry: Mutex<Registry>,
}

impl TargetStore {
    pub fn new(base: PathBuf, roots: Vec<PathBuf>, limits: TargetLimits) -> Result<Self> {
        if limits.max_targets == 0
            || limits.max_live_captures == 0
            || limits.max_total_bytes == 0
            || limits.max_capture_bytes == 0
            || limits.max_files == 0
            || limits.max_depth == 0
            || limits.max_directory_entries == 0
            || limits.ttl.is_zero()
        {
            return Err(invalid_query("snapshot limits must all be nonzero"));
        }
        // Capture traversal runs in the protocol process, whose blocking threads
        // use a smaller stack than the isolated analysis worker. Never accept an
        // operator limit that lets untrusted directories exhaust that stack.
        if limits.max_depth > 256 {
            return Err(invalid_query(
                "snapshot depth cannot exceed the safe server ceiling of 256",
            ));
        }
        if limits.max_capture_bytes > limits.max_total_bytes {
            return Err(invalid_query(
                "capture byte budget exceeds total snapshot budget",
            ));
        }
        if roots.is_empty() {
            return Err(invalid_query("at least one allowed root is required"));
        }
        let base = fs::canonicalize(base)?;
        if !base.is_dir() {
            return Err(invalid_query("server base must be a directory"));
        }
        let mut allowed = Vec::new();
        for path in roots {
            let path = fs::canonicalize(if path.is_absolute() {
                path
            } else {
                base.join(path)
            })?;
            if !path.is_dir() {
                return Err(invalid_query("allowed roots must be directories"));
            }
            #[cfg(unix)]
            let directory = open_directory(&path)?;
            allowed.push(Root {
                path,
                #[cfg(unix)]
                directory,
            });
        }
        // Prefer the most specific root when authorized roots overlap.
        allowed.sort_by_key(|root| std::cmp::Reverse(root.path.components().count()));
        let storage = Arc::new(
            tempfile::Builder::new()
                .prefix("rxd-mcp-targets-")
                .rand_bytes(24)
                .tempdir()?,
        );
        let storage_canonical = fs::canonicalize(storage.path())?;
        Ok(Self {
            base,
            roots: allowed,
            limits,
            storage,
            storage_canonical,
            usage: Arc::new(Mutex::new(Usage::default())),
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
                let mut registry = self.registry.lock().map_err(|_| poisoned())?;
                expire(&mut registry);
                let capture = registry
                    .handles
                    .get(&target.target_id)
                    .map(|handle| Arc::clone(&handle.capture))
                    .ok_or_else(target_invalid)?;
                let compatible = match mode {
                    CaptureMode::File | CaptureMode::Asar => capture.input.kind == TargetKind::File,
                    CaptureMode::JavaScriptDirectory => capture.input.kind != TargetKind::Bundle,
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
        expire(&mut registry);
        registry
            .handles
            .remove(target_id)
            .map(|_| ())
            .ok_or_else(target_invalid)
    }

    /// Release expired handles even when the service has no analysis requests.
    pub fn purge_expired(&self) -> Result<()> {
        let mut registry = self.registry.lock().map_err(|_| poisoned())?;
        expire(&mut registry);
        Ok(())
    }

    pub fn child(
        &self,
        bytes: &[u8],
        name: &str,
        origin: TargetOrigin,
        check: CaptureCheck,
    ) -> Result<TargetInfo> {
        self.purge_expired()?;
        let name = safe_name(name)?;
        let mut lease = CaptureLease::new(Arc::clone(&self.usage), self.limits, check)?;
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
                asar_dependencies_captured: false,
            },
            requires_asar_plan: false,
            canonical_paths: Vec::new(),
            _directory: directory,
            _lease: lease,
            _storage: Arc::clone(&self.storage),
        });
        self.publish(capture)
    }

    pub fn publish(&self, capture: Arc<CapturedInput>) -> Result<TargetInfo> {
        if capture.requires_asar_plan && !capture.input.asar_dependencies_captured {
            return Err(invalid_query(
                "ASAR capture requires an isolated dependency plan before publication",
            ));
        }
        let mut registry = self.registry.lock().map_err(|_| poisoned())?;
        expire(&mut registry);
        if registry.handles.len() >= self.limits.max_targets {
            return Err(resource("target handle count budget exceeded"));
        }
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
                expires: Instant::now()
                    .checked_add(self.limits.ttl)
                    .ok_or_else(|| invalid_query("target retention is too large"))?,
            },
        );
        Ok(info)
    }

    pub fn capture_paths(
        &self,
        paths: &[PathBuf],
        mode: CaptureMode,
        check: CaptureCheck,
    ) -> Result<Arc<CapturedInput>> {
        check()?;
        {
            let mut registry = self.registry.lock().map_err(|_| poisoned())?;
            expire(&mut registry);
        }
        if paths.is_empty() || (paths.len() != 1 && !matches!(mode, CaptureMode::Bundle)) {
            return Err(invalid_query(
                "capture requires one path or an explicit input bundle",
            ));
        }
        if paths.len() > self.limits.max_files {
            return Err(resource("snapshot file count budget exceeded"));
        }
        let mut lease = CaptureLease::new(Arc::clone(&self.usage), self.limits, check)?;
        let directory = tempfile::Builder::new()
            .prefix("capture-")
            .tempdir_in(self.storage.path())?;
        let mut files = Vec::new();
        let mut captured_paths = Vec::new();
        let mut original_paths = Vec::new();
        let mut canonical_paths = Vec::new();
        let mut seen = HashMap::<PathBuf, (PathBuf, InputInfo)>::new();
        for (index, input) in paths.iter().enumerate() {
            lease.check()?;
            let canonical = self.resolve(input)?;
            let original = self.display_path(input);
            if matches!(mode, CaptureMode::JavaScriptDirectory) && canonical.is_dir() {
                return self.capture_directory(original, canonical, directory, lease);
            }
            original_paths.push(original.clone());
            canonical_paths.push(canonical.clone());
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
            asar_dependencies_captured: false,
        };
        Ok(Arc::new(CapturedInput {
            input,
            requires_asar_plan: matches!(mode, CaptureMode::Asar),
            canonical_paths,
            _directory: directory,
            _lease: lease,
            _storage: Arc::clone(&self.storage),
        }))
    }

    fn capture_directory(
        &self,
        original: PathBuf,
        canonical: PathBuf,
        directory: TempDir,
        mut lease: CaptureLease,
    ) -> Result<Arc<CapturedInput>> {
        let source = self.open_allowed(&canonical, true)?;
        let destination = directory.path().join("root");
        fs::create_dir(&destination)?;
        let mut files = Vec::new();
        walk_directory(
            &source,
            Path::new(""),
            1,
            &mut WalkState {
                entries: 0,
                files: &mut files,
                lease: &mut lease,
                original_root: &original,
                canonical_root: &canonical,
                destination_root: &destination,
                excluded_root: &self.storage_canonical,
            },
        )?;
        files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        lease.check()?;
        let source_id = manifest_hash(&files);
        lease.control = None;
        let capture = CapturedInput {
            input: WorkerInput {
                paths: vec![destination],
                original_paths: vec![original],
                files,
                source_id: source_id.clone(),
                snapshot_id: source_id,
                input_bytes: lease.bytes as u64,
                captured_bytes: lease.bytes as u64,
                kind: TargetKind::Directory,
                origin: None,
                asar_dependencies_captured: false,
            },
            requires_asar_plan: false,
            canonical_paths: vec![canonical],
            _directory: directory,
            _lease: lease,
            _storage: Arc::clone(&self.storage),
        };
        Ok(Arc::new(capture))
    }

    /// Complete a staged ASAR capture from a bounded plan produced by an isolated worker.
    /// No archive/header parser runs in the protocol process.
    pub fn complete_asar(
        &self,
        mut capture: Arc<CapturedInput>,
        dependencies: &[AsarDependency],
        check: CaptureCheck,
    ) -> Result<Arc<CapturedInput>> {
        check()?;
        if capture.input.asar_dependencies_captured {
            return Ok(capture);
        }
        if dependencies.len()
            > self
                .limits
                .max_files
                .saturating_sub(capture.input.files.len())
        {
            return Err(resource("snapshot dependency count budget exceeded"));
        }
        let captured = Arc::get_mut(&mut capture).ok_or_else(|| {
            invalid_query("ASAR capture must be finalized before it is published or acquired")
        })?;
        captured._lease.control = Some(check);
        if captured.input.kind != TargetKind::File || captured.input.paths.len() != 1 {
            return Err(invalid_query(
                "ASAR dependency plan requires one archive input",
            ));
        }
        if captured.input.origin.is_some() && !dependencies.is_empty() {
            return Err(invalid_input(
                "extracted ASAR requires unpacked dependencies absent from its captured origin",
            ));
        }
        let original = &captured.input.files[0].original_path;
        let mut original_unpacked = original.as_os_str().to_owned();
        original_unpacked.push(".unpacked");
        let mut canonical_unpacked = captured
            .canonical_paths
            .first()
            .unwrap_or(original)
            .as_os_str()
            .to_owned();
        canonical_unpacked.push(".unpacked");
        let canonical_unpacked = PathBuf::from(canonical_unpacked);
        let mut snapshot_unpacked = captured.input.paths[0].as_os_str().to_owned();
        snapshot_unpacked.push(".unpacked");
        let original_unpacked = PathBuf::from(original_unpacked);
        let snapshot_unpacked = PathBuf::from(snapshot_unpacked);
        let mut seen = std::collections::HashSet::new();
        for dependency in dependencies {
            captured._lease.check()?;
            validate_entry(&dependency.entry)?;
            if dependency.entry.split('/').count() > self.limits.max_depth {
                return Err(resource("snapshot dependency path depth budget exceeded"));
            }
            if !seen.insert(&dependency.entry) {
                return Err(invalid_input("duplicate ASAR dependency in worker plan"));
            }
            if dependency.size
                > self
                    .limits
                    .max_capture_bytes
                    .saturating_sub(captured._lease.bytes) as u64
            {
                return Err(resource("snapshot dependency byte budget exceeded"));
            }
            let relative = PathBuf::from(&dependency.entry);
            let original_path = original_unpacked.join(&relative);
            // Open the declared path itself beneath the held root descriptor. ASAR's
            // filesystem dependency semantics reject symlinks at every component.
            let source = self.open_allowed(&canonical_unpacked.join(&relative), false)?;
            let destination = snapshot_unpacked.join(&relative);
            fs::create_dir_all(
                destination
                    .parent()
                    .ok_or_else(|| invalid_input("ASAR dependency has no parent"))?,
            )?;
            let info = copy_file(
                source,
                &destination,
                original_path,
                Some(relative),
                true,
                &mut captured._lease,
            )?;
            if info.input_bytes != dependency.size {
                return Err(invalid_input("unpacked file size differs from ASAR header"));
            }
            captured.input.files.push(info);
        }
        captured.input.captured_bytes = captured._lease.bytes as u64;
        captured.input.snapshot_id = if dependencies.is_empty() {
            captured.input.source_id.clone()
        } else {
            manifest_hash(&captured.input.files)
        };
        captured._lease.check()?;
        captured.input.asar_dependencies_captured = true;
        captured._lease.control = None;
        Ok(capture)
    }

    fn display_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_owned()
        } else {
            self.base.join(path)
        }
    }

    fn resolve(&self, path: &Path) -> Result<PathBuf> {
        let absolute = self.display_path(path);
        let canonical = fs::canonicalize(absolute)?;
        if canonical.starts_with(&self.storage_canonical) {
            return Err(Error::new(
                "ACCESS_DENIED",
                "managed snapshot storage must be selected through a target handle",
            ));
        }
        if canonical.to_str().is_none() {
            return Err(invalid_input("input paths must be UTF-8"));
        }
        if !self
            .roots
            .iter()
            .any(|root| canonical.starts_with(&root.path))
        {
            return Err(Error::new(
                "ACCESS_DENIED",
                "input resolves outside configured roots",
            ));
        }
        Ok(canonical)
    }

    fn open_allowed(&self, canonical: &Path, directory: bool) -> Result<File> {
        let root = self
            .roots
            .iter()
            .find(|root| canonical.starts_with(&root.path))
            .ok_or_else(|| {
                Error::new("ACCESS_DENIED", "input resolves outside configured roots")
            })?;
        #[cfg(unix)]
        let file = open_beneath(
            &root.directory,
            canonical
                .strip_prefix(&root.path)
                .map_err(|_| invalid_input("input is not under its allowed root"))?,
            directory,
        )?;
        #[cfg(not(unix))]
        let file = open_checked(canonical, &root.path, directory)?;
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
fn walk_directory(
    source: &File,
    relative: &Path,
    depth: usize,
    state: &mut WalkState<'_>,
) -> Result<()> {
    state.lease.check()?;
    let before = source.metadata()?;
    if depth > state.lease.limits.max_depth {
        return Err(resource("snapshot directory depth budget exceeded"));
    }
    let names = directory_names(
        source,
        &state.canonical_root.join(relative),
        &mut state.entries,
        state.lease.limits.max_directory_entries,
        state.lease.control.as_ref(),
    )?;
    for (name, kind) in names {
        state.lease.check()?;
        if name.to_str().is_none() {
            return Err(invalid_input("recursive input paths must be UTF-8"));
        }
        validate_snapshot_component(&name)?;
        let relative_path = relative.join(&name);
        let original_path = state.original_root.join(&relative_path);
        let canonical_path = state.canonical_root.join(&relative_path);
        if canonical_path.starts_with(state.excluded_root) {
            continue; // Capturing a broad root must not recursively capture its own growing storage.
        }
        let destination_path = state.destination_root.join(&relative_path);
        match kind {
            LocalKind::Directory => {
                let child = open_child_directory(source, &name, &canonical_path)?;
                fs::create_dir(&destination_path)?;
                walk_directory(&child, &relative_path, depth + 1, state)?;
            }
            LocalKind::File
                if matches!(
                    Path::new(&name)
                        .extension()
                        .and_then(|extension| extension.to_str()),
                    Some("js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx")
                ) =>
            {
                if state.files.len() >= state.lease.limits.max_files {
                    return Err(resource("snapshot file count budget exceeded"));
                }
                let file = open_child_file(source, &name, &canonical_path)?;
                state.files.push(copy_file(
                    file,
                    &destination_path,
                    original_path,
                    Some(relative_path),
                    false,
                    state.lease,
                )?);
            }
            _ => {}
        }
    }
    if source.metadata()?.modified().ok() != before.modified().ok() {
        return Err(Error::new(
            "INPUT_CHANGED",
            "directory changed while its snapshot was captured",
        ));
    }
    Ok(())
}

struct WalkState<'a> {
    entries: usize,
    files: &'a mut Vec<InputInfo>,
    lease: &'a mut CaptureLease,
    original_root: &'a Path,
    canonical_root: &'a Path,
    destination_root: &'a Path,
    excluded_root: &'a Path,
}

#[derive(Clone, Copy)]
enum LocalKind {
    File,
    Directory,
    Other,
}

#[cfg(unix)]
fn directory_names(
    source: &File,
    _original: &Path,
    entries: &mut usize,
    max: usize,
    control: Option<&CaptureCheck>,
) -> Result<Vec<(std::ffi::OsString, LocalKind)>> {
    use std::{
        ffi::CStr,
        os::{fd::AsRawFd, unix::ffi::OsStrExt},
    };
    // SAFETY: source is live; atomically duplicate ownership with CLOEXEC so a
    // concurrently spawned analysis worker cannot inherit the temporary stream.
    let fd = unsafe { libc::fcntl(source.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: fd is a fresh descriptor and fdopendir takes ownership on success.
    let stream = unsafe { libc::fdopendir(fd) };
    if stream.is_null() {
        // SAFETY: fdopendir failed, so the duplicated descriptor remains ours.
        unsafe {
            libc::close(fd);
        }
        return Err(std::io::Error::last_os_error().into());
    }
    struct DirectoryStream(*mut libc::DIR);
    impl Drop for DirectoryStream {
        fn drop(&mut self) {
            // SAFETY: this stream is owned, non-null, and closed once.
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    let stream = DirectoryStream(stream);
    let mut names = Vec::new();
    loop {
        check_capture(control)?;
        set_errno(0);
        // SAFETY: stream remains live; each returned record is used before the next call.
        let item = unsafe { libc::readdir(stream.0) };
        if item.is_null() {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(0) {
                return Err(error.into());
            }
            break;
        }
        // SAFETY: readdir returned a record with a NUL-terminated d_name.
        let name = unsafe { CStr::from_ptr((*item).d_name.as_ptr()) }.to_bytes();
        if matches!(name, b"." | b"..") {
            continue;
        }
        if *entries >= max {
            return Err(resource("snapshot directory entry budget exceeded"));
        }
        *entries += 1;
        let name = std::ffi::OsStr::from_bytes(name).to_owned();
        let kind = directory_kind(source, &name)?;
        names.push((name, kind));
    }
    names.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(names)
}

#[cfg(unix)]
fn set_errno(value: i32) {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    // SAFETY: errno location points to writable thread-local storage.
    unsafe {
        *libc::__errno_location() = value;
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    // SAFETY: errno location points to writable thread-local storage.
    unsafe {
        *libc::__error() = value;
    }
}

#[cfg(unix)]
fn directory_kind(source: &File, name: &std::ffi::OsStr) -> Result<LocalKind> {
    use std::{
        ffi::CString,
        os::{fd::AsRawFd, unix::ffi::OsStrExt},
    };
    let name =
        CString::new(name.as_bytes()).map_err(|_| invalid_input("NUL in directory entry"))?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: source and name remain live, stat is a writable output, and no symlink is followed.
    if unsafe {
        libc::fstatat(
            source.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: fstatat initialized stat on success above.
    let mode = unsafe { stat.assume_init() }.st_mode & libc::S_IFMT;
    Ok(if mode == libc::S_IFREG {
        LocalKind::File
    } else if mode == libc::S_IFDIR {
        LocalKind::Directory
    } else {
        LocalKind::Other
    })
}

#[cfg(unix)]
fn open_child_directory(source: &File, name: &std::ffi::OsStr, _original: &Path) -> Result<File> {
    open_beneath(source, Path::new(name), true)
}
#[cfg(unix)]
fn open_child_file(source: &File, name: &std::ffi::OsStr, _original: &Path) -> Result<File> {
    open_beneath(source, Path::new(name), false)
}

#[cfg(windows)]
fn directory_names(
    source: &File,
    _original: &Path,
    entries: &mut usize,
    max: usize,
    control: Option<&CaptureCheck>,
) -> Result<Vec<(std::ffi::OsString, LocalKind)>> {
    use std::os::windows::{ffi::OsStringExt, io::AsRawHandle};
    #[repr(C)]
    struct DirectoryRecord {
        next: u32,
        index: u32,
        times: [i64; 6],
        attributes: u32,
        name_bytes: u32,
        ea_size: u32,
        short_name_length: i8,
        short_name: [u16; 12],
        file_id: i64,
        name: [u16; 1],
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(
            handle: *mut std::ffi::c_void,
            class: i32,
            information: *mut std::ffi::c_void,
            bytes: u32,
        ) -> i32;
    }
    let mut buffer = vec![0u64; 8192]; // The Windows directory records require 8-byte alignment.
    let mut names = Vec::new();
    let mut class = 11; // FileIdBothDirectoryRestartInfo, followed by continuation class 10.
    loop {
        check_capture(control)?;
        // SAFETY: source is an opened directory, buffer is aligned writable memory.
        let status = unsafe {
            GetFileInformationByHandleEx(
                source.as_raw_handle(),
                class,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 8) as u32,
            )
        };
        class = 10;
        if status == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(18) {
                break;
            } // ERROR_NO_MORE_FILES.
            return Err(error.into());
        }
        let bytes = buffer.len() * 8;
        let name_start = std::mem::offset_of!(DirectoryRecord, name);
        let mut offset = 0usize;
        loop {
            check_capture(control)?;
            if offset
                .checked_add(std::mem::size_of::<DirectoryRecord>())
                .is_none_or(|end| end > bytes)
            {
                return Err(invalid_input("invalid Windows directory record"));
            }
            // SAFETY: the aligned buffer contains an OS-written record prefix within bounds.
            let record = unsafe {
                &*buffer
                    .as_ptr()
                    .cast::<u8>()
                    .add(offset)
                    .cast::<DirectoryRecord>()
            };
            let name_bytes = record.name_bytes as usize;
            if !name_bytes.is_multiple_of(2) || name_bytes > bytes - offset - name_start {
                return Err(invalid_input("invalid Windows directory name range"));
            }
            // SAFETY: the UTF-16 name range is aligned and checked within the OS buffer.
            let name = unsafe {
                std::slice::from_raw_parts(
                    buffer
                        .as_ptr()
                        .cast::<u8>()
                        .add(offset + name_start)
                        .cast::<u16>(),
                    name_bytes / 2,
                )
            };
            let name = std::ffi::OsString::from_wide(name);
            if name != "." && name != ".." {
                if *entries >= max {
                    return Err(resource("snapshot directory entry budget exceeded"));
                }
                *entries += 1;
                let kind = if record.attributes & (0x400 | 0x40) != 0 {
                    LocalKind::Other
                } else if record.attributes & 0x10 != 0 {
                    LocalKind::Directory
                } else {
                    LocalKind::File
                };
                names.push((name, kind));
            }
            if record.next == 0 {
                break;
            }
            let next = record.next as usize;
            if !next.is_multiple_of(8) || next < name_start || next > bytes - offset {
                return Err(invalid_input("invalid Windows directory continuation"));
            }
            offset += next;
        }
    }
    names.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(names)
}

#[cfg(not(any(unix, windows)))]
fn directory_names(
    _source: &File,
    _original: &Path,
    _entries: &mut usize,
    _max: usize,
    _control: Option<&CaptureCheck>,
) -> Result<Vec<(std::ffi::OsString, LocalKind)>> {
    Err(invalid_input(
        "snapshot directory handles are unsupported on this platform",
    ))
}
#[cfg(not(unix))]
fn open_child_directory(_source: &File, _name: &std::ffi::OsStr, original: &Path) -> Result<File> {
    open_checked(
        original,
        original
            .parent()
            .ok_or_else(|| invalid_input("directory has no parent"))?,
        true,
    )
}
#[cfg(not(unix))]
fn open_child_file(_source: &File, _name: &std::ffi::OsStr, original: &Path) -> Result<File> {
    open_checked(
        original,
        original
            .parent()
            .ok_or_else(|| invalid_input("file has no parent"))?,
        false,
    )
}

fn expire(registry: &mut Registry) {
    let now = Instant::now();
    registry.handles.retain(|_, handle| handle.expires > now);
}

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
    if before.len() > lease.limits.max_capture_bytes.saturating_sub(lease.bytes) as u64 {
        return Err(resource("snapshot capture byte budget exceeded"));
    }
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

fn validate_entry(entry: &str) -> Result<()> {
    if entry.len() > 4096 {
        return Err(resource("container path byte budget exceeded"));
    }
    if entry.is_empty()
        || entry.split('/').any(|component| {
            component.is_empty()
                || matches!(component, "." | "..")
                || component.contains(['\\', ':', '\0'])
        })
    {
        return Err(invalid_input(
            "container dependency requires a safe relative entry path",
        ));
    }
    for component in entry.split('/') {
        validate_snapshot_component(std::ffi::OsStr::new(component))?;
    }
    Ok(())
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

/// Windows normalizes short absolute paths before CreateFile, even with
/// create_new. Reject device/stream and aliased names before any private write.
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
    // dup/try_clone shares a directory stream offset with the configured root.
    // Reopen its static "." relative to the held descriptor for a fresh cursor.
    // SAFETY: root is live and the static C string is NUL terminated.
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
                "opened input resolves outside configured roots",
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
        "target is unknown, closed, expired, or belongs to another server process",
    )
}
fn poisoned() -> Error {
    Error::new("WORKER_FAILURE", "snapshot bookkeeping lock was poisoned")
}
