// A tyre's line since the last frame, drawn into the ruts' map (ruts.rs):
// a groove deepest along the middle, kept where deeper than what is there.

struct Out {
    @builtin(position) position: vec4<f32>,
    // Across the groove, -1 to 1, and how deep it is.
    @location(0) across: f32,
    @location(1) pass_depth: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @location(0) ends: vec4<f32>, @location(1) shape: vec4<f32>) -> Out {
    let a = ends.xy;
    let b = ends.zw;
    let along = normalize(b - a + vec2<f32>(1e-9, 0.0));
    let side = vec2<f32>(-along.y, along.x) * shape.x * 1.5;
    let corner = array<vec2<f32>, 6>(
        vec2<f32>(0.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    )[vi];
    let p = mix(a, b, corner.x) + side * corner.y;
    var out: Out;
    out.position = vec4<f32>(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.0, 1.0);
    out.across = corner.y * 1.5;
    out.pass_depth = shape.y;
    return out;
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    let d = abs(in.across);
    return vec4<f32>(in.pass_depth * (1.0 - smoothstep(0.2, 1.0, d)), 0.0, 0.0, 1.0);
}
