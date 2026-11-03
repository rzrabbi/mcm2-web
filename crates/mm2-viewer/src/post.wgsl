// The enhanced drawing's last steps (post.rs): ambient occlusion, bloom,
// and tone mapping the HDR scene onto the screen. None of it the game's.

struct Post {
    // From the screen back to the camera's space.
    inv_proj: mat4x4<f32>,
    // Width, height, and one over each.
    size: vec4<f32>,
    // x ambient occlusion, y bloom, z tone mapping: 1 on.
    flags: vec4<f32>,
    // x occlusion radius (ft), y its strength, z bloom strength, w how
    // far (px per ft at 1 ft) the radius spreads on the screen.
    params: vec4<f32>,
    // The sun's place on the screen (0 to 1) and its shafts' strength (0
    // for none), and its colour.
    sun: vec4<f32>,
    sun_colour: vec4<f32>,
    // From this frame's screen to the last frame's, for motion blur.
    reproject: mat4x4<f32>,
    // Replays' camera look: x the distance in focus (ft), y the blur's
    // size out of focus (px), z the motion blur's strength, w 1 on.
    cinematic: vec4<f32>,
    // x 1 when the scene is drawn smaller than the screen and stretched
    // over it.
    scaled: vec4<f32>,
};

@group(0) @binding(0) var<uniform> post: Post;
@group(0) @binding(1) var depth_tex: DEPTH_TEXTURE;
@group(0) @binding(2) var hdr_tex: texture_2d<f32>;
@group(0) @binding(3) var ao_tex: texture_2d<f32>;
@group(0) @binding(4) var src_tex: texture_2d<f32>;
@group(0) @binding(5) var smooth_sampler: sampler;
@group(0) @binding(6) var bloom_tex: texture_2d<f32>;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// One triangle over the whole screen.
@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> Out {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: Out;
    out.position = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(p.x, 1.0 - p.y);
    return out;
}

fn depth_at(p: vec2<i32>) -> f32 {
    let size = vec2<i32>(post.size.xy);
    return textureLoad(depth_tex, clamp(p, vec2<i32>(0), size - 1), 0);
}

// Where pixel `p` shows, in the camera's space (it looks down -z).
fn view_at(p: vec2<i32>) -> vec3<f32> {
    let uv = (vec2<f32>(p) + 0.5) * post.size.zw;
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth_at(p), 1.0);
    let v = post.inv_proj * ndc;
    return v.xyz / v.w;
}

// Ambient occlusion (the Alchemy estimate, McGuire 2011): how much of the
// open sky round each pixel the nearby surfaces hide.
@fragment
fn fs_ao(in: Out) -> @location(0) vec4<f32> {
    // Worked out at half the scene's size, then blurred and stretched.
    let p = vec2<i32>(in.position.xy) * 2;
    if (depth_at(p) >= 1.0) {
        return vec4<f32>(1.0);
    }
    let c = view_at(p);
    // The surface's normal, from the nearer neighbours each way.
    let l = view_at(p - vec2<i32>(1, 0));
    let r = view_at(p + vec2<i32>(1, 0));
    let u = view_at(p - vec2<i32>(0, 1));
    let d = view_at(p + vec2<i32>(0, 1));
    let dx = select(c - l, r - c, abs(r.z - c.z) < abs(c.z - l.z));
    let dy = select(c - u, d - c, abs(d.z - c.z) < abs(c.z - u.z));
    var n = normalize(cross(dx, dy));
    if (dot(n, c) > 0.0) {
        n = -n;
    }
    let radius = post.params.x;
    let reach = clamp(radius * post.params.w / max(-c.z, 0.1), 2.0, 80.0);
    // A different turn each pixel; the blur after evens it out.
    let noise = fract(52.9829189 * fract(dot(in.position.xy, vec2<f32>(0.06711056, 0.00583715))));
    var sum = 0.0;
    let count = 12;
    for (var i = 0; i < count; i++) {
        let t = (f32(i) + 0.5) / f32(count);
        let angle = (f32(i) * 2.39996 + noise * 6.2832);
        let q = p + vec2<i32>(vec2<f32>(cos(angle), sin(angle)) * reach * sqrt(t));
        let v = view_at(q) - c;
        let vv = dot(v, v);
        let near = 1.0 - smoothstep(radius * radius * 0.5, radius * radius, vv);
        sum += max(dot(v, n) - 0.01 * -c.z, 0.0) / (vv + 0.05) * near;
    }
    let ao = max(1.0 - 2.0 * post.params.y * sum / f32(count), 0.0);
    return vec4<f32>(ao, ao, ao, 1.0);
}

// The occlusion blurred over 4×4 pixels, in four filtered taps.
@fragment
fn fs_ao_blur(in: Out) -> @location(0) vec4<f32> {
    let t = post.size.zw;
    var sum = 0.0;
    sum += textureSampleLevel(ao_tex, smooth_sampler, in.uv + vec2<f32>(-1.0, -1.0) * t, 0.0).r;
    sum += textureSampleLevel(ao_tex, smooth_sampler, in.uv + vec2<f32>(1.0, -1.0) * t, 0.0).r;
    sum += textureSampleLevel(ao_tex, smooth_sampler, in.uv + vec2<f32>(-1.0, 1.0) * t, 0.0).r;
    sum += textureSampleLevel(ao_tex, smooth_sampler, in.uv + vec2<f32>(1.0, 1.0) * t, 0.0).r;
    let ao = sum * 0.25;
    return vec4<f32>(ao, ao, ao, 1.0);
}

