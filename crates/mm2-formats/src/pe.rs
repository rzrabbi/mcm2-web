//! String tables from Windows DLL resources: the game keeps its menu text
//! in `UILANG.DLL` and its in-race text in `LANG.DLL`, looked up by number
//! (a dialog's `LocalTextId`).

use crate::reader::Reader;
use crate::{Error, Result};
use std::collections::HashMap;

const RT_STRING: u32 = 6;

/// Every string in the file's string tables, by id.
pub fn strings(bytes: &[u8]) -> Result<HashMap<u32, String>> {
    let mut r = Reader::new(bytes, 0x3c);
    let pe = r.u32("pe offset")? as usize;
    let mut r = Reader::new(bytes, pe);
    if r.take(4, "pe signature")? != b"PE\0\0" {
        return Err(Error::BadMagic);
    }
    r.u16("machine")?;
    let sections = r.u16("section count")? as usize;
    r.take(12, "pe header")?;
    let optional = r.u16("optional header size")? as usize;
    r.u16("characteristics")?;
    let opt_start = r.pos;
    let magic = r.u16("optional magic")?;
    // The resource directory is data directory 2.
    let dirs = opt_start + if magic == 0x20b { 112 } else { 96 };
    let mut d = Reader::new(bytes, dirs + 2 * 8);
    let rsrc_rva = d.u32("resource rva")?;
    let mut s = Reader::new(bytes, opt_start + optional);
    let mut rsrc = None;
    for _ in 0..sections {
        let _name = s.take(8, "section name")?;
        let vsize = s.u32("section size")?;
        let va = s.u32("section rva")?;
        let raw_size = s.u32("section raw size")?;
        let raw = s.u32("section raw offset")?;
        s.take(16, "section rest")?;
        if rsrc_rva >= va && rsrc_rva < va + vsize.max(raw_size) {
            rsrc = Some((va, raw + (rsrc_rva - va)));
        }
    }
    let (va, base) = rsrc.ok_or(Error::Corrupt("no resource section"))?;
    let base = base as usize;
    let to_file = |rva: u32| (rva - va) as usize + (base - (rsrc_rva - va) as usize);

    let mut out = HashMap::new();
    for (ty, ty_dir) in entries(bytes, base, base)? {
        if ty != RT_STRING {
            continue;
        }
        for (block, name_dir) in entries(bytes, base, ty_dir)? {
            for (_lang, data) in entries(bytes, base, name_dir)? {
                let mut e = Reader::new(bytes, data);
                let rva = e.u32("string data rva")?;
                let size = e.u32("string data size")? as usize;
                let mut t = Reader::new(bytes, to_file(rva));
                let end = t.pos + size;
                for i in 0..16 {
                    if t.pos >= end {
                        break;
                    }
                    let len = t.u16("string length")? as usize;
                    let units: Vec<u16> = t
                        .take(len * 2, "string")?
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect();
                    if len > 0 {
                        let id = (block - 1) * 16 + i;
                        out.entry(id)
                            .or_insert_with(|| String::from_utf16_lossy(&units));
                    }
                }
            }
        }
    }
    Ok(out)
}

/// The entries of the resource directory at `dir`: each id and where its
/// subdirectory or data entry is.
fn entries(bytes: &[u8], base: usize, dir: usize) -> Result<Vec<(u32, usize)>> {
    let mut r = Reader::new(bytes, dir + 12);
    let named = r.u16("named entries")? as usize;
    let ids = r.u16("id entries")? as usize;
    let mut out = Vec::new();
    for _ in 0..named + ids {
        let id = r.u32("resource id")?;
        let off = r.u32("resource offset")?;
        out.push((id & 0x7fff_ffff, base + (off & 0x7fff_ffff) as usize));
    }
    Ok(out)
}
