//! GPU meshes and the vertex format they use.
//!
//! A [`Vertex`] carries a position, a texture coordinate and a texture-array
//! **layer** index, so one mesh can reference many block textures while still
//! being a single draw call. [`MeshData`] is the CPU-side result that chunk
//! meshing produces (see [`crate::world::mesh_chunk`]); [`Mesh`] uploads it.

use wgpu::util::DeviceExt;

/// A single vertex: a position, a texture coordinate, a texture-array layer and
/// the packed shading for the corner it belongs to.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    /// Position in mesh space.
    pub position: [f32; 3],
    /// Texture coordinate within the selected layer.
    pub uv: [f32; 2],
    /// Which texture-array layer to sample.
    pub layer: u32,
    /// Packed shading and face index; see [`pack_light`]. Its low *two* bytes vary from corner
    /// to corner — one brightness per light channel — which is what lets a face shade smoothly,
    /// and what lets lava's glow survive the sun's shadow.
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

    /// The same buffer, read for its positions alone.
    ///
    /// The shadow pass casts a shadow with nothing but where the corners are, so it
    /// declares this: the same stride, but one attribute instead of five, which is 12
    /// bytes a vertex the shadow pass never has to fetch.
    pub const POSITION_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x3],
    };
}

/// Opaque white: leaves the sampled texture's colour untouched. Most blocks.
pub const UNTINTED: u32 = 0xFFFF_FFFF;

/// A layer index meaning "draw no texture at all": the shader paints the tint as a flat
/// colour instead of sampling.
///
/// The inventory's panels and highlights have no texture of their own, and this lets them
/// ride the same pipeline, bind group and vertex format as everything else.
pub const NO_TEXTURE: u32 = u32::MAX;

/// One face of a block icon: which face of the cube it is, and its four `(position, uv)`
/// corners.
pub type IconFace = (usize, [([f32; 2], [f32; 2]); 4]);

/// The three faces of a unit cube an inventory icon shows — its top and two sides — with
/// each corner's position in icon space and its texture coordinate, in the usual
/// `[top-left, top-right, bottom-right, bottom-left]` order.
///
/// `face` indexes [`CUBE_FACES`], so the fragment shader shades every face exactly as it
/// would in the world. That is what makes a slot read as a *cube*; a flat colour would just
/// read as a logo.
pub struct BlockIcon {
    /// Top first, then the two visible sides.
    pub faces: [IconFace; 3],
    /// How tall the icon stands when it is 1.0 wide, so a caller can keep its proportions.
    pub height: f32,
}

impl BlockIcon {
    /// Build the view. Cheap enough to call per icon per frame: it is four points a face and
    /// no matrix at all.
    ///
    /// The view is fixed — yaw the cube 45° to look at a corner, tilt it back 30° to bring
    /// the top into sight, then simply drop the depth. An axonometric projection, in other
    /// words. The silhouette is then scaled to exactly 1.0 wide and centred on the origin.
    pub fn new() -> Self {
        let (yaw_sin, yaw_cos) = std::f32::consts::FRAC_PI_4.sin_cos();
        let (pitch_sin, pitch_cos) = (std::f32::consts::PI / 6.0).sin_cos();
        let project = |corner: [f32; 3]| {
            let [x, y, z] = corner;
            let (x, z) = (x * yaw_cos - z * yaw_sin, x * yaw_sin + z * yaw_cos);
            // Tilting *back* like this raises the far top corner and lowers the near bottom
            // one, which is the classic cube icon.
            [x, y * pitch_cos + z * pitch_sin]
        };

        // [+Y, -X, -Z]: the top, and the two sides this view can see.
        let mut faces: [IconFace; 3] = std::array::from_fn(|slot| {
            let face = [2, 1, 5][slot];
            let corners = CUBE_FACES[face].map(|(corner, uv)| (project(corner), uv));
            (face, corners)
        });

        let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
        for (_, corners) in &faces {
            for (corner, _) in corners {
                for (axis, value) in corner.iter().enumerate() {
                    min[axis] = min[axis].min(*value);
                    max[axis] = max[axis].max(*value);
                }
            }
        }
        let width = max[0] - min[0];
        let centre = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
        for (_, corners) in &mut faces {
            for (corner, _) in corners {
                corner[0] = (corner[0] - centre[0]) / width;
                corner[1] = (corner[1] - centre[1]) / width;
            }
        }

        Self {
            faces,
            height: (max[1] - min[1]) / width,
        }
    }
}

