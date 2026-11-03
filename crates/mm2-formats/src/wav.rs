//! RIFF WAVE sounds: the game's are plain PCM, 8 or 16 bits, mono or
//! stereo, mostly 22050 Hz.

use crate::reader::Reader;
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct Sound {
    pub rate: u32,
    pub channels: u16,
    /// Samples from -1 to 1, channels interleaved.
    pub samples: Vec<f32>,
}

impl Sound {
    /// Sample frames (one sample per channel).
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }
}

pub fn parse(bytes: &[u8]) -> Result<Sound> {
    let mut r = Reader::new(bytes, 0);
    if r.take(4, "riff")? != b"RIFF" {
        return Err(Error::BadMagic);
    }
    r.u32("riff size")?;
    if r.take(4, "wave")? != b"WAVE" {
        return Err(Error::BadMagic);
    }
    let mut format = None;
    let mut data: Option<&[u8]> = None;
    while r.pos + 8 <= bytes.len() {
        let id = r.take(4, "chunk id")?;
        let size = r.u32("chunk size")? as usize;
        let size = size.min(bytes.len() - r.pos);
        let body = r.take(size, "chunk")?;
        if size % 2 == 1 && r.pos < bytes.len() {
            r.pos += 1;
        }
        match id {
            b"fmt " => {
                let mut f = Reader::new(body, 0);
                let tag = f.u16("wave format")?;
                let channels = f.u16("channels")?;
                let rate = f.u32("rate")?;
                f.u32("byte rate")?;
                f.u16("block align")?;
                let bits = f.u16("bits")?;
                if tag != 1 || !matches!(bits, 8 | 16) || channels == 0 {
                    return Err(Error::Corrupt("unsupported wave (only 8 or 16 bit PCM)"));
                }
                format = Some((channels, rate, bits));
            }
            b"data" => data = Some(body),
            _ => {}
        }
    }
    let (channels, rate, bits) = format.ok_or(Error::Corrupt("wave without fmt"))?;
    let data = data.ok_or(Error::Corrupt("wave without data"))?;
    let samples = if bits == 8 {
        data.iter().map(|&b| (b as f32 - 128.0) / 128.0).collect()
    } else {
        data.chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
            .collect()
    };
    Ok(Sound {
        rate,
        channels,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eight_bit_mono() {
        let mut b = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        b.extend_from_slice(&16u32.to_le_bytes());
        for v in [1u16, 1] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&22050u32.to_le_bytes());
        b.extend_from_slice(&22050u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&8u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&[128, 255, 0, 0]);
        let s = parse(&b).unwrap();
        assert_eq!(s.rate, 22050);
        assert_eq!(s.samples, [0.0, 127.0 / 128.0, -1.0]);
    }
}
