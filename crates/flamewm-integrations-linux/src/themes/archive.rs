//! Bounded in-memory tar reader (ustar, pax `x`/`g`, GNU `L`/`K`). Parses only.

use std::fs::File;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};

use super::import::{ImportError, MAX_ENTRIES, MAX_TOTAL_BYTES};
use super::xz::XzReader;

const BLOCK: usize = 512;

/// Decode a `.tar.xz` file (magic checked by `XzReader`) and parse it with the
/// same bounded tar path as `.tar.gz`. Output is capped by `take`.
pub(crate) fn parse_xz(file: File, cancel: &AtomicBool) -> Result<Vec<RawEntry>, ImportError> {
    let mut archive_bytes = Vec::new();
    XzReader::new(file)
        .take(MAX_TOTAL_BYTES + 1)
        .read_to_end(&mut archive_bytes)
        .map_err(|err| ImportError::Corrupt(format!("xz stream: {err}")))?;
    if archive_bytes.len() as u64 > MAX_TOTAL_BYTES {
        return Err(ImportError::TooLarge);
    }
    parse(&archive_bytes, MAX_ENTRIES, cancel)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RawKind {
    File,
    Dir,
    Symlink(Vec<u8>),
    Hardlink(Vec<u8>),
    Other(u8),
}

#[derive(Debug)]
pub(crate) struct RawEntry {
    pub(crate) path: Vec<u8>,
    pub(crate) kind: RawKind,
    pub(crate) data: Vec<u8>,
}

#[derive(Default)]
struct Pending {
    path: Option<Vec<u8>>,
    link: Option<Vec<u8>>,
    size: Option<u64>,
}

pub(crate) fn parse(
    archive: &[u8],
    max_entries: usize,
    cancel: &AtomicBool,
) -> Result<Vec<RawEntry>, ImportError> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    let mut pax = Pending::default();
    let mut gnu_name: Option<Vec<u8>> = None;
    let mut gnu_link: Option<Vec<u8>> = None;
    while offset < archive.len() {
        if cancel.load(Ordering::Relaxed) {
            return Err(ImportError::Cancelled);
        }
        let header = archive
            .get(offset..offset + BLOCK)
            .ok_or_else(|| ImportError::Corrupt("truncated tar header".into()))?;
        if header.iter().all(|&b| b == 0) {
            break;
        }
        verify_checksum(header)?;
        let typeflag = header[156];
        let declared = parse_octal(&header[124..136])?;
        let meta_record = matches!(typeflag, b'x' | b'g' | b'L' | b'K');
        let this = if meta_record {
            Pending::default()
        } else {
            std::mem::take(&mut pax)
        };
        let size = this.size.unwrap_or(declared);
        if size > MAX_TOTAL_BYTES {
            return Err(ImportError::TooLarge);
        }
        let body = usize::try_from(size).map_err(|_| ImportError::TooLarge)?;
        let data_start = offset + BLOCK;
        let data_end = data_start
            .checked_add(body)
            .ok_or(ImportError::TooLarge)?;
        let data = archive
            .get(data_start..data_end)
            .ok_or_else(|| ImportError::Corrupt("truncated tar entry".into()))?;
        offset = data_end
            .checked_add((BLOCK - body % BLOCK) % BLOCK)
            .ok_or(ImportError::TooLarge)?;
        match typeflag {
            b'x' => {
                pax = parse_pax(data)?;
                continue;
            }
            b'g' => continue,
            b'L' => {
                gnu_name = Some(trim_nul(data).to_vec());
                continue;
            }
            b'K' => {
                gnu_link = Some(trim_nul(data).to_vec());
                continue;
            }
            _ => {}
        }
        if out.len() >= max_entries {
            return Err(ImportError::TooManyEntries);
        }
        let path = this
            .path
            .or_else(|| gnu_name.take())
            .unwrap_or_else(|| ustar_name(header));
        let link = this
            .link
            .or_else(|| gnu_link.take())
            .unwrap_or_else(|| trim_nul(&header[157..257]).to_vec());
        gnu_name = None;
        gnu_link = None;
        let (kind, data) = match typeflag {
            b'0' | 0 | b'7' => (RawKind::File, data.to_vec()),
            b'5' => (RawKind::Dir, Vec::new()),
            b'2' => (RawKind::Symlink(link), Vec::new()),
            b'1' => (RawKind::Hardlink(link), Vec::new()),
            other => (RawKind::Other(other), Vec::new()),
        };
        out.push(RawEntry { path, kind, data });
    }
    Ok(out)
}

fn verify_checksum(header: &[u8]) -> Result<(), ImportError> {
    let stored = parse_octal(&header[148..156])?;
    let sum: u64 = header
        .iter()
        .enumerate()
        .map(|(i, &b)| {
            if (148..156).contains(&i) {
                u64::from(b' ')
            } else {
                u64::from(b)
            }
        })
        .sum();
    if sum != stored {
        return Err(ImportError::Corrupt("tar checksum mismatch".into()));
    }
    Ok(())
}

fn parse_octal(field: &[u8]) -> Result<u64, ImportError> {
    if field.first().is_some_and(|&b| b & 0x80 != 0) {
        return Err(ImportError::Corrupt("unsupported tar numeric encoding".into()));
    }
    let token = field
        .split(|&b| b == 0 || b == b' ')
        .find(|s| !s.is_empty())
        .unwrap_or(&[]);
    let mut value: u64 = 0;
    for &b in token {
        if !(b'0'..=b'7').contains(&b) {
            return Err(ImportError::Corrupt("invalid tar octal field".into()));
        }
        value = value
            .checked_mul(8)
            .and_then(|v| v.checked_add(u64::from(b - b'0')))
            .ok_or_else(|| ImportError::Corrupt("tar octal overflow".into()))?;
    }
    Ok(value)
}

fn parse_pax(data: &[u8]) -> Result<Pending, ImportError> {
    let corrupt = || ImportError::Corrupt("malformed pax record".into());
    let mut pending = Pending::default();
    let mut rest = data;
    while !rest.is_empty() {
        let space = rest.iter().position(|&b| b == b' ').ok_or_else(corrupt)?;
        let len: usize = std::str::from_utf8(&rest[..space])
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(corrupt)?;
        if len < space + 2 || len > rest.len() || rest[len - 1] != b'\n' {
            return Err(corrupt());
        }
        let body = &rest[space + 1..len - 1];
        let eq = body.iter().position(|&b| b == b'=').ok_or_else(corrupt)?;
        let (key, value) = (&body[..eq], &body[eq + 1..]);
        match key {
            b"path" => pending.path = Some(value.to_vec()),
            b"linkpath" => pending.link = Some(value.to_vec()),
            b"size" => {
                pending.size = Some(
                    std::str::from_utf8(value)
                        .ok()
                        .and_then(|s| s.parse::<u64>().ok())
                        .ok_or_else(corrupt)?,
                );
            }
            _ => {}
        }
        rest = &rest[len..];
    }
    Ok(pending)
}

fn ustar_name(header: &[u8]) -> Vec<u8> {
    let name = trim_nul(&header[0..100]);
    if &header[257..263] == b"ustar\0" {
        let prefix = trim_nul(&header[345..500]);
        if !prefix.is_empty() {
            let mut full = prefix.to_vec();
            full.push(b'/');
            full.extend_from_slice(name);
            return full;
        }
    }
    name.to_vec()
}

fn trim_nul(field: &[u8]) -> &[u8] {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    &field[..end]
}
