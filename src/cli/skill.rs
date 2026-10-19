//! `rxd skill install`: installs the bundled rxd skill for coding agents.
//!
//! The skill is a single `SKILL.md` in the Agent Skills layout
//! (`<skills-dir>/<name>/SKILL.md`), embedded in the binary so a release build can set up an
//! agent without a checkout. Installing is a plain file write rather than a call into the
//! agents' own tooling, and each target is the user-level directory that agent documents:
//!
//! | Agent | User-level skills directory |
//! |---|---|
//! | pi | `~/.pi/agent/skills` |
//! | Codex | `$CODEX_HOME/skills`, else `~/.codex/skills` |
//! | Claude Code | `~/.claude/skills` |
//! | cross-agent | `~/.agents/skills` |
//!

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The frontmatter `name` and directory name of the installed skill.
pub const SKILL_NAME: &str = "rxd";

/// The skill text, written verbatim.
///
/// Kept as a plain file under `src/skills/rxd/` so it is reviewed as documentation, not
/// escaped inside a Rust string. `--print` emits exactly these bytes, which is what makes
/// that flag useful for agents this command does not know about.
pub const SKILL_MD: &str = include_str!("../skills/rxd/SKILL.md");

/// An agent whose skill directory `rxd skill install` knows how to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    Pi,
    Codex,
    Claude,
    /// The cross-agent `.agents/skills/` convention from the Agent Skills specification.
    Agents,
}

impl Agent {
    /// Every agent, in the order `all` installs them.
    pub const ALL: [Agent; 4] = [Agent::Pi, Agent::Codex, Agent::Claude, Agent::Agents];

    /// Parses the `AGENT` arguments. `all` expands, duplicates collapse.
    fn parse(names: &[String]) -> Result<Vec<Agent>> {
        let mut selected: Vec<Agent> = Vec::new();
        for name in names {
            let expanded: &[Agent] = match name.to_ascii_lowercase().as_str() {
                "pi" => &[Agent::Pi],
                "codex" => &[Agent::Codex],
                "claude" | "claude-code" => &[Agent::Claude],
                "agents" => &[Agent::Agents],
                "all" => &Agent::ALL,
                other => bail!("unknown agent `{other}` (want pi, codex, claude, agents or all)"),
            };
            for agent in expanded {
                if !selected.contains(agent) {
                    selected.push(*agent);
                }
            }
        }
        Ok(selected)
    }

    /// The user-level skills directory this agent scans.
    ///
    /// `codex_home` is passed in instead of read from the environment here, so the paths
    /// stay testable without touching process-wide state.
    fn skills_dir(self, home: &Path, codex_home: &Path) -> PathBuf {
        match self {
            Agent::Pi => home.join(".pi/agent/skills"),
            Agent::Codex => codex_home.join("skills"),
            Agent::Claude => home.join(".claude/skills"),
            Agent::Agents => home.join(".agents/skills"),
        }
    }

    /// Whether this agent looks like it is in use on this machine.
    ///
    /// Only each agent's own directory is consulted, so a machine using none of them ends up
    /// with the cross-agent location instead of a fresh dotdir per unused tool.
    fn detected(self, home: &Path, codex_home: &Path) -> bool {
        match self {
            Agent::Pi => home.join(".pi").is_dir(),
            Agent::Codex => codex_home.is_dir(),
            Agent::Claude => home.join(".claude").is_dir(),
            Agent::Agents => home.join(".agents/skills").is_dir(),
        }
    }
}

/// `rxd skill install` without `AGENT` installs for every agent that looks installed; when none
/// does, the cross-agent location is the only sensible place left.
fn default_agents(home: &Path, codex_home: &Path) -> Vec<Agent> {
    let installed: Vec<Agent> = [Agent::Pi, Agent::Codex, Agent::Claude]
        .into_iter()
        .filter(|agent| agent.detected(home, codex_home))
        .collect();
    if installed.is_empty() {
        vec![Agent::Agents]
    } else {
        installed
    }
}

/// The `SKILL.md` path inside a skills directory.
fn skill_path(skills_dir: &Path) -> PathBuf {
    skills_dir.join(SKILL_NAME).join("SKILL.md")
}

