// Grass for the enhanced drawing (grass.rs), none of it the game's: tufts
// on a grid round the camera, each where the painted ground is grassy,
// three crossed cards whose blades are drawn in the shader, swaying in the
// wind and lit and shadowed as the ground is.

struct Globals {
    view_proj: mat4x4<f32>,
    camera: vec4<f32>,
    grid: vec4<f32>,
    fog: vec4<f32>,
    fog_range: vec4<f32>,
    ambient: vec4<f32>,
    sun: vec4<f32>,
    to_sun: vec4<f32>,
    shadow_view: mat4x4<f32>,
    shadow_on: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var textures: texture_2d_array<f32>;
@group(0) @binding(2) var tex_sampler: sampler;
@group(0) @binding(3) var tile_map: texture_2d<u32>;

#include "enhanced.wgsl"

@group(1) @binding(0) var<uniform> enh: Enh;
@group(1) @binding(1) var sun_map: texture_depth_2d_array;
@group(1) @binding(2) var sun_sampler: sampler_comparison;
@group(1) @binding(6) var rut_map: texture_2d<f32>;
@group(1) @binding(7) var rut_sampler: sampler;

#include "enhanced_fns.wgsl"

// The terrain's heights, in terrain units.
@group(2) @binding(0) var heights: texture_2d<f32>;

// Tufts a side of the grid, and ft between them.
const SIDE: u32 = 320u;
const STEP: f32 = 1.0;
// Where the grass thins out and ends, ft.
const FADE: f32 = 90.0;
const END: f32 = 150.0;
// The far layer: bigger tufts, further apart, from where the near ones thin
// out to as far again, so that the ground has its grass before the bike gets
// there. (Tufts a side, ft between them, where it ends, how much bigger.)
const FAR_SIDE: u32 = 256u;
const FAR_STEP: f32 = 2.6;
const FAR_END: f32 = 330.0;
const FAR_SIZE: f32 = 2.4;

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn height_at(t: vec2<f32>) -> f32 {
    let size = i32(globals.grid.x);
    let c = clamp(t, vec2<f32>(0.0), vec2<f32>(f32(size - 1)));
    let i = vec2<i32>(floor(c));
    let f = c - vec2<f32>(i);
    let j = min(i + 1, vec2<i32>(size - 1));
    let a = textureLoad(heights, i, 0).r;
    let b = textureLoad(heights, vec2<i32>(j.x, i.y), 0).r;
    let d = textureLoad(heights, vec2<i32>(i.x, j.y), 0).r;
    let e = textureLoad(heights, j, 0).r;
    return mix(mix(a, b, f.x), mix(d, e, f.x), f.y);
}

// The painted ground's colour at terrain point `t`, blurred a little.
fn painted_at(t: vec2<f32>) -> vec3<f32> {
    let tiles = i32(globals.grid.w);
    let tc = t / globals.grid.z;
    let cell = clamp(vec2<i32>(floor(tc)), vec2<i32>(0), vec2<i32>(tiles - 1));
    let layer = textureLoad(tile_map, cell, 0).r;
    if (layer == 0xffffu) {
        return vec3<f32>(0.3, 0.3, 0.2);
    }
    return textureSampleLevel(textures, tex_sampler, tc - vec2<f32>(cell), layer, 3.0).rgb;
}

// How grassy painted colour `p` (linear) is, as the ground's shader sorts it.
fn grassy(p: vec3<f32>) -> f32 {
    let c = pow(p, vec3<f32>(1.0 / 2.2));
    return smoothstep(0.0, 0.06, c.g - c.r * 0.86) * smoothstep(0.0, 0.04, c.g - c.b);
}

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world: vec3<f32>,
    @location(2) colour: vec3<f32>,
    @location(3) @interpolate(flat) seed: f32,
    @location(4) normal: vec3<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> Out {
    var out: Out;
    out.clip = vec4<f32>(0.0, 0.0, 2.0, 1.0);
    let scale = globals.grid.y;
    // The tuft's place: its grid cell round the camera, moved a little.
    let near_tufts = SIDE * SIDE;
    let far = ii >= near_tufts;
    let step_ft = select(STEP, FAR_STEP, far);
    let side = select(SIDE, FAR_SIDE, far);
    let end = select(END, FAR_END, far);
    let n = select(ii, ii - near_tufts, far);
    let centre = floor(globals.camera.xz / step_ft);
    let cell = centre + vec2<f32>(f32(n % side), f32(n / side)) - f32(side / 2u);
    let r1 = hash(cell);
    let r2 = hash(cell + 17.31);
    let r3 = hash(cell + 41.7);
    let xz = (cell + vec2<f32>(r1, r2)) * step_ft;
    let dist = length(xz - globals.camera.xz);
    if (dist > end || (far && dist < 95.0)) {
        return out;
    }
    let t = vec2<f32>(xz.x, -xz.y) / scale;
    let painted = painted_at(t);
    var density = grassy(painted);
    // None where the tyres have worn it away.
    if (enh.ruts.w > 0.5) {
        let ruv = (xz - enh.ruts.xy) / enh.ruts.z;
        density *= 1.0 - smoothstep(0.1, 0.4, textureSampleLevel(rut_map, rut_sampler, ruv, 0.0).r);
    }
    // Thinner further off.
    density *= 1.0 - smoothstep(FADE, END, dist) * 0.5 * select(1.0, 0.0, far);
    if (r3 > density) {
        return out;
    }
    let ground = height_at(t) * scale;
    // Three cards crossed, each two triangles.
    let card = vi / 6u;
    let corner = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    )[vi % 6u];
    let turn = r1 * 6.2832 + f32(card) * 2.0944;
    let across = vec3<f32>(cos(turn), 0.0, sin(turn));
    // Taller a little with distance, to keep the ground looking covered, but
    // down to nothing at the end, so no ring shows and no tuft pops in or out
    // as the camera moves.
    var tall = mix(0.7, 1.6, hash(cell + 3.3));
    var wide = 1.45 + 0.3 * r2;
    if (far) {
        // Clumps at a distance: grown in from where the near tufts end, and
        // down again at the far end.
        let k = smoothstep(95.0, 150.0, dist) * (1.0 - smoothstep(FAR_END * 0.7, FAR_END, dist));
        tall *= FAR_SIZE * k;
        wide *= FAR_SIZE * k;
    } else {
        // Taller a little with distance, to keep the ground looking covered,
        // but down to nothing at the end, so no ring shows and no tuft pops
        // in or out as the camera moves.
        tall *= (1.0 + smoothstep(FADE * 0.5, END * 0.7, dist) * 0.4) * (1.0 - smoothstep(END * 0.6, END, dist));
    }
    // Sway in the wind, more at the tips.
    let time = enh.world.x;
    let gust = sin(time * 1.6 + xz.x * 0.21 + xz.y * 0.17) * 0.5 + sin(time * 2.7 + xz.x * 0.5) * 0.25;
    let bend = vec3<f32>(0.7, 0.0, 0.4) * gust * 0.18 * corner.y * corner.y * tall;
    let world = vec3<f32>(xz.x, ground - 0.05, xz.y) + across * (corner.x - 0.5) * wide + vec3<f32>(0.0, corner.y * tall, 0.0) + bend;
    out.clip = globals.view_proj * vec4<f32>(world, 1.0);
    out.uv = corner;
    out.world = world;
    // The grass's colour is the ground's, a little fresher.
    let c = pow(painted, vec3<f32>(1.0 / 2.2));
    let lum = dot(c, vec3<f32>(0.3, 0.59, 0.11));
    let fresh = mix(vec3<f32>(lum), c, 1.4) * mix(1.0, 1.35, hash(cell + 9.1));
    out.colour = pow(max(fresh, vec3<f32>(0.0)), vec3<f32>(2.2));
    out.seed = hash(cell + 5.5);
    out.normal = normalize(vec3<f32>(0.0, 1.0, 0.0) + cross(across, vec3<f32>(0.0, 1.0, 0.0)) * 0.4);
    return out;
}

