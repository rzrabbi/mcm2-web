//! Sound: a small software mixer in the style of the game's DirectSound
//! buffers. Each voice plays one sound at a frequency, volume and pan the
//! game sets, looping or once.
//!
//! Without the `audio` feature (or without an output device) everything
//! here is silent but keeps working, so the game runs the same.

use mm2_formats::wav::Sound;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// A playing sound, by number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Voice(u64);

/// Frames over which a voice fades in, out, or crossfades to a new sound,
/// so starting, stopping or rewriting a stream never clicks.
const DECLICK: f32 = 256.0;

/// The sound a voice was playing when `switch` rewrote it, fading out.
struct Ghost {
    sound: Arc<Sound>,
    pos: f64,
    frequency: f32,
    left: f32,
}

struct Playing {
    id: u64,
    sound: Arc<Sound>,
    /// Position in source frames.
    pos: f64,
    /// Source frames a second (the sound's own rate, or a new frequency).
    frequency: f32,
    /// The Doppler shift on top of the frequency (DirectSound 3D's).
    pitch: f32,
    volume: f32,
    /// -1 left to 1 right.
    pan: f32,
    /// Where volume, pan and pitch are heading: the game sets them once a
    /// frame, and they glide there over each block so nothing steps.
    target: (f32, f32, f32),
    /// `place` has set them once; the first time they take effect at once.
    placed: bool,
    looping: bool,
    /// 0 to 1, fading in at the start.
    fade: f32,
    /// Fading out before the voice is dropped.
    stopping: bool,
    ghost: Option<Ghost>,
    /// What plays on, in turn and without a gap, when this sound ends.
    then: VecDeque<(Arc<Sound>, bool)>,
}

impl Playing {
    fn new(id: u64, sound: Arc<Sound>, pos: f64, volume: f32, looping: bool) -> Playing {
        Playing {
            id,
            frequency: sound.rate as f32,
            sound,
            pos,
            pitch: 1.0,
            volume,
            pan: 0.0,
            target: (volume, 0.0, 1.0),
            placed: false,
            looping,
            fade: 0.0,
            stopping: false,
            ghost: None,
            then: VecDeque::new(),
        }
    }
}

/// The frame `i` of `sound` for the interpolator: past the end it wraps
/// round a looping sound and holds the last frame of a one-shot; before the
/// start it holds the first.
fn frame(sound: &Sound, i: i64, looping: bool, ch: usize) -> (f32, f32) {
    let n = sound.frames() as i64;
    let i = if looping {
        i.rem_euclid(n)
    } else {
        i.clamp(0, n - 1)
    } as usize;
    let s = &sound.samples[i * ch..];
    if ch == 1 { (s[0], s[0]) } else { (s[0], s[1]) }
}

/// The sound at fractional frame `pos`, by Catmull-Rom cubic through the
/// four frames around it: far cleaner than taking the nearest frame, which
/// rasps whenever the pitch is shifted or the device's rate differs from the
/// sample's.
fn sample(sound: &Sound, pos: f64, looping: bool) -> (f32, f32) {
    let ch = sound.channels as usize;
    let i = pos.floor();
    let t = (pos - i) as f32;
    let i = i as i64;
    let at = |k: i64| frame(sound, i + k, looping, ch);
    if t == 0.0 {
        return at(0);
    }
    let (a, b, c, d) = (at(-1), at(0), at(1), at(2));
    let cubic = |p0: f32, p1: f32, p2: f32, p3: f32| {
        p1 + 0.5
            * t
            * (p2 - p0 + t * (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3 + t * (3.0 * (p1 - p2) + p3 - p0)))
    };
    (cubic(a.0, b.0, c.0, d.0), cubic(a.1, b.1, c.1, d.1))
}

/// The loudest the mix may be before it is turned down, and where its soft
/// clip starts.
const CEILING: f32 = 0.9;
const KNEE: f32 = 0.75;

