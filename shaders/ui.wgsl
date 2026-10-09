// Screen-space HUD shader (the crosshair, the inventory's nine slots and the creative screen).
//
// Reuses the block texture array and the scene bind group, but the vertex
// positions are already in clip space (NDC), so no transform is applied — the
// per-frame uniform (binding 0) is simply unused here. The HUD carries no lighting
// of its own, so only the directional *face* shade applies: that is what gives an
// inventory block icon its solid look. The block's colour *tint* is applied too, so
// a greyscale texture (water, oak leaves) shows up in a slot the same colour it does
// in the world. A `mesh::NO_TEXTURE` layer means "no texture": the tint is the colour,
// which is how the panel and the highlights are drawn.

@group(0) @binding(1) var block_textures: texture_2d_array<f32>;
@group(0) @binding(2) var block_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) layer: u32,
    // Packed shading: the HUD reads only the face index back out of it — an icon is not lit,
    // only shaded by which way it faces — but the vertex format is shared with the world, so
    // it is here whether or not the caller set it.
    @location(3) light: u32,
    // Colour multiplied over the sample, as 0xAARRGGBB (`Block::tint`).
    @location(4) tint: u32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    @location(2) @interpolate(flat) light: u32,
    @location(3) @interpolate(flat) tint: u32,
};

@vertex
fn vs_main(vertex: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    // Already in clip space.
    out.clip_position = vec4<f32>(vertex.position, 1.0);
    out.uv = vertex.uv;
    out.layer = vertex.layer;
    out.light = vertex.light;
    out.tint = vertex.tint;
    return out;
}

// How much light each way a face points catches — the same table the world shader shades
// with. The HUD has no lighting of its own (its vertices always carry full daylight), so
// only this directional part applies — and it is what makes an inventory block icon read as
// a cube: a bright top, and two sides at different brightnesses. It comes from
// `shaders/sun.wgsl`, which `Renderer::compose` pastes onto the front of this file.

@fragment
fn fs_blend(in: VertexOutput) -> @location(0) vec4<f32> {
    // The block's colour, packed as 0xAARRGGBB exactly as `Block::tint` writes it.
    let tint = decode_tint(in.tint);

    // `mesh::NO_TEXTURE` marks flat UI — a panel or a highlight, with no block texture of
    // its own — and the tint *is* the colour.
    if (in.layer == 0xFFFFFFFFu) {
        return vec4<f32>(block_color(vec4<f32>(1.0), tint), tint.a);
    }

    let color = textureSample(block_textures, block_sampler, in.uv, i32(in.layer));
    // Match the world shader's cutout so transparent texels vanish.
    if (color.a < 0.05) {
        discard;
    }

    // A block icon rides the face index in its packed shading, so its sides shade like a
    // block's would. A sprite — a flower, say — arrives with the top face and full daylight,
    // and so comes out flat and bright, the way an item's flat sprite should. The HUD has no
    // sun and no shadow map, so its `lit` is the unshadowed one.
    let shade = face_shade((in.light >> 16u) & 7u);
    return vec4<f32>(block_color(color, tint) * lit(shade, 1.0), color.a * tint.a);
}
