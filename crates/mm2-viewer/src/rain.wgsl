// Rain for the enhanced drawing (rain.rs), none of it the game's: three
// sheets of streaks falling past the eye, nearer ones bigger and faster,
// slanting in the wind, lit by the sky.

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

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> Out {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    var out: Out;
    out.position = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

fn hash(x: f32) -> f32 {
    return fract(sin(x * 91.345) * 47453.5453);
}

@fragment
fn fs_rain(in: Out) -> @location(0) vec4<f32> {
    let rain = enh.world.z;
    let time = enh.world.x;
    // The screen's shape, so streaks are the same however wide it is.
    let aspect = enh.sky.x;
    var cover = 0.0;
    for (var k = 0; k < 3; k++) {
        let layer = f32(k);
        let columns = mix(70.0, 220.0, layer / 2.0);
        let speed = mix(2.6, 1.3, layer / 2.0);
        let length_ = mix(0.09, 0.035, layer / 2.0);
        // Slanting with the wind, and turning with the eye.
        var p = vec2<f32>(in.uv.x * aspect, in.uv.y);
        p.x += p.y * 0.12 + globals.camera.x * 0.002 * (3.0 - layer);
        let col = floor(p.x * columns);
        let x = fract(p.x * columns) - 0.5;
        let r = hash(col + layer * 17.0);
        let y = fract(p.y - time * speed * (0.8 + r * 0.4) + r * 7.0);
        let line = smoothstep(0.08, 0.0, abs(x) * (1.0 + layer)) * smoothstep(0.0, length_ * 0.3, y) * smoothstep(length_, length_ * 0.5, y);
        // Not every column has a drop at once.
        let some = step(hash(col * 1.7 + floor(p.y - time * speed + r * 7.0) + layer), 0.55);
        cover += line * some * mix(0.35, 0.18, layer / 2.0);
    }
    let a = clamp(cover * rain, 0.0, 0.5);
    let c = (enh.horizon.rgb * 0.9 + globals.ambient.rgb * 0.3) * a;
    return vec4<f32>(c, a);
}