/// A smooth ceiling: passes anything under `KNEE` untouched and rounds
/// what is over it towards 1, so a peak is never a hard flat top.
fn soft_clip(x: f32) -> f32 {
    let a = x.abs();
    if a <= KNEE {
        x
    } else {
        let room = 1.0 - KNEE;
        x.signum() * (KNEE + room * ((a - KNEE) / room).tanh())
    }
}

#[derive(Default)]
struct Mixer {
    voices: Vec<Playing>,
    next: u64,
    rate: f32,
    /// The EAX reverb on the whole mix, while a race asks for one.
    reverb: Option<crate::reverb::Tail>,
    /// The limiter's gain (1 when the mix is quiet enough), and the last
    /// frames of the mix it holds back, with the gain each one needs.
    limit: f32,
    ahead: VecDeque<(f32, f32, f32)>,
}

impl Mixer {
    /// Adds the voices into `out`, interleaved stereo.
    fn mix(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let rate = self.rate.max(1.0) as f64;
        let block = (out.len() / 2).max(1) as f32;
        self.voices.retain_mut(|v| {
            if v.sound.frames() == 0 {
                return false;
            }
            if !v.placed {
                v.volume = v.target.0;
            }
            // Glide volume, pan and pitch to where the game last put them.
            let (dv, dp, dk) = (
                (v.target.0 - v.volume) / block,
                (v.target.1 - v.pan) / block,
                (v.target.2 - v.pitch) / block,
            );
            for frame_out in out.chunks_exact_mut(2) {
                if v.pos >= v.sound.frames() as f64 {
                    if let Some((next, looping)) = v.then.pop_front() {
                        v.pos -= v.sound.frames() as f64;
                        v.sound = next;
                        v.looping = looping;
                        v.frequency = v.sound.rate as f32;
                        if v.sound.frames() == 0 {
                            return false;
                        }
                    } else if !v.looping {
                        return false;
                    } else {
                        v.pos %= v.sound.frames() as f64;
                    }
                }
                v.volume += dv;
                v.pan += dp;
                v.pitch += dk;
                v.fade = if v.stopping {
                    v.fade - 1.0 / DECLICK
                } else {
                    (v.fade + 1.0 / DECLICK).min(1.0)
                };
                if v.fade <= 0.0 {
                    return false;
                }
                let (l, r) = sample(&v.sound, v.pos, v.looping);
                let gain = v.volume * v.fade;
                let (left, right) = (1.0 - v.pan.max(0.0), 1.0 + v.pan.min(0.0));
                frame_out[0] += l * gain * left;
                frame_out[1] += r * gain * right;
                v.pos += (v.frequency * v.pitch) as f64 / rate;
                if let Some(g) = &mut v.ghost {
                    g.left -= 1.0 / DECLICK;
                    if g.left <= 0.0 {
                        v.ghost = None;
                    } else {
                        let (l, r) = sample(&g.sound, g.pos, false);
                        let gain = v.volume * g.left;
                        frame_out[0] += l * gain * left;
                        frame_out[1] += r * gain * right;
                        g.pos += (g.frequency * v.pitch) as f64 / rate;
                    }
                }
            }
            true
        });
        if let Some(tail) = &mut self.reverb {
            tail.apply(out);
        }
        // Several engines at once add up past full scale. The mix is held
        // back a millisecond so the limiter can see a peak coming and ease
        // the gain down before it arrives, lets the gain back up slowly,
        // and rounds whatever still pokes out.
        let ahead = (self.rate * 0.0013) as usize;
        if self.limit == 0.0 {
            self.limit = 1.0;
        }
        if self.ahead.len() != ahead {
            self.ahead = VecDeque::from(vec![(0.0, 0.0, 1.0); ahead]);
        }
        let release = 1.0 / (0.2 * self.rate.max(1.0));
        for frame_out in out.chunks_exact_mut(2) {
            let peak = frame_out[0].abs().max(frame_out[1].abs());
            let need = if peak > CEILING { CEILING / peak } else { 1.0 };
            self.ahead.push_back((frame_out[0], frame_out[1], need));
            let (l, r, _) = self.ahead.pop_front().unwrap();
            let target = self.ahead.iter().fold(1.0f32, |m, a| m.min(a.2));
            if target < self.limit {
                self.limit += (target - self.limit) * 0.1;
            } else {
                self.limit = (self.limit + release).min(target);
            }
            frame_out[0] = soft_clip(l * self.limit).clamp(-1.0, 1.0);
            frame_out[1] = soft_clip(r * self.limit).clamp(-1.0, 1.0);
        }
    }
}

