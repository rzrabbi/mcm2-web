@group(0) @binding(0) var<uniform> turn: mat4x4<f32>;
@group(0) @binding(1) var faces: texture_2d_array<f32>;
@group(0) @binding(2) var faces_sampler: sampler;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) face: u32,
};

@vertex
fn vs_main(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>, @location(2) face: u32) -> Out {
    var out: Out;
    let p = turn * vec4<f32>(position, 1.0);
    // At the far plane, behind everything.
    out.position = p.xyww;
    out.uv = uv;
    out.face = face;
    return out;
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(faces, faces_sampler, in.uv, in.face).rgb, 1.0);
}

// ---- The enhanced drawing (graphics.rs), none of it the game's. ----

#include "enhanced.wgsl"

@group(1) @binding(0) var<uniform> enh: Enh;

struct EnhancedOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) face: u32,
    @location(2) dir: vec3<f32>,
};

@vertex
fn vs_enhanced(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>, @location(2) face: u32) -> EnhancedOut {
    var out: EnhancedOut;
    let p = turn * vec4<f32>(position, 1.0);
    out.position = p.xyww;
    out.uv = uv;
    out.face = face;
    out.dir = position;
    return out;
}

// The sky with a glow round the sun, bright enough to bloom, and haze
// thickening to the horizon so the land's haze meets it.
@fragment
fn fs_enhanced(in: EnhancedOut) -> @location(0) vec4<f32> {
    var c = textureSample(faces, faces_sampler, in.uv, in.face).rgb;
    if (enh.flags.w > 0.5) {
        let d = normalize(in.dir);
        let s = max(dot(d, enh.to_sun.xyz), 0.0);
        let sun = enh.sun.rgb;
        c += sun * (pow(s, 6.0) * 0.12 + pow(s, 64.0) * 0.35 + pow(s, 900.0) * 2.5);
        if (enh.flags.z > 0.5) {
            let low = 1.0 - smoothstep(0.0, 0.22, d.y);
            c = mix(c, enh.fog.rgb, low * 0.55);
        }
    }
    return vec4<f32>(c, 1.0);
}
