// Water: what turns a flat blue sheet into water.
//
// The blended pass draws water itself, after the lit scene (see `block.wgsl`), and a water
// surface here is *flat* — one quad per block, a hair short of the top of its cell — so every
// ripple has to be made up in the shading. That is all this file does: it builds a normal for
// the surface and lets the light catch it.
//
// The normal is the facet's own, tilted by two slopes:
//
// 1. **The water height map.** The water texture *is* the height map — it is greyscale and takes
//    its colour from the block's tint — so a central difference across a texel of it is the
//    slope of the surface, exactly as a bump map works. The ripples drawn on the water are
//    therefore the ripples that catch the light.
// 2. **Fractional Brownian motion.** Octaves of value noise in *world* space, scrolling with
//    time: being noise it has no period, so the surface never repeats from one block to the
//    next, and being scrolled it is never still.
//
// Both fade out with distance, which is not only taste: a 16-texel height map spread over a
// whole sea aliases into shimmer, and what a sea looks like from far off is a sheet of sky
// anyway.
//
// The normal then drives a highlight from the sun and a sheen of sky at grazing angles — the two
// things that say "water" rather than "blue floor". Everything below is a knob: the constants
// are the whole of the look.
//
// `Renderer::compose` pastes `shaders/sun.wgsl` in front of this file and `block.wgsl` behind it,
// so the light curve and the face shade are the ones every other pass uses.

/// How many octaves of noise the ripples have. Each doubles the frequency and halves the
/// amplitude, so a ripple has smaller ripples on it; four is detail inside detail for little
/// cost, and two would read as one fat wobble.
const FBM_OCTAVES: i32 = 4;

/// The ripple pattern's size, in patterns per block, and how fast it drifts, in blocks a second.
/// The two together set whether the sea reads as a lake in a breeze or a storm.
const RIPPLE_SCALE: f32 = 0.35;
const RIPPLE_SPEED: f32 = 0.06;

/// How far the height map's slope tilts the surface, and how far the noise's does. 1.0 is about
/// 45°; these are well under it, so the surface stays recognisably flat and the ripples are
/// shading rather than lumps.
///
/// The first is calibrated to the water texture, whose slopes are *gentle*: measured across the
/// tile, a texel differs from its neighbours by 0.04 of the whole range on average and 0.18 at
/// the steepest, so a strength of 2.0 would tilt the surface by less than five degrees — ripples
/// you could not see. Eight is what makes them read as water. A higher-contrast height map wants
/// this well under two.
const BUMP_STRENGTH: f32 = 8.0;
const NOISE_STRENGTH: f32 = 0.5;

/// How quickly the fine detail dies away with distance, per block. A tenth of this and the
/// nearest water crawls with ripples all the way to the horizon; ten times it and the sea is
/// glass.
const DETAIL_FADE: f32 = 0.04;

/// The sun's highlight: how tight it is (higher is a smaller, sharper glint) and how bright.
const HIGHLIGHT_POWER: f32 = 48.0;
const HIGHLIGHT_STRENGTH: f32 = 0.35;

/// The sheen of sky picked up at grazing angles — the reason a sea seen from a low angle reads as
/// sky rather than as water — how strong it gets, and what colours the two glints are.
const SHEEN_STRENGTH: f32 = 0.30;
const SHEEN_COLOR: vec3<f32> = vec3<f32>(0.55, 0.72, 0.90);
const SUN_COLOR: vec3<f32> = vec3<f32>(1.0, 0.97, 0.90);
/// A hash of a lattice point, in 0..1: the coin the noise is built from.
///
/// It mixes the *bits* of the coordinates rather than their values, because a `sin`-based hash is
/// only as good as the driver's sine — and a ripple pattern that differed between GPUs would be a
/// bug nobody could chase.
fn hash21(p: vec2<f32>) -> f32 {
    var h = bitcast<u32>(p.x) * 0x27220a95u ^ bitcast<u32>(p.y) * 0x85ebca6bu;
    h = (h ^ (h >> 15u)) * 0x2545f491u;
    h = h ^ (h >> 13u);
    return f32(h & 0xffffffu) / f32(0xffffffu);
}

/// Value noise: the lattice corners hashed and blended smoothly between, in -1..1.
fn value_noise(p: vec2<f32>) -> f32 {
    let cell = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash21(cell);
    let b = hash21(cell + vec2<f32>(1.0, 0.0));
    let c = hash21(cell + vec2<f32>(0.0, 1.0));
    let d = hash21(cell + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y) * 2.0 - 1.0;
}

