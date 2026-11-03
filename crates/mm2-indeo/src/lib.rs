//! An Indeo Video Interactive 5 (`IV50`) decoder, for the movies of
//! Motocross Madness 2 (`UI/INTRO.AVI`, `CREDITS.AVI`, `EVENTS.AVI`).
//!
//! This is a port to Rust of FFmpeg's Indeo 5 decoder
//! (`libavcodec/indeo5.c`, `ivi.c` and `ivi_dsp.c`, by Maxim Poliakovski),
//! and like them it is under the GNU Lesser General Public License,
//! version 2.1 or later (see `COPYING.LGPLv2.1`). Only what Indeo 5 uses is
//! here: no Indeo 4, no B-frames. The scalable (wavelet band) mode is
//! recognised but not decoded.

mod bits;
mod dsp;
mod tables;

use bits::{Bits, HuffTab, Vlc, BLK_DESCS, MB_DESCS};
use dsp::Transform;
use tables::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Invalid(&'static str),
    Unsupported(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::Invalid(s) => write!(f, "invalid Indeo 5 data: {s}"),
            Error::Unsupported(s) => write!(f, "unsupported Indeo 5 feature: {s}"),
        }
    }
}

impl std::error::Error for Error {}

/// A decoded picture, YUV 4:1:0: the chroma planes are a quarter of the
/// width and height, rounded up.
#[derive(Debug, Clone)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl Frame {
    pub fn chroma_width(&self) -> usize {
        self.width.div_ceil(4)
    }

    /// The picture as RGBA (BT.601, studio range, as Indeo decoders did).
    pub fn rgba(&self) -> Vec<u8> {
        let cw = self.chroma_width();
        let mut out = Vec::with_capacity(self.width * self.height * 4);
        for y in 0..self.height {
            for x in 0..self.width {
                let l = self.y[y * self.width + x] as f32 - 16.0;
                let ci = (y / 4) * cw + x / 4;
                let u = self.u[ci] as f32 - 128.0;
                let v = self.v[ci] as f32 - 128.0;
                let c = |f: f32| f.round().clamp(0.0, 255.0) as u8;
                out.push(c(1.164 * l + 1.596 * v));
                out.push(c(1.164 * l - 0.392 * u - 0.813 * v));
                out.push(c(1.164 * l + 2.017 * u));
                out.push(255);
            }
        }
        out
    }
}

/// `RVMapDesc`: what each block symbol means, a run of zeros and a value.
#[derive(Clone)]
pub(crate) struct RvMap {
    pub eob: u8,
    pub esc: u8,
    pub run: [u8; 256],
    pub val: [i16; 256],
}

const FRAMETYPE_INTRA: u32 = 0;
const FRAMETYPE_INTER: u32 = 1;
const FRAMETYPE_INTER_SCAL: u32 = 2;
const FRAMETYPE_NULL: u32 = 4;
const PIC_SIZE_ESC: u32 = 15;
const IS_PROTECTED: u32 = 0x20;

const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];
const VERTICAL_SCAN_8X8: [u8; 64] = {
    let mut s = [0u8; 64];
    let mut i = 0;
    while i < 64 {
        s[i] = ((i % 8) * 8 + i / 8) as u8;
        i += 1;
    }
    s
};
const HORIZONTAL_SCAN_8X8: [u8; 64] = {
    let mut s = [0u8; 64];
    let mut i = 0;
    while i < 64 {
        s[i] = i as u8;
        i += 1;
    }
    s
};
const DIRECT_SCAN_4X4: [u8; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];

/// `IVI_TOSIGNED`: the sign is in the lowest bit.
fn to_signed(v: u32) -> i32 {
    let half = (v >> 1) as i32;
    if v & 1 != 0 {
        half + 1
    } else {
        -half
    }
}

fn scale_mv(mv: i32, scale: i32) -> i32 {
    (mv + (mv > 0) as i32 + (scale - 1)) >> scale
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct PicConfig {
    pic_width: usize,
    pic_height: usize,
    chroma_width: usize,
    chroma_height: usize,
    tile_width: usize,
    tile_height: usize,
    luma_bands: usize,
    chroma_bands: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct Mb {
    xpos: i32,
    ypos: i32,
    buf_offs: i32,
    kind: u32,
    cbp: u32,
    q_delta: i32,
    mv_x: i32,
    mv_y: i32,
}

#[derive(Debug, Clone, Default)]
struct Tile {
    xpos: usize,
    ypos: usize,
    width: usize,
    height: usize,
    mb_size: usize,
    mbs: Vec<Mb>,
}

#[derive(Clone, Default)]
struct Band {
    plane: usize,
    band_num: usize,
    width: usize,
    height: usize,
    pitch: usize,
    aheight: usize,
    bufsize: usize,
    bufs: [Vec<i16>; 4],
    mb_size: usize,
    blk_size: usize,
    is_halfpel: bool,
    transform: Transform,
    scan: &'static [u8],
    transform_size: usize,
    /// Which dequantisation matrix (0 to 4, or the 4x4 one).
    quant_mat: usize,
    tiles: Vec<Tile>,
    inherit_mv: bool,
    inherit_qdelta: bool,
    qdelta_present: bool,
    corr: Vec<u8>,
    rvmap_sel: usize,
    blk_vlc: HuffTab,
    glob_quant: i32,
}

impl Band {
    fn base(&self, intra: bool) -> &'static [u16] {
        match (self.blk_size, intra) {
            (8, true) => &BASE_QUANT_8X8_INTRA[self.quant_mat],
            (8, false) => &BASE_QUANT_8X8_INTER[self.quant_mat],
            (_, true) => &BASE_QUANT_4X4_INTRA,
            (_, false) => &BASE_QUANT_4X4_INTER,
        }
    }

    fn scale(&self, intra: bool) -> &'static [u8] {
        match (self.blk_size, intra) {
            (8, true) => &SCALE_QUANT_8X8_INTRA[self.quant_mat],
            (8, false) => &SCALE_QUANT_8X8_INTER[self.quant_mat],
            (_, true) => &SCALE_QUANT_4X4_INTRA,
            (_, false) => &SCALE_QUANT_4X4_INTER,
        }
    }
}

