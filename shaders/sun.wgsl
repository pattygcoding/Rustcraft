// Sunlight, shared by every pass that lights something.
//
// This file has no bindings and no entry points of its own: it is a *prelude*, pasted
// onto the front of each shader that needs it (`Renderer::compose`), so that the per-face
// shade, the sun/ambient mix and the tint maths have exactly one definition. Three shaders
// light something — the deferred pass in `lighting.wgsl`, and the blended water and the HUD
// in `block.wgsl` and `ui.wgsl` — and two copies of a shade table would drift.
//
// There is deliberately no light *curve* in here any more. Brightness arrives already curved:
// smooth lighting averages the light of the cells around a *corner*, and that average has to
// be taken in brightness rather than in levels, or a gradient would step a whole level at a
// time. So the mesher applies the curve (`world::light::brightness`) and bakes the answer
// into the vertices, for the rasterizer to interpolate. One definition of it, in Rust, where
// a test can hold it to Minecraft's shape.

// --- Brightness -----------------------------------------------------------------
//
// What the passes below multiply together is a 0-1 **brightness**: how much of full daylight
// reaches a surface. It comes from the voxel light of the cells around it — the sun that found
// its way there *around* whatever is nearby, at levels 0-15 (see `world::light`) — through
// Minecraft's light curve, applied and averaged per corner by the mesher. Ambient occlusion is
// folded into the same number too, so a corner where two walls meet is dimmer than one out in
// the open before this file is reached at all.

/// How much light each way a face points catches. An old trick, and the one that makes a
/// block's sides read as solid: without it every face of a flat-coloured cube looks the
/// same. Faces are ordered [+X, -X, +Y, -Y, +Z, -Z].
fn face_shade(face: u32) -> f32 {
    if (face == 2u) {
        return 1.0; // top
    }
    if (face == 3u) {
        return 0.5; // bottom
    }
    if (face == 4u || face == 5u) {
        return 0.8; // north / south
    }
    return 0.6; // east / west
}

/// The unit normal of a face, in world space. Faces are ordered [+X, -X, +Y, -Y, +Z, -Z],
/// matching `mesh::CUBE_FACES`.
fn face_normal(face: u32) -> vec3<f32> {
    switch (face) {
        case 0u: { return vec3<f32>(1.0, 0.0, 0.0); }
        case 1u: { return vec3<f32>(-1.0, 0.0, 0.0); }
        case 2u: { return vec3<f32>(0.0, 1.0, 0.0); }
        case 3u: { return vec3<f32>(0.0, -1.0, 0.0); }
        case 4u: { return vec3<f32>(0.0, 0.0, 1.0); }
        default: { return vec3<f32>(0.0, 0.0, -1.0); }
    }
}

/// Which of the six faces a normal belongs to — the inverse of [`face_normal`], for the
/// passes that read a normal out of the G-buffer. A voxel world only ever has these six,
/// so the largest component wins and its sign picks the side.
fn face_of_normal(normal: vec3<f32>) -> u32 {
    let a = abs(normal);
    if (a.x >= a.y && a.x >= a.z) {
        return select(1u, 0u, normal.x > 0.0);
    }
    if (a.y >= a.z) {
        return select(3u, 2u, normal.y > 0.0);
    }
    return select(5u, 4u, normal.z > 0.0);
}

// --- Sun and ambient ----------------------------------------------------------
//
// How the two halves of the light stack up. In the open `shadow` is 1.0 and the total is
// AMBIENT + DIRECT = 1.0, so lit ground comes out exactly as bright as it did before
// there was a sun at all; a shadow takes it down to AMBIENT. That is deliberate: it makes
// the shadow pass a *change* to the picture rather than a rewrite of it, which is far
// easier to judge and to tune.
const AMBIENT: f32 = 0.42;
const DIRECT: f32 = 0.58;

/// The light a surface gets, from how bright it is and whether the sun reaches it.
///
/// `shade` is the baked, interpolated corner brightness out of the G-buffer — or, in the
/// passes that shade themselves, that number times the face shade. `shadow` is 1.0 in the sun
/// and 0.0 in shade; percentage-closer filtering makes it anything between them at an edge, so
/// the two multiply smoothly. The result is raised to 2.2 because the render target and the
/// textures are sRGB, and the GPU multiplies colour in linear space: the exponent lands the
/// shading on the gamma-encoded texture the way Minecraft does, and the floor keeps unlit
/// caves very dark grey rather than pure black.
fn lit(shade: f32, shadow: f32) -> f32 {
    return max(pow(shade * (AMBIENT + DIRECT * shadow), 2.2), 0.004);
}

/// The light a surface gets, from *both* of its light channels.
///
/// `sky` is the baked, interpolated corner brightness of the sunlight that found its way here
/// around whatever is nearby — the half the shadow map may take away. `block` is the same for
/// the light blocks *emit* (lava, and later torches), which is not sunlight at all and so is
/// never shadowed: it is lit as if nothing stood in its way. Both arrive already multiplied by
/// the face shade.
///
/// The two are combined with a `max` rather than a sum, because either one alone can light a
/// surface: out in the open the sky's is 1.0 and the block term is nothing, while in a cave
/// beside a lava pool it is the other way round — and that glow must survive the shadow that
/// lies over everything underground. In a world with no light-emitting blocks the block term is
/// always zero, so this returns exactly what [`lit`] on the sky alone always did.
fn lit_channels(sky: f32, block: f32, shadow: f32) -> f32 {
    return max(lit(sky, shadow), lit(block, 1.0));
}

/// A block's colour, unpacked from the `0xAARRGGBB` word the mesher writes (`Block::tint`).
///
/// Some textures are greyscale and carry their colour here instead: water is tinted blue
/// and oak leaves green, while everything else is opaque white, which leaves the sample
/// alone.
fn decode_tint(tint_bits: u32) -> vec4<f32> {
    return vec4<f32>(
        f32((tint_bits >> 16u) & 255u),
        f32((tint_bits >> 8u) & 255u),
        f32(tint_bits & 255u),
        f32((tint_bits >> 24u) & 255u),
    ) / 255.0;
}

/// A block's colour, ready for the sRGB target it is about to be written to.
///
/// The target and the textures are both sRGB, so the GPU multiplies colour in *linear*
/// space. Raising the tint to 2.2 cancels that back out, which is what lets a tint be
/// authored as the colour you want to see — water as its blue, leaves as their green.
fn block_color(sample: vec4<f32>, tint: vec4<f32>) -> vec3<f32> {
    return sample.rgb * pow(tint.rgb, vec3<f32>(2.2));
}
