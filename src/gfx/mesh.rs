//! GPU meshes and the vertex format they use.
//!
//! A [`Vertex`] carries a position, a texture coordinate and a texture-array
//! **layer** index, so one mesh can reference many block textures while still
//! being a single draw call. [`MeshData`] is the CPU-side result that chunk
//! meshing produces (see [`crate::world::mesh_chunk`]); [`Mesh`] uploads it.

use wgpu::util::DeviceExt;

/// A single vertex: a position, a texture coordinate, a texture-array layer and
/// the packed lighting for the face it belongs to.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    /// Position in mesh space.
    pub position: [f32; 3],
    /// Texture coordinate within the selected layer.
    pub uv: [f32; 2],
    /// Which texture-array layer to sample.
    pub layer: u32,
    /// Packed light levels and face index; see [`pack_light`].
    pub light: u32,
    /// Colour multiplied over the sampled texture, as `0xRRGGBB`.
    ///
    /// Lets a greyscale texture carry its colour in code — water is tinted blue —
    /// without duplicating the image. See `Block::tint`.
    pub tint: u32,
}

impl Vertex {
    /// How the GPU reads a [`Vertex`] out of a vertex buffer.
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![
            0 => Float32x3, 1 => Float32x2, 2 => Uint32, 3 => Uint32, 4 => Uint32
        ],
    };
}

/// Opaque white: leaves the sampled texture's colour untouched. Most blocks.
pub const UNTINTED: u32 = 0xFFFF_FFFF;

/// Pack a face's lighting into the single `u32` the shader shades with:
///
/// * bits 0–3 — **sky light**, 0–15 (Minecraft's range: 15 is open daylight);
/// * bits 4–7 — **block light**, 0–15 (torches and other sources; 0 for now);
/// * bits 8–10 — the **face index** (`[+X, -X, +Y, -Y, +Z, -Z]`), so the shader can
///   catch the light differently on each side of a block.
pub const fn pack_light(sky: u8, block: u8, face: usize) -> u32 {
    (sky as u32 & 0xF) | ((block as u32 & 0xF) << 4) | ((face as u32 & 0x7) << 8)
}

/// Full daylight from above: open sky, no block light, top face. The screen-space
/// HUD is not part of the world, so it is drawn at full brightness.
pub const FULL_LIGHT: u32 = pack_light(15, 0, 2);

/// The texture-array layer for each of a block's six faces.
///
/// Face order is `[+X, -X, +Y, -Y, +Z, -Z]`, matching [`CUBE_FACES`].
#[derive(Clone, Copy, Debug)]
pub struct BlockFaces {
    /// Layer index per face.
    pub layers: [u32; 6],
}

impl BlockFaces {
    /// Explicit per-face layers, in `[+X, -X, +Y, -Y, +Z, -Z]` order.
    pub fn new(layers: [u32; 6]) -> Self {
        Self { layers }
    }

    /// The same texture on every face (stone, dirt, ...).
    pub fn uniform(layer: u32) -> Self {
        Self::new([layer; 6])
    }

    /// A "grass-like" block: `top` on `+Y`, `bottom` on `-Y`, `side` elsewhere.
    pub fn column(top: u32, side: u32, bottom: u32) -> Self {
        Self::new([side, side, top, bottom, side, side])
    }
}

/// Per-face cube geometry: four corners (offsets from the block centre) paired
/// with their texture coordinates, ordered
/// `[top-left, top-right, bottom-right, bottom-left]`. Face order is
/// `[+X, -X, +Y, -Y, +Z, -Z]`.
#[rustfmt::skip]
pub const CUBE_FACES: [[([f32; 3], [f32; 2]); 4]; 6] = [
    // +X
    [([ 0.5,  0.5,  0.5], [0.0, 0.0]), ([ 0.5,  0.5, -0.5], [1.0, 0.0]), ([ 0.5, -0.5, -0.5], [1.0, 1.0]), ([ 0.5, -0.5,  0.5], [0.0, 1.0])],
    // -X
    [([-0.5,  0.5, -0.5], [0.0, 0.0]), ([-0.5,  0.5,  0.5], [1.0, 0.0]), ([-0.5, -0.5,  0.5], [1.0, 1.0]), ([-0.5, -0.5, -0.5], [0.0, 1.0])],
    // +Y (top)
    [([-0.5,  0.5, -0.5], [0.0, 0.0]), ([ 0.5,  0.5, -0.5], [1.0, 0.0]), ([ 0.5,  0.5,  0.5], [1.0, 1.0]), ([-0.5,  0.5,  0.5], [0.0, 1.0])],
    // -Y (bottom)
    [([-0.5, -0.5,  0.5], [0.0, 0.0]), ([ 0.5, -0.5,  0.5], [1.0, 0.0]), ([ 0.5, -0.5, -0.5], [1.0, 1.0]), ([-0.5, -0.5, -0.5], [0.0, 1.0])],
    // +Z (front)
    [([-0.5,  0.5,  0.5], [0.0, 0.0]), ([ 0.5,  0.5,  0.5], [1.0, 0.0]), ([ 0.5, -0.5,  0.5], [1.0, 1.0]), ([-0.5, -0.5,  0.5], [0.0, 1.0])],
    // -Z (back)
    [([ 0.5,  0.5, -0.5], [0.0, 0.0]), ([-0.5,  0.5, -0.5], [1.0, 0.0]), ([-0.5, -0.5, -0.5], [1.0, 1.0]), ([ 0.5, -0.5, -0.5], [0.0, 1.0])],
];