#[derive(Clone, Default)]
struct Plane {
    width: usize,
    height: usize,
    bands: Vec<Band>,
}

/// An Indeo 5 stream's decoder: frames go in order, each one's data as the
/// AVI holds it.
pub struct Decoder {
    planes: [Plane; 3],
    pic_conf: PicConfig,
    gop_invalid: bool,
    is_scalable: bool,
    frame_type: u32,
    prev_frame_type: u32,
    gop_flags: u32,
    frame_flags: u32,
    mb_vlc: HuffTab,
    buf_switch: usize,
    dst_buf: usize,
    ref_buf: usize,
    ref2_buf: usize,
    inter_scal: bool,
    buf_invalid: [bool; 4],
    mb_vlcs: [Vlc; 8],
    blk_vlcs: [Vlc; 8],
}

/// Buffers for the band being decoded: where it goes and what it refers to.
struct Bufs<'a> {
    buf: &'a mut [i16],
    reference: &'a [i16],
}

impl Decoder {
    /// A decoder for pictures of this size (the AVI's), until the first
    /// key frame says otherwise.
    pub fn new(width: usize, height: usize) -> Decoder {
        let vlcs = |descs: [&[u8]; 8]| {
            descs.map(|d| {
                Vlc::from_desc(&bits::HuffDesc { xbits: d.to_vec() })
                    .expect("built-in Indeo codebooks are valid")
            })
        };
        let pic_conf = PicConfig {
            pic_width: width,
            pic_height: height,
            chroma_width: width.div_ceil(4),
            chroma_height: height.div_ceil(4),
            tile_width: width,
            tile_height: height,
            luma_bands: 1,
            chroma_bands: 1,
        };
        let mut d = Decoder {
            planes: Default::default(),
            pic_conf,
            gop_invalid: true,
            is_scalable: false,
            frame_type: 0,
            prev_frame_type: 0,
            gop_flags: 0,
            frame_flags: 0,
            mb_vlc: HuffTab::default(),
            buf_switch: 0,
            dst_buf: 0,
            ref_buf: 0,
            ref2_buf: 0,
            inter_scal: false,
            buf_invalid: [false; 4],
            mb_vlcs: vlcs(MB_DESCS),
            blk_vlcs: vlcs(BLK_DESCS),
        };
        d.init_planes(&pic_conf);
        d
    }

    /// `ff_ivi_init_planes`.
    fn init_planes(&mut self, cfg: &PicConfig) {
        self.planes[0].width = cfg.pic_width;
        self.planes[0].height = cfg.pic_height;
        let (cw, ch) = (cfg.pic_width.div_ceil(4), cfg.pic_height.div_ceil(4));
        for p in 1..3 {
            self.planes[p].width = cw;
            self.planes[p].height = ch;
        }
        for p in 0..3 {
            let n = if p == 0 {
                cfg.luma_bands
            } else {
                cfg.chroma_bands
            };
            let plane = &mut self.planes[p];
            let (bw, bh) = if n == 1 {
                (plane.width, plane.height)
            } else {
                (plane.width.div_ceil(2), plane.height.div_ceil(2))
            };
            let align = if p == 0 { 16 } else { 8 };
            let wa = bw.div_ceil(align) * align;
            let ha = bh.div_ceil(align) * align;
            plane.bands = (0..n)
                .map(|b| Band {
                    plane: p,
                    band_num: b,
                    width: bw,
                    height: bh,
                    pitch: wa,
                    aheight: ha,
                    bufsize: wa * ha,
                    scan: &ZIGZAG,
                    ..Default::default()
                })
                .collect();
        }
    }

