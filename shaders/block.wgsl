// Vertex + fragment shader for textured blocks.
//
// All block textures live in a single `texture_2d_array`; each vertex carries a
// `layer` index selecting which texture to sample. That means one pipeline and
// one bind group cover every block type, and different faces of a block can use
// different textures (e.g. grass top vs. side) with no extra draw calls.
//
// Light rides along in the same vertex: see the lighting section below.

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
    // Packed lighting, built by the mesher (`pack_light`).
    @location(3) light: u32,
    // Colour multiplied over the sample, as 0xRRGGBB (`Block::tint`).
    @location(4) tint: u32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // Integer varyings must be flat (not interpolated).
    @location(1) @interpolate(flat) layer: u32,
    @location(2) @interpolate(flat) light: u32,
    @location(3) @interpolate(flat) tint: u32,
};

@vertex
fn vs_main(vertex: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = globals.mvp * vec4<f32>(vertex.position, 1.0);
    out.uv = vertex.uv;
    out.layer = vertex.layer;
    out.light = vertex.light;
    out.tint = vertex.tint;
    return out;
}

// --- Lighting ---------------------------------------------------------------
//
// Two 0-15 light channels travel packed into one `u32`: sky light in bits 0-3 and
// block light in bits 4-7, with the face index in bits 8-10. It is always daytime,
// so the sun is at full strength and there is no day/night scaling yet.
//
// A cell is lit by whichever channel is brighter. Level 15 is open daylight, and
// the curve below falls away the way Minecraft's lightmap does, so shadow deepens
// quickly with distance from the sky.

fn light_curve(level: u32) -> f32 {
    // Minecraft's lightmap curve: 15 -> 1.0, 8 -> 0.22, 0 -> 0.
    let f = f32(level) / 15.0;
    return f / (4.0 - 3.0 * f);
}

// How much light each way a face points catches. The sky is directly overhead, so
// tops get everything; north/south sides get a little less, east/west a little
// less again, and undersides the least. Faces are ordered [+X, -X, +Y, -Y, +Z, -Z].
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

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = textureSample(block_textures, block_sampler, in.uv, i32(in.layer));
    // Alpha cutout: drop fully-transparent texels so cutout blocks (leaves,
    // foliage, ...) render correctly in the opaque pass without blending.
    if (color.a < 0.5) {
        discard;
    }

    let sky = in.light & 15u;
    let block = (in.light >> 4u) & 15u;
    let face = (in.light >> 8u) & 7u;
    let shade = light_curve(max(sky, block)) * face_shade(face);

    // Some textures are greyscale and carry their colour here instead: water is
    // tinted blue and translucent, while everything else is opaque white, which
    // leaves the sample alone.
    let tint = vec4<f32>(
        f32((in.tint >> 16u) & 255u),
        f32((in.tint >> 8u) & 255u),
        f32(in.tint & 255u),
        f32((in.tint >> 24u) & 255u),
    ) / 255.0;

    // The render target and textures are sRGB, so the GPU multiplies colour in
    // linear space. Raising the colour factors to 2.2 cancels that out, landing
    // them on the gamma-encoded texture exactly as Minecraft's block colour and
    // light shading do. Alpha is not gamma-encoded, so it is used as-is. The floor
    // keeps unlit caves very dark grey instead of pure black.
    let light_factor = max(pow(shade, 2.2), 0.004);
    return vec4<f32>(
        color.rgb * pow(tint.rgb, vec3<f32>(2.2)) * light_factor,
        color.a * tint.a,
    );
}