/// Append one face of the unit cube whose minimum corner is `origin` (in mesh
/// space) to the vertex and index buffers.
///
/// `face` indexes [`CUBE_FACES`]; `layer` selects the texture-array layer; `light`
/// is the packed lighting for this face (see [`pack_light`]); `tint` is the colour
/// multiplied over the texture ([`UNTINTED`] to leave it alone).
pub fn push_face(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    origin: [f32; 3],
    face: usize,
    layer: u32,
    light: u32,
    tint: u32,
) {
    let base = vertices.len() as u32;
    for (corner, uv) in CUBE_FACES[face] {
        vertices.push(Vertex {
            position: [
                origin[0] + 0.5 + corner[0],
                origin[1] + 0.5 + corner[1],
                origin[2] + 0.5 + corner[2],
            ],
            uv,
            layer,
            light,
            tint,
        });
    }
    // Two triangles per quad: (0,1,2) and (0,2,3).
    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// One pass's worth of geometry on the CPU: vertices, plus indices into them.
#[derive(Default)]
pub struct GeometryData {
    /// Vertex data.
    pub vertices: Vec<Vertex>,
    /// Index data.
    pub indices: Vec<u32>,
}

impl GeometryData {
    /// Whether there is anything to draw. Empty sets are never uploaded, because a
    /// GPU buffer cannot be zero-sized.
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// A chunk's mesh, already split for the two passes.
///
/// `opaque` is depth-writing and unblended; `translucent` (water) is blended, so it
/// must be drawn separately — after all the opaque geometry, and back to front
/// across chunks. Keeping the two as separate vertex *and* index buffers means each
/// pass binds exactly what it needs with no index offsets to reason about, and a
/// chunk with no water allocates nothing for the blended pass.
#[derive(Default)]
pub struct MeshData {
    /// Depth-writing geometry: terrain.
    pub opaque: GeometryData,
    /// Blended geometry: water.
    pub translucent: GeometryData,
}

/// A geometry set living on the GPU.
struct Geometry {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

impl Geometry {
    /// Upload `data`, or `None` when there is nothing to draw.
    fn upload(device: &wgpu::Device, label: &str, data: &GeometryData) -> Option<Self> {
        if data.is_empty() {
            return None;
        }
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{label}-vertices")),
            contents: bytemuck::cast_slice(&data.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{label}-indices")),
            contents: bytemuck::cast_slice(&data.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Some(Self {
            vertices: vertex_buffer,
            indices: index_buffer,
            index_count: data.indices.len() as u32,
        })
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
}

/// A chunk's geometry on the GPU: one set per pass, each absent when the chunk has
/// nothing to draw for it.
pub struct Mesh {
    opaque: Option<Geometry>,
    translucent: Option<Geometry>,
}

impl Mesh {
    /// Upload `data` to the GPU.
    pub fn upload(device: &wgpu::Device, label: &str, data: &MeshData) -> Self {
        Self {
            opaque: Geometry::upload(device, &format!("{label}-opaque"), &data.opaque),
            translucent: Geometry::upload(
                device,
                &format!("{label}-translucent"),
                &data.translucent,
            ),
        }
    }

    /// Draw the opaque set. A no-op for a chunk that has none.
    pub fn draw_opaque(&self, pass: &mut wgpu::RenderPass<'_>) {
        if let Some(geometry) = &self.opaque {
            geometry.draw(pass);
        }
    }

    /// Draw the translucent set (call *after* [`Mesh::draw_opaque`]). A no-op for a
    /// chunk with no water.
    pub fn draw_translucent(&self, pass: &mut wgpu::RenderPass<'_>) {
        if let Some(geometry) = &self.translucent {
            geometry.draw(pass);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_packing_round_trips() {
        let packed = pack_light(12, 3, 4);
        assert_eq!(packed & 0xF, 12, "sky light");
        assert_eq!((packed >> 4) & 0xF, 3, "block light");
        assert_eq!((packed >> 8) & 0x7, 4, "face");
    }

    #[test]
    fn light_levels_do_not_spill_between_fields() {
        // Both channels are four bits wide, so an over-large value wraps within its
        // own field rather than bleeding into the neighbouring one.
        let packed = pack_light(16, 16, 0);
        assert_eq!(packed & 0xF, 0, "sky stays in its nibble");
        assert_eq!((packed >> 4) & 0xF, 0, "block stays in its nibble");
        assert_eq!((packed >> 8) & 0x7, 0, "the face field is untouched");
    }

    #[test]
    fn full_light_is_daylight_from_above() {
        assert_eq!(FULL_LIGHT & 0xF, 15, "open sky");
        assert_eq!((FULL_LIGHT >> 4) & 0xF, 0, "no block light");
        assert_eq!((FULL_LIGHT >> 8) & 0x7, 2, "the +Y face");
    }
}