@fragment
fn fs_main(in: Out, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // Ten blades on the card, each narrowing to its tip.
    var cover = 0.0;
    for (var k = 0; k < 10; k++) {
        let s = in.seed * 13.0 + f32(k) * 1.618;
        let x = fract(s * 0.73) * 0.84 + 0.08;
        let h = 0.55 + fract(s * 1.37) * 0.45;
        let lean = (fract(s * 2.11) - 0.5) * 0.5;
        let w = 0.05 * (1.0 - in.uv.y / h);
        let d = abs(in.uv.x - (x + lean * in.uv.y * in.uv.y));
        cover = max(cover, smoothstep(w, w * 0.4, d) * step(in.uv.y, h));
    }
    if (cover < 0.05) {
        discard;
    }
    let dist = length(in.world - globals.camera.xyz);
    var n = normalize(in.normal);
    if (!front) {
        n = -n;
    }
    let sun = sun_visible(in.world, vec3<f32>(0.0, 1.0, 0.0), dist, 0.0);
    // Darker at the roots, where the light is hidden; light through the
    // blades when the sun is behind.
    let root = mix(0.45, 1.0, smoothstep(0.0, 0.7, in.uv.y));
    let facing = max(dot(n, globals.to_sun.xyz), 0.0) * 0.6 + 0.4 * max(globals.to_sun.y, 0.0);
    let v = normalize(globals.camera.xyz - in.world);
    let through = pow(max(dot(-v, globals.to_sun.xyz), 0.0), 4.0) * 0.5;
    let game = globals.ambient.rgb * 1.05 + globals.sun.rgb * (facing + through) * sun;
    let lit = in.colour * pow(soft_clamp(game), vec3<f32>(2.2)) * root * (1.0 - enh.world.y * 0.2);
    return vec4<f32>(apply_haze(lit, in.world, dist), cover);
}