    /// `ff_ivi_init_tiles`.
    fn init_tiles(&mut self, tile_width: usize, tile_height: usize) -> Result<(), Error> {
        let mut ref_tiles: Vec<Tile> = Vec::new();
        for p in 0..3 {
            let mut tw = if p == 0 {
                tile_width
            } else {
                tile_width.div_ceil(4)
            };
            let mut th = if p == 0 {
                tile_height
            } else {
                tile_height.div_ceil(4)
            };
            if p == 0 && self.planes[0].bands.len() == 4 {
                if tw % 2 != 0 || th % 2 != 0 {
                    return Err(Error::Unsupported("odd tiles"));
                }
                tw >>= 1;
                th >>= 1;
            }
            if tw == 0 || th == 0 {
                return Err(Error::Invalid("tile size"));
            }
            for b in 0..self.planes[p].bands.len() {
                let band = &mut self.planes[p].bands[b];
                let mut tiles = Vec::new();
                let mut y = 0;
                while y < band.height {
                    let mut x = 0;
                    while x < band.width {
                        let width = (band.width - x).min(tw);
                        let height = (band.height - y).min(th);
                        let n = width.div_ceil(band.mb_size.max(1))
                            * height.div_ceil(band.mb_size.max(1));
                        if (p != 0 || b != 0)
                            && ref_tiles.get(tiles.len()).map(|t| t.mbs.len()) != Some(n)
                        {
                            return Err(Error::Invalid("reference tile mismatch"));
                        }
                        tiles.push(Tile {
                            xpos: x,
                            ypos: y,
                            width,
                            height,
                            mb_size: band.mb_size,
                            mbs: vec![Mb::default(); n],
                        });
                        x += tw;
                    }
                    y += th;
                }
                band.tiles = tiles;
                if p == 0 && b == 0 {
                    ref_tiles = band.tiles.clone();
                }
            }
        }
        Ok(())
    }

    /// Decodes one frame. A null frame (the picture doesn't change) gives
    /// `None`.
    pub fn decode(&mut self, data: &[u8]) -> Result<Option<Frame>, Error> {
        let mut bits = Bits::new(data);
        self.decode_pic_hdr(&mut bits)?;
        if self.gop_invalid {
            return Err(Error::Invalid("no key frame yet"));
        }
        if self.gop_flags & IS_PROTECTED != 0 {
            return Err(Error::Unsupported("password-protected clip"));
        }
        self.switch_buffers();
        let nonnull = self.frame_type != FRAMETYPE_NULL;
        if nonnull {
            self.buf_invalid[self.dst_buf] = true;
            for p in 0..3 {
                for b in 0..self.planes[p].bands.len() {
                    self.decode_band(&mut bits, p, b)?;
                }
            }
            self.buf_invalid[self.dst_buf] = false;
        } else {
            if self.is_scalable {
                return Err(Error::Invalid("null frame in a scalable stream"));
            }
            for p in 0..3 {
                if self.planes[p].bands[0].bufs[self.dst_buf].is_empty() {
                    return Err(Error::Invalid("null frame before a picture"));
                }
            }
        }
        if self.buf_invalid[self.dst_buf] {
            return Err(Error::Invalid("broken reference"));
        }
        if !nonnull {
            return Ok(None);
        }
        if self.is_scalable {
            return Err(Error::Unsupported("scalable (wavelet) pictures"));
        }
        let out = |plane: &Plane, dst: usize| {
            let band = &plane.bands[0];
            let src = &band.bufs[dst];
            let mut px = Vec::with_capacity(plane.width * plane.height);
            for y in 0..plane.height {
                for x in 0..plane.width {
                    px.push((src[y * band.pitch + x] as i32 + 128).clamp(0, 255) as u8);
                }
            }
            px
        };
        Ok(Some(Frame {
            width: self.planes[0].width,
            height: self.planes[0].height,
            y: out(&self.planes[0], self.dst_buf),
            u: out(&self.planes[2], self.dst_buf),
            v: out(&self.planes[1], self.dst_buf),
        }))
    }