// `c` with any channel that is not a number, or is infinite, as 0, and
// the rest held to what a half float keeps. One such pixel in the scene
// (a bright glint past a half float's range, say) would otherwise spread
// through every step of the bloom and black out a block of the screen
// round it.
fn finite(c: vec3<f32>) -> vec3<f32> {
    let bad = (bitcast<vec3<u32>>(c) & vec3<u32>(0x7f800000u)) == vec3<u32>(0x7f800000u);
    return select(clamp(c, vec3<f32>(0.0), vec3<f32>(60000.0)), vec3<f32>(0.0), bad);
}

// Bloom's first step: half size, keeping only what is brighter than
// white, softly.
@fragment
fn fs_bloom_first(in: Out) -> @location(0) vec4<f32> {
    let c = finite(textureSampleLevel(hdr_tex, smooth_sampler, in.uv, 0.0).rgb);
    let bright = max(max(c.r, c.g), c.b);
    let knee = 0.35;
    let soft = clamp(bright - 1.0 + knee, 0.0, 2.0 * knee);
    let keep = max(soft * soft / (4.0 * knee + 1e-4), bright - 1.0) / max(bright, 1e-4);
    return vec4<f32>(c * keep, 1.0);
}

// Each next step down: half size again, from four filtered taps.
@fragment
fn fs_bloom_down(in: Out) -> @location(0) vec4<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(src_tex));
    var c = vec3<f32>(0.0);
    c += textureSampleLevel(src_tex, smooth_sampler, in.uv + vec2<f32>(-1.0, -1.0) * t, 0.0).rgb;
    c += textureSampleLevel(src_tex, smooth_sampler, in.uv + vec2<f32>(1.0, -1.0) * t, 0.0).rgb;
    c += textureSampleLevel(src_tex, smooth_sampler, in.uv + vec2<f32>(-1.0, 1.0) * t, 0.0).rgb;
    c += textureSampleLevel(src_tex, smooth_sampler, in.uv + vec2<f32>(1.0, 1.0) * t, 0.0).rgb;
    return vec4<f32>(c * 0.25, 1.0);
}

// Back up: the smaller step spread with a tent and added on.
@fragment
fn fs_bloom_up(in: Out) -> @location(0) vec4<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(src_tex));
    var c = textureSampleLevel(src_tex, smooth_sampler, in.uv, 0.0).rgb * 4.0;
    c += textureSampleLevel(src_tex, smooth_sampler, in.uv + vec2<f32>(-t.x, 0.0), 0.0).rgb * 2.0;
    c += textureSampleLevel(src_tex, smooth_sampler, in.uv + vec2<f32>(t.x, 0.0), 0.0).rgb * 2.0;
    c += textureSampleLevel(src_tex, smooth_sampler, in.uv + vec2<f32>(0.0, -t.y), 0.0).rgb * 2.0;
    c += textureSampleLevel(src_tex, smooth_sampler, in.uv + vec2<f32>(0.0, t.y), 0.0).rgb * 2.0;
    c += textureSampleLevel(src_tex, smooth_sampler, in.uv + vec2<f32>(-t.x, -t.y), 0.0).rgb;
    c += textureSampleLevel(src_tex, smooth_sampler, in.uv + vec2<f32>(t.x, -t.y), 0.0).rgb;
    c += textureSampleLevel(src_tex, smooth_sampler, in.uv + vec2<f32>(-t.x, t.y), 0.0).rgb;
    c += textureSampleLevel(src_tex, smooth_sampler, in.uv + vec2<f32>(t.x, t.y), 0.0).rgb;
    return vec4<f32>(c / 16.0, 1.0);
}

// Khronos PBR Neutral tone mapping: colours up to about 0.8 stay as they
// are, so the game's own colours keep their look, and only what is
// brighter rolls off toward white.
fn neutral(colour: vec3<f32>) -> vec3<f32> {
    let start = 0.8 - 0.04;
    let desaturation = 0.15;
    // (Its toe, which darkens the darkest colours, is left out.)
    var c = colour;
    let peak = max(c.r, max(c.g, c.b));
    if (peak < start) {
        return c;
    }
    let d = 1.0 - start;
    let new_peak = 1.0 - d * d / (peak + d - start);
    c *= new_peak / peak;
    let g = 1.0 - 1.0 / (desaturation * (peak - new_peak) + 1.0);
    return mix(c, vec3<f32>(new_peak), g);
}

