use std::{io::Write, path::Path};

use anyhow::{Result, ensure};
use rxd_core::{output::Output, perf::Perf};
use rxd_dotnet::{Bundle, Limits, Resources};

use super::Command;
use crate::cli::table::{self, Cell};

pub(super) fn validate_output(input: &Path, output: Option<&Path>) -> Result<()> {
    let Some(output) = output else {
        return Ok(());
    };
    let source = std::fs::canonicalize(input)?;
    let destination = std::fs::canonicalize(output).ok().or_else(|| {
        let directory = output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        Some(
            std::fs::canonicalize(directory)
                .ok()?
                .join(output.file_name()?),
        )
    });
    let same = destination.as_deref() == Some(source.as_path());
    #[cfg(unix)]
    let same = {
        use std::os::unix::fs::MetadataExt;
        same || std::fs::metadata(input)
            .ok()
            .zip(std::fs::metadata(output).ok())
            .is_some_and(|(left, right)| left.dev() == right.dev() && left.ino() == right.ino())
    };
    ensure!(
        !same,
        "INVALID_QUERY: output cannot overwrite the analyzed input"
    );
    Ok(())
}

pub(super) fn execute(
    command: &Command,
    bytes: &[u8],
    limits: Limits,
    perf: &mut Perf,
    sink: &mut crate::cli::Output<'_>,
) -> Result<bool> {
    if !matches!(
        command,
        Command::Entries { .. }
            | Command::ExtractEntry { .. }
            | Command::Resources { .. }
            | Command::ExtractResource { .. }
    ) {
        return Ok(false);
    }
    perf.source_id = Some(rxd_core::identity::input_id(bytes));
    let start = perf.start_stage();
    let mut output = Output::new(limits.output_bytes);
    let records = match command {
        Command::Entries { filter, case, .. } => {
            let bundle = Bundle::parse(bytes, limits)?.ok_or_else(|| {
                anyhow::anyhow!("UNSUPPORTED_INPUT: input is not a .NET single-file bundle")
            })?;
            let pattern = filter
                .as_deref()
                .map(|text| rxd_core::filter::Contains::new(text, case.sensitive()));
            output
                .write_all(table::header(&["at", "kind", "size", "compressed_size"]).as_bytes())?;
            let mut rows = 0;
            perf.counters.entities = Some(bundle.entries.len());
            for entry in &bundle.entries {
                if pattern
                    .as_ref()
                    .is_some_and(|pattern| !pattern.matches(entry.path))
                {
                    continue;
                }
                output.write_all(
                    table::line(&[
                        Cell::Text(entry.path),
                        Cell::Text(entry.kind),
                        Cell::Atom(&entry.size.to_string()),
                        Cell::Atom(&entry.compressed_size.to_string()),
                    ])?
                    .as_bytes(),
                )?;
                rows += 1;
            }
            rows
        }
        Command::ExtractEntry { at, .. } => {
            let bundle = Bundle::parse(bytes, limits)?.ok_or_else(|| {
                anyhow::anyhow!("UNSUPPORTED_INPUT: input is not a .NET single-file bundle")
            })?;
            let payload = bundle.extract(at, limits.output_bytes)?;
            output.write_all(&payload)?;
            1
        }
        Command::Resources {
            within,
            filter,
            case,
            ..
        } => {
            let resources = Resources::parse(bytes, limits)?;
            let members;
            let resources = if let Some(at) = within {
                members = resources.members(at, limits)?;
                &members
            } else {
                &resources.roots
            };
            let pattern = filter
                .as_deref()
                .map(|text| rxd_core::filter::Contains::new(text, case.sensitive()));
            output.write_all(
                table::header(&["at", "name", "type", "size", "parent_at"]).as_bytes(),
            )?;
            perf.counters.entities = Some(resources.len());
            let mut rows = 0;
            for resource in resources {
                if pattern
                    .as_ref()
                    .is_some_and(|pattern| !pattern.matches(&resource.name))
                {
                    continue;
                }
                let size = resource.size().map(|size| size.to_string());
                output.write_all(
                    table::line(&[
                        Cell::Atom(&resource.at),
                        Cell::Text(&resource.name),
                        Cell::Text(&resource.resource_type),
                        size.as_deref().map_or(Cell::Null, Cell::Atom),
                        resource.parent_at.as_deref().map_or(Cell::Null, Cell::Atom),
                    ])?
                    .as_bytes(),
                )?;
                rows += 1;
            }
            rows
        }
        Command::ExtractResource { at, .. } => {
            let resources = Resources::parse(bytes, limits)?;
            output.write_all(resources.extract(at, limits)?)?;
            1
        }
        _ => unreachable!(),
    };
    perf.record("query", start);
    perf.counters.records = Some(records);
    perf.counters.output_bytes = Some(output.len());
    perf.measure("write", || {
        crate::cli::write_output(&output, command.options().output.as_deref(), sink)
    })?;
    Ok(true)
}
