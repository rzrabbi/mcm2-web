//! Race tapes: the game's recordings of a race, used for saved replays
//! (`Record\*.vcr`), ghost races (`*.gho`) and the demo the credits roll
//! over (`UI/DEMO.RPL`). The tape is `VCRtape.dat` while a race records
//! (`0x49d4c0`); saving a replay copies it (`0x421a40`).
//!
//! A tape is a header (0xeb0 bytes from the game as shipped, 0xe84 in the
//! older build that made `UI/DEMO.RPL`) and then records, each
//! `[size = len + 12][type][id][len][data]`:
//!
//! * type −1, a frame: the race clock and how many bikes follow;
//! * type 1, a bike's whole state (0x58 bytes, `0x493620`);
//! * type 13, a bike's change since its last record (0x16 bytes in the
//!   demo, 0x17 in the game as shipped, `0x493a80` / `0x494390`);
//! * type 10, a stunt score; 0x87, who is "it" in Moto-Tag; 0x10 and 0x11,
//!   other objects (the tag ball).
//!
//! The same packets go over the network in a multiplayer race.

use crate::{Error, Result};

/// The game copies "MCMVCR" with its NUL and leaves the eighth byte as it
/// was (`0x49d4c0`), so only the first seven count.
pub const MAGIC: &[u8; 8] = b"MCMVCR\0\0";
const MAGIC_LEN: usize = 7;
pub const VERSION: &[u8; 4] = b"2.0\x1a";
/// Where the records start: the game as shipped (`0x49d4c0` writes 0xeb0
/// bytes, the Replay screen reads as many), and the demo's older build.
pub const HEADER_LEN: usize = 0xeb0;
pub const OLD_HEADER_LEN: usize = 0xe84;
/// Where the copy of the race settings sits, and how long it is (the
/// game's 0x7b words from its settings block, `+0x2d70`).
pub const SETTINGS_AT: usize = 0x34;
pub const SETTINGS_LEN: usize = 0x7b * 4;
/// The riders, as shipped (`0x49d620`): 0x124 bytes each from 0x224, id,
/// computer flag, name and the four model and skin names, then the bike's
/// size (bike +0x738), a flag byte (+0x737) and the plate number.
const RIDERS_AT: usize = 0x224;
const RIDER_LEN: usize = 0x124;
/// The older build's: 0x120 bytes each from 0x21c, size, flags and id
/// first.
const OLD_RIDERS_AT: usize = 0x21c;
const OLD_RIDER_LEN: usize = 0x120;
/// How many riders the header has room for.
pub const MAX_RIDERS: usize = 8;

pub const FRAME: i32 = -1;
pub const FULL: i32 = 1;
pub const STUNT: i32 = 10;
pub const DELTA: i32 = 13;
pub const TAG_IT: i32 = 0x87;

/// How far a delta moves per step: position and velocity in feet, angles
/// and spin in radians (2π/128).
pub const MOVE_STEP: f32 = 0.234375;
pub const TURN_STEP: f32 = 0.049087387;

/// Settings words, by their offset in the settings block.
pub mod setting {
    /// 0 a single race or practice, 2 a career race, 4 a ghost race.
    pub const MODE: usize = 0x00;
    /// 0 Quarry, 1 Baja, 2 Nationals, 3 Supercross, 4 Tag, 5 Enduro.
    pub const EVENT: usize = 0x04;
    pub const LAPS: usize = 0x20;
    pub const OPPONENTS: usize = 0x24;
    /// Which course of the track, a byte (the ghost list matches it,
    /// `0x44fb30`).
    pub const COURSE: usize = 0x34;
    /// The track's name, a string.
    pub const TRACK: usize = 0x36;
    pub const TRACK_LEN: usize = 32;
    pub const TAG_MINUTES: usize = 0x140;
    pub const TAG_BALL: usize = 0x144;
    pub const TAG_STUNTS: usize = 0x148;
}

