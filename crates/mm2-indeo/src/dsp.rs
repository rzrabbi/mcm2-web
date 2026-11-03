//! Inverse transforms and motion compensation (FFmpeg's `ivi_dsp.c`).
//! Picture buffers are `i16` with a pitch; coefficients are `i32`.

/// The inverse transforms Indeo 5 picks by plane and band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Transform {
    #[default]
    Slant8x8,
    RowSlant8,
    ColSlant8,
    Put8x8,
    Slant4x4,
}

impl Transform {
    pub fn is_2d(self) -> bool {
        matches!(self, Transform::Slant8x8 | Transform::Slant4x4)
    }

    pub fn inverse(
        self,
        input: &[i32; 64],
        out: &mut [i16],
        offs: usize,
        pitch: usize,
        flags: &[u8; 8],
    ) {
        match self {
            Transform::Slant8x8 => inverse_slant_8x8(input, out, offs, pitch, flags),
            Transform::RowSlant8 => row_slant8(input, out, offs, pitch),
            Transform::ColSlant8 => col_slant8(input, out, offs, pitch, flags),
            Transform::Put8x8 => put_pixels_8x8(input, out, offs, pitch),
            Transform::Slant4x4 => inverse_slant_4x4(input, out, offs, pitch, flags),
        }
    }

    /// The transform of a block with only its DC coefficient.
    pub fn dc(self, dc: i32, out: &mut [i16], offs: usize, pitch: usize, blk: usize) {
        let c = ((dc + 1) >> 1) as i16;
        match self {
            Transform::Slant8x8 | Transform::Slant4x4 => {
                for y in 0..blk {
                    out[offs + y * pitch..][..blk].fill(c);
                }
            }
            Transform::RowSlant8 => {
                out[offs..][..blk].fill(c);
                for y in 1..blk {
                    out[offs + y * pitch..][..blk].fill(0);
                }
            }
            Transform::ColSlant8 => {
                for y in 0..blk {
                    let row = &mut out[offs + y * pitch..][..blk];
                    row.fill(0);
                    row[0] = c;
                }
            }
            Transform::Put8x8 => {
                for y in 0..8 {
                    out[offs + y * pitch..][..8].fill(0);
                }
                out[offs] = dc as i16;
            }
        }
    }
}

/// `IVI_INV_SLANT8`, before compensation: coefficients in order, results
/// in order.
fn slant8(i: [i32; 8]) -> [i32; 8] {
    let (s1, s4, s8, s5, s2, s6, s3, s7) = (i[0], i[1], i[2], i[3], i[4], i[5], i[6], i[7]);
    // IVI_SLANT_PART4(s4, s5, t4, t5)
    let t4 = s5 + ((s4 * 4 - s5 + 4) >> 3);
    let t5 = s4 + ((-s4 - s5 * 4 + 4) >> 3);
    let (t1, t5) = (s1 + t5, s1 - t5);
    let (t2, t6) = (s2 + s6, s2 - s6);
    let (t7, t3) = (s7 + s3, s7 - s3);
    let (t4, t8) = (t4 + s8, t4 - s8);
    let (t1, t2) = (t1 + t2, t1 - t2);
    let (t4, t3) = ireflect(t4, t3);
    let (t5, t6) = (t5 + t6, t5 - t6);
    let (t8, t7) = ireflect(t8, t7);
    let (t1, t4) = (t1 + t4, t1 - t4);
    let (t2, t3) = (t2 + t3, t2 - t3);
    let (t5, t8) = (t5 + t8, t5 - t8);
    let (t6, t7) = (t6 + t7, t6 - t7);
    [t1, t2, t3, t4, t5, t6, t7, t8]
}

/// `IVI_IREFLECT`: a, b = 1/2, 5/4.
fn ireflect(s1: i32, s2: i32) -> (i32, i32) {
    let o1 = ((s1 + s2 * 2 + 2) >> 2) + s1;
    let o2 = ((s1 * 2 - s2 + 2) >> 2) - s2;
    (o1, o2)
}

/// `IVI_INV_SLANT4`, before compensation.
fn slant4(i: [i32; 4]) -> [i32; 4] {
    let (s1, s4, s2, s3) = (i[0], i[1], i[2], i[3]);
    let (t1, t2) = (s1 + s2, s1 - s2);
    let (t4, t3) = ireflect(s4, s3);
    let (t1, t4) = (t1 + t4, t1 - t4);
    let (t2, t3) = (t2 + t3, t2 - t3);
    [t1, t2, t3, t4]
}

