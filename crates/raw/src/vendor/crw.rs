//! Canon CRW (CIFF) container parse. Clean-room: derived from Canon's public CIFF spec and measurements on the
//! eleven CC0 CRW samples on raw.pixls.us (see docs/crw-provenance.md). The entropy-coded sensor payload is NOT
//! decoded here: its Huffman tables are not in the files or in ITU T.81 and could not be derived from CC0 inputs.
//!
//! Layout: `II`, u32 heap offset, `HEAPCCDR`; the last 4 bytes of a heap give the directory offset (relative to the
//! heap start); a directory is a u16 count then 10-byte entries (u16 tag, u32 length, u32 offset). Tag bits 14..15
//! `0x4000` mean the value is stored inline in the entry; bits 11..13 `0x2800/0x3000` mean the value is a sub-directory.
use crate::{RawError, Result};

/// Maximum directories/entries visited by one parse (a self-referencing directory cannot loop forever).
pub const VISIT_BUDGET: usize = 4096;
const MAX_DEPTH: usize = 8;

/// One directory entry: tag, absolute payload offset and length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub tag: u16,
    pub offset: usize,
    pub len: usize,
    pub inline: bool,
}

/// Parsed container: every entry found, and the image size from tag 0x1031 when present.
#[derive(Debug, Default)]
pub struct Ciff {
    pub entries: Vec<Entry>,
    pub size: Option<(u32, u32)>,
}

fn u16le(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o.checked_add(2)?)?.try_into().ok()?))
}
fn u32le(b: &[u8], o: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(o..o.checked_add(4)?)?.try_into().ok()?) as usize)
}

fn walk(b: &[u8], start: usize, end: usize, depth: usize, budget: &mut usize, out: &mut Vec<Entry>) -> Option<()> {
    if depth > MAX_DEPTH || end > b.len() || end < start + 4 {
        return None;
    }
    *budget = budget.checked_sub(1)?;
    let dir = start.checked_add(u32le(b, end - 4)?)?;
    let n = u16le(b, dir)? as usize;
    let mut p = dir + 2;
    for _ in 0..n {
        *budget = budget.checked_sub(1)?;
        let tag = u16le(b, p)?;
        let (len, off) = (u32le(b, p + 2)?, u32le(b, p + 6)?);
        p += 10;
        if tag & 0xc000 == 0x4000 {
            out.push(Entry { tag: tag & 0x3fff, offset: p - 8, len: 8, inline: true });
        } else {
            let abs = start.checked_add(off)?;
            if abs.checked_add(len)? > end {
                continue;
            }
            out.push(Entry { tag, offset: abs, len, inline: false });
            if matches!(tag & 0x3800, 0x2800 | 0x3000) && len >= 4 {
                walk(b, abs, abs + len, depth + 1, budget, out)?;
            }
        }
    }
    Some(())
}

/// Parse the container. Never panics; a malformed or looping directory is an error, not a hang.
pub fn parse(bytes: &[u8]) -> Result<Ciff> {
    let bad = || RawError::Unsupported("not a CIFF file".into());
    if bytes.len() < 14 || &bytes[0..2] != b"II" || &bytes[6..14] != b"HEAPCCDR" {
        return Err(bad());
    }
    let heap = u32le(bytes, 2).ok_or_else(bad)?;
    let mut entries = Vec::new();
    let mut budget = VISIT_BUDGET;
    walk(bytes, heap, bytes.len(), 0, &mut budget, &mut entries).ok_or_else(bad)?;
    let size = entries.iter().find(|e| e.tag == 0x1031 && !e.inline && e.len >= 8).and_then(|e| {
        let w = u16le(bytes, e.offset + 2)? as u32;
        let h = u16le(bytes, e.offset + 4)? as u32;
        Some((w, h))
    });
    Ok(Ciff { entries, size })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(dir_entries: &[(u16, u32, u32)], heap_len: usize) -> Vec<u8> {
        let mut v = vec![0u8; 14];
        v[..2].copy_from_slice(b"II");
        v[2..6].copy_from_slice(&14u32.to_le_bytes());
        v[6..14].copy_from_slice(b"HEAPCCDR");
        let dir_off = heap_len - 4 - 2 - 10 * dir_entries.len();
        v.resize(14 + heap_len, 0);
        let d = 14 + dir_off;
        v[d..d + 2].copy_from_slice(&(dir_entries.len() as u16).to_le_bytes());
        for (i, (t, l, o)) in dir_entries.iter().enumerate() {
            let p = d + 2 + 10 * i;
            v[p..p + 2].copy_from_slice(&t.to_le_bytes());
            v[p + 2..p + 6].copy_from_slice(&l.to_le_bytes());
            v[p + 6..p + 10].copy_from_slice(&o.to_le_bytes());
        }
        let e = v.len() - 4;
        v[e..].copy_from_slice(&(dir_off as u32).to_le_bytes());
        v
    }

    #[test]
    fn self_referencing_directory_terminates() {
        // A sub-directory entry that points at the whole heap: the walk would recurse forever without a bound.
        let heap = 64;
        let v = file(&[(0x2800, heap as u32, 0)], heap);
        assert!(parse(&v).is_err());
    }

    #[test]
    fn empty_and_truncated_inputs_do_not_panic() {
        assert!(parse(&[]).is_err());
        assert!(parse(b"II").is_err());
        assert!(parse(&[0u8; 100]).is_err());
        let v = file(&[], 32);
        assert!(parse(&v[..10]).is_err());
    }

    #[test]
    fn plain_directory_parses() {
        let v = file(&[(0x1031, 8, 0)], 64);
        let c = parse(&v).unwrap();
        assert_eq!(c.entries.len(), 1);
    }
}