    /// `decode_gop_header`.
    fn decode_gop_header(&mut self, bits: &mut Bits) -> Result<(), Error> {
        self.gop_flags = bits.get(8);
        if self.gop_flags & 1 != 0 {
            bits.get(16); // the GOP header's size
        }
        if self.gop_flags & IS_PROTECTED != 0 {
            bits.get(32); // lock word
        }
        let tile_size = if self.gop_flags & 0x40 != 0 {
            64usize << bits.get(2)
        } else {
            0
        };
        if tile_size > 256 {
            return Err(Error::Invalid("tile size"));
        }
        let mut cfg = PicConfig {
            luma_bands: bits.get(2) as usize * 3 + 1,
            chroma_bands: bits.get(1) as usize * 3 + 1,
            ..Default::default()
        };
        let is_scalable = cfg.luma_bands != 1 || cfg.chroma_bands != 1;
        if is_scalable && (cfg.luma_bands != 4 || cfg.chroma_bands != 1) {
            return Err(Error::Invalid("band subdivision"));
        }
        let size = bits.get(4);
        if size == PIC_SIZE_ESC {
            cfg.pic_height = bits.get(13) as usize;
            cfg.pic_width = bits.get(13) as usize;
        } else {
            cfg.pic_height = (COMMON_PIC_SIZES[size as usize * 2 + 1] as usize) << 2;
            cfg.pic_width = (COMMON_PIC_SIZES[size as usize * 2] as usize) << 2;
        }
        if cfg.pic_width == 0 || cfg.pic_height == 0 {
            return Err(Error::Invalid("picture size"));
        }
        if self.gop_flags & 2 != 0 {
            return Err(Error::Unsupported("YV12 pictures"));
        }
        cfg.chroma_height = cfg.pic_height.div_ceil(4);
        cfg.chroma_width = cfg.pic_width.div_ceil(4);
        if tile_size == 0 {
            cfg.tile_height = cfg.pic_height;
            cfg.tile_width = cfg.pic_width;
        } else {
            cfg.tile_height = tile_size;
            cfg.tile_width = tile_size;
        }
        let mut blk_size_changed = false;
        if cfg != self.pic_conf || self.gop_invalid {
            self.init_planes(&cfg);
            self.pic_conf = cfg;
            self.is_scalable = is_scalable;
            blk_size_changed = true;
        }
        for p in 0..=1 {
            let n = if p == 0 {
                cfg.luma_bands
            } else {
                cfg.chroma_bands
            };
            for i in 0..n {
                let band = &mut self.planes[p].bands[i];
                band.is_halfpel = bits.get1();
                let mb_flag = bits.get1();
                let blk_size = 8 >> bits.get(1);
                let mb_size = blk_size << (!mb_flag) as u32;
                if p == 0 && blk_size == 4 {
                    return Err(Error::Unsupported("4x4 luma blocks"));
                }
                // As in FFmpeg, each band's answer replaces the last.
                blk_size_changed = mb_size != band.mb_size || blk_size != band.blk_size;
                if blk_size_changed {
                    band.mb_size = mb_size;
                    band.blk_size = blk_size;
                }
                if bits.get1() {
                    return Err(Error::Unsupported("extended transform info"));
                }
                let (t, scan, size): (Transform, &'static [u8], usize) = match (p << 2) + i {
                    0 => (Transform::Slant8x8, &ZIGZAG, 8),
                    1 => (Transform::RowSlant8, &VERTICAL_SCAN_8X8, 8),
                    2 => (Transform::ColSlant8, &HORIZONTAL_SCAN_8X8, 8),
                    3 => (Transform::Put8x8, &HORIZONTAL_SCAN_8X8, 8),
                    _ => (Transform::Slant4x4, &DIRECT_SCAN_4X4, 4),
                };
                band.transform = t;
                band.scan = scan;
                band.transform_size = size;
                if band.transform_size != band.blk_size {
                    return Err(Error::Invalid("transform and block size mismatch"));
                }
                let quant_mat = if p == 0 {
                    if cfg.luma_bands > 1 {
                        i + 1
                    } else {
                        0
                    }
                } else {
                    5
                };
                if band.blk_size == 8 {
                    if quant_mat >= 5 {
                        return Err(Error::Invalid("quantisation matrix"));
                    }
                    band.quant_mat = quant_mat;
                }
                if bits.get(2) != 0 {
                    return Err(Error::Invalid("band end marker"));
                }
            }
        }
        for i in 0..cfg.chroma_bands {
            let b1 = self.planes[1].bands[i].clone();
            let b2 = &mut self.planes[2].bands[i];
            b2.width = b1.width;
            b2.height = b1.height;
            b2.mb_size = b1.mb_size;
            b2.blk_size = b1.blk_size;
            b2.is_halfpel = b1.is_halfpel;
            b2.quant_mat = b1.quant_mat;
            b2.scan = b1.scan;
            b2.transform = b1.transform;
            b2.transform_size = b1.transform_size;
        }
        if blk_size_changed {
            self.init_tiles(cfg.tile_width, cfg.tile_height)?;
        }
        if self.gop_flags & 8 != 0 {
            if bits.get(3) != 0 {
                return Err(Error::Invalid("alignment bits"));
            }
            if bits.get1() {
                bits.skip(24); // transparency colour
            }
        }
        bits.align();
        bits.skip(23);
        if bits.get1() {
            while bits.get(16) & 0x8000 != 0 {}
        }
        bits.align();
        Ok(())
    }

    /// `decode_pic_hdr`.
    fn decode_pic_hdr(&mut self, bits: &mut Bits) -> Result<(), Error> {
        if bits.get(5) != 0x1f {
            return Err(Error::Invalid("picture start code"));
        }
        self.prev_frame_type = self.frame_type;
        self.frame_type = bits.get(3);
        if self.frame_type >= 5 {
            self.frame_type = FRAMETYPE_INTRA;
            return Err(Error::Invalid("frame type"));
        }
        bits.get(8); // frame number
        if self.frame_type == FRAMETYPE_INTRA {
            if let Err(e) = self.decode_gop_header(bits) {
                self.gop_invalid = true;
                return Err(e);
            }
            self.gop_invalid = false;
        }
        if self.frame_type == FRAMETYPE_INTER_SCAL && !self.is_scalable {
            self.frame_type = FRAMETYPE_INTER;
            return Err(Error::Invalid("scalable frame in a plain stream"));
        }
        if self.frame_type != FRAMETYPE_NULL {
            self.frame_flags = bits.get(8);
            if self.frame_flags & 1 != 0 {
                bits.get(24); // picture header size
            }
            if self.frame_flags & 0x10 != 0 {
                bits.get(16); // checksum
            }
            if self.frame_flags & 0x20 != 0 {
                skip_hdr_extension(bits)?;
            }
            let coded = self.frame_flags & 0x40 != 0;
            self.mb_vlc.read(bits, coded)?;
            bits.skip(3);
        }
        bits.align();
        Ok(())
    }

