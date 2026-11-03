//! The player's bike, after the game's full model (`Vehicle::Simulate`
//! 0x52ffb0 and the Bike and KrustyBike overrides it calls).
//!
//! The game's model is a hybrid, and this follows its split:
//!
//! * **Forces move the body.** Gravity, drag (0x40c210), the shocks
//!   (0x52eac0), the drive at the rear tyre (0x492b20), the brakes
//!   (0x519850) and the tyres' rolling and side friction (0x51b1d0) add up
//!   to one force, integrated in full once the race is running (0x506ed0)
//!   and only vertically before the start (0x497590). Resistances share a
//!   budget so that together they never reverse the bike in one step.
//! * **Pitch is a torque**, from the shocks and brakes; only its pitch part
//!   is kept.
//! * **Turning and lean are kinematic.** Steering bends the velocity like
//!   a bicycle (R = wheelbase / sin(steer), 0x52d800) and the body yaws
//!   with it, plus a power slide when the rear spins up while leaned
//!   (0x40ce00); the bike leans towards a target from the turn rate and the
//!   steering (0x40c830).
//! * **In the air** (0x40bcc0) the rider's lean pitches the bike and the
//!   bars yaw it; the gas and brake can also tip it (the gyro options), and
//!   the bike rights its roll.
//!
//! It runs in the game's fixed 0.02 s steps. Units are feet, seconds,
//! pounds and slugs.

use crate::drivetrain::Drivetrain;
use crate::spec::Spec;
use crate::wreck::{self, Wreck};
use crate::Ground;
use glam::{Mat3, Mat4, Quat, Vec3};

/// The game's physics step (Vehicle ctor, 0x506b20).
pub const STEP: f32 = 0.02;
/// More steps than this in one frame are dropped, as the game does.
const MAX_STEPS: u32 = 3;

/// Bike geometry, in model space, from the bike models' frames (Honda.slb:
/// TireFront, TireRear, SwingArm, HandleBars, NullFrame). The four bikes
/// share it to within an inch.
const FRONT_AXLE: Vec3 = Vec3::new(0.0, 1.151, 2.308);
const REAR_AXLE: Vec3 = Vec3::new(0.0, 1.115, -2.657);
const SWINGARM_PIVOT: Vec3 = Vec3::new(0.0, 1.619, -0.731);
const HANDLEBARS: Vec3 = Vec3::new(0.0, 3.701, 0.95);
/// The centre of mass, with the rider on (NullFrame).
pub const COM: Vec3 = Vec3::new(0.0, 2.5, -0.333);
/// The tyres' radii, front and rear: the height of each tyre frame's
/// box (0x518845 stores it from 0x502f30), 1.177 and 1.143 ft on all four
/// bikes.
const TYRE_RADII: [f32; 2] = [1.177, 1.143];
/// How far past the tyre the probe looks for a solid object under it.
const OBJECT_REACH: f32 = 0.1;
/// Axle to axle (+0x4ec).
const WHEELBASE: f32 = FRONT_AXLE.z - REAR_AXLE.z;
/// The rotational inertia's box sizes squared, ft², about the pitch, yaw
/// and roll axes; the inertia is the mass / 12 times these. Read from the
/// running game (f_e4..f_ec = 1/96.76, 1/60.49, 1/48.72 slug·ft² at 395 lb).
pub const INERTIA: Vec3 = Vec3::new(94.66, 59.18, 47.66);
/// The same for the crashed bike (f_f0..f_f8, which 0x496c60 and 0x52bb10
/// take once f_444 is set): the bike's own box, read from the running
/// game as 1/41.90, 1/35.83, 1/14.54 slug·ft² at the unchanged 395 lb.
const CRASHED_INERTIA: Vec3 = Vec3::new(40.990, 35.050, 14.219);

/// Front fork travel (InlineShock, length 1) and the swingarm's swing
/// (RotatingShock, 0.5585 rad), from the Bike ctor (0x4087e0).
const FORK_TRAVEL: f32 = 1.0;
const SWINGARM_SWING: f32 = 0.558_505_3;
/// Shock springs, softest and stiffest (Bike ctor defaults), lb/ft, and
/// the damping ratio; the spring stiffens with compression (0x4feb20).
const SPRING: [(f32, f32); 2] = [(800.0, 1700.0), (400.0, 1000.0)];
const DAMPING: f32 = 0.7;
/// Above this shock speed, ft/s, the damping eases (0x4feb20).
const DAMPING_KNEE: f32 = 80.0;
/// Share of the bike's weight the shocks' damping sees (Vehicle param 19).
const SPRUNG: f32 = 0.9;
/// The rider's weight, lb, which the bike carries until he comes off
/// (0x408450 passes 165 to 0x52ab70; a crash puts it back to 0).
pub const RIDER: f32 = 165.0;
/// How hard the frame bounces off the ground (the port's own stand-in for
/// the game's frame points): not at all.
const BOTTOM_BOUNCE: f32 = 0.0;

/// Steering lock at a standstill, narrowing with the square root of speed
/// over 180 ft/s (0x40b150): 0.698 rad (40°) times the Steering slider's
/// factor (f_524, [`Sliders`]). Read live in the original as 0.4712
/// ([+0x47c]+8) at the default; its bars at 45.7 ft/s settle at 0.2339
/// where this gives 0.2338.
const STEER_LOCK: f32 = 0.698_131_6;
const STEER_FADE: f32 = 180.0;
/// Lean limit and rate (Vehicle ctor: 0.698 rad, 2.094 rad/s).
const LEAN_LIMIT: f32 = 0.698_131_6;
const LEAN_RATE: f32 = 2.094_395;

/// Air drag, −k·|v|·v (physics base: 0.001).
const DRAG: f32 = 0.001;
/// Tyre friction: rolling, and sideways per tyre (front, rear), times the
/// load (tyre ctor 0x5185f0: 0.15, 2 × {0.1, 1}).
const ROLLING: f32 = 0.15;
const SIDE: [f32; 2] = [0.2, 2.0];
/// Brake force per tyre at full pedal, lb (255), and its pitch leverage
/// (front 0.5, rear 1).
const BRAKE: f32 = 255.0;
const BRAKE_TORQUE: [f32; 2] = [0.5, 1.0];
/// A spin this slow counts as none for the brakes' torque, rad/s.
const STILL_SPIN: f32 = 1e-3;
/// The brake ramps on and off, per second (0x52e8f0).
const BRAKE_ON: f32 = 10.0;
const BRAKE_OFF: f32 = 25.0;
/// Below this rear-tyre speed the rear can spin up (tyre ctor: 45).
const SPIN_SPEED: f32 = 45.0;
/// Backing up (0x4933e0, 0x48fe50, 0x4d7900): below this speed, ft/s,
/// the Reverse control has the rider paddle the bike back. The
/// BackUpTop/BackUpBottom motion's phase runs 0 to 1 at this rate per ft/s
/// (at least 1 ft/s), and while it is between the two marks the feet push
/// with (PUSH_END − phase) × PUSH lb.
const BACK_UP_SPEED: f32 = 6.0;
const PADDLE_RATE: f32 = 0.553_71;
const PUSH_START: f32 = 0.12;
const PUSH_END: f32 = 0.4762;
const PUSH: f32 = 1010.67;

/// Air control (0x40bcc0): pitch and yaw per unit input, rad/s.
const AIR_PITCH: f32 = 0.83;
const AIR_YAW: f32 = 0.2;
/// How long the air pose takes to follow the bars, s (`f_704`).
const POSE_LAG: f32 = 0.55;
/// Spin damping per step: on the ground, in the air and always (0.85,
/// 0.9, 0.999).
const GROUND_SPIN: f32 = 0.85;
const AIR_SPIN: f32 = 0.9;
const SPIN_DECAY: f32 = 0.999;

/// Crash limits (0x40a950, 0x492650): roll, the nose within 5° of
/// vertical, an endo, a lowside, and a loss of speed in one step of more
/// than `crash_tolerance` × 1000 × h.
const CRASH_ROLL: f32 = 1.919_86;
const CRASH_VERTICAL: f32 = 0.996_194_7;
const CRASH_ENDO: f32 = 0.766;
const CRASH_LOWSIDE: f32 = 0.707;
const CRASH_DROP: f32 = 1000.0;
/// How far the centre of mass keeps above the ground, the frame's
/// underside, and how the body slides on it (ours; the game's frame and
/// ragdoll collide instead).
const BODY_CLEARANCE: f32 = 1.0;
const BODY_FRICTION: f32 = 0.6;
/// The rider's head, above the centre of mass.
const HEAD: f32 = 3.0;
/// A crashed bike within this height of the ground falls over, ft; its
/// centre stands this high when upright, ft; gravity, ft/s².
const TOPPLE_REACH: f32 = 4.0;
const TOPPLE_HEIGHT: f32 = 2.0;
const GRAVITY_FT: f32 = 32.2;

/// The crashed bike's corners, in model space, and how far out each
/// touches (the tyres' radius; the bars, seat, fender and pegs by their
/// own size). Ours: the game's frame collision points are not all mapped.
const TUMBLE_POINTS: [(Vec3, f32); 8] = [
    (FRONT_AXLE, TYRE_RADII[0]),
    (REAR_AXLE, TYRE_RADII[1]),
    (Vec3::new(1.305, 3.7, 0.95), 0.2),
    (Vec3::new(-1.305, 3.7, 0.95), 0.2),
    (Vec3::new(0.0, 3.3, -1.0), 0.3),
    (Vec3::new(0.0, 2.9, -2.8), 0.2),
    (Vec3::new(0.6, 1.2, -0.3), 0.1),
    (Vec3::new(-0.6, 1.2, -0.3), 0.1),
];
/// How a crashed bike bounces off the ground and slides on it: every
/// impulse on a crashed body bounces 0.1 (0x52bb10 takes 0.1 in place of
/// f_14c once f_444 is set).
const TUMBLE_BOUNCE: f32 = 0.1;
const TUMBLE_FRICTION: f32 = 0.15;

/// How long a crash lasts before the rider is put back, s (the original's
/// tapes reset 2.2–2.5 s after the crash flag sets).
const CRASH_TIME: f32 = 2.4;
/// The game's own crash clock (f_458, set to 2 in 0x52b470), which
/// [`Bike::touch_clock`] runs as 0x40b010 does.
pub const GAME_CRASH_TIME: f32 = 2.0;
/// For its first half second the game's crash clock runs only on steps
/// the thrown rider touches something (0x40b010 clears the Wrecker's
/// +0xac while the clock is above f_710 = f_458 − 0.5).
/// How far clear both tyres must be for the Kahuna Dumpster, ft (0x5597ac).
const KAHUNA_CLEAR: f32 = -3.5;
/// How near the world's edge throws the rider back, ft (0x559534), how
/// much of the way to the middle the throw covers a second (0x5597b0) and
/// how fast it goes up, ft/s (120.0).
const EDGE: f32 = 105.0;
const EDGE_THROW: f32 = 0.055;
const EDGE_RISE: f32 = 120.0;
/// Put back this far behind the confirmed course place, ft (0x4eb170).
const RESET_BACK: f32 = 3.5;
/// How the bike bounces off a solid object while ridden (f_14c, applied
/// in 0x5050b0; the running game holds 0.1, the same as crashed), and how
/// much farther than its depth a collision point is pushed out of one
/// (0x435cb0).
pub(crate) const RESTITUTION: f32 = 0.1;
pub(crate) const PUSH_MARGIN: f32 = 0.075;

/// The bike's collision points from its `.col` (BIKE.RES `honda.col`):
/// the frame's, in model space, and each wheel's about its axle. The
/// file's parts follow the model's frames counted from 1: Bike, TireFront
/// and TireRear.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hull {
    pub body: Vec<Vec3>,
    pub wheels: [Vec<Vec3>; 2],
    /// The same parts as meshes, for the other bikes' points to hit
    /// (0x405fb0 against an object of type 100): `None` for the frame,
    /// else the wheel.
    pub meshes: Vec<(Option<usize>, mm2_formats::col::Body)>,
}

impl Hull {
    pub fn new(col: &mm2_formats::col::Collision) -> Self {
        let mut hull = Hull::default();
        for part in &col.parts {
            let (list, wheel) = match part.frame {
                Some(1) => (&mut hull.body, None),
                Some(7) => (&mut hull.wheels[0], Some(0)),
                Some(8) => (&mut hull.wheels[1], Some(1)),
                _ => continue,
            };
            hull.meshes.push((wheel, part.body.clone()));
            let m = &part.body.matrix;
            let own =
                Mat3::from_cols_array(&[m[0], m[1], m[2], m[3], m[4], m[5], m[6], m[7], m[8]]);
            let at = Vec3::new(m[9], m[10], m[11]);
            for node in &part.body.points {
                if let mm2_formats::col::Node::Leaf(p) = node {
                    let p = own * Vec3::from(*p) + at;
                    if !list.iter().any(|q| q.distance(p) < 1e-4) {
                        list.push(p);
                    }
                }
            }
        }
        hull
    }
}

