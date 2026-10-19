//! Local registration only: never starts the service, workers or client applications.
use anyhow::{Context, Result, bail};
use clap::{Args, Parser};
use serde_json::{Value, json};
use std::{
    env, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Args)]
pub struct Options {
    /// Native user target: pi, cc (claude/claude-code), codex, or all. Omit to detect.
    #[arg(short = 'a', long = "agent")]
    agents: Vec<String>,
    /// Allowed analysis roots; relative values resolve against --base.
    #[arg(long)]
    root: Vec<PathBuf>,
    /// Stable relative-path base. Initially defaults to the installation directory.
    #[arg(long)]
    base: Option<PathBuf>,
    /// Backend catalog filter (comma separated).
    #[arg(long, value_delimiter = ',')]
    backends: Vec<String>,
    /// Emit the planned registration without creating or changing files.
    #[arg(long)]
    preview: bool,
    /// Replace conflicting rxd startup fields, retaining other entry settings.
    #[arg(long)]
    replace: bool,
}

#[derive(Parser)]
struct ExistingServe {
    #[command(flatten)]
    options: super::Options,
}

struct Snapshot {
    bytes: Option<Vec<u8>>,
    metadata: Option<fs::Metadata>,
}
impl Snapshot {
    fn read(path: &Path) -> Result<Self> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    bytes: None,
                    metadata: None,
                });
            }
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            bail!("configuration must be a regular non-symlink file");
        }
        if metadata.len() > MAX_CONFIG_BYTES {
            bail!("configuration exceeds 4 MiB limit");
        }
        let mut bytes = Vec::new();
        let mut open = fs::OpenOptions::new();
        open.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            open.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = open.open(path)?;
        if !file.metadata()?.is_file() {
            bail!("configuration must be a regular file");
        }
        file.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            bail!("configuration exceeds 4 MiB limit");
        }
        Ok(Self {
            bytes: Some(bytes),
            metadata: Some(metadata),
        })
    }
    fn matches(&self, other: &Self) -> bool {
        self.bytes == other.bytes
            && match (&self.metadata, &other.metadata) {
                (None, None) => true,
                (Some(a), Some(b)) => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::MetadataExt;
                        a.dev() == b.dev()
                            && a.ino() == b.ino()
                            && a.mode() == b.mode()
                            && a.mtime() == b.mtime()
                            && a.mtime_nsec() == b.mtime_nsec()
                            && a.ctime() == b.ctime()
                            && a.ctime_nsec() == b.ctime_nsec()
                    }
                    #[cfg(not(unix))]
                    {
                        a.modified().ok() == b.modified().ok()
                            && a.permissions().readonly() == b.permissions().readonly()
                    }
                }
                _ => false,
            }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum Outcome {
    Installed,
    Updated,
    Unchanged,
    Conflict,
    Failed,
    NotWritten,
}

#[derive(Clone, serde::Serialize)]
struct TargetReport {
    agent: &'static str,
    path: PathBuf,
    outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    registration: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warnings: Option<Vec<&'static str>>,
}

#[derive(Debug)]
struct Conflict;
impl std::fmt::Display for Conflict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("conflict: rxd ownership/transport is ambiguous (including relocated custom executable names); use --replace to replace its startup fields")
    }
}
impl std::error::Error for Conflict {}

struct Plan {
    path: PathBuf,
    before: Snapshot,
    bytes: Vec<u8>,
    report: TargetReport,
}

fn utf8(path: &Path) -> Result<String> {
    Ok(path
        .to_str()
        .context("registration paths must be UTF-8")?
        .to_owned())
}
fn directory(path: &Path) -> Result<PathBuf> {
    let path = fs::canonicalize(path).context("base/root must be an existing directory")?;
    if !path.is_dir() {
        bail!("base/root must be a directory");
    }
    Ok(path)
}