    /// `switch_buffers`.
    fn switch_buffers(&mut self) {
        match self.prev_frame_type {
            FRAMETYPE_INTRA | FRAMETYPE_INTER => {
                self.buf_switch ^= 1;
                self.dst_buf = self.buf_switch;
                self.ref_buf = self.buf_switch ^ 1;
            }
            FRAMETYPE_INTER_SCAL => {
                if !self.inter_scal {
                    self.ref2_buf = 2;
                    self.inter_scal = true;
                }
                std::mem::swap(&mut self.dst_buf, &mut self.ref2_buf);
                self.ref_buf = self.ref2_buf;
            }
            _ => {}
        }
        match self.frame_type {
            FRAMETYPE_INTRA | FRAMETYPE_INTER => {
                if self.frame_type == FRAMETYPE_INTRA {
                    self.buf_switch = 0;
                }
                self.inter_scal = false;
                self.dst_buf = self.buf_switch;
                self.ref_buf = self.buf_switch ^ 1;
            }
            _ => {}
        }
    }

    /// `decode_band_hdr`: false for an empty band.
    fn decode_band_hdr(&mut self, bits: &mut Bits, p: usize, b: usize) -> Result<bool, Error> {
        let frame_flags = self.frame_flags;
        let band = &mut self.planes[p].bands[b];
        let flags = bits.get(8);
        if flags & 1 != 0 {
            return Ok(false);
        }
        if frame_flags & 0x80 != 0 {
            bits.get(24); // data size
        }
        band.inherit_mv = flags & 2 != 0;
        band.inherit_qdelta = flags & 8 != 0;
        band.qdelta_present = flags & 4 != 0;
        if !band.qdelta_present {
            band.inherit_qdelta = true;
        }
        band.corr.clear();
        if flags & 0x10 != 0 {
            let n = bits.get(8) as usize;
            if n > 61 {
                return Err(Error::Invalid("too many corrections"));
            }
            band.corr = (0..n * 2).map(|_| bits.get(8) as u8).collect();
        }
        band.rvmap_sel = if flags & 0x40 != 0 {
            bits.get(3) as usize
        } else {
            8
        };
        band.blk_vlc.read(bits, flags & 0x80 != 0)?;
        if bits.get1() {
            bits.get(16); // checksum
        }
        band.glob_quant = bits.get(5) as i32;
        if flags & 0x20 != 0 {
            bits.align();
            skip_hdr_extension(bits)?;
        }
        bits.align();
        Ok(true)
    }

    /// `decode_band`.
    fn decode_band(&mut self, bits: &mut Bits, p: usize, b: usize) -> Result<(), Error> {
        let (dst, rf) = (self.dst_buf, self.ref_buf);
        if self.pic_conf.luma_bands <= 1 && (dst == 2 || rf == 2) {
            return Err(Error::Invalid("band buffer points to no data"));
        }
        if !self.decode_band_hdr(bits, p, b)? {
            return Err(Error::Invalid("empty band"));
        }
        // The run/value table with this band's corrections.
        let band = &self.planes[p].bands[b];
        let mut rv = RVMAPS[band.rvmap_sel].clone();
        for pair in band.corr.chunks(2) {
            let (i1, i2) = (pair[0] as usize, pair[1] as usize);
            rv.run.swap(i1, i2);
            rv.val.swap(i1, i2);
            if i1 == rv.eob as usize || i2 == rv.eob as usize {
                rv.eob ^= (i1 ^ i2) as u8;
            }
            if i1 == rv.esc as usize || i2 == rv.esc as usize {
                rv.esc ^= (i1 ^ i2) as u8;
            }
        }
        let size = band.bufsize;
        let mv_scale = (self.planes[0].bands[0].mb_size >> 3) as i32 - (band.mb_size >> 3) as i32;
        // The first luma band's tiles give the others their motion and
        // quantiser deltas.
        let ref_tiles: Option<Vec<Vec<Mb>>> = (p != 0 || b != 0).then(|| {
            self.planes[0].bands[0]
                .tiles
                .iter()
                .map(|t| t.mbs.clone())
                .collect()
        });
        let mut band = std::mem::take(&mut self.planes[p].bands[b]);
        for i in [dst, rf] {
            if band.bufs[i].len() != size {
                band.bufs[i] = vec![0; size];
            }
        }
        let mut buf = std::mem::take(&mut band.bufs[dst]);
        let reference = std::mem::take(&mut band.bufs[rf]);
        let mut tiles = std::mem::take(&mut band.tiles);
        let mut result = Ok(());
        let mut pos = bits.count();
        for (t, tile) in tiles.iter_mut().enumerate() {
            if tile.mb_size != band.mb_size {
                result = Err(Error::Invalid("macroblock sizes mismatch"));
                break;
            }
            let ref_mbs = ref_tiles
                .as_ref()
                .and_then(|r| r.get(t))
                .map(|v| v.as_slice());
            let mut bufs = Bufs {
                buf: &mut buf,
                reference: &reference,
            };
            if bits.get1() {
                if let Err(e) = process_empty_tile(&band, tile, ref_mbs, mv_scale, &mut bufs) {
                    result = Err(e);
                    break;
                }
            } else {
                let data_size = tile_data_size(bits);
                if data_size == 0 {
                    result = Err(Error::Invalid("tile data size is zero"));
                    break;
                }
                if let Err(e) = self.decode_mb_info(bits, &band, tile, ref_mbs) {
                    result = Err(e);
                    break;
                }
                if let Err(e) = self.decode_blocks(bits, &band, tile, &rv, &mut bufs) {
                    result = Err(e);
                    break;
                }
                if (bits.count() - pos) >> 3 != data_size {
                    result = Err(Error::Invalid("tile data size mismatch"));
                    break;
                }
                pos += data_size << 3;
            }
        }
        band.tiles = tiles;
        band.bufs[dst] = buf;
        band.bufs[rf] = reference;
        self.planes[p].bands[b] = band;
        bits.align();
        result
    }

