//! Bounds-checked little-endian reader shared by the format parsers.

use crate::{Error, Result};

pub(crate) struct Reader<'a> {
    pub bytes: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8], pos: usize) -> Self {
        Reader { bytes, pos }
    }

    pub fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&end| end <= self.bytes.len())
            .ok_or(Error::Truncated {
                what,
                offset: self.pos,
            })?;
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    pub fn u8(&mut self, what: &'static str) -> Result<u8> {
        Ok(self.take(1, what)?[0])
    }

    pub fn u16(&mut self, what: &'static str) -> Result<u16> {
        let b = self.take(2, what)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self, what: &'static str) -> Result<u32> {
        let b = self.take(4, what)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn f32(&mut self, what: &'static str) -> Result<f32> {
        Ok(f32::from_bits(self.u32(what)?))
    }

    pub fn seek(&mut self, pos: usize, what: &'static str) -> Result<()> {
        if pos > self.bytes.len() {
            return Err(Error::Truncated { what, offset: pos });
        }
        self.pos = pos;
        Ok(())
    }
}
