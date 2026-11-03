//! The race's EAX reverb (Ctrl+V), heard in software: the game asks the
//! card's EAX listener for environment 9, Arena, at volume 0.361, a 7 s
//! decay and damping 0.332 (0x4e9250), and for no reverb (environment 17,
//! volume 0) when switched off. No card does EAX any more, so this is a
//! Schroeder–Moorer reverb (Freeverb's layout) tuned to those numbers:
//! eight damped combs whose feedback dies 60 dB in the decay time, then
//! four all-passes, added to the dry mix at the volume.

/// Freeverb's comb and all-pass lengths at 44.1 kHz, and the right
/// channel's spread.
const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const PASSES: [usize; 4] = [556, 441, 341, 225];
const SPREAD: usize = 23;

/// EAX's Arena (environment 9).
pub const ARENA: Reverb = Reverb { volume: 0.361, decay: 7.0, damping: 0.332 };

/// An EAX environment's three numbers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reverb {
    /// The reverb's level against the dry sound, 0 to 1.
    pub volume: f32,
    /// Seconds for the tail to fall 60 dB.
    pub decay: f32,
    /// How much faster the highs die, 0 to 2 (EAX's range).
    pub damping: f32,
}

struct Comb {
    line: Vec<f32>,
    at: usize,
    feedback: f32,
    low: f32,
}

impl Comb {
    fn run(&mut self, x: f32, damp: f32) -> f32 {
        let y = self.line[self.at];
        self.low = y * (1.0 - damp) + self.low * damp;
        self.line[self.at] = x + self.low * self.feedback;
        self.at = (self.at + 1) % self.line.len();
        y
    }
}

struct Pass {
    line: Vec<f32>,
    at: usize,
}

impl Pass {
    fn run(&mut self, x: f32) -> f32 {
        let b = self.line[self.at];
        self.line[self.at] = x + b * 0.5;
        self.at = (self.at + 1) % self.line.len();
        b - x
    }
}

/// The running reverb, one set of lines a channel.
pub struct Tail {
    pub reverb: Reverb,
    combs: [Vec<Comb>; 2],
    passes: [Vec<Pass>; 2],
    damp: f32,
}

impl Tail {
    pub fn new(reverb: Reverb, rate: f32) -> Tail {
        let scale = rate / 44100.0;
        let len = |n: usize, ch: usize| (((n + ch * SPREAD) as f32 * scale) as usize).max(1);
        let combs = std::array::from_fn(|ch| {
            COMBS
                .iter()
                .map(|&n| {
                    let n = len(n, ch);
                    Comb {
                        line: vec![0.0; n],
                        at: 0,
                        // 60 dB down in the decay time: g^(T / delay) = 10^-3.
                        feedback: 10f32.powf(-3.0 * n as f32 / (rate * reverb.decay.max(0.1))),
                        low: 0.0,
                    }
                })
                .collect()
        });
        let passes = std::array::from_fn(|ch| {
            PASSES.iter().map(|&n| Pass { line: vec![0.0; len(n, ch)], at: 0 }).collect()
        });
        Tail {
            reverb,
            combs,
            passes,
            damp: (reverb.damping / 2.0).clamp(0.0, 0.95),
        }
    }

    /// Adds the reverb of interleaved stereo `out` to it.
    pub fn apply(&mut self, out: &mut [f32]) {
        // Freeverb's input gain, so eight combs don't swamp the dry sound.
        const INPUT: f32 = 0.015;
        let wet = self.reverb.volume;
        for frame in out.chunks_exact_mut(2) {
            let x = (frame[0] + frame[1]) * INPUT;
            for ch in 0..2 {
                let mut y: f32 = self.combs[ch].iter_mut().map(|c| c.run(x, self.damp)).sum();
                for p in &mut self.passes[ch] {
                    y = p.run(y);
                }
                frame[ch] += y * wet;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arena_rings_on_and_dies_away() {
        let rate = 22050.0;
        let mut tail = Tail::new(ARENA, rate);
        let mut out = vec![0.0f32; 2 * rate as usize * 8];
        out[0] = 1.0;
        out[1] = 1.0;
        tail.apply(&mut out);
        let energy = |from: f32, to: f32| -> f32 {
            out[(from * rate) as usize * 2..(to * rate) as usize * 2].iter().map(|s| s * s).sum()
        };
        let early = energy(0.1, 0.6);
        let late = energy(6.5, 7.0);
        assert!(early > 0.0);
        // About 60 dB down after the decay time (the damping takes more).
        let db = 10.0 * (late / early).log10();
        assert!((-80.0..-45.0).contains(&db), "{db}");
    }
}
