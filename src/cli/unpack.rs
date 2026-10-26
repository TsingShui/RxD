//! Whole-archive extraction for ASAR and ZIP containers.
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use clap::Args;
use rxd_core::{Error, Limits, Result, perf::Perf};

#[derive(Args)]
pub(crate) struct Command {
    /// ASAR or ZIP-family archive to unpack.
    pub(super) archive: PathBuf,
    /// New directory that receives every extracted file.
    #[arg(short, long, value_name = "DIR")]
    pub(super) output: PathBuf,
    /// Emit phase timings as JSONL on stderr.
    #[arg(long)]
    pub(super) perf: bool,
}

enum Format {
    Asar,
    Zip,
}

fn detect(path: &Path) -> Result<Format> {
    let mut file = fs::File::open(path)?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)
        .map_err(|_| Error::new("INVALID_INPUT", "archive is too short"))?;
    if magic == [4, 0, 0, 0] {
        return Ok(Format::Asar);
    }
    if magic.starts_with(b"PK") {
        return Ok(Format::Zip);
    }
    Err(Error::new(
        "UNSUPPORTED_INPUT",
        "expected an ASAR or ZIP archive",
    ))
}

fn receipt(format: &str, output: &Path, files: usize, bytes: u64) -> Vec<u8> {
    let mut line = serde_json::to_vec(&serde_json::json!({
        "record": "unpack",
        "detail": {
            "complete": true,
            "format": format,
            "path": output.to_string_lossy(),
            "files": files,
            "bytes": bytes,
        }
    }))
    .expect("receipt serializes");
    line.push(b'\n');
    line
}

pub(crate) fn execute(
    command: Command,
    policy: rxd_core::BudgetPolicy,
    sink: &mut super::Output<'_>,
) -> Result<()> {
    let budget = policy.for_file(&command.archive)?;
    let limits = Limits::from(budget);
    let mut perf = Perf::new(command.perf, "analysis", "unpack", "unpack");
    perf.limits = Some(limits);
    let result = (|| {
        if command.output.symlink_metadata().is_ok() {
            return Err(Error::new(
                "INVALID_QUERY",
                "output directory must not already exist",
            ));
        }
        if let (Ok(input), Ok(output)) = (
            command.archive.canonicalize(),
            command.output.canonicalize(),
        ) && input == output
        {
            return Err(Error::new(
                "INVALID_QUERY",
                "output must not replace the input archive",
            ));
        }
        let format = detect(&command.archive)?;
        fs::create_dir(&command.output)?;
        let extracted = match format {
            Format::Asar => rxd_asar::unpack(
                &command.archive,
                &command.output,
                limits,
                budget.entries,
                budget.metadata_bytes,
                &mut perf,
            )
            .map(|result| ("asar", result.files, result.bytes)),
            Format::Zip => rxd_zip::unpack(&command.archive, &command.output, budget, &mut perf)
                .map(|result| ("zip", result.files, result.bytes)),
        };
        match extracted {
            Ok((format, files, bytes)) => {
                let bytes = receipt(format, &command.output, files, bytes);
                perf.measure("write", || super::write_output(&bytes, None, sink))
            }
            Err(error) => {
                let _ = fs::remove_dir_all(&command.output);
                Err(error)
            }
        }
    })();
    super::finish(perf, command.perf, result)
}
