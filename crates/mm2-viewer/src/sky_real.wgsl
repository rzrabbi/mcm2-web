// The enhanced drawing's real sky (sky.rs), none of it the game's: the
// sky's colour from overhead to the horizon by where the sun is, the sun's
// disc and glow, clouds that drift and are lit from the sun, and the
// painted sky's distant hills kept along the horizon.

struct SkyGlobals {
    turn: mat4x4<f32>,
    // From the screen back to a way to look, in drawing coordinates.
    inverse: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> sky_globals: SkyGlobals;
@group(0) @binding(1) var faces: texture_2d_array<f32>;
@group(0) @binding(2) var faces_sampler: sampler;

#include "enhanced.wgsl"

@group(1) @binding(0) var<uniform> enh: Enh;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> Out {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    var out: Out;
    // At the far plane, behind everything.
    out.position = vec4<f32>(p, 1.0, 1.0);
    out.ndc = p;
    return out;
}

fn hash(p: vec2<f32>) -> f32 {
    let q = fract(p * vec2<f32>(0.1031, 0.1030));
    let r = q + dot(q, q.yx + 33.33);
    return fract((r.x + r.y) * r.x);
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2<f32>(1.0, 0.0)), u.x), mix(hash(i + vec2<f32>(0.0, 1.0)), hash(i + vec2<f32>(1.0, 1.0)), u.x), u.y);
}

fn fbm(p: vec2<f32>) -> f32 {
    var sum = 0.0;
    var amp = 0.5;
    var q = p;
    for (var k = 0; k < 6; k++) {
        sum += noise(q) * amp;
        q = vec2<f32>(q.x * 1.6 - q.y * 1.2, q.x * 1.2 + q.y * 1.6) + 7.1;
        amp *= 0.5;
    }
    return sum;
}

// The painted sky in way `d` (drawing coordinates), from the side faces.
fn painted(d: vec3<f32>) -> vec4<f32> {
    let g = vec3<f32>(d.x, d.y, -d.z);
    let a = abs(g);
    var face = 0;
    var uv = vec2<f32>(0.0);
    if (a.z >= a.x) {
        if (g.z > 0.0) {
            face = 0;
            uv = vec2<f32>(g.x, g.y) / a.z;
        } else {
            face = 2;
            uv = vec2<f32>(-g.x, g.y) / a.z;
        }
    } else {
        if (g.x < 0.0) {
            face = 1;
            uv = vec2<f32>(g.z, g.y) / a.x;
        } else {
            face = 3;
            uv = vec2<f32>(-g.z, g.y) / a.x;
        }
    }
    let t = vec2<f32>((uv.x + 1.0) * 0.5, (1.0 - uv.y) * 0.5);
    return textureSampleLevel(faces, faces_sampler, t, face, 0.0);
}

@fragment
fn fs_real(in: Out) -> @location(0) vec4<f32> {
    let far = sky_globals.inverse * vec4<f32>(in.ndc, 1.0, 1.0);
    let d = normalize(far.xyz / far.w);
    let l = enh.to_sun.xyz;
    let s = max(dot(d, l), 0.0);
    let up = max(d.y, 0.0);
    // The sky's colour, bluer overhead, paler and lit by the sun low down.
    var c = mix(enh.horizon.rgb, enh.zenith.rgb, pow(up, 0.45));
    c += enh.sun.rgb * (pow(s, 8.0) * 0.18 + pow(s, 64.0) * 0.3) * (1.0 - enh.world.w * 0.7);
    if (d.y < 0.0) {
        c = enh.horizon.rgb * mix(1.0, 0.75, smoothstep(0.0, -0.3, d.y));
    }
    // The sun's disc, hidden by heavy cloud.
    let disc = smoothstep(0.99985, 0.99995, s) * 40.0 * (1.0 - enh.world.w);
    c += enh.sun.rgb * disc;
    // Clouds on a layer 3000 ft up, drifting with the wind.
    if (d.y > 0.01) {
        let p = d.xz / d.y * 3000.0 + vec2<f32>(enh.sky.z, enh.sky.z * 0.4);
        let cover = enh.world.w;
        let q = p / 5000.0;
        let f = fbm(q);
        // Puffs with firm edges in fair weather, a sheet when overcast.
        let lo = mix(0.62, 0.3, cover);
        let density = smoothstep(lo, lo + mix(0.12, 0.3, cover), f);
        // Lit from the sun's side: thinner toward the sun, brighter.
        let toward = fbm(q + l.xz * 0.08);
        let lit = clamp(0.55 + (f - toward) * 4.0, 0.15, 1.0);
        let shade = mix(vec3<f32>(0.42, 0.45, 0.52), vec3<f32>(1.0), lit) * mix(enh.zenith.rgb * 0.6 + enh.horizon.rgb * 0.4, enh.sun.rgb * 1.2 + vec3<f32>(0.25), 0.7);
        let rim = pow(s, 12.0) * (1.0 - density) * 0.8;
        let cloud = shade * (1.0 - enh.world.z * 0.4) + enh.sun.rgb * rim;
        let fade = smoothstep(0.01, 0.2, d.y);
        c = mix(c, cloud, density * fade);
    }
    // The painted sky's hills and trees along the horizon, where it is
    // not sky: unlike the painted sky above it.
    if (d.y < 0.22 && d.y > -0.05) {
        let here = painted(d);
        let above = painted(normalize(vec3<f32>(d.x, 0.35, d.z)));
        // Land is darker than the sky above it, and stands on the horizon:
        // painted clouds low down are neither.
        let darker = smoothstep(0.1, 0.2, dot(above.rgb - here.rgb, vec3<f32>(0.3, 0.59, 0.11)));
        let foot = painted(normalize(vec3<f32>(d.x, 0.005, d.z)));
        let grounded = smoothstep(0.08, 0.16, dot(above.rgb - foot.rgb, vec3<f32>(0.3, 0.59, 0.11)));
        let dim = 1.0 - smoothstep(0.4, 0.55, dot(here.rgb, vec3<f32>(0.3, 0.59, 0.11)));
        let land = darker * grounded * dim * here.a * foot.a * (1.0 - smoothstep(0.1, 0.2, d.y));
        let hazed = mix(here.rgb, enh.horizon.rgb, 0.35);
        c = mix(c, hazed, land);
    }
    return vec4<f32>(c * enh.sky.w, 1.0);
}