    /// `decode_mb_info`.
    fn decode_mb_info(
        &self,
        bits: &mut Bits,
        band: &Band,
        tile: &mut Tile,
        ref_mbs: Option<&[Mb]>,
    ) -> Result<(), Error> {
        let row_offset = (band.mb_size * band.pitch) as i32;
        let mut offs = (tile.ypos * band.pitch + tile.xpos) as i32;
        if ref_mbs.is_none() && ((band.qdelta_present && band.inherit_qdelta) || band.inherit_mv) {
            return Err(Error::Invalid("inheritance without a reference"));
        }
        let n = tile.width.div_ceil(band.mb_size) * tile.height.div_ceil(band.mb_size);
        if tile.mbs.len() != n {
            return Err(Error::Invalid("tile size mismatch"));
        }
        let mv_scale = (self.planes[0].bands[0].mb_size >> 3) as i32 - (band.mb_size >> 3) as i32;
        let (mut mv_x, mut mv_y) = (0i32, 0i32);
        let mb_vlc = self.mb_vlc.vlc(&self.mb_vlcs);
        let first_band = band.plane == 0 && band.band_num == 0;
        let qdelta_everywhere = first_band && self.frame_flags & 8 != 0;
        let inherit = |r: &Mb| {
            if mv_scale != 0 {
                (scale_mv(r.mv_x, mv_scale), scale_mv(r.mv_y, mv_scale))
            } else {
                (r.mv_x, r.mv_y)
            }
        };
        let mut i = 0usize;
        let mut y = tile.ypos;
        while y < tile.ypos + tile.height {
            let mut mb_offset = offs;
            let mut x = tile.xpos;
            while x < tile.xpos + tile.width {
                let r = ref_mbs.and_then(|r| r.get(i)).copied();
                let mb = &mut tile.mbs[i];
                mb.xpos = x as i32;
                mb.ypos = y as i32;
                mb.buf_offs = mb_offset;
                if bits.get1() {
                    if self.frame_type == FRAMETYPE_INTRA {
                        return Err(Error::Invalid("empty macroblock in an intra picture"));
                    }
                    mb.kind = 1;
                    mb.cbp = 0;
                    mb.q_delta = 0;
                    if qdelta_everywhere {
                        mb.q_delta = to_signed(bits.vlc(mb_vlc)?);
                    }
                    mb.mv_x = 0;
                    mb.mv_y = 0;
                    if band.inherit_mv {
                        if let Some(r) = &r {
                            (mb.mv_x, mb.mv_y) = inherit(r);
                        }
                    }
                } else {
                    mb.kind = if let (true, Some(r)) = (band.inherit_mv, &r) {
                        r.kind
                    } else if self.frame_type == FRAMETYPE_INTRA {
                        0
                    } else {
                        bits.get(1)
                    };
                    let blks = if band.mb_size != band.blk_size { 4 } else { 1 };
                    mb.cbp = bits.get(blks);
                    mb.q_delta = 0;
                    if band.qdelta_present {
                        if band.inherit_qdelta {
                            if let Some(r) = &r {
                                mb.q_delta = r.q_delta;
                            }
                        } else if mb.cbp != 0 || qdelta_everywhere {
                            mb.q_delta = to_signed(bits.vlc(mb_vlc)?);
                        }
                    }
                    if mb.kind == 0 {
                        mb.mv_x = 0;
                        mb.mv_y = 0;
                    } else if band.inherit_mv && r.is_some() {
                        (mb.mv_x, mb.mv_y) = inherit(&r.unwrap());
                    } else {
                        mv_y += to_signed(bits.vlc(mb_vlc)?);
                        mv_x += to_signed(bits.vlc(mb_vlc)?);
                        mb.mv_x = mv_x;
                        mb.mv_y = mv_y;
                    }
                }
                let s = band.is_halfpel as i32;
                let (xi, yi, pitch, ms) =
                    (x as i32, y as i32, band.pitch as i32, band.mb_size as i32);
                if mb.kind != 0
                    && (xi + (mb.mv_x >> s) + (yi + (mb.mv_y >> s)) * pitch < 0
                        || xi + ((mb.mv_x + s) >> s) + ms - 1
                            + (yi + ms - 1 + ((mb.mv_y + s) >> s)) * pitch
                            > band.bufsize as i32 - 1)
                {
                    return Err(Error::Invalid("motion vector outside the reference"));
                }
                i += 1;
                mb_offset += band.mb_size as i32;
                x += band.mb_size;
            }
            offs += row_offset;
            y += band.mb_size;
        }
        bits.align();
        Ok(())
    }