pub struct Audio {
    mixer: Arc<Mutex<Mixer>>,
    /// Keeps the output stream open.
    #[cfg(feature = "audio")]
    _stream: Option<cpal::Stream>,
}

impl Audio {
    /// Opens the default output device, or a silent mixer without one.
    pub fn new() -> Self {
        let mixer = Arc::new(Mutex::new(Mixer {
            rate: 44100.0,
            ..Default::default()
        }));
        #[cfg(feature = "audio")]
        {
            let stream = open(mixer.clone())
                .map_err(|e| warn(&format!("no sound: {e}")))
                .ok();
            Audio {
                mixer,
                _stream: stream,
            }
        }
        #[cfg(not(feature = "audio"))]
        Audio { mixer }
    }

    /// A mixer with no output device, read back with `render`.
    pub fn offline(rate: f32) -> Self {
        Audio {
            mixer: Arc::new(Mutex::new(Mixer {
                rate,
                ..Default::default()
            })),
            #[cfg(feature = "audio")]
            _stream: None,
        }
    }

    /// Mixes the next `frames` frames, interleaved stereo.
    pub fn render(&self, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        self.mixer.lock().unwrap().mix(&mut out);
        out
    }

    /// Another handle on the same mixer, for whatever must start or stop
    /// sounds where the first isn't to hand (the menus' movies).
    pub fn share(&self) -> Audio {
        Audio {
            mixer: self.mixer.clone(),
            #[cfg(feature = "audio")]
            _stream: None,
        }
    }

    /// Plays `sound` at its own rate.
    pub fn play(&self, sound: &Arc<Sound>, volume: f32, looping: bool) -> Voice {
        self.play_from(sound, 0.0, volume, looping)
    }

    /// Plays `sound` from `start` seconds in.
    pub fn play_from(&self, sound: &Arc<Sound>, start: f32, volume: f32, looping: bool) -> Voice {
        let mut m = self.mixer.lock().unwrap();
        m.next += 1;
        let id = m.next;
        m.voices.push(Playing::new(
            id,
            sound.clone(),
            (start.max(0.0) * sound.rate as f32) as f64,
            volume,
            looping,
        ));
        Voice(id)
    }

    /// Plays `sound` on `voice` from its start straight away, as the game
    /// rewrites a stream buffer from the play cursor on.
    pub fn switch(&self, voice: Voice, sound: &Arc<Sound>, looping: bool) {
        let mut m = self.mixer.lock().unwrap();
        if let Some(v) = m.voices.iter_mut().find(|v| v.id == voice.0) {
            let old = std::mem::replace(&mut v.sound, sound.clone());
            v.ghost = Some(Ghost {
                sound: old,
                pos: v.pos,
                frequency: v.frequency,
                left: 1.0,
            });
            v.pos = 0.0;
            v.frequency = sound.rate as f32;
            v.looping = looping;
            v.then.clear();
        }
    }

    /// Queues `sound` to follow on `voice` after what it plays and has
    /// queued.
    pub fn then(&self, voice: Voice, sound: &Arc<Sound>, looping: bool) {
        let mut m = self.mixer.lock().unwrap();
        if let Some(v) = m.voices.iter_mut().find(|v| v.id == voice.0) {
            v.then.push_back((sound.clone(), looping));
        }
    }