/// What the rider is asking for this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Controls {
    /// 0 to 1.
    pub throttle: f32,
    /// 0 to 1.
    pub brake: f32,
    /// −1 (left) to 1 (right).
    pub steer: f32,
    /// −1 (back) to 1 (forward): moves the rider's weight, and pitches the
    /// bike in the air.
    pub lean: f32,
}

/// Why a crash happened (the game's `f_460`, set in 0x40a950 and
/// 0x492650).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Crash {
    /// Lost too much speed in one step: a bad landing or a wall (4).
    Impact,
    /// Rolled or pitched over (6, 8).
    Flipped,
    /// Over the bars, front down and rear up (7).
    Endo,
    /// The rear slid out from under a leaned bike (0xd).
    Lowside,
    /// The rider or body hit something (0xb, 10).
    Hit,
    /// The rider let go on purpose, action 8 (crash 1, 0x492650).
    Bail,
    /// Rode off the world: thrown back in toward its middle (0xc, the
    /// game's launch.wav).
    Edge,
}

#[derive(Debug, Clone, Copy)]
struct Tyre {
    /// Suspension compression, 0 (extended) to the travel.
    squeeze: f32,
    /// The compression the damper last saw (shock +0x8c): springing back
    /// out between hits is not felt as shock speed.
    felt: f32,
    travel: f32,
    /// Where the tyre touches, the ground there, and the tyre's heading
    /// along it.
    contact: Vec3,
    normal: Vec3,
    forward: Vec3,
    /// The tyre's axle, turned by the bars and tilted with the bike
    /// (tyre +0x23c).
    axle: Vec3,
    /// How far into the ground the extended tyre would be.
    depth: f32,
    down: bool,
    /// Share of the weight (0x406ee0) and the normal force, lb.
    load: f32,
    normal_force: f32,
    /// Sine of the slip angle (+0x1d0), and which way the tyre's ground
    /// moves under it at the start of the step (+0x108).
    slip: f32,
    moving: Vec3,
    /// On a solid object rather than the terrain (surface 7, which no
    /// track's zones grip less than fully).
    on_object: bool,
    /// How far past the end of its travel the shock is pushed this step.
    bottomed: f32,
}

impl Tyre {
    /// The grip of what the tyre stands on (tyre+0x144).
    fn friction(&self, ground: &dyn Ground) -> f32 {
        if self.on_object {
            1.0
        } else {
            ground.friction(self.contact)
        }
    }

    /// Its grip under power (tyre+0x284).
    fn traction(&self, ground: &dyn Ground) -> f32 {
        if self.on_object {
            1.0
        } else {
            ground.traction(self.contact)
        }
    }
}

/// The Controls page's sliders as the bike uses them (0x496a50): each
/// 0 to 100 slider gives 0.8 + 0.004 × its value, the Steering one
/// 0.575 + 0.002 × it; at the default 50, 1 and 0.675. Computer riders
/// keep the defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sliders {
    /// Times the steering lock (f_524).
    pub steering: f32,
    /// Times the bars in the cross-up pose (f_63c).
    pub cross_up: f32,
    /// Times the lean's pitch in the air (f_528).
    pub pitch: f32,
    /// Times the brake's ramps on (f_534) and off (f_538).
    pub press_brakes: f32,
    pub release_brakes: f32,
}

impl Default for Sliders {
    fn default() -> Self {
        Sliders::from_values(50, 50, 50, 50, 50)
    }
}

impl Sliders {
    pub fn from_values(steering: u32, cross_up: u32, pitch: u32, press_brakes: u32, release_brakes: u32) -> Self {
        Sliders {
            steering: 0.575 + 0.002 * steering as f32,
            cross_up: slider(cross_up),
            pitch: slider(pitch),
            press_brakes: slider(press_brakes),
            release_brakes: slider(release_brakes),
        }
    }
}

/// A 0 to 100 slider's factor, 0.8 to 1.2 (0x496a50).
pub fn slider(value: u32) -> f32 {
    0.8 + 0.004 * value.min(100) as f32
}

#[derive(Debug, Clone)]
pub struct Bike {
    /// The centre of mass.
    pub position: Vec3,
    pub velocity: Vec3,
    last_velocity: Vec3,
    /// The place before the last step, to draw in between steps.
    previous: Option<(Vec3, Mat3)>,
    /// Body to world: columns right, up and forward.
    orientation: Mat3,
    /// Spin, rad/s, in body axes.
    pub spin: Vec3,
    /// Heading about +y (0 = +z, right turn positive), pitch (nose up
    /// positive) and lean (right positive), in radians, from the
    /// orientation.
    pub heading: f32,
    pub pitch: f32,
    pub lean: f32,
    /// The steering angle at the bars.
    pub steer: f32,
    /// The rider's inputs as the riding motions read them: the steer,
    /// −1..1 (f_504), and the lean, eased toward its input 0.25 a step
    /// (f_508).
    pub rider: (f32, f32),
    /// The Controls page's sliders.
    pub sliders: Sliders,
    pub spec: Spec,
    pub drivetrain: Drivetrain,
    tyres: [Tyre; 2],
    pub airborne: bool,
    air_time: f32,
    /// The rider's steer pose in the air (0x407900, `f_520`): 1 or 2
    /// until the bars go left, 3 or 4 after, until they go right.
    air_phase: u8,
    /// Falling clear of the ground with the air pose on (`f_6fc`), the
    /// bars as the pose follows them (`f_644`) and how far it is through
    /// the cross-up motion, 0 to 1 (`f_65c`).
    air_pose: bool,
    air_bars: f32,
    pub pose_phase: f32,
    /// The bike model's cross-up motion (`CrosBik1`–`3`, which share it):
    /// its root frame's turn from the bike at each frame, read from
    /// `RES/BIKE.RES` by the caller. The air pose draws the bike crossed
    /// up by it, and coming out of the pose turns the bike itself by it.
    /// None, no cross-ups.
    pub cross_up: Option<std::sync::Arc<Vec<Quat>>>,
    /// The crash in progress and the time left of it.
    pub crash: Option<(Crash, f32)>,
    /// Before the gate drops only vertical forces apply (race +0x18a).
    pub racing: bool,
    /// Only whole 0.02 s steps, a frame shorter than that waiting for
    /// the next (live play). The game runs one step of a short frame's
    /// length instead, but its damping (×0.85 on the ground, ×0.9 in the
    /// air) and crash limits are per step: at 120 fps the bike would lose
    /// its spin more than twice as fast a second as at the 50 fps or less
    /// it was played at. False runs the game's accumulator exactly.
    pub fixed_step: bool,
    /// The player's crash tolerance: 1, 1.05 or 1.15 by difficulty.
    pub crash_tolerance: f32,
    /// Whether the race's progress tracker finds the bike on the course
    /// (+0x78c, 0x4eb170). Off it, in any race mode but practice, the
    /// drive is halved (0x492b20). The caller decides; it starts true.
    pub on_course: bool,
    /// The confirmed place on the course and the way along it there, set
    /// by the race; a crash puts the rider back just behind it.
    pub reset_to: Option<(Vec3, Vec3)>,
    /// How long a crash lasts, s. With `touch_clock` the clock runs as the
    /// game's 0x40b010 does: it starts only once the thrown rider touches
    /// something, and for its first half second it runs only on steps the
    /// rider touches, so a rider thrown high or bouncing stays down longer.
    /// The computer riders use this with [`GAME_CRASH_TIME`]; the player's
    /// 2.4 s is fitted to the original's tapes.
    pub crash_time: f32,
    pub touch_clock: bool,
    /// The Wrecker's +0xac: the thrown rider has touched something since
    /// the crash clock last cleared it.
    touched: bool,
    /// The collision points run against solid objects; none, no objects.
    pub hull: Option<std::sync::Arc<Hull>>,
    /// The Reverse control (TogReverse) is held.
    pub reverse: bool,
    /// The Clutch key is held (action 12): the engine revs free.
    pub clutch: bool,
    /// The back-up motion's phase, 0 to 1 (+0x664), and its rate (+0x668).
    paddle: (f32, f32),
    /// The rider thrown off in the crash in progress.
    pub wreck: Option<Wreck>,
    /// Where the bike's collision points last met a solid object, this
    /// step (0x405fb0, which throws the sparks).
    pub hit: Option<Vec3>,
    seed: u32,
    /// A trick or a taunt is running (`f_430`): it ends the air pose, and
    /// a crash meanwhile is a fall, not a bail. The caller sets it.
    pub stunting: bool,
    pending: f32,
    /// This step's length: 0.02 s, or the whole frame when frames come
    /// faster than that (0x506b20).
    h: f32,
    /// Options: the gas and brake tip the bike in the air.
    pub gas_gyro: bool,
    pub brake_gyro: bool,
    /// The brake's ramp, 0 to 1 (tyre +0x29c).
    brake: f32,
    /// The rear tyre's grip after wheelspin, 0 to 1 (+0x2b8), and the
    /// drive force, lb (+0x280).
    pub traction: f32,
    drive: f32,
    /// A power slide: yaw per step, its change per step and steps left
    /// (+0x628, +0x62c, +0x630).
    slide: (f32, f32, i32),
    /// The turn rate, rad/s (+0x43c).
    pub turn_rate: f32,
    /// Smoothed vertical acceleration (+0x588), and last step's lean.
    lift: f32,
    last_lean: f32,
    /// The last step's length (+0x1e8): the shocks measure the motion
    /// since they were last evaluated over it.
    last_h: f32,
    /// How fast a crashed bike is falling onto its side, rad/s.
    topple: f32,
    /// Last step's acceleration, ft/s² (+0x70).
    accel: f32,
}

impl Bike {
    /// A bike standing on the ground at `position` (the model's origin,
    /// between the tyres) facing `direction`, with the default spec.
    pub fn new(ground: &dyn Ground, position: Vec3, direction: Vec3) -> Self {
        Bike::with_spec(ground, position, direction, Spec::default())
    }

    pub fn with_spec(ground: &dyn Ground, position: Vec3, direction: Vec3, spec: Spec) -> Self {
        let tyre = |travel| Tyre {
            squeeze: 0.0,
            felt: 0.0,
            travel,
            contact: Vec3::ZERO,
            normal: Vec3::Y,
            forward: Vec3::Z,
            axle: Vec3::NEG_X,
            depth: 0.0,
            down: false,
            load: 0.5,
            normal_force: 0.0,
            slip: 0.0,
            on_object: false,
            bottomed: 0.0,
            moving: Vec3::ZERO,
        };
        let mut bike = Bike {
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            last_velocity: Vec3::ZERO,
            previous: None,
            orientation: Mat3::IDENTITY,
            spin: Vec3::ZERO,
            heading: 0.0,
            pitch: 0.0,
            lean: 0.0,
            rider: (0.0, 0.0),
            sliders: Sliders::default(),
            steer: 0.0,
            drivetrain: Drivetrain::stock(1.0, 1.0),
            spec,
            tyres: [tyre(FORK_TRAVEL), tyre(SWINGARM_SWING * swingarm_length())],
            airborne: false,
            air_time: 0.0,
            air_phase: 1,
            air_pose: false,
            air_bars: 0.0,
            pose_phase: 0.0,
            cross_up: None,
            crash: None,
            racing: true,
            fixed_step: false,
            crash_tolerance: 1.05,
            on_course: true,
            reset_to: None,
            crash_time: CRASH_TIME,
            touch_clock: false,
            touched: false,
            hull: None,
            reverse: false,
            clutch: false,
            paddle: (0.0, 0.0),
            wreck: None,
            hit: None,
            seed: 0x2545_f491,
            stunting: false,
            pending: 0.0,
            h: STEP,
            gas_gyro: false,
            brake_gyro: false,
            brake: 0.0,
            traction: 1.0,
            drive: 0.0,
            slide: (0.0, 0.0, 0),
            turn_rate: 0.0,
            lift: 0.0,
            last_lean: 0.0,
            last_h: STEP,
            topple: 0.0,
            accel: 0.0,
        };
        bike.reset(ground, position, direction);
        bike
    }

