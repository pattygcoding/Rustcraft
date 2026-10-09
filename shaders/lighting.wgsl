// Deferred lighting: one fullscreen pass that puts the light on every pixel the geometry
// pass left behind.
//
// The geometry pass writes what a surface *is* — its colour, the way it faces and how far
// away it is — and nothing about the light. This pass reads those back, reconstructs the
// world position from the depth, and lights it. The expensive part, working out whether
// the sun actually reaches this point, therefore runs once per *pixel on screen* rather
// than once per fragment drawn, however many surfaces overlap that pixel.
//
// `shaders/sun.wgsl` is pasted onto the front of this file (`Renderer::compose`), so the
// light curve and the face shade are the same ones the water and the HUD use.

// Matches `Globals` in `gfx::renderer`: one buffer, shared with the passes that project
// geometry, so both fields are declared even though only one is read here.
struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
};

// Matches `shadow::SunUniform`.
struct Sun {
    view_proj: mat4x4<f32>,
    direction: vec4<f32>,
    params: vec4<f32>,
};

// The G-buffer, one texel per pixel of the screen. The albedo's `rgb` is the block's colour
// with no light on it; its `a` is how bright the surface is — the brightness its corners were
// baked with, interpolated across the face, and already through the light curve, which is why
// this pass does not apply one.
@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var albedo_buffer: texture_2d<f32>;
@group(0) @binding(2) var normal_buffer: texture_2d<f32>;
@group(0) @binding(3) var scene_depth: texture_depth_2d;

// The sun, and the depth buffer it sees the world through.
@group(1) @binding(0) var<uniform> sun: Sun;
@group(1) @binding(1) var shadow_map: texture_depth_2d;
// A *comparison* sampler: the depth test happens inside the sampler, so one tap already
// answers "is this point lit?".
@group(1) @binding(2) var shadow_sampler: sampler_comparison;

/// The sky, which is also what this pass clears to.
const SKY: vec3<f32> = vec3<f32>(0.55, 0.72, 0.90);

/// A triangle that covers the screen, built from the vertex index alone — no vertex
/// buffer, no attributes: (-1,-1), (3,-1), (-1,3). One triangle rather than a quad, so the
/// seam down the middle is not shaded twice.
@vertex
fn vs_fullscreen(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32((index << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(index & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

/// Where a pixel is in the world, given what the depth buffer holds there.
///
/// Undoing the projection is the only way a deferred pass can know *where* a pixel is: the
/// geometry pass recorded a depth, and the inverse of view·projection turns that plus the
/// pixel's place on the screen back into a world position.
fn world_at(pixel: vec2<f32>, depth: f32) -> vec3<f32> {
    let size = vec2<f32>(textureDimensions(albedo_buffer));
    // Pixel coordinates run down the screen; clip space runs up it.
    let ndc = vec2<f32>(pixel.x / size.x * 2.0 - 1.0, 1.0 - pixel.y / size.y * 2.0);
    let point = globals.inv_view_proj * vec4<f32>(ndc, depth, 1.0);
    return point.xyz / point.w;
}

/// Where a world point lands in the shadow map, as `(u, v, depth)`.
///
/// The GPU twin of `shadow::Sun::shadow_coord`, and the whole of the sun's side of the
/// shadow test: send the point into the map's own space and see what is there.
fn shadow_coord(world: vec3<f32>) -> vec3<f32> {
    let clip = sun.view_proj * vec4<f32>(world, 1.0);
    // Depth needs no rescaling: this projection already outputs wgpu's 0..1, with 1.0
    // meaning "nothing here".
    return vec3<f32>(clip.x * 0.5 + 0.5, 0.5 - clip.y * 0.5, clip.z);
}

/// How much sun reaches `world`: 1.0 in the open, 0.0 in shade, and the gradient between
/// the two along a shadow's edge.
fn sun_visibility(world: vec3<f32>, normal: vec3<f32>) -> f32 {
    // Step off the surface along its own normal before asking. Without that, a surface lit
    // at a grazing angle compares against *itself* — the texel it occupies holds its own
    // depth, and rounding decides the rest, which is the classic shadow acne. A texel is
    // roughly a tenth of a block, so moving less than one cannot visibly detach a shadow
    // from the thing casting it.
    let coord = shadow_coord(world + normal * (sun.params.x * 0.7));
    if (coord.x < 0.0 || coord.x > 1.0 || coord.y < 0.0 || coord.y > 1.0) {
        // Outside the box the map knows nothing, and its clamped edge texel would
        // otherwise cast shadows that are not there. Anything out of range is lit.
        return 1.0;
    }

    // Nine taps in a 3x3 spread. `Linear` on a comparison sampler already averages four
    // stored depths per tap, so this is a soft edge about a dozen texels across for nine
    // comparisons — enough to stop a shadow's edge reading as a staircase.
    var sum = 0.0;
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let tap = coord.xy + vec2<f32>(f32(x), f32(y)) * sun.params.y;
            // `...Level` rather than plain `textureSampleCompare`: this call sits inside a
            // branch, and a sampling call free to choose its own mip level may not.
            sum = sum + textureSampleCompareLevel(shadow_map, shadow_sampler, tap, coord.z);
        }
    }
    return sum / 9.0;
}

/// Light every pixel the geometry pass wrote.
@fragment
fn fs_lighting(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let texel = vec2<i32>(pixel.xy);
    let depth = textureLoad(scene_depth, texel, 0);
    if (depth >= 1.0) {
        // Nothing was drawn here, so the sky shows through. This pass covers every pixel,
        // so the sky has to be written rather than left to the clear.
        return vec4<f32>(SKY, 1.0);
    }

    // Loading rather than sampling: one texel per pixel means a filter would only blur the
    // G-buffer into itself. A load from an sRGB texture still decodes to linear, so the
    // albedo arrives in the space the multiplication below wants.
    let stored = textureLoad(albedo_buffer, texel, 0);
    let properties = textureLoad(normal_buffer, texel, 0);
    let normal = properties.rgb * 2.0 - 1.0;
    let world = world_at(pixel.xy, depth);

    // The light the geometry pass left behind, per channel: the albedo's alpha holds the sky
    // brightness the mesher baked into this face's corners (interpolated across it, and already
    // through the light curve), and the normal's alpha holds the *block* brightness — the light
    // of the blocks that emit it. Sky light is the ambient half: how much sunlight found its way
    // here *around* whatever is nearby, which is what keeps a cave dark, and what the shadow map
    // then decides the direct half of. Block light is nobody's shadow to take away, which is why
    // it is kept apart all the way to here. The face shade rides along as the old per-side
    // darkening that makes a cube read as a cube.
    let shadow = sun_visibility(world, normal);
    let shade = face_shade(face_of_normal(normal));
    let light = lit_channels(stored.a * shade, properties.a * shade, shadow);
    return vec4<f32>(stored.rgb * light, 1.0);
}
