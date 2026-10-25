//! Print the embedded Agent Skill to stdout.

use std::io::Write;

/// The Agent Skill Markdown compiled into the binary.
const SKILL_MD: &str = include_str!("../skills/rxd/SKILL.md");

pub fn execute() -> rxd_core::Result<()> {
    std::io::stdout()
        .lock()
        .write_all(SKILL_MD.as_bytes())
        .map_err(|error| rxd_core::Error::new("SKILL_ERROR", error.to_string()))
}