    /// Puts the bike upright and at rest on the ground at `origin` (the
    /// model's origin) facing `direction`, pitched to the slope, with its
    /// static sag; any crash is over.
    pub fn reset(&mut self, ground: &dyn Ground, origin: Vec3, direction: Vec3) {
        self.touched = false;
        let flat = Vec3::new(direction.x, 0.0, direction.z).normalize_or(Vec3::Z);
        let heading = flat.x.atan2(flat.z);
        let (front, _) = ground.terrain(
            origin.x + flat.x * FRONT_AXLE.z,
            origin.z + flat.z * FRONT_AXLE.z,
        );
        let (rear, _) = ground.terrain(
            origin.x + flat.x * REAR_AXLE.z,
            origin.z + flat.z * REAR_AXLE.z,
        );
        let pitch = ((front - rear) / WHEELBASE).atan();
        self.orientation = Mat3::from_rotation_y(heading) * Mat3::from_rotation_x(-pitch);
        self.spin = Vec3::ZERO;
        self.velocity = Vec3::ZERO;
        self.last_velocity = Vec3::ZERO;
        self.topple = 0.0;
        self.crash = None;
        self.wreck = None;
        self.steer = 0.0;
        self.slide = (0.0, 0.0, 0);
        self.turn_rate = 0.0;
        self.lift = 0.0;
        self.accel = 0.0;
        self.brake = 0.0;
        self.paddle = (0.0, 0.0);
        self.air_pose = false;
        self.pose_phase = 0.0;
        // Static sag: each spring carries its share of the weight.
        let weight = self.spec.weight + RIDER;
        for (i, t) in self.tyres.iter_mut().enumerate() {
            t.squeeze = sag(i, weight * 0.5).min(t.travel);
            t.felt = t.squeeze;
        }
        let sag_drop = (self.tyres[0].squeeze + self.tyres[1].squeeze) / 2.0;
        let base = (front + rear) / 2.0;
        let origin = Vec3::new(origin.x, base - sag_drop * 0.8, origin.z);
        self.position = origin + self.orientation * COM;
        self.update_angles();
    }

    /// Puts the bike into a state read from the running game (a memory
    /// watch), to step on from it: centre of mass, velocity, the body's
    /// right, up and forward, the spin in world axes and both shocks'
    /// compressions, ft.
    pub fn set_state(&mut self, position: Vec3, velocity: Vec3, axes: Mat3, spin: Vec3, squeeze: [f32; 2]) {
        self.position = position;
        self.velocity = velocity;
        self.last_velocity = velocity;
        self.orientation = axes;
        self.orthonormalize();
        self.spin = self.orientation.transpose() * spin;
        for (t, s) in self.tyres.iter_mut().zip(squeeze) {
            t.squeeze = s.min(t.travel);
            t.felt = t.squeeze;
        }
        self.previous = None;
        self.update_angles();
    }

    /// The heading, pitch and lean from the orientation (0x4ba1a0).
    fn update_angles(&mut self) {
        let m = self.orientation;
        let look = m.z_axis;
        self.heading = look.x.atan2(look.z);
        self.pitch = look.y.clamp(-1.0, 1.0).asin();
        self.lean = (-m.x_axis.y).atan2(m.y_axis.y);
    }

    /// Body to world.
    pub fn rotation(&self) -> Mat3 {
        self.orientation
    }

    pub fn forward(&self) -> Vec3 {
        self.orientation.z_axis
    }

    pub fn up(&self) -> Vec3 {
        self.orientation.y_axis
    }

    /// Speed along the bike, ft/s.
    pub fn speed(&self) -> f32 {
        self.velocity.dot(self.forward())
    }

    /// Where to draw the bike: its model's origin and axes, in game
    /// coordinates.
    pub fn place(&self) -> Mat4 {
        let r = self.orientation;
        let origin = self.position - r * COM;
        let mut m = Mat4::from_mat3(r);
        m.w_axis = origin.extend(1.0);
        m
    }

    /// Where to draw the bike between steps (ours: the game draws once a
    /// step; drawing each frame at the last step's place judders when the
    /// screen runs at another rate than the 50 steps a second).
    pub fn drawn_place(&self) -> Mat4 {
        let now = self.place();
        let Some((position, orientation)) = self.previous else {
            return now;
        };
        if position.distance(self.position) > 20.0 {
            return now;
        }
        // A step the length of the frame lands where the frame is drawn.
        let t = if self.h < STEP { 1.0 } else { (self.pending / STEP).clamp(0.0, 1.0) };
        let from = Quat::from_mat3(&orientation);
        let to = Quat::from_mat3(&self.orientation);
        let r = Mat3::from_quat(from.slerp(to, t));
        let mut m = Mat4::from_mat3(r);
        m.w_axis = (position.lerp(self.position, t) - r * COM).extend(1.0);
        m
    }

    /// Suspension compression, 0 to 1, front and rear (for the model's
    /// Shocks and SwingArm frames).
    pub fn suspension(&self) -> [f32; 2] {
        self.tyres.map(|t| t.squeeze / t.travel)
    }

    /// How far the rear tyre is into the ground, ft (below 0 clear of it;
    /// tyre +0x150).
    pub fn rear_depth(&self) -> f32 {
        self.tyres[1].depth
    }

    /// Whether each tyre, front and rear, is on the ground.
    pub fn tyres_down(&self) -> [bool; 2] {
        self.tyres.map(|t| t.down)
    }

    /// Both shocks' compressions, front and rear, ft.
    pub fn squeezes(&self) -> [f32; 2] {
        self.tyres.map(|t| t.squeeze)
    }

    /// Advances the bike by `dt` seconds in the game's fixed steps.
    pub fn step(&mut self, ground: &dyn Ground, controls: Controls, dt: f32) {
        self.hit = None;
        // The game's accumulator (0x506b20): whole 0.02 s steps while the
        // frames are longer, at most three a frame, and then the rest of
        // the frame as one more step of its own (0x52ffb0, when over
        // 1e-4 s; a smaller rest waits for the next frame). A shorter
        // frame runs one step of its own length, so above 50 fps the step
        // follows the frame rate.
        let t = self.pending + dt;
        let n = (t / STEP) as u32;
        if n == 0 && self.fixed_step {
            self.pending = t;
            return;
        }
        if n == 0 {
            self.pending = 0.0;
            self.h = if dt == 0.0 { STEP } else { t };
            self.tick(ground, controls);
            return;
        }
        self.pending = t - n as f32 * STEP;
        for _ in 0..n.min(MAX_STEPS) {
            self.h = STEP;
            self.tick(ground, controls);
        }
        if !self.fixed_step && self.pending > 1e-4 {
            self.h = self.pending;
            self.pending = 0.0;
            self.tick(ground, controls);
        }
    }

    /// Where the rear tyre touches the ground, when it does, and how much
    /// it spins, 0 (gripping) to 1 (+0x2bc, one less the traction): what
    /// the dirt spray and dust come from (0x52f430, 0x52f650).
    pub fn rear_contact(&self) -> Option<(Vec3, f32)> {
        let t = &self.tyres[1];
        (t.down && self.crash.is_none()).then(|| (t.contact, 1.0 - self.traction))
    }

    /// Each tyre's contact while it is on the ground, front then rear, and
    /// whether it stands on a solid object (surface 7) rather than the
    /// terrain: where the dust and dirt come from (0x52f430, 0x52f650).
    pub fn tyre_contacts(&self) -> [Option<(Vec3, bool)>; 2] {
        self.tyres.map(|t| t.down.then_some((t.contact, t.on_object)))
    }

    /// How fast each drawn wheel turns, front then rear, rad/s, while its
    /// tyre is on the ground (0x519ab0, 0x518c40): the tyre's speed along
    /// itself over its radius, the rear's with its wheelspin (+0x290) on
    /// top. With both tyres down, a wheel below 0.1 ft/s stands still.
    /// None for a tyre in the air, where the wheel keeps turning as it was.
    pub fn wheel_rates(&self) -> [Option<f32>; 2] {
        let both = self.tyres.iter().all(|t| t.down);
        let spin = self.roost().2;
        [0, 1].map(|i| {
            let t = &self.tyres[i];
            if !t.down || self.crash.is_some() {
                return None;
            }
            let speed = self.velocity.dot(t.forward);
            if both && speed <= 0.1 {
                return Some(0.0);
            }
            let spin = if i == 1 { spin } else { 0.0 };
            Some((speed + spin) / TYRE_RADII[i])
        })
    }

    /// The rear tyre's traction (+0x2b8) and spin (+0x2bc, one less the
    /// traction), and how hard it throws: +0x290 over the step (0x519ab0),
    /// the spin times the drive over the mass.
    pub fn roost(&self) -> (f32, f32, f32) {
        let rider = if self.crash.is_some() { 0.0 } else { RIDER };
        let mass = (self.spec.weight + rider) * 0.031_055_9;
        let spin = 1.0 - self.traction;
        (self.traction, spin, spin * self.drive / mass)
    }

    /// Turns the body by `angle` about `axis` (world, unit length), around
    /// `pivot`.
    fn turn(&mut self, axis: Vec3, angle: f32, pivot: Vec3) {
        if angle == 0.0 || !axis.is_normalized() {
            return;
        }
        let q = Mat3::from_quat(Quat::from_axis_angle(axis, angle));
        self.orientation = q * self.orientation;
        self.position = pivot + q * (self.position - pivot);
    }

