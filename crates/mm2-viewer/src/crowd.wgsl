// The enhanced drawing's crowd (crowd.rs), none of it the game's: blocky
// little spectators, lit and shadowed as the rest, swaying as they wait and
// jumping with their arms up while a bike goes by.

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

#include "enhanced.wgsl"

@group(1) @binding(0) var<uniform> enh: Enh;
@group(1) @binding(1) var sun_map: texture_depth_2d_array;
@group(1) @binding(2) var sun_sampler: sampler_comparison;

#include "enhanced_fns.wgsl"

struct Bikes {
    at: array<vec4<f32>, 8>,
};

@group(2) @binding(0) var<uniform> bikes: Bikes;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) @interpolate(flat) colour: vec3<f32>,
};

// Turned `a` radians about x.
fn pitch(p: vec3<f32>, a: f32) -> vec3<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec3<f32>(p.x, p.y * c - p.z * s, p.y * s + p.z * c);
}

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) part: vec2<u32>,
    @location(3) place: vec4<f32>,
    @location(4) shirt: vec4<f32>,
    @location(5) legs: vec4<f32>,
) -> Out {
    let seed = shirt.w;
    let t = enh.world.x;
    // How close the nearest bike is: they cheer it from 90 ft.
    var near = 1e9;
    for (var i = 0; i < 8; i++) {
        let b = bikes.at[i];
        if (b.w > 0.5) {
            near = min(near, distance(b.xz, place.xz));
        }
    }
    let cheer = 1.0 - smoothstep(30.0, 90.0, near);
    var p = position * legs.w;
    var n = normal;
    if (part.y != 0u) {
        // An arm: hanging and swinging a little, or up and waving.
        let side = select(1.0, -1.0, part.y == 1u);
        let wave = sin(t * (7.0 + seed * 3.0) + seed * 40.0 + side) * 0.35;
        let idle = sin(t * 1.3 + seed * 20.0 + side) * 0.08;
        let a = mix(idle, -2.75 + wave, cheer * step(0.25, fract(seed * 7.3 + side * 0.31)));
        let shoulder = vec3<f32>(0.0, 4.8, 0.0) * legs.w;
        p = shoulder + pitch(p - shoulder, a);
        n = pitch(n, a);
    }
    // Shifting about while they wait; jumping when a bike is near.
    let hop = abs(sin(t * (6.0 + seed * 2.0) + seed * 30.0)) * 0.6 * cheer * step(0.5, fract(seed * 13.7));
    p.y += hop;
    let sway = sin(t * 0.9 + seed * 50.0) * 0.04;
    let yaw = place.w + sway;
    let c = cos(yaw);
    let s = sin(yaw);
    let turned = vec3<f32>(p.x * c + p.z * s, p.y, -p.x * s + p.z * c);
    let world = place.xyz + turned;
    var out: Out;
    out.clip = globals.view_proj * vec4<f32>(world, 1.0);
    out.world = world;
    out.normal = vec3<f32>(n.x * c + n.z * s, n.y, -n.x * s + n.z * c);
    // Skin tones and hair from the seed.
    let skin = mix(vec3<f32>(0.62, 0.42, 0.3), vec3<f32>(0.2, 0.11, 0.06), fract(seed * 5.1));
    let hair = mix(vec3<f32>(0.03, 0.02, 0.015), vec3<f32>(0.35, 0.22, 0.08), step(0.7, fract(seed * 9.7)));
    switch part.x {
        case 0u: { out.colour = legs.rgb; }
        case 1u: { out.colour = shirt.rgb; }
        case 2u: { out.colour = skin; }
        default: { out.colour = hair; }
    }
    return out;
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let dist = length(in.world - globals.camera.xyz);
    let sun = sun_visible(in.world, n, dist, 0.0);
    let sky = mix(0.95, 1.15, n.y * 0.5 + 0.5);
    let lit = globals.ambient.rgb * sky + globals.sun.rgb * max(dot(n, globals.to_sun.xyz), 0.0) * sun;
    let c = in.colour * pow(soft_clamp(lit), vec3<f32>(2.2));
    return vec4<f32>(apply_haze(c, in.world, dist), 1.0);
}