/// Pack the shading of one vertex into the single `u32` the shaders read:
///
/// * bits 0–7 — **sky brightness**: how much daylight falls on this *corner*, 0–255, baked by the
///   mesher ([`crate::world::light::brightness`]). The rasterizer interpolates it across the face,
///   which is the whole of smooth lighting: two corners under a tree and two out in the open make
///   a gradient rather than a step.
/// * bits 8–15 — **block brightness**: the same, for the light coming from blocks that emit it
///   (lava, later torches). It rides along in its own field rather than being folded into the sky
///   brightness, because the two are not treated alike downstream: the sun's shadow map may take
///   the daylight away, but nothing may take the block light away.
/// * bits 16–18 — the **face index** (`[+X, -X, +Y, -Y, +Z, -Z]`), so the shaders can catch the
///   light differently on each side of a block.
///
/// Note the bytes are *brightnesses*, not light levels: the light curve is applied where the
/// averaging happens (in Rust), so the shaders only multiply. See `world::light`.
pub const fn pack_light(sky: u8, block: u8, face: usize) -> u32 {
    (sky as u32) | ((block as u32) << 8) | ((face as u32 & 0x7) << 16)
}

/// Full daylight from above, with no light from blocks: open sky, top face. The screen-space HUD
/// is not part of the world, so it is drawn at full brightness.
pub const FULL_LIGHT: u32 = pack_light(255, 0, 2);

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

/// How far a cross sprite's planes reach from the block's centre on each axis.
///
/// Minecraft's `block/cross` model is about 1.27 blocks across, which puts each corner
/// at 0.45 — just inside the cell, so a flower never pokes into a neighbour.
const CROSS_REACH: f32 = 0.45;

/// A cross sprite: two planes at right angles, the way Minecraft draws flowers.
///
/// Each plane is a quad in the same `[top-left, top-right, bottom-right, bottom-left]`
/// order as [`CUBE_FACES`] — so the same UVs and the same two triangles — but rotated
/// 45° about the block's centre, cutting through the cell on the diagonal.
#[rustfmt::skip]
pub const CROSS_PLANES: [[([f32; 3], [f32; 2]); 4]; 2] = [
    // Along one diagonal, (-x, -z) → (+x, +z).
    [
        ([-CROSS_REACH,  0.5, -CROSS_REACH], [0.0, 0.0]),
        ([ CROSS_REACH,  0.5,  CROSS_REACH], [1.0, 0.0]),
        ([ CROSS_REACH, -0.5,  CROSS_REACH], [1.0, 1.0]),
        ([-CROSS_REACH, -0.5, -CROSS_REACH], [0.0, 1.0]),
    ],
    // ...and across it, (-x, +z) → (+x, -z).
    [
        ([-CROSS_REACH,  0.5,  CROSS_REACH], [0.0, 0.0]),
        ([ CROSS_REACH,  0.5, -CROSS_REACH], [1.0, 0.0]),
        ([ CROSS_REACH, -0.5, -CROSS_REACH], [1.0, 1.0]),
        ([-CROSS_REACH, -0.5,  CROSS_REACH], [0.0, 1.0]),
    ],
];