    /// One step of `self.h`.
    fn tick(&mut self, ground: &dyn Ground, c: Controls) {
        self.previous = Some((self.position, self.orientation));
        let h = self.h;
        // The shocks spring back and damp over the step before this one
        // (0x52e520 and 0x519350 take +0x1e8, not +0x13c): what moved
        // them happened then, so a short last step of a frame (0x52ffb0)
        // doesn't see the whole 0.02 s step's motion as a sudden jolt.
        let shock_h = self.last_h;
        self.last_h = h;
        let crashed = self.crash.is_some();
        let c = if crashed { Controls::default() } else { c };
        self.rider.0 = c.steer.clamp(-1.0, 1.0);
        self.rider.1 += (c.lean.clamp(-1.0, 1.0) - self.rider.1).clamp(-0.25, 0.25);
        if !crashed {
            self.pose(h);
        }
        // Bike and rider, or the bike alone once he is off (0x505760:
        // mass = weight × 0.0310559).
        let rider = if crashed { 0.0 } else { RIDER };
        let weight = self.spec.weight + rider;
        let mass = weight * 0.031_055_9;
        let speed_before = self.velocity.length();

        // Bars, engine and brake. The bars reach the input at once.
        let full = STEER_LOCK * self.sliders.steering;
        let lock = (full - (full * full * speed_before / STEER_FADE).sqrt()).clamp(0.0, full);
        self.steer = c.steer.clamp(-1.0, 1.0) * lock;
        self.drivetrain.update(h, c.throttle > 0.0, self.clutch);
        self.brake = if c.brake > 0.0 {
            (self.brake + BRAKE_ON * self.sliders.press_brakes * h).min(c.brake.min(1.0))
        } else {
            (self.brake - BRAKE_OFF * self.sliders.release_brakes * h).max(0.0)
        };

        if crashed {
            self.tumble(ground, h);
            self.orthonormalize();
            self.update_angles();
            self.last_velocity = self.velocity;
            self.crash_step(ground, h);
            return;
        }

        // Tyres on the ground and their share of the weight.
        for i in 0..2 {
            self.contact(ground, i);
            self.relax(i, shock_h);
        }
        let down = self.tyres.iter().filter(|t| t.down).count();
        self.airborne = down == 0;
        self.update_loads();

        // Gravity and drag, which the tyres' loads see (0x406ee0).
        let mut force = Vec3::new(0.0, -weight, 0.0);
        let mut torque = Vec3::ZERO;
        let mut brake_torque = Vec3::ZERO;
        let mut budget = speed_before;
        // How much of a resistance of `want` lb fits: no more than would
        // take `most` ft/s off this step, and all of them together no more
        // than the speed.
        let room = |budget: &mut f32, want: f32, most: f32| -> f32 {
            let most = most.min(*budget).max(0.0);
            let fits = mass / h * most;
            if want <= fits {
                *budget -= want * h / mass;
                want
            } else {
                *budget -= most;
                fits
            }
        };
        if speed_before > 0.001 {
            let want = DRAG * speed_before * speed_before;
            let got = room(&mut budget, want, speed_before);
            force -= self.velocity / speed_before * got;
        }
        let carried = force;
        let right = self.orientation.x_axis;
        let cos_lean = self.lean.cos();
        let grip_at = |t: &Tyre| t.friction(ground);

        // Shocks (0x4feb20, 0x52eac0): spring and damper along the
        // contact normal, less as the bike leans; only the pitch part of
        // their torque is kept.
        let lean_rate = (self.rider.1 - self.last_lean) / h;
        let pump = if self.lift > 5.0 && self.rider.1 > 0.4 && lean_rate > 11.0 {
            ((self.lift * 0.6).min(100.0) * lean_rate * 5.0).min(10000.0)
        } else {
            0.0
        };
        for i in 0..2 {
            // Only a touching tyre bottoms: 0x52ffb0 sets +0x5a0 from the
            // touching shocks and clears it when neither touches, so a
            // shock still squeezed from the last landing kicks nothing in
            // the air.
            if !self.tyres[i].down {
                self.tyres[i].bottomed = 0.0;
                continue;
            }
            let push = self.shock(i, shock_h, rider, crashed);
            let t = self.tyres[i];
            let n_force = (-(carried * t.load).dot(t.normal)).max(0.0);
            self.tyres[i].normal_force = n_force;
            let mut f = t.normal * push * cos_lean;
            // The rider's pump (0x40d3d0), up through each loaded shock.
            if n_force > 30.0 {
                f.y += pump;
            }
            let at = t.contact - self.position;
            torque += right * at.cross(f).dot(right);
            force += f;
        }
        self.bottom_out(ground, rider, crashed);

        // Drive at the rear tyre, cut by wheelspin (0x492b20, 0x519670).
        let rear = self.tyres[1];
        self.drive = 0.0;
        self.traction = 1.0;
        // Backing up replaces the drive (0x4933e0: slow, Reverse held, no
        // gas).
        let backing = self.reverse && !crashed && c.throttle <= 0.0 && speed_before < BACK_UP_SPEED;
        if backing {
            let (phase, rate) = &mut self.paddle;
            if *phase < 0.45 {
                *rate = speed_before.max(1.0) * h * PADDLE_RATE;
            }
            *phase += *rate;
            if *phase > 1.0 {
                *phase -= 1.0;
            }
        } else {
            self.paddle.0 = 0.0;
        }
        if rear.down && backing {
            let phase = self.paddle.0;
            if (PUSH_START..PUSH_END).contains(&phase) {
                self.drive = -(PUSH_END - phase) * PUSH;
                force += rear.forward * self.drive;
            }
        } else if rear.down && !crashed && self.racing {
            // No drive at all before the start (0x492b20 skips 0x4d7900
            // until the race is on), so gas at the gate can't kick off a
            // power slide.
            let along = self.velocity.dot(rear.forward);
            let mut drive = self.drivetrain.drive_force(speed_before) * c.throttle.clamp(0.0, 1.0);
            if along < SPIN_SPEED {
                let x = drive / mass - self.accel;
                let s = if x < 1.01 {
                    1.0
                } else {
                    (400.0 - x.min(320.0)) * 0.0025
                };
                let g = 1.05 - self.rider.1 * 0.25;
                // Nose up by less than 37°, the rear grips better, up
                // to 1.25 level (0x519670: f_60 = cos pitch, f_a4 the
                // look's rise).
                let up = self.pitch.cos() * 1.25;
                let k = if self.forward().y > 0.0 && up > 1.0 {
                    up
                } else {
                    1.0
                };
                let least = (along / SPIN_SPEED).clamp(0.0, 1.0);
                let t = (s * g * k).clamp(least, 1.0) * rear.traction(ground);
                self.traction = t;
                drive *= t;
            }
            if !self.on_course {
                drive *= 0.5;
            }
            self.drive = drive;
            let f = rear.forward * drive;
            force += f;
            // The drive's torque about the centre (0x52f290 through
            // 0x492c50 → 0x407660) counts only with the rider leaning
            // back past 0.125: then it lifts the nose, less as the bike
            // already points up or down. Nose rising faster than 1 rad/s
            // last step, a fifth of it counts whatever the lean; nearly
            // stopped and pointing up past 50° on the front, none.
            let lift = if speed_before < 10.0 && self.pitch > 0.8727 && self.tyres[0].down {
                0.0
            } else if self.spin.x < -1.0 {
                0.2
            } else {
                1.6 * (1.4 - self.pitch.abs().min(1.0)) * (-(self.rider.1 + 0.125)).max(0.0)
            };
            torque += (rear.contact - self.position).cross(f) * lift;
        }

        // Brakes (0x519850), then rolling and side friction (0x51b1d0),
        // within what is left of the budget.
        let braking = self.tyres.iter().filter(|t| t.down).count();
        if self.brake > 0.0 && !crashed {
            for (t, lever) in self.tyres.iter().zip(BRAKE_TORQUE) {
                if !t.down {
                    continue;
                }
                let along = self.velocity.dot(t.forward);
                let want = grip_at(t) * BRAKE * self.brake;
                let got = room(&mut budget, want, along.abs() / braking as f32);
                let f = -t.forward * got * along.signum();
                brake_torque += (t.contact - self.position).cross(f) * lever;
                force += f;
            }
        }
        for (i, side) in SIDE.into_iter().enumerate() {
            let t = self.tyres[i];
            self.tyres[i].slip = 0.0;
            if !t.down {
                continue;
            }
            let v = self.velocity - t.normal * self.velocity.dot(t.normal);
            let s = v.length();
            if s < 1e-3 {
                continue;
            }
            let dir = v / s;
            // Slip against the tyre's own heading, the front's turned by
            // the bars (0x51b1d0 reads tyre +0x230 for each tyre).
            // The sine from the cross product, as the game reads +0x288:
            // on a face the heading and the path part by 1e-4 rad or so,
            // which sqrt(1 - cos²) rounds to nothing in single precision.
            let cos = dir.dot(t.forward).abs().min(1.0);
            let sin = dir.cross(t.forward).length().min(1.0);
            self.tyres[i].slip = sin;
            self.tyres[i].moving = dir;
            let want = (side * sin + ROLLING * cos) * t.normal_force * grip_at(&t);
            let got = room(&mut budget, want, s);
            force -= dir * got;
            // Its torque about the centre joins the brakes' (0x52d070,
            // r(+0xf0) x F into the static-friction accumulator).
            brake_torque += (t.contact - self.position).cross(-dir * got);
        }

        // Integrate the force: all of it in the race, only its vertical
        // part before the start (0x506ed0, 0x497590).
        if self.racing {
            self.velocity += force / mass * h;
        } else {
            self.velocity.y += force.y / mass * h;
        }
        let inv_inertia = Vec3::ONE / (INERTIA * mass / 12.0);
        let body_torque = self.orientation.transpose() * torque;
        let mut alpha = body_torque * inv_inertia;
        // The brakes' torque works like static friction on the spin
        // (0x496c60 → 0x4cfe30): it can't start the bike turning, takes
        // the spin's other torques away up to its size while it is still,
        // and stops a turn rather than reverse it.
        let brake_alpha = self.orientation.transpose() * brake_torque * inv_inertia;
        for k in 0..3 {
            let b = brake_alpha[k];
            if b == 0.0 {
                continue;
            }
            if self.spin[k].abs() < STILL_SPIN {
                if alpha[k] != 0.0 && alpha[k].signum() != b.signum() {
                    alpha[k] = if b.abs() >= alpha[k].abs() {
                        0.0
                    } else {
                        alpha[k] + b
                    };
                }
            } else {
                let w = self.spin[k] + alpha[k] * h;
                if w.signum() == b.signum() || w.abs() > (b * h).abs() || w == 0.0 {
                    alpha[k] += b;
                } else {
                    self.spin[k] = 0.0;
                    alpha[k] = 0.0;
                }
            }
        }
        self.spin += alpha * h;
        self.spin *= SPIN_DECAY;

        // Ground or air handling. Its damping of the spin comes after the
        // spin has turned the bike this step (0x530825 and 0x40bcc0 run
        // after 0x52fd10), so a step turns by the undamped spin.
        let mut damp = 1.0;
        if down > 0 {
            let (sum, n) = self
                .tyres
                .iter()
                .filter(|t| t.down)
                .fold((Vec3::ZERO, 0.0), |(s, n), t| (s + t.contact, n + 1.0));
            self.air_time = 0.0;
            // What the step does on the ground (0x40d200): with both tyres
            // down, or one while turning, it rides; on the rear alone going
            // straight with the front near the ground it only leans, the
            // spin left undamped, so the nose drops over a crest; with the
            // other tyre far off it does neither.
            let [f, r] = &self.tyres;
            let near = |t: &Tyre| t.depth > -5.2;
            let ride = if down == 2 || self.turn_rate != 0.0 || self.steer != 0.0 {
                Some(true)
            } else if r.down {
                near(f).then_some(false)
            } else {
                near(r).then_some(true)
            };
            if let Some(full) = ride {
                // The spin is damped only on a step that turns the bike
                // (0x530825): riding straight, nothing holds the pitch.
                let turned = crashed || self.ride(ground, h, sum / n, full);
                if full && turned {
                    damp = GROUND_SPIN;
                }
            }
        } else {
            self.air_time += h;
            if !crashed {
                self.fly(c, h);
            }
            damp = AIR_SPIN;
        }

        // Spin and position (0x52fd10; Adams-Bashforth, 0x497490).
        let w = self.orientation * self.spin;
        let angle = w.length() * h;
        if angle > 1e-6 {
            self.turn(w.normalize(), angle, self.position);
        }
        self.spin *= damp;
        self.position += (self.velocity * 3.0 - self.last_velocity) * 0.5 * h;
        let lift = (self.velocity.y - self.last_velocity.y) / h;
        self.lift += (lift - self.lift) * (h / 0.1).min(1.0);
        self.accel = (self.velocity - self.last_velocity).length() / h;
        self.last_velocity = self.velocity;
        self.last_lean = self.rider.1;

        if crashed {
            self.fall_over(ground, h);
        }
        self.collide(ground, h, mass);
        self.orthonormalize();
        self.update_angles();

        if self.crash.is_some() {
            self.crash_step(ground, h);
        } else if self.off_the_edge(ground) {
            self.crash = Some((Crash::Edge, CRASH_TIME));
            self.wreck = Some(self.throw(Crash::Edge));
        } else if let Some(why) = self.check_crash(ground, speed_before) {
            self.crash = Some((why, self.crash_time));
            self.wreck = Some(self.throw(why));
        }
    }

    /// The crash's clock and the thrown rider; when it runs out, the rider
    /// is put back on the bike.
    fn crash_step(&mut self, ground: &dyn Ground, h: f32) {
        let Some((why, left)) = self.crash else {
            return;
        };
        let place = self.place();
        let over = match &mut self.wreck {
            Some(w) => w.step(ground, h, place, self.velocity),
            None => true,
        };
        let left = if self.touch_clock {
            self.touched |= self.wreck.as_ref().is_some_and(|w| w.touching);
            if self.touched {
                left - h
            } else {
                left
            }
        } else {
            left - h
        };
        self.crash = Some((why, left));
        if left <= 0.0 || (over && self.wreck.is_none()) {
            // Back on the course where the rider was last on it,
            // facing along it (0x4eb170); off the course graph, where
            // the rider came to rest, facing the way the bike was
            // going.
            let (at, dir) = match self.reset_to {
                Some((at, dir)) => (at - dir * RESET_BACK, dir),
                None => {
                    let at = self.wreck.as_ref().map_or(self.position, |w| w.position);
                    let look = self.forward();
                    let dir = if look.x.abs() + look.z.abs() > 1e-3 {
                        look
                    } else {
                        self.up()
                    };
                    (at, dir)
                }
            };
            self.reset(ground, Vec3::new(at.x, 0.0, at.z), dir);
        }
    }

