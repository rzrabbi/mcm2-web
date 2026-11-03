//! Targa images, as the front end's art and the fonts use them: true
//! colour (type 2) or run-length encoded (type 10), 16, 24 or 32 bits a
//! pixel, rows stored bottom-up unless the descriptor says otherwise.

use crate::reader::Reader;
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// RGBA, 8 bits a channel, top row first.
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        self.rgba[i..i + 4].try_into().unwrap()
    }
}

pub fn parse(bytes: &[u8]) -> Result<Image> {
    let mut r = Reader::new(bytes, 0);
    let id_len = r.u8("tga id length")?;
    let has_palette = r.u8("tga palette type")?;
    let kind = r.u8("tga image type")?;
    let _palette = r.take(5, "tga palette spec")?;
    let _origin = r.take(4, "tga origin")?;
    let width = r.u16("tga width")? as u32;
    let height = r.u16("tga height")? as u32;
    let bpp = r.u8("tga depth")?;
    let descriptor = r.u8("tga descriptor")?;
    r.take(id_len as usize, "tga id")?;
    if has_palette != 0 || !matches!(kind, 2 | 10) || !matches!(bpp, 16 | 24 | 32) {
        return Err(Error::Corrupt("unsupported tga (only true colour)"));
    }
    let size = bpp as usize / 8;
    let count = (width * height) as usize;
    let mut pixels = Vec::with_capacity(count);
    let pixel = |r: &mut Reader| -> Result<[u8; 4]> {
        let p = r.take(size, "tga pixel")?;
        Ok(match size {
            2 => {
                let v = u16::from_le_bytes([p[0], p[1]]);
                let c = |s: u16| (((v >> s) & 31) * 255 / 31) as u8;
                // The attribute bit is unused (0) in the game's art, so 16
                // bit pixels are opaque.
                [c(10), c(5), c(0), 255]
            }
            3 => [p[2], p[1], p[0], 255],
            _ => [p[2], p[1], p[0], p[3]],
        })
    };
    if kind == 2 {
        for _ in 0..count {
            pixels.push(pixel(&mut r)?);
        }
    } else {
        while pixels.len() < count {
            let head = r.u8("tga packet")?;
            let run = (head & 0x7f) as usize + 1;
            if head & 0x80 != 0 {
                let p = pixel(&mut r)?;
                pixels.extend(std::iter::repeat_n(p, run));
            } else {
                for _ in 0..run {
                    pixels.push(pixel(&mut r)?);
                }
            }
        }
        pixels.truncate(count);
    }
    let top_down = descriptor & 0x20 != 0;
    let mut rgba = Vec::with_capacity(count * 4);
    for y in 0..height {
        let row = if top_down { y } else { height - 1 - y };
        let start = (row * width) as usize;
        for p in &pixels[start..start + width as usize] {
            rgba.extend_from_slice(p);
        }
    }
    Ok(Image {
        width,
        height,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(kind: u8, w: u16, h: u16, bpp: u8, desc: u8) -> Vec<u8> {
        let mut b = vec![0, 0, kind, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        b.extend_from_slice(&w.to_le_bytes());
        b.extend_from_slice(&h.to_le_bytes());
        b.extend_from_slice(&[bpp, desc]);
        b
    }

    #[test]
    fn bottom_up_16_bit() {
        let mut b = header(2, 2, 2, 16, 0);
        // Bottom row: red, green. Top row: blue, white.
        for v in [0x7c00u16, 0x03e0, 0x001f, 0x7fff] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let img = parse(&b).unwrap();
        assert_eq!(img.pixel(0, 0), [0, 0, 255, 255]);
        assert_eq!(img.pixel(1, 0), [255, 255, 255, 255]);
        assert_eq!(img.pixel(0, 1), [255, 0, 0, 255]);
        assert_eq!(img.pixel(1, 1), [0, 255, 0, 255]);
    }

    #[test]
    fn run_length() {
        let mut b = header(10, 3, 1, 24, 0x20);
        b.extend_from_slice(&[0x81, 1, 2, 3, 0x00, 9, 8, 7]);
        let img = parse(&b).unwrap();
        assert_eq!(img.pixel(0, 0), [3, 2, 1, 255]);
        assert_eq!(img.pixel(1, 0), [3, 2, 1, 255]);
        assert_eq!(img.pixel(2, 0), [7, 8, 9, 255]);
    }
}
