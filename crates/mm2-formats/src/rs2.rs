//! `RS2` archives.
//!
//! Used by `ENGINE.RES`, the `.TEX` texture packs and the `UI/*.DAT` screens.
//! All integers are little-endian.
//!
//! ```text
//! "RS2\0"
//! u32 entry_count
//! entry_count times:
//!     u32 name_len          // includes the trailing NUL
//!     u8  name[name_len]
//!     u32 offset            // from the start of the file
//!     u32 size
//! file data
//! ```

use crate::reader::Reader;
use crate::{Error, Result};

pub const MAGIC: &[u8] = b"RS2\0";

/// One named file inside an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub offset: u32,
    pub size: u32,
}

/// A parsed archive that borrows the file's bytes.
#[derive(Debug, Clone)]
pub struct Archive<'a> {
    bytes: &'a [u8],
    entries: Vec<Entry>,
}

impl<'a> Archive<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        if !bytes.starts_with(MAGIC) {
            return Err(Error::BadMagic);
        }
        let mut r = Reader {
            bytes,
            pos: MAGIC.len(),
        };
        let count = r.u32("entry count")?;
        // Each entry takes at least 13 bytes, so a bogus count fails fast
        // instead of reserving a huge Vec.
        let mut entries = Vec::with_capacity((count as usize).min(bytes.len() / 13));
        for _ in 0..count {
            let name_len = r.u32("name length")? as usize;
            let name_at = r.pos;
            let raw = r.take(name_len, "name")?;
            let raw = raw.strip_suffix(&[0]).unwrap_or(raw);
            // Names are plain ASCII in the shipped data; Latin-1 keeps any
            // stray high bytes readable rather than failing.
            if raw.contains(&0) {
                return Err(Error::BadName { offset: name_at });
            }
            let name: String = raw.iter().map(|&b| b as char).collect();
            let offset = r.u32("entry offset")?;
            let size = r.u32("entry size")?;
            let end = offset as u64 + size as u64;
            if end > bytes.len() as u64 {
                return Err(Error::EntryOutOfBounds { name, offset, size });
            }
            entries.push(Entry { name, offset, size });
        }
        Ok(Archive { bytes, entries })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn data(&self, entry: &Entry) -> &'a [u8] {
        let start = entry.offset as usize;
        &self.bytes[start..start + entry.size as usize]
    }

    /// Finds an entry by name, ignoring ASCII case like the game's file APIs.
    pub fn get(&self, name: &str) -> Option<&'a [u8]> {
        self.entries
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(name))
            .map(|e| self.data(e))
    }
}

/// Builds an archive in memory. Used by tests; handy for tools later.
pub fn build(files: &[(&str, &[u8])]) -> Vec<u8> {
    let table_len: usize = files.iter().map(|(n, _)| 4 + n.len() + 1 + 8).sum();
    let mut offset = MAGIC.len() + 4 + table_len;
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for (name, data) in files {
        out.extend_from_slice(&(name.len() as u32 + 1).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        offset += data.len();
    }
    for (_, data) in files {
        out.extend_from_slice(data);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let bytes = build(&[("a.ini", b"[x]\n"), ("Tex0.tga", &[1, 2, 3])]);
        let ar = Archive::parse(&bytes).unwrap();
        assert_eq!(ar.entries().len(), 2);
        assert_eq!(ar.entries()[1].name, "Tex0.tga");
        assert_eq!(ar.get("A.INI"), Some(&b"[x]\n"[..]));
        assert_eq!(ar.get("tex0.TGA"), Some(&[1u8, 2, 3][..]));
    }

    #[test]
    fn rejects_out_of_bounds_entry() {
        let mut bytes = build(&[("a", b"xyz")]);
        bytes.truncate(bytes.len() - 1);
        assert!(matches!(
            Archive::parse(&bytes),
            Err(Error::EntryOutOfBounds { .. })
        ));
    }

    #[test]
    fn rejects_truncated_table() {
        let bytes = build(&[("abc", b"")]);
        assert!(matches!(
            Archive::parse(&bytes[..10]),
            Err(Error::Truncated { .. })
        ));
    }

    #[test]
    fn rejects_wrong_magic() {
        assert_eq!(Archive::parse(b"FAOE....").unwrap_err(), Error::BadMagic);
    }
}