    /// A crashed bike on its own (0x52ffb0 with the rider off): a rigid
    /// body under gravity that bounces and slides on its frame's corners,
    /// so it cartwheels and tumbles to rest.
    fn tumble(&mut self, ground: &dyn Ground, h: f32) {
        // The rider's weight stays in the mass (f_154 keeps 395 lb).
        let mass = (self.spec.weight + RIDER) * 0.031_055_9;
        let inertia = CRASHED_INERTIA * mass / 12.0;
        self.velocity.y -= GRAVITY_FT * h;
        self.position += self.velocity * h;
        let w = self.orientation * self.spin;
        let angle = w.length() * h;
        if angle > 1e-6 {
            self.turn(w.normalize(), angle, self.position);
        }
        let mut deepest = 0.0f32;
        let mut push = Vec3::ZERO;
        for (corner, radius) in TUMBLE_POINTS {
            let r = self.orientation * (corner - COM);
            let p = self.position + r;
            let (height, normal) = ground.terrain(p.x, p.z);
            let n = normal.normalize_or(Vec3::Y);
            let depth = (height + radius - p.y) * n.y;
            if depth <= 0.0 {
                continue;
            }
            if depth > deepest {
                deepest = depth;
                push = n * depth;
            }
            // The world inverse inertia, and the point's velocity.
            let inv = |v: Vec3| self.orientation * ((self.orientation.transpose() * v) / inertia);
            let w = self.orientation * self.spin;
            let v = self.velocity + w.cross(r);
            let vn = v.dot(n);
            if vn >= 0.0 {
                continue;
            }
            let k = 1.0 / mass + n.dot(inv(r.cross(n)).cross(r));
            let j = -(1.0 + TUMBLE_BOUNCE) * vn / k;
            let mut impulse = n * j;
            let vt = v - n * vn;
            if vt.length() > 1e-3 {
                let t = vt.normalize();
                let kt = 1.0 / mass + t.dot(inv(r.cross(t)).cross(r));
                impulse -= t * (TUMBLE_FRICTION * j).min(vt.length() / kt);
            }
            self.velocity += impulse / mass;
            let dw = inv(r.cross(impulse));
            self.spin += self.orientation.transpose() * dw;
        }
        self.position += push;
        self.spin *= SPIN_DECAY;
        self.airborne = deepest <= 0.0;
        // Walls stop it as they stop a rider.
        self.collide(ground, h, mass);
    }

    /// Moves the bike by `shift` and changes its velocity by `push`, as
    /// when another bike runs into it; `crash` throws the rider off (the
    /// game's crash 2, hit by another bike).
    pub fn knock(&mut self, shift: Vec3, push: Vec3, crash: bool) {
        self.position += shift;
        self.velocity += push;
        self.last_velocity += push;
        if crash && self.crash.is_none() {
            self.crash = Some((Crash::Hit, self.crash_time));
            self.wreck = Some(self.throw(Crash::Hit));
        }
    }

    /// Throws the rider off for `why`, as when a computer rider's
    /// two-wheel model finds a crash and hands the bike to this model.
    pub fn fall(&mut self, why: Crash) {
        if self.crash.is_none() {
            self.crash = Some((why, self.crash_time));
            self.wreck = Some(self.throw(why));
        }
    }

    /// The Kahuna Dumpster (action 8, 0x492650): in the air, both tyres
    /// more than 3.5 ft clear (0x5597ac) and no trick running, the rider
    /// lets go of the bike (`f_448` 2, the Kahuna bail).
    pub fn dump(&mut self) -> bool {
        let clear = self.tyres.iter().all(|t| t.depth < KAHUNA_CLEAR);
        if self.crash.is_some() || self.stunting || !self.airborne || !clear {
            return false;
        }
        self.crash = Some((Crash::Bail, self.crash_time));
        self.wreck = Some(self.throw(Crash::Bail));
        true
    }

    /// The rider's crash motion for `why` (the Wrecker's lists): a fall in
    /// the air (0x537980), else the bail 0x40a950 picks with the crash
    /// (`f_448`, an index into BackOver, Endo, Kahuna, BackOver, LeftOver,
    /// RightOver; these rules never pick Kahuna, only KrustyBike's deliberate
    /// bail on action 8 does, 0x492650).
    fn throw(&mut self, why: Crash) -> Wreck {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        let side = if self.lean > 0.0 {
            "RightOver"
        } else {
            "LeftOver"
        };
        let look = self.forward();
        let motion = match why {
            Crash::Bail => Some("Kahuna"),
            // `f_448` 1.
            Crash::Edge => Some("Endo"),
            // Thrown mid-trick, the rider falls (0x48e9b0 reads `f_430`).
            _ if self.stunting => None,
            Crash::Endo | Crash::Impact => Some("Endo"),
            // Nose straight down or up (rule 8), then the roll (rule 6).
            Crash::Flipped if look.y.abs() > CRASH_VERTICAL => {
                Some(if look.y <= 0.0 { "Endo" } else { "BackOver" })
            }
            Crash::Flipped | Crash::Lowside => Some(side),
            // The game sorts a body hit by the hit's push in the rider's
            // axes; this takes the bike's own velocity, reversed.
            Crash::Hit => {
                let push = self.orientation.transpose() * -self.velocity;
                Some(if push.x < -1.0 {
                    "RightOver"
                } else if push.x > 1.0 {
                    "LeftOver"
                } else if push.z >= -0.9 {
                    "BackOver"
                } else {
                    "Endo"
                })
            }
        };
        let (motion, bail) = match motion {
            Some(m) => (m, true),
            None => (wreck::fall(self.seed), false),
        };
        let spin = self.orientation * self.spin;
        Wreck::new(self.place(), self.velocity, spin, motion, bail, self.seed)
    }

    /// Where tyre `i` meets the ground with its shock fully out (0x519c30).
    fn contact(&mut self, ground: &dyn Ground, i: usize) {
        let r = self.orientation;
        let origin = self.position - r * COM;
        let extended = origin + r * axle(i, 0.0);
        // The tyre's heading on the ground; the bars turn the front about
        // the raked fork, so leaned into a turn its axle (tyre +0x23c)
        // tilts further than the bike.
        let (look, axle) = if i == 0 {
            let fork = (r * (HANDLEBARS - FRONT_AXLE)).normalize();
            let q = Quat::from_axis_angle(fork, self.steer);
            (q * r.z_axis, q * -r.x_axis)
        } else {
            (r.z_axis, -r.x_axis)
        };
        // The ground is felt where the wheel's rim meets it (0x519c30):
        // from the axle, the tyre's radius down the wheel's plane against
        // the ground's normal, sampled there. A second try takes its
        // normal a radius ahead (and toward the lean), so a face is met
        // before the axle is over it; the deeper of the two counts.
        let flat_axle = axle.normalize_or(Vec3::X);
        let radius = TYRE_RADII[i];
        let rim = |n: Vec3| -> (f32, Vec3, Vec3) {
            let along = n.cross(flat_axle).normalize_or(look);
            let mut down = along.cross(flat_axle).normalize_or(Vec3::NEG_Y);
            if down.dot(n) > 0.0 {
                down = -down;
            }
            let c = extended + down * radius;
            let (h, nc) = ground.terrain(c.x, c.z);
            let nc = nc.normalize_or(Vec3::Y);
            ((h - c.y) * nc.y, c, nc)
        };
        let (_, below) = ground.terrain(extended.x, extended.z);
        let (mut depth, mut contact, mut normal) = rim(below.normalize_or(Vec3::Y));
        let ahead = Vec3::new(look.x, 0.0, look.z).normalize_or(Vec3::Z);
        let toward = Vec3::new(r.x_axis.x, 0.0, r.x_axis.z).normalize_or(Vec3::ZERO);
        let probe = extended + ahead * radius + toward * (self.lean.sin() * radius);
        let (_, there) = ground.terrain(probe.x, probe.z);
        let second = rim(there.normalize_or(Vec3::Y));
        if second.0 > depth {
            (depth, contact, normal) = second;
        }
        // A solid object under the tyre (a bridge, a ramp) carries it
        // when it is pressed deeper than the terrain: a ray of the tyre's
        // radius from the axle, into the ground in the wheel's plane
        // (0x51af00, 0x4367f0), surface 7.
        let flat = axle.normalize_or(Vec3::X);
        let into = -(normal - flat * normal.dot(flat)).normalize_or(Vec3::Y);
        let mut on_object = false;
        let reach = radius + OBJECT_REACH;
        if let Some(c) = ground.crossing(extended, extended + into * reach) {
            let pressed = radius - c.along * reach;
            if pressed > depth {
                depth = pressed;
                contact = c.point;
                normal = c.normal.normalize_or(Vec3::Y);
                on_object = true;
            }
        }
        // The wheel sits where its shock last left it (0x519c30 starts
        // from the tyre frame's centre): the rim it meets the ground with,
        // and so the point the shock and friction push at, rides up (and,
        // on the raked fork, back) with the compression.
        contact += r * (self::axle(i, self.tyres[i].squeeze) - self::axle(i, 0.0));
        let t = &mut self.tyres[i];
        t.forward = normal.cross(axle).normalize_or(look);
        t.axle = axle.normalize_or(Vec3::NEG_X);
        t.normal = normal;
        t.contact = contact;
        t.depth = depth;
        t.down = depth > -0.01;
        t.on_object = on_object;
    }

    /// The shock springs back out no faster than its travel per second
    /// (0x52e520, 0x4ff100, 0x4ff950), and three times that again once
    /// the tyre is clear: a wheel that leaves a crest stays tucked up, and
    /// it touches only when the ground reaches it where it is.
    fn relax(&mut self, i: usize, h: f32) {
        let stroke = self.orientation * travel_axis(i, self.tyres[i].squeeze);
        let lean = self.lean.cos().max(0.05);
        let t = &mut self.tyres[i];
        let square = stroke.dot(t.normal).max(0.05);
        let reach = t.depth / square;
        let rate = t.travel * h / lean;
        // Springing out all the way resets the shock (0x4ff100 calls its
        // vtable slot 0, 0x4fe990), the damper's last compression (+0x8c)
        // too: a wheel that touches again next counts as compressing from
        // nothing rather than still extending.
        let out = |t: &mut Tyre, by: f32| {
            if t.squeeze > 0.0 {
                if t.squeeze <= by {
                    t.squeeze = 0.0;
                    t.felt = 0.0;
                } else {
                    t.squeeze -= by;
                }
            }
        };
        out(t, rate);
        if reach < t.squeeze && t.squeeze > 0.0 {
            out(t, 3.0 * rate);
        }
        t.down = reach * square > t.squeeze * square - 0.01;
        // Off the ground the shock isn't evaluated, and 0x52f090 sets its
        // last compression (+0x8c) to where it has relaxed to (+0x90), so
        // the damper next counts only the motion from there.
        if !t.down {
            t.felt = t.squeeze;
        }
    }

    /// The weight split between the tyres from where they touch (0x406ee0).
    /// The game gives each tyre the share of its own distance from the
    /// centre of mass, so the farther tyre carries more.
    fn update_loads(&mut self) {
        let [f, r] = [self.tyres[0], self.tyres[1]];
        let (front, rear) = match (f.down, r.down) {
            (true, true) => {
                let df = self.position.distance(f.contact);
                let dr = self.position.distance(r.contact);
                let sum = (df + dr).max(1e-3);
                (df / sum, dr / sum)
            }
            (true, false) => (1.0, 0.0),
            (false, true) => (0.0, 1.0),
            (false, false) => (0.5, 0.5),
        };
        self.tyres[0].load = front;
        self.tyres[1].load = rear;
        for t in &mut self.tyres {
            if !t.down {
                t.normal_force = 0.0;
            }
        }
    }

    /// A bottomed shock (the step 0x52ffb0, f_5a0 → 0x43b9d0, 0x5050b0): with any tyre
    /// past its travel the bike's collision points meet the terrain, the
    /// body is pushed out by the deepest of them along their mean normal,
    /// and it takes one impulse at their middle. Without collision points
    /// the bottomed tyre's own contact stands in.
    fn bottom_out(&mut self, ground: &dyn Ground, rider: f32, crashed: bool) {
        if self.tyres.iter().all(|t| t.bottomed <= 0.0) {
            return;
        }
        let Some(hull) = self.hull.clone() else {
            for t in self.tyres {
                if t.bottomed > 0.0 {
                    self.ground_impulse(t.contact, t.normal, t.bottomed, rider, crashed);
                }
            }
            return;
        };
        let mut deepest = 0.0f32;
        let (mut middle, mut normals, mut count) = (Vec3::ZERO, Vec3::ZERO, 0);
        let parts = [
            (Vec3::ZERO, &hull.body),
            (axle(0, self.tyres[0].squeeze), &hull.wheels[0]),
            (axle(1, self.tyres[1].squeeze), &hull.wheels[1]),
        ];
        for (offset, points) in parts {
            for &p in points {
                let at = self.position + self.orientation * (p + offset - COM);
                let (height, normal) = ground.terrain(at.x, at.z);
                let normal = normal.normalize_or(Vec3::Y);
                let depth = (height - at.y) * normal.y;
                if depth > 0.0 {
                    deepest = deepest.max(depth);
                    middle += at;
                    normals += normal;
                    count += 1;
                }
            }
        }
        if count > 0 {
            let normal = normals.normalize_or(Vec3::Y);
            self.ground_impulse(middle / count as f32, normal, deepest, rider, crashed);
        }
    }