/// How one face is drawn: everything [`push_face`] needs beyond *where* the face is.
///
/// Gathered into a struct rather than trailing parameters because the list is long
/// enough that clippy objects to passing it positionally.
#[derive(Clone, Copy, Debug)]
pub struct FaceStyle {
    /// Which texture-array layer to sample.
    pub layer: u32,
    /// Packed shading for each of the face's four corners, in the same
    /// `[top-left, top-right, bottom-right, bottom-left]` order as the corners themselves
    /// (see [`CUBE_FACES`]); see [`pack_light`].
    ///
    /// Four values rather than one is what smooth lighting *is*: the mesher bakes a corner
    /// from the cells that meet around it, so a corner in a wall's crease is darker than one
    /// out in the open, and the rasterizer interpolates between them. The HUD passes the same
    /// value four times.
    pub light: [u32; 4],
    /// Colour multiplied over the sampled texture, as `0xAARRGGBB`; [`UNTINTED`]
    /// leaves the texture alone.
    pub tint: u32,
    /// How far above the block's floor the face's top edge reaches, as a fraction of
    /// a block: `1.0` for a block that fills its cell, less for water, whose surface
    /// sits low so a shoreline shows a step down into it (see `Block::surface_height`).
    pub height: f32,
}

/// Append one face of the unit cube whose minimum corner is `origin` (in mesh
/// space) to the vertex and index buffers.
///
/// `face` indexes [`CUBE_FACES`]; `style` says which texture to sample, how the face
/// is lit, what colour it takes and how tall it is.
pub fn push_face(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    origin: [f32; 3],
    face: usize,
    style: FaceStyle,
) {
    push_plane(vertices, indices, origin, CUBE_FACES[face], style);
}