    /// `ivi_decode_blocks`.
    fn decode_blocks(
        &self,
        bits: &mut Bits,
        band: &Band,
        tile: &Tile,
        rv: &RvMap,
        bufs: &mut Bufs,
    ) -> Result<(), Error> {
        let blk = band.blk_size;
        let num_blocks = if band.mb_size != blk { 4 } else { 1 };
        let vlc = band.blk_vlc.vlc(&self.blk_vlcs);
        let mut prev_dc = 0i32;
        let pitch = band.pitch as i32;
        for mb in &tile.mbs {
            let intra = mb.kind == 0;
            let mut cbp = mb.cbp;
            let mut offs = mb.buf_offs;
            let q = (band.glob_quant + mb.q_delta).clamp(0, 23) as usize;
            let quant = band.scale(intra)[q] as u32;
            let (mut mv_x, mut mv_y, mut mc_type) = (0, 0, 0u32);
            if !intra {
                mv_x = mb.mv_x;
                mv_y = mb.mv_y;
                if band.is_halfpel {
                    mc_type = (((mv_y & 1) << 1) | (mv_x & 1)) as u32;
                    mv_x >>= 1;
                    mv_y >>= 1;
                }
                let hp = band.is_halfpel as i32;
                let (dx, dy) = (mb.mv_x >> hp, mb.mv_y >> hp);
                let (cx, cy) = (mb.mv_x & hp, mb.mv_y & hp);
                let ms = band.mb_size as i32;
                if mb.xpos + dx < 0
                    || mb.xpos + dx + ms + cx > pitch
                    || mb.ypos + dy < 0
                    || mb.ypos + dy + ms + cy > band.aheight as i32
                {
                    return Err(Error::Invalid("motion vector out of bounds"));
                }
            }
            for n in 0..num_blocks {
                if n & 1 != 0 {
                    offs += blk as i32;
                } else if n == 2 {
                    offs -= blk as i32;
                    offs += blk as i32 * pitch;
                }
                if cbp & 1 != 0 {
                    decode_coded_block(
                        bits,
                        band,
                        vlc,
                        rv,
                        bufs,
                        mv_x,
                        mv_y,
                        &mut prev_dc,
                        intra,
                        mc_type,
                        quant,
                        offs,
                    )?;
                } else {
                    let size = (pitch * band.aheight as i32 - offs) as isize;
                    let min = ((blk as i32 - 1) * pitch + blk as i32) as isize;
                    if min > size {
                        return Err(Error::Invalid("block outside the picture"));
                    }
                    if intra {
                        band.transform
                            .dc(prev_dc, bufs.buf, offs as usize, band.pitch, blk);
                    } else {
                        motion(band, bufs, offs, mv_x, mv_y, mc_type, false)?;
                    }
                }
                cbp >>= 1;
            }
        }
        bits.align();
        Ok(())
    }
}

fn skip_hdr_extension(bits: &mut Bits) -> Result<(), Error> {
    loop {
        let len = bits.get(8) as isize;
        if 8 * len > bits.left() {
            return Err(Error::Invalid("header extension"));
        }
        bits.skip(8 * len as usize);
        if len == 0 {
            return Ok(());
        }
    }
}

/// `ivi_dec_tile_data_size`.
fn tile_data_size(bits: &mut Bits) -> usize {
    let mut len = 0;
    if bits.get1() {
        len = bits.get(8) as usize;
        if len == 255 {
            len = bits.get(24) as usize;
        }
    }
    bits.align();
    len
}

/// `ivi_mc` without the second reference Indeo 5 never has.
fn motion(
    band: &Band,
    bufs: &mut Bufs,
    offs: i32,
    mv_x: i32,
    mv_y: i32,
    mc_type: u32,
    add: bool,
) -> Result<(), Error> {
    let pitch = band.pitch as i32;
    let ref_offs = offs + mv_y * pitch + mv_x;
    let buf_size = pitch * band.aheight as i32;
    let blk = band.blk_size as i32;
    let min_size = pitch * (blk - 1) + blk;
    let ref_size = (mc_type > 1) as i32 * pitch + (mc_type & 1) as i32;
    if offs < 0
        || ref_offs < 0
        || buf_size - min_size < offs
        || buf_size - min_size - ref_size < ref_offs
    {
        return Err(Error::Invalid("motion compensation outside the picture"));
    }
    dsp::mc(
        bufs.buf,
        offs as usize,
        bufs.reference,
        ref_offs as usize,
        band.pitch,
        band.blk_size,
        mc_type,
        add,
    );
    Ok(())
}

