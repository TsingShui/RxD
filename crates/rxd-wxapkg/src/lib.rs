use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use aes::Aes256;
use cbc::{
    Decryptor,
    cipher::{BlockDecryptMut, KeyIvInit, block_padding::NoPadding},
};
use pbkdf2::pbkdf2_hmac;
use rxd_core::{Error, Limits, Result, perf::Perf};
use sha1::Sha1;

const HEADER_BYTES: usize = 18;
const PREFIX_BYTES: usize = 6;
const ENCRYPTED_BLOCK_BYTES: usize = 1024;
const SALT: &[u8] = b"saltiest";
const IV: &[u8; 16] = b"the iv: 16 bytes";
const MAX_NAME_BYTES: usize = 4096;

#[derive(Debug)]
pub struct Summary {
    pub files: usize,
    pub bytes: u64,
    pub encrypted: bool,
    pub appid: Option<String>,
}

struct Header {
    index_length: usize,
    count: usize,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new("INVALID_INPUT", message)
}

fn unsupported(message: impl Into<String>) -> Error {
    Error::new("UNSUPPORTED_INPUT", message)
}

fn resource(message: impl Into<String>) -> Error {
    Error::new("RESOURCE_LIMIT", message)
}

fn be32(bytes: &[u8], at: usize) -> usize {
    u32::from_be_bytes(bytes[at..at + 4].try_into().expect("four bytes")) as usize
}

fn has_header(bytes: &[u8]) -> bool {
    bytes.len() >= HEADER_BYTES && bytes[0] == 0xbe && bytes[13] == 0xed
}

fn parse_header(bytes: &[u8]) -> Option<Header> {
    if !has_header(bytes) {
        return None;
    }
    let index_length = be32(bytes, 5);
    let body_length = be32(bytes, 9);
    let count = be32(bytes, 14);
    let index_end = HEADER_BYTES.checked_add(index_length)?;
    index_end.checked_add(body_length)?;
    if index_end > bytes.len() {
        return None;
    }
    Some(Header {
        index_length,
        count,
    })
}