/// Append a quad — four `(corner offset, uv)` pairs in the same
/// `[top-left, top-right, bottom-right, bottom-left]` order as [`CUBE_FACES`] — around
/// the block whose minimum corner is `origin`.
///
/// Corner offsets are measured from the block's *centre*, so a face of a full cube
/// sits at ±0.5 while a cross sprite's corners ride the diagonal (see
/// [`CROSS_PLANES`]). `style.height` moves the quad's upper edge up from the block's
/// floor, which is how water stops short of the cell above it; `style.light` gives each
/// corner its own brightness, which is how a face shades smoothly across itself.
pub fn push_plane(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    origin: [f32; 3],
    corners: [([f32; 3], [f32; 2]); 4],
    style: FaceStyle,
) {
    let base = vertices.len() as u32;
    for ((corner, uv), light) in corners.into_iter().zip(style.light) {
        // Only the quad's upper edge moves: a short face is measured *up* from the
        // block's floor rather than squashed around its middle. Corners that never
        // reach the top of the block — the bottom face's — are left alone.
        let y = if corner[1] > 0.0 {
            origin[1] + style.height
        } else {
            origin[1] + 0.5 + corner[1]
        };
        vertices.push(Vertex {
            position: [origin[0] + 0.5 + corner[0], y, origin[2] + 0.5 + corner[2]],
            uv,
            layer: style.layer,
            light,
            tint: style.tint,
        });
    }
    // Two triangles per quad: (0,1,2) and (0,2,3).
    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// Append a screen-space quad — four `(corner, uv)` pairs around `center`, scaled by `half`
/// — to the vertex and index buffers.
///
/// Positions are already clip space (NDC), so unlike [`push_plane`] there is no block origin
/// to offset and the [`FaceStyle::height`] shortening does not apply: `center` *is* where the
/// quad lands. The style still carries the texture layer, the colour and the light the
/// fragment shader reads — the inventory uses the face index to shade the sides of a block
/// icon, and [`NO_TEXTURE`] to paint a panel. Nothing in the HUD is lit, so its four corners
/// all carry the same value.
pub fn push_screen_plane(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    center: [f32; 2],
    half: [f32; 2],
    corners: [([f32; 2], [f32; 2]); 4],
    style: FaceStyle,
) {
    let base = vertices.len() as u32;
    for ((corner, uv), light) in corners.into_iter().zip(style.light) {
        vertices.push(Vertex {
            position: [
                center[0] + corner[0] * half[0],
                center[1] + corner[1] * half[1],
                0.0,
            ],
            uv,
            layer: style.layer,
            light,
            tint: style.tint,
        });
    }
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

    /// A face style with only the height of interest set — the rest is what the HUD
    /// would use for its icons, so it stays out of the way.
    fn style_of(height: f32) -> FaceStyle {
        FaceStyle {
            layer: 0,
            light: [FULL_LIGHT; 4],
            tint: UNTINTED,
            height,
        }
    }

    #[test]
    fn light_packing_round_trips() {
        let packed = pack_light(200, 30, 4);
        assert_eq!(packed & 255, 200, "sky brightness");
        assert_eq!((packed >> 8) & 255, 30, "block brightness");
        assert_eq!((packed >> 16) & 0x7, 4, "face");
    }

    #[test]
    fn full_brightness_does_not_spill_into_the_next_field() {
        // Both brightness bytes fill themselves exactly, so neither may leak into the face
        // index above them.
        let packed = pack_light(255, 255, 7);
        assert_eq!(packed & 255, 255, "sky brightness");
        assert_eq!((packed >> 8) & 255, 255, "block brightness");
        assert_eq!((packed >> 16) & 0x7, 7, "the face field is untouched");
    }

    #[test]
    fn the_two_light_channels_are_independent() {
        // A face in a cave lit by a lava pool: no daylight at all, and a full block brightness.
        // Folding the channels together would have lost that distinction.
        let lava_lit = pack_light(0, 255, 2);
        assert_eq!(lava_lit & 255, 0, "no daylight down there");
        assert_eq!(
            (lava_lit >> 8) & 255,
            255,
            "but the lava's light is untouched"
        );
        // And the other way round: open, sunlit ground with nothing burning nearby.
        assert_eq!(FULL_LIGHT & 255, 255, "daylight");
        assert_eq!((FULL_LIGHT >> 8) & 255, 0, "no block light");
    }

    #[test]
    fn full_light_is_daylight_from_above() {
        assert_eq!(FULL_LIGHT & 255, 255, "open sky");
        assert_eq!((FULL_LIGHT >> 8) & 255, 0, "from no source at all");
        assert_eq!((FULL_LIGHT >> 16) & 0x7, 2, "the +Y face");
    }

    #[test]
    fn each_corner_carries_its_own_brightness() {
        // Smooth lighting only works if the four corners keep the order they came in:
        // a light landing on the wrong corner would run the gradient backwards.
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut style = style_of(1.0);
        style.light = [
            pack_light(10, 0, 4),
            pack_light(20, 0, 4),
            pack_light(30, 0, 4),
            pack_light(40, 0, 4),
        ];
        push_face(&mut vertices, &mut indices, [0.0, 0.0, 0.0], 4, style);

        let corners: Vec<u32> = vertices.iter().map(|v| v.light & 255).collect();
        assert_eq!(corners, [10, 20, 30, 40], "corner for corner");
    }

    #[test]
    fn a_full_height_face_fills_its_block() {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        push_face(
            &mut vertices,
            &mut indices,
            [0.0, 0.0, 0.0],
            4,
            style_of(1.0),
        );

        assert_eq!(indices.len(), 6, "two triangles per face");
        let heights: Vec<f32> = vertices.iter().map(|v| v.position[1]).collect();
        assert_eq!(heights, [1.0, 1.0, 0.0, 0.0], "floor to ceiling");
    }

    #[test]
    fn a_short_face_lowers_only_its_top_edge() {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        // A side face of water: the floor stays put, only the top edge drops.
        let water = 0.875;
        push_face(
            &mut vertices,
            &mut indices,
            [0.0, 0.0, 0.0],
            4,
            style_of(water),
        );

        let heights: Vec<f32> = vertices.iter().map(|v| v.position[1]).collect();
        assert_eq!(
            heights,
            [water, water, 0.0, 0.0],
            "measured up from the floor"
        );
    }

    #[test]
    fn a_short_top_face_drops_as_a_whole() {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        // Every +Y corner is an upper one, so the whole face moves down.
        let water = 0.875;
        push_face(
            &mut vertices,
            &mut indices,
            [0.0, 7.0, 0.0],
            2,
            style_of(water),
        );

        for vertex in &vertices {
            assert_eq!(vertex.position[1], 7.0 + water, "the surface sits low");
        }
    }

    #[test]
    fn cross_planes_are_two_diagonals_inside_the_cell() {
        assert_eq!(CROSS_PLANES.len(), 2, "a cross is two planes");

        for plane in CROSS_PLANES {
            for (corner, _) in plane {
                // Every corner rides a diagonal: it is equally far out in x and z (the
                // two planes run in opposite directions)...
                assert_eq!(
                    corner[0].abs(),
                    corner[2].abs(),
                    "corner is off the diagonal: {corner:?}"
                );
                // ...and stays inside the cell, so a flower never pokes through the
                // wall of the block next to it.
                assert!(
                    corner[0].abs() < 0.5,
                    "corner pokes out of the cell: {corner:?}"
                );
            }
            // The planes stand a full block tall.
            assert_eq!(plane[0].0[1], 0.5, "top edge");
            assert_eq!(plane[3].0[1], -0.5, "bottom edge");
        }

        // The second plane runs *across* the first, not parallel to it.
        assert_eq!(CROSS_PLANES[0][1].0[0], CROSS_PLANES[0][1].0[2], "one way");
        assert_eq!(
            CROSS_PLANES[1][1].0[0], -CROSS_PLANES[1][1].0[2],
            "the other way"
        );
    }

    #[test]
    fn the_icon_shows_the_top_and_the_two_sides() {
        let icon = BlockIcon::new();
        // +Y is the top; the two sides that face this view are -X and -Z.
        assert_eq!(icon.faces.map(|(face, _)| face), [2, 1, 5]);
    }

    #[test]
    fn the_icon_is_one_unit_wide_and_taller_than_it_is_wide() {
        let icon = BlockIcon::new();
        let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
        for (_, corners) in &icon.faces {
            for (corner, _) in corners {
                for (axis, value) in corner.iter().enumerate() {
                    min[axis] = min[axis].min(*value);
                    max[axis] = max[axis].max(*value);
                }
            }
        }

        assert!(
            (max[0] - min[0] - 1.0).abs() < 1e-5,
            "exactly one unit wide"
        );
        assert!((min[0] + max[0]).abs() < 1e-5, "centred on x");
        let height = max[1] - min[1];
        assert!((height - icon.height).abs() < 1e-5, "height is reported");
        assert!(
            height > 1.0 && height < 1.5,
            "a cube seen from a corner is a little taller than wide: {height}"
        );
    }

    #[test]
    fn every_icon_face_is_a_parallelogram() {
        // The view is affine, so each face's four corners must stay a parallelogram:
        // opposite corners meet at the same centre.
        for (_, corners) in &BlockIcon::new().faces {
            let a = [
                (corners[0].0[0] + corners[2].0[0]) * 0.5,
                (corners[0].0[1] + corners[2].0[1]) * 0.5,
            ];
            let b = [
                (corners[1].0[0] + corners[3].0[0]) * 0.5,
                (corners[1].0[1] + corners[3].0[1]) * 0.5,
            ];
            assert!(
                (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5,
                "not a parallelogram: {corners:?}"
            );
        }
    }

    #[test]
    fn a_cross_sprite_is_two_quads_in_the_middle_of_the_block() {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for plane in CROSS_PLANES {
            push_plane(
                &mut vertices,
                &mut indices,
                [0.0, 0.0, 0.0],
                plane,
                style_of(1.0),
            );
        }

        assert_eq!(vertices.len(), 8, "two quads of four corners");
        assert_eq!(indices.len(), 12, "two triangles each");

        for vertex in &vertices {
            // Stood on the floor at the middle of the block, not in a corner of it.
            assert!(
                (0.0..=1.0).contains(&vertex.position[1]),
                "outside the cell: {vertex:?}"
            );
            assert!(
                (0.0..=1.0).contains(&vertex.position[0])
                    && (0.0..=1.0).contains(&vertex.position[2]),
                "off the block: {vertex:?}"
            );
        }
        let heights: Vec<f32> = vertices.iter().map(|v| v.position[1]).collect();
        assert!(
            heights.contains(&0.0) && heights.contains(&1.0),
            "floor to ceiling: {heights:?}"
        );
    }
}