    /// Out of the ground by `depth` along `normal`, and an impulse at
    /// `point` (0x5050b0): restitution f_14c (0.1, as crashed), against
    /// the mass and the inertia about that point, and ridden only 0.3,
    /// 0.1, 0.3 of the spin (world x, y, z for the contact's speed, body
    /// axes for the change) counts.
    fn ground_impulse(&mut self, point: Vec3, normal: Vec3, depth: f32, rider: f32, crashed: bool) {
        self.position += normal * depth;
        let mass = (self.spec.weight + rider) * 0.031_055_9;
        let r = point - self.position;
        let scale = if crashed {
            Vec3::ONE
        } else {
            Vec3::new(0.3, 0.1, 0.3)
        };
        let m = self.orientation;
        let into = (self.velocity + ((m * self.spin) * scale).cross(r)).dot(normal);
        if into < 0.0 {
            let inv_inertia = Vec3::ONE / (INERTIA * mass / 12.0);
            let arm = m.transpose() * r.cross(normal);
            let turn = m * (arm * inv_inertia);
            let j = -(1.0 + RESTITUTION) * into / (1.0 / mass + normal.dot(turn.cross(r)));
            self.velocity += normal * (j / mass);
            self.spin += arm * inv_inertia * scale * j;
        }
    }

    /// Tyre `i`'s shock: compresses to the ground and returns its push, lb
    /// (InlineShock 0x4ff1f0 / RotatingShock 0x4ffa50, force 0x4feb20).
    /// Past its travel the tyre bottoms out and the ground stops the body.
    fn shock(&mut self, i: usize, h: f32, rider: f32, crashed: bool) -> f32 {
        // The shock measures the ground along its own stroke (0x4ff1f0,
        // 0x4ffa50): the depth over how square the stroke stands to the
        // ground, so the raked fork compresses more than the tyre sinks.
        let stroke = self.orientation * travel_axis(i, self.tyres[i].squeeze);
        let square = stroke.dot(self.tyres[i].normal).max(0.05);
        let t = &mut self.tyres[i];
        let was = t.felt;
        t.squeeze = (t.depth / square).clamp(0.0, t.travel);
        t.felt = t.squeeze;
        // Bottomed only at the end of its travel (0x4fee80 sets +0x50 when
        // the compression reaches it); below that, depth − squeeze·square is
        // rounding.
        let bottom = if t.depth / square >= t.travel {
            t.depth - t.travel * square
        } else {
            0.0
        };
        t.bottomed = bottom;
        let (squeeze, travel, load) = (t.squeeze, t.travel, t.load);
        // The shock moves no faster than the bike along its axis (0x4fee80):
        // the ground stepping under a rolling wheel is not a hit.
        let along = {
            let axis = self.orientation * travel_axis(i, squeeze);
            let r = self.orientation * (axle(i, squeeze) - COM);
            let v = self.velocity + (self.orientation * self.spin).cross(r);
            v.dot(axis).abs()
        };
        let rate = ((squeeze - was) / h).clamp(-along, along);
        let (soft, stiff) = SPRING[i];
        let k = soft + (stiff - soft) * (squeeze / travel);
        // The damping mass (0x52ab70): most of the bike, all of the rider.
        let sprung = (SPRUNG * self.spec.weight + rider) / 32.2;
        let mut damper = 2.0 * DAMPING * (load.max(0.05) * sprung * k).sqrt() * rate;
        if rate.abs() > DAMPING_KNEE {
            damper *= ((rate.abs() - DAMPING_KNEE) * 0.5 + DAMPING_KNEE) / rate.abs();
        }
        if crashed {
            damper = 0.0;
        }
        // Only the part along the ground's normal pushes (0x52eac0), and
        // none at all from a bottomed shock: the shock reports it
        // (0x4ff1f0's last outputs), the tyre's +0x15c is set and
        // 0x52eac0 skips that tyre, so only the impulse above stops it.
        if bottom > 0.0 {
            return 0.0;
        }
        (k * squeeze + damper) * square
    }

    /// On the ground (0x406bd0, 0x52d800, 0x40ce00, 0x40c830): steering
    /// bends the velocity and the body yaws with it, the rear can slide,
    /// and the bike leans into the turn.
    /// Whether it turned the bike: by the bars' turn (f_4b8) of 1e-4 rad
    /// or more, or by a kick of more than that.
    fn ride(&mut self, ground: &dyn Ground, h: f32, pivot: Vec3, full: bool) -> bool {
        let [f, r] = self.tyres;
        let n =
            (f.normal * f.down as u8 as f32 + r.normal * r.down as u8 as f32).normalize_or(Vec3::Y);
        let mut yaw = 0.0;
        // Either tyre down with the other near (0x406bd0): the rear more
        // than 0.2 ft off the ground turns nothing even on the front.
        let near = f.down || (r.down && f.depth > -0.2);
        if full && self.steer.abs() > 1e-4 && near && r.depth > -0.2 {
            let grip = ((f.friction(ground) + r.friction(ground)) / 2.0).min(1.0);
            let side = n.cross(f.forward).normalize_or(Vec3::ZERO);
            let speed = self.velocity.length();
            // The speed along the ground (0x518cd0 → f_434): the velocity
            // flattened onto the front tyre's ground, whichever way the
            // wheel points.
            let along = (self.velocity - f.normal * self.velocity.dot(f.normal)).length();
            // The turning circle is the wheelbase over the sine of the
            // bars, times how square the front axle sits to the ground's
            // side (0x406eb0): leaned over, the bike turns tighter.
            let radius = WHEELBASE / self.steer.sin() * -f.axle.dot(side);
            let dv = along * along / radius * h * (1.5 * grip).min(1.0);
            if speed > 1e-3 {
                let turned = (self.velocity + side * dv).normalize_or(self.velocity) * speed;
                let flat = |v: Vec3| v - n * v.dot(n);
                let (a, b) = (flat(self.velocity), flat(turned));
                yaw = a.cross(b).dot(n).atan2(a.dot(b));
                self.velocity = turned;
            }
        }
        self.turn_rate = yaw / h;
        let corner = yaw;
        let ground_lean = self.ground_lean();

        // A power slide while the rear spins up leaned over, or else the
        // bike swings round to where it is going (0x40ce00), both only
        // with both tyres down (f_5a8).
        let rear_along = self.velocity.dot(r.forward);
        let both = f.down && r.down;
        if both && self.traction < 0.9 && self.drive > 0.0 && ground_lean.abs() > 0.005 {
            let spin = 1.0 - self.traction;
            let inv = 12.0 / (INERTIA.y * self.spec.mass());
            let kick =
                ((0.5 * WHEELBASE * spin + 0.5 * WHEELBASE) * inv * h * spin * self.drive * 32.0)
                    .min(1.57);
            let step = (kick * h * (ground_lean.abs() * 5.0).min(1.5)).copysign(ground_lean);
            self.slide = (step, -1.333 * h * step, (0.75 / h - 0.5).round() as i32);
        } else if self.slide.0 != 0.0 {
            let was = self.slide.0;
            self.slide.0 += self.slide.1;
            self.slide.2 -= 1;
            if self.slide.0.signum() != was.signum() || self.slide.2 < 0 {
                self.slide.0 = 0.0;
            }
        }
        if self.slide.0 != 0.0 {
            yaw += self.slide.0;
        } else if both && rear_along > 7.0 && r.slip >= 1e-4 && f.slip < 0.5 {
            // Against the way the rear was moving at the start of the
            // step, before the bars bent the velocity: measured after,
            // the heading runs a step's turn ahead of the velocity.
            // It gives way to the bars: a step that corners by more
            // than it would swing gets none of it (0x40cfa0), so a slide
            // held through a turn stays.
            let s = r.forward.cross(r.moving);
            if s.length() < 0.4795 {
                let a = s.length().asin().min(0.8 * h);
                if a >= corner.abs() {
                    yaw += a.copysign(s.dot(n));
                }
            }
        }
        if full {
            self.turn(n, yaw, pivot);
        }
        let turned = corner.abs() >= 1e-4 || (yaw - corner).abs() > 1e-4;

        // Lean: towards the turn, from the turn rate and the bars,
        // limited in angle and rate.
        let square = self.forward().dot(r.forward).abs();
        let mut target = (self.turn_rate * 0.2 + self.steer * 0.05) * square;
        if self.slide.0 != 0.0 && target.abs() < 0.087 {
            target = 0.087f32.copysign(self.slide.0);
        }
        let target = target.clamp(-LEAN_LIMIT, LEAN_LIMIT);
        // Righting itself (moving towards upright from the side it leans
        // to) goes at 0.8 of the rate (0x40c830).
        let side = |x: f32| if x >= 0.0 { 1 } else { -1 };
        // The lean it measures (0x40b220): the world's roll below 32 ft/s;
        // faster, the ground's roll weighs in as the bars turn, all of it
        // from 30° of bars.
        let current = if self.velocity.length() < 32.0 {
            self.lean
        } else {
            let k = ((0.523_598_8 - self.steer.abs()) * 1.909_859_3).max(0.0);
            ground_lean - (ground_lean - self.lean) * k
        };
        let rate = LEAN_RATE
            * if side(current - target) == side(current) {
                0.8
            } else {
                1.0
            };
        let d = (target - current).clamp(-rate * h, rate * h);
        let look = self.forward();
        self.turn(look, -d, pivot);
        self.lean += d;
        // Roll spin against the lean's turn is cancelled by 1.5 times its
        // rate, never past zero (0x40c830); spin with it is left alone.
        let c = -d / h;
        let w = self.spin.z;
        if c > 0.0 && w < 0.0 {
            self.spin.z = (w + 1.5 * c).min(0.0);
        } else if c <= 0.0 && w > 0.0 {
            self.spin.z = (w + 1.5 * c).max(0.0);
        }
        turned
    }

    /// The rider's air pose (0x407900). Falling clear of the ground, the
    /// front more than 2 ft up and the rear more than 1 ft, the pose turns
    /// on and follows the bars through the cross-up motion: right takes
    /// it to a quarter of the way through (full cross one way), left to
    /// three quarters (the other way), with the steps between kept so the
    /// bike swings back through the middle. Anywhere else (vtable +0xa4,
    /// 0x40d9d0) the pose ends, and the bike takes the turn the motion
    /// had drawn it at: it lands crossed up.
    fn pose(&mut self, h: f32) {
        // A trick or a taunt ends it too (0x496a20).
        let clear = self.airborne
            && !self.stunting
            && self.tyres[0].depth < -2.0
            && self.tyres[1].depth < -1.0;
        if !clear {
            if self.air_pose {
                let turn = self.cross_turn();
                self.orientation *= Mat3::from_quat(turn);
                self.orthonormalize();
                self.update_angles();
            }
            self.air_pose = false;
            return;
        }
        if !self.air_pose {
            self.air_bars = 0.0;
            self.air_phase = 1;
        }
        self.air_pose = true;
        // The game's bars read positive to the left.
        let bars = -(self.rider.0 * self.sliders.cross_up).clamp(-1.0, 1.0);
        self.air_bars += (bars - self.air_bars).clamp(-1.0, 1.0) * h.min(POSE_LAG) / POSE_LAG;
        let x = self.air_bars;
        let (phase, pose) = match self.air_phase {
            1 => {
                let p = -0.25 * x;
                if p < 0.0 {
                    (3, 0.5 - p)
                } else if p > 0.225 {
                    (2, 0.5 - p)
                } else {
                    (1, p)
                }
            }
            2 => {
                let p = 0.25 * x + 0.5;
                (if p > 0.5 { 3 } else { 2 }, p)
            }
            3 => {
                let p = 0.25 * x + 0.5;
                if p < 0.5 {
                    (1, 0.5 - p)
                } else if p > 0.725 {
                    (4, 1.5 - p)
                } else {
                    (3, p)
                }
            }
            _ => {
                let p = 1.0 - 0.25 * x;
                if p > 1.0 {
                    (1, p - 1.0)
                } else {
                    (4, p)
                }
            }
        };
        self.air_phase = phase;
        self.pose_phase = pose;
    }

    /// The cross-up motion's turn of the bike at the pose's phase, its
    /// frames spread over the phase 0 to 1 and turned between.
    fn cross_turn(&self) -> Quat {
        let Some(frames) = self.cross_up.as_deref().filter(|f| !f.is_empty()) else {
            return Quat::IDENTITY;
        };
        let at = self.pose_phase.clamp(0.0, 1.0) * (frames.len() - 1) as f32;
        let i = (at as usize).min(frames.len() - 1);
        let j = (i + 1).min(frames.len() - 1);
        frames[i].slerp(frames[j], at - i as f32)
    }

