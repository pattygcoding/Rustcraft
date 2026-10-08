// Screen-space HUD shader (the hotbar).
//
// Reuses the block texture array and the scene bind group, but the vertex
// positions are already in clip space (NDC), so no transform is applied — the
// per-frame uniform (binding 0) is simply unused here.

@group(0) @binding(1) var block_textures: texture_2d_array<f32>;
@group(0) @binding(2) var block_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) layer: u32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
};

@vertex
fn vs_main(vertex: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    // Already in clip space.
    out.clip_position = vec4<f32>(vertex.position, 1.0);
    out.uv = vertex.uv;
    out.layer = vertex.layer;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = textureSample(block_textures, block_sampler, in.uv, i32(in.layer));
    // Match the world shader's cutout so transparent texels vanish.
    if (color.a < 0.05) {
        discard;
    }
    return color;
}