/// A bike and rider on the tape.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rider {
    pub cc: u32,
    pub flags: u32,
    pub id: u32,
    /// The plate number (only the shipped build keeps it).
    pub plate: u32,
    pub computer: bool,
    pub name: String,
    pub bike_model: String,
    pub rider_model: String,
    pub bike_skin: String,
    pub rider_skin: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub kind: i32,
    pub id: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Tape {
    /// The date and time it was made, or the description it was saved
    /// with (31 characters at most).
    pub description: String,
    /// How long it runs, in seconds.
    pub length: f32,
    pub settings: Vec<u8>,
    pub riders: Vec<Rider>,
    pub records: Vec<Record>,
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b[..end].iter().map(|&c| c as char).collect()
}

fn put_str(out: &mut [u8], s: &str) {
    let n = s.len().min(out.len().saturating_sub(1));
    out[..n].copy_from_slice(&s.as_bytes()[..n]);
    out[n..].fill(0);
}

/// Whether records run from `o`: the first few hang together, or the tape
/// ends right there.
fn records_at(b: &[u8], mut o: usize) -> bool {
    for _ in 0..4 {
        if o == b.len() {
            return true;
        }
        if o + 16 > b.len() {
            return false;
        }
        let (size, len) = (u32_at(b, o) as usize, u32_at(b, o + 12) as usize);
        if size != len + 12 || o + 16 + len > b.len() {
            return false;
        }
        o += 16 + len;
    }
    true
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

impl Tape {
    pub fn new(settings: Vec<u8>, riders: Vec<Rider>) -> Tape {
        let mut settings = settings;
        settings.resize(SETTINGS_LEN, 0);
        Tape {
            description: String::new(),
            length: 0.0,
            settings,
            riders,
            records: Vec::new(),
        }
    }

    pub fn setting(&self, at: usize) -> u32 {
        u32_at(&self.settings, at)
    }

    pub fn set_setting(&mut self, at: usize, v: u32) {
        self.settings[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    pub fn course(&self) -> usize {
        self.settings[setting::COURSE] as usize
    }

    pub fn set_course(&mut self, course: usize) {
        self.settings[setting::COURSE] = course as u8;
    }

    pub fn track(&self) -> String {
        cstr(&self.settings[setting::TRACK..setting::TRACK + setting::TRACK_LEN])
    }

    pub fn set_track(&mut self, name: &str) {
        put_str(
            &mut self.settings[setting::TRACK..setting::TRACK + setting::TRACK_LEN],
            name,
        );
    }

    pub fn push(&mut self, kind: i32, id: u32, data: Vec<u8>) {
        self.records.push(Record { kind, id, data });
    }

    pub fn parse(bytes: &[u8]) -> Result<Tape> {
        if bytes.len() < OLD_HEADER_LEN {
            return Err(Error::Truncated {
                what: "tape header",
                offset: bytes.len(),
            });
        }
        if bytes[..MAGIC_LEN] != MAGIC[..MAGIC_LEN] {
            return Err(Error::BadMagic);
        }
        // Which build made it: the records follow its header.
        let old = !(bytes.len() >= HEADER_LEN && records_at(bytes, HEADER_LEN))
            && records_at(bytes, OLD_HEADER_LEN);
        let mut records = Vec::new();
        let mut o = if old { OLD_HEADER_LEN } else { HEADER_LEN };
        while o + 16 <= bytes.len() {
            let size = u32_at(bytes, o) as usize;
            let len = u32_at(bytes, o + 12) as usize;
            let end = o + 16 + len;
            if size != len + 12 || end > bytes.len() {
                return Err(Error::Truncated {
                    what: "tape record",
                    offset: o,
                });
            }
            records.push(Record {
                kind: u32_at(bytes, o + 4) as i32,
                id: u32_at(bytes, o + 8),
                data: bytes[o + 16..end].to_vec(),
            });
            o = end;
        }
        // As many riders as the frames carry bikes.
        let bikes = records
            .iter()
            .find(|r| r.kind == FRAME && r.data.len() >= 12)
            .map_or(0, |r| u32_at(&r.data, 8) as usize)
            .min(MAX_RIDERS);
        let riders = (0..bikes)
            .map(|k| {
                if old {
                    let r = &bytes[OLD_RIDERS_AT + k * OLD_RIDER_LEN..][..OLD_RIDER_LEN];
                    return Rider {
                        cc: u32_at(r, 0),
                        flags: u32_at(r, 4),
                        id: u32_at(r, 8),
                        plate: 0,
                        computer: r[0xc] != 0,
                        name: cstr(&r[0xd..0x1d]),
                        bike_model: cstr(&r[0x1d..0x5d]),
                        rider_model: cstr(&r[0x5d..0x9d]),
                        bike_skin: cstr(&r[0x9d..0xdd]),
                        rider_skin: cstr(&r[0xdd..0x11d]),
                    };
                }
                let r = &bytes[RIDERS_AT + k * RIDER_LEN..][..RIDER_LEN];
                Rider {
                    id: u32_at(r, 0),
                    computer: r[4] != 0,
                    name: cstr(&r[5..0x15]),
                    bike_model: cstr(&r[0x15..0x55]),
                    rider_model: cstr(&r[0x55..0x95]),
                    bike_skin: cstr(&r[0x95..0xd5]),
                    rider_skin: cstr(&r[0xd5..0x115]),
                    cc: u32_at(r, 0x118),
                    flags: u32_at(r, 0x11c),
                    plate: u32_at(r, 0x120),
                }
            })
            .collect();
        Ok(Tape {
            description: cstr(&bytes[0x10..0x30]),
            length: f32_at(bytes, 0x30),
            settings: bytes[SETTINGS_AT..SETTINGS_AT + SETTINGS_LEN].to_vec(),
            riders,
            records,
        })
    }

    /// Writes the tape as the game as shipped does (0xeb0-byte header).
    pub fn write(&self) -> Vec<u8> {
        let mut out = vec![0u8; HEADER_LEN];
        out[..8].copy_from_slice(MAGIC);
        out[8..12].copy_from_slice(VERSION);
        for (k, r) in self.riders.iter().take(MAX_RIDERS).enumerate() {
            let o = &mut out[RIDERS_AT + k * RIDER_LEN..][..RIDER_LEN];
            o[0..4].copy_from_slice(&r.id.to_le_bytes());
            o[4] = r.computer as u8;
            put_str(&mut o[5..0x15], &r.name);
            put_str(&mut o[0x15..0x55], &r.bike_model);
            put_str(&mut o[0x55..0x95], &r.rider_model);
            put_str(&mut o[0x95..0xd5], &r.bike_skin);
            put_str(&mut o[0xd5..0x115], &r.rider_skin);
            o[0x118..0x11c].copy_from_slice(&r.cc.to_le_bytes());
            o[0x11c..0x120].copy_from_slice(&r.flags.to_le_bytes());
            o[0x120..0x124].copy_from_slice(&r.plate.to_le_bytes());
        }
        let n = self.settings.len().min(SETTINGS_LEN);
        out[SETTINGS_AT..SETTINGS_AT + n].copy_from_slice(&self.settings[..n]);
        put_str(&mut out[0x10..0x30], &self.description);
        out[0x30..0x34].copy_from_slice(&self.length.to_le_bytes());
        for r in &self.records {
            out.extend(((r.data.len() + 12) as u32).to_le_bytes());
            out.extend(r.kind.to_le_bytes());
            out.extend(r.id.to_le_bytes());
            out.extend((r.data.len() as u32).to_le_bytes());
            out.extend(&r.data);
        }
        out
    }

    /// Plays the records through, giving each frame's bikes.
    pub fn timeline(&self) -> Timeline {
        let mut t = Timeline::default();
        let mut now: Vec<Option<State>> = vec![None; self.riders.len()];
        let mut time = 0.0;
        let mut started = false;
        for r in &self.records {
            match r.kind {
                FRAME if r.data.len() >= 12 => {
                    if started {
                        t.frames.push(Frame {
                            time,
                            bikes: now.clone(),
                        });
                    }
                    started = true;
                    time = f32_at(&r.data, 0);
                    let bikes = u32_at(&r.data, 8) as usize;
                    if now.len() < bikes {
                        now.resize(bikes, None);
                    }
                }
                FULL => {
                    if let (Some(s), Some(slot)) =
                        (State::full(&r.data), now.get_mut(r.id as usize))
                    {
                        *slot = Some(s);
                    }
                }
                DELTA => {
                    if let (Some(d), Some(Some(s))) =
                        (Delta::read(&r.data), now.get_mut(r.id as usize))
                    {
                        d.apply(s);
                    }
                }
                STUNT if r.data.len() >= 8 => t.stunts.push(Stunt {
                    time,
                    bike: r.id as usize,
                    score: u32_at(&r.data, 4),
                }),
                _ => t.other.push((time, r.clone())),
            }
        }
        if started {
            t.frames.push(Frame { time, bikes: now });
        }
        t
    }
}

/// A bike's state as a tape keeps it (the full packet's fields).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct State {
    /// The race place shown in the game's (`+0x784`).
    pub place: u8,
    /// Milliseconds since this bike's last record, as the packet says.
    pub step: u16,
    pub word_790: u16,
    pub word_7a0: u16,
    /// Where it is (centre of mass), in feet.
    pub position: [f32; 3],
    /// Roll, pitch and heading, in radians.
    pub angles: [f32; 3],
    pub lap_time: f32,
    pub best_lap: f32,
    pub total: f32,
    pub spin: [f32; 3],
    pub velocity: [f32; 3],
    /// The rider's pose and limbs (`+0x650`…`+0x660`).
    pub pose: [u8; 4],
    /// The game's millisecond clock when it was taken.
    pub clock: u32,
    /// Trick, crash and finish bits (`+0x430`, `+0x478`, `+0x7a4`…).
    pub flags: [u8; 4],
    /// The order the bike runs in (`+0x11c0`).
    pub index: u8,
}

fn vec_at(b: &[u8], o: usize) -> [f32; 3] {
    [f32_at(b, o), f32_at(b, o + 4), f32_at(b, o + 8)]
}

fn put_vec(b: &mut [u8], o: usize, v: [f32; 3]) {
    for (i, x) in v.iter().enumerate() {
        b[o + i * 4..o + i * 4 + 4].copy_from_slice(&x.to_le_bytes());
    }
}

impl State {
    pub const FULL_LEN: usize = 0x58;

    pub fn full(d: &[u8]) -> Option<State> {
        if d.len() < Self::FULL_LEN {
            return None;
        }
        let w = |o: usize| u16::from_le_bytes([d[o], d[o + 1]]);
        Some(State {
            place: d[1],
            step: w(2),
            word_790: w(4),
            word_7a0: w(6),
            position: vec_at(d, 8),
            angles: vec_at(d, 0x14),
            lap_time: f32_at(d, 0x20),
            best_lap: f32_at(d, 0x28),
            total: f32_at(d, 0x2c),
            spin: vec_at(d, 0x30),
            velocity: vec_at(d, 0x3c),
            pose: d[0x48..0x4c].try_into().unwrap(),
            clock: u32_at(d, 0x4c),
            flags: d[0x50..0x54].try_into().unwrap(),
            index: d[0x54],
        })
    }

    pub fn write_full(&self) -> Vec<u8> {
        let mut d = vec![0u8; Self::FULL_LEN];
        d[1] = self.place;
        d[2..4].copy_from_slice(&self.step.to_le_bytes());
        d[4..6].copy_from_slice(&self.word_790.to_le_bytes());
        d[6..8].copy_from_slice(&self.word_7a0.to_le_bytes());
        put_vec(&mut d, 8, self.position);
        put_vec(&mut d, 0x14, self.angles);
        d[0x20..0x24].copy_from_slice(&self.lap_time.to_le_bytes());
        d[0x28..0x2c].copy_from_slice(&self.best_lap.to_le_bytes());
        d[0x2c..0x30].copy_from_slice(&self.total.to_le_bytes());
        put_vec(&mut d, 0x30, self.spin);
        put_vec(&mut d, 0x3c, self.velocity);
        d[0x48..0x4c].copy_from_slice(&self.pose);
        d[0x4c..0x50].copy_from_slice(&self.clock.to_le_bytes());
        d[0x50..0x54].copy_from_slice(&self.flags);
        d[0x54] = self.index;
        d
    }

    /// Between this and `b`, `f` of the way (angles the short way round).
    pub fn lerp(&self, b: &State, f: f32) -> State {
        let l = |x: [f32; 3], y: [f32; 3]| std::array::from_fn(|i| x[i] + (y[i] - x[i]) * f);
        let angles =
            std::array::from_fn(|i| self.angles[i] + wrap(b.angles[i] - self.angles[i]) * f);
        let mut s = if f < 0.5 { *self } else { *b };
        s.position = l(self.position, b.position);
        s.velocity = l(self.velocity, b.velocity);
        s.angles = angles;
        s
    }
}

/// An angle brought into −π…π.
fn wrap(a: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    a - (a / tau).round() * tau
}

/// A bike's change since its last record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Delta {
    pub position: [i8; 3],
    /// Milliseconds: below 128 as `ms << 1`, else `(ms >> 3) << 1 | 1`.
    pub step: u8,
    pub angles: [i8; 3],
    pub spin: [i8; 3],
    pub velocity: [i8; 3],
    pub flags: [u8; 4],
    pub pose: [u8; 4],
    /// Only in the game as shipped; the demo's deltas stop before it.
    pub index: Option<u8>,
}

impl Delta {
    pub const LEN: usize = 0x17;

    pub fn read(d: &[u8]) -> Option<Delta> {
        if d.len() < 0x16 {
            return None;
        }
        let v = |o: usize| [d[o] as i8, d[o + 1] as i8, d[o + 2] as i8];
        Some(Delta {
            position: v(1),
            step: d[4],
            angles: v(5),
            spin: v(8),
            velocity: v(0xb),
            flags: d[0xe..0x12].try_into().unwrap(),
            pose: d[0x12..0x16].try_into().unwrap(),
            index: d.get(0x16).copied(),
        })
    }

    pub fn write(&self) -> Vec<u8> {
        let mut d = vec![0u8; Self::LEN];
        let mut put = |o: usize, v: [i8; 3]| {
            for (i, x) in v.iter().enumerate() {
                d[o + i] = *x as u8;
            }
        };
        put(1, self.position);
        put(5, self.angles);
        put(8, self.spin);
        put(0xb, self.velocity);
        d[4] = self.step;
        d[0xe..0x12].copy_from_slice(&self.flags);
        d[0x12..0x16].copy_from_slice(&self.pose);
        d[0x16] = self.index.unwrap_or(0);
        d
    }

    pub fn millis(&self) -> u32 {
        if self.step & 1 == 0 {
            (self.step >> 1) as u32
        } else {
            ((self.step >> 1) as u32) << 3
        }
    }

    /// What the game does with a delta (`0x494390`).
    pub fn apply(&self, s: &mut State) {
        for i in 0..3 {
            s.position[i] += self.position[i] as f32 * MOVE_STEP;
            s.velocity[i] += self.velocity[i] as f32 * MOVE_STEP;
            s.angles[i] += self.angles[i] as f32 * TURN_STEP;
            s.spin[i] += self.spin[i] as f32 * TURN_STEP;
        }
        let ms = self.millis();
        s.step = ms as u16;
        s.clock = s.clock.wrapping_add(ms);
        s.flags = self.flags;
        s.pose = self.pose;
        if let Some(i) = self.index {
            s.index = i;
        }
    }

    /// The delta from what a player of the tape has (`from`) to `to`, or
    /// `None` when it won't fit and a full record must go instead
    /// (`0x493a80`: 30 feet, a whole turn, a second). Measuring from what
    /// the player has, not the last true state, carries each rounding
    /// over to the next delta.
    pub fn between(from: &State, to: &State) -> Option<Delta> {
        let q = |d: f32, step: f32| -> Option<i8> {
            let n = (d / step).round();
            (n.abs() <= 127.0).then_some(n as i8)
        };
        let mut out = Delta {
            flags: to.flags,
            pose: to.pose,
            index: Some(to.index),
            ..Delta::default()
        };
        for i in 0..3 {
            out.position[i] = q(to.position[i] - from.position[i], MOVE_STEP)?;
            out.velocity[i] = q(to.velocity[i] - from.velocity[i], MOVE_STEP)?;
            out.angles[i] = q(wrap(to.angles[i] - from.angles[i]), TURN_STEP)?;
            out.spin[i] = q(to.spin[i] - from.spin[i], TURN_STEP)?;
        }
        let ms = to.clock.wrapping_sub(from.clock);
        out.step = match ms {
            0..=127 => (ms << 1) as u8,
            128..=1016 => (((ms >> 3) << 1) | 1) as u8,
            _ => return None,
        };
        Some(out)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stunt {
    pub time: f32,
    pub bike: usize,
    pub score: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Frame {
    /// The race clock, in seconds.
    pub time: f32,
    /// Each bike as of this frame; `None` before its first full record.
    pub bikes: Vec<Option<State>>,
}

#[derive(Debug, Clone, Default)]
pub struct Timeline {
    pub frames: Vec<Frame>,
    pub stunts: Vec<Stunt>,
    /// Records it doesn't follow (the tag ball, who is "it"), by time.
    pub other: Vec<(f32, Record)>,
}

impl Timeline {
    pub fn length(&self) -> f32 {
        self.frames.last().map_or(0.0, |f| f.time)
    }

    /// Bike `bike` at `time`, between the frames either side.
    pub fn sample(&self, bike: usize, time: f32) -> Option<State> {
        let i = self.frames.partition_point(|f| f.time <= time);
        let get = |k: usize| {
            self.frames
                .get(k)
                .and_then(|f| f.bikes.get(bike).copied().flatten())
        };
        match (i.checked_sub(1).and_then(get), get(i)) {
            (Some(a), Some(b)) => {
                let (ta, tb) = (self.frames[i - 1].time, self.frames[i].time);
                let f = if tb > ta {
                    (time - ta) / (tb - ta)
                } else {
                    0.0
                };
                // A full record after a jump (a reset) isn't slid to.
                let far = (0..3).any(|k| (b.position[k] - a.position[k]).abs() > 30.0);
                Some(if far {
                    a
                } else {
                    a.lerp(&b, f.clamp(0.0, 1.0))
                })
            }
            (a, b) => a.or(b),
        }
    }
}

/// Writes a race to a tape the way the game does: a frame record each
/// time, then each bike as a delta from what the tape already says, or
/// whole when a delta won't do or the keyframe time is up.
pub struct Recorder {
    pub tape: Tape,
    sent: Vec<Option<State>>,
    since_full: Vec<f32>,
    frames: u32,
    /// Seconds between a bike's whole records (the game's `+0x341c`; its
    /// demo has them about every 7.6 s).
    pub keyframe: f32,
}

impl Recorder {
    pub fn new(tape: Tape) -> Recorder {
        Recorder {
            tape,
            sent: Vec::new(),
            since_full: Vec::new(),
            frames: 0,
            keyframe: 7.5,
        }
    }

    /// One frame of the race: the clock and every bike.
    pub fn frame(&mut self, time: f32, dt: f32, bikes: &[State]) {
        self.frames += 1;
        let mut f = Vec::with_capacity(12);
        f.extend(time.to_le_bytes());
        f.extend(self.frames.to_le_bytes());
        f.extend((bikes.len() as u32).to_le_bytes());
        self.tape.push(FRAME, 0, f);
        self.sent.resize(bikes.len(), None);
        self.since_full.resize(bikes.len(), 0.0);
        for (i, now) in bikes.iter().enumerate() {
            self.since_full[i] += dt;
            let delta = match &self.sent[i] {
                Some(had) if self.since_full[i] < self.keyframe => Delta::between(had, now),
                _ => None,
            };
            match delta {
                Some(d) => {
                    let mut had = self.sent[i].unwrap();
                    d.apply(&mut had);
                    self.sent[i] = Some(had);
                    self.tape.push(DELTA, i as u32, d.write());
                }
                None => {
                    let mut s = *now;
                    s.step = self.sent[i].map_or(0, |h| now.clock.wrapping_sub(h.clock) as u16);
                    self.sent[i] = Some(s);
                    self.since_full[i] = 0.0;
                    self.tape.push(FULL, i as u32, s.write_full());
                }
            }
        }
        self.tape.length = time;
    }

    pub fn stunt(&mut self, bike: usize, score: u32) {
        let mut d = vec![0u8; 8];
        d[1] = bike as u8;
        d[4..8].copy_from_slice(&score.to_le_bytes());
        self.tape.push(STUNT, bike as u32, d);
    }

    pub fn finish(self) -> Tape {
        self.tape
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(x: f32, h: f32, clock: u32) -> State {
        State {
            position: [x, 10.0, -x],
            angles: [0.0, 0.1, h],
            velocity: [3.0, 0.0, 1.0],
            clock,
            ..State::default()
        }
    }

    #[test]
    fn tapes_write_and_read_back() {
        let riders = vec![Rider {
            name: "Mike".into(),
            bike_model: "suzuki.mcf".into(),
            ..Rider::default()
        }];
        let mut tape = Tape::new(Vec::new(), riders.clone());
        tape.set_track("Iffendic");
        tape.set_setting(setting::LAPS, 3);
        let mut rec = Recorder::new(tape);
        let mut t = 0.0;
        for k in 0..400 {
            let x = k as f32 * 0.37;
            let h = 3.1 + k as f32 * 0.01; // runs past π
            rec.frame(t, 0.05, &[state(x, h, 1000 + k * 50)]);
            t += 0.05;
        }
        let bytes = rec.finish().write();
        let back = Tape::parse(&bytes).unwrap();
        assert_eq!(back.riders, riders);
        assert_eq!(back.track(), "Iffendic");
        assert_eq!(back.setting(setting::LAPS), 3);
        let line = back.timeline();
        assert_eq!(line.frames.len(), 400);
        let last = line.frames[399].bikes[0].unwrap();
        let want = state(399.0 * 0.37, 3.1 + 3.99, 1000 + 399 * 50);
        // Rounding is carried over, so it never drifts past one step.
        assert!((last.position[0] - want.position[0]).abs() <= MOVE_STEP);
        assert!(wrap(last.angles[2] - want.angles[2]).abs() <= TURN_STEP);
        assert_eq!(last.clock, want.clock);
        assert!(back.records.iter().filter(|r| r.kind == FULL).count() >= 3);
    }

    #[test]
    fn reads_both_builds_headers() {
        let riders = vec![Rider {
            name: "Mike".into(),
            cc: 250,
            plate: 7,
            ..Rider::default()
        }];
        let mut tape = Tape::new(Vec::new(), riders.clone());
        tape.push(
            FRAME,
            0,
            [0f32.to_le_bytes(), 0f32.to_le_bytes(), 1u32.to_le_bytes()].concat(),
        );
        let mut bytes = tape.write();
        // The shipped game's header, its eighth magic byte left dirty.
        assert_eq!(u32_at(&bytes, HEADER_LEN), 12 + 12);
        bytes[7] = 0xc1;
        assert_eq!(Tape::parse(&bytes).unwrap().riders, riders);
        // The demo's build: a shorter header and the riders laid out
        // size, flags, id first.
        let mut old = vec![0u8; OLD_HEADER_LEN];
        old[..0x34].copy_from_slice(&bytes[..0x34]);
        let r = &mut old[OLD_RIDERS_AT..][..OLD_RIDER_LEN];
        r[0..4].copy_from_slice(&250u32.to_le_bytes());
        put_str(&mut r[0xd..0x1d], "Mike");
        old.extend(&bytes[HEADER_LEN..]);
        let back = Tape::parse(&old).unwrap();
        assert_eq!(
            (back.riders[0].cc, back.riders[0].name.as_str()),
            (250, "Mike")
        );
    }

    #[test]
    fn deltas_carry_their_time() {
        for ms in [0u32, 1, 127, 128, 500, 1016] {
            let d = Delta::between(&state(0.0, 0.0, 0), &state(1.0, 0.0, ms)).unwrap();
            assert!(ms.abs_diff(d.millis()) < 8, "{ms} -> {}", d.millis());
        }
        assert!(Delta::between(&state(0.0, 0.0, 0), &state(1.0, 0.0, 1100)).is_none());
        assert!(Delta::between(&state(0.0, 0.0, 0), &state(40.0, 0.0, 10)).is_none());
    }
}