    /// How the bike is drawn: turned by the cross-up motion while the air
    /// pose is on.
    pub fn drawn_turn(&self) -> Quat {
        if self.air_pose && self.crash.is_none() {
            self.cross_turn()
        } else {
            Quat::IDENTITY
        }
    }

    /// In the air (0x40bcc0): the rider's lean pitches the bike and the
    /// bars yaw it, straight onto the angles; the flight path bends a
    /// little towards where the bike points; the roll rights itself.
    fn fly(&mut self, c: Controls, h: f32) {
        let mut pitch = self.rider.1 * self.sliders.pitch * AIR_PITCH * h;
        let yaw = c.steer * AIR_YAW * h;
        // Only while the air pose is on (f_6fc), the rear 2 ft up.
        if self.air_pose && self.tyres[1].depth < -2.0 {
            if self.brake_gyro && self.brake > 0.0 {
                pitch += AIR_PITCH * h * self.brake;
            }
            if self.gas_gyro && c.throttle > 0.0 && c.brake == 0.0 {
                pitch -= AIR_PITCH * h * self.drivetrain.throttle;
            }
            let speed = self.velocity.length();
            if speed > 1e-3 {
                let dir = self.velocity / speed;
                let look = self.forward();
                let dot = dir.dot(look);
                if dot > 0.55 && dot < 0.984 {
                    // Always the same side of the path: which one comes
                    // from the steer pose's phase (`f_65c < 0.5`), not
                    // from where the bike points.
                    let across = Vec3::new(dir.z, 0.0, -dir.x);
                    let sign = if self.pose_phase < 0.5 { 1.0 } else { -1.0 };
                    let push = (1.0 - dot) * 3.412_97 * speed * 0.075 * sign * h;
                    self.velocity += across * push;
                    self.velocity *= speed / (push + speed);
                }
            }
        }
        let angle = (pitch * pitch + yaw * yaw).sqrt();
        if angle > 1e-4 {
            let axis = self.orientation * Vec3::new(pitch, yaw, 0.0) / angle;
            self.turn(axis.normalize(), angle, self.position);
        }
        // Self-righting: faster the longer the bike is up.
        let t = self.air_time;
        let rate = (self.lean.abs() / h).min(t * t * t * 0.357_792);
        let look = self.forward();
        self.turn(look, (rate * h).copysign(self.lean), self.position);
    }

    /// A riderless bike on the ground falls onto its side and lies there
    /// (ours: the game's crashed bike is a body of spheres that topples by
    /// itself; ours balances on its tyres, so it is tipped over).
    fn fall_over(&mut self, ground: &dyn Ground, h: f32) {
        let (height, _) = ground.terrain(self.position.x, self.position.z);
        if self.position.y - height > TOPPLE_REACH {
            return;
        }
        let right = self.orientation.x_axis;
        let side = if right.y.abs() > 1e-3 {
            right.y
        } else {
            -self.lean
        };
        let target = Vec3::Y * side.signum();
        let angle = right.angle_between(target);
        if angle < 0.05 {
            self.topple = 0.0;
            return;
        }
        // Gravity tips it faster the further over it is.
        self.topple += (GRAVITY_FT / TOPPLE_HEIGHT * angle.cos().max(0.0) + 1.0) * h;
        let axis = right.cross(target).normalize_or_zero();
        let step = (self.topple * h).min(angle);
        self.turn(axis, step, self.position);
        self.spin *= 0.9;
    }

    /// The frame against the ground (ours, for the game's frame points)
    /// and the collision points against solid objects.
    fn collide(&mut self, ground: &dyn Ground, h: f32, mass: f32) {
        let (height, normal) = ground.terrain(self.position.x, self.position.z);
        let normal = normal.normalize_or(Vec3::Y);
        let into_ground = (height + BODY_CLEARANCE - self.position.y) * normal.y;
        if into_ground > 0.0 {
            self.position += normal * into_ground;
            let into = self.velocity.dot(normal);
            if into < 0.0 {
                self.velocity -= normal * into * (1.0 + BOTTOM_BOUNCE);
            }
            // The body slides on the dirt.
            let along = self.velocity - normal * self.velocity.dot(normal);
            let slow = (BODY_FRICTION * 32.2 * h).min(along.length());
            self.velocity -= along.normalize_or(Vec3::ZERO) * slow;
        }
        // Ridden it bounces 0.3, crashed 0.1 (0x52bb10).
        let bounce = if self.crash.is_some() {
            0.1
        } else {
            RESTITUTION
        };
        self.hit_objects(ground, mass, bounce);
    }

    /// The collision points against solid objects (0x52fc60, 0x439e10):
    /// each point that passed through a face from its front since the
    /// last step is pushed back out along the face, and the bike bounces
    /// off at their middle (0x405fb0 → 0x5050b0).
    fn hit_objects(&mut self, ground: &dyn Ground, mass: f32, bounce: f32) {
        let (Some(hull), Some((was_at, was_turned))) = (self.hull.clone(), self.previous) else {
            return;
        };
        let place = |at: Vec3, turned: Mat3, p: Vec3| at + turned * (p - COM);
        let mut out = Vec3::ZERO;
        let (mut middle, mut count) = (Vec3::ZERO, 0);
        let mut normals: Vec<Vec3> = Vec::new();
        let mut moving = Vec3::ZERO;
        // A wheel the probe has put on an object rides it on its tyre.
        // The probe is taken again where the step has moved the bike to
        // (0x519c30 runs ahead of the 0x52fc60 sweep): a tyre that has
        // just reached a steep face, like the railway's bank, rides onto
        // it instead of its rim points striking the face side-on and
        // rolling the bike over.
        let on_object = if self.crash.is_none() {
            let kept = self.tyres;
            for i in 0..2 {
                self.contact(ground, i);
            }
            let on = self.tyres.map(|t| t.on_object);
            self.tyres = kept;
            on
        } else {
            self.tyres.map(|t| t.on_object)
        };
        let none = Vec::new();
        let wheels = [0, 1].map(|i| {
            let t = &self.tyres[i];
            (
                axle(i, t.squeeze),
                if on_object[i] { &none } else { &hull.wheels[i] },
            )
        });
        for (offset, points) in [(Vec3::ZERO, &hull.body)].into_iter().chain(wheels) {
            for &p in points {
                let from = place(was_at, was_turned, p + offset);
                let to = place(self.position, self.orientation, p + offset);
                for _ in 0..4 {
                    let Some(c) = ground.crossing(from, to + out) else {
                        break;
                    };
                    out += c.normal * c.depth;
                    middle += c.point;
                    moving = c.velocity;
                    count += 1;
                    if !normals.iter().any(|n| n.distance(c.normal) < 1e-4) {
                        normals.push(c.normal);
                    }
                }
            }
        }
        if count == 0 {
            return;
        }
        self.hit = Some(middle / count as f32);
        let normal = normals.iter().sum::<Vec3>().normalize_or_zero();
        let r = middle / count as f32 - self.position;
        self.position += out + out.normalize_or_zero() * PUSH_MARGIN;
        let w = self.orientation * self.spin;
        // Against a moving vehicle it is the speed into it that bounces.
        let into = normal.dot(self.velocity + w.cross(r) - moving);
        if into < 0.0 {
            let box2 = if self.crash.is_some() { CRASHED_INERTIA } else { INERTIA };
            let inertia = box2 * mass / 12.0;
            let twist = self.orientation.transpose() * r.cross(normal) / inertia;
            let turn = (self.orientation * twist).cross(r).dot(normal);
            let j = -(1.0 + bounce) * into / (1.0 / mass + turn);
            self.velocity += normal * j / mass;
            self.spin += twist * j;
        }
    }

    /// Within 105 ft of the world's edge (0x48edd0), the bike is flung
    /// back toward the world's middle, 20 ft up: 0.055 of the way there a
    /// second across and 120 ft/s up, tumbling (0x492650).
    fn off_the_edge(&mut self, ground: &dyn Ground) -> bool {
        let Some((width, depth)) = ground.edge() else {
            return false;
        };
        let p = self.position;
        let near = |x: f32, size: f32| x < EDGE || x > size - EDGE;
        if !near(p.x, width) && !near(p.z, depth) {
            return false;
        }
        let middle = Vec3::new(width / 2.0, 20.0, depth / 2.0);
        self.velocity = (middle - p) * EDGE_THROW;
        self.velocity.y = EDGE_RISE;
        self.last_velocity = self.velocity;
        let mut rand = || {
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 17;
            self.seed ^= self.seed << 5;
            (self.seed >> 17) as f32 / 32768.0
        };
        // Up to a radian a second of pitch and yaw (`f_d8`).
        self.spin = Vec3::new(rand(), rand(), 0.02);
        true
    }

    /// The roll against the touching tyres' ground (0x52dcf0, f_4ac),
    /// not the world's: on a side slope an upright bike reads as leaned.
    /// The world lean when neither tyre touches.
    fn ground_lean(&self) -> f32 {
        let [f, r] = self.tyres;
        if !(f.down || r.down) {
            return self.lean;
        }
        let n =
            (f.normal * f.down as u8 as f32 + r.normal * r.down as u8 as f32).normalize_or(Vec3::Y);
        let m = self.orientation;
        (-m.x_axis.dot(n)).atan2(m.y_axis.dot(n))
    }

    fn check_crash(&self, ground: &dyn Ground, speed_before: f32) -> Option<Crash> {
        let h = self.h;
        let up = self.up();
        let look = self.forward();
        let head = self.position + up * HEAD;
        if head.y < ground.terrain(head.x, head.z).0 {
            return Some(Crash::Hit);
        }
        let [f, r] = self.tyres;
        // Over the bars: only the front down, the nose dropping (f_d8 > 0)
        // and the body more than 40° off the rear tyre's line (f_4b4), with
        // the air pose off unless airborne (f_6fc).
        let tipping = self.spin.x > 0.0 && look.dot(r.forward).abs() < CRASH_ENDO;
        if f.down && !r.down && tipping && (self.airborne || !self.air_pose) {
            return Some(Crash::Endo);
        }
        if look.y.abs() > CRASH_VERTICAL || self.lean.abs() > CRASH_ROLL {
            return Some(Crash::Flipped);
        }
        // Leaned over against the ground (f_4b0), not the world: hard over
        // on a banked turn the bike sits upright to the berm and rides on.
        if r.down && r.slip > CRASH_LOWSIDE && self.ground_lean().cos() < CRASH_LOWSIDE {
            return Some(Crash::Lowside);
        }
        // The limit scales with the step, which the game took to be 0.02 s
        // or longer; above 50 fps the step shrinks with the frame and a
        // hit that takes its speed in one step (a bounce off a face) would
        // crash at 120 fps where it doesn't at 50, so the limit stops
        // shrinking at 0.02 s.
        if self.velocity.length() - speed_before < -self.crash_tolerance * CRASH_DROP * h.max(STEP) {
            return Some(Crash::Impact);
        }
        None
    }

    fn orthonormalize(&mut self) {
        let look = self.orientation.z_axis.normalize();
        let right = self.orientation.y_axis.cross(look).normalize();
        let up = look.cross(right);
        self.orientation = Mat3::from_cols(right, up, look);
    }

    /// What the rider's riding motions are picked by (0x48f390).
    pub fn pose_inputs(&self) -> PoseInputs {
        let [front, rear] = self.tyres;
        let backing = self.reverse && self.crash.is_none() && self.paddle.0 > 0.0;
        PoseInputs {
            depth: [front.depth, rear.depth],
            rear_down: rear.down,
            rear_speed: self.velocity.dot(rear.forward),
            airborne: self.airborne,
            air_pose: self.air_pose.then_some(self.pose_phase),
            paddle: backing.then_some(self.paddle.0),
        }
    }
}

/// The bike's state the rider's riding motions read (0x48f390).
#[derive(Debug, Clone, Copy, Default)]
pub struct PoseInputs {
    /// How far into the ground each extended tyre would be, front and rear,
    /// ft (+0x150; negative is clear of it).
    pub depth: [f32; 2],
    pub rear_down: bool,
    /// The rear tyre's speed along itself, ft/s (+0x27c).
    pub rear_speed: f32,
    /// Off the ground (f_108).
    pub airborne: bool,
    /// The air pose's phase through the cross-up motion while it is on
    /// (`f_6fc`, `f_65c`).
    pub air_pose: Option<f32>,
    /// Backing up, and the paddle's phase 0 to 1 (`f_664`).
    pub paddle: Option<f32>,
}

