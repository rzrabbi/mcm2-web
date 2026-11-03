struct Globals {
    view_proj: mat4x4<f32>,
    camera: vec4<f32>,
    // The fog's colour (linear), and its start and end in x and y.
    fog: vec4<f32>,
    fog_range: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;

struct Out {
    @builtin(position) position: vec4<f32>,
    // At the sample's centre, not the pixel's: with anti-aliasing an edge
    // pixel is shaded where the square may not reach, and the colour run
    // on past a dark corner goes below 0 there.
    @location(0) @interpolate(perspective, centroid) uv: vec2<f32>,
    @location(1) @interpolate(perspective, centroid) colour: vec4<f32>,
    @location(2) world: vec3<f32>,
};

@vertex
fn vs_main(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>, @location(2) colour: vec4<f32>) -> Out {
    var out: Out;
    out.position = globals.view_proj * vec4<f32>(position, 1.0);
    out.uv = uv;
    out.colour = colour;
    out.world = position;
    return out;
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    let c = textureSample(atlas, atlas_sampler, in.uv);
    let a = c.a * in.colour.a;
    if (a < 0.004) {
        discard;
    }
    // The vertex colour is the game's 0..255 value: modulate in its space.
    // Never below 0, or the power is not a number.
    let rgb = c.rgb * pow(max(in.colour.rgb, vec3<f32>(0.0)), vec3<f32>(2.2));
    let dist = length(in.world - globals.camera.xyz);
    let range = max(globals.fog_range.y - globals.fog_range.x, 1.0);
    let fog = clamp((dist - globals.fog_range.x) / range, 0.0, 1.0);
    return vec4<f32>(mix(rgb, globals.fog.rgb, fog), a);
}