/// Fractional Brownian motion: octaves of [`value_noise`], each at double the frequency and half
/// the amplitude of the one before.
///
/// The offsets between octaves matter more than they look: without them every octave's lattice
/// lines up and the sum shows a grid.
fn fbm(p: vec2<f32>) -> f32 {
    var total = 0.0;
    var point = p;
    var amplitude = 0.5;
    for (var octave = 0; octave < FBM_OCTAVES; octave = octave + 1) {
        total = total + value_noise(point) * amplitude;
        point = point * 2.0 + vec2<f32>(19.19, 7.31);
        amplitude = amplitude * 0.5;
    }
    return total;
}

/// A tangent frame for `facet`: two unit vectors across the surface, and the normal itself, so a
/// pair of slopes can be turned into a tilt whichever way the surface happens to face.
fn tangent_frame(facet: vec3<f32>) -> mat3x3<f32> {
    // Any axis not parallel to the normal will do, and the flattest is the safest.
    let helper = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(facet.y) > 0.9);
    let tangent = normalize(cross(helper, facet));
    return mat3x3<f32>(tangent, cross(facet, tangent), facet);
}

/// The slope of the water's height map at `uv`, as two slopes in the surface's own frame.
///
/// Two taps either side and a central difference: a bump map, in one function. The taps are at
/// mip level 0 on purpose — the finest ripples are the point — and the sampler repeats, so they
/// wrap round the tile instead of flattening at its edge.
fn height_slope(uv: vec2<f32>, layer: i32) -> vec2<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(block_textures, 0));
    let left = textureSampleLevel(block_textures, block_sampler, uv - vec2<f32>(texel.x, 0.0), layer, 0.0).r;
    let right = textureSampleLevel(block_textures, block_sampler, uv + vec2<f32>(texel.x, 0.0), layer, 0.0).r;
    let up = textureSampleLevel(block_textures, block_sampler, uv - vec2<f32>(0.0, texel.y), layer, 0.0).r;
    let down = textureSampleLevel(block_textures, block_sampler, uv + vec2<f32>(0.0, texel.y), layer, 0.0).r;
    return vec2<f32>(right - left, down - up) * 0.5;
}

/// The water's surface normal at a point: the facet's normal tilted by the height map's slope and
/// by the drifting noise, both scaled back by `detail`, which the caller fades with distance.
fn ripple_normal(
    facet: vec3<f32>,
    uv: vec2<f32>,
    layer: i32,
    world: vec3<f32>,
    time: f32,
    detail: f32,
) -> vec3<f32> {
    let frame = tangent_frame(facet);

    // The height map's slope, across the surface...
    let bump = height_slope(uv, layer) * BUMP_STRENGTH;

    // ...and the noise's, sampled in world space so the pattern belongs to the world rather than
    // to each block, and scrolled so the surface moves. A forward difference of three samples is
    // plenty for a slope: the constant error it carries is nothing to a ripple.
    let p = vec2<f32>(dot(world, frame[0]), dot(world, frame[1])) * RIPPLE_SCALE
        + vec2<f32>(time * RIPPLE_SPEED, time * RIPPLE_SPEED * 0.7);
    let step = 0.35;
    let base = fbm(p);
    let noise = vec2<f32>(
        fbm(p + vec2<f32>(step, 0.0)) - base,
        fbm(p + vec2<f32>(0.0, step)) - base,
    ) / step * NOISE_STRENGTH;

    let tilt = (bump + noise) * detail;
    return normalize(facet + frame[0] * tilt.x + frame[1] * tilt.y);
}

/// The colour the water's surface picks up on top of the block's own shading: a highlight where
/// the sun's reflection lines up with the eye, and a sheen of sky at grazing angles.
///
/// The highlight is scaled by `sky_light` and the sheen by `light`, because only the first of
/// those is the *sun*: a pool lit by lava in a cave should not catch a sunbeam, though it can
/// still be glossy.
fn water_glint(
    normal: vec3<f32>,
    world: vec3<f32>,
    eye: vec3<f32>,
    sun: vec3<f32>,
    sky_light: f32,
    light: f32,
) -> vec3<f32> {
    let view = normalize(eye - world);
    let half_vector = normalize(view + sun);
    let highlight = pow(max(dot(normal, half_vector), 0.0), HIGHLIGHT_POWER);
    let sheen = pow(1.0 - max(dot(normal, view), 0.0), 5.0);
    return SUN_COLOR * (highlight * HIGHLIGHT_STRENGTH * sky_light)
        + SHEEN_COLOR * (sheen * SHEEN_STRENGTH * light);
}