/// The axle of tyre `i` (0 front, 1 rear) in model space with the
/// suspension `squeeze` feet in.
pub(crate) fn axle(i: usize, squeeze: f32) -> Vec3 {
    if i == 0 {
        let fork = (HANDLEBARS - FRONT_AXLE).normalize();
        FRONT_AXLE + fork * squeeze
    } else {
        let arm = REAR_AXLE - SWINGARM_PIVOT;
        let angle = squeeze / arm.length();
        SWINGARM_PIVOT + Quat::from_rotation_x(angle) * arm
    }
}

/// The way axle `i` moves as its shock compresses, body space.
fn travel_axis(i: usize, squeeze: f32) -> Vec3 {
    (axle(i, squeeze + 0.01) - axle(i, squeeze)).normalize()
}

fn swingarm_length() -> f32 {
    (REAR_AXLE - SWINGARM_PIVOT).length()
}

/// How far spring `i` sinks under `load` lb.
fn sag(i: usize, load: f32) -> f32 {
    let (soft, stiff) = SPRING[i];
    let travel = if i == 0 {
        FORK_TRAVEL
    } else {
        SWINGARM_SWING * swingarm_length()
    };
    // k(x)·x = load with k rising linearly over the travel.
    let a = (stiff - soft) / travel;
    (-soft + (soft * soft + 4.0 * a * load).sqrt()) / (2.0 * a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Flat;

    /// Flat ground with a kicker: up 6 ft over z 40 to 55, then a drop.
    struct Kicker;

    impl Ground for Kicker {
        fn terrain(&self, _x: f32, z: f32) -> (f32, Vec3) {
            if (40.0..55.0).contains(&z) {
                let slope = 0.4;
                ((z - 40.0) * slope, Vec3::new(0.0, 1.0, -slope).normalize())
            } else {
                (0.0, Vec3::Y)
            }
        }
    }

    fn ride(bike: &mut Bike, g: &dyn Ground, c: Controls, seconds: f32) -> bool {
        let mut flew = false;
        for _ in 0..(seconds * 60.0) as usize {
            bike.step(g, c, 1.0 / 60.0);
            flew |= bike.airborne;
            assert!(bike.position.is_finite(), "{bike:?}");
        }
        flew
    }

    const GAS: Controls = Controls {
        throttle: 1.0,
        brake: 0.0,
        steer: 0.0,
        lean: 0.0,
    };

    #[test]
    fn stands_still_on_its_springs() {
        let mut bike = Bike::new(&Flat, Vec3::ZERO, Vec3::Z);
        ride(&mut bike, &Flat, Controls::default(), 3.0);
        let origin = bike.place().w_axis;
        assert!(origin.y.abs() < 0.5, "origin {origin:?}");
        assert!(bike.speed().abs() < 0.1);
        assert!(bike.pitch.abs() < 0.05, "pitch {}", bike.pitch);
        assert!(bike.tyres_down() == [true, true]);
        let s = bike.suspension();
        assert!(
            s[0] > 0.02 && s[0] < 0.6 && s[1] > 0.02 && s[1] < 0.6,
            "{s:?}"
        );
        assert!(bike.crash.is_none());
    }

    #[test]
    fn held_at_the_gate_before_the_start() {
        let mut bike = Bike::new(&Flat, Vec3::ZERO, Vec3::Z);
        bike.racing = false;
        ride(&mut bike, &Flat, GAS, 2.0);
        assert!(bike.velocity.length() < 0.5, "{:?}", bike.velocity);
        assert!(bike.drivetrain.rpm > 0.0);
    }

    #[test]
    fn accelerates_to_a_top_speed() {
        let mut bike = Bike::new(&Flat, Vec3::ZERO, Vec3::Z);
        ride(&mut bike, &Flat, GAS, 2.0);
        let early = bike.speed();
        ride(&mut bike, &Flat, GAS, 30.0);
        let top = bike.speed();
        ride(&mut bike, &Flat, GAS, 2.0);
        assert!(early > 25.0, "after 2 s {early}");
        assert!(top > 80.0 && top < 200.0, "top {top}");
        assert!((bike.speed() - top).abs() < 2.0, "still accelerating");
        assert!(bike.drivetrain.gear > 0);
        assert!(bike.crash.is_none(), "{:?}", bike.crash);
    }

    #[test]
    fn turns_leans_and_brakes() {
        let mut bike = Bike::new(&Flat, Vec3::ZERO, Vec3::X);
        ride(&mut bike, &Flat, GAS, 3.0);
        let heading = bike.forward();
        let right = Controls {
            throttle: 0.5,
            steer: 1.0,
            ..Default::default()
        };
        ride(&mut bike, &Flat, right, 0.3);
        // Facing +x, a right turn swings towards -z (left-handed).
        assert!(bike.forward().z < heading.z - 0.2, "{:?}", bike.forward());
        assert!(bike.lean > 0.1, "lean {}", bike.lean);
        assert!(bike.crash.is_none(), "{:?}", bike.crash);
        let brake = Controls {
            brake: 1.0,
            ..Default::default()
        };
        ride(&mut bike, &Flat, brake, 5.0);
        assert!(bike.speed().abs() < 1.0, "speed {}", bike.speed());
        assert!(bike.lean.abs() < 0.1);
        assert!(bike.crash.is_none(), "{:?}", bike.crash);
    }

    #[test]
    fn backs_up_slowly() {
        let mut bike = Bike::new(&Flat, Vec3::ZERO, Vec3::Z);
        bike.reverse = true;
        ride(&mut bike, &Flat, Controls::default(), 3.0);
        let back = bike.velocity.dot(bike.forward());
        assert!(back < -0.5, "speed along {back}");
        assert!(bike.speed() < BACK_UP_SPEED + 1.0, "speed {}", bike.speed());
        assert!(bike.position.z < 0.0);
        // The gas ends it.
        bike.reverse = false;
        ride(&mut bike, &Flat, GAS, 2.0);
        assert!(bike.velocity.dot(bike.forward()) > 10.0);
    }

    #[test]
    fn off_the_course_is_slower() {
        let mut on = Bike::new(&Flat, Vec3::ZERO, Vec3::Z);
        let mut off = Bike::new(&Flat, Vec3::ZERO, Vec3::Z);
        off.on_course = false;
        ride(&mut on, &Flat, GAS, 4.0);
        ride(&mut off, &Flat, GAS, 4.0);
        assert!(
            off.speed() < on.speed() * 0.85,
            "{} vs {}",
            off.speed(),
            on.speed()
        );
    }

    #[test]
    fn jumps_and_lands() {
        let mut bike = Bike::new(&Kicker, Vec3::ZERO, Vec3::Z);
        let flew = ride(&mut bike, &Kicker, GAS, 6.0);
        assert!(flew, "never left the kicker");
        assert!(bike.position.z > 100.0);
        assert!(bike.crash.is_none(), "{:?}", bike.crash);
        assert!(!bike.airborne);
    }

    #[test]
    fn lands_crossed_up() {
        let mut bike = Bike::new(&Flat, Vec3::ZERO, Vec3::Z);
        // A made-up motion that yaws the bike 0.01 rad a frame.
        let frames = (0..61)
            .map(|i| Quat::from_rotation_y(i as f32 * 0.01))
            .collect();
        bike.cross_up = Some(std::sync::Arc::new(frames));
        bike.airborne = true;
        for t in &mut bike.tyres {
            t.depth = -3.0;
        }
        // Full right: the pose lags the bars, then holds a quarter of the
        // way through the motion, or just past it.
        bike.rider.0 = 1.0;
        bike.pose(0.02);
        assert!(bike.pose_phase > 0.0 && bike.pose_phase < 0.01);
        for _ in 0..150 {
            bike.pose(0.02);
        }
        assert!(
            (0.25..0.276).contains(&bike.pose_phase),
            "{}",
            bike.pose_phase
        );
        assert_eq!(bike.drawn_turn(), bike.cross_turn());
        // Within 2 ft of the ground the bike takes the drawn turn.
        let heading = bike.heading;
        let want = bike.pose_phase * 60.0 * 0.01;
        bike.tyres[0].depth = -1.0;
        bike.pose(0.02);
        assert!((bike.heading - heading - want).abs() < 1e-3);
        assert_eq!(bike.drawn_turn(), Quat::IDENTITY);
    }

    #[test]
    fn lean_pitches_in_the_air() {
        let pitch_after = |lean: f32| {
            let mut bike = Bike::new(&Kicker, Vec3::ZERO, Vec3::Z);
            let mut c = GAS;
            let mut most: f32 = 0.0;
            for _ in 0..600 {
                if bike.airborne {
                    c.lean = lean;
                }
                bike.step(&Kicker, c, 1.0 / 60.0);
                if bike.airborne {
                    most = if lean < 0.0 {
                        most.max(bike.pitch)
                    } else {
                        most.min(bike.pitch)
                    };
                }
            }
            most
        };
        let back = pitch_after(-1.0);
        let forward = pitch_after(1.0);
        assert!(back > forward + 0.3, "back {back} forward {forward}");
    }

    #[test]
    fn the_world_edge_throws_the_rider_back_in() {
        struct Island;
        impl Ground for Island {
            fn terrain(&self, _: f32, _: f32) -> (f32, Vec3) {
                (0.0, Vec3::Y)
            }
            fn edge(&self) -> Option<(f32, f32)> {
                Some((4096.0, 4096.0))
            }
        }
        let mut bike = Bike::new(&Island, Vec3::new(2048.0, 0.0, 4000.0), Vec3::Z);
        ride(&mut bike, &Island, Controls::default(), 0.1);
        assert!(matches!(bike.crash, Some((Crash::Edge, _))));
        assert_eq!(bike.wreck.as_ref().unwrap().motion, "Endo");
        // Back toward the middle and up.
        assert!(bike.velocity.z < 0.0 && bike.velocity.y > 50.0, "{}", bike.velocity);
        // In from the edge it rides on.
        let mut bike = Bike::new(&Island, Vec3::new(2048.0, 0.0, 3900.0), Vec3::Z);
        ride(&mut bike, &Island, Controls::default(), 0.1);
        assert!(bike.crash.is_none());
    }

    #[test]
    fn thrown_mid_trick_falls_and_kahuna_bails() {
        let mut bike = Bike::new(&Flat, Vec3::new(0.0, 10.0, 0.0), Vec3::Z);
        let at = bike.position;
        bike.turn(Vec3::Z, std::f32::consts::PI, at);
        bike.update_angles();
        bike.stunting = true;
        ride(&mut bike, &Flat, Controls::default(), 0.5);
        let w = bike.wreck.as_ref().expect("no wreck");
        assert!(wreck::FALLS.contains(&w.motion) || wreck::HITS.contains(&w.motion), "{}", w.motion);

        let mut bike = Bike::new(&Flat, Vec3::ZERO, Vec3::Z);
        bike.position.y += 30.0;
        ride(&mut bike, &Flat, Controls::default(), 0.2);
        assert!(bike.dump(), "{:?} {}", bike.pose_inputs().depth, bike.airborne);
        assert_eq!(bike.wreck.as_ref().unwrap().motion, "Kahuna");
        // Not on the ground.
        let mut bike = Bike::new(&Flat, Vec3::new(0.0, 0.0, 0.0), Vec3::Z);
        ride(&mut bike, &Flat, Controls::default(), 0.5);
        assert!(!bike.dump());
    }

    #[test]
    fn upside_down_is_a_crash_and_it_recovers() {
        let mut bike = Bike::new(&Flat, Vec3::new(0.0, 10.0, 0.0), Vec3::Z);
        let at = bike.position;
        bike.turn(Vec3::Z, std::f32::consts::PI, at);
        bike.update_angles();
        ride(&mut bike, &Flat, Controls::default(), 0.5);
        assert!(bike.crash.is_some());
        // A bail (0x48e9b0 falls only mid-trick or crash 9), or the fall
        // after a short one has let go.
        let w = bike.wreck.as_ref().expect("no wreck");
        let bailed = ["LeftOver", "RightOver", "BackOver", "Endo"].contains(&w.motion);
        assert!(bailed || (crate::wreck::FALLS.contains(&w.motion) && w.stage != crate::wreck::Stage::Bail), "{}", w.motion);
        ride(&mut bike, &Flat, Controls::default(), 6.5);
        assert!(bike.wreck.is_none());
        assert!(bike.crash.is_none());
        assert!(bike.up().y > 0.9);
    }
}
