//! The little-endian bit reader and the Huffman codebooks of the Indeo
//! bitstream (FFmpeg's `get_bits.h` with `BITSTREAM_READER_LE`, and
//! `ivi_create_huff_from_desc`).

use crate::Error;

/// The longest code (`IVI_VLC_BITS`).
pub const VLC_BITS: u32 = 13;

/// Reads bits from the lowest bit of each byte up.
pub struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Bits { data, pos: 0 }
    }

    /// The next `n` (at most 25) bits without taking them; past the end
    /// reads zeros.
    pub fn peek(&self, n: u32) -> u32 {
        let byte = self.pos >> 3;
        let mut v: u64 = 0;
        for i in 0..5 {
            if let Some(&b) = self.data.get(byte + i) {
                v |= (b as u64) << (8 * i);
            }
        }
        ((v >> (self.pos & 7)) & ((1u64 << n) - 1)) as u32
    }

    pub fn get(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        if n > 24 {
            let lo = self.get(16);
            return lo | (self.get(n - 16) << 16);
        }
        let v = self.peek(n);
        self.pos += n as usize;
        v
    }

    pub fn get1(&mut self) -> bool {
        self.get(1) != 0
    }

    pub fn skip(&mut self, n: usize) {
        self.pos += n;
    }

    pub fn align(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }

    pub fn count(&self) -> usize {
        self.pos
    }

    pub fn left(&self) -> isize {
        (self.data.len() * 8) as isize - self.pos as isize
    }

    /// A symbol of the codebook (`get_vlc2` with one level).
    pub fn vlc(&mut self, vlc: &Vlc) -> Result<u32, Error> {
        let (sym, len) = vlc.table[self.peek(VLC_BITS) as usize];
        if len == 0 {
            return Err(Error::Invalid("unknown Huffman code"));
        }
        self.pos += len as usize;
        Ok(sym as u32)
    }
}

/// A codebook descriptor: how many codes each row of a unary prefix has
/// (`IVIHuffDesc`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HuffDesc {
    pub xbits: Vec<u8>,
}

/// A decoding table for the next [`VLC_BITS`] bits: the symbol and its
/// length (0 for no code).
#[derive(Clone)]
pub struct Vlc {
    table: Vec<(u16, u8)>,
}

impl Vlc {
    /// The codes of a descriptor: row `i` is `i` ones, a zero (but for the
    /// last row), then `xbits[i]` bits; the symbols count up from 0.
    pub fn from_desc(desc: &HuffDesc) -> Result<Vlc, Error> {
        let rows = desc.xbits.len();
        let mut table = vec![(0u16, 0u8); 1 << VLC_BITS];
        let mut pos = 0usize;
        for (i, &x) in desc.xbits.iter().enumerate() {
            let x = x as u32;
            let not_last = (i != rows - 1) as u32;
            let prefix = ((1u32 << i) - 1) << (x + not_last);
            for j in 0..1u32 << x {
                // Only 256 codes are allowed, though some books have more.
                if pos >= 256 {
                    break;
                }
                let mut bits = i as u32 + x + not_last;
                if bits > VLC_BITS {
                    return Err(Error::Invalid("Huffman code too long"));
                }
                let code = prefix | j;
                if bits == 0 {
                    bits = 1;
                }
                // The code is read first bit first from a stream that
                // fills from the low bit, so its bits go in reversed.
                let mut rev = 0u32;
                for b in 0..bits {
                    rev |= ((code >> (bits - 1 - b)) & 1) << b;
                }
                let step = 1usize << bits;
                let mut idx = rev as usize;
                while idx < table.len() {
                    if table[idx].1 == 0 {
                        table[idx] = (pos as u16, bits as u8);
                    }
                    idx += step;
                }
                pos += 1;
            }
        }
        Ok(Vlc { table })
    }
}

/// The eight built-in macroblock codebooks (`ivi_mb_huff_desc`).
pub const MB_DESCS: [&[u8]; 8] = [
    &[0, 4, 5, 4, 4, 4, 6, 6],
    &[0, 2, 2, 3, 3, 3, 3, 5, 3, 2, 2, 2],
    &[0, 2, 3, 4, 3, 3, 3, 3, 4, 3, 2, 2],
    &[0, 3, 4, 4, 3, 3, 3, 3, 3, 2, 2, 2],
    &[0, 4, 4, 3, 3, 3, 3, 2, 3, 3, 2, 1, 1],
    &[0, 4, 4, 4, 4, 3, 3, 3, 2],
    &[0, 4, 4, 4, 4, 3, 3, 2, 2, 2],
    &[0, 4, 4, 4, 3, 3, 2, 3, 2, 2, 2, 2],
];

/// The eight built-in block codebooks (`ivi_blk_huff_desc`).
pub const BLK_DESCS: [&[u8]; 8] = [
    &[1, 2, 3, 4, 4, 7, 5, 5, 4, 1],
    &[2, 3, 4, 4, 4, 7, 5, 4, 3, 3, 2],
    &[2, 4, 5, 5, 5, 5, 6, 4, 4, 3, 1, 1],
    &[3, 3, 4, 4, 5, 6, 6, 4, 4, 3, 2, 1, 1],
    &[3, 4, 4, 5, 5, 5, 6, 5, 4, 2, 2],
    &[3, 4, 5, 5, 5, 5, 6, 4, 3, 3, 2, 1, 1],
    &[3, 4, 5, 5, 5, 6, 5, 4, 3, 3, 2, 1, 1],
    &[3, 4, 4, 5, 5, 5, 6, 5, 5],
];

/// Which codebook a band or the macroblocks use (`IVIHuffTab`): one of
/// the built-in eight, or one the stream spelled out.
#[derive(Clone, Default)]
pub struct HuffTab {
    custom_desc: HuffDesc,
    custom: Option<Vlc>,
    /// `None` for the custom book.
    pub sel: Option<usize>,
}

impl HuffTab {
    /// `ff_ivi_dec_huff_desc`.
    pub fn read(&mut self, bits: &mut Bits, coded: bool) -> Result<(), Error> {
        if !coded {
            self.sel = Some(7);
            return Ok(());
        }
        let sel = bits.get(3) as usize;
        if sel != 7 {
            self.sel = Some(sel);
            return Ok(());
        }
        let rows = bits.get(4) as usize;
        if rows == 0 {
            return Err(Error::Invalid("empty custom Huffman table"));
        }
        let desc = HuffDesc {
            xbits: (0..rows).map(|_| bits.get(4) as u8).collect(),
        };
        if desc != self.custom_desc || self.custom.is_none() {
            match Vlc::from_desc(&desc) {
                Ok(v) => {
                    self.custom = Some(v);
                    self.custom_desc = desc;
                }
                Err(e) => {
                    self.custom = None;
                    self.custom_desc = HuffDesc::default();
                    return Err(e);
                }
            }
        }
        self.sel = None;
        Ok(())
    }

    pub fn vlc<'a>(&'a self, built_in: &'a [Vlc; 8]) -> &'a Vlc {
        match (self.sel, &self.custom) {
            (Some(s), _) => &built_in[s],
            (None, Some(c)) => c,
            (None, None) => &built_in[7],
        }
    }
}
