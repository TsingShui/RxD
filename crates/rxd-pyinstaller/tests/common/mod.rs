#![allow(dead_code)]

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use flate2::{Compression, write::ZlibEncoder};

pub const MAGIC: [u8; 8] = [0x4d, 0x45, 0x49, 0x0c, 0x0b, 0x0a, 0x0b, 0x0e];
pub const PYZ_MAGIC: [u8; 4] = [0x61, 0x0d, 0x0d, 0x0a];

pub fn deflate(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

pub fn write(directory: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = directory.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

pub struct Entry {
    pub name: String,
    pub kind: u8,
    pub flag: u8,
    pub data: Vec<u8>,
    pub stored: Option<u32>,
    pub uncompressed: Option<u32>,
    pub offset: Option<u32>,
}

impl Entry {
    pub fn compressed(name: &str, kind: u8, payload: &[u8]) -> Self {
        Self {
            name: name.to_owned(),
            kind,
            flag: 1,
            data: deflate(payload),
            stored: None,
            uncompressed: Some(payload.len() as u32),
            offset: None,
        }
    }

    pub fn stored(name: &str, kind: u8, payload: &[u8]) -> Self {
        Self {
            name: name.to_owned(),
            kind,
            flag: 0,
            data: payload.to_vec(),
            stored: None,
            uncompressed: None,
            offset: None,
        }
    }
}

pub struct Archive {
    pub python: u32,
    pub entries: Vec<Entry>,
    pub trailing: Vec<u8>,
}

impl Archive {
    pub fn new(entries: Vec<Entry>) -> Self {
        Self {
            python: 309,
            entries,
            trailing: Vec::new(),
        }
    }

    pub fn python(mut self, version: u32) -> Self {
        self.python = version;
        self
    }

    pub fn trailing(mut self, bytes: &[u8]) -> Self {
        self.trailing = bytes.to_vec();
        self
    }

    pub fn build(&self) -> Vec<u8> {
        let mut overlay = Vec::new();
        let mut toc = Vec::new();
        for entry in &self.entries {
            let offset = entry.offset.unwrap_or(overlay.len() as u32);
            if entry.offset.is_none() {
                overlay.extend_from_slice(&entry.data);
            }
            let stored = entry.stored.unwrap_or(entry.data.len() as u32);
            let uncompressed = entry.uncompressed.unwrap_or(entry.data.len() as u32);
            toc.extend_from_slice(&(18 + entry.name.len() as u32).to_be_bytes());
            toc.extend_from_slice(&offset.to_be_bytes());
            toc.extend_from_slice(&stored.to_be_bytes());
            toc.extend_from_slice(&uncompressed.to_be_bytes());
            toc.push(entry.flag);
            toc.push(entry.kind);
            toc.extend_from_slice(entry.name.as_bytes());
        }
        let mut bytes = overlay;
        let toc_offset = bytes.len() as u32;
        bytes.extend_from_slice(&toc);
        let package_length = bytes.len() as u32 + 88;
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&package_length.to_be_bytes());
        bytes.extend_from_slice(&toc_offset.to_be_bytes());
        bytes.extend_from_slice(&(toc.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&self.python.to_be_bytes());
        bytes.extend_from_slice(&[0u8; 64]);
        bytes.extend_from_slice(&self.trailing);
        bytes
    }
}

pub struct PyzBuilder {
    pub magic: [u8; 4],
    pub modules: Vec<(String, bool, Option<Vec<u8>>)>,
    pub dictionary: bool,
}

impl PyzBuilder {
    pub fn new(magic: [u8; 4]) -> Self {
        Self {
            magic,
            modules: Vec::new(),
            dictionary: false,
        }
    }

    pub fn module(mut self, name: &str, package: bool, payload: &[u8]) -> Self {
        self.modules
            .push((name.to_owned(), package, Some(payload.to_vec())));
        self
    }

    pub fn empty_module(mut self, name: &str, package: bool) -> Self {
        self.modules.push((name.to_owned(), package, None));
        self
    }

    pub fn dictionary(mut self) -> Self {
        self.dictionary = true;
        self
    }

    pub fn build(&self) -> Vec<u8> {
        let mut body = vec![0u8; 12];
        let mut rows = Vec::new();
        for (name, package, payload) in &self.modules {
            let offset = body.len() as u32;
            let data = payload.as_deref().map(deflate).unwrap_or_default();
            body.extend_from_slice(&data);
            rows.push((name, *package, offset, data.len() as u32));
        }
        let toc_position = body.len() as u32;
        let mut toc = Vec::new();
        if self.dictionary {
            toc.push(b'{');
            for (name, package, offset, length) in &rows {
                write_text(&mut toc, name);
                write_info(&mut toc, *package, *offset, *length);
            }
            toc.push(b'0');
        } else {
            toc.push(b'[');
            toc.extend_from_slice(&(rows.len() as u32).to_le_bytes());
            for (name, package, offset, length) in &rows {
                toc.push(b'(');
                toc.extend_from_slice(&2u32.to_le_bytes());
                write_text(&mut toc, name);
                write_info(&mut toc, *package, *offset, *length);
            }
        }
        body.extend_from_slice(&toc);
        body[..4].copy_from_slice(b"PYZ\0");
        body[4..8].copy_from_slice(&self.magic);
        body[8..12].copy_from_slice(&toc_position.to_be_bytes());
        body
    }
}

fn write_text(toc: &mut Vec<u8>, name: &str) {
    toc.push(b'u');
    toc.extend_from_slice(&(name.len() as u32).to_le_bytes());
    toc.extend_from_slice(name.as_bytes());
}

fn write_info(toc: &mut Vec<u8>, package: bool, offset: u32, length: u32) {
    toc.push(b'(');
    toc.extend_from_slice(&3u32.to_le_bytes());
    for value in [u32::from(package), offset, length] {
        toc.push(b'i');
        toc.extend_from_slice(&(value as i32).to_le_bytes());
    }
}

pub fn pyc_header(magic: [u8; 4], header: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; header];
    bytes[..4].copy_from_slice(&magic);
    bytes
}
