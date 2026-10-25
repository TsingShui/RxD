//! Selected container bytes and admission shared by code-backend adapters.
use std::{fs::OpenOptions, io::Read, path::Path};

#[cfg(feature = "backend-asar")]
use rxd_core::Limits;
use rxd_core::{Budget, BudgetPolicy, Error, Origin, Result, perf::Perf};

#[derive(clap::Args)]
pub(crate) struct EntryArgs {
    /// Select an ASAR/ZIP entry; --at still selects an object inside its contents.
    #[arg(
        long,
        value_name = "ENTRY",
        long_help = "Analyze one container entry in memory. Use an ASAR path, ZIP entries.at, or a unique exact ZIP path. Coordinates remain entry-relative.\nExample: rxd js entities extension.vsix --entry extension/dist/main.ts"
    )]
    pub entry: Option<String>,
}

pub(super) struct Entry {
    pub bytes: Vec<u8>,
    #[cfg(any(feature = "backend-js", feature = "backend-html"))]
    pub path: String,
    pub budget: Budget,
    pub origin: Origin,
}

fn magic(input: &Path) -> Result<[u8; 4]> {
    if !input.symlink_metadata()?.is_file() {
        return Err(Error::new(
            "INVALID_INPUT",
            "container input must be a regular file, not a symlink",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    let mut file = options.open(input)?;
    if !file.metadata()?.is_file() {
        return Err(Error::new(
            "INVALID_INPUT",
            "container input must be a regular file",
        ));
    }
    let mut magic = [0; 4];
    file.read_exact(&mut magic)
        .map_err(|_| Error::new("INVALID_INPUT", "truncated container header"))?;
    Ok(magic)
}

pub(super) fn entry(
    input: &Path,
    at: &str,
    output: Option<&Path>,
    policy: BudgetPolicy,
    perf: &mut Perf,
) -> Result<Entry> {
    let container_budget = policy.for_file(input)?;
    let (bytes, _path, format, coordinate) = match magic(input)? {
        [4, 0, 0, 0] => {
            #[cfg(feature = "backend-asar")]
            {
                let mut limits = Limits::from(container_budget);
                if matches!(policy, BudgetPolicy::InputScaled) {
                    limits.input_bytes = rxd_core::MAX_SPAN_BYTES;
                }
                let bytes = rxd_asar::read_entry(
                    at,
                    rxd_asar::Options {
                        input,
                        output,
                        limits,
                        max_entries: container_budget.entries,
                        max_header_bytes: container_budget.metadata_bytes,
                    },
                    perf,
                )?;
                (bytes, at.to_owned(), "asar", at.to_owned())
            }
            #[cfg(not(feature = "backend-asar"))]
            return Err(Error::new(
                "UNSUPPORTED_INPUT",
                "ASAR support is not enabled",
            ));
        }
        [b'P', b'K', 3, 4] | [b'P', b'K', 5, 6] | [b'P', b'K', 7, 8] => {
            #[cfg(feature = "backend-zip")]
            {
                let selected = rxd_zip::read_entry(
                    at,
                    rxd_zip::Options {
                        input,
                        output,
                        budget: container_budget,
                    },
                    perf,
                )?;
                (selected.bytes, selected.path, "zip", selected.at)
            }
            #[cfg(not(feature = "backend-zip"))]
            return Err(Error::new(
                "UNSUPPORTED_INPUT",
                "ZIP support is not enabled",
            ));
        }
        _ => {
            return Err(Error::new(
                "INVALID_INPUT",
                "expected an ASAR or ZIP container",
            ));
        }
    };
    let budget = match policy {
        BudgetPolicy::InputScaled => Budget::for_input_bytes(bytes.len())?,
        BudgetPolicy::Fixed(budget) => budget,
    };
    Ok(Entry {
        bytes,
        #[cfg(any(feature = "backend-js", feature = "backend-html"))]
        path: _path,
        budget,
        origin: Origin {
            container: input.to_string_lossy().into_owned(),
            format,
            entry: coordinate,
        },
    })
}
