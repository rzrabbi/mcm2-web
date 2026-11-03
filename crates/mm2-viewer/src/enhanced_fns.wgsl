fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// `n` bent by the bumps a texture's brightness `height` suggests, as it
// changes across the screen (bump mapping without tangents, Mikkelsen
// 2010).
fn bump(n: vec3<f32>, world: vec3<f32>, height: f32, strength: f32) -> vec3<f32> {
    let dp_dx = dpdx(world);
    let dp_dy = dpdy(world);
    let dh_dx = dpdx(height) * strength;
    let dh_dy = dpdy(height) * strength;
    let r1 = cross(dp_dy, n);
    let r2 = cross(n, dp_dx);
    let det = dot(dp_dx, r1);
    let grad = sign(det) * (dh_dx * r1 + dh_dy * r2);
    let bent = abs(det) * n - grad;
    if (dot(bent, bent) < 1e-12) {
        return n;
    }
    return normalize(bent);
}

// The game clamps its light at full; let it run a little past, rolling off.
fn soft_clamp(c: vec3<f32>) -> vec3<f32> {
    let over = max(c - vec3<f32>(0.9), vec3<f32>(0.0));
    return min(c, vec3<f32>(0.9)) + over / (vec3<f32>(1.0) + over * 2.5);
}

// How much of the sun reaches `world` (1 all, 0 none), from the cascade
// its distance from the camera falls in, filtered over 3×3 texels; seen
// from `lift` ft off the surface, so what lies flat on it (painted ruts,
// planks) casts nothing.
fn sun_visible(world: vec3<f32>, n: vec3<f32>, dist: f32, lift: f32) -> f32 {
    if (enh.flags.y < 0.5 || dist > enh.splits.w) {
        return 1.0;
    }
    var c = 0u;
    if (dist > enh.splits.x) {
        c = 1u;
    }
    if (dist > enh.splits.y) {
        c = 2u;
    }
    var seen = cascade_seen(world, n, lift, c);
    // Over the last of a cascade the next one is blended in, so no ring of
    // a change in the shadows' sharpness shows where they meet.
    if (c < 2u) {
        let edge = select(enh.splits.x, enh.splits.y, c == 1u);
        let k = smoothstep(edge * 0.8, edge, dist);
        if (k > 0.0) {
            seen = mix(seen, cascade_seen(world, n, lift, c + 1u), k);
        }
    }
    // Fade out over the last tenth of the last cascade.
    let fade = smoothstep(enh.splits.w * 0.9, enh.splits.w, dist);
    return mix(seen, 1.0, fade);
}

// How much of the sun reaches `world` by cascade `c`, filtered over 3×3
// texels.
fn cascade_seen(world: vec3<f32>, n: vec3<f32>, lift: f32, c: u32) -> f32 {
    let texel = enh.texel[c];
    // Off the surface by a texel or two, so it does not shadow itself.
    let lifted = world + n * (texel * 1.5 + lift) + enh.to_sun.xyz * texel;
    let p = enh.cascades[c] * vec4<f32>(lifted, 1.0);
    let uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || p.z > 1.0) {
        return 1.0;
    }
    let step = 1.0 / 2048.0;
    var sum = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            sum += textureSampleCompareLevel(sun_map, sun_sampler, uv + vec2<f32>(f32(x), f32(y)) * step, c, p.z);
        }
    }
    return sum / 9.0;
}

// The enhanced fog: a haze that thickens with distance and never quite
// hides the land, in place of the game's fog wall.
fn haze_amount(dist: f32) -> f32 {
    // Eased in from where it starts, so that the land does not show a ring
    // there: no haze, and a rise that gathers, as the thickness x²/(x + w)
    // does, and then goes on as it would.
    let x = max(dist - enh.haze.x, 0.0);
    let eased = x * x / (x + 0.3 / enh.haze.y);
    return 1.0 - exp(-eased * enh.haze.y);
}

fn apply_haze(c: vec3<f32>, world: vec3<f32>, dist: f32) -> vec3<f32> {
    if (enh.flags.z > 0.5) {
        return mix(c, enh.fog.rgb, haze_amount(dist));
    }
    let range = max(globals.fog_range.y - globals.fog_range.x, 1.0);
    let fog = clamp((dist - globals.fog_range.x) / range, 0.0, 1.0);
    return mix(c, globals.fog.rgb, fog);
}
