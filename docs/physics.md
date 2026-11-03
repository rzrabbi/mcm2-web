# MM2 bike and rider physics (from Mcm2.exe, codename krusty2)

This is a map of how Motocross Madness 2 simulates the bike, made from a Ghidra
decompile of the unprotected `GAME/Mcm2.exe` (not part of this repository). Addresses
are virtual addresses in that exe (image base 0x400000). Field names like `f_64`
are byte offsets into the bike object. Status labels: **[solid]** means read
carefully, **[likely]** means the structure is clear but the meaning is inferred,
**[open]** means not decoded yet.

Units are **feet, seconds and pounds**. Gravity is 32 ft/s² (and 32.2 in places), the wheelbase is
7 ft, and the world is left-handed (x right, y up, z forward), which matches the viewer.

## 1. Architecture

```
SoultreePhysicsObject  (SoulTreePhysics.cpp 0x505ae0..0x509530: rigid body, fixed-step loop)
  └ Vehicle            (Vehicle.cpp 0x52b470..0x531000: tyres, shocks, drivetrain, input)
      └ Bike           (Bike.cpp 0x404f40..0x415170: bike-specific forces, crashes, AI solver)
          └ KrustyBike (KrustyBike.cpp 0x48d850..0x499850: game glue, controls, rider anim)
```

| vtable | address | slots |
|---|---|---|
| Vehicle | 0x55de08 | 97 |
| Bike | 0x5559b8 | 103 |
| KrustyBike | 0x559600 | 103 |

The object also has secondary vtables at offsets +0x4 and +0x21c (multiple inheritance).

Other source files and where they start:

| file | start | role |
|---|---|---|
| BikeAI.cpp | 0x415170 | AI line following |
| bikerace.cpp | 0x418cd0 | race object |
| CollisionObject.cpp | 0x432d60 | .col loading, BVH |
| CollisionPoint.cpp | 0x43b2d0 | point-vs-world contacts |
| ConstraintMethodCollisionModel.cpp | 0x43d790 | contact constraints |
| SelectiveGravityModel / Shock | 0x4fe5a0 | InlineShock, RotatingShock |
| SteeringControl.cpp | 0x509fa0 | steering filter |
| Terrain.cpp | 0x50ad70 | height and normal sample 0x50d3b0 |
| Tire.cpp | 0x518c00..0x51b4f0 | tyre contact, slip, friction |
| wrecker.cpp | 0x535730 | crash and ragdoll |
| drivetrain | 0x4d7090 / 0x4d76a0 / 0x4d7780 | engine, gears |

## 2. Per-frame entry: two different physics paths

`KrustyBike` vtable slot 49 = **0x497740** is called once per frame with dt. **[solid]**

```c
if (!crashed /*f_444*/ && (isAI /*f_734*/ || race->f_b8) && race->f_18a)
    Bike_SimulateSimple(this, dt);        // 0x414000: AI / "autopilot" two-wheel solver
else
    Bike_Simulate(this, dt);              // 0x40d820 -> Vehicle::Simulate 0x52ffb0
```

* **The player path** uses the full Vehicle model, with tyres, shocks, drivetrain and a rigid body. A crashed bike also takes this path.
* **AI riders** use a cheaper two-point model (section 4). It runs **once per frame with the whole frame's dt**, with no fixed step, so it is frame-rate dependent: AI riders shake and fall above about 60 fps. In a port, give it a fixed step and interpolate between the last two steps for drawing. `race+0xb8` forces
  the simple model for everyone; only the BajaRace constructor sets it (section 13). `race+0x18a` is set
  when the countdown ends.

For the port, the AI solver is the easiest thing to reproduce, and it already
feels like MM2. The player model is the faithful one.

## 3. Bike object field map **[solid unless marked]**

| field | meaning |
|---|---|
| f_c (vec3) | position |
| f_18..f_44 | world matrix (rebuilt from look/up by 0x4ba1a0) |
| f_28 | 32.2 / weight (gravity scale) |
| f_64 (vec3) | linear velocity |
| f_7c (vec3) | previous velocity |
| f_88 (vec3) | look (forward) |
| f_94 (vec3) | up |
| f_a0 / f_ac | previous look / up |
| f_b8, f_bc | speed (scalar) |
| f_cc, f_d8 | angular velocity (+ previous) |
| f_e4..f_f8 | inverse inertia |
| f_13c / f_140 | step dt / 1/dt |
| f_158 | mass |
| f_15c | weight, default 165 |
| f_164 | 3x3 inertia tensor |
| f_1e0 / f_1e4 / f_1ec | leftover time / fixed step (0.02 s) / max steps |
| f_444 | crashed flag |
| f_460 | state (4 = crashing) |
| f_468 | ControlInterface* |
| f_47c | SteeringControl* |
| f_480 | drivetrain* |
| f_4f8 / f_4fc / f_500 | input axis objects: steer / lean / analog throttle |
| f_504 | steer, −1..1 |
| f_508 | lean (rider pitch), −1..1 |
| f_520 | rider anim pose index, chosen from steer (0x40d9d0) |
| f_53c / f_544 | tyre array / count (2) |
| f_5f0 / f_5f4 | front / rear tyre |
| f_734 | is AI |
| f_740 | race*; `race+0x4c` = terrain |
| f_79c | engine class 0..2 (indexes the thrust table) |
| f_7e0 / f_7ec | front / rear wheel velocity (AI path) |
| f_7f8 / f_7fc | front / rear suspension state (AI path) |
| f_838 | AI data (+0x96c target speed, +0x970 power multiplier) |
| f_15e0 | collision world |

Inertia comes from 0x5068xx: a sphere is `1/(0.4·m·r²)` and a box is `1/(m/12·(w²+h²))` per axis.

## 4. AI / simple model: 0x414000 → solver 0x40faa0 **[solid]**

The bike is a dumbbell: two wheel points 3.5 ft ahead of and behind the centre.

```c
void Bike_SimulateSimple(Bike* b, float dt) {
    vec3 fwd = b->look * 3.5f;
    vec3 front = b->pos + fwd, rear = b->pos - fwd;
    vec3 vf = b->vel + cross(b->angVel, fwd);
    vec3 vr = b->vel - cross(b->angVel, fwd);
    float controls[3] = { b->steer, b->lean, throttleOrBrake /* +1 gas, -1 brake */ };
    bool airborne; float maxImpact;
    Solve2Wheel(controls, &vf, &vr, &b->vel, 0, &b->pos, &b->look, &b->up, dt, dt,
                race->terrain, &airborne, &maxImpact, b->collisionWorld,
                &b->frontSusp, &b->rearSusp, frontTyre+0x260, rearTyre+0x260,
                &b->ai, thrustTable[b->engineClass], b->f_824, b->f_11b0);   // 0x40faa0
    b->angVel = cross(b->look, vf - b->vel) * 0.2857f;   // = 1/3.5
    // control smoothing
    steer = approach(steer, steerTarget, 0.125f);   // per call
    lean  = approach(lean,  leanTarget,  0.25f);
    riderBlend = (lean + 2) * 0.25f;
    // crash tests (section 8)
}
```

**Frame rate.** The original runs this once a frame with the frame's
`dt`, and much of it is per call: the steer and lean smoothing, the
suspension damping (from the substep length), the speed-loss and
speed-jump crash tests, and the controller's 2% trick chance. So the
computer riders ride as on the AI tape only at the tape's frame rate,
0.028 s a frame. Run each screen frame, the port's riders crashed a
third more often at 60 and 120 fps (12 whole SX01 races: 38 crashes at
the tape's frames, 49 at 60 or 120 fps; the tape has 31). Live play
therefore runs the computer riders' controller and model in frames of
0.028 s (`race::AI_FRAME`, 35–36 crashes at 60 and 120 fps) and draws
them between frames; a crashed computer rider's full model takes whole
0.02 s steps each frame like the player's bike. Scripted runs
(`--ai-check`, `--ride`) still step them at the tape's own frames.

**The air routine.** The bike's air routine `Bk::0x40bcc0` (section on
air control) runs on the computer riders too, every frame in the air:

- With the air pose on (`f_6fc`, off while a trick runs) and the bike
  pointing 10° to 57° off its path (`0.55 < dir·look < 0.984`), the path
  is pushed sideways, `(1 − dir·look) · 3.41297 · speed · 0.075 · dt`,
  always to the side the steer pose's phase picks (`f_65c < 0.5`: the
  computer riders' is 0), and the speed is cut by as much. On the AI
  tape the rider loses up to 5 ft/s and drifts 2 to 3 ft/s sideways over
  a long jump, nose up over the far side; fitted over 4,600 air frames
  the law leaves 0.03 ft/s² of a 1.7 ft/s² median.
- The spin is damped ×0.9 (`f_d8..f_e0`). With it the replayed riders'
  pitch in the air matches the tape to 0.01–0.05 rad; without it the
  nose dropped twice as far after a hop. Damping on the ground too makes
  the replays worse, so it is in the air only.

Replays (`--from 10,15,..,180 --for 2.1`): median error at 2 s 1.54 →
1.34 ft, p75 3.46 → 2.84 ft. The lean's pitch (`f_528`) is 0 for them.

Inside `Solve2Wheel` (0x40faa0, 17.5 KB), as the computer riders use it
(ported in `mm2-physics/src/computer.rs`, checked against the AI tape
`ai_race4.csv`):

```
h = min(dt, 0.05); called once a frame with the frame's dt, in substeps of h
damp = ((h - 0.005) * 4.21053 + 0.6) * 0.6
for each substep:
  a = (0, -32, 0) at each end
  per end: ground height + normal (terrain, or a solid's top face)
    gap = (p + up*spring - ground).n ; on the ground when gap <= 0.5
    off: spring -= 90*spring * h*h/2 * 3
    on:  into = v.n; if into < 0 remove it (spring >= 1) or leave
         up*into*(up.n)*damp; then if |v| fell: v = 0.9 v + 0.1 |v_before| dir(v)
         push p out by gap; ground pull/spring as in the code
  rear on the ground, drive (lap and gate races):
         f = (limit - |vR|) / h, at most power * THRUST[class][int |vR|], at least -256
         f >= 0: aR += look*f ; braking scales vR; past a stop both ends stop
  steering, either end down: d = the aim (front down) or level(vF)
         obstacle scan: d = level(vF - O (vF.O)) when blocked (not d -= o(d.o))
         45° cap (0.70755) toward the side; gate events d = level(5 level(vF) + d)
         want = d * rough_sqrt(vFx² + vFz²)  (0x461ac0, see below)
         change = (want - vF) levelled, at most 0.2 |vF|; aF += change / h
  the rear's sideways slide is removed completely (a and v/h)
  pull and velocity along the bike are shared 0.35 front / 0.65 rear
  p += a h²/2 + v h (air: front -= up*pitch*h, rear += up*pitch*h); v += a h
  look = dir(pF - pR); the aim is made again
  up = (normalize(S (pull.S) + (0, 512, 0)) h + up 4dt) / (4dt + h), made square to look
last: both ends out of the ground, the rear with the front's spring and the
rear's query point from the substep before (a slip in the game, kept)
```

**The quick square root (0x461ac0).** The root's mantissa comes from a table
of 256 (built by 0x461a50: the roots of 1 + i/128 and 2 + i/64 with only the
top 7 mantissa bits kept), so it comes out up to 0.8% short. The front's
steering target speed uses it, so every substep pulls the front back by up to
0.8% of its speed, a drag that grows with speed. With the exact root the port's
riders gained about 4 ft/s² too much above 60 ft/s and reached 105 ft/s on
SX01's start straight against the game's 92; with it the replay check's speed
bias is gone (median 0.0 ft/s after 1 s over 61 runs). The jump planner's
distances and needed speed also use it.

**The controls and crash tests (0x414000).** In the air the pitch rate is
(landing normal . look − 0.087) · 3.5 / air time (air time and normal from a
0.1 s flight trace, 0x40f170), and the lean follows a global (0x57dc10, shared
by every computer rider) at rate min(dt, 0.5)/0.5. On the ground the lean
target is ±0.1 when |look.y| ≥ 0.3, else 0. Steer anim follows spin.y · −1.2732
at 0.125 a call, lean at 0.25. A crash when |impact| > 100 (the impact is kept
signed and compared by size, then halved each substep), the speed changes by
more than 44 ft/s in a frame, up.y ≤ 0, or a solid is touched for over 0.5 s
(`f_814` adds the frame while the bike's collision object has a contact whose
type `f_134` is not 100 bike, 101 rider or 1000 a tree's .col; scene objects
are type 0, the eco-system's prisms 1001). After a crash the rider runs the
full Vehicle model (section 5) until reset.

Before those, 0x414000 also runs the player's crash rules on the computer
riders (v90 0x40bc50 → 0x40aeb0 → KrustyBike 0x492650 → 0x40a950). Most read
tyre state the two-wheel model never sets; the ones that fire are:
- **Speed lost in a frame** (0x492950, computer branch): crash when
  (speed − last speed) · K < −1000 · dt, K by level 1–3 from
  `{4, 3.25, 2.5}` in Supercross (event 3) and `{3.75, 3.25, 2.5}` elsewhere.
  At level 2 and 35 fps that is about 9 ft/s, which is what throws the
  computer riders on most hard landings: 21 of the AI tape's 31 crashes are
  landings with no trick running.
- **Landing mid-trick** (state 9): the other 9.
- **Landed on** (crash 2): another bike more than 1.5 ft above it touched it
  (0x405fb0 sets `f_608` on the lower bike; side by side they only bump,
  the impulse scaled 1.0 to 0.2 by how far behind the contact is).

The bike's collision object is swept against the solids each frame as the
player's is (0x48ea00 → 0x52fc60), pushing the bike out.

**Air poses.** The AI tape's pose bytes are `(f_650 & 15) | f_658 << 4`,
`f_65c`, `f_660`, `f_654` (0x4936dd). In the air the computer riders play the
cross-up rows 11–12 (CrosOv1–2) at phase 0, from their first frame off the
ground until a few frames before touching down (84% of air frames).

**The controller (0x415170)** each frame, before 0x414000:
- Lap races (events 2, 3): the line is 100 points along the course from the
  rider's confirmed place (probe pos + 3.5 look, found 0–100 ft ahead or 0–50
  ft back, confirmed when 0–100 ft ahead), at
  t · speed for t = 0, 0.1 … 9.9 s, each at bike.y + 2; off the course one
  point 100 ft on. Limit 1000, power by level (bike +0x7c4/7c8/7cc = 1.1, 1.3,
  1.5, set at 0x490c60), look-ahead speed · 0.25, which 0x40f310 widens by
  the distance across the line in lap races. Finished riders pull aside
  at 40 ft/s. Over 6 s off the course (0x492ac0) puts them back.
- Every rider's confirmed place starts at the start line's place, t + 0.1
  (0x4eb170 fills it when the found place is empty). The gate is about 32 ft
  behind the line on SX01, inside the 50 ft the tracker looks back, so the
  riders count as on the course but the confirmed place stays on the line
  until they cross it: the line starts there, and with the widened
  look-ahead the riders turn gently into their lanes (0.1 to 0.2 rad, as
  the tapes show) instead of at the 45° limit.
- Gate races (events 1, 5): straight at the gate from 300 ft, otherwise
  through gate ∓ speed · look; the limit drops to 40 when the gate is more
  than 45° off.
- The Stunt Quarry: wander. Trick chance 2% a frame in races, race +0xa4 in
  the quarry; the wait after a trick is 25/15/5 s by level in the quarry
  only (in lap and gate races the wait is one frame, see below).
- Lane ±7 ft from rand (±3 in Baja and Enduro). Engine class by cc: under 126
  → 0, under 500 → 1, else 2.

**How close it is.** `mm2-viewer TRACK --screenshot /dev/null --riders 5
--level 2 --lanes 0,0.5,4,-6.5,-1 --ai-check ai_race4.csv --from 10,20,..
--for 2 --ai-csv OUT` starts the port's riders from the tape's states and runs
the tape's frame lengths. Over 61 two-second runs on SX01 at level 2 (the
tape's level; level 3 is too quick off the line and level 1 too slow) the
median position error is 0.7 ft after 1 s and 1.9 ft after 2 s, 38 runs under
a foot after 1 s. Whole 200 s races give the same mean speed (about 61 ft/s)
and top speeds 91–95 against 88–93. With the speed-loss rule the replays'
two crashes in the original happen in the port at the same moments (one
extra in the port); a whole race gives 40 crashes over the four computer
riders against the tape's 31 (44 since the trick wait fix), and fewer tricks
(16 against 25; 12 before that fix).

