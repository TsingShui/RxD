//! Install the embedded Agent Skill into user-level agent directories.

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The frontmatter `name` and directory name of the installed skill.
pub const SKILL_NAME: &str = "rxd";

/// Embedded bytes shared by install and --print.
pub const SKILL_MD: &str = include_str!("../skills/rxd/SKILL.md");

/// Supported Agent Skill destinations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    Pi,
    Codex,
    Claude,
    /// The cross-agent `.agents/skills/` convention from the Agent Skills specification.
    Agents,
}

impl Agent {
    pub const ALL: [Agent; 4] = [Agent::Pi, Agent::Codex, Agent::Claude, Agent::Agents];

    /// Expand all and deduplicate selected agents.
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

    fn skills_dir(self, home: &Path, codex_home: &Path) -> PathBuf {
        match self {
            Agent::Pi => home.join(".pi/agent/skills"),
            Agent::Codex => codex_home.join("skills"),
            Agent::Claude => home.join(".claude/skills"),
            Agent::Agents => home.join(".agents/skills"),
        }
    }

    /// Detect existing agent directories; otherwise use the cross-agent location.
    fn detected(self, home: &Path, codex_home: &Path) -> bool {
        match self {
            Agent::Pi => home.join(".pi").is_dir(),
            Agent::Codex => codex_home.is_dir(),
            Agent::Claude => home.join(".claude").is_dir(),
            Agent::Agents => home.join(".agents/skills").is_dir(),
        }
    }
}

/// Install for detected agents, otherwise the cross-agent directory.
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

/// Resolve install destinations; --dir is a skills root and excludes explicit agents.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Installed,
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