// The bolder look: richer colour, more contrast round the middle grey
// (worked in the screen's gamma, as the eye sees it), and the corners a
// little darker.
fn grade(linear: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
    var c = pow(max(linear, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    let grey = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    c = mix(vec3<f32>(grey), c, 1.1);
    // An S-curve: contrast 1.15 about 0.45.
    c = clamp((c - 0.45) * 1.08 + 0.45, vec3<f32>(0.0), vec3<f32>(1.0));
    c = mix(c, c * c * (3.0 - 2.0 * c), 0.15);
    // Warm the lights a touch and cool the shadows.
    c += vec3<f32>(0.015, 0.005, -0.015) * (grey - 0.4);
    let d = uv - 0.5;
    c *= 1.0 - 0.28 * smoothstep(0.25, 0.75, dot(d, d) * 2.0);
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(2.2));
}

// Shafts of sunlight (light scattered in the air, after Mitchell 2007):
// marching toward the sun on the screen, the open sky on the way shines
// along the line, and whatever stands in front of it, hills, trees,
// riders, leaves its shadow in the light.
fn shafts(uv: vec2<f32>, position: vec2<f32>) -> vec3<f32> {
    let steps = 40;
    let reach = 0.85;
    let delta = (post.sun.xy - uv) * reach / f32(steps);
    // A different start each pixel, so the steps don't band.
    let jitter = fract(52.9829189 * fract(dot(position, vec2<f32>(0.06711056, 0.00583715))));
    var q = uv + delta * jitter;
    var decay = 1.0;
    var sum = 0.0;
    for (var i = 0; i < steps; i++) {
        q += delta;
        if (any(q < vec2<f32>(0.0)) || any(q > vec2<f32>(1.0))) {
            break;
        }
        let d = depth_at(vec2<i32>(q * post.size.xy));
        // Only the sky lights the air; brighter near the sun.
        let open = select(0.0, 1.0, d >= 1.0);
        let near = 1.0 - smoothstep(0.0, 0.3, length((q - post.sun.xy) * vec2<f32>(post.size.x * post.size.w, 1.0)));
        sum += open * near * near * decay;
        decay *= 0.965;
    }
    return post.sun_colour.rgb * sum / f32(steps) * post.sun.z;
}

// The replay camera's look: what is nearer or further than the rider in
// focus is soft, as through a lens, and the world smears along the way
// the camera swings (from where each pixel was on the last frame).
fn cinematic(uv: vec2<f32>, p: vec2<i32>) -> vec3<f32> {
    let d = depth_at(p);
    let z = -view_at(p).z;
    let focus = post.cinematic.x;
    // The circle of confusion, px; the sky as far as anything.
    let far = select(z, 10000.0, d >= 1.0);
    let coc = clamp(abs(far - focus) / max(far, 1.0) * post.cinematic.y * clamp(sqrt(focus / 12.0), 0.5, 1.5), 0.0, post.cinematic.y * 1.5);
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, min(d, 0.99999), 1.0);
    let before = post.reproject * ndc;
    let was = vec2<f32>(before.x / before.w * 0.5 + 0.5, 0.5 - before.y / before.w * 0.5);
    var motion = (uv - was) * post.cinematic.z;
    let longest = 0.04;
    if (length(motion) > longest) {
        motion = normalize(motion) * longest;
    }
    let jitter = fract(52.9829189 * fract(dot(vec2<f32>(p), vec2<f32>(0.06711056, 0.00583715))));
    var sum = vec3<f32>(0.0);
    var weight = 0.0;
    let taps = 16;
    for (var i = 0; i < taps; i++) {
        let t = (f32(i) + jitter) / f32(taps);
        let angle = f32(i) * 2.39996;
        let disc = vec2<f32>(cos(angle), sin(angle)) * sqrt(t) * coc * post.size.zw;
        let q = uv + disc + motion * (t - 0.5);
        // Nearer, sharper things do not bleed onto what is behind them.
        let qz = -view_at(vec2<i32>(q * post.size.xy)).z;
        let w = select(1.0, 0.2, qz < z * 0.8 && coc > 1.5);
        sum += textureSampleLevel(hdr_tex, smooth_sampler, q, 0.0).rgb * w;
        weight += w;
    }
    return sum / max(weight, 1e-4);
}

@fragment
fn fs_composite(in: Out) -> @location(0) vec4<f32> {
    var p = vec2<i32>(in.position.xy);
    var c: vec3<f32>;
    if (post.scaled.x > 0.5) {
        p = vec2<i32>(in.uv * post.size.xy);
        c = textureSampleLevel(hdr_tex, smooth_sampler, in.uv, 0.0).rgb;
    } else {
        c = textureLoad(hdr_tex, p, 0).rgb;
    }
    if (post.cinematic.w > 0.5) {
        c = cinematic(in.uv, p);
    }
    c = finite(c);
    if (post.flags.x > 0.5) {
        c *= textureSampleLevel(ao_tex, smooth_sampler, in.uv, 0.0).r;
    }
    if (post.sun.z > 0.0) {
        c += shafts(in.uv, in.position.xy);
    }
    if (post.flags.y > 0.5) {
        c += textureSampleLevel(bloom_tex, smooth_sampler, in.uv, 0.0).rgb * post.params.z;
    }
    if (post.flags.z > 0.5) {
        c = neutral(c);
    }
    if (post.flags.w > 0.5) {
        c = grade(c, in.uv);
    }
    return vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