    /// Places a playing voice as DirectSound 3D hears it: its volume, pan
    /// and Doppler shift.
    pub fn place(&self, voice: Voice, volume: f32, pan: f32, pitch: f32) {
        let mut m = self.mixer.lock().unwrap();
        if let Some(v) = m.voices.iter_mut().find(|v| v.id == voice.0) {
            v.target = (volume.max(0.0), pan.clamp(-1.0, 1.0), pitch.clamp(0.25, 4.0));
            if !v.placed {
                v.volume = v.target.0;
                v.pan = v.target.1;
                v.pitch = v.target.2;
                v.placed = true;
            }
        }
    }

    /// Whether a voice is still playing.
    pub fn playing(&self, voice: Voice) -> bool {
        self.mixer.lock().unwrap().voices.iter().any(|v| v.id == voice.0)
    }

    pub fn stop(&self, voice: Voice) {
        if let Some(v) = self.mixer.lock().unwrap().voices.iter_mut().find(|v| v.id == voice.0) {
            v.stopping = true;
        }
    }

    /// Sets the listener's EAX environment, or none.
    pub fn set_reverb(&self, reverb: Option<crate::reverb::Reverb>) {
        let mut m = self.mixer.lock().unwrap();
        if m.reverb.as_ref().map(|t| t.reverb) != reverb {
            let rate = m.rate;
            m.reverb = reverb.map(|r| crate::reverb::Tail::new(r, rate));
        }
    }

    pub fn stop_all(&self) {
        self.mixer.lock().unwrap().voices.clear();
    }
}

impl Default for Audio {
    fn default() -> Self {
        Audio::new()
    }
}

/// Reports a problem with the sound: on stderr, or in the browser's console.
pub fn warn(msg: &str) {
    #[cfg(target_arch = "wasm32")]
    web_sys::console::warn_1(&msg.into());
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("{msg}");
}

/// The sound device's own sample rate, which the page noted before it
/// wrapped `AudioContext` (`window.mm2NativeRate`).
#[cfg(all(feature = "audio", target_arch = "wasm32"))]
fn native_rate() -> Option<u32> {
    let rate = js_sys::Reflect::get(&js_sys::global(), &"mm2NativeRate".into())
        .ok()?
        .as_f64()?;
    (8000.0..=96000.0).contains(&rate).then_some(rate as u32)
}

