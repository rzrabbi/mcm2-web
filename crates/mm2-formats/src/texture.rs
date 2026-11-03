//! Model textures. They keep their `.tga` names but are not TGA files.
//!
//! ```text
//! u32 format (22 = RGB555, 26 = ARGB4444), u32 (2), u32 width, u32 height
//! u32 name_len, name   // palette for 8-bit cards, e.g. "RGBMaps.pal8"
//! mip chain            // as in terrain textures
//! ```
//!
//! Track makers' tools also wrote the game's other format codes (see
//! [`Stored`]), some as one level without mips.

use crate::reader::Reader;
use crate::trn::{read_image, PixelFormat, Stored, Texture};
use crate::{Error, Result};

pub fn parse(bytes: &[u8]) -> Result<Texture> {
    let mut r = Reader::new(bytes, 0);
    let Some(stored) = Stored::from_code(r.u32("texture format")?) else {
        return from_targa(bytes);
    };
    r.u32("texture version")?;
    let width = r.u32("texture width")?;
    let height = r.u32("texture height")?;
    if width != height {
        return Err(Error::Corrupt("texture is not square"));
    }
    let name_len = r.u32("palette name length")? as usize;
    r.take(name_len, "palette name")?;
    read_image(&mut r, width, stored)
}

/// A real Targa file under a texture's name: bike makers' installers put
/// their paint into `BIKE.RES` this way. Kept with its alpha when it has
/// any, else opaque.
fn from_targa(bytes: &[u8]) -> Result<Texture> {
    let unknown = Error::Corrupt("unknown texture format");
    let image = crate::tga::parse(bytes).map_err(|_| unknown)?;
    let size = image.width;
    if image.height != size || !size.is_power_of_two() || size > 1024 {
        return Err(Error::Corrupt("texture is not square"));
    }
    let alpha = image.rgba.chunks_exact(4).any(|p| p[3] < 255);
    let (code, data): (u32, Vec<u8>) = if alpha {
        (0xc, image.rgba.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect())
    } else {
        (1, image.rgba.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0]]).collect())
    };
    let stored = Stored { packed: false, ..Stored::from_code(code).expect("a known code") };
    read_image(&mut Reader::new(&data, 0), size, stored)
}

/// The overlay pictures in `RES/GLOBAL.RES` (the speedometer, the race
/// panel, the radar): `.tga` names again, but one level of any size.
///
/// ```text
/// u32 format (11 = ARGB4444, 7 = RGB555), u32 bytes after this field,
/// u32 width, u32 height, u32 name_len, name   // the palette
/// u32 bytes (this field on), pixels (LZW unless width × height × 2)
/// ```
pub fn parse_overlay(bytes: &[u8]) -> Result<crate::tga::Image> {
    // Some are model textures, with their mip chain.
    if bytes.get(4..8) == Some(&[2, 0, 0, 0]) {
        let t = parse(bytes)?;
        return Ok(crate::tga::Image {
            width: t.size,
            height: t.size,
            rgba: t.rgba8(0),
        });
    }
    let mut r = Reader::new(bytes, 0);
    let code = r.u32("overlay format")?;
    let format = match code {
        11 => PixelFormat::Argb4444,
        7 => PixelFormat::Rgb555,
        _ => PixelFormat::from_code(code).ok_or(Error::Corrupt("unknown overlay format"))?,
    };
    r.u32("overlay size")?;
    let width = r.u32("overlay width")?;
    let height = r.u32("overlay height")?;
    let name_len = r.u32("palette name length")? as usize;
    r.take(name_len, "palette name")?;
    // The packed stream starts with its own length.
    let start = r.pos;
    let len = r.u32("overlay pixels")? as usize;
    let size = (width * height * 2) as usize;
    let data = if len == size + 4 {
        r.take(size, "overlay pixels")?.to_vec()
    } else {
        crate::lzw::decompress(&bytes[start..], size)?
    };
    let texture = Texture {
        size: width,
        format,
        levels: vec![data
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect()],
    };
    Ok(crate::tga::Image {
        width,
        height,
        rgba: texture.rgba8(0),
    })
}
