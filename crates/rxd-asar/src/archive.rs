//! Read the Chromium Pickle header and only the selected ASAR byte range.
use std::{
    collections::BTreeMap,
    fmt,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use rxd_core::{Error, Result};
use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};

use crate::{
    Entry, Storage,
    local::{self, invalid},
};

#[derive(Deserialize)]
struct Node {
    files: Option<Files>,
    size: Option<u64>,
    offset: Option<String>,
    link: Option<String>,
    unpacked: Option<bool>,
}

// A normal map deserializer silently keeps the last duplicate path. Reject ambiguity.
struct Files(BTreeMap<String, Node>);
impl<'de> Deserialize<'de> for Files {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct Unique;
        impl<'de> Visitor<'de> for Unique {
            type Value = Files;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a unique ASAR entry map")
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Files, A::Error> {
                let mut files = BTreeMap::new();
                while let Some((name, node)) = map.next_entry::<String, Node>()? {
                    if files.insert(name, node).is_some() {
                        return Err(serde::de::Error::custom("duplicate ASAR entry"));
                    }
                }
                Ok(Files(files))
            }
        }
        deserializer.deserialize_map(Unique)
    }
}

pub struct Archive {
    file: File,
    data_start: u64,
    archive_size: u64,
    unpacked: PathBuf,
}

fn word(bytes: &[u8], start: usize) -> u32 {
    u32::from_le_bytes(
        bytes[start..start + 4]
            .try_into()
            .expect("checked header range"),
    )
}

impl Archive {
    pub fn open(
        path: &Path,
        max_header_bytes: usize,
        max_files: usize,
    ) -> Result<(Self, BTreeMap<String, Entry>)> {
        let mut file = local::open(path)?;
        let archive_size = file.metadata()?.len();
        let mut prefix = [0; 8];
        file.read_exact(&mut prefix)
            .map_err(|_| invalid("truncated ASAR size pickle"))?;
        let header_size = word(&prefix, 4) as usize;
        if word(&prefix, 0) != 4 || header_size < 8 || !header_size.is_multiple_of(4) {
            return Err(invalid("invalid ASAR size pickle"));
        }
        if header_size > max_header_bytes {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "ASAR header byte budget exceeded",
            ));
        }
        let data_start = 8 + header_size as u64;
        if data_start > archive_size {
            return Err(invalid("ASAR header extends beyond archive"));
        }
        let mut header = vec![0; header_size];
        file.read_exact(&mut header)
            .map_err(|_| invalid("truncated ASAR header"))?;
        let payload_size = word(&header, 0) as usize;
        let json_size = word(&header, 4) as usize;
        if payload_size != header_size - 4
            || json_size > i32::MAX as usize
            || json_size.checked_add(8).is_none_or(|end| end > header_size)
            || (json_size + 4).next_multiple_of(4) != payload_size
        {
            return Err(invalid("invalid ASAR header pickle"));
        }
        let root: Node = serde_json::from_slice(&header[8..8 + json_size])
            .map_err(|e| invalid(format!("invalid ASAR JSON header: {e}")))?;
        if root.files.is_none()
            || root.size.is_some()
            || root.offset.is_some()
            || root.link.is_some()
        {
            return Err(invalid("ASAR root must be a directory"));
        }
        let mut index = Index {
            max_entries: max_files,
            nodes: 0,
            entries: BTreeMap::new(),
        };
        index.walk(root, "", false)?;
        let mut unpacked = path.as_os_str().to_owned();
        unpacked.push(".unpacked");
        Ok((
            Self {
                file,
                data_start,
                archive_size,
                unpacked: PathBuf::from(unpacked),
            },
            index.entries,
        ))
    }

    pub fn read(&mut self, path: &str, entry: &Entry, max_bytes: usize) -> Result<Vec<u8>> {
        // Validate only the selected entry, against the same open file handle.
        // This precedes both allocation and seeking, including zero-length entries.
        let packed_start = if let Storage::Packed { offset } = entry.storage {
            Some(
                self.packed_range(offset, entry.size, self.file.metadata()?.len())
                    .map_err(RangeError::diagnostic)?,
            )
        } else {
            None
        };
        if entry.size > max_bytes as u64 {
            return Err(Error::new("RESOURCE_LIMIT", "input byte budget exceeded"));
        }
        match entry.storage {
            Storage::Unpacked => {
                let bytes = local::read(local::open_entry(&self.unpacked, path)?, max_bytes)?;
                if bytes.len() as u64 != entry.size {
                    return Err(invalid("unpacked file size differs from ASAR header"));
                }
                Ok(bytes)
            }
            Storage::Packed { .. } => {
                self.file
                    .seek(SeekFrom::Start(packed_start.expect("checked packed entry")))?;
                let mut bytes = vec![0; entry.size as usize];
                self.file
                    .read_exact(&mut bytes)
                    .map_err(|_| invalid("truncated ASAR entry"))?;
                Ok(bytes)
            }
            Storage::Link { .. } => Err(invalid("ASAR links are listed but never followed")),
        }
    }

    /// Metadata-only range status; does not read entry bytes or unpacked files.
    pub fn range_error(&self, entry: &Entry) -> Option<RangeError> {
        if let Storage::Packed { offset } = entry.storage {
            self.packed_range(offset, entry.size, self.archive_size)
                .err()
        } else {
            None
        }
    }

    fn packed_range(
        &self,
        offset: u64,
        size: u64,
        archive_size: u64,
    ) -> std::result::Result<u64, RangeError> {
        let start = self
            .data_start
            .checked_add(offset)
            .ok_or(RangeError::Overflow)?;
        let end = start.checked_add(size).ok_or(RangeError::Overflow)?;
        if end > archive_size {
            return Err(RangeError::OutOfBounds);
        }
        Ok(start)
    }
}