Later checks against the decomp, all now in the port:

- The speed-loss rule (0x492950) matches: the tape's crashes sit at a
  threshold of about dt at K = 3.25. The solids and bike bounces set the
  speed afresh (0x52bb10, 0x52bc20) and the frame's last speed becomes that
  one, so a bounce's speed change never counts as the next frame's loss.
- The spin is kept in the bike's own axes (+0xd8) and turned back into the
  world's each frame (0x414000).
- Crash reset 0x40b010: the clock (`f_454` = 2.0 s, 3.5 for crash 2) runs
  only once the thrown rider touches (Wrecker +0xac, set at 0x535d50); at 0
  the rider is put back where the wreck lies, so the course tracker follows
  the wreck. The game also restarts the clock while it is above
  `f_458` − 0.5 if the rider is clear again; the port's one-sphere rider
  bounces clear far more often than the game's, so the port leaves that out
  (crashes then last 3.1 s against the tape's 2.96, 4.5 with it).
- Bike to bike (0x405fb0 for type 100, 0x52bc20 → 0x505330,
  `mm2_physics::bump`): each bike's collision points are swept against the
  other's `honda.col` meshes (frame 104 triangles, each wheel 36), pushed
  out, and both take one impulse `j = −(1+e)(va−vb)·n / (n·[(Ia⁻¹(ra×n))×ra
  + (Ib⁻¹(rb×n))×rb] + 1/ma + 1/mb)`, e = 0.1. The point velocities are
  scaled by where the touch is against the heading: under 0.5236 rad 1.0,
  to 2.09 0.8, to 2.62 0.6, beyond 0.2 (past π the game's angle goes
  negative and counts as 1.0). The game adds +j to both bikes' spins. A
  bike more than 1.5 ft below the other is landed on (`f_608`, crash 2).

With these, 12 whole races from the tape's start give 35.9 crashes and
16.7 tricks on average against the tape's 31 and 25. Each event checks
out on its own (landings from a 2 s lead: median peak ratio 0.99, crash
outcomes 37 both, 14 port only, 12–14 tape only), so the gap is the race's
pace: the port's laps take 69–76 s against 63–69, its start pulls less from
45 to 65 ft/s, and it runs about 4 ft/s slower through the SX01 rhythm
section, so fewer long flights and fewer tricks. Replays started with
`--for 2` begin with the springs at 0, which lowers the first takeoff; use
a lead of 2 s or more.

**Thrust tables** at 0x56d05c: 3 × 800 bytes, f32, indexed by integer speed in
ft/s (0..199, 154 entries non-zero). They are force-like values that fall with speed:

| class | at 0 | at 153 |
|---|---|---|
| 0 | 39.4 | 7.06 |
| 1 | 40.0 | 7.40 |
| 2 | 56.4 | 7.70 |

Dump them straight from the exe at RVA 0x16d05c, for example with pefile `pe.get_data(0x16d05c, 2400)`.

## 5. Player model: Vehicle::Simulate 0x52ffb0 **[structure solid, some details likely]**

Fixed-step accumulator, 0x506b20, and the step loop 0x52ffb0:

```c
t = leftover + dt; n = int(t / 0.02);
if (n > 0) { leftover = t - n*0.02; n = min(n, maxSteps); h = 0.02; }
else       { leftover = 0; n = 1; h = (dt == 0) ? 0.02 : t; }
for (i = 0; i < n; i++) Step(h);
// 0x52ffb0, after the last whole step: the rest of the frame is one more step
if (leftover > 1e-4) { Step(leftover); leftover = 0; }
```

**The rest of a frame is a step of its own (2026-10-24).** A 0.022 s frame runs a 0.02 s step and
then a 0.002 s one (memory watch cb_plain: f_144 = 0.022, f_13c = 0.002, f_1e0 = 0), so per-step
factors like the 0.9 air spin damping act twice that frame (0.81 a frame there, 0.9 in jfland's
shorter frames). Only a rest of 1e-4 s or less waits for the next frame. The shocks (0x52e520's
spring-back and 0x519350's damper) take f_1e8, the length of the step before, not f_13c: they measure
how far the shock moved since it was last evaluated, which happened over that step. Without it the
short step saw the 0.02 s step's motion as a jolt ten times as fast, and a tyre over a crest dropped
off the ground every other step (NAT12 crest brake take: tyres down 86%/87% against the original's
98%/96%; with f_1e8, 98%/93%).

So a frame shorter than 0.02 s (above 50 fps) runs one step of the whole frame time, not none: the
original's step length follows the frame rate there. The comparison tapes sample once a frame, about
every 0.017 s (in the air vy drops by 32.2 × the sample interval each sample), and memory watches read
f_13c = h at 0.014–0.019 s. So the tapes were made with steps of about 0.017 s, not 0.02 s.
**Live play steps 0.02 s (2026-10-23, deliberate).** The per-step factors (spin damping ×0.85 on
the ground and ×0.9 in the air, the speed-loss crash limit) make the game frame-rate dependent above
50 fps: the memory watches under CrossOver show frames as short as 0.001 s each damping the spin by
0.9. The game was played at 50 fps or less, and the tapes ran at about 0.017–0.021 s. At 120 fps the
port's speed rms against the tapes was 1.91 and pitch rms 0.057; with whole 0.02 s steps (the bike
drawn between its last two steps) 1.39 and 0.050, close to the 1.54 and 0.047 with the tapes' own
frames. So the race steps the player in whole 0.02 s steps (`Bike::fixed_step`, `Race::fixed_step`);
the scripted runs (`--ride`, ghosts, network tests) keep the game's accumulator.

**Bottoming against the terrain (2026-10-24).** When a shock bottoms (tyre +0x15c, bike f_5a0), the
step 0x52ffb0 calls 0x43b9d0 on the bike's collision records (f_12c, f_130 of them) and 0x5050b0 through
0x52bb10: the body is pushed out along the records' mean normal by the deepest one, and one impulse
(restitution f_14c, spin scaled 0.3/0.1/0.3) acts at their middle. The port had pushed and kicked at the
bottomed tyre's own contact, 2.7 ft ahead of the centre on the fork, which threw the nose up off faces: from
the original's state on SX01's second jump (memwatch7 jfland, 420 ms) it left at 0.75 rad against 0.49.
With the bike's .col points (body and both wheels) tested against the terrain instead it leaves at 0.53,
and the SX04 face at 0.61 against 0.60 (was 0.63). Over the local runs with three start headings (0,
±0.001 rad) the crash outcomes match 54, 56 and 56 of 78 (were 52, 55, 52). A 0.001 rad heading change
alone flips 9 runs' outcomes, so single-run scores move by about 3 from chance.

**Spin damping after the turn (2026-10-23).** In 0x52ffb0 the ground damping (0x530825) and the air
damping (0x40bcc0) come after 0x52fd10 has turned the bike, so a step turns by the undamped spin; the
port had damped first. Replays from the original's state: SX04 face pitch error 0.036 → 0.009 (takeoff
0.63 against 0.60, was 0.77), jump_flat lip 0.022 → 0.009. Local runs at 120 fps with fixed steps:
52/78 crash outcomes (50 before), speed rms 1.22 (1.39), pitch rms 0.051 (0.050). Still open: the
SX04 straight landing and jump_flat's jump 2 crash in the port with 0.02 s steps (memwatch7 has the
original's landings, but the gas was off in those takes from about 0.5 s into the dump).

