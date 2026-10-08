// Vertex + fragment shader for textured blocks.
//
// All block textures live in a single `texture_2d_array`; each vertex carries a
// `layer` index selecting which texture to sample. That means one pipeline and
// one bind group cover every block type, and different faces of a block can use
// different textures (e.g. grass top vs. side) with no extra draw calls.

struct Globals {
    // Column-major, as WGSL and wgpu expect.
    mvp: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
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
    // Integer varyings must be flat (not interpolated).
    @location(1) @interpolate(flat) layer: u32,
};

@vertex
fn vs_main(vertex: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = globals.mvp * vec4<f32>(vertex.position, 1.0);
    out.uv = vertex.uv;
    out.layer = vertex.layer;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = textureSample(block_textures, block_sampler, in.uv, i32(in.layer));
    // Alpha cutout: drop fully-transparent texels so cutout blocks (leaves,
    // foliage, ...) render correctly in the opaque pass without blending.
    if (color.a < 0.5) {
        discard;
    }
    return color;
}