fn half(x: i32) -> i16 {
    ((x + 1) >> 1) as i16
}

fn inverse_slant_8x8(
    input: &[i32; 64],
    out: &mut [i16],
    offs: usize,
    pitch: usize,
    flags: &[u8; 8],
) {
    let mut tmp = [0i32; 64];
    for i in 0..8 {
        if flags[i] != 0 {
            let r = slant8(std::array::from_fn(|k| input[i + 8 * k]));
            for k in 0..8 {
                tmp[i + 8 * k] = r[k];
            }
        }
    }
    for i in 0..8 {
        let row = &mut out[offs + i * pitch..][..8];
        let src: [i32; 8] = std::array::from_fn(|k| tmp[i * 8 + k]);
        if src.iter().all(|&v| v == 0) {
            row.fill(0);
        } else {
            let r = slant8(src);
            for k in 0..8 {
                row[k] = half(r[k]);
            }
        }
    }
}

fn inverse_slant_4x4(
    input: &[i32; 64],
    out: &mut [i16],
    offs: usize,
    pitch: usize,
    flags: &[u8; 8],
) {
    let mut tmp = [0i32; 16];
    for i in 0..4 {
        if flags[i] != 0 {
            let r = slant4(std::array::from_fn(|k| input[i + 4 * k]));
            for k in 0..4 {
                tmp[i + 4 * k] = r[k];
            }
        }
    }
    for i in 0..4 {
        let row = &mut out[offs + i * pitch..][..4];
        let src: [i32; 4] = std::array::from_fn(|k| tmp[i * 4 + k]);
        if src.iter().all(|&v| v == 0) {
            row.fill(0);
        } else {
            let r = slant4(src);
            for k in 0..4 {
                row[k] = half(r[k]);
            }
        }
    }
}

fn row_slant8(input: &[i32; 64], out: &mut [i16], offs: usize, pitch: usize) {
    for i in 0..8 {
        let row = &mut out[offs + i * pitch..][..8];
        let src: [i32; 8] = std::array::from_fn(|k| input[i * 8 + k]);
        if src.iter().all(|&v| v == 0) {
            row.fill(0);
        } else {
            let r = slant8(src);
            for k in 0..8 {
                row[k] = half(r[k]);
            }
        }
    }
}

fn col_slant8(input: &[i32; 64], out: &mut [i16], offs: usize, pitch: usize, flags: &[u8; 8]) {
    for i in 0..8 {
        let r = if flags[i] != 0 {
            slant8(std::array::from_fn(|k| input[i + 8 * k])).map(half)
        } else {
            [0; 8]
        };
        for k in 0..8 {
            out[offs + i + k * pitch] = r[k];
        }
    }
}

fn put_pixels_8x8(input: &[i32; 64], out: &mut [i16], offs: usize, pitch: usize) {
    for y in 0..8 {
        for x in 0..8 {
            out[offs + y * pitch + x] = input[y * 8 + x] as i16;
        }
    }
}

/// Motion compensation of a `size` square block (`ivi_mc_NxN_delta` and
/// `_no_delta`): from `reference` at `ref_offs` into `buf` at `offs`,
/// interpolated at half pixels by `mc_type`; added to what the block holds
/// when `add` (the coded difference is already there).
#[allow(clippy::too_many_arguments)]
pub fn mc(
    buf: &mut [i16],
    offs: usize,
    reference: &[i16],
    ref_offs: usize,
    pitch: usize,
    size: usize,
    mc_type: u32,
    add: bool,
) {
    for i in 0..size {
        let r = ref_offs + i * pitch;
        let w = r + pitch;
        for j in 0..size {
            let v = match mc_type {
                0 => reference[r + j],
                1 => ((reference[r + j] as i32 + reference[r + j + 1] as i32) >> 1) as i16,
                2 => ((reference[r + j] as i32 + reference[w + j] as i32) >> 1) as i16,
                _ => {
                    ((reference[r + j] as i32
                        + reference[r + j + 1] as i32
                        + reference[w + j] as i32
                        + reference[w + j + 1] as i32)
                        >> 2) as i16
                }
            };
            let d = &mut buf[offs + i * pitch + j];
            *d = if add { d.wrapping_add(v) } else { v };
        }
    }
}
