//! Bitmap fonts in `RES/FONTS.RES`: a `.tga` sheet of glyphs and a `.cell`
//! file giving each character's box on it.
//!
//! ```text
//! u32 count
//! count × { u32 char, f32 left, f32 top, f32 right, f32 bottom }   // 0 to 1
//! ```

use crate::reader::Reader;
use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    pub ch: char,
    /// Left, top, right, bottom on the sheet, 0 to 1.
    pub uv: [f32; 4],
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cells {
    pub glyphs: Vec<Glyph>,
}

impl Cells {
    pub fn glyph(&self, ch: char) -> Option<&Glyph> {
        self.glyphs.iter().find(|g| g.ch == ch)
    }
}

pub fn parse_cells(bytes: &[u8]) -> Result<Cells> {
    let mut r = Reader::new(bytes, 0);
    let count = r.u32("cell count")?.min(1 << 16);
    let mut glyphs = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let code = r.u32("cell char")?;
        let uv = [
            r.f32("cell")?,
            r.f32("cell")?,
            r.f32("cell")?,
            r.f32("cell")?,
        ];
        if let Some(ch) = char::from_u32(code) {
            glyphs.push(Glyph { ch, uv });
        }
    }
    Ok(Cells { glyphs })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_glyph() {
        let mut b = 1u32.to_le_bytes().to_vec();
        b.extend_from_slice(&65u32.to_le_bytes());
        for v in [0.25f32, 0.0, 0.5, 0.125] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let cells = parse_cells(&b).unwrap();
        assert_eq!(cells.glyph('A').unwrap().uv, [0.25, 0.0, 0.5, 0.125]);
    }
}