// Query mount identity, not device/parent equality: APFS firmlinks and same-device
// bind mounts can otherwise silently authorize a whole mounted filesystem.
#[cfg(target_os = "macos")]
fn is_mount_root(path: &Path) -> Result<bool> {
    use std::{ffi::CStr, os::unix::ffi::OsStrExt};
    let name = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let mut info = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: name is NUL-terminated and info points to writable statfs storage.
    if unsafe { libc::statfs(name.as_ptr(), info.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: statfs initialized info on success; the fixed name array is read
    // within its bounds and checked for a terminator before interpretation.
    let info = unsafe { info.assume_init() };
    let bytes: Vec<u8> = info.f_mntonname.iter().map(|byte| *byte as u8).collect();
    let mount = CStr::from_bytes_until_nul(&bytes)?.to_bytes();
    if mount.is_empty() {
        bail!("filesystem returned an empty mount path");
    }
    Ok(fs::canonicalize(Path::new(std::ffi::OsStr::from_bytes(mount)))? == path)
}

#[cfg(target_os = "linux")]
fn is_mount_root(path: &Path) -> Result<bool> {
    use std::os::unix::ffi::OsStrExt;
    // The process's own namespace includes same-device bind mounts. Bound the
    // complete read; truncation or unavailable procfs must not grant authority.
    const MAX_MOUNTINFO_BYTES: u64 = 4 * 1024 * 1024;
    let mut bytes = Vec::new();
    fs::File::open("/proc/self/mountinfo")?
        .take(MAX_MOUNTINFO_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_MOUNTINFO_BYTES {
        bail!("mountinfo is empty or exceeds 4 MiB limit");
    }
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let encoded = line
            .split(|byte| *byte == b' ')
            .nth(4)
            .context("invalid mountinfo record")?;
        let mut mount = Vec::with_capacity(encoded.len());
        let mut index = 0;
        while index < encoded.len() {
            if encoded[index] == b'\\' {
                let escaped = encoded
                    .get(index + 1..index + 4)
                    .context("invalid mountinfo escape")?;
                mount.push(match escaped {
                    b"040" => b' ',
                    b"011" => b'\t',
                    b"012" => b'\n',
                    b"134" => b'\\',
                    _ => bail!("invalid mountinfo escape"),
                });
                index += 4;
            } else {
                mount.push(encoded[index]);
                index += 1;
            }
        }
        if mount == path.as_os_str().as_bytes() {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(windows)]
fn is_mount_root(path: &Path) -> Result<bool> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::GetVolumePathNameW;
    const MAX_VOLUME_PATH: usize = 32768;
    let mut name: Vec<u16> = path.as_os_str().encode_wide().collect();
    if name.len() >= MAX_VOLUME_PATH {
        bail!("volume path exceeds Windows path limit");
    }
    name.push(0);
    let mut volume = vec![0u16; MAX_VOLUME_PATH];
    // SAFETY: the input is NUL-terminated, and the output has the declared size.
    if unsafe { GetVolumePathNameW(name.as_ptr(), volume.as_mut_ptr(), volume.len() as u32) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let end = volume
        .iter()
        .position(|unit| *unit == 0)
        .filter(|end| *end > 0)
        .context("invalid Windows volume path")?;
    let mount = PathBuf::from(std::ffi::OsString::from_wide(&volume[..end]));
    Ok(fs::canonicalize(mount)? == path)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn is_mount_root(_path: &Path) -> Result<bool> {
    bail!("mount identity is unavailable on this platform; use explicit --root")
}

fn launch(options: &Options, old: Option<&Value>, executable: &Path) -> Result<Value> {
    if let Some(entry) = old {
        for key in ["command", "type", "url", "transport"] {
            if entry.get(key).is_some_and(|value| !value.is_string()) {
                bail!("rxd startup fields must be strings");
            }
        }
        if entry.get("args").is_some_and(|value| {
            value
                .as_array()
                .is_none_or(|args| args.iter().any(|arg| !arg.is_string()))
        }) {
            bail!("rxd args must be an array of strings");
        }
    }
    let prior = old.and_then(|entry| {
        let command = entry.get("command")?.as_str()?;
        let args: Vec<&str> = entry
            .get("args")?
            .as_array()?
            .iter()
            .map(Value::as_str)
            .collect::<Option<_>>()?;
        if (Path::new(command) != executable
            && !["rxd", "rxd.exe"]
                .iter()
                .any(|name| Path::new(command).file_name() == Some(std::ffi::OsStr::new(name))))
            || args.get(..2)? != ["mcp", "serve"]
            || entry.get("url").is_some()
            || entry.get("type").is_some_and(|value| value != "stdio")
            || entry.get("transport").is_some_and(|value| value != "stdio")
        {
            return None;
        }
        let serve = ExistingServe::try_parse_from(
            std::iter::once(command).chain(args.iter().skip(2).copied()),
        )
        .ok()?
        .options;
        Some((
            serve,
            args.into_iter().map(str::to_owned).collect::<Vec<_>>(),
        ))
    });
    if old.is_some() && prior.is_none() && !options.replace {
        bail!(Conflict);
    }
    let base = directory(
        &options
            .base
            .clone()
            .or_else(|| prior.as_ref().and_then(|(serve, _)| serve.base.clone()))
            .unwrap_or(env::current_dir()?),
    )?;
    let selected_roots = if !options.root.is_empty() {
        options.root.clone()
    } else {
        prior
            .as_ref()
            .map(|(serve, _)| serve.root.clone())
            .unwrap_or_default()
    };
    let roots = if selected_roots.is_empty() {
        if home_path()?.starts_with(&base)
            || base.parent().is_none()
            || is_mount_root(&base).context(
                "cannot determine filesystem/volume root; select an explicit analysis --root",
            )?
        {
            bail!(
                "refusing implicit home-wide/filesystem-wide access; select an intended analysis directory with --root"
            );
        }
        vec![base.clone()]
    } else {
        selected_roots
            .iter()
            .map(|root| {
                directory(&if root.is_absolute() {
                    root.clone()
                } else {
                    base.join(root)
                })
            })
            .collect::<Result<Vec<_>>>()?
    };
    let backends = if options.backends.is_empty() {
        prior
            .as_ref()
            .map(|(serve, _)| serve.backends.clone())
            .unwrap_or_default()
    } else {
        options.backends.clone()
    };
    let catalog = crate::query::catalog();
    for backend in &backends {
        if !catalog.iter().any(|tool| &tool.backend == backend) {
            bail!("unsupported MCP backend filter");
        }
    }
    let mut args = vec![
        "mcp".to_string(),
        "serve".to_string(),
        "--base".to_string(),
        utf8(&base)?,
    ];
    for root in roots {
        args.push("--root".into());
        args.push(utf8(&root)?);
    }
    if !backends.is_empty() {
        args.push("--backends".into());
        args.push(backends.join(","));
    }
    if let Some((_, old_args)) = prior {
        let mut iter = old_args.into_iter().skip(2);
        while let Some(arg) = iter.next() {
            if ["--base", "--root", "--backends"].contains(&arg.as_str()) {
                iter.next();
            } else if !["--base=", "--root=", "--backends="]
                .iter()
                .any(|prefix| arg.starts_with(prefix))
            {
                args.push(arg);
            }
        }
    }
    Ok(json!({"type":"stdio", "command":utf8(executable)?, "args":args}))
}

fn json_plan(options: &Options, agent: Agent, path: PathBuf, executable: &Path) -> Result<Plan> {
    let before = Snapshot::read(&path)?;
    let mut document: Value = match &before.bytes {
        Some(bytes) => {
            serde_json::from_slice::<UniqueJson>(bytes)
                .map_err(|_| anyhow::anyhow!("invalid or duplicate-key JSON configuration"))?
                .0
        }
        None => json!({}),
    };
    let document_map = document
        .as_object_mut()
        .context("configuration must be a JSON object")?;
    let servers = document_map
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("mcpServers must be an object")?;
    let old = servers.get("rxd");
    if old.is_some_and(|entry| !entry.is_object()) {
        bail!("rxd must be an object");
    }
    let registration = launch(options, old, executable)?;
    let mut entry = old.cloned().unwrap_or_else(|| json!({}));
    let map = entry.as_object_mut().unwrap();
    // These fields select incompatible transports, not user-owned stdio preferences.
    for key in ["url", "transport"] {
        map.remove(key);
    }
    for key in ["type", "command", "args"] {
        map.insert(key.into(), registration[key].clone());
    }
    let outcome = if old.is_none() {
        Outcome::Installed
    } else if old == Some(&entry) {
        Outcome::Unchanged
    } else {
        Outcome::Updated
    };
    servers.insert("rxd".into(), entry);
    let bytes = if outcome == Outcome::Unchanged {
        before.bytes.clone().unwrap()
    } else {
        let mut bytes = serde_json::to_vec_pretty(&document)?;
        bytes.push(b'\n');
        bytes
    };
    Ok(Plan {
        report: TargetReport {
            agent: agent.name(),
            path: path.clone(),
            outcome,
            registration: Some(registration),
            warnings: Some(if agent == Agent::Pi {
                pi_warnings(&path)
            } else {
                Vec::new()
            }),
        },
        path,
        before,
        bytes,
    })
}

fn pi_warnings(config: &Path) -> Vec<&'static str> {
    let mut warnings = Vec::new();
    let settings = config.with_file_name("settings.json");
    if let Ok(snapshot) = Snapshot::read(&settings)
        && let Some(settings) = snapshot
            .bytes
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        && let Some(extensions) = settings["extensions"].as_array()
    {
        for extension in extensions.iter().filter_map(Value::as_str) {
            if extension == "-builtin:mcp" {
                warnings.push("native_mcp_disabled");
            } else if !extension.starts_with('-') && extension.to_lowercase().contains("mcp") {
                warnings.push("possible_mcp_extension_override");
            }
        }
    }
    warnings.sort_unstable();
    warnings.dedup();
    warnings
}

struct Prepared {
    temporary: tempfile::NamedTempFile,
    parent_identity: PathBuf,
}
fn prepare(plan: &Plan) -> Result<Option<Prepared>> {
    if plan.report.outcome == Outcome::Unchanged {
        return Ok(None);
    }
    let parent = plan.path.parent().context("configuration has no parent")?;
    fs::create_dir_all(parent)?;
    let parent_identity = fs::canonicalize(parent)?;
    if let Some(metadata) = &plan.before.metadata
        && metadata.permissions().readonly()
    {
        bail!("configuration is read-only");
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    if let Some(metadata) = &plan.before.metadata {
        temporary
            .as_file()
            .set_permissions(metadata.permissions())?;
    }
    temporary.write_all(&plan.bytes)?;
    temporary.as_file().sync_all()?;
    Ok(Some(Prepared {
        temporary,
        parent_identity,
    }))
}
fn publish(plan: &Plan, prepared: Option<Prepared>) -> Result<()> {
    if !plan.before.matches(&Snapshot::read(&plan.path)?) {
        bail!("concurrent configuration modification detected; retry");
    }
    if let Some(prepared) = prepared {
        if fs::canonicalize(plan.path.parent().unwrap())? != prepared.parent_identity {
            bail!("concurrent configuration directory modification detected; retry");
        }
        prepared
            .temporary
            .persist(&plan.path)
            .map_err(|error| error.error)?;
    }
    Ok(())
}

pub fn execute(options: Options) -> rxd_core::Result<()> {
    run(options).map_err(|error| rxd_core::Error::new("INSTALL_FAILED", error.to_string()))
}
fn run(options: Options) -> Result<()> {
    let targets = targets(&options.agents)?;
    let executable = env::current_exe()?;
    for (_, path) in &targets {
        utf8(path)?;
    }
    let mut plans = Vec::new();
    let mut reports = Vec::new();
    let mut failed = false;
    for (agent, path) in targets {
        let plan = if agent == Agent::Codex {
            toml_plan(&options, path.clone(), &executable)
        } else {
            json_plan(&options, agent, path.clone(), &executable)
        }
        .and_then(|plan| {
            if plan.bytes.len() as u64 > MAX_CONFIG_BYTES {
                bail!("generated configuration exceeds 4 MiB limit");
            }
            Ok(plan)
        });
        match plan {
            Ok(plan) => {
                reports.push(plan.report.clone());
                plans.push(Some(plan));
            }
            Err(error) => {
                let message = error.to_string();
                reports.push(TargetReport {
                    agent: agent.name(),
                    path: path.clone(),
                    outcome: if error.is::<Conflict>() {
                        Outcome::Conflict
                    } else {
                        Outcome::Failed
                    },
                    registration: None,
                    warnings: None,
                });
                eprintln!("{} {}: {message}", agent.name(), path.display());
                plans.push(None);
                failed = true;
            }
        }
    }
    let mut prepared = Vec::new();
    if !failed && !options.preview {
        // Stage every writable target first. Parse and predictable filesystem failures
        // must not replace an earlier selected configuration.
        for (plan, report) in plans.iter().flatten().zip(&mut reports) {
            match prepare(plan) {
                Ok(file) => prepared.push(file),
                Err(error) => {
                    eprintln!("{} {}: {error}", report.agent, plan.path.display());
                    report.outcome = Outcome::Failed;
                    prepared.push(None);
                    failed = true;
                }
            }
        }
    }
    if failed {
        for report in &mut reports {
            if matches!(report.outcome, Outcome::Installed | Outcome::Updated) {
                report.outcome = Outcome::NotWritten;
            }
        }
    } else if !options.preview {
        for ((plan, report), file) in plans.iter().flatten().zip(&mut reports).zip(prepared) {
            if let Err(error) = publish(plan, file) {
                eprintln!("{} {}: {error}", report.agent, plan.path.display());
                report.outcome = Outcome::Failed;
                failed = true;
            }
        }
    }
    println!("{}", json!({"preview":options.preview, "targets":reports}));
    if failed {
        bail!("registration failed; inspect per-target outcomes (no cross-client transaction)");
    }
    eprintln!(
        "Registration persisted/planned only, not a verified session connection. Pi requires native MCP (tested 1.3.0); older clients must be upgraded separately. Reload Pi with /reload then /mcp; restart Claude Code or Codex and inspect /mcp. Project settings, policy, disabled servers/built-ins and Pi /mcp extensions can override registration; no enablement or policy was changed."
    );
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Agent {
    Pi,
    Cc,
    Codex,
}
impl Agent {
    const ALL: [Self; 3] = [Self::Pi, Self::Cc, Self::Codex];
    fn name(self) -> &'static str {
        match self {
            Self::Pi => "pi",
            Self::Cc => "cc",
            Self::Codex => "codex",
        }
    }
    fn executable(self) -> &'static str {
        match self {
            Self::Cc => "claude",
            _ => self.name(),
        }
    }
}
fn override_dir(key: &str, default: PathBuf, home: &Path) -> Result<PathBuf> {
    let value = env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or(default);
    let value = if key == "PI_CODING_AGENT_DIR" {
        if value == Path::new("~") {
            home.to_owned()
        } else if let Ok(tail) = value.strip_prefix("~/") {
            home.join(tail)
        } else {
            value
        }
    } else {
        value
    };
    Ok(if value.is_absolute() {
        value
    } else {
        env::current_dir()?.join(value)
    })
}
fn on_path(name: &str) -> bool {
    env::var_os("PATH").is_some_and(|path| {
        env::split_paths(&path).any(|dir| {
            let path = dir.join(if cfg!(windows) {
                format!("{name}.exe")
            } else {
                name.to_owned()
            });
            fs::metadata(path).is_ok_and(|metadata| {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
                }
                #[cfg(not(unix))]
                {
                    metadata.is_file()
                }
            })
        })
    })
}
fn home_path() -> Result<PathBuf> {
    let home = env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| env::var_os("USERPROFILE").filter(|value| !value.is_empty()))
        .context("nonempty HOME or USERPROFILE is required")?;
    let mut ancestor = std::path::absolute(PathBuf::from(home))?;
    let mut missing = Vec::new();
    loop {
        match fs::canonicalize(&ancestor) {
            Ok(mut resolved) => {
                for component in missing.iter().rev() {
                    resolved.push(component);
                }
                utf8(&resolved)?;
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(
                    ancestor
                        .file_name()
                        .context("cannot resolve home directory")?
                        .to_owned(),
                );
                ancestor.pop();
            }
            Err(error) => return Err(error.into()),
        }
    }
}
fn targets(names: &[String]) -> Result<Vec<(Agent, PathBuf)>> {
    let home = home_path()?;
    let pi = override_dir("PI_CODING_AGENT_DIR", home.join(".pi/agent"), &home)?;
    let cc = override_dir("CLAUDE_CONFIG_DIR", home.join(".claude"), &home)?;
    let codex = override_dir("CODEX_HOME", home.join(".codex"), &home)?;
    // Native Claude Code 2.1.301 resolves the legacy file first, even if both exist.
    let legacy = cc.join(".config.json");
    let cc_path = if fs::symlink_metadata(&legacy).is_ok() {
        legacy
    } else {
        env::var_os("CLAUDE_CONFIG_DIR")
            .filter(|value| !value.is_empty())
            .map(|_| cc.join(".claude.json"))
            .unwrap_or_else(|| home.join(".claude.json"))
    };
    let candidates = [
        (Agent::Pi, pi.join("mcp.json"), pi.is_dir()),
        (Agent::Cc, cc_path, cc.is_dir()),
        (Agent::Codex, codex.join("config.toml"), codex.is_dir()),
    ];
    let mut selected = Vec::new();
    for name in names {
        let agents: &[Agent] = match name.as_str() {
            "pi" => &[Agent::Pi],
            "cc" | "claude" | "claude-code" => &[Agent::Cc],
            "codex" => &[Agent::Codex],
            "all" => &Agent::ALL,
            _ => bail!("unsupported agent; choose pi, cc (claude/claude-code), codex or all"),
        };
        for agent in agents {
            if !selected.contains(agent) {
                selected.push(*agent);
            }
        }
    }
    let result: Vec<_> = candidates
        .into_iter()
        .filter(|(agent, path, directory_exists)| {
            if names.is_empty() {
                *directory_exists || path.exists() || on_path(agent.executable())
            } else {
                selected.contains(agent)
            }
        })
        .map(|(agent, path, _)| (agent, path))
        .collect();
    if result.is_empty() {
        bail!("no supported agent detected; explicitly select -a pi, -a cc, -a codex or -a all");
    }
    Ok(result)
}

fn toml_plan(options: &Options, path: PathBuf, executable: &Path) -> Result<Plan> {
    use toml_edit::{DocumentMut, Item, Table, value};
    let before = Snapshot::read(&path)?;
    let mut document = match &before.bytes {
        Some(bytes) => std::str::from_utf8(bytes)
            .ok()
            .and_then(|text| text.parse::<DocumentMut>().ok())
            .context("invalid TOML configuration")?,
        None => DocumentMut::new(),
    };
    let servers = document
        .as_table_mut()
        .entry("mcp_servers")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_like_mut()
        .context("mcp_servers must be a table")?;
    let old = if let Some(item) = servers.get("rxd") {
        let table = item.as_table_like().context("rxd must be a table")?;
        let mut fields = serde_json::Map::new();
        for key in ["command", "type", "url", "transport"] {
            if let Some(item) = table.get(key) {
                fields.insert(
                    key.into(),
                    json!(
                        item.as_str()
                            .context("rxd startup fields must be strings")?
                    ),
                );
            }
        }
        if let Some(item) = table.get("args") {
            let args = item
                .as_array()
                .context("rxd args must be an array")?
                .iter()
                .map(|value| value.as_str().context("rxd args must be strings"))
                .collect::<Result<Vec<_>>>()?;
            fields.insert("args".into(), json!(args));
        }
        Some(Value::Object(fields))
    } else {
        None
    };
    let mut registration = launch(options, old.as_ref(), executable)?;
    registration.as_object_mut().unwrap().remove("type");
    let outcome = if old.is_none() {
        Outcome::Installed
    } else if ["command", "args"]
        .iter()
        .all(|key| old.as_ref().unwrap().get(key) == registration.get(key))
        && ["type", "url", "transport"]
            .iter()
            .all(|key| old.as_ref().unwrap().get(key).is_none())
    {
        Outcome::Unchanged
    } else {
        Outcome::Updated
    };
    let entry = servers
        .entry("rxd")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_like_mut()
        .unwrap();
    for key in ["type", "url", "transport"] {
        entry.remove(key);
    }
    let mut args = toml_edit::Array::new();
    for arg in registration["args"].as_array().unwrap() {
        args.push(arg.as_str().unwrap());
    }
    for (key, mut item) in [
        ("command", value(registration["command"].as_str().unwrap())),
        ("args", value(args)),
    ] {
        if let Some(existing) = entry.get(key).and_then(Item::as_value) {
            *item.as_value_mut().unwrap().decor_mut() = existing.decor().clone();
        }
        entry.insert(key, item);
    }
    let bytes = if outcome == Outcome::Unchanged {
        before.bytes.clone().unwrap()
    } else {
        document.to_string().into_bytes()
    };
    // Validate the complete generated document before any publication.
    std::str::from_utf8(&bytes)?
        .parse::<DocumentMut>()
        .context("cannot encode TOML registration")?;
    Ok(Plan {
        report: TargetReport {
            agent: "codex",
            path: path.clone(),
            outcome,
            registration: Some(registration),
            warnings: None,
        },
        path,
        before,
        bytes,
    })
}

// Duplicate keys make selective preservation ambiguous; serde_json::Value alone
// silently accepts the last value. Keep the parser's normal recursion limit.
struct UniqueJson(Value);
impl<'de> serde::Deserialize<'de> for UniqueJson {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueJson;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("JSON without duplicate keys")
            }
            fn visit_bool<E: serde::de::Error>(
                self,
                value: bool,
            ) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(json!(value)))
            }
            fn visit_i64<E: serde::de::Error>(
                self,
                value: i64,
            ) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(json!(value)))
            }
            fn visit_u64<E: serde::de::Error>(
                self,
                value: u64,
            ) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(json!(value)))
            }
            fn visit_f64<E: serde::de::Error>(
                self,
                value: f64,
            ) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(json!(value)))
            }
            fn visit_str<E: serde::de::Error>(
                self,
                value: &str,
            ) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(json!(value)))
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(UniqueJson(value)) = seq.next_element()? {
                    values.push(value);
                }
                Ok(UniqueJson(Value::Array(values)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some((key, UniqueJson(value))) = map.next_entry::<String, UniqueJson>()? {
                    if values.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate JSON key"));
                    }
                }
                Ok(UniqueJson(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}
