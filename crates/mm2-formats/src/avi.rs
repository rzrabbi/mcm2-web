//! AVI movies (`UI/INTRO.AVI`, `CREDITS.AVI`, `EVENTS.AVI`): the RIFF
//! layout, one video stream and at most one audio stream, and the
//! Microsoft ADPCM sound the game's movies carry.
//!
//! The video frames stay coded; their codec (Indeo 5 in the game) is
//! decoded elsewhere.

use crate::{Error, Result};

/// A movie: its streams' formats and every chunk of each, in order.
#[derive(Debug, Clone)]
pub struct Avi<'a> {
    pub width: u32,
    pub height: u32,
    /// Frames per second of the video stream.
    pub fps: f64,
    /// The video codec's four characters, e.g. `IV50`.
    pub codec: [u8; 4],
    /// Each video frame's data; empty for a dropped frame (the picture
    /// stays).
    pub frames: Vec<&'a [u8]>,
    pub audio: Option<Audio<'a>>,
}

/// The sound stream.
#[derive(Debug, Clone)]
pub struct Audio<'a> {
    /// `WAVE_FORMAT_PCM` (1) or `WAVE_FORMAT_ADPCM` (2).
    pub format: u16,
    pub channels: u16,
    pub rate: u32,
    pub bits: u16,
    pub block_align: u16,
    /// The ADPCM coefficient pairs, from the format's extra data.
    pub coefs: Vec<(i16, i16)>,
    pub chunks: Vec<&'a [u8]>,
}

fn u16_at(b: &[u8], o: usize) -> Result<u16> {
    b.get(o..o + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or(Error::Corrupt("AVI cut short"))
}

fn u32_at(b: &[u8], o: usize) -> Result<u32> {
    b.get(o..o + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or(Error::Corrupt("AVI cut short"))
}

/// A RIFF chunk: tag, list type (for lists), body.
type Chunk<'a> = ([u8; 4], Option<[u8; 4]>, &'a [u8]);

/// The chunks inside a RIFF list body.
fn chunks(body: &[u8]) -> Vec<Chunk<'_>> {
    let mut out = Vec::new();
    let mut o = 0;
    while o + 8 <= body.len() {
        let tag: [u8; 4] = body[o..o + 4].try_into().unwrap();
        let n = u32::from_le_bytes(body[o + 4..o + 8].try_into().unwrap()) as usize;
        let end = (o + 8 + n).min(body.len());
        let data = &body[o + 8..end];
        if &tag == b"LIST" || &tag == b"RIFF" {
            if data.len() >= 4 {
                out.push((tag, Some(data[..4].try_into().unwrap()), &data[4..]));
            }
        } else {
            out.push((tag, None, data));
        }
        o += 8 + n + (n & 1);
    }
    out
}

pub fn parse(bytes: &[u8]) -> Result<Avi<'_>> {
    let top = chunks(bytes);
    let Some((_, Some(kind), riff)) = top.first().copied() else {
        return Err(Error::Corrupt("not a RIFF file"));
    };
    if &kind != b"AVI " {
        return Err(Error::Corrupt("not an AVI"));
    }
    let mut avi = Avi {
        width: 0,
        height: 0,
        fps: 15.0,
        codec: [0; 4],
        frames: Vec::new(),
        audio: None,
    };
    // Which stream number is the video and which the sound.
    let mut video = None;
    let mut sound = None;
    for (tag, kind, body) in chunks(riff) {
        match (&tag, kind.as_ref()) {
            (b"LIST", Some(b"hdrl")) => {
                let mut stream = 0u32;
                for (tag, kind, body) in chunks(body) {
                    match (&tag, kind.as_ref()) {
                        (b"avih", None) => {
                            avi.width = u32_at(body, 32)?;
                            avi.height = u32_at(body, 36)?;
                        }
                        (b"LIST", Some(b"strl")) => {
                            let parts = chunks(body);
                            let strh = parts.iter().find(|p| &p.0 == b"strh").map(|p| p.2);
                            let strf = parts.iter().find(|p| &p.0 == b"strf").map(|p| p.2);
                            let (Some(strh), Some(strf)) = (strh, strf) else {
                                continue;
                            };
                            match &strh[..4] {
                                b"vids" if video.is_none() => {
                                    video = Some(stream);
                                    let scale = u32_at(strh, 20)?;
                                    let rate = u32_at(strh, 24)?;
                                    if scale > 0 && rate > 0 {
                                        avi.fps = rate as f64 / scale as f64;
                                    }
                                    avi.codec = strf
                                        .get(16..20)
                                        .ok_or(Error::Corrupt("AVI video format"))?
                                        .try_into()
                                        .unwrap();
                                    avi.width = u32_at(strf, 4)?;
                                    avi.height = u32_at(strf, 8)?;
                                }
                                b"auds" if sound.is_none() => {
                                    sound = Some(stream);
                                    let mut coefs = Vec::new();
                                    let format = u16_at(strf, 0)?;
                                    if format == 2 {
                                        // cbSize, samples per block, count, pairs.
                                        let n = u16_at(strf, 20)? as usize;
                                        for i in 0..n {
                                            coefs.push((
                                                u16_at(strf, 22 + i * 4)? as i16,
                                                u16_at(strf, 24 + i * 4)? as i16,
                                            ));
                                        }
                                    }
                                    avi.audio = Some(Audio {
                                        format,
                                        channels: u16_at(strf, 2)?,
                                        rate: u32_at(strf, 4)?,
                                        block_align: u16_at(strf, 12)?,
                                        bits: u16_at(strf, 14)?,
                                        coefs,
                                        chunks: Vec::new(),
                                    });
                                }
                                _ => {}
                            }
                            stream += 1;
                        }
                        _ => {}
                    }
                }
            }
            (b"LIST", Some(b"movi")) => collect(body, video, sound, &mut avi),
            _ => {}
        }
    }
    if video.is_none() {
        return Err(Error::Corrupt("AVI without video"));
    }
    Ok(avi)
}