/// `ivi_decode_coded_blocks`.
#[allow(clippy::too_many_arguments)]
fn decode_coded_block(
    bits: &mut Bits,
    band: &Band,
    vlc: &Vlc,
    rv: &RvMap,
    bufs: &mut Bufs,
    mv_x: i32,
    mv_y: i32,
    prev_dc: &mut i32,
    intra: bool,
    mc_type: u32,
    quant: u32,
    offs: i32,
) -> Result<(), Error> {
    let base = band.base(intra);
    let blk = band.blk_size;
    let num_coeffs = (blk * blk) as i32;
    let col_mask = blk - 1;
    let pitch = band.pitch as i32;
    let min_size = pitch * (band.transform_size as i32 - 1) + band.transform_size as i32;
    if min_size > pitch * band.aheight as i32 - offs {
        return Err(Error::Invalid("block outside the picture"));
    }
    let mut trvec = [0i32; 64];
    let mut col_flags = [0u8; 8];
    let mut scan_pos: i32 = -1;
    let mut sym = 0u32;
    while scan_pos <= num_coeffs {
        sym = bits.vlc(vlc)?;
        if sym == rv.eob as u32 {
            break;
        }
        let (run, val): (i32, i32) = if sym == rv.esc as u32 {
            let run = bits.vlc(vlc)? as i32 + 1;
            let lo = bits.vlc(vlc)?;
            let hi = bits.vlc(vlc)?;
            (run, to_signed((hi << 6) | lo))
        } else {
            if sym >= 256 {
                return Err(Error::Invalid("block symbol"));
            }
            (rv.run[sym as usize] as i32, rv.val[sym as usize] as i32)
        };
        scan_pos += run;
        if scan_pos >= num_coeffs || scan_pos < 0 {
            break;
        }
        let pos = band.scan[scan_pos as usize] as usize;
        let q = (base[pos] as u32 * quant) >> 9;
        let mut val = val;
        if q > 1 {
            let sign = val.signum();
            val = val
                .wrapping_mul(q as i32)
                .wrapping_add(sign.wrapping_mul((((q ^ 1) - 1) >> 1) as i32));
        }
        trvec[pos] = val;
        col_flags[pos & col_mask] |= (val != 0) as u8;
    }
    if scan_pos < 0 || (scan_pos >= num_coeffs && sym != rv.eob as u32) {
        return Err(Error::Invalid("corrupt block"));
    }
    if intra && band.transform.is_2d() {
        *prev_dc += trvec[0];
        trvec[0] = *prev_dc;
        col_flags[0] |= (*prev_dc != 0) as u8;
    }
    band.transform
        .inverse(&trvec, bufs.buf, offs as usize, band.pitch, &col_flags);
    if !intra {
        motion(band, bufs, offs, mv_x, mv_y, mc_type, true)?;
    }
    Ok(())
}

/// `ivi_process_empty_tile`: copied from the reference, moved where the
/// first band says.
fn process_empty_tile(
    band: &Band,
    tile: &mut Tile,
    ref_mbs: Option<&[Mb]>,
    mv_scale: i32,
    bufs: &mut Bufs,
) -> Result<(), Error> {
    let ms = band.mb_size;
    let n = tile.width.div_ceil(ms) * tile.height.div_ceil(ms);
    if tile.mbs.len() != n {
        return Err(Error::Invalid("tile size mismatch"));
    }
    let clear_first = !band.qdelta_present && band.plane == 0 && band.band_num == 0;
    let pitch = band.pitch;
    let hp = band.is_halfpel as i32;
    let mut offs = tile.ypos * pitch + tile.xpos;
    let mut need_mc = false;
    let mut i = 0;
    let mut y = tile.ypos;
    while y < tile.ypos + tile.height {
        let mut mb_offset = offs;
        let mut x = tile.xpos;
        while x < tile.xpos + tile.width {
            let mb = &mut tile.mbs[i];
            mb.xpos = x as i32;
            mb.ypos = y as i32;
            mb.buf_offs = mb_offset as i32;
            mb.kind = 1;
            mb.cbp = 0;
            if clear_first {
                mb.q_delta = band.glob_quant;
                mb.mv_x = 0;
                mb.mv_y = 0;
            }
            if let Some(r) = ref_mbs.and_then(|r| r.get(i)) {
                if band.inherit_qdelta {
                    mb.q_delta = r.q_delta;
                }
                if band.inherit_mv {
                    if mv_scale != 0 {
                        mb.mv_x = scale_mv(r.mv_x, mv_scale);
                        mb.mv_y = scale_mv(r.mv_y, mv_scale);
                    } else {
                        mb.mv_x = r.mv_x;
                        mb.mv_y = r.mv_y;
                    }
                    need_mc |= mb.mv_x != 0 || mb.mv_y != 0;
                    let (dx, dy) = (mb.mv_x >> hp, mb.mv_y >> hp);
                    let (cx, cy) = (mb.mv_x & hp, mb.mv_y & hp);
                    if mb.xpos + dx < 0
                        || mb.xpos + dx + ms as i32 + cx > pitch as i32
                        || mb.ypos + dy < 0
                        || mb.ypos + dy + ms as i32 + cy > band.aheight as i32
                    {
                        return Err(Error::Invalid("motion vector out of bounds"));
                    }
                }
            }
            i += 1;
            mb_offset += ms;
            x += ms;
        }
        offs += ms * pitch;
        y += ms;
    }
    if band.inherit_mv && need_mc {
        let num_blocks = if ms != band.blk_size { 4 } else { 1 };
        for mb in &tile.mbs {
            let (mut mv_x, mut mv_y) = (mb.mv_x, mb.mv_y);
            let mut mc_type = 0;
            if band.is_halfpel {
                mc_type = (((mv_y & 1) << 1) | (mv_x & 1)) as u32;
                mv_x >>= 1;
                mv_y >>= 1;
            }
            for n in 0..num_blocks {
                let o = mb.buf_offs
                    + (band.blk_size * ((n & 1) + ((n & 2) != 0) as usize * pitch)) as i32;
                motion(band, bufs, o, mv_x, mv_y, mc_type, false)?;
            }
        }
    } else {
        for row in 0..tile.height {
            let o = (tile.ypos + row) * pitch + tile.xpos;
            bufs.buf[o..o + tile.width].copy_from_slice(&bufs.reference[o..o + tile.width]);
        }
    }
    Ok(())
}
