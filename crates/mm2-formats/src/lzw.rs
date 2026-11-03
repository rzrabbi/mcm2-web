//! The game's LZW decompressor (`Lzw.cpp` in the original).
//!
//! The stream is read MSB-first from big-endian 32-bit words. The first
//! word holds the packed size and is skipped. Codes start 9 bits wide, and
//! three codes are reserved:
//!
//! * `0x100` – end of data
//! * `0x101` – widen codes by one bit
//! * `0x102` – clear the dictionary (the next code is a literal)
//!
//! A stream that ends before `out_len` bytes is padded with zeros.

use crate::{Error, Result};

const END: u32 = 0x100;
const WIDEN: u32 = 0x101;
const CLEAR: u32 = 0x102;
const FIRST_FREE: u32 = 0x103;

struct Bits<'a> {
    src: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn read(&mut self, width: u32) -> Result<u32> {
        let mut v = 0;
        for _ in 0..width {
            let byte = *self.src.get(self.pos >> 3).ok_or(Error::Truncated {
                what: "LZW stream",
                offset: self.pos >> 3,
            })?;
            v = (v << 1) | u32::from((byte >> (7 - (self.pos & 7))) & 1);
            self.pos += 1;
        }
        Ok(v)
    }
}

/// Decompresses `src` into exactly `out_len` bytes.
pub fn decompress(src: &[u8], out_len: usize) -> Result<Vec<u8>> {
    let mut bits = Bits { src, pos: 32 };
    let mut out = Vec::with_capacity(out_len);
    // Each dictionary entry is (prefix code, last byte, first byte).
    let mut dict: Vec<(u32, u8, u8)> = Vec::new();
    let mut scratch = Vec::new();
    'clear: loop {
        dict.clear();
        let mut width = 9;
        let mut prev = bits.read(width)?;
        if prev == END {
            break;
        }
        if prev > 0xff {
            return Err(Error::Corrupt("LZW stream starts with a non-literal"));
        }
        out.push(prev as u8);
        loop {
            let code = bits.read(width)?;
            match code {
                END => break 'clear,
                CLEAR => continue 'clear,
                WIDEN => {
                    width += 1;
                    continue;
                }
                _ => {}
            }
            let next = FIRST_FREE + dict.len() as u32;
            let first = if code < next {
                expand(code, &dict, &mut scratch);
                out.extend(scratch.iter().rev());
                *scratch.last().unwrap()
            } else if code == next {
                // The code being defined right now: prev + first(prev).
                expand(prev, &dict, &mut scratch);
                let first = *scratch.last().unwrap();
                out.extend(scratch.iter().rev());
                out.push(first);
                first
            } else {
                return Err(Error::Corrupt("LZW code out of range"));
            };
            dict.push((prev, first, first_of(prev, &dict)));
            prev = code;
            if out.len() >= out_len {
                break 'clear;
            }
        }
    }
    // The game (0x4a1940) decodes up to the end code and leaves the rest
    // of its buffer as it was. Some track makers' tools end the stream a
    // byte early (Angel Delight II's cells), so the rest is zeros.
    out.resize(out_len, 0);
    Ok(out)
}

/// Writes the bytes of `code` into `buf` in reverse order.
fn expand(mut code: u32, dict: &[(u32, u8, u8)], buf: &mut Vec<u8>) {
    buf.clear();
    while code > 0xff {
        let (prefix, last, _) = dict[(code - FIRST_FREE) as usize];
        buf.push(last);
        code = prefix;
    }
    buf.push(code as u8);
}

fn first_of(code: u32, dict: &[(u32, u8, u8)]) -> u8 {
    if code > 0xff {
        dict[(code - FIRST_FREE) as usize].2
    } else {
        code as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Packs codes MSB-first after a 32-bit header, like the game's writer.
    fn pack(codes: &[(u32, u32)]) -> Vec<u8> {
        let mut bits: Vec<bool> = vec![false; 32];
        for &(code, width) in codes {
            bits.extend((0..width).rev().map(|i| code >> i & 1 == 1));
        }
        bits.resize(bits.len().div_ceil(32) * 32, false);
        bits.chunks(8)
            .map(|b| b.iter().fold(0u8, |acc, &bit| acc << 1 | bit as u8))
            .collect()
    }

    #[test]
    fn decodes_literals_and_back_references() {
        // "abab" + the KwKwK case: a, b, 0x103 ("ab"), 0x105 ("aba").
        let src = pack(&[
            (b'a' as u32, 9),
            (b'b' as u32, 9),
            (0x103, 9),
            (0x105, 9),
            (END, 9),
        ]);
        assert_eq!(decompress(&src, 7).unwrap(), b"abababa");
    }

    #[test]
    fn handles_widen_and_clear() {
        let src = pack(&[
            (b'x' as u32, 9),
            (WIDEN, 9),
            (b'y' as u32, 10),
            (CLEAR, 10),
            (b'z' as u32, 9),
            (END, 9),
        ]);
        assert_eq!(decompress(&src, 3).unwrap(), b"xyz");
    }

    #[test]
    fn pads_short_streams_with_zeros() {
        let src = pack(&[(b'a' as u32, 9), (END, 9)]);
        assert_eq!(decompress(&src, 3).unwrap(), b"a\0\0");
    }
}
