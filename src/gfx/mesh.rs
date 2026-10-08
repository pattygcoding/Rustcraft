//! GPU meshes and the vertex format they use.
//!
//! A [`Vertex`] carries a position, a texture coordinate and a texture-array
//! **layer** index, so one mesh can reference many block textures while still
//! being a single draw call. [`MeshData`] is the CPU-side result that chunk
//! meshing produces (see [`crate::world::mesh_chunk`]); [`Mesh`] uploads it.

use wgpu::util::DeviceExt;

/// A single vertex: a position, a texture coordinate and a texture-array layer.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    /// Position in mesh space.
    pub position: [f32; 3],
    /// Texture coordinate within the selected layer.
    pub uv: [f32; 2],
    /// Which texture-array layer to sample.
    pub layer: u32,
}

impl Vertex {
    /// How the GPU reads a [`Vertex`] out of a vertex buffer.
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Uint32],
    };
}

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
/// `face` indexes [`CUBE_FACES`]; `layer` selects the texture-array layer.
pub fn push_face(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    origin: [f32; 3],
    face: usize,
    layer: u32,
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
        });
    }
    // Two triangles per quad: (0,1,2) and (0,2,3).
    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// A CPU-side mesh ready to upload.
///
/// Indices are laid out as opaque first, then transparent, with
/// `opaque_index_count` marking the boundary.
#[derive(Default)]
pub struct MeshData {
    /// Vertex data.
    pub vertices: Vec<Vertex>,
    /// Index data.
    pub indices: Vec<u32>,
    /// Indices `0..opaque_index_count` are opaque; anything after is transparent.
    pub opaque_index_count: u32,
}

/// A GPU-resident indexed mesh.
pub struct Mesh {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    opaque_index_count: u32,
    transparent_index_count: u32,
}

impl Mesh {
    /// Upload `data` to the GPU.
    pub fn upload(device: &wgpu::Device, label: &str, data: &MeshData) -> Self {
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

        Self {
            vertex_buffer,
            index_buffer,
            opaque_index_count: data.opaque_index_count,
            transparent_index_count: data.indices.len() as u32 - data.opaque_index_count,
        }
    }

    /// Draw the opaque run. A no-op if there is none.
    pub fn draw_opaque(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.opaque_index_count == 0 {
            return;
        }
        self.bind(pass);
        pass.draw_indexed(0..self.opaque_index_count, 0, 0..1);
    }

    /// Draw the transparent run (call *after* [`Mesh::draw_opaque`]). A no-op if
    /// there is none.
    pub fn draw_transparent(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.transparent_index_count == 0 {
            return;
        }
        self.bind(pass);
        let start = self.opaque_index_count;
        pass.draw_indexed(start..start + self.transparent_index_count, 0, 0..1);
    }

    fn bind(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
    }
}
