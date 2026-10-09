// Vertex + fragment shader for textured blocks.
//
// Two jobs, one vertex format.
//
// The **opaque** entry point is the geometry pass of the deferred renderer: it writes
// what each surface *is* — its colour, the way it faces, and how bright its corners were
// baked — and leaves the lighting to `lighting.wgsl`, which does it once per pixel. It still
// throws away transparent texels, so leaves and flowers stay cutout and keep writing
// depth.
//
// The **blended** entry point is not deferred at all. Translucency has to blend in order,
// after the lit scene, so water shades itself here.
//
// `shaders/sun.wgsl` is pasted onto the front of this file (`Renderer::compose`), so the
// curve, the face shade and the tint maths are the ones every other pass uses.

struct Globals {
    // Column-major, as WGSL and wgpu expect.
    view_proj: mat4x4<f32>,
    // Only the deferred lighting pass reconstructs world positions; the two share one
    // buffer, so this is declared whether or not it is read.
    inv_view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var block_textures: texture_2d_array<f32>;
@group(0) @binding(2) var block_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) layer: u32,
    // Packed shading, built by the mesher (`mesh::pack_light`): this corner's *sky* brightness
    // in bits 0-7, its *block* brightness in bits 8-15, and the face index in bits 16-18.
    @location(3) light: u32,
    // Colour multiplied over the sample, as 0xRRGGBB (`Block::tint`).
    @location(4) tint: u32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // The corner's sky brightness, *interpolated* across the face — which is the whole of smooth
    // lighting: two corners under a tree and two out in the open shade off one into the other
    // instead of meeting at a step.
    @location(1) sky_brightness: f32,
    // And the same for the light blocks *emit*: lava now, torches later. It travels in its own
    // varying because the two are not treated alike downstream — the sun's shadow map may take
    // the daylight away, but nothing may take this away.
    @location(2) block_brightness: f32,
    // Integer varyings must be flat (not interpolated).
    @location(3) @interpolate(flat) layer: u32,
    @location(4) @interpolate(flat) face: u32,
    @location(5) @interpolate(flat) tint: u32,
};

@vertex
fn vs_main(vertex: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = globals.view_proj * vec4<f32>(vertex.position, 1.0);
    out.uv = vertex.uv;
    out.sky_brightness = f32(vertex.light & 255u) / 255.0;
    out.block_brightness = f32((vertex.light >> 8u) & 255u) / 255.0;
    out.layer = vertex.layer;
    out.face = (vertex.light >> 16u) & 7u;
    out.tint = vertex.tint;
    return out;
}

// What the geometry pass leaves behind for `lighting.wgsl` to light. One texel per pixel
// of the screen, so no lighting is decided here at all — only what the surface is.
struct Surface {
    // The block's colour with no light on it, plus the brightness its corners were baked with
    // in the alpha (see the mesher's smooth lighting). Brightness is 0-1 in *linear* space —
    // alpha in an sRGB target is not gamma-encoded — so it survives the round trip through the
    // G-buffer untouched, and a whole byte of it is plenty: it is the byte a vertex carries.
    @location(0) albedo: vec4<f32>,
    // Which way the face points, as `normal * 0.5 + 0.5` so it fits a 0..1 channel. A voxel
    // world only has the six axis directions, which eight bits hold exactly. The alpha was
    // spare, and now carries the *block* brightness — the light no shadow may take away, which
    // is why it needs a channel of its own rather than riding along with the sky's.
    @location(1) normal: vec4<f32>,
}

// The two fragment entry points are the two ways Minecraft draws a see-through block.
// `Renderer::create_pipeline` picks between them: the opaque pass writes the G-buffer, the
// blended pass shades itself.

/// **Cutout** (the opaque pass, which is the geometry pass of the deferred renderer): texels
/// below the cutoff are thrown away, so a cutout block — oak leaves, a flower, the clear
/// middle of a window pane — reads as solid with no blending, and the pass still writes
/// depth. What it *writes* is the surface itself: its colour, its normal, and the brightness
/// its four corners carry, interpolated across the face by the rasterizer. The sun is not
/// consulted here at all.
///
/// The cutoff is deliberately low — Minecraft's is 0.1 — because this pass samples
/// *mipmapped* textures. A shrinking leaf averages its transparent holes into the foliage
/// around it, and a high cutoff would keep nibbling those averaged texels away until a
/// distant canopy dissolved into holes. At 0.1 a far tree reads as solid leaves, which is
/// exactly Minecraft's `cutout_mipped` behaviour.
@fragment
fn fs_cutout(in: VertexOutput) -> Surface {
    let color = textureSample(block_textures, block_sampler, in.uv, i32(in.layer));
    if (color.a < 0.1) {
        discard;
    }

    var surface: Surface;
    // The interpolated brightness goes straight into the alpha. It is already through the light
    // curve — the mesher applies it, because smoothing has to average in brightness rather than
    // in levels (see `world::light`) — so the deferred pass has only a multiply left to do.
    surface.albedo = vec4<f32>(block_color(color, decode_tint(in.tint)), in.sky_brightness);
    // The face index becomes a real world normal here, so the lighting pass has one to shade
    // with — and to step the pixel off its own surface before it asks the shadow map.
    surface.normal = vec4<f32>(face_normal(in.face) * 0.5 + 0.5, in.block_brightness);
    return surface;
}

/// **Translucent** (the blended pass): Minecraft's other transparent render type, used for
/// water and glass. There is deliberately no alpha test here — the texture's own alpha
/// blends, so a faintly tinted pane stays faint instead of vanishing.
///
/// This pass is *not* deferred. It is drawn after the lit scene, so it has to shade itself,
/// and it has the same smoothly-lit corners plus the face shade to do it with — the shadow map
/// is not bound here. That is a limitation, not a rule: `lit(shade, 1.0)` is exactly the old
/// formula, because the ambient and direct weights add up to one (see **Shadows** in
/// AGENTS.md).
@fragment
fn fs_blend(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = textureSample(block_textures, block_sampler, in.uv, i32(in.layer));
    let tint = decode_tint(in.tint);

    // Water is lit smoothly like everything else — its corners come from the mesher too — so
    // the sea dims into a shore and a shadow on the bed above it shades the surface as well.
    let shade = face_shade(in.face);
    // No shadow map is bound in this pass, so the sun's side of the light is simply "the sun is
    // not blocked" — but the block light rides along at full strength, so a lava pool seen
    // through water still glows through it.
    let light = lit_channels(in.sky_brightness * shade, in.block_brightness * shade, 1.0);
    return vec4<f32>(block_color(color, tint) * light, color.a * tint.a);
}