#[cfg(feature = "audio")]
fn open(mixer: Arc<Mutex<Mixer>>) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("no output device")?;
    let config = device.default_output_config()?;
    let format = config.sample_format();
    #[cfg_attr(not(target_arch = "wasm32"), allow(unused_mut))]
    let mut config: cpal::StreamConfig = config.into();
    // The browser's backend asks for 44.1 kHz whatever the device runs at,
    // and the browser then resamples every sample of the mix. Open the
    // context at the device's own rate (the page probes it) so the mixer's
    // output goes straight out.
    #[cfg(target_arch = "wasm32")]
    if let Some(rate) = native_rate() {
        config.sample_rate = cpal::SampleRate(rate);
    }
    let channels = config.channels as usize;
    mixer.lock().unwrap().rate = config.sample_rate.0 as f32;
    let mut stereo = Vec::new();
    let err = |e| warn(&format!("sound: {e}"));
    let stream = match format {
        cpal::SampleFormat::F32 => device.build_output_stream(
            &config,
            move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let frames = out.len() / channels;
                stereo.resize(frames * 2, 0.0);
                mixer.lock().unwrap().mix(&mut stereo);
                for (o, s) in out.chunks_exact_mut(channels).zip(stereo.chunks_exact(2)) {
                    for (c, v) in o.iter_mut().enumerate() {
                        *v = s[c.min(1)];
                    }
                }
            },
            err,
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_output_stream(
            &config,
            move |out: &mut [i16], _: &cpal::OutputCallbackInfo| {
                let frames = out.len() / channels;
                stereo.resize(frames * 2, 0.0);
                mixer.lock().unwrap().mix(&mut stereo);
                for (o, s) in out.chunks_exact_mut(channels).zip(stereo.chunks_exact(2)) {
                    for (c, v) in o.iter_mut().enumerate() {
                        *v = (s[c.min(1)] * 32767.0) as i16;
                    }
                }
            },
            err,
            None,
        )?,
        other => return Err(format!("unsupported sample format {other:?}").into()),
    };
    stream.play()?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A voice already faded in, so the numbers are the sound's own.
    fn voice(sound: Arc<Sound>, frequency: f32) -> Playing {
        let mut v = Playing::new(1, sound, 0.0, 1.0, false);
        v.frequency = frequency;
        v.fade = 1.0;
        v
    }

    #[test]
    fn mixes_at_the_set_frequency() {
        let sound = Arc::new(Sound {
            rate: 100,
            channels: 1,
            samples: vec![0.5, 0.25],
        });
        let mut m = Mixer {
            rate: 100.0,
            ..Default::default()
        };
        m.voices.push(voice(sound, 100.0));
        let mut out = [0.0; 6];
        m.mix(&mut out);
        assert_eq!(out, [0.5, 0.5, 0.25, 0.25, 0.0, 0.0]);
        assert!(m.voices.is_empty());
    }

    #[test]
    fn plays_on_into_the_next_sound() {
        let one = |x| Arc::new(Sound { rate: 100, channels: 1, samples: vec![x] });
        let mut m = Mixer { rate: 100.0, ..Default::default() };
        let mut v = voice(one(0.5), 100.0);
        v.then = VecDeque::from([(one(0.25), true)]);
        m.voices.push(v);
        let mut out = [0.0; 6];
        m.mix(&mut out);
        assert_eq!(out, [0.5, 0.5, 0.25, 0.25, 0.25, 0.25]);
    }

    #[test]
    fn a_sine_resampled_stays_clean() {
        // A 1 kHz sine at 22.05 kHz, played at 48 kHz: what comes out must
        // be that sine, not a staircase of it.
        let n = 22050;
        let samples: Vec<f32> = (0..n)
            .map(|i| 0.5 * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 22050.0).sin())
            .collect();
        let sound = Arc::new(Sound { rate: 22050, channels: 1, samples });
        let mut m = Mixer { rate: 48000.0, ..Default::default() };
        m.voices.push(voice(sound, 22050.0));
        let mut out = vec![0.0; 2 * 4800];
        m.mix(&mut out);
        // The limiter holds the mix back by about a millisecond.
        let held = (48000.0 * 0.0013) as usize;
        let worst = (200..4700)
            .map(|k| {
                let t = (k - held) as f32;
                let want = 0.5 * (2.0 * std::f32::consts::PI * 1000.0 * t / 48000.0).sin();
                (out[2 * k] - want).abs()
            })
            .fold(0.0, f32::max);
        assert!(worst < 0.002, "error {worst}");
    }

    #[test]
    fn many_loud_voices_never_clip() {
        let sound = Arc::new(Sound {
            rate: 48000,
            channels: 1,
            samples: vec![0.9; 48000],
        });
        let mut m = Mixer { rate: 48000.0, ..Default::default() };
        for id in 0..6 {
            let mut v = voice(sound.clone(), 48000.0);
            v.id = id;
            m.voices.push(v);
        }
        let mut out = vec![0.0; 2 * 4800];
        m.mix(&mut out);
        assert!(out.iter().all(|s| s.abs() < 0.95), "peak {}", out.iter().fold(0.0f32, |m, s| m.max(s.abs())));
        // Once the limiter has caught the peak the mix sits at its ceiling.
        assert!(out[2 * 4000] > 0.8, "{}", out[2 * 4000]);
    }

    #[test]
    fn a_stopped_voice_fades_instead_of_cutting() {
        let sound = Arc::new(Sound { rate: 100, channels: 1, samples: vec![0.5; 1000] });
        let mut m = Mixer { rate: 100.0, ..Default::default() };
        let mut v = voice(sound, 100.0);
        v.stopping = true;
        m.voices.push(v);
        let mut out = vec![0.0; 2 * 100];
        m.mix(&mut out);
        assert!(out[0] < 0.5 && out[0] > out[2 * 99]);
    }
}