/// Resolves the `SKILL.md` paths to write from the command line.
///
/// An empty `agents` means auto-detection. `--dir`, when given, is the whole destination - a
/// skills directory such as `.claude/skills`, not the skill directory itself - and cannot be
/// combined with an explicit `AGENT`, because silently ignoring one of the two would hide a
/// typo in the other.
pub fn targets(agents: &[String], dir: Option<&Path>) -> Result<Vec<PathBuf>> {
    if let Some(dir) = dir {
        if !agents.is_empty() {
            bail!("AGENT and --dir are mutually exclusive; --dir already names one destination");
        }
        return Ok(vec![skill_path(dir)]);
    }
    let home = home_dir()?;
    let codex_home = codex_home(&home);
    let agents = if agents.is_empty() {
        default_agents(&home, &codex_home)
    } else {
        Agent::parse(agents)?
    };
    Ok(agents
        .into_iter()
        .map(|agent| skill_path(&agent.skills_dir(&home, &codex_home)))
        .collect())
}

/// What installing at one path did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The skill was not there before.
    Installed,
    /// An existing skill was replaced.
    Updated,
    /// The file already had exactly this content.
    Unchanged,
}

impl Outcome {
    /// The word the CLI prints in front of the path.
    pub fn verb(self) -> &'static str {
        match self {
            Outcome::Installed => "installed",
            Outcome::Updated => "updated",
            Outcome::Unchanged => "unchanged",
        }
    }
}

/// Writes the bundled skill to `path`, creating the skill directory.
///
/// Re-running this command is the normal way to update the skill, so an identical file is
/// left untouched and reported as such instead of being rewritten.
pub fn install(path: &Path) -> Result<Outcome> {
    let outcome = match fs::read(path) {
        Ok(current) if current == SKILL_MD.as_bytes() => return Ok(Outcome::Unchanged),
        Ok(_) => Outcome::Updated,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Outcome::Installed,
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let directory = path.parent().context("skill path has no directory")?;
    fs::create_dir_all(directory).with_context(|| format!("create {}", directory.display()))?;
    fs::write(path, SKILL_MD).with_context(|| format!("write {}", path.display()))?;
    Ok(outcome)
}

/// The home directory, as the agents themselves resolve it.
fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .context("cannot determine the home directory (HOME is unset); use --dir")
}

/// Codex reads `$CODEX_HOME`; `~/.codex` is only the fallback.
fn codex_home(home: &Path) -> PathBuf {
    env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| home.join(".codex"))
}

#[derive(Subcommand)]
pub enum Command {
    /// Install or update the embedded rxd skill for coding agents.
    Install(InstallArgs),
}

#[derive(Args)]
pub struct InstallArgs {
    /// Agents: pi, codex, claude, claude-code, agents or all (case-insensitive).
    /// Default: detected Pi, Codex and Claude directories, else ~/.agents/skills.
    #[arg(short = 'a', long = "agent", value_name = "AGENT", num_args = 1..)]
    agents: Vec<String>,
    /// Skills root directory; the skill is written to DIR/rxd/SKILL.md.
    #[arg(short, long, value_name = "DIR")]
    dir: Option<PathBuf>,
    /// Print the exact embedded skill without writing files.
    #[arg(long)]
    print: bool,
}

pub fn execute(command: Command) -> rxd_core::Result<()> {
    execute_inner(command)
        .map_err(|error| rxd_core::Error::new("SKILL_ERROR", format!("{error:#}")))
}

fn execute_inner(command: Command) -> Result<()> {
    let Command::Install(args) = command;
    if args.print {
        if args.dir.is_some() || !args.agents.is_empty() {
            bail!("--print writes the skill to stdout; drop --agent and --dir");
        }
        std::io::stdout().lock().write_all(SKILL_MD.as_bytes())?;
    } else {
        for path in targets(&args.agents, args.dir.as_deref())? {
            let outcome = install(&path)?;
            eprintln!("rxd skill {} at {}", outcome.verb(), path.display());
        }
    }
    Ok(())
}
