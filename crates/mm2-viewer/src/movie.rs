//! The game's movies (`UI/*.AVI`, Indeo 5 with Microsoft ADPCM sound),
//! played as the game's video control did (`0x47bd80`, `vfwdeco.cpp`):
//! frames at the stream's rate from when it starts, the sound alongside,
//! and for a looping movie a fresh start when it ends (`0x44c190`).

use crate::audio::{Audio, Voice};
use crate::sprites::TextureId;
use crate::ui::Gfx;
use mm2_formats::wav::Sound;
use std::path::Path;
use std::sync::Arc;

pub struct Movie {
    frames: Vec<Vec<u8>>,
    fps: f64,
    width: usize,
    height: usize,
    decoder: mm2_indeo::Decoder,
    /// The next frame to decode.
    next: usize,
    /// The newest picture, waiting to go to the texture.
    fresh: Option<Vec<u8>>,
    texture: Option<TextureId>,
    sound: Option<Arc<Sound>>,
    voice: Option<Voice>,
    /// The mixer it plays its sound on, to stop it however it ends.
    audio: Option<Audio>,
    start: Option<f32>,
    looping: bool,
}

impl Movie {
    pub fn open(path: &Path, looping: bool) -> Option<Movie> {
        let bytes = crate::platform::fs::read(path).ok()?;
        let avi = mm2_formats::avi::parse(&bytes).ok()?;
        if &avi.codec[..] != b"IV50" && &avi.codec[..] != b"iv50" {
            return None;
        }
        let sound = avi.audio.as_ref().map(|a| {
            Arc::new(Sound {
                rate: a.rate,
                channels: a.channels,
                samples: a.samples().iter().map(|&s| s as f32 / 32768.0).collect(),
            })
        });
        let (width, height) = (avi.width as usize, avi.height as usize);
        Some(Movie {
            frames: avi.frames.iter().map(|f| f.to_vec()).collect(),
            fps: avi.fps,
            width,
            height,
            decoder: mm2_indeo::Decoder::new(width, height),
            next: 0,
            fresh: None,
            texture: None,
            sound,
            voice: None,
            audio: None,
            start: None,
            looping,
        })
    }

    /// A movie with nothing in it, for one that can't be read.
    pub fn none() -> Movie {
        Movie {
            frames: Vec::new(),
            fps: 15.0,
            width: 1,
            height: 1,
            decoder: mm2_indeo::Decoder::new(1, 1),
            next: 0,
            fresh: None,
            texture: None,
            sound: None,
            voice: None,
            audio: None,
            start: None,
            looping: false,
        }
    }

    /// How long it runs, in seconds.
    pub fn length(&self) -> f32 {
        (self.frames.len() as f64 / self.fps.max(1.0)) as f32
    }

    pub fn playing(&self) -> bool {
        self.start.is_some()
    }

    /// Starts it from the first frame.
    pub fn play(&mut self, audio: Option<&Audio>, now: f32) {
        self.stop();
        self.decoder = mm2_indeo::Decoder::new(self.width, self.height);
        self.next = 0;
        self.start = Some(now);
        if let (Some(a), Some(s)) = (audio, &self.sound) {
            self.voice = Some(a.play(s, 1.0, false));
            self.audio = Some(a.share());
        }
    }

    pub fn stop(&mut self) {
        if let (Some(a), Some(v)) = (&self.audio, self.voice.take()) {
            a.stop(v);
        }
        self.start = None;
    }

    /// Whether a movie that doesn't loop has played to its end.
    pub fn done(&self, now: f32) -> bool {
        !self.looping && self.start.is_some_and(|s| now - s >= self.length())
    }

    /// Keeps a looping movie going round.
    pub fn tick(&mut self, audio: Option<&Audio>, now: f32) {
        if self.looping && self.start.is_some_and(|s| now - s >= self.length()) {
            self.play(audio, now);
        }
    }

    /// Decodes up to the frame for `now`, then draws the picture into
    /// `rect` (x, y, width, height).
    pub fn draw(&mut self, gfx: &mut Gfx, now: f32, rect: [f32; 4]) {
        let Some(start) = self.start else {
            return;
        };
        let want = (((now - start).max(0.0) as f64) * self.fps) as usize;
        let want = want.min(self.frames.len().saturating_sub(1));
        while self.next <= want && self.next < self.frames.len() {
            let data = &self.frames[self.next];
            self.next += 1;
            // An empty chunk is a dropped frame: the picture stays.
            if data.is_empty() {
                continue;
            }
            match self.decoder.decode(data) {
                Ok(Some(f)) => self.fresh = Some(f.rgba()),
                Ok(None) => {}
                Err(e) => eprintln!("movie frame {}: {e}", self.next - 1),
            }
        }
        if let Some(rgba) = self.fresh.take() {
            match self.texture {
                Some(t) if gfx.sprites.size(t) == (self.width as u32, self.height as u32) => {
                    gfx.sprites.update(gfx.queue, t, &rgba)
                }
                _ => {
                    let image = mm2_formats::tga::Image {
                        width: self.width as u32,
                        height: self.height as u32,
                        rgba,
                    };
                    self.texture = Some(gfx.sprites.upload(gfx.device, gfx.queue, &image));
                }
            }
        }
        if let Some(t) = self.texture {
            // A movie over the whole screen fills a wide window.
            if rect == [0.0, 0.0, 640.0, 480.0] {
                gfx.sprites.stretched(t, [0.0, 0.0, 1.0, 1.0], rect);
            } else {
                gfx.sprites.quad(t, [0.0, 0.0, 1.0, 1.0], rect, [1.0; 4]);
            }
        }
    }
}

impl Drop for Movie {
    fn drop(&mut self) {
        self.stop();
    }
}