The port stepped 0.02 s whatever the frame until 2026-10-22; it now runs this accumulator
(`Bike::step`), the race hands the player's bike the whole frame, and the scripted viewer takes
`--fps N` or `--frames FILE` (a tape's frame times) to step as the tape did. Started from the
original's state on the SX04 jump face (`MM2_STATE`, read from a memory watch) and stepped with the
original's own h, the port leaves the face at pitch 0.58 against the original's 0.61 (0.66 with
0.02 s steps) and its vertical speed in the air matches to 0.2 ft/s.

`Step(h)`, in call order. Before the loop, 0x43bd20 runs once with no motion to seed the contacts.

1. **Drivetrain** 0x4d76a0 / 0x4d7780: ramp the throttle, pick the gear, compute rpm and torque (section 6).
   The inputs are read here through v80 (gas), v81 (brake) and v82 (low-speed button).
2. Rest clamp: if speed < 0.001 (or NaN), set v = 0.
3. `v53` 0x40ddf0 → Bike 0x40b340: bike pre-step (rider weight shift from lean). **[likely]**
4. **Tyre contact** 0x52e520 → Tire 0x519c30 for each tyre **[solid]**. It samples the terrain, then the solid objects
   (the object probe below).
   With p0 the wheel centre, A = look × up the axle (the front's turned by the bars) and R the radius:
   N0 = normal under p0; fwd = normalize(N0 × A); d = fwd × A, negated if it faces along N0; c1 = p0 + R·d;
   (h1, N1) = terrain at c1; depth1 = (h1 − c1.y)·N1.y. A second probe at p0 + R·fwd + R·sin(roll)·
   normalize(fwd × Y) gives N2; d2 from N2 the same way; c2 = p0 + R·d2; depth2 likewise. The deeper one
   is the contact, with the normal sampled there, and fwd is rebuilt from it (+0x150 depth, +0xe4 normal).
   The object probe (end of 0x519c30): a ray from p0 along the chosen d, R long, goes to the collision
   world (0x51af00 builds it, 0x4367f0 / 0x439e10 run it, the hit lands in tyre+0x58/+0x5c as a fraction
   t, the face and its normal). If (1 − t)·R beats the terrain depth, the contact moves to the hit point
   (0x42b430), the normal to the face's, the surface to 7 and fwd is rebuilt (0x51b310). Surface 7 reads
   `TerrainZone8`, which no track has, so its grip is 1. The port casts R + 0.1 ft and lets a wheel the
   probe put on an object skip that wheel's collision points, whose rims (1.16 / 1.12 ft) stand just past
   R = 1.13 and would otherwise lift the bike off before the tyre touches. On Ski Lodge's bridge Enduro 2
   now rides across like the original (speed RMS 2.25 → 0.39 ft/s, crash into the semi at 2.24 s against
   2.21 s; 2026-10-22).
   Before it, the shock relaxes by travel / (0.25 s) × dt × 0.25 (0x4ff100 inline, 0x4ff950 rotating;
   the 0.25 s is the shock ctor's last argument), and by × 0.75 more and a second sample if the tyre is
   clear. Each sample (0x519c30) is taken where the rim meets the ground. From the axle, the tyre's
   radius goes down the wheel's plane against the normal under the axle, and the ground is sampled
   there (depth = (ground y − point y) · normal y). A second try takes its normal from a point a
   radius ahead, offset by sin(roll) · radius sideways. The deeper of the two wins, so a face is met
   before the axle is over it. The point on the rim is the contact (+0xcc). The port does this since
   2026-10-22. (The direction of the probe's sideways offset is a guess: toward the lean.)
   The port does the relaxation since 2026-10-22 (`relax()` in bike.rs): the tyre touches only when the ground
   reaches the wheel where it is, so a wheel leaving a crest stays tucked up (the memwatch on SX04 shows the
   front going from 0.88 to 0 in about 0.24 s once clear, 4 travels a second).
   The relaxation in full (0x52e520): each tyre's shock first extends by `travel / (tyre+0x2a4 · shock+0xac)
   · h · 0.25`, where +0xac is 0.25 s and tyre+0x2a4 is the bike's f_4b0 = cos(roll) (0x52dfcc; 1.0 from the
   ctor, 0x52b104). So a touching tyre's shock extends at only its travel per second. The ground is then
   sampled (0x519c30); if the tyre is clear, the shock extends by three times that again and the ground is
   sampled once more. 0x4fea60 then marks the new compression as the previous one (+0x90), but the damper's
   speed uses +0x8c, which only 0x4fee80 updates, so the extension is not felt as shock movement. Ported on
   its own (2026-10-22), it left 44 of 62 comparison runs matching their crash outcome, as before, but the
   mean speed error went from 2.3 to 2.9 ft/s (enduro1_straight and nat1_straight diverged after their
   jumps), and it did not reproduce NAT12's crest freeze, so it is not in the port yet.
5. **Body collision points** 0x43bd20 **[solid]**. For each CollisionPoint, take its world position
   (from the frame, local point +0x8), copy it, and snap the copy to the ground with Terrain 0x50d3b0,
   which writes the normal (+0x2c) and the surface type (+0xbc, low 3 bits).
   `depth = (snapped.y - p.y) * n.y` (+0x98). The point is in contact if `depth >= threshold` (+0x9c).
   Friction (+0x8c) comes from a per-surface table, or 1.0.
6. 0x52f090 → Tire 0x518cd0 (contact velocity, forward speed into `f_434`), then Tire 0x519350
   (suspension: InlineShock 0x4ff1f0 for the front fork, RotatingShock 0x4ffa50 for the rear swingarm).
7. Bike forces, all accumulated into the step's force F:
   * v6 0x40c210, **air drag**: `Fd = -f_148 * |v| * v`. It is limited by a shared friction budget
     to `m/h * min(|v|, budget)` and subtracts what it used from that budget.
   * v7 0x406ee0 sets the front/rear load split. With both tyres down each tyre's share is **its own**
     distance from the centre of mass over the sum (`rear = d_rear/(d_f+d_r)`), so the farther tyre carries
     more; with one tyre down it carries all of it. `tyre+0xfc = load * F`, where F so far is gravity
     (`f_18c = -weight`) plus drag; the tyre's normal force is `N = max(0, -dot(tyre+0xfc, n))`.
   * v72 0x406bd0, **cornering** (the front down, or the rear with the front within 0.2 ft, and the rear
     within 0.2 ft either way; otherwise f_4b8 and f_43c are zeroed). The side axis is `normalize(cross(n_avg, fwd))`.
     It calls 0x52d800 with the grip `min((frontGrip+rearGrip)/2, 1)`, which turns the velocity
     kinematically, like a bicycle:
     ```c
     R = wheelbase(f_4ec) / sin(steerAngle);          // 0x52d7c0
     dv_side = fwdSpeed(f_434)^2 / R * h * min(1.5*grip, 1);
     v = normalize(v + side*dv_side) * |v|;           // speed kept, heading changes
     f_4b8 = headingChange; f_43c = f_4b8 / h;        // used for the lean target
     ```
   * v86 0x492b20, **rear drive force**, through 0x4d7900:
     `F = hp(rpm) * gearRatio[gear] * 341.62 / max(speed, 1.01)` along the rear tyre's forward axis,
     times the throttle input (`f_4a4`, 1). hp(rpm) is piecewise-linear over the presets.pb curve (section 6).
     **Wheelspin** 0x519670, below 45 ft/s rear forward speed and not crashed:
     ```c
     x = drive/m - |accel(f_70)|;
     s = x < 1.01 ? 1 : (400 - min(x, 320)) * 0.0025;
     g = 1.05 - lean * 0.25;                                      // 0x40d9b0
     k = (prevLook.y(f_a4) > 0 && cos(pitch)(f_60) * 1.25 > 1) ? cos(pitch) * 1.25 : 1;
     t = clamp(s*g*k, fwd/45, 1) * surfaceGrip;
     tyre+0x2b8 = t;  drive *= t;  tyre+0x2bc = 1 - t;
     ```
     So with the nose up by less than 37° the rear grips up to 1.25× better, and on a 0.9 zone (snow) a
     level bike's t reaches exactly 0.9, which is not below the power slide's 0.9. f_60 is cos(pitch) from
     0x4ba1a0 (f_48..f_50 roll, pitch, heading; f_54..f_60 their sines and cosines). An earlier version of
     this doc read k as `min(…, 1)`, which kept Enduro 3's start sliding: fixing it (2026-10-22) took
     enduro3_straight's speed error from 35 to 0.3 ft/s and the 75 local runs' mean from 3.6 to 2.2.
     The low-speed button adds `(0.4762 - f_664) * 1010.67` while `0.12 <= f_664 < 0.4762` (a launch boost, **[likely]**).
     A global option (`*(0x5743ac)+0x2d70`) halves the drive force and load when `f_78c == 0` **[likely: a beginner setting]**.
8. **Collision response** 0x43b9d0 (when the body points touch): average the contact points and normals,
   take the deepest depth d, and compute `r = avgPoint - pos` and the contact velocity `v + ω×r`. Then
   push the body out: `pos += d*n` (both the frame translation and f_c). The scale s is (0.3, 0.1, 0.3) while
   riding and 1.0 when crashed, applied in v3 0x48eba0. Afterwards there is a speed clamp: if the speed grew
   by more than 1.3× and is above 5, scale it back to the old speed. If it is below 0.1 and the old speed was
   below 0.001, stop the bike.
9. 0x506ce0, **contact forces** **[solid]**: for each active point,
   `fn = k(+0xa0) * dot(F, n)`. If fn < 0 (pushing into the ground), the reaction is `-fn * n`. A virtual on the
   point adds friction. The forces are summed into F, and `torque += R_body * cross(r, f)`.
10. **Traction / brake** 0x52d070 → Tire 0x519850:
    `F = zoneFriction(tyre+0x144) * 255(+0x2a0) * brakeRamp(+0x29c)` per braking tyre along `-fwd`, capped by
    `m/h * min(budget, fwdSpeed / nBraking)`. The ramp rises at `f_534*10`/s and falls at `f_538*25`/s
    (0x52e8f0). Its torque is scaled by 0.5 front, 1.0 rear. That torque goes to its own accumulator (the step's
    local +0x74, not the shocks' +0x4c), which the integrator 0x506ed0 hands to vtable +0x3c, 0x496c60: turned
    into the body frame and times the inverse inertia, it is applied by 0x4cfe30 like static friction, per
    axis: with the spin still it only cancels the other torques' opposite part up to its own size (and does
    nothing alone), and spinning it adds unless it would carry the spin through zero, which instead stops it.
    So hard braking on the flat doesn't dip the nose: the tapes hold pitch at 0.022 through a full stop,
    with spin exactly 0, where the port used to dive 0.09 rad. The port counts a spin under 0.001 rad/s as still. Then each tyre's contact model (vtable on
    tyre+0xb8, 0x51b1d0) gives **rolling and side friction** against the contact velocity:
    ```c
    cos = |dot(v̂, tyreFwd)|;  sin = sqrt(1 - cos²);        // tyre+0x1d4, +0x1d0
    F = -(2*p9*sin + 0.15*cos) * N * zoneFriction * v̂;     // p9 = 0.1 front, 1.0 rear
    ```
    0x51b1d0 runs on the contact sub-object at tyre+0xb8, so its fields sit 0xb8 below the tyre's: the
    rolling term is +0x270 × +0x1d0 and the side term +0x1cc × +0x1d4, which the constructor 0x5185f0
    fills from its 3rd, 8th, 4th and 9th arguments. The one call (0x4092f7) passes 0.15, 1.0, 2.0 and
    the per-tyre p9, which confirms the formula above; N is tyre+0x12c and the zone friction tyre+0x144.
    The result is capped by `m/h * min(budget, slipSpeed)`. Every resistance shares one budget that starts at the speed,
    so together they can never reverse the bike in one step.
    tyreFwd is each tyre's own forward (+0x230), so the front's is turned by the bars and a steered front
    scrubs speed. The port measured both tyres against the rear's until 2026-10-22; switching took the 75 local
    runs' median speed error from 1.17 to 0.83 ft/s and steer_right's from 1.17 to 0.48.
11. **Shock spring** 0x52ea50 → 0x4feb20:
    ```c
    k = lerp(kMin(+0x9c), kMax(+0xa0), compression(+0x70));  // progressive spring
    c = 2 * zeta(+0xa4) * sqrt(m * k);                       // critical-damping ratio
    F = -k*x - c*v;  if (|v| < vThresh(+0xa8)) ...           // small-velocity cutoff
    ```
    The compression is measured along the shock's own stroke (0x4ff1f0 inline, 0x4ffa50 rotating): the
    depth under the axle over the cosine between the stroke and the ground's normal, so the fork, raked
    about 28°, reads about 13% more than the tyre sinks and bottoms sooner. The spring and damper act along
    the stroke and only their part along the normal pushes. The rotating shock's stroke is the tangent of
    the swingarm's arc; its travel is the chord `2·L·sin(0.5585/2)`. (The port's swingarm swung down as it
    compressed until 2026-10-22.)
    Nothing clamps the sum (0x4feb20 adds the spring and damper vectors; 0x52eac0 projects them as
    they are), so a shock springing back out faster than the spring pushes pulls the body toward the
    ground. Over a crest that keeps the front on the ground while the nose drops: on NAT12 the port's
    front now leaves the ground by at most 0.06 ft, against the original's 0.05 (0.27 with the old
    clamp). Over the local runs the mean speed error went from 1.80 to 1.31 ft/s.
    The damper's `v` is set in 0x4fee80: `|Δcompression| / h`, clamped to the bike's speed along the shock
    axis (`|v·axis|`, passed in as `|proj v|·dt`). A rolling wheel whose ground steps up under it therefore
    doesn't read as a hard hit. Compression ≥ travel sets the bottomed flag (+0x50). The port clamps the same
    way (`bike.rs`, shock()). On the comparison tapes this took NAT01's speed error from 8.5 to 2.8 ft/s RMS,
    Enduro 3's from 21 to 11, and moved straight_long's tent-roof crash from 10.15 s to 11.65 s (original 11.58).
12. 0x52eac0 projects each tyre's shock force onto the shock axis, `(dot(F,a)/dot(a,a))·a·f_4b0`, and adds
    it to F. The torque from it is turned into body space and **only its x (pitch) part is kept**.
    0x52f290 adds each driven tyre's drive force (+0x248, already cut by wheelspin) and its torque about the
    centre, `cross(contact offset +0xf0, F)` through vtable +0x130. The player's bike (0x492c50 → 0x407660)
    scales that torque: none with the bars-up pose (+0x734) or below 10 ft/s pointing up past 0.8727 rad on the
    front; 0.2 when last step's pitch spin (f_d8) is below −1 (nose rising faster than 1 rad/s); otherwise
    `1.6 · (1.4 − min(|pitch|, 1)) · max(0, −(f_508 + 0.125))`. So the drive only lifts the nose with the rider
    leaning back (f_508 negative): on lean_ground, holding lean back at 80 mph raises the pitch 0.068 rad in
    0.45 s, and the port matches it to 0.003. f_508 (the eased lean) is also what the wheelspin's
    `1.05 − 0.25·lean` (vtable +0xbc, 0x40d9b0), the pump (0x40d3d0) and the air pitch (0x40bcc0) read; the port
    used the raw input for those and now uses the eased value.
13. **v14 0x497590, velocity integration** **[solid]**:
    ```c
    alpha = invInertia * torque;  omega += alpha*h;  omega *= 0.999;
    if (race+0x18a)  v += F * f_24 * h;            // race running: 0x506ed0, all of F (f_24 = 1/m)
    else             v.y += F.y * f_24 * h;        // before the start: held at the gate
    speed = |v|;
    ```
    race+0x18a is set by the race code (0x41e0b0) when the start countdown ends. Only the pre-start mode is
    vertical-only; in the race the drive, brakes, friction and drag all move the bike. The mass comes from the
    bike's weight (0x505760): `f_158 = m = W * 0.0310559`, `f_24 = 1/m`, gravity force `-W`, and the shocks'
    damping mass `f_4dc = (0.9*W)/32.2`. W is the category's `Weight` in presets.pb (section 6).
    Inertia is a box from the model bounds: about `m/12 * (89.3, 58.9, 41.6)` ft² for pitch, yaw and roll.
14. **v61 0x40ce00, yaw kick** (returns a yaw angle per step, applied about the ground normal with the
    cornering heading change). An earlier version of this doc called it a pitch kick and a wheelie; it is a
    **power slide**:
    * **Power slide** while the rear spins up (`rear traction +0x2b8 < 0.9`, drive `+0x280 > 0`) and the bike is
      leaned (`|roll f_4ac| > 0.005`). f_4ac (0x52dcf0) is the roll **against the ground**: the angle about
      the wheel-to-wheel line (0x40dc60) between the bike's up and the touching tyres' average contact
      normal (0x52da50), or the world roll f_48 when no tyre touches. So an upright bike on a side slope
      counts as leaned, and spinning up there slides it (Enduro 3's start sits on a 5° slope: the original
      rolls 0.087 and swings right from the first frame). currentLean 0x40b220 is the world roll f_48 below
      32 ft/s (f_b8); faster it is `f_4ac - (f_4ac - f_48) * max(0.5236 - |bars|, 0) * 1.9099`, so the
      ground's roll counts in proportion to the bars up to 30°. f_4b0 = cos(f_4ac). Ported 2026-10-22, but the
      port's rear still keeps traction ≥ 0.9 at that launch (0.95 on the first step, whose drive the
      throttle ramp halves), so it does not slide there yet: the original must spin the rear harder in its
      first steps. The kick:
      `kick = (0.5*L*k + 0.5*L) * invI_yaw(f_e8) * h * k * drive * 32` (L = wheelbase f_4ec, k = 1 - traction
      = rear+0x2bc), capped at 1.57, then times `min(5*|roll|, 1.5) * h`, signed like the roll. It is set fresh
      each step the slide holds, and when it stops it decays linearly over `round(0.75/0.02 - 0.5)` = 37 steps:
      `f_62c = 0.02 * -1.333 * kick` per step.
    * Otherwise, **alignment**: rear forward speed (+0x148) > 7, the rear slipping (+0x288 ≥ 0.0001) and the
      front not (< 0.5): yaw toward the velocity by `asin|cross(rearVelDir, rearFwd)|` when that is < 0.4795,
      at most 0.8·h per step, and only if that is at least this step's cornering turn |f_4b8| (0x40cfa0; the
      step passes |f_4b8| in): while the bars turn the bike faster than 0.8 rad/s nothing straightens it, so a
      slide held through a turn stays (brake_turn_clean: slip 0.2–0.4 after landing under brake and steer,
      which the port lost within 0.3 s until 2026-10-22; speed error 5.6 → 3.5 ft/s). rearVelDir is the tyre's contact-velocity direction +0x108, which 0x518cd0 sets at the start of
      the step, before the cornering bends the velocity. Measured after the bend (as the port did until
      2026-10-22), the heading ran one step's turn ahead of the velocity: ~0.09 rad in a fast turn where the
      original's is ~0.01, so the bike snapped back early when the bars reversed (sx12_turn speed error 13.4 → 3.2).
15. v46 0x40c400 picks the **pitch pivot**: on the ground it is a point on the line between the tyre
    contacts, `lerp(front, rear, clamp(rearSusp(+0x2b8) - 0.4, 0, 1))`, or the front contact for a stoppie.
    In the air or in a collision it is the centre of mass (f_194).
16. v95 0x52fd10 **rotates the body** by `|ω|·h` about `ω/|ω|` around the centre of mass (0x502080 =
    rotate frame about a pivot).
    The centre of mass follows the rider's fore-aft lean (0x407570, not ported yet): f_640 is a lag
    filter, time constant `6.3 · step` = 0.126 s, run in the load split 0x406ee0. It eases toward
    `0.5 + 0.25 · f_508`, and the centre's body z is moved by `(f_640 − 0.5) · 4`, up to ±1 ft. Its x is
    set to 0. f_18 is then rebuilt from the frame. The tapes put f_18 at the centre of mass: the
    physics velocity matches the motion of a point about 2.2 ft above the model origin.
    It runs every step from 0x4925b0 (constants 0.5, 4.0 at 0x5558ac/0x5558b8), skipped while f_444 is
    set. Read literally it would add up to 1 ft per step, so the frame must take the shift back each
    step. The lean_ground tape shows no trace of it: across 1.5 s of forward lean, f_18's height
    (5.18) and its place along the path (within the tape's ±1 ft sample jitter) don't move, and the
    pitch stays at 0.022. Not ported, because nothing in the tapes depends on it.
17. Mode from v62 0x40d200 (decoded 2026-10-22; f_4a8 counts the touching tyres, tyre+0x260, f_5a8 =
    both touch, f_5a4 = any): crashed (+0x700) → 2; none touching, or crashed (f_444) → 0; both → 1. One
    touching: if f_43c or the bars are non-zero → 1; otherwise, rear touching → 3 if the front's +0x150 is
    above −5.2 ft, else 0; front touching → 1 if the rear's is above −5.2, else 0. The step (0x53079d) runs
    the air handler when f_108 is set, else mode 1/2 rotates by the kick and damps ω by 0.85, and mode 1/3
    leans; mode 0 does neither. So on the rear alone going straight (a crest, a wheelie) the spin is not
    damped and the nose drops freely: NAT12's crest pitch now follows the original (−0.20 rad at 0.9 s on
    both). The power slide and the rear-slip alignment in 0x40ce00 also need both tyres (f_5a8). Over the
    75 local runs: crash outcomes 51 → 49 (Baja 2's handlebar now clips a gate post by 0.07 ft the original
    clears), mean speed error 3.47 → 3.29 ft/s, nat_turn 6.8 → 3.8, enduro1_straight 8.4 → 3.7.
    * **Damping only on a turning step** (0x530825, decoded 2026-10-22): mode 1/2 rotates the body about
      the pivot and damps ω by 0.85 only when the bars' turn this step |f_4b8| ≥ 1e-4 rad (set by 0x52d070,
      zero without steering) or else the yaw kick |0x40ce00| > 1e-4. Riding straight with no kick, neither
      happens: nothing holds the pitch spin. The port damped every ground step, so a front tyre hitting a
      face (the rear lifting, mode 1) pitched up at ~0.6 rad/s where the original reaches 1.8; Nat3's start
      stalled on its first bump instead of surging to 27 ft/s. Over the 81 local runs the mean speed error
      went 1.95 → 1.73 ft/s (nat1_straight 14.9 → 8.9, nat2_turn 4.6 → 1.0, straight_long 4.2 → 1.5), crash
      outcomes 51 → 50: jump_flat and sx08_straight now leave their ramp faces with 2-3× the original's pitch
      spin, because on the face the port's rear lifts where the original's keeps touching and aligning.
    * **A bottomed tyre takes an impulse** (decoded 2026-10-22). A tyre past its travel sets +0x15c, so its
      contact point joins 0x43b9d0's collision points: the body is pushed out by the deepest point along the
      averaged normal, and 0x5050b0 (vtable +0xc of the body, via 0x5061a0) applies
      `j = −(1+e)·vn / (1/m + n·((R·(invI ⊙ Rᵀ(r×n))) × r))` with e = +0x14c (0.1, read from the running game; 0.1 crashed),
      vn = n·(v + (ω_world ⊙ s) × r), Δv = j·n/m and Δω_body = j·(Rᵀ(r×n) ⊙ invI) ⊙ s, where s = (0.3, 0.1, 0.3)
      ridden and (1, 1, 1) crashed. The port used to stop the body's whole speed into the ground at the
      tyre, with no lever. A front tyre hitting a ramp face then launched the bike to full climb speed in
      one step (jump_flat: vy 3 → 33 ft/s where the original goes 7 → 15 → 28). Over the local runs the mean
      speed error went 1.73 → 1.65 ft/s (baja3_turn 7.3 → 1.0, straight_long 1.5 → 0.9), with crash outcomes
      50 → 49.
    * **On the ground** (mode 1/2, turning): rotate by the kick about the pivot, then ω *= 0.85. Then
      v92 0x40c830, **lean** (mode 1/3):
      ```c
      target = (f_43c*0.2 + steerAngle*0.05) * f_4b4;     // lateral accel + steer
      if (kick) target = max(|target|, 0.087) * sign;     // at least 5° lean while power sliding
      target = clamp(target, ±f_580 /*0.698 = 40°*/);
      d = currentLean(0x40b220) - target;   // see below
      d = clamp(d, ±f_584 /*2.094 rad/s*/ * (sameSide ? 0.8 : 1) * h);
      rotate body about its forward axis by d (about the pivot);
      // any spin opposing the correction is cancelled by 1.5*d/h, never past 0
      ```
    * **In the air** (f_108), v91 0x40bcc0:
      ```c
      pitch = f_528 * lean * h * 0.83;   yaw = steer * h * -0.2;
      if (f_430) { pitch *= 0.5; yaw = 0; }
      if (f_6fc && rear(+0x150) < -2) {                  // air pose on, rear 2 ft up
          if (BrakeGyro) pitch += 0.83*h*brake;          // brake: nose down
          if (GasGyro && gas && !lowSpeed) pitch -= 0.83*h*throttle;   // gas: nose up
      }
      // trajectory bend, inside the same `rear more than 2 ft up` test:
      //   if 0.55 < dot(v̂, look) < 0.984, push = (1-dot)*3.41297*|v|*0.075*h*side,
      //   v += push*(v̂.z, 0, -v̂.x);  v *= |v| / (|v| + push)
      // side = f_65c < 0.5 ? +1 : -1. f_65c is the rider's steer pose phase (0x407c60), which
      // restarts at phase 1 (0) on take-off: steering left (f_504 > 0) moves it to phase 3/4 and
      // side -1, steering right back to 1/2 and side +1. So a straight flight always loses speed,
      // as the tapes show (sx04: 51.3 -> 47.9 ft/s across a 1.5 s jump, sideways and back), and
      // only after a left steer does the push speed the bike up. The scale-down is by
      // |v|/(|v|+push), not back to the old speed.
      rotate about the centre of mass by |(pitch,yaw)| around the body axis (pitch, yaw, 0);
      omega *= 0.9;
      // auto-level roll: omega.z = sign(f_2c) * min(|f_2c|/h, 0.357792*t^3) (+ a decaying wobble)
      ```
    * Then v90 0x40bc50, the bail check (see section 8).
    * **Air pose and cross-up landings** (0x407900, once a frame, `ret 4` with the frame time; decoded
      2026-10-22 and checked against a memory watch of a NAT01 air-steer landing). The pose is on (`f_6fc`)
      while f_108 is set, the front tyre's +0x150 is below −2 and the rear's below −1 (clear of the ground).
      Turning on, it zeroes the lag `f_644[0]` and sets `f_520 = 1`. Each frame the lag follows the bars,
      `x += (f_504 − x) · min(dt, 0.55) / 0.55` (`f_704` = 0.55, the step clamped to ±1), and the phase
      `f_65c` follows x by `f_520`:
      1: `p = −x/4`; p < 0 → 3 with `0.5 − p`; p > 0.225 → 2 with `0.5 − p`. 2: `p = x/4 + 0.5`; p > 0.5 → 3.
      3: `p = x/4 + 0.5`; p < 0.5 → 1 with `0.5 − p`; p > 0.725 → 4 with `1.5 − p`. 4: `p = 1 − x/4`; p > 1 →
      1 with `p − 1`. So full right holds the phase near 0.25, full left near 0.75, and letting go swings it
      on through 0.5 or 1. The bike model plays CrosBik1–3 (bCross0x.MOT, 61 frames, all three share the
      root node's track) at frame `f_65c · 60`; the root turns about the bike axis (0.175, 0.646, −0.743) up
      to 1.68 rad at frames 15 and 45. Each frame 0x40ccc0 stores the drawn root's angles in +0x48..+0x50
      (the physics angles +0x2c..+0x34 otherwise). When the pose ends (the else branch calls vtable +0xa4,
      0x40d9d0), the game reads the root node's world look/up (0x501380), sets the model and the physics
      look/up from it (0x500e90, vtable +0x88), rebuilds +0x2c from them and clears f_6fc and f_430: **the
      bike itself takes the cross-up turn**, about 2 ft above the ground, with position, velocity and spin
      unchanged. On nat2 the watch shows phase 0.1937 and a 1.466 rad jump about that axis; frame 11.62 of
      the motion gives 1.464. A trick (f_430) ends the same way, with its own motion's root (not ported).
      The tapes record +0x2c, so they show the jump at that frame. After it the original kicks up a yaw spin
      of about −1.9 rad/s on touchdown (air_steer) and straightens out; the port still slides on sideways
      there, so air_steer now crashes where the original does not. nat_turn and nat1_turn now crash at the
      original's time (3.58 vs 3.59 s, 3.69 vs 3.60 s).
      What swings the heading after that touchdown (airsteer_obj_decoded.txt, 2026-10-22): the yaw spin
      (+0xdc) stays 0 for the first 0.25 s. The original lands front first at pitch −0.21, rolled 0.84,
      and the shocks give a nose-up pitch spin (+0xd8) of about −2 rad/s. About the rolled body's x axis
      that pitch spin also turns the look sideways, so the heading runs on from 0.69 to 0.90 before
      the −1.9 rad/s yaw spin brings it back. The port lands at pitch −0.12, so it gets less of that.
      The difference comes from the take-off. On the jump face the port's pitch spin follows the
      original's until the fork is about 88% compressed, then runs about 1 rad/s faster for two steps
      (sx04 memory watch: original peaks at −2.52; port −3.6 to −4.0, with front pushes near 1900–2400 lb).
      So the port leaves the lip 0.03 rad more nose-up. Off the lip, the port's rear touches for two more
      steps and pulls (its damper sees the jump from the pre-gap compression), which kills the nose-down
      spin of ~0.45 rad/s that the original keeps and lets decay ×0.9 a step (pitch 0.29 → 0.195 in the air;
      port holds 0.31). A bottomed shock pushes nothing: 0x4ff1f0 reports bottoming in its last outputs, the tyre's +0x15c is set and 0x52eac0 skips it, so only the bottoming impulse stops the fork (ported; it trims the face spin from −4.0 to −3.6 rad/s, the original's peak is −2.52). Not yet found: what else in the original caps the face spin.
    * **Roll spin and tyre friction torque** (checked 2026-10-22). The lean step (0x40c830) does not zero
      the roll spin: per body axis it cancels spin that opposes the lean's own turn rate by 1.5 times that
      rate, never past zero (+0x1b8 = axis·d/h), and leaves spin with the turn alone. 0x52d070 adds each
      tyre's rolling and side friction torque, r(+0xf0) × F, to the same static-friction accumulator as
      the brakes (applied by 0x4cfe30 per axis; it tests the spin for exactly zero). Both are ported, with
      the port's 1e-3 stillness threshold kept: an exact-zero test lets every tiny spin take the full
      friction torque and broke 22 more runs. Over the local runs: 51 → 52 crash outcomes match, mean
      speed rms 1.97 → 2.09 (steer_right 2.75 → 0.58, nat_straight 2.9 → 8.1). On the air_steer landing
      the original's tyre contact points (pt1/pt2 are the tyres, vtable 0x55d880) never enter the
      contact-force state (+0xb4), so the original's −1.5 rad/s yaw comes from the tyre friction torque
      once the spin is non-zero; the port still slides on there.
18. 0x4ba1a0 orthonormalises and rebuilds the matrix from look (f_88) and up (f_94). Then
    `x += (3*v - v_prev)/2 * h` (v48 0x497490, **2nd-order Adams-Bashforth**) and `v_prev = v`.

**Frame-rate dependence.** The 0.85, 0.9 and 0.999 damping factors, the 1.5 cancel and the 37-step
power-slide decay are per step, so above 50 fps they act more often each second. The port keeps them
per step, as the game does.

### Constructor constants

* **Vehicle ctor 0x52b470**, called from the Bike ctor 0x4087e0 with `(0.9, 0.698 /*40°*/, 2 tyres, 0.18, 230)`.
  It sets f_438 = 180, f_580 = 0.698 (40°, max lean?), f_584 = 2.094 (120°), f_458 = 2,
  f_44c = 200, f_450 = −1000, and input smoothing filters 0.14 and 0.5.
  It builds the drivetrain with 0.02, the physics base 0x508c30 with (0.02 step, 100, 0.001, 0.1, 3 max steps),
  and SteeringControl 0x509fa0.
* **Shocks**: defaults when no setup is loaded:

  | | front | rear |
  |---|---|---|
  | spring kMax / kMin | 1700 / 800 | 1000 / 400 |
  | damping ζ | 0.7 | 0.7 |
  | vel threshold | 80 | 80 |

  With setup params these are scaled instead, with shock factor 0.25.
  The options sliders SldFrontDamp / SldRearDamp (0x4b84f0) feed ζ.
* **Tyres** (ctor 0x5185f0): 0.15 rolling, 2 × p9 side (p9 0.1 front, 1 rear), 45 ft/s wheelspin speed, 255 lb
  brake, brake torque 0.5 front / 1 rear, brake ramp f_534·10 on and f_538·25 off.
* **Steering** (0x40b150, 0x52fba0): `max = 0.698 * f_524`, `f_524 = 0.575 + slider * 0.002` (0.675 by
  default, 0.471 rad); `lock = clamp(max - sqrt(max² * speed / 180), 0, max)`. Once a frame, before
  the steps, 0x52fba0(input, lock, n) sets the hinge rate `f_4bc = (-input·lock - angle) / (n·h +
  leftover)` (0 if under 1e-5), and each step turns the hinge by `f_4bc · h` (0x530d10). So the bars reach
  `input * lock` by the end of the frame when there is no leftover, and otherwise cover `n·h / (n·h +
  leftover)` of the gap, finishing next frame: a lag of at most one step that depends on frame timing.
  The port has no leftover and moves the bars at once. Modelling a steady leftover of 0.01 s or 0.02 s
  (2026-10-22) changed no comparison run's crash outcome and did not remove the port's steering lead on
  the turn tapes (roll responds about 0.03 s and heading about 0.06 s before the original's), so that
  lead is more likely when the input is read than how the bars move. The other sliders (0x496a50) map to 0.8–1.2: f_528 air control, f_52c / f_530 throttle
  press and release, f_534 / f_538 brake press and release, f_63c.
  **Confirmed live (2026-10-22 memwatch, SX14 practice, turn_geom_decoded.txt):** [+0x47c]+8 = 0.4712, the
  bars at 45.7 ft/s settle at 0.2339 (the formula gives 0.2338), f_4ec = 4.965 ft (the port's wheelbase), and
  the turn rate f_43c is 1.20 × v·sin(bars)/wheelbase with the bike leaned 0.52 rad. The port had taken
  the lock as the bare 0.698 and made up the difference with a fitted TURN = 0.75 on the turning radius.
  The extra 1.2 comes from the front axle (+0x23c) that 0x406eb0 dots with the ground's side: the bars turn
  the wheel about the raked fork (bars − front axle, 28° back), so leaned into a turn the axle tilts past
  the bike's lean. With the lock at 0.698 × 0.675, the fork-axis steering and no TURN, steer_right's
  heading follows the original to 0.02 rad through a 3.5 rad/s turn, and the local crash outcomes go
  49 → 54 of 81.
* **Pump** (0x40d3d0): `f_588` is the vertical acceleration, low-pass filtered (0x52ffb0). When `f_588 > 5`,
  the lean input > 0.4 and the lean moved faster than 11/s, each loaded shock gets an extra upward
  `min(min(f_588*0.6, 100) * leanRate * 5, 10000)` lb for the step (shock +0x94).
* **Collision points**: (±1.305, 0.225, −0.228) and (3.345, −3.206, …), restitution 0.6.
* **Model frames used**: NullFrame, Handlebars, Shocks, SwingArm, TireFront, TireRear.
  Motions: LeanR01–03, Wheelie, BLeanR02, BWheelie.

## 6. Drivetrain 0x4d7090 **[solid]**

* Gear ratios: 2.0, 1.97, 1.95, 1.9, 1.85 (5 gears, automatic).
* **The curve comes from UI/UIRES.RES `presets.pb`.** Each `[Category_N]` (1 = 125, 2 = 250, 3 = 400 thumper,
  4 = 500, 5 = 600 thumper) has `Weight`, `RpmLowerLimit`, `RpmUpperLimit` and eleven `rpmNNNNN` **horsepower**
  points from the lower limit to the upper; `[Category_NPreset_1..3]` hold the shipped bikes' curves (the
  CR250 is `Category_2Preset_1`: 24, 28, 33, 38, 39, 41, 43, 45, 41, 35, 22 hp over 5000–10000 rpm). A tuned
  custom curve uses the category's own. The category is picked from the bike's engine size in bikes.pb
  with the table 0x572bec (125, 250, 400, 500, 600). Weights: 210, 230, 270, 240, 280 lb.
* The ctor lays the table out as (0 rpm, 0 hp) then the eleven points; torque = hp·5252.11/rpm (+0x160).
  `rpm = throttle · RpmUpperLimit` after the ramp in 0x4d76a0 (throttle += h/1 · open rate; −= h · close
  rate · 0.6, or · 0.2 while shifting). Shift time 0.05 s; a shift scales the throttle by the ratio of the
  two gears.
* **Shift points**: for each gear, of the points above the torque peak, the one whose torque best matches
  the torque at `min(r·x, peakRpm)` with `r = ratio[g+1]/ratio[g]`; the game shifts up at a random rpm in
  `[x−100, min(x+100, top)]` while the revs rise, and down below 1500 (+0x7c) while they fall.
* The drive force 0x4d7900 interpolates the **horsepower** table: `F = hp(rpm) * ratio * 341.62 / max(speed, 1.01)`.
* With no curve given (param 5 < 1) the ctor falls back to this torque table (lb·ft, 3000–10250 rpm), which
  the shipped game never uses:

  | rpm | torque | rpm | torque |
  |---|---|---|---|
  | 3000 | 10 | 6750 | 39 |
  | 3250 | 11.5 | 7250 | 43 |
  | 3750 | 15 | 8250 | 40.5 |
  | 4750 | 20.5 | 8750 | 37 |
  | 5250 | 24 | 9250 | 28.5 |
  | 5750 | 26 | 10250 | 25 |

* bikes.pb (inside UI/UIRES.RES) gives name, texture, engine size and BikeFourStroke; the engine size
  picks the presets.pb category.
* **The drag is 0.001 for the player's bike too.** The race builds the bike (0x52b470 → 0x508c30 →
  0x505ae0) with the literal 0.001 as `f_148`, so the 0x40c210 drag is `0.001·|v|·v`. With the curves
  above a 250 tops out near 185 ft/s (126 mph) on the flat. That fits the rest of the game: the engine
  sound only switches to PowerBand above 100 mph (section 15) and the speedometer clamps its needle at
  100 (section 16), so speeds past 100 mph are expected.

### Off the course, and backing up (0x492b20, 0x4933e0) **[solid]**

* **Off the course the drive is halved.** The race's progress tracker 0x4eb170 probes 3.5 ft ahead of each
  bike against the course graph (0x51c9c0, 100 ft search) and sets `f_78c` when it finds the bike on the
  course (time off it accumulates in `f_76c`). In any race mode (`0x2d70`) but 0, which shows laps without
  a total and is most likely practice, 0x492b20 halves the drive force while `f_78c` is 0.
* **Reverse.** The KeyConfig control TogReverse (action 0xd) is read by 0x4933e0: below 6 ft/s with the
  throttle closed and not crashed, it sets `f_478+2`. The rider then plays BackUpTop/BackUpBottom: its phase
  `f_664` runs 0 to 1 at `max(speed, 1) · h · 0.55371` per step (the rate is held once the phase passes
  0.45), and while the phase is in [0.12, 0.4762) the drive call 0x4d7900 is replaced by a push backwards
  along the rear tyre of `(0.4762 − phase) · 1010.67` lb.

## 7. Input → physics **[solid]**

0x52e3e0 reads the three axis objects (value at +0x24):

```c
lean  = -leanAxis;                     // f_508, stick forward/back
steer = -steerAxis;  if (devType...) steer = steer*steer*steer;   // f_504
throttle = analogAxis; if (throttle == -1) digital; else apply 0.2 deadzone
```

Buttons go through `ControlInterface->vt[3](action, …)`. Action ids: 3, 4 (gas), 0xc (brake),
0x1f and 0x2d. KrustyBike v80 0x493270 is throttle, v81 0x493380 is brake, and v82 0x4933e0 is
Reverse (action 0xd, see section 6). Option toggles ChkBrakeGyro and ChkGasGyro, the Press/Release Throttle/Brake
sliders (throttle ramp rates) and SteeringSlider (0x4b74e0) shape the inputs.

Rate limits on the AI path: steer 0.125 per call, lean 0.25 per call.

## 8. Crashes **[triggers solid, Wrecker solid]**

**AI path** (0x414000): a crash starts when `|Δspeed| > 44` ft/s in one step, when `up.y <= 0`, when
a solid has been touched for over 0.5 s (`f_814`), when the solver's impact passes f_778 = 100, or by
the player's rules, which run on the AI too (section 4).

**Player path**: Bike v99 0x40a950 (from KrustyBike 0x492650) checks these in order and sets `f_460` = the
crash reason and `f_448` = the bail, an index into the Wrecker's list +0x48: 0 BackOver, 1 Endo, 2 Kahuna,
3 BackOver, 4 LeftOver, 5 RightOver. The crash rules below never pick Kahuna; the KrustyBike checks do.

**KrustyBike's crash check** (vt 99, 0x492650) runs before the Bike rules (0x40a950):

* A trigger through vt 12 (0x48edd0 on `f_5a4`) launches the bike at 120 ft/s upward with a random
  spin: bail 1 (Endo), state 0xc. Not mapped further.
* **Deliberate bail:** action 8 in the air (`f_108`), with no trick running and both tyres off the
  ground (tyre +0x150 under a threshold) gives bail 2 (Kahuna), state 1.
* **Landing mid-trick:** a trick still running (`f_430`) once the bike is down (`f_108` clear) crashes
  it, state 9. The bail is 3 (BackOver) when the front tyre is not touching (+0x260 = 0); otherwise
  at a speed of 30 ft/s or less it is 5 (RightOver) if `f_e0 ≤ 0`, else 4 (LeftOver); faster, it is
  1 (Endo). The stunt is cleared (0x48e860), so it scores nothing.

| f_460 | condition | bail (`f_448`) |
|---|---|---|
| 0xb | a rider body part touches something and the push on it, in the rider's axes, has y > 1 | x < −1 RightOver; x > 1 LeftOver; else z ≥ −0.9 BackOver, otherwise Endo |
| 7 | only the front tyre is down, `f_4b4 < limit` and pitch spin > 0 (over the bars) | Endo |
| 8 | `|dot(prevLook, up_world)| > f_708 = cos 5°` (nose straight up or down) | nose down (`f_a4 ≤ 0`) Endo, up BackOver |
| 6 | lean `|f_48| > 1.91986` rad (110°) | `f_48 ≤ 0` LeftOver, else RightOver |
| 0xd | rear contact factor (+0x288) > 0.707 while `f_4b0 < 0.707` (landed sideways) | `f_48 ≥ 0` RightOver, else LeftOver |
| 4 | v70 0x492950: lost speed in one step, `speed - lastSpeed < K * -1000 * h` (about 20 ft/s); K is 1.15, 1.05 or 1.0 for the player by difficulty (+0x60c), 2.5–4 for the AI | Endo |
| 0xa | `impact * f_44c(200) < x * f_24` (a crash from force) | `f_e0 ≤ 0` RightOver, else LeftOver |
| 2 | hit another bike (`f_608`): the relative heading `d` | d > 0.707 Endo; d ≥ −0.707 by `f_e0` as 0xa; else BackOver |

**Deliberate change (2026-10-23): the speed-loss limit (check 4) uses h no shorter than 0.02 s.**
Above 50 fps the original's step is the frame time (section 5), so its limit shrinks with the
frame: about 21 ft/s in one step at 50 fps or less, which is how the game was played in 1999, but
8.75 ft/s at 120 fps. A bounce off a jump face takes its speed in one step whatever the step, so at
120 fps the port crashed on landings the original survives (jump_flat: 83.7 → 68.1 ft/s crashed at
120 fps, not at 60 fps or in the original). With the limit held at 0.02 s, over the local runs
stepped at 120 fps the port's crashes went 55 → 49 (the original has 50), and jump_flat survives;
with the tapes' own frame times the outcomes stay 55/78.

The `+0x51 & 7` bail seen in 0x494610 is the replay and network playback: each recorded frame packs the
bail (`+0x51 & 7`), the crashed flag (bit 3, `f_444`), the on-course flag (bit 6, `f_78c`), `f_478` (bit 7)
and race running (`+0x52` bit 0, `f_479`).

**The bars in a crash** (0x40ddf0 → 0x40b340). Normally the steering hinge (+0x47c) turns by the steering
rate each step. While crashed it is driven instead by the torque of the tyre and frame contacts about its
axis, scaled by h and the per-axis factors +0x610–0x618, damped ×0.99 per step and stopped at ±0.785 rad,
where it bounces back (×−0.02 on the ground, ×−0.1 in the air). It only moves the bars.

The Vehicle step v28 0x52fc60 resolves .col overlap against the CollisionObject (0x439e10), moves the
bike back out, and calls **v27 0x48e9b0 Crash()** when flagged (f_138).

**Wrecker** (wrecker.cpp, vtable 0x55e1ac, update slot 10 = 0x535d50) **[solid]**. There is no jointed
ragdoll: the rider is one rigid body (+0x40) with Rider.col's part spheres, playing a crash motion from
Rider.mcf. KrustyBike 0x496160 starts it: in the air (f_430) the random fall 0x537980, on the ground the
bail 0x537890 with the crash record's motion. The motion lists, filled in 0x492286:

* +0x48 bails: Endo, Kahuna, BackOver, LeftOver, RightOver.
* +0x50 falls (random): Fall01–Fall04.
* +0x58 landings (random, when the body first touches, state 3): LeftHit, RightHit, FeetHitL, FeetHitR, HeadHit.


* `0x537890 Start(anim)`: switches the rider's collision model on (+0x38, a ConstraintMethodCollisionModel
  over Rider.col's 6 parts), plays the crash motion `f_448` and sets the state (+0x44) to 1.
* `0x537980 StartRagdoll()`: state 2, picks a random crash motion (0.5 blend), and hands the rider's
  physics object (+0x40) the bike's velocity (+0xbc) and spin (+0xc8).
* Update: in state 0 the rider follows the bike and records each part's velocity by finite difference
  (/dt). In states 1 and 2 it runs substeps of 0.02 s (dt/3 when dt > 0.06), stepping the bike (+0x3c) and
  rider (+0x40) bodies, then 0x537690 and 0x538050. 0x537690 is not a contact: for each Rider.col part
  that touches it emits dirt (0x537410, 0x536db0). 0x538050 keeps the rider off the bike: each part is
  taken into the bike's model frame (set by 0x537f20 from 0x4924e6) and tested against a box with centre
  (0, 2, 0) and half size (0.5, 1.5, 3) ft. The parts inside give per-axis depths, which are averaged,
  scaled by 5 (+0x118) and dt, turned to world axes and added to the rider body's position (+0x10c)
  each substep. How it chooses the side on each axis is unclear in the decompile **[likely]**; the
  crate's `wreck::push_off_bike` pushes out through the nearest face. State 1 hands over to the ragdoll
  at 50 % through the motion. 0x537410 / 0x536db0 emit dirt particles along the slide (rate 2000/s,
  clamped to 500–1000).
* The rider's contacts **[solid]**: 0x40a240 builds the rider object (Wrecker 0x535600/0x535800,
  "rider.col", collision model type 0x65) and adds 15 points on Rider.mcf frames through 0x537f70
  (x, y, z, frame): Pelvis (0, 0, -0.35), UprTorso, Head (0, 0.75, 0), UprArmL/R, LwrArmL/R, HandL/R
  (0, -0.5, 0), LwrLegL/R, and two on each foot, (0, -0.5, 0.75) and (0, -1, 0). These go to the
  collision model's +0x100/+0x104 list, which its update (0x43ca10, vtable 0x5562a8 slot 11) tests
  against the terrain and flags per point (+0x10, for the dirt); its impulse at the deepest point needs
  +0x108, which 0x535830 clears, so they never push the body. The pushes come from Rider.col's six
  part hulls (frames 1, 5, 9, 11, 13 and one more, loaded by 0x43c980 → 0x4337c0): the collision
  system calls the model's contact callback (+0x7c = 0x43c7a0) per contact, which runs 0x43cd50 with
  the contact's point, normal and time in the step. 0x43cd50 is a plain impulse: j = (1 - +0xf4) ×
  -(1 + +0xb8)(v·n) / (1/m + angular terms), capped at +0xf8, then the body's speed (+0x154) and
  angular momentum (+0x16c) are scaled by 1 - +0xc0 × +0xf0, with +0xc0 the step's time (stored by
  0x43ca10). The rider's model has +0xb8 = 0 (no bounce, ctor 0x43c870), and 0x535830 sets +0xf0 =
  0.99, +0xf4 = 0.95 (5 % of the stopping impulse), +0xf8 = 2000, mass 200 (body vtable +0x74,
  Wrecker +0x68 = mass / 12) and copies a zero matrix (0x4a2950) into the body's inertia (+0x190) and
  inverse inertia (+0x1d0), so contacts never turn the rider: the fall motions do. The crate's `wreck`
  takes the posed hull corners from the viewer and answers the deepest one each step this way. The
  collision system's sweep stops the body at the surface; the crate pushes it out along the normal
  **[likely]**. One contact a step is checked against six recorded crashes in the original (SX01,
  2026-10-24, Wrecker+0x6c every 8 ms): once down, the speed falls by about e each second (0.99/s of a
  single contact), and slides from 30–37 ft/s cover 23–27 ft before the reset 2.3–3 s after the crash;
  the crate's test `slides_as_far_as_the_original` holds the port to that.
* The motions and the hand-over **[solid for the order, likely for the re-rooting]**. +0xb8 is the
  first frame under the rider model's root (Pelvis), and +0x6c its world placement, kept each update.
  0x537bf0 takes the pelvis's speed (position change / dt into +0xbc) and spin (from the change of its
  axes, into +0xc8). In state 1 the bike and rider bodies are not stepped; once the bail motion is half
  played the update calls 0x537b00: state 2, a random fall into +0xd4, faded in over 0.5 s (0x4aa1e0)
  unless a fade is already running (the player's +0x34), and the rider body gets the pelvis's speed and
  spin of that frame. The recordings show it: let go 291 and 342 ms after the crash (LeftOver and
  RightOver are 10 frames at 15/s), at 16.6 ft/s mostly downward against a bike stopped by the wall, with
  4.6 rad/s of spin. A landing (state 2 to 3) also only picks its motion into +0xd4; the update starts
  it once no fade runs, so it waits for the fall's fade (recorded: picked 38 ms after the let-go, started
  496 ms after it). +0x530/+0x538 then ask, at the end of the fade, for +0xb4: the model's root (+0x1a0)
  is moved so the pelvis stays where it was before the frame's motion step. The crate's `wreck` uses the
  pelvis as the body and draws the model round it, which keeps the pelvis just as continuous. The motion
  player's step (0x4a8720, vtable 0x55a350 slot 7) finds the keys either side of the time (0x4a6db0)
  and mixes them, so crash motions keyed at 15/s move smoothly at any frame rate.
* After a crash the bike runs the full rigid body until the reset (option ChkWreckResetOnTrack).
  Every impulse on it bounces 0.1: 0x52bb10 (vtable +0xc, which the frame, tyre-bottom and object
  contacts all reach) takes 0.1 in place of f_14c once f_444 is set, with no scaling of the impulse.
  The port's tumble bounced 0.6, so a bike crashing into the ground at 48 ft/s spun at 13 rad/s and
  bounced back up at 31 ft/s on sx08_straight; at 0.1 it rises at most 7 ft/s (original 4).

### Line following (BikeAI) **[solid for 0x40f310]**

A computer rider's line is a polyline of up to 200 points (path +0x0 count, +0x4 segment, +0x8 t, +0xc
points, +0x96c speed limit, +0x974 look-ahead), built from the track's course. Each frame 0x40f310:

1. Finds the nearest point (not counting the last) and checks the segments before and after it with the
   line–line solve 0x47c740 (along t, across s, s signed along `(dz, 0, -dx)`); a segment wins when
   `0 ≤ t ≤ 1` and `s` is below the best distance.
2. Walks the look-ahead (plus `|s|` in game modes 2 and 3) along the line from there.
3. Aims at that point moved by the rider's lane offset along the segment's perpendicular, and returns the
   level unit direction to it.

**Jump planner 0x40e000** **[solid]** (the AI calls it with 0.5 s and steps of 0.05 s). A ghost of the
rider starts at the rider with its velocity and steers for the path's stored (segment, t), which 0x40f310
left at the look-ahead point. Each step:

1. If the ghost is more than 0.5 ft above the terrain it is airborne. The first such step records the
   take-off (place, velocity, level heading, time). In the air, if the line's current segment has turned
   more than 45° (dot < 0.70755) from the take-off heading, the flight is cut there.
2. On the ground it sits on the terrain; the velocity loses its part into the ground, keeping its speed if
   that part was under 0.70755 of it. A ghost landing within 0.5 s of the take-off records the landing.
3. It moves (`y += vy·dt − 16·dt²`, `vy −= 32·dt`), t advances by `|v_level|·dt / segment length` (a whole
   1 per segment passed, whatever the segments' lengths), and the level velocity is re-aimed at the line
   point (moved by the lane offset). Airborne on the last segment also cuts the flight.

The loop runs while under 0.5 s or airborne. **Only a cut flight sets a speed**, so riders slow for jumps
that land in a corner or at the end of the line, not for ones down a straight. For a cut longer than 20 ft:
`need = √(d² · 32 / (2 · (d · sin θ + Δy))) / cos θ` (d level distance, θ the take-off climb, Δy the cut
point's height over the take-off, as the game writes it); the limit +0x96c is `need`, or `speed − 20` when
the rider is more than 20 faster, and at least 40. Outputs: time to take-off (10 when none), the landing
direction when the take-off is under 0.5 s away (the AI aims there), and the launch velocity at the limit.
The game writes the ghost's segment and t back into the path; 0x40f310 recomputes them next frame.

After the planner the AI caps its turn: an aim more than 45° off the heading is replaced by the heading
turned 45° towards it (0x40faa0).

**Obstacle scan 0x416440** **[solid]**. The AI scans from 3 ft above itself to where it will be in 0.25 s
(radius 2) and passes its speed:

* Solids (CollisionObject nodes in a grid query of the corridor's box) near the corridor are hit-tested
  with five rays: the corridor, 3 ft up, 1.5 ft down and 3 ft to each side. Hits on faces steeper than 45°
  count, and on the 3-ft-up ray any hit; the nearest (ignoring those past 10 ft when the caller asks)
  gives the direction to the obstacle.
* Vegetation (when `0x2d80` is set) blocks inside the radius and adds a quarter of the way to each tree.
* Movers (CollisionObject types 100/101, and 0x69) are tested as segments from where they are to where
  they head: crossing the corridor blocks it (and bends the dodge vector by the game's own odd measure);
  one within 10 ft ahead that does not cross pushes the dodge vector away by up to 10 ft/s.

Returns blocked, the nearest hit (1000 when none), the normalised sum of the obstacle directions and the
change to the dodge vector. 0x40faa0 then takes the part of its steering direction heading into the
obstacles out (`d −= o·(d·o)` when positive) and adds the dodge change to its target velocity.

**Steering the front wheel** (0x40faa0, right after the scan) **[solid]**. With `v` the front wheel's
velocity and `d` the steering direction:

1. Avoid: `d −= o·(d·o)` when positive, levelled and normalised (above).
2. Reach: when `d` is more than 45° (dot < 0.70755) from the bike's heading, `d` becomes the heading
   turned 45° toward `d`'s side (the jump planner's rule).
3. In the Baja and Enduro (events 1 and 5): `d = normalize_level(d + 5 × normalize_level(v))`, which
   damps the turn toward the current heading.
4. Wanted velocity `w = |v_level| × d + dodge`; the level change `w − v` is capped at `0.2 × |v|` and
   applied as an acceleration of `change / dt`, so it lands in full within the step. (The game tests
   the Baja case and the others separately, with the same cap.)

The crate's `ai::steer` does steps 3 and 4.

`.wpt` files (in each Enduro and Baja `.env`) are not the AI line: they are the checkpoint race's gates,
`[Waypoints] NumWaypoints`, `PointNX/Z` and `PointNLookX/Z`, used for WaypointBox and the chime.

## 9. .col collision format **[solid, all 248 files parse to EOF]**

Found in COLSEG.RES (track pieces) and BIKE.RES (bikes, Rider.col). Loaders: 0x4337c0 →
0x43adb0 / 0x43ae70 / 0x43aff0. BVH readers: 0x42f010, 0x42eec0, 0x42f0b0. 0x508c30 turns
`model.slb` into `model.col`. All values are little-endian.

```
u32 type
type 0: Body
type 1: u32 count, u32 unknown, then count × { u32 frameIndex (into the model's frames), Body }
        (Rider.col has 6 parts, honda.col 3)

Body:
  f32[12]  3x3 rotation + translation
  u32      a  (unknown, flags?)
  u32      hasMesh
  if hasMesh:
     u32 nVerts; vec3[nVerts]
     TriNode (recursive):
        f32 key
        if key >= 0: f32[6] bbox (min, max); TriNode left; TriNode right
        else:        f32 d; vec3 normal; u16[3] vertex indices      // plane n·p = d
  u32      hasTree2
  if hasTree2:
     PtNode (recursive):
        f32 key
        if key >= 0: f32[6] bbox; PtNode left; PtNode right
        else:        vec3 point          // collision points (sphere centres?)
```

The reference parser is `docs/re/colparse.py` in the upstream openmcm2 repository.

## 10. Terrain and surfaces

* Terrain height and normal come from 0x50d3b0 (Terrain.cpp), using the .trn quadtree the viewer already loads.
  **[solid]** Each cell is two triangles: split (x,z)–(x+1,z+1) where x+z is even, (x+1,z)–(x,z+1)
  where it is odd. The height is the plane of the triangle under the point. The normal is the stored
  vertex normals (i8 / 127) of that triangle's corners, each weighted by 1 / (squared distance in cells,
  at least 0.001), then normalized. On an even cell's lower triangle (00, 10, 11) the code takes the
  normal of corner 01 with its distance instead of corner 11; the port keeps it. Every caller passes 0
  for the face-normal flag. The tyre (0x519c30) also probes the .col world (item 4).
* **Bike against solid objects** (0x52fc60 → 0x439e10) **[solid]**. The bike's `.col` (BIKE.RES
  `honda.col`, type 1) has three parts on model frames 1, 7 and 8 counted from 1 (Bike, TireFront,
  TireRear): the frame's hull and two wheel prisms, each with a point tree. Every point is swept from
  last step's pose to this one against the object's triangles (0x428fb0 BVH-vs-BVH, then 0x428ae0):
  a point that passed through a face from its front is pushed back out along the face normal by its
  depth. Pushes add up over points and objects; the contact is the average crossing point, the normal
  the normalized sum of the distinct face normals, and the bike moves by the push plus 0.075 ft along it
  (0x435cb0). Per object hit, the callback 0x406af0 → 0x405fb0 applies a rigid-body impulse at the
  contact (0x52bb10 → 0x5050b0): `j = -(1+e)·vn / (1/m + n·((I⁻¹(r×n))×r))`, e = f_14c ridden (0.1 in the running
  game, the sx10 wall memory watch: 81.6 ft/s into the wall, 6.3 back out), 0.1 crashed, with no friction. Then 0x40aeb0 → 0x492650 crashes the bike only if a trick is running;
  otherwise the usual checks of 0x40a950 decide (the speed-loss check 4 catches head-on hits).
* `.scn` `[TerrainZoneN]` has SurfaceFriction, SurfaceDrag and SurfaceTraction (0x4f0d50 reads zones 2–8 into
  three 8-float tables at terrain+0x3a0 / +0x3c0 / +0x3e0, default 1.0; zone 1 stays 1.0). Each terrain
  sample's record starts with a surface byte (0x484890); the ground probe takes the nearest of the cell's
  four corners (0x50d3b0) and the tyre keeps its low 3 bits (0x51a710), or 7 on an object (0x51ab95). Zone
  = type + 1. The tyre copies friction to `tyre+0x144` (brakes, side and rolling friction, cornering grip
  0x406c88) and traction to `tyre+0x284` (wheelspin, 0x519670). Only SkiLodge sets any below 1: zones 2, 3
  and 5 have friction and traction 0.9 (zone 2 drag 0.9), and most of its snow is type 2. Ported
  2026-10-22: Enduro 3's launch now slides like the original (roll 0.086, heading +0.45 by 0.4 s). The
  port then keeps sliding past 0.7 s where the original straightens and accelerates, and Enduro 2 now
  meets the semi 0.1 s later and is not flagged crashed, so 42 of 62 runs match their crash outcome
  (43 before).

## 11. Suggested port order

1. Swap `ride.rs` for the **AI dumbbell model** (section 4) with the real constants: gravity 32,
   wheelbase 7, spring 90, thrust table, lateral grip 0.2, steer/lean rate limits, and the 44 ft/s crash rule.
   Run it at 0.02 s fixed steps, or in the game's ≤0.05 substeps.
2. Load .col and collide the wheel points against it (BVH of triangles).
3. Move to the Vehicle model: rigid body + 2 tyres + inline/rotating shocks + drivetrain.
4. Wrecker.

`engine/crates/mm2-physics` follows this: `Bike` (section 5 with the race flag, presets.pb specs, the
crash rules with the game's bail choice, the Wrecker's rider as `Bike::wreck`, `Bike::on_course` for the
off-course cut and `Bike::reverse` for backing up), `Spec`, `Drivetrain`, and `ai::Path` (0x40f310) with
`Path::plan_jump` (0x40e000), `ai::within_reach` (the 45° cap), and `ai::scan` / `ai::avoid` (0x416440;
solids are sampled through `Ground::solid` rather than ray-cast).

## 12. Reproducing the analysis

The scripts are in `docs/re/ghidra/` in the upstream openmcm2 repository. Ghidra 12.1.4 headless, Java 21:

```
analyzeHeadless proj mm2 -import Mcm2.exe
  -postScript MkFuncs.java      # create functions at every vtable target
  -postScript ParamId.java      # decompiler parameter-ID analysis
  -postScript CommitParams.java # commit decompiler params to the DB (run twice)
  -postScript ApplyClass.java   # struct Bk (0x1600) + Bk_vtbl from vtable 0x559600
  -postScript DumpAll.java      # out/decomp.c, functions.tsv, strings, symbols, callgraph
```

Then use `show.py` (annotates DAT_ constants and hex floats) and `cg.py` (callers and callees), with
`MM2_RE_DIR` pointing at the dump output. Don't force `__thiscall` on 0x40faa0, 0x40e000, 0x40f310 or
0x40df20: they are free functions, and forcing it breaks the decompile.

## 13. Open items

* **race+0xb8** is written only by the BajaRace constructor 0x404ef0, to `0x2d94 + players` (`0x2d94` is
  the career stage, `global+0x18` the player count, 1 in single player). While it is nonzero, 0x497740
  runs every uncrashed bike, the player's too, on the simple model 0x414000. So Baja probably rides on the
  AI model for everyone **[likely]**; this is worth checking in the running game.
* Action 9 in the air (0x48f230, needs `f_6fc`) is not mapped; it is not a trick.
* The Wrecker's push off the bike (0x538050): the side it pushes toward on each axis.
* **NAT12 crest, full gas** (memory watch nat12_crest_tyres.csv, 2026-10-22). The original's front stays
  0.013–0.05 ft clear (t0) all the way over; the port's front sits 0.005–0.01 ft closer, so its depth
  snaps to 0 and it counts as down. With both tyres down, the rear's tiny side slip gives a yaw over
  1e-4 rad, the step counts as a turn and the 0.85 ground spin damping runs, so the nose drops at
  ~1.3 rad/s instead of the original's 1.66. The rear then follows the ground down (pulling) instead of
  leaving it at 30.2 ft/s, and drives on to 37 ft/s. **Resolved the same day:** the tyre ctor stores
  each tyre's own radius (+0x274, 0x518845, from the tyre frame's box via 0x502f30), and the tyre
  frames' LOD0 vertices reach 1.177 ft (front) and 1.143 ft (rear) on all four bikes; the port had one
  guessed 1.13. With the two radii the front stays 0.04–0.06 ft clear over the crest as in the watch,
  the nose drops at 1.56 rad/s and the rear leaves at 32.9 ft/s; level riding pitches 0.022 (SX01
  tape 0.022, was 0.015). Local runs: 53/78 crash outcomes (was 52), median speed rms 0.63 (0.65);
  the mean rose 1.26 → 1.86 on jump runs whose paths split (nat_straight, nat1, baja3_turn).
* **Head-on wall hits** (sx10_turn at 85 ft/s). The original comes off the wall at 9.9 ft/s, the
  change of velocity exactly along the wall's normal (92 ft/s for 84 ft/s into it), with about 3 rad/s
  of spin. The memory watch (mw_sx10wall, 2026-10-22) shows why: its contact (f_128+0xa0) is on the
  front plate, 2.6 ft ahead of and 0.4 ft above the centre of mass, nearly in line with the normal, and
  f_444 is set in the same sample, so the bounce is the crashed 0.1. The port's path reaches the wall
  about 5 ft further along it, where its front points pass the face's plane with no triangle there and
  only the bar end (1.85 ft up) meets a face. So this is a path difference before the hit, not a
  collision rule. **[likely]**
  **Resolved 2026-10-23: the wall was too high.** The scene's `[Stadium]` Bottom and Top (the scene
  reader before 0x4f1af0 puts them at +0x6a8 and +0x6a4) go straight to 0x509b50 as the wall's y range: from Bottom up
  to Bottom + Top. That range is not scaled and not moved by the frame's Position. The port had
  Position.y + Bottom·Scale, so SX10's walls started at y 4 and the front plate (y 2.98) passed under
  them. Replayed from the original's state, the port now hits on the same step and leaves at
  12.6 ft/s (original 11.8) on the same new heading. sx10_turn's speed after the hit now follows
  the tape (6.6 s: 4.9 against 6.8, was 10.5; 7.3 s: 2.3 against 3.0, was 15.1).
* **Enduro 3 waiting on the slope. Resolved 2026-10-22.** The port's rear looked bottomed on the spawn
  drop because `depth − squeeze·square` was rounding, not a real bottoming; 0x4fee80 sets +0x50 only
  when the compression reaches the travel. With that test the port takes no 0x5050b0 impulse, no yaw
  spin starts, and enduro3_turn keeps its heading through the countdown (memory watch mw_enduro3rest:
  heading 0.14512 constant, spin_y exactly 0).
* **Jump faces** (sx04_straight). The memory watch (mw_sx04face) put the shock forces within 10 % of
  the port's, but the torque arms were different. The original's tyre contact, and so r (tyre +0xf0)
  in 0x52eac0's `r × F`, is on the rim of the wheel where the shock has put it: the front contact rides
  0.6 ft up and 0.3 ft back along the raked fork as it compresses. The port took the rim of the fully
  extended wheel, so on the face its front pushed 0.6 ft further ahead of the centre of mass. The nose
  rose too fast, the rear bottomed and the bike looped over at 2.3 s. With the contact moved by the
  shock's displacement, the bike leaves the face at pitch 0.67 (original 0.63) and at the original's
  speed. Local runs: 54/78 crash outcomes (was 52), mean speed rms 1.61 (1.66), median 0.635 (0.65).
  Still different: after the landing the original's nose drops to −0.39 and the port's only to −0.21,
  and the port arrives at the second jump 10 ft/s slower and crashes there.
  2026-10-23: the replays compared each port row with the original's next step. Lined up by
  position, the replay from 0.1 s into mw_sx04face matches step by step until the front shock nears
  full travel high on the face (pitch 0.1). The terrain's height and normal under the front tyre
  match the watch's tyre +0xcc and +0xe4 to 3 or 4 digits. From there the port's front bottoms a step
  early (sq 1.0, depth 0.95 at x 1005.07, against the original's 0.90), and the rear compresses 0.1
  to 0.3 ft less over the lip (0.40 to 0.68 against 0.70 to 0.90). So the original leaves with the
  nose spin nearly stopped (−0.14 rad/s) and holds pitch 0.59 in the air; the port leaves at −1.1
  and rises to 0.85. The landing is not in the watch. Not resolved.
* **Rotational inertia.** The running game's f_e4..f_ec (ridden) are 1/96.76, 1/60.49 and 1/48.72
  slug·ft² at 395 lb, so the box sizes squared are (94.66, 59.18, 47.66); the port has (89.3, 58.9,
  41.57). Crashed, f_f0..f_f8 are 1/41.90, 1/35.83, 1/14.54 with the same 12.267 slug mass (the
  weight stays 395 lb, f_154). The port's tumble now uses both (it had the ridden box and the bike's
  own weight): across the 34 runs where both crash, the speed rms over 1.5 s after the crash fell
  11.4 → 10.4 ft/s. The port's wreck still slides further than the original's in most runs (before the
  rider's collision model above was ported).
  With the measured ridden values, the nat12_crest rear leaves at 30.2 ft/s (original 29.3, port
  32.9 before) and the NAT runs' speed rms drops (nat1_straight 5.3 → 2.5, nat_straight 7.2 → 5.1),
  but the SX04 face over-rotates (takeoff pitch 0.78 against 0.63) and the local runs fall to 51/78.
  On the face the original damps the spin by 0.85 on most steps from pitch 0.16 (0x530825 runs on a
  step whose corner from 0x40ce00 passes 1e-4: its rear slips a little, heading −0.003 rad over the
  face); the port's rear slip is mostly 0 there and it damps on 4 steps of 12. (The value 0x52d070
  counts down and 0x530825 tests is |f_4b8|, the bars' turn, not the speed.)
  **Applied 2026-10-22** once the step length and the slip were right (section 5): from the original's
  state on the SX04 face the port now leaves at 0.58 with either inertia, and the measured values fix
  the air spin; local runs with the tapes' frame times: pitch rms 0.0599 → 0.0547.
* **Damper after a wheel leaves the ground (fixed 2026-10-22).** Off the ground the shock isn't
  evaluated and 0x52f090 copies its relaxed compression (+0x90, which 0x4fea60 sets to +0x98 after
  each relax) into the last evaluated one (+0x8c). The port kept the compression from before the wheel
  left, so when the rear touched again a step later on a lip the damper saw 8 ft/s of extension and
  pulled the bike down by 650 lb, tipping the nose up. jump_flat now lands at pitch 0.27 (original
  0.18, was 0.32) with speed rms 0.75 (was 1.99). Local runs: 57/78 crash outcomes, median speed rms
  0.545, pitch rms 0.0454.
* **Rear-only roll over a crest** (memory watch mw_natcrest, from 0.1 s). From the original's state the
  port's nose-down spin grows about 30 % faster while only the rear is down (1.46 rad/s against 1.23
  after 8 steps), with the rear shock, drive (750 lb) and rolling friction (60 lb) all matching the
  watch. The extra is about the size of the rolling friction's torque (r × F, 130 ft·lb), which
  0x52d070 adds to the static-friction accumulator; why the original's spin shows less of it is not
  found. From the front's landing on (0.4 s) the replay matches step by step.
  **Resolved 2026-10-23: not the friction.** The original's step alpha (bike +0xc0, which 0x4cfe30
  leaves with the friction torque in it) matches the port's with the friction. The difference is
  the front shock's last step on the ground. When relaxing would take a shock past full extension,
  0x4ff100 calls the shock's vtable slot 0 (0x4fe990, vtable 0x55ce04). That resets everything,
  including the damper's last compression +0x8c. So when the front touches again at 0.013 ft, the
  damper sees it compressing from 0 and pushes 80 lb (shock +0x58 in the watch). The port kept +0x8c
  at 0.056, saw the wheel still extending at 2.7 ft/s, and pulled 205 lb. Its nose-down spin grew
  14 rad/s² on that step instead of the original's 5.9. With the reset, the nat12 crest replay's
  pitch error is 0.0158 (was 0.0264) and its vy error is 0.42 (0.67). Local runs: pitch rms
  0.0454 → 0.0428, 55/78 crash outcomes (57; sx04_straight now matches; baja2_turn, baja3_straight
  and enduro1_straight split late).
  Found (2026-10-22): the rear slip is real geometry, not noise. The face's normal leans a little
  sideways (n = 0.1305, 0.983, −0.1322 against a heading of 0.70, −0.71), so projected onto it the
  path and the heading part by 1.5e-4 to 7e-4 rad (tyre +0x288, nonzero on 76 % of face samples; 31 %
  on the flat SX10 straight, all of them on the NAT12 crest). The port gets the same angle but takes
  the sine as sqrt(1 − cos²), which single precision rounds to 0 or to steps of 3.45e-4. Taking it
  from the cross product instead brings the SX04 takeoff to pitch 0.58 (original 0.59) and halves
  the pitch error on nat12_crest and nat_straight, but the local runs go 57 → 54/78 (mean rms 1.58 →
  1.57, median 0.645 → 0.63; pitch rms 0.0647 → 0.0639), and the measured ridden inertia on top
  gives 52/78. Not applied yet: with it, the port's rear shock sits 0.1 to 0.15 ft deeper than the
  original's (0.84 → 1.06 ft against 0.73 → 0.90) in the 0.08 s between the front and the rear
  leaving the lip, and the bike leaves with the nose dropping at 0.6 rad/s (original rising at 0.13).
  Applied later the same day, with the step length (section 5): started from the original's exact
  state on the face, the port with the old sine loops over (pitch 1.5 rad); with the cross product it
  leaves at 0.66, and 0.58 with the original's step lengths (original 0.61). On the face the shock
  forces and their lever arms then match the original's step by step (front 553, 730, 757 … lb against
  the port's 533, 749, 788 …). Local runs with the tapes' frame times: 56/78 crash outcomes, median
  rms 0.56, pitch rms 0.0599 (0.02 s steps without the slip fix: 57/78, 0.645, 0.0647). The suite
  totals move by a crash or two either way with any change of timing, so the replays are the better
  test.
* **Crests** (nat_straight at 0.65 s). The port's rear kept pulling to the ground for 0.07 s longer
  than the original's. The memory watch mw_natcrest has the shocks for it; not compared yet.

## 15. Engine sound (Audio.cpp, ctor 0x4e7160, update 0x4e8720) **[solid]**

The engine note is **not** pitch-shifted or volume-shaped by rpm. The game picks a recorded sample by the
bike's speed band and splices it into a streaming buffer.

**Setup (0x4e7160).** Reads `audio_16.ini` (16-bit sound) or `audio_08.ini`. Sections `[125]`, `[250]`,
`[400]`; the bike's class comes from its engine size at +0x738 (below 250 → `[125]`) and the four-stroke
flag +0x737 (set → `[400]`, else `[250]`). Each section has `Total<List>` and `<List>_%d` entries, up to 10
samples per list, for the lists Idle, LowTorque, MidTorque, HighTorque, PowerBand, Decel, Sputter and Rev,
plus `LowTorqueMaxSpeed` (20), `MidTorqueMaxSpeed` (55) and `HighTorqueMaxSpeed` (100), all in mph. The
samples are raw PCM loaded from Audio.res (0x4ea290). Each list's sample length is divided by the divisor at
+0x468, which depends on how many classes are loaded and on the 16-bit flag.

**Update (0x4e8720), every frame.** The 4 bikes nearest the camera get an engine voice (distances past
500 ft on either level axis count as 500, so there is no cut-off; see `sound.md`).
Speed in mph is bike `f_b8` × 0.68182. The state machine:

| Condition | List | State |
|---|---|---|
| On the ground, gas, speed < LowTorqueMaxSpeed | LowTorque | 2 |
| On the ground, gas, speed < MidTorqueMaxSpeed | MidTorque | 2 |
| On the ground, gas, speed < HighTorqueMaxSpeed | HighTorque | 2 |
| On the ground, gas, speed ≥ HighTorqueMaxSpeed | PowerBand | 3 |
| Gas released | Decel | 5 |
| Airborne for 0.2 s | Decel, or Rev with gas | 5 / 6 |
| Not racing yet (+0x479 == 0), gas | Rev | 6 |
| No gas, stopped | Idle | |

Each time a state starts, one sample is picked at random from its list.

**Streaming (0x4e97c0, 0x4e9aa0).** Each voice has a 22050-sample ring buffer (1 s at 22 kHz), refilled
in halves of 11025. When a half has played, the next piece of the current sample is copied in, and a new
pick is spliced in at the half boundary when the state has changed. There is no crossfade, resampling or
gain ramp in this path.

**Other sounds in the same driver.** `LandHard01` on every landing of the followed bike; `launch.wav` when
its crash reason `f_460` is 0xc (riding off the edge of the world, not the start); wrecks, falls, the crowd
and its reactions, and the scenes' own sounds: all in `sound.md`.

For the port: play the picked sample at its native rate; to sound like the original, switch samples at the
speed thresholds rather than shifting pitch.

## 16. In-race HUD (TrackOverlay.cpp, 0x51dc00–0x523f00) **[layout solid, fonts likely]**

All positions are in 640×480 units scaled by screen width/640 (0.0015625) and height/480 (0.0020833).
Bitmaps use magenta 0xff00ff as the transparent key.

**Speedometer (load 0x51ddf0, draw 0x51dfc0, needle 0x51e340).** `instr1.tga` is drawn as a sprite at the
overlay's position (+0xe8, +0xec). A red (0xff0000) 2-vertex line strip is drawn over it as the needle,
from the dial centre at (+64, +85) in 640×480 units to a tip on a radius of 45 × width/640 (35 at load,
45 once the screen size is known).

* Shown value: bike +0xbc × 0.68182 (ft/s to mph). While airborne (+0x108) the value holds the take-off
  speed and falls by 0.5 mph per frame to 0.
* Dial: max 100 mph, mid 50 mph. Angle (degrees, 0 = straight up, clockwise) is
  `230 + 130·v/50` below 50 mph and `130·(v−50)/50` above, so the needle sweeps 260° evenly
  (2.6°/mph) from lower left at 0 to lower right at 100.
* Tip = (cx + sin(a)·r, cy − cos(a)·r).

**Race text panel (ctor 0x51eb30, update 0x51f540; Enduro/Baja variant 0x51fbb0).** `BajaScore.tga`
(128×128) holds six lines of white GDI text, drawn with `DrawTextA` (DT_SINGLELINE | DT_NOCLIP) onto the
panel's surface. A line is redrawn only when its text changes. The labels are strings in LANG.DLL:

| Line | Rect (panel px) | Text |
|---|---|---|
| 0x835 | 14,20 – 128,30 | `Position: %d / %d` |
| 0x836 | 14,36 – 128,46 | `Lap: %d / %d` (just `Lap: %d` in game modes 0 and 4) |
| 0x837 | 14,53 – 128,63 | `Lap Time: ` + current lap (+0x74c) |
| 0x838 | 14,69 – 128,79 | `Best Lap: ` + best lap (+0x750) |
| 0x839 | 14,87 – 128,97 | `TBL: ` + time behind the leader (+0x770, section 17) |
| 0x13b8 | 14,105 – 100,115 | `TRT` + total race time (+0x754; a replay reads it from +0x1b8) |

The Enduro/Baja panel swaps the lap line for `Gate: ` (0x13ad). Times are formatted by 0x51dd10 as
`%2d:%05.2f`, or `-:--.--` when unset; 0x51dcc0 formats `%2.2d:%2.2d`. Fonts: the UI font named by the
locale (weight 500, or 700 when bold is set; `Small Fonts`, weight 400, when none) at 16 px for the first
lines and 12 px for the rest.

**Standings (0x51f070, 0x520090).** `%d) %s : %.0f` per rider.

**Radar (load 0x520e90, draw 0x521520, blip 0x5219b0).** Top right of the screen. Screens wider than 800
use `radar256.tga` (centre right−131, top+129, radius 118 px), otherwise `radar128.tga` (centre right−68,
top+65, radius 55 px).

* Heading-up: rotated by the player's heading (−f_50), held at the last value while the rider is crashed
  (+0x444).
* Scale: radius × (1/180) / zoom px per ft. Zoom (0x57b2fc) runs 1 to 5, default 2, so the rim is
  180 × zoom ft away. Two keys step it.
* Other riders are 3-px dashes (two line strips, rows y and y+1): light green 0x50e650 inside, dark green
  0x28aa28 clamped to the rim along their bearing when out of range (not drawn at the rim in modes 2 and 3).
* The overhead name tag of the followed rider (0x523010) is fed the distance and the rider's name (+0x5e0).

**Overhead rider markers (0x51e760).** Visibility by a terrain raycast every 4 frames (0x51e680), fading
in and out (0x51e5b0).

**Other overlays.** `%.0f/%.0ffps` counter (0x521410); `hat256.tga` with Lucida Console text (0x5225d0, the
chat/console box); `arialsm` from Fonts.res for the score panel.

## 17. Race progress: laps, gates, positions, times (0x4eb170, 0x4eb7e0) **[solid]**

The race update 0x41e0b0 calls one of two trackers each frame: 0x4eb170 for lap races (a course graph from
the track's `.tdf` and a finish line) and 0x4eb7e0 for Baja and Enduro (a linked list of gates). Each
rider has an entry in a list (+0x50 next, +4 the bike); computer and human riders are treated the same.
`engine/crates/mm2-physics/src/race.rs` has the rules below (`Line`, `Timing`, `positions`,
`behind_leader`, `finish_the_rest`); the course graph search is the caller's.

**Lines** (finish line and gates, tested by 0x47c5b0): a centre (+0), a forward normal (+0xc), a half-width
vector (+0x18) and a height (+0x28). A rider's move from last frame's probe to this frame's crosses it when
the level paths meet within both segments, the crossing point is no more than the height above the
centre, and (for one-way lines) the move goes along the normal. The probe is the bike's position plus
3.5 × its look vector. The test is `move · normal ≥ 0`, so a line whose normal is zero counts either way:
custom tracks with a "hidden" start gate store their finish line (`.tdf` tag 16) with no direction, and
their finish arch, flag girl and her box, turned to face along it (0x500bb0), collapse to nothing.

**Lap race, per rider per frame (0x4eb170):**

1. Find the nearest course place (node, segment, t) to the probe within 100 ft (0x51c9c0). Found sets
   `f_78c` (on the course, section 6); not found adds dt to `f_76c` and skips the rest.
2. With `a` the course distance (0x51d420) from the last confirmed place (+0x44) to the finish's place
   and `b` from the new place (+0x38): if `a < 100` and `a < b`, the new place is pulled back to the
   last.
3. Near the finish (`0 ≤ a < 100`), test the finish line (one way). A crossing (0x4ea650): crossings
   (+0x7a0) + 1, which is the laps done (the HUD's `Lap: %d / %d` shows min(crossings + 1, laps) and
   the lap count global+0x2d90, so the start is not a crossing); finished (+0x7a4) at crossings ≥ the
   lap count unless the race has no end; lap time (+0x74c) = race clock (+0x754) − the clock at the last
   crossing (+0x774), at least 0; the split +0x77c[crossings % 100] = the clock; the best lap (+0x750)
   takes the lap time at the second crossing or when it is lower, so lap 1, timed from the gate drop,
   never counts as a best lap. The confirmed place jumps to the finish's place.
4. With `c` from the confirmed place to the new place: when `0 < c < 100` and `c < a`, the new place is
   confirmed and `c` is added to the distance ridden (+0x14).
5. The clock runs while the rider has not finished.

**Gate race (0x4eb7e0, 0x4ea750)** is the same with gates: passing the current gate (+0x7b8) moves to the
next and adds to the gates passed (+0x790), with a split per gate (+0x780[gates % 600]); past the last gate
a lap is done (laps, lap time and best lap as above) and the next gate is the first again. With `f_735`
set, a WaypointBox trigger (`f_7c0`) reports the pass instead of the line test, and the best lap is not
kept. 0x4052c0 moves the player's waypoint marker to the next gate.

**Positions** (+0x784, 1 = first). Lap race (0x4eacd0): sort by crossings, less one when the rider has
passed the finish's place on the course but not yet crossed the line (entry +0x18 = −1), then by course
distance to the finish (less is ahead). Gate race (0x4eaeb0): by gates passed, then by squared level
distance to the next gate. Finished riders are not moved.

**Time behind the leader** (0x4eafa0, the HUD's TBL, +0x770): for every rider but the leader, its split at
its own count minus the leader's split at the same count, 0 when negative.

**Single player, after the player finishes** (`global+0x18 == 1`), every computer rider still out is
finished at once, in position order: with `left` laps to go its race clock becomes `left × lap time` (0
with no lap done), and at least `1 + rand·10` s more than the rider ahead; in a one-lap race the lap time
is that clock, and a rider with no lap gets the player's lap time + `1 + rand·10`; the best lap takes the
lap time if better; the rider is marked finished with all laps. (The game overwrites the clock with that
product rather than adding to it.)

**Reset after a crash** (end of 0x4eb170): the bike's reset place (+0x10c) is the confirmed course place,
interpolated along its segment (0x51d700), facing along the segment (0x51d7b0, +0x118), moved back 3.5 ft
along that heading. When the rider is off the course graph it falls back to the bike's own position and
look.

**Race modes and events.** `0x2d70` is the race mode from RaceModeDropDown (0 shows laps without a total,
most likely practice; the career's Start sets 1). `0x2d74` is the event: 0 Stunt Quarry
(QuarryStuntEvent), 1 Baja (BajaRace, waypnt.slt), 2 Nationals (NationalRace), 3 Supercross
(SuperCrossRace), 4 an event with no entry fee (it also shows laps without a total), and 5 Enduro (EnduroFee in the
fee table of section 18; the AI treats 1 and 5 alike).

## 18. Stunts: tricks, points and stunt money (KrustyBike 0x48e8c0–0x4908b6, 0x4967d0, 0x496fa0) **[solid unless marked]**

**Starting a trick** (rider update, 0x48fa00). Only the player's rider (not AI, not a replay), uncrashed,
with both tyres off the ground (tyre +0x150 under a small threshold) and no trick under way. Trick
button 6 or 7 (`vt[84](6|7, 0x3f)`) with the stick off centre picks trick `dir` for button 6, or `8 + dir`
for button 7 alone. 0x48e8c0 then plays rider motion +0x1420[i] and bike motion +0x1460[i] and zeroes
the trick clocks. Action 9 with `f_6fc` set goes to 0x48f230 instead (not mapped).

**Stick direction** (vt[88] 0x52cbf0) from the lean `f_508` and the raw steer `f_474` (−steer axis,
before the cube): none while lean² + steer² ≤ 0.25; otherwise an axis counts when it is over
`0.52057 × |stick|`:

| | steer + | steer 0 | steer − |
|---|---|---|---|
| lean + | 3 | 4 | 0 |
| lean 0 | 7 | | 5 |
| lean − | 2 | 6 | 1 |

**Tricks, by index** (constructor 0x490c30: rider motion names at +0x1420, bike `b<name>` at +0x1460):
0 BarKneel, 1 SplitX, 2 BKahuna, 3 Cordova, 4 HeelClicker, 5 NacNac, 6 SuperMan, 7 AirWalk, 8 BarHop,
9 SeatGrab, 10 SaranWrap, 11 CliffHanger, 12 HeartAttack, 13 TailGrab, 14 LazyBoy, 15 DCanCan. Alts
(index + 16, motions at +0x14a0 / +0x14e0): BarKneel, HeelClicker, NacNac, SuperMan, SaranWrap and
DCanCan.

**While a trick runs** (each tick, `dt`):

* The trick clock `f_1520 += dt`, capped for tricks 0–15 at 0x572ce0: 1.611, 1.3, 2.267, 2.333, 1.667,
  1.5, 2.333, 2.4, 1.722, 1.8, 1.765, 2.333, 2.133, 2.133, 1.938, 1.722 s. Alts are not capped.
* Holding a trick button while the rider's frame is strictly between `H` and `H + 3`, with `H` from
  0x5730e0 (14, 16, 17, 18, 11, 14, 17, 18, 15, 17, 15, 18, 15, 15, 17, 16), moves `dt` from the clock into
  the uncapped hold bonus `f_1528`, and sets `f_153c`. So a trick scores its length, plus however long
  it is held at its hold pose. For Alts the game reads past the end of the table: only BarKneelAlt
  finds a usable frame (1); the other Alts never hold.
* Letting go after a hold (`f_153c`), with the stick moved to a direction that is neither the trick's
  own (`index % 8`) nor next to it, turns a trick that has one into its Alt (0x48e940): the time so far
  goes to `f_1524`, the clock restarts, the Alt motions play and `f_153f` (Alt used) is set. Neighbours
  run round the stick: 0–4–3–7–2–6–1–5–0.

**Trick end** (0x4967d0, when the rider motion finishes): `secs = f_1528 + f_1520 + f_1524`; points
`f_1530 += secs × 2500`; the tricks this jump `f_153d` + 1 becomes the multiplier `f_1538`; a stat
+0x758 adds `secs`.

**Landing** (0x496fa0): base points = `f_1530 − fmod(f_1530, 100)`; the multiplier gets + 0.5 if an
Alt was used; the score is base × multiplier. 0x4ea560 plays an announcer clip for a score of 10000 or
more in events 0 and 4 [likely]. Then (0x48e860) everything resets for the next jump; a crash resets it
too, and landing before the trick motion ends is itself a crash (section 8), so it scores nothing.

* **Stunt Quarry (event 0)** and a player's freestyle run in event 4: the race's stunt display
  (0x48e190) shows `base / 100` in 4 digits (5 from 10000), and `×m.m` when the multiplier is 1.05 or
  more. The largest single score is kept at +0x788 (`LargestSingleStuntPoints`).
* **Paying events** (Baja, Nationals, Supercross, Enduro) pay money instead, from the Pro Circuit
  tables (PCTables.pb, read by 0x4d7bd0; defaults in brackets): entry fees indexed by event,
  `BajaFee` (500), `NationalsFee` (1500), `SupercrossFee` (2500), `EnduroFee` (300) at +0x1229 + 4·event,
  `PurseMultiplier` (17.5), `StuntCapPct` (0.075). A stunt's share is `min(base × 1.6e-5, 1)`, so 62500
  base points is a full share, and it pays `share × $400`. The rider's stunt total `f_7b4` (in $400
  units) is capped at `int(fee × PurseMultiplier) × StuntCapPct / 400`: once there nothing more pays,
  and the stunt that would pass it pays nothing but fills the total to the cap. The multiplier does
  not count toward money. The player sees LANG.DLL 0x14d8 + `%s %.0f.00` (paid) or 0x14d9 +
  `%s %.0f.00)` (cap reached) for 3.25 s.

This confirms the event numbering of section 2: 1 Baja, 2 Nationals, 3 Supercross, 5 Enduro (the fee
table's order); 4 has no fee.

**Computer riders' tricks** (AI controller 0x415170). Only riders with `f_15e4` (set for AI) trick.
The trick timer `f_7dc` counts up while under the wait for the AI level (global+0x60c: 25, 15, 5 s at
levels 1–3, bike +0x7d0..+0x7d8). Only the quarry branch maps the level to those waits: the lap and
gate branches compare the timer with the level's int itself loaded as float bits (0x415199 stores
the raw int at 0x20(%esp), 0x4162c2 `flds`/`fcomps` it), a denormal, so in a race the wait lasts a
single frame. Once past it, in the air and not in a trick, the rider predicts its
flight time (0x40f170, below). With chance `f_15dc` (%: in the Stunt Quarry race +0x9c/+0xa0/+0xa4 =
10/50/100 by level; the quarry's wander branch reloads +0xa4 each frame) it tries `n` = 5, 4, 3, 2, 1:
pick a trick for `air / n − 0.5` s (0x48e730), and the first that fits starts (0x48e8c0) and zeroes the
timer.

0x48e730 walks 12 rows at 0x572d20 (0x50 bytes each: f32 time, i32 counts for all / button 6 / button 7,
then trick indices). The first row whose time is under the budget gives the set: all of it, the first
`n6` (button 6 tricks), or the `n7` after them (the first `n6` when `n7` is 0). It picks
`round(rand·count − 0.5)` within the set. For a computer rider the trick fails when less than 0.1 s
would be spare; otherwise its hold time `f_152c = rand × spare × 0.8`, used in place of a button hold.
Two rows' counts disagree with their lists (1.9375 counts 8 of 9 entries, 1.8 counts 9 of 8, reading a
zero, BarKneel).

**Flight prediction** (0x40f170): a ghost from the bike's position and velocity steps 0.1 s: x and z by
`v·0.1`, y by `vy·0.1 − 0.16`, then `vy −= 3.2`, until the terrain below (0x50d3b0) is 6 ft or more
above the ghost [likely: the call's out value read as the terrain height]. The steps × 0.1 is the air time.

The crate's `stunt` module holds this: `direction`, `neighbours`, `trick_for`, `Stunt` (start, tick,
finish, land), `Purse::pay`, and for computer riders `AI_TRICKS`, `ai_pick`, `ai_trick`,
`flight_time`, `AI_TRICK_WAIT` and `AI_TRICK_CHANCE`.

## 19. Pro Circuit career: money and points (ProCircuit.cpp 0x4d7bd0, 0x4dd440; 0x4600f0) **[solid]**

**Tables** (UI/UIRES.RES `PCTables.pb`, `[ProCircuit]`, read by 0x4d7bd0 into the career object
global+0x3440). The exe's fallbacks apply only when the file is missing; the shipped file says:

| key | shipped | exe fallback |
|---|---|---|
| StartingCapital | 50000 | 5000 |
| BajaFee / NationalsFee / SupercrossFee / EnduroFee | 5000 / 15000 / 25000 / 3000 | 500 / 1500 / 2500 / 300 |
| PurseMultiplier | 17.5 | 17.5 |
| RepairCapPct / MedicalCapPct / StuntCapPct | 0.075 / 0.0375 / 0.075 | 0.1 / 0.05 / 0.075 |
| `%d_PlacePct`, 1–11 | 0.33 0.23 0.14 0.09 0.07 0.05 0.04 0.03 0.02 | 33 23 14 9 7 5 4 3 2 |

The fees sit at +0x1229 + 4·event, so event 0 and 4 have none. When the place shares add up to 1 or
less, 1st place gets the rest. So section 18's stunt money examples use the fallback fees; with the
shipped file the stunt cap at a Nationals is int(15000 × 17.5) × 0.075 = $19687.50.

**Schedule** (`PCSched.pb`, read by 0x4d8b70 per series `[Enduro]`, `[Baja]`, `[Nationals]`,
`[Supercross]`): `NumberOfTracks`, `Laps` (3 if absent), `Track_N`. Shipped: Enduro and Baja 5 tracks of
5 laps (Airfield, Farm, OpenPit, SkiLodge, Trailer; Baja01–05); Nationals 16 tracks of 10 laps (Nat01–12,
VV, Iffendic, hillside, Chanos) and Supercross 17 (SX01–17), each with `BonusTrack` = the last.

**Race points** (0x4600f0, career mode `0x2d70 == 2`, local riders only): 20, 17, 15, 12, 10, 8, 6, 5, 3,
1 for places 1–10, added to the result's points (+0x28). The career standings then re-sort by points.

**Crash costs** (0x48ea00, the bike's per-step hook vt 28, which the simple model 0x414000 also calls):
each crash adds to the bike's repair (+0x7ac) and medical (+0x7b0) units, once, on the first step the bike
is crashed. The test is `f_444` (crashed) set while `f_5b4` is clear. `f_5b4` copies `f_444` at the end of
every step (0x52ffb0, 0x494610) and is cleared with it on a reset (0x40aeb0, 0x492650). Both units grow by
`rand × 0.667`. A computer rider (`f_734`) first adds another `rand × 0.15` to each. So a player's crash costs
up to $333 of repairs and $167 of medical bills, an AI rider's up to $409 and $204, all before the caps.
The units are zeroed in 0x490a10 and copied to the result by 0x45d670. Nothing else writes them, so bike
damage adds nothing beyond this. **[solid for the formula; once per crash rests on the f_5b4 ordering]**

**Payout** (0x4dd440, after each career race): purse `P = int(fee × PurseMultiplier)`; the fee is added
to the career's fees paid (+0x454). Shares are normalised over the field size (+0x460). For each result:

* medical = int(min(P × MedicalCapPct, units × 250)); repair = int(min(P × RepairCapPct, units × 500));
* stunts = int(min(P × StuntCapPct, stunt units × 400));
* prize = int(floor(P × share[place − 1]));
* the rider's record (+0x465 + 0x140·rider) keeps this race's and the season's totals, and money
  `+= prize + stunts − repair − medical − fee`.

The prizes' rounding leftover `P − Σ prize` goes to the rider with the most points this race (its
prize, season prize and money). Race order (+0x475) and season order (+0x48d) are sorts by this race's
and the season's points.

The crate's `career` module holds this: `Tables` (`default`, `shipped`, `purse`, `stunt_purse`),
`PLACE_POINTS`, `CRASH_UNITS` and `pay`.

## 20. Race camera views (FollowCam.cpp, KrustyBikeCamera vtable 0x55980c) **[views solid, angle meanings likely]**

The race builds a KrustyBikeCamera (0x4e2d70 → 0x498d40 → FollowCamera init 0x464070). It passes the
target and 0.55 and 0.3 (two easing filters, on +0x16c and the distance +0x220), 27 (+0x228), 7 (+0x2d0,
a height), 1.048 rad (+0x2e8, the yaw reset angle), a history of 2, and the mode list 0–7. The init drops
modes 5, 6 and 7 from the cycle, which leaves five views. It also loads `FlybyCam.vue`, the flyby path
(0x465ff0; lines `CAMERA name name x y z` followed by a 3×3 frame).

**Modes** (+0x244; the name is LANG.DLL 5049 + mode, flashed for 1.5 s by 0x499270):

| mode | name | set by | distance ft (+0x220) | elevation rad (+0x22c) | yaw rad (+0x234) | FOV° (+0x258) |
|---|---|---|---|---|---|---|
| 0 | Chase Cam | 0x467c00 | 11.72 | π/3 | 0 | 85 |
| 1 | Slot Cam | 0x467be0 | 22.68 | 0.6833 | −0.6098 | kept |
| 2 | Sky Cam | 0x467bc0 | 75.98 | 0.9648 | −0.2562 | kept |
| 3 | Rider Cam | 0x467ba0 | 0 | 0 | 0 | restored from +0x2f0 |
| 4 | User Cam | 0x4991e0 | global+0x2934 | +0x2938 | +0x293c | +0x2940 |
| 5 | Bird's-Eye Cam | debug free camera on the keyboard (0x464960) | | | | |
| 6 | (no name) | | | | | |
| 7 | flyby / TV | FlybyCam.vue | | | | `(200 − d)/180·60 + 10`, 10–70 |

* The camera button is action 10 (0x499050). The mode is saved to global+0x2930 (0x499250) and
  restored at the next race (0x499230).
* The Rider Cam sits at the rider's `Head` node, eased 0.38 of the way toward the follow point
  (0x417db0). It rolls with the bike's lean: +0x308 = speed share × bike+0x43c × 0.55, or × 0.42 in
  the other views when +0x390 is set (0x4990e0) **[likely]**.
* The follow point (0x531b80) is the target plus the view's offset. The camera is kept above the
  terrain, pushed up by 0.6 × its depth below the ground and eased by 0.01 when clear, and raised a
  further 3 ft.
* Home and End (DIK 0xC7, 0xCF) narrow or widen the FOV by 3° a press within its limits. The FOV eases
  through the filter at +0x294.
* Elevation and yaw are radians. Reading +0x22c as elevation and +0x234 as yaw from behind is
  **[likely]**: 0x531b80 uses `1 − |angle|·2/π` for both as height weights.

The crate's `camera` module holds the view table (`View`, `Preset`, `name`, `next`), the easing
constants and `flyby_fov`.

## 21. Stunt Quarry sessions and scripted traffic

### Stunt Quarry (event 0; event 4 the same) **[solid]**

The race update 0x41e0b0 calls 0x4eb060 for these events instead of the lap tracker.

- **End.** The race clock is race+0x70 minutes plus race+0x74 seconds. Once it reaches `global+0x2eb0 × 60 − 1` s, every rider is marked finished (+0x7a4). The check is skipped when 0x4eb060's argument is set (endless play). In events 0 and 4, global+0x2eb0 is the LapsListBox value read as minutes, default 5. The HUD counts down from it.
- **Ranking.** Riders are sorted by bike+0x768, highest first (comparator 0x45f8c0). +0x768 is the stunt total: after each clean landing, once the f_141c delay has passed and while the rider hasn't finished, the score from 0x496fa0 (base points × multiplier, section 18) is added. The result record copies it to result+0x2c.
- **High scores.** 0x49d180 picks the table: 1 for a 5-minute session, 2 for 10, 0 otherwise.
- **Computer riders** (0x415170's last branch, for any event but 1, 2, 3 and 5) have no course, so they wander. Each frame the solver gets a single target, the point at bike+0x818 (height 0). When the rider is within 50 ft of it on the level, it picks a new one: `u, w = rand()/32768` twice, `dir = normalise(u, 0, w)`, and `target = S·(666.66 + 444·dir.x, 0, 666.66 + 444·dir.z)`. S is the terrain's horizontal scale, Terrain +0x40, reached through bike+0x1f4 **[likely]**. Because u and w are never negative, every target falls on the quarter circle on the +x, +z side of the centre; the port keeps this. The target speed +0x11a4 is 1000, so the simple model (section 4) never lets off the gas. +0x11a8 is 1.5 (+0x7cc) and +0x11ac is speed × 0.25. Obstacle avoidance still runs in the solver. Riders don't look for jumps. They trick off whatever launches them, with the chance reloaded every frame from race+0xa4 and the 25/15/5 s wait (section 18). `ai::Wander` has it.

### Scripted vehicles (CarProcedural.cpp: init 0x430520, loader 0x430920, update 0x430ca0, path 0x431ad0) **[path solid, axes likely]**

Scene sections with `AnimationType=Procedural` (135 in the shipped scenes) are cars, trucks and trains on fixed loops. Keys: `SLT`, `Offset`, `VUE`, `FPS` (15 everywhere), `LagDistance`, `FrontWheelsTurn`, `NumberOfTires`, `Sound*`, `ExcludeFromCameraIteration`, `MotionToPlay1`.

- **.vue files** (PROCVUE.RES) are 3ds Max text: `FRAME n`, then `transform "Body" m00 … m22 x y z`. FlybyCam.vue uses `camera "Camera01"` lines with one extra number. The loader keeps only the frame number and position of each key (16-byte records at +0x1b4, count +0x6c).
- **Path.** Time loops over `last frame / FPS` (fmod, then ×FPS). Between keys i and i+1 the position is a cubic Hermite (basis 0x431e20). The tangent at key i is `(p_i − p_{i−1} + p_{i+1} − p_i) × (f_{i+1} − f_i)/(f_{i+1} − f_{i−1})`, and at key i+1 it is `(p_{i+2} − p_{i+1} + p_{i+1} − p_i) × (f_{i+1} − f_i)/(f_{i+2} − f_i)`. At the ends the tangent is the chord. The current segment is cached at +0x64.
- **Clock.** +0x194 starts at 0 when the session starts (not during the menus or loading) and gains dt each update, wrapped with fmod over `last frame / FPS`. Every vehicle is deterministic from there: the Ski Lodge semi's rear meets an idle player's spawn line about 23 s in.
- **Lag (0x431800).** `LagDistance` shifts a unit back along the path by arc length, which is how a trailer runs 21 ft behind its cab. The search walks dt from 0 in steps of 2 s, halving the step each time it turns around, adding (or, going back, subtracting) the chord `|path(t − dt) − previous|` to a running length until that is within 0.1 ft of the distance, or 200 steps pass. A chord whose square is exactly 1 counts as 1. A negative distance looks ahead. With `LagDistance` 0 there is no search. t′ = t − lag.
- **Placement (0x430ca0).** pos = path(t′). The velocity (+0x40) is `(path(t′ + 0.1) − pos) × 10` (+0x1a4 = 0.1, +0x1a8 = 10) with y = 0; it is what a bike hitting the vehicle bounces against. The heading looks 1 ft ahead, `normalise(path(t′ − lag(t′, −1)) − pos)`, blended as `0.2 × new + 0.8 × old` (+0x1ac = 0.8) and not renormalised. The body stands at the path's x, z plus `Offset`, at the height of whatever is under it: the terrain (0x50d3b0), or an object it drives over through its own collision probe (0x439e10), as the trucks cross the Ski Lodge's SL-Bridge. The ground normal is blended `0.5 × new + 0.5 × old` (+0x1b0 = 0.5). The model matrix (0x4367f0) has that normal as up and the heading laid on it as forward.
- **Solid.** The model's `.col` in COLSEG.RES (SemiCab.COL, SemiTrailer.COL, …) moves with it. The port tests a bike's collision points against it in the vehicle's own space, from where it was to where it is, and bounces off the speed relative to the vehicle's velocity: an Enduro 2 tape that rear-ends the trailer at 61 ft/s crashes at the same moment in both (2.21 s against 2.19 s after the throttle).
- **Wheels.** Nodes `TireFrontL/R`, `TireBackL/R`, `TireMidL/R` spin by distance travelled. With `FrontWheelsTurn`, the front pair turns by atan2 toward the path, clamped to ±30°.
- **Axes [likely].** Max is z-up: vue x → game x, vue z → game y, vue y → game z.

The crate's `procedural::Path` parses a .vue file and evaluates this spline.
