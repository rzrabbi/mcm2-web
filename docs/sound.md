# Sound in the original game

How Mcm2.exe plays its sound, from a Ghidra decompile of the exe, and what the
port does. The engine note itself is in `physics.md` section 15. Code: `crates/mm2-viewer/src/sound.rs`.

## DirectSound setup (RaceSound, 0x4e7160 / 0x4e81a0)

* Every race sound except the crowd and the chimes is a **DirectSound 3D buffer**. The listener is the camera
  (camera +0x170 position, +0x17c orientation), and its velocity is the followed bike's (0x4e84a0).
* Listener: distance factor **0.3048** (the game works in feet), rolloff **1**, Doppler factor **1**.
  So each sound is full volume inside its minimum distance, min/d out to its maximum distance, and
  Doppler-shifted at 343 m/s = 1125 ft/s. Every 3D buffer is made with `DSBCAPS_MUTE3DATMAXDISTANCE`
  (flags `| 0x20010` in 0x4c1c90), so beyond its maximum distance it is silent: a computer rider crashing
  more than 500 ft from the camera is not heard.
* Minimum / maximum distances: engines 50 / 500 ft, wrecks and falls 50 / 500, the hard landing 10 / 500.
* Engine buffers get the bike's place and velocity every frame. Wrecks and falls are placed where the bike was
  when they started. The hard landing follows the followed bike.
* Race volume: the In-Game Volume slider sets the primary buffer to `volume × 25 − 2500` hundredths of a dB
  (0 → silence). In-Game Sound Effects off plays nothing (every play goes through 0x4ea510, which checks it).
* 16-bit sound reads `Audio_16.ini` and its `16_*.wav` samples; 8-bit reads `Audio_08.ini`'s `8_*.wav`.
* EAX (when on): environment 9 (arena) in Supercross and Moto-Tag, 17 (mountains) elsewhere. Not ported.
* Hotkeys in a race: sound on/off (key 0x1f), EAX on/off (0x2f), and in Supercross the crowd volume, cycling
  0, −5 dB and off (0x2e). Not ported.

## Engines (0x4e8720)

* Every bike's state machine runs every frame (section 15). The **four bikes nearest the camera** get one
  of the four streaming buffers, however far away they are: distances are side to side and front to back,
  and anything past 500 ft on either axis counts as 500 (0x4e91d0).
* A bike without a buffer keeps its state but its stream stands still (0x4e9aa0 needs a buffer); when it gets
  a buffer it carries on from where it stood.
* Each bike plays its own class's samples: below 250 cc `[125]`, two-strokes from 250 cc up `[250]`
  (the 500 too), four-strokes `[400]`.
* With 16-bit sound and more than one class in the race, the lists shrink to save memory: two classes, the
  other class keeps half of each list; three classes, the others keep a quarter and the player's half. Lists of
  one sample keep it. (0x4e7160 is called with 6 as its last argument, which picks this branch.)
* The speed is the bike's scalar speed `f_b8` × 0.68182, truncated to whole mph, against whole-mph bands.

## Effects (0x4e8720's second loop)

* **LandHard01**: every time the followed bike touches down (`f_108` clears), unless the last one still plays.
  There is no "hard" threshold.
* **Wrecks**: while a bike is crashed (`f_444`), each time it lands (`f_484`, set by 0x52d3e0 when the
  airborne flag clears) a random `wreckNN.wav`, unless that bike's last wreck still plays. Each of the six is a
  single buffer shared by all bikes; 0x4e9050 tries ten random picks for one not playing.
* **Falls**: while crashed, every frame the thrown rider's body touches something (Wrecker +0xb0, from its
  collision model's +0x58), a random `fallNN.wav`, unless that bike's last fall still plays. So the rider grunts
  again and again while sliding.
* **launch.wav**: when the followed bike's crash reason is 0xc, riding off the edge of the world (0x48edd0
  launches the bike at 120 ft/s). Not at the start. The port has no edge launch, so it never plays.
* **Waypoint.wav**: with the Waypoint Chime option on, when the followed bike passes a gate (+0x7b8) or
  crosses the line (+0x7a0).
* **It.wav**: a new IT in Moto-Tag (0x4ae360 → 0x4ea5f0).
* **bonus.wav**: a stunt worth 10,000 points or more (score × multiplier) in the Stunt Quarry or Moto-Tag
  (0x496fa0 → 0x4ea560).

## Crowd (scenes with `[Sounds] CrowdPresent=T`: the 17 stadiums and 3 Moto-Tag arenas)

* `CrowdLoop.wav` loops, flat, from the start.
* The followed rider moves up a place: `Applause03` on taking the lead, otherwise `Applause01` at most every
  15 s. (`Applause02` needs `rand()` to return 0.)
* Supercross only, as the followed rider crashes: `boo_01` when leading, otherwise `oh_01` at most every 30 s.
* Supercross only, a stunt: `Applause01` at most every 15 s.

## Scene sounds (SoundEmitter, 0x402080–0x403100)

Static models, animations, scripted vehicles and `[StaticSoundN]` (type Emitter) sections can name a sound:
`SoundResourceFile` (".wav" added), `SoundMinDistance`, `SoundMaxDistance` (both 0 when missing),
`SoundLooping`, `SoundOneShot`, `SoundOneShotDistance` (20), `SoundRandomTriggerPercent` (100),
`SoundForce2D` (a software buffer), and for animations `SoundAttachToObject`, the frame it hangs on.
299 such sounds across the tracks: windmills, wind generators, trains, cars and trucks, planes, oil wells,
power lines, radios and more.

* Each frame (0x4024d0) the emitter measures the camera's distance side to side and front to back. It is in
  range while that is under 1.2 × its maximum distance; a one-shot is in range once, within its one-shot
  distance.
* Coming in range it rolls its trigger chance once (`rand() / 32768 × 100 > percent` skips it) and plays,
  looping or once. Going out of range stops it.
* It is a 3D buffer with its own min/max distance, at the frame it hangs on, with a velocity from its movement.

## Music

None in a race. The game opens the CD audio device only to stop it and read its status (0x4320a0, 0x4320c0);
there is no MCI play. The menus' sound comes from their movies.

## Checked against a recording (SX01, 2026-10-23)

The original's engine buffer, captured under CrossOver with the same inputs, against the port's
`--engine-wav`. Lined up 0.1 s at a time against every `8_250*` sample, both play the same sequence (idle,
low torque, power band, decel into idle, mid torque, decel, idle) with every change of state within 0.05 s,
and the same levels: the port is 1 dB louder once the capture's first 0.25 s is left out. That first stretch
is an empty 8-bit buffer (bytes of 0, so full-scale DC), which made the capture's mean look 5 dB louder.

* Which sample of a list plays is the C runtime's `rand() % count` (0x4e9aa0), seeded from the clock
  (`srand(0x4c41d0())`), so it differs from run to run in the original too. In the recording the first gear
  change came 0.19 s later in the original only because it drew `LowTorque02` (1.07 s) where the port drew
  `LowTorque04` (0.88 s).
* `--engine-wav` now records the engines alone, as the capture does. It used to mix in `LandHard01`, which
  is what sounded like a click at 8.46 s: the port's bike touched down there (`f_108` cleared), which the
  game also thumps for, but the engine capture can't show.

The pan law of DirectSound's software 3D is not in the game; the port pans by the side the sound is on.
