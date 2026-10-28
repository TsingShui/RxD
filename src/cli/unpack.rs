//! Whole-archive extraction for ASAR, ZIP/HAP, PyInstaller and wxapkg containers.
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use clap::Args;
use rxd_core::{Error, Limits, Result, perf::Perf};

#[derive(Args)]
pub(crate) struct Command {
    /// ASAR, ZIP-family (including HAP), PyInstaller or wxapkg input, or PyInstaller one-dir root.
    pub(super) archive: PathBuf,
    /// New directory that receives every extracted file.
    #[arg(short, long, value_name = "DIR")]
    pub(super) output: PathBuf,
    /// Parallel ZIP-family extraction workers; deflated trees scale further than Stored ones.
    #[arg(long, default_value_t = super::default_threads(), value_parser = super::positive)]
    pub(super) threads: usize,
    /// Emit phase timings as JSONL on stderr.
    #[arg(long)]
    pub(super) perf: bool,
    /// WeChat appid for encrypted V1MMWX wxapkg inputs.
    #[arg(long, value_name = "WXID")]
    pub(super) appid: Option<String>,
}

enum Format {
    Asar,
    Zip,
    PyInstaller,
    Wxapkg,
}

fn detect(path: &Path) -> Result<Format> {
    let mut file = fs::File::open(path)?;
    let mut magic = [0u8; 6];
    file.read_exact(&mut magic)
        .map_err(|_| Error::new("INVALID_INPUT", "archive is too short"))?;
    if magic[..4] == [4, 0, 0, 0] {
        return Ok(Format::Asar);
    }
    if magic.starts_with(b"PK") {
        return Ok(Format::Zip);
    }
    if rxd_wxapkg::detect(path)? {
        return Ok(Format::Wxapkg);
    }
    if magic.starts_with(b"V1MMWX") {
        return Ok(Format::Wxapkg);
    }
    if rxd_pyinstaller::detect(path)? {
        return Ok(Format::PyInstaller);
    }
    Err(Error::new(
        "UNSUPPORTED_INPUT",
        "expected an ASAR, ZIP-family (including HAP), PyInstaller or wxapkg input",
    ))
}

struct Receipt<'a> {
    format: &'a str,
    output: &'a Path,
    files: usize,
    bytes: u64,
    pyinstaller: Option<&'a rxd_pyinstaller::Summary>,
    wxapkg: Option<&'a rxd_wxapkg::Summary>,
    layout: Option<&'a str>,
    launcher: Option<&'a Path>,
}

fn receipt(receipt: Receipt<'_>) -> Vec<u8> {
    let Receipt {
        format,
        output,
        files,
        bytes,
        pyinstaller,
        wxapkg,
        layout,
        launcher,
    } = receipt;
    let mut detail = serde_json::json!({
        "complete": true,
        "format": format,
        "path": output.to_string_lossy(),
        "files": files,
        "bytes": bytes,
    });
    if let Some(summary) = pyinstaller {
        detail["python"] = serde_json::json!(summary.python);
        detail["pyz_files"] = serde_json::json!(summary.pyz_files);
        detail["pyz_bytes"] = serde_json::json!(summary.pyz_bytes);
        detail["skipped"] = serde_json::json!(summary.skipped);
    }
    if let Some(summary) = wxapkg {
        detail["encrypted"] = serde_json::json!(summary.encrypted);
        detail["appid"] = serde_json::json!(summary.appid);
    }
    if let Some(layout) = layout {
        detail["layout"] = serde_json::json!(layout);
    }
    if let Some(launcher) = launcher {
        detail["launcher"] = serde_json::json!(launcher.to_string_lossy());
    }
    let mut line = serde_json::to_vec(&serde_json::json!({
        "record": "unpack",
        "detail": detail,
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
    let mut perf = Perf::new(command.perf, "analysis", "unpack", "unpack");
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
        let onedir = command.archive.is_dir();
        let archive = if onedir {
            rxd_pyinstaller::find_launcher(&command.archive)?
        } else {
            command.archive.clone()
        };
        let budget = policy.for_file(&archive)?;
        let limits = Limits::from(budget);
        perf.limits = Some(limits);
        let format = if onedir {
            Format::PyInstaller
        } else {
            detect(&archive)?
        };
        fs::create_dir(&command.output)?;
        let mut pyinstaller = None;
        let mut wxapkg = None;
        let extracted = match format {
            Format::Asar => rxd_asar::unpack(
                &archive,
                &command.output,
                limits,
                budget.entries,
                budget.metadata_bytes,
                &mut perf,
            )
            .map(|result| ("asar", result.files, result.bytes)),
            Format::Zip => rxd_zip::unpack_filtered(
                &archive,
                &command.output,
                budget,
                &mut perf,
                command.threads,
                &|_| true,
            )
            .map(|result| ("zip", result.files, result.bytes)),
            Format::PyInstaller => rxd_pyinstaller::unpack(
                &archive,
                &command.output,
                limits,
                budget.entries,
                &mut perf,
            )
            .map(|result| {
                pyinstaller = Some(result);
                ("pyinstaller", 0, 0)
            }),
            Format::Wxapkg => rxd_wxapkg::unpack(
                &archive,
                &command.output,
                limits,
                budget.entries,
                command.appid.as_deref(),
                &mut perf,
            )
            .map(|result| {
                let files = result.files;
                let bytes = result.bytes;
                wxapkg = Some(result);
                ("wxapkg", files, bytes)
            }),
        };
        match extracted {
            Ok((format, files, bytes)) => {
                let pyinstaller = pyinstaller.as_ref();
                let files = pyinstaller.map_or(files, |summary| summary.files);
                let bytes = pyinstaller.map_or(bytes, |summary| summary.bytes);
                let layout =
                    (format == "pyinstaller").then_some(if onedir { "onedir" } else { "onefile" });
                let launcher = onedir.then_some(archive.as_path());
                let bytes = receipt(Receipt {
                    format,
                    output: &command.output,
                    files,
                    bytes,
                    pyinstaller,
                    wxapkg: wxapkg.as_ref(),
                    layout,
                    launcher,
                });
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
