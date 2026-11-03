// Dust clouds behind the bikes for the enhanced drawing (dust.rs), none of
// them the game's: soft, lumpy puffs the colour of the ground they rose
// from, wispy at the edges and thicker in clumps, lit as balls of dust
// (bright toward the sun, darker beneath), that billow, drift and thin
// out, shadowed as the ground is.

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

#include "enhanced_fns.wgsl"

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) centre: vec3<f32>,
    @location(2) @interpolate(flat) look: vec4<f32>,
    @location(3) @interpolate(flat) colour: vec3<f32>,
    // Across the puff, in the world: with the way to the eye, its surface.
    @location(4) side: vec3<f32>,
    @location(5) @interpolate(flat) to_eye: vec3<f32>,
    // 1 for a drop of spray.
    @location(6) @interpolate(flat) drop: f32,
    // The puff's own across and up, in the world.
    @location(7) @interpolate(flat) right: vec3<f32>,
};

// The painted ground's colour (linear) under drawing point `xz`, blurred.
fn ground_colour(xz: vec2<f32>) -> vec3<f32> {
    let t = vec2<f32>(xz.x, -xz.y) / globals.grid.y;
    let tiles = i32(globals.grid.w);
    let tc = t / globals.grid.z;
    let cell = clamp(vec2<i32>(floor(tc)), vec2<i32>(0), vec2<i32>(tiles - 1));
    let layer = textureLoad(tile_map, cell, 0).r;
    if (layer == 0xffffu) {
        return vec3<f32>(0.35, 0.28, 0.2);
    }
    return textureSampleLevel(textures, tex_sampler, tc - vec2<f32>(cell), layer, 4.0).rgb;
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @location(0) place: vec4<f32>, @location(1) look: vec4<f32>, @location(2) home: vec4<f32>) -> Out {
    let corner = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    )[vi];
    let to_eye = normalize(globals.camera.xyz - place.xyz);
    let right = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), to_eye));
    let up = cross(to_eye, right);
    // Brought toward the eye by its size and shrunk to match, so it looks
    // the same but the ground in front of it does not cut it off flat.
    let d = length(globals.camera.xyz - place.xyz);
    let pull = select(min(place.w, d * 0.5), 0.0, home.z > 0.5);
    var side = right * corner.x + up * corner.y;
    var world = place.xyz + to_eye * pull + side * place.w * (d - pull) / max(d, 1e-3);
    if (home.z > 0.5) {
        // A drop is drawn streaked along the way it flies, as in a blink.
        let v = look.yzw - to_eye * dot(look.yzw, to_eye);
        let along = select(up, normalize(v), length(v) > 1e-3);
        let across = normalize(cross(along, to_eye));
        let reach = place.w + length(v) * 0.02;
        side = across * corner.x + along * corner.y;
        world = place.xyz + across * corner.x * place.w + along * corner.y * reach;
    }
    var out: Out;
    out.clip = globals.view_proj * vec4<f32>(world, 1.0);
    out.uv = corner;
    out.centre = place.xyz;
    out.look = look;
    // Lighter than the ground and a little paler, as fine dust is, but
    // keeping its warmth.
    let g = ground_colour(home.xy);
    let grey = vec3<f32>(dot(g, vec3<f32>(0.3, 0.59, 0.11)));
    out.colour = mix(grey, g, 0.8) * 2.3 + vec3<f32>(0.06, 0.05, 0.035);
    if (home.z > 0.5) {
        // Muddy water: the ground's colour, a little darker.
        out.colour = g * 0.7;
    }
    out.side = side;
    out.to_eye = to_eye;
    out.drop = home.z;
    out.right = right;
    return out;
}

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2<f32>(1.0, 0.0)), u.x), mix(hash(i + vec2<f32>(0.0, 1.0)), hash(i + vec2<f32>(1.0, 1.0)), u.x), u.y);
}

// The dust's lumps at `q`, churned by `churn`: four layers of noise.
fn lumps(q: vec2<f32>, churn: f32) -> f32 {
    return noise(q) * 0.5 + noise(q * 2.1 + 3.1 - churn * 0.6) * 0.27 + noise(q * 4.3 + 7.7) * 0.15 + noise(q * 8.9 + 1.3 + churn) * 0.08;
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    let dist = length(in.centre - globals.camera.xyz);
    if (in.drop > 0.5) {
        // A drop: a soft streak, the sky's light caught in it.
        let r = length(in.uv);
        if (r > 1.0) {
            discard;
        }
        let a = (1.0 - smoothstep(0.3, 1.0, r)) * in.look.x * smoothstep(1.5, 4.0, dist);
        let lit = globals.ambient.rgb * 1.1 + globals.sun.rgb * 0.5;
        let c = in.colour * pow(soft_clamp(lit), vec3<f32>(2.2)) + enh.horizon.rgb * 0.03;
        return vec4<f32>(apply_haze(c, in.centre, dist) * a, a);
    }
    // Lumps of dust from four layers of noise that churn and roll as it
    // ages: wispy at the rim, clumped and thicker inside.
    let s = in.look.y * 50.0;
    let churn = in.look.z * 0.35;
    let q = in.uv * 1.6 + vec2<f32>(s, s * 0.7) + vec2<f32>(churn * 0.4, -churn);
    let n = lumps(q, churn);
    let r = length(in.uv) + (n - 0.5) * 0.9;
    if (r > 1.0) {
        discard;
    }
    let body = 1.0 - smoothstep(0.2, 0.98, r);
    let clumps = mix(0.45, 1.5, smoothstep(0.3, 0.72, n));
    let alpha = clamp(body * clumps, 0.0, 1.0) * in.look.x;
    if (alpha < 0.004) {
        discard;
    }
    // Lit as a ball: its surface faces out from the middle and toward the
    // eye, tilted by each lump, so the lumps are bright on the sun's side
    // and shaded on the other; the light wraps round a little, as through
    // dust.
    let ball = normalize(in.side * min(r, 1.0) + in.to_eye * sqrt(max(1.0 - r * r, 0.05)));
    let up = cross(in.to_eye, in.right);
    let slope = vec2<f32>(lumps(q + vec2<f32>(0.06, 0.0), churn) - n, lumps(q + vec2<f32>(0.0, 0.06), churn) - n) / 0.06;
    let sn = normalize(ball - (in.right * slope.x + up * slope.y) * 0.35);
    // Shadowed at its top, so the bike's own shadow does not black out
    // the dust right behind it.
    let sun = sun_visible(in.centre + vec3<f32>(0.0, in.look.w, 0.0), vec3<f32>(0.0, 1.0, 0.0), dist, 0.0);
    let l = globals.to_sun.xyz;
    let wrap = clamp((dot(sn, l) + 0.3) / 1.3, 0.0, 1.0);
    // Light coming through the thin rim when the sun is behind it.
    let behind = pow(max(dot(-in.to_eye, l), 0.0), 3.0) * (1.6 - body * 0.8);
    let sky = mix(0.9, 1.3, sn.y * 0.5 + 0.5);
    let lit = globals.ambient.rgb * sky + globals.sun.rgb * sun * (wrap + behind);
    // Its thick clumps shade themselves a little.
    let shade = mix(1.0, 0.72, body * smoothstep(0.4, 0.8, n));
    let c = in.colour * shade * pow(soft_clamp(lit), vec3<f32>(2.2));
    let hazed = apply_haze(c, in.centre, dist);
    // Thin near the eye, so a puff never fills the screen or hides the
    // rider from the chase camera.
    let near = smoothstep(4.0, 16.0, dist);
    let a = alpha * near;
    return vec4<f32>(hazed * a, a);
}