#[derive(Clone, Copy)]
pub enum RangeError {
    OutOfBounds,
    Overflow,
}

impl RangeError {
    pub fn code(self) -> &'static str {
        match self {
            Self::OutOfBounds => "out_of_bounds",
            Self::Overflow => "overflow",
        }
    }
    fn diagnostic(self) -> Error {
        invalid(match self {
            Self::OutOfBounds => "ASAR entry extends beyond archive",
            Self::Overflow => "ASAR entry range overflows",
        })
    }
}

struct Index {
    max_entries: usize,
    nodes: usize,
    entries: BTreeMap<String, Entry>,
}

impl Index {
    fn walk(&mut self, node: Node, prefix: &str, inherited_unpack: bool) -> Result<()> {
        let unpacked = inherited_unpack || node.unpacked.unwrap_or(false);
        if let Some(files) = node.files {
            if node.size.is_some() || node.offset.is_some() || node.link.is_some() {
                return Err(invalid("ambiguous ASAR directory entry"));
            }
            for (name, child) in files.0 {
                if name.contains('/') {
                    return Err(invalid("ASAR entry name contains a separator"));
                }
                local::validate_entry(&name)?;
                self.nodes += 1;
                if self.nodes > self.max_entries {
                    return Err(Error::new(
                        "RESOURCE_LIMIT",
                        "container entry budget exceeded",
                    ));
                }
                let path = if prefix.is_empty() {
                    name
                } else {
                    format!("{prefix}/{name}")
                };
                local::validate_entry(&path)?;
                self.walk(child, &path, unpacked)?;
            }
            return Ok(());
        }
        let entry = if let Some(link) = node.link {
            if node.size.is_some() || node.offset.is_some() {
                return Err(invalid("ambiguous ASAR link entry"));
            }
            local::validate_entry(&link)?;
            Entry {
                size: 0,
                storage: Storage::Link { target: link },
            }
        } else {
            let size = node
                .size
                .ok_or_else(|| invalid("ASAR file is missing size"))?;
            let storage = if unpacked {
                Storage::Unpacked
            } else {
                let offset = node
                    .offset
                    .ok_or_else(|| invalid("packed ASAR file is missing offset"))?;
                if offset.is_empty() || !offset.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(invalid("ASAR offset must be a decimal string"));
                }
                let offset = offset
                    .parse::<u64>()
                    .map_err(|_| invalid("ASAR offset overflows"))?;
                Storage::Packed { offset }
            };
            Entry { size, storage }
        };
        self.entries.insert(prefix.to_owned(), entry);
        Ok(())
    }
}