/// Cheap header check for content-based format selection.
pub fn detect(path: &Path) -> Result<bool> {
    let mut file = File::open(path)?;
    let mut bytes = [0u8; HEADER_BYTES];
    match file.read_exact(&mut bytes) {
        Ok(()) => Ok(has_header(&bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// Find a WeChat appid in a path component, used by encrypted PC/Mac packages.
pub fn appid_from_path(path: &Path) -> Option<String> {
    for part in path.components() {
        let text = part.as_os_str().to_str()?;
        if text.len() == 18
            && text.starts_with("wx")
            && text[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Some(text.to_owned());
        }
    }
    None
}

fn decrypt(bytes: &[u8], appid: &str) -> Result<Vec<u8>> {
    if bytes.len() < PREFIX_BYTES + ENCRYPTED_BLOCK_BYTES {
        return Err(invalid("encrypted wxapkg is too short"));
    }
    if !bytes.starts_with(b"V1MMWX") {
        return Err(unsupported("encrypted wxapkg has an unknown prefix"));
    }
    let mut key = [0u8; 32];
    pbkdf2_hmac::<Sha1>(appid.as_bytes(), SALT, 1000, &mut key);
    let mut block = bytes[PREFIX_BYTES..PREFIX_BYTES + ENCRYPTED_BLOCK_BYTES].to_vec();
    let cipher = Decryptor::<Aes256>::new((&key).into(), IV.into());
    let plain = cipher
        .decrypt_padded_mut::<NoPadding>(&mut block)
        .map_err(|_| unsupported("encrypted wxapkg block is invalid"))?;
    let xor_key = appid
        .chars()
        .rev()
        .nth(1)
        .ok_or_else(|| invalid("appid is too short"))? as u8;
    let mut output = Vec::with_capacity(bytes.len());
    output.extend_from_slice(&plain[..ENCRYPTED_BLOCK_BYTES - 1]);
    output.extend(
        bytes[PREFIX_BYTES + ENCRYPTED_BLOCK_BYTES..]
            .iter()
            .map(|byte| byte ^ xor_key),
    );
    Ok(output)
}

fn safe_relative(name: &str) -> Result<PathBuf> {
    let name = name.strip_prefix('/').unwrap_or(name);
    if name.is_empty() || name.len() > MAX_NAME_BYTES || name.contains(['\0', '\\', ':']) {
        return Err(invalid(format!("unsafe wxapkg entry name: {name}")));
    }
    let mut path = PathBuf::new();
    for component in name.split('/') {
        if component.is_empty() || matches!(component, "." | "..") {
            return Err(invalid(format!("unsafe wxapkg entry name: {name}")));
        }
        path.push(component);
    }
    Ok(path)
}

fn write_new(root: &Path, relative: &Path, bytes: &[u8]) -> Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

pub fn unpack(
    input: &Path,
    destination: &Path,
    limits: Limits,
    max_entries: usize,
    appid: Option<&str>,
    perf: &mut Perf,
) -> Result<Summary> {
    if max_entries == 0 {
        return Err(resource("wxapkg entry budget is zero"));
    }
    let length = fs::metadata(input)?.len();
    if length > limits.input_bytes as u64 {
        return Err(resource("wxapkg input exceeds the input byte budget"));
    }
    let length = usize::try_from(length)
        .map_err(|_| resource("wxapkg input exceeds the addressable byte range"))?;
    let mut file = File::open(input)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| resource("wxapkg allocation failed"))?;
    file.read_to_end(&mut bytes)?;
    perf.counters.input_bytes = Some(bytes.len());
    let (bytes, encrypted, resolved_appid) = match parse_header(&bytes) {
        Some(_) => (bytes, false, None),
        None => {
            let appid = appid
                .map(ToOwned::to_owned)
                .or_else(|| appid_from_path(input))
                .ok_or_else(|| unsupported("encrypted wxapkg requires an appid"))?;
            let plain = perf.measure("parse", || decrypt(&bytes, &appid))?;
            if parse_header(&plain).is_none() {
                return Err(unsupported(
                    "wxapkg decryption did not produce a valid header",
                ));
            }
            (plain, true, Some(appid))
        }
    };
    let header = parse_header(&bytes).ok_or_else(|| invalid("invalid wxapkg header"))?;
    let index_end = HEADER_BYTES + header.index_length;
    if header.count > max_entries {
        return Err(resource("wxapkg entry count exceeds the entry limit"));
    }
    let mut position = HEADER_BYTES;
    let mut names = HashSet::new();
    let mut records = Vec::with_capacity(header.count);
    for _ in 0..header.count {
        let name_len = read_u32(&bytes, position, index_end)?;
        position += 4;
        let name_end = position
            .checked_add(name_len)
            .ok_or_else(|| invalid("wxapkg entry name length overflow"))?;
        if name_len == 0 || name_len > MAX_NAME_BYTES || name_end > index_end {
            return Err(invalid("invalid wxapkg entry name length"));
        }
        let name = std::str::from_utf8(&bytes[position..name_end])
            .map_err(|_| invalid("wxapkg entry name is not UTF-8"))?;
        position = name_end;
        let offset = read_u32(&bytes, position, index_end)?;
        position += 4;
        let size = read_u32(&bytes, position, index_end)?;
        position += 4;
        let relative = safe_relative(name)?;
        if !names.insert(relative.clone()) {
            return Err(invalid(format!("duplicate wxapkg entry: {name}")));
        }
        records.push((relative, offset, size));
    }
    if position > index_end {
        return Err(invalid("wxapkg index exceeds its declared length"));
    }
    let mut files = 0;
    let mut total = 0u64;
    for (relative, offset, size) in records {
        let end = offset
            .checked_add(size)
            .ok_or_else(|| invalid("wxapkg entry size overflow"))?;
        if offset < position || end > bytes.len() {
            return Err(invalid(format!(
                "wxapkg entry is outside the data area: {}",
                relative.display()
            )));
        }
        let data = &bytes[offset..end];
        perf.measure("write", || write_new(destination, &relative, data))?;
        files += 1;
        total += data.len() as u64;
    }
    perf.counters.output_bytes = usize::try_from(total).ok();
    Ok(Summary {
        files,
        bytes: total,
        encrypted,
        appid: resolved_appid,
    })
}

fn read_u32(bytes: &[u8], position: usize, limit: usize) -> Result<usize> {
    let end = position
        .checked_add(4)
        .ok_or_else(|| invalid("wxapkg index position overflow"))?;
    if end > limit {
        return Err(invalid("truncated wxapkg index"));
    }
    Ok(be32(bytes, position))
}