/// The stream chunks of the movie list, in `rec ` lists or not.
fn collect<'a>(body: &'a [u8], video: Option<u32>, sound: Option<u32>, avi: &mut Avi<'a>) {
    for (tag, kind, data) in chunks(body) {
        if kind.is_some() {
            collect(data, video, sound, avi);
            continue;
        }
        let num = std::str::from_utf8(&tag[..2])
            .ok()
            .and_then(|s| s.parse::<u32>().ok());
        match (num, &tag[2..]) {
            // Usually `dc`, but EVENTS.AVI names its frames `00IV`.
            (Some(n), kind) if Some(n) == video && kind != b"pc" => avi.frames.push(data),
            (Some(n), b"wb") if Some(n) == sound => {
                if let Some(a) = &mut avi.audio {
                    a.chunks.push(data);
                }
            }
            _ => {}
        }
    }
}

impl Audio<'_> {
    /// All the sound as interleaved 16-bit samples.
    pub fn samples(&self) -> Vec<i16> {
        // ADPCM blocks run across chunks, so the stream goes together first.
        let data: Vec<u8> = self.chunks.concat();
        match self.format {
            1 if self.bits == 16 => data
                .chunks_exact(2)
                .map(|s| i16::from_le_bytes([s[0], s[1]]))
                .collect(),
            1 => data.iter().map(|&b| (b as i16 - 128) << 8).collect(),
            2 => {
                let mut out = Vec::new();
                let align = self.block_align.max(1) as usize;
                for block in data.chunks(align) {
                    decode_ms_adpcm(block, self.channels as usize, &self.coefs, &mut out);
                }
                out
            }
            _ => Vec::new(),
        }
    }
}

/// How the step size follows each nibble (Microsoft ADPCM).
const ADAPT: [i32; 16] = [
    230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230,
];

/// One Microsoft ADPCM block: per channel a predictor, a step and two
/// samples, then a nibble per sample, channels interleaved.
fn decode_ms_adpcm(block: &[u8], channels: usize, coefs: &[(i16, i16)], out: &mut Vec<i16>) {
    let ch = channels.clamp(1, 2);
    if block.len() < 7 * ch || coefs.is_empty() {
        return;
    }
    let mut pred = [(0i32, 0i32); 2];
    let mut delta = [0i32; 2];
    let mut s1 = [0i32; 2];
    let mut s2 = [0i32; 2];
    let mut o = 0;
    for pr in pred.iter_mut().take(ch) {
        let p = (block[o] as usize).min(coefs.len() - 1);
        *pr = (coefs[p].0 as i32, coefs[p].1 as i32);
        o += 1;
    }
    let word = |o: usize| i16::from_le_bytes([block[o], block[o + 1]]) as i32;
    for d in delta.iter_mut().take(ch) {
        *d = word(o);
        o += 2;
    }
    for s in s1.iter_mut().take(ch) {
        *s = word(o);
        o += 2;
    }
    for s in s2.iter_mut().take(ch) {
        *s = word(o);
        o += 2;
    }
    // The header's samples come out oldest first.
    for &s in s2.iter().take(ch) {
        out.push(s as i16);
    }
    for &s in s1.iter().take(ch) {
        out.push(s as i16);
    }
    let mut c = 0;
    for &b in &block[o..] {
        for nib in [b >> 4, b & 15] {
            let signed = if nib & 8 != 0 {
                nib as i32 - 16
            } else {
                nib as i32
            };
            // Rounds toward zero, as FFmpeg does.
            let predicted = (s1[c] * pred[c].0 + s2[c] * pred[c].1) / 256;
            let sample = (predicted + signed * delta[c]).clamp(-32768, 32767);
            s2[c] = s1[c];
            s1[c] = sample;
            delta[c] = ((ADAPT[nib as usize] * delta[c]) >> 8).max(16);
            out.push(sample as i16);
            c = (c + 1) % ch;
        }
    }
}
