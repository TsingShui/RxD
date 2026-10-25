use std::{io::Write, path::PathBuf};

use super::{filter::CaseArgs, finish, write_output, xref::Selector};
use clap::{Args, Subcommand};
use rxd_core::{BudgetPolicy, Result, perf::Perf};

#[derive(Args)]
pub(crate) struct Common {
    /// One regular local ZIP-family archive, identified by its contents.
    input: PathBuf,
    /// Write output atomically; extract writes original entry bytes here.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Write timing/resource JSONL to stderr.
    #[arg(long)]
    perf: bool,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Enumerate central-directory records without decoding payloads.
    #[command(
        after_long_help = "Examples:\n  rxd zip entries sdk.aar -f jar\n  rxd zip entries extension.vsix -f main\nOutput: file JSONL; at is zip:<central-directory-index>, including duplicate names.\nCopy at to zip extract or a code backend's --entry. Directory/link/unsafe entries remain explicit."
    )]
    Entries {
        #[command(flatten)]
        common: Common,
        /// Substring over the entry path, folded unless --case sensitive.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
    },
    /// Decode one Stored/Deflate file into --output and return its receipt.
    #[command(mut_arg("at", |arg| arg.required(true).value_name("ENTRY_AT").help("Copy zip entries.at; a unique exact entry path also works")), mut_arg("output", |arg| arg.required(true)), after_long_help = "Examples:\n  rxd zip extract sdk.aar --at classes.jar -o target/classes.jar\n  rxd jar classes target/classes.jar\nRequired: --at and -o FILE. Duplicate names require zip:N coordinates.\nOutput: original entry bytes in FILE and one extraction receipt on stdout.\nNo encrypted entries, link following, bulk extraction, or automatic nested recursion.")]
    Extract {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        selector: Selector,
    },
}

impl Command {
    fn common(&self) -> &Common {
        match self {
            Self::Entries { common, .. } | Self::Extract { common, .. } => common,
        }
    }
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Entries { .. } => "entries",
            Self::Extract { .. } => "extract",
        }
    }
    pub(crate) fn perf(&self) -> bool {
        self.common().perf
    }
}

pub(super) fn execute(
    command: Command,
    policy: BudgetPolicy,
    sink: &mut super::Output<'_>,
) -> Result<()> {
    let common = command.common();
    let input = common.input.clone();
    let destination = common.output.clone();
    let emit_perf = common.perf;
    let budget = policy.for_file(&input)?;
    let mut perf = Perf::new(emit_perf, "analysis", "zip", command.name());
    perf.limits = Some(budget.into());
    let query = match command {
        Command::Entries { filter, case, .. } => rxd_zip::Query::Entries {
            contains: filter,
            case_sensitive: case.sensitive(),
        },
        Command::Extract { selector, .. } => rxd_zip::Query::Extract {
            at: selector.resolve().map_err(super::classify)?,
        },
    };
    let result = (|| {
        let response = rxd_zip::run(
            query,
            rxd_zip::Options {
                input: &input,
                output: destination.as_deref(),
                budget,
            },
            &mut perf,
        )?;
        perf.measure("write", || match response {
            rxd_zip::Response::List(bytes) => write_output(&bytes, destination.as_deref(), sink),
            rxd_zip::Response::Extract { bytes, receipt } => {
                write_output(&bytes, destination.as_deref(), sink)?;
                sink.write_all(&receipt).map_err(rxd_core::Error::from)
            }
        })
    })();
    finish(perf, emit_perf, result)
}
