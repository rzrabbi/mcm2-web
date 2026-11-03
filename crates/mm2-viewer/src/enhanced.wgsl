// The enhanced drawing's settings (enhance.rs EnhGlobals).
struct Enh {
    cascades: array<mat4x4<f32>, 3>,
    // Each cascade's reach, ft, and in w where the shadows end.
    splits: vec4<f32>,
    // x lighting, y shadows, z haze, w sky: 1 on.
    flags: vec4<f32>,
    // The haze's start, thickness per ft, and the far plane; w how
    // strongly the new ground's detail shows (0 to 1).
    haze: vec4<f32>,
    sun: vec4<f32>,
    to_sun: vec4<f32>,
    fog: vec4<f32>,
    // Each cascade's texel, ft.
    texel: vec4<f32>,
    // The clock (s), wetness, rain, cloud cover.
    world: vec4<f32>,
    // The ruts' map: its corner (x, z), the ft it spans, on.
    ruts: vec4<f32>,
    // x the new ground, y grass, z the real sky, w the bikes' paint.
    more: vec4<f32>,
    // The photo materials' average colours (materials.rs).
    material_average: array<vec4<f32>, 9>,
    // x the screen's width over its height, y the bikes' mud, z cloud
    // drift, w exposure.
    sky: vec4<f32>,
    zenith: vec4<f32>,
    horizon: vec4<f32>,
    // The smooth ground's patch: its corner (terrain units), cells a side,
    // on.
    fine_ground: vec4<f32>,
    // x the trial track's objects in photo materials.
    surfaces: vec4<f32>,
};
