//! Sun shadows: the depth buffer the sun sees the world through.
//!
//! A chunk's mesh *is* the world's outer shell — hidden-face culling removes only the
//! faces between two solid blocks (see [`crate::world::mesher`]) — so every surface the
//! sun can look at is already in the mesh. Casting a shadow therefore costs one more
//! draw of the *same* vertex buffers under a different matrix, and no new geometry at
//! all. That is the whole trick, and it is why shadows fit the existing renderer so well.
//!
//! [`Sun`] is the direction and the world → shadow-space matrix; [`ShadowMap`] is the
//! texture the shadow pass renders into and the lighting pass samples.

use glam::camera::rh::proj::directx;
use glam::camera::rh::view::look_at_mat4;
use glam::{Mat4, Vec3};

/// Shadow-map resolution, texels per side.
///
/// `2 * RADIUS / RESOLUTION` works out at about 11 cm per texel — fine enough that a
/// tree casts a *tree-shaped* shadow rather than a blob, and small enough that a
/// depth-only redraw of the visible world every frame costs little.
pub const RESOLUTION: u32 = 2048;

/// Half the width of the box the sun looks at, in blocks.
///
/// It covers the whole streamed region — [`crate::world::RENDER_RADIUS`] chunks each
/// way, plus one for luck — so everything you can see is inside the map. Terrain
/// further off than this casts no shadow, which at this draw distance you cannot see.
pub const RADIUS: f32 =
    (crate::world::RENDER_RADIUS as f32 + 1.0) * crate::world::CHUNK_SIZE as f32;

/// Format of the shadow map. `Depth32Float` is widely supported and needs no feature.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// How far the light camera stands back from the middle of the box, in blocks.
///
/// The projection's depth range is twice this, which comfortably spans the whole
/// 256-block-tall world at this sun angle.
const DISTANCE: f32 = 400.0;

/// Direction sunlight comes *from*, before it is normalised: high in the sky, a little
/// to the east and a little to the south.
///
/// Tilted on purpose. A sun straight overhead would drop every shadow directly beneath
/// the block that casts it — a tree's shadow would hide under the tree — and would make
/// the light camera's idea of "up" ambiguous.
const TILT: Vec3 = Vec3::new(0.45, 0.85, 0.28);

/// The direction sunlight comes from, as a unit vector.
pub fn direction() -> Vec3 {
    TILT.normalize()
}

/// The sun, and the matrix that maps world space into its shadow map.
///
/// The box only ever *translates* — its orientation is fixed by [`direction`] — so a
/// world position's shadow coordinates depend on where the box is and nothing else.
/// That is what lets the texel grid be held still as the player walks (see [`snap`]),
/// which is what stops shadow edges from crawling.
#[derive(Clone, Copy, Debug)]
pub struct Sun {
    /// Unit vector pointing from the world *toward* the sun.
    pub direction: Vec3,
    /// World → shadow space: `xy` in `[0, 1]` across the map, `z` the depth to compare
    /// against the texel stored there. Matches the shader's `shadow_coord`.
    pub view_proj: Mat4,
    /// The middle of the box, already snapped to the texel grid.
    pub centre: Vec3,
}

impl Sun {
    /// Aim the box at the camera's position.
    pub fn new(eye: Vec3) -> Self {
        let centre = snap(eye);
        Self {
            direction: direction(),
            view_proj: view_projection(centre),
            centre,
        }
    }

    /// Keep the box on `eye`, without letting the texel grid slide.
    ///
    /// Returns whether the matrix actually changed. While the player moves less than a
    /// texel it does not, and a caller may skip the shadow pass entirely.
    pub fn follow(&mut self, eye: Vec3) -> bool {
        let centre = snap(eye);
        if centre == self.centre {
            return false;
        }
        self.centre = centre;
        self.view_proj = view_projection(centre);
        true
    }

    /// Where a world position lands in the shadow map, as `(u, v, depth)`.
    ///
    /// The GPU does the same arithmetic in `shaders/sun.wgsl`, and this is the half of the
    /// shadow test a test can actually run: the shader's `shadow_coord` has to agree with this
    /// one, and the two are written out side by side so they can be compared.
    #[cfg(test)]
    pub fn shadow_coord(&self, world: Vec3) -> Vec3 {
        let clip = self.view_proj * world.extend(1.0);
        // `x` and `y` come out of the projection in NDC (-1..1) and have to become map
        // coordinates (0..1, with v = 0 at the *top*, as a texture is stored). `z` does
        // **not**: the DirectX/wgpu convention this projection uses already puts depth in
        // 0..1, with 1.0 at the far plane, which is exactly the value the map stores.
        Vec3::new(clip.x * 0.5 + 0.5, 0.5 - clip.y * 0.5, clip.z)
    }

    /// The values the shaders need: the matrix, the direction, and how big a texel is.
    pub fn uniform(&self) -> SunUniform {
        SunUniform {
            view_proj: self.view_proj.to_cols_array_2d(),
            direction: [self.direction.x, self.direction.y, self.direction.z, 0.0],
            params: [
                2.0 * RADIUS / RESOLUTION as f32,
                1.0 / RESOLUTION as f32,
                0.0,
                0.0,
            ],
        }
    }
}

/// Whether a shadow coordinate lands inside the map.
///
/// Outside it the map knows nothing, and the caller has to read that as **lit**: a
/// clamped edge texel would otherwise cast a shadow that is not there. Depth is left out
/// on purpose — a fragment behind the light's near plane is nearer than anything the map
/// holds, so it compares as lit by itself. (The shader inlines the same range test; this is
/// the version a test can run.)
#[cfg(test)]
pub fn covers(coord: Vec3) -> bool {
    coord.x >= 0.0 && coord.x <= 1.0 && coord.y >= 0.0 && coord.y <= 1.0
}

/// The sun as the shaders see it. Laid out to match `struct Sun` in `shaders/sun.wgsl`:
/// a `mat4x4<f32>`, then two `vec4<f32>`s.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SunUniform {
    /// World → shadow space, column-major.
    pub view_proj: [[f32; 4]; 4],
    /// The direction toward the sun: `xyz`, with `w` unused padding.
    pub direction: [f32; 4],
    /// `x` is how many blocks one texel covers, `y` how much of the map one texel is.
    pub params: [f32; 4],
}

/// The sun's depth buffer, and the sampler that reads it.
pub struct ShadowMap {
    /// Written by the shadow pass, sampled by the lighting pass.
    pub view: wgpu::TextureView,
    /// A *comparison* sampler, so one `textureSampleCompareLevel` tap answers "is this
    /// fragment in shadow?" with 0 or 1 — and with `Linear` filtering four stored depths
    /// are averaged per tap, which is percentage-closer filtering for free.
    pub sampler: wgpu::Sampler,
}

impl ShadowMap {
    /// Create the map and its sampler.
    ///
    /// It is sized in *blocks*, not pixels, so unlike the G-buffer it does **not** need
    /// rebuilding when the window is resized.
    pub fn new(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow-map"),
            size: wgpu::Extent3d {
                width: RESOLUTION,
                height: RESOLUTION,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            // Rendered into by the shadow pass, read by the lighting pass.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            // "Is the depth stored here at least the fragment's?" — the classic test, and
            // the reason a *comparison* sampler is the one to bind.
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        Self { view, sampler }
    }
}

/// Build the world → shadow-space matrix for a box centred on `centre`.
fn view_projection(centre: Vec3) -> Mat4 {
    let eye = centre + direction() * DISTANCE;
    let view = look_at_mat4(eye, centre, up_hint());
    // Near is a hair above zero and far is twice the stand-off, so the box's middle sits
    // at a depth near 0.5 with the whole world to either side of it.
    let projection = directx::orthographic(-RADIUS, RADIUS, -RADIUS, RADIUS, 0.1, 2.0 * DISTANCE);
    projection * view
}

/// The "up" hint [`look_at_mat4`] needs.
///
/// A sun straight overhead would make `Y` degenerate — the view direction and the hint
/// would be parallel — so it falls back to `Z` and a silly [`TILT`] cannot produce NaNs.
fn up_hint() -> Vec3 {
    let forward = -direction();
    if forward.cross(Vec3::Y).length_squared() < 1e-6 {
        Vec3::Z
    } else {
        Vec3::Y
    }
}

/// The light camera's two sideways axes, in world space.
///
/// Derived here — the canonical look-at basis is `right = forward × up`,
/// `up = right × forward` — rather than read back out of the matrix, so that [`snap`] and
/// [`view_projection`] cannot disagree about which way the box is turned.
fn light_axes() -> (Vec3, Vec3) {
    let forward = -direction();
    let right = forward.cross(up_hint()).normalize();
    (right, right.cross(forward))
}

/// Move `eye` onto the box's texel lattice.
///
/// As the player walks, the light view slides with them, so the map's texels land on
/// different bits of the world every frame and every shadow edge crawls. Quantising the
/// box's position along the light's own three axes fixes it: the box lands on the same
/// texels across a whole texel of movement, so nothing moves until the player has
/// actually crossed one.
///
/// The answer is rebuilt from the lattice *indices* rather than nudged from `eye`, so one
/// cell always gives one answer, bit for bit — otherwise two routes into the same cell
/// would differ in the last bits and the shadow pass would never spot that nothing moved.
/// (The lattice is the same size on all three axes; along the light itself, where it has
/// no effect on the projection at all, it merely tracks the player.)
fn snap(eye: Vec3) -> Vec3 {
    let texel = 2.0 * RADIUS / RESOLUTION as f32;
    let basis = {
        let (right, up) = light_axes();
        [right, up, -direction()]
    };
    basis
        .into_iter()
        .map(|axis| axis * ((axis.dot(eye) / texel).round() * texel))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The light camera looks *at* the middle of the box, so it must land in the middle
    /// of the map — and about half way down the depth range, with room either side.
    #[test]
    fn the_box_middle_is_the_middle_of_the_map() {
        let sun = Sun::new(Vec3::new(120.0, 70.0, -40.0));
        let coord = sun.shadow_coord(sun.centre);
        assert!((coord.x - 0.5).abs() < 2e-3, "{coord:?}");
        assert!((coord.y - 0.5).abs() < 2e-3, "{coord:?}");
        assert!(
            (coord.z - 0.5).abs() < 1e-2,
            "the box middle sits mid-depth: {coord:?}"
        );
    }

    /// Sunlight comes from above — and a point pushed toward the sun is *nearer* it, which
    /// is the sign the shadow test depends on.
    #[test]
    fn sunlight_comes_from_above() {
        let sun = Sun::new(Vec3::ZERO);
        assert!(sun.direction.y > 0.5, "a high sun: {:?}", sun.direction);
        assert!((sun.direction.length() - 1.0).abs() < 1e-6, "a unit vector");
        assert!(sun.direction.y < 0.999, "tilted, not straight overhead");

        let open = Vec3::new(8.0, 64.0, 8.0);
        let nearer = open + sun.direction * 32.0;
        assert!(
            sun.shadow_coord(nearer).z < sun.shadow_coord(open).z,
            "sunward is less depth"
        );
    }

    /// The whole point of snapping: the box holds still across small movements, so
    /// shadows do not crawl — and snapping an already-snapped box changes nothing again.
    #[test]
    fn the_texel_grid_stands_still() {
        let texel = 2.0 * RADIUS / RESOLUTION as f32;
        let (right, up) = light_axes();
        let mut sun = Sun::new(Vec3::new(3.0, 64.0, 5.0));

        // Snapped, the box sits exactly on the lattice: its position along each of the
        // light's sideways axes is a whole number of texels.
        let latticed = |v: f32| (v / texel - (v / texel).round()).abs() < 1e-3;
        assert!(latticed(right.dot(sun.centre)), "on the lattice");
        assert!(latticed(up.dot(sun.centre)), "on the lattice");

        // From *here* — the lattice is the only place these offsets are predictable from —
        // anything inside half a texel stays put, and a texel and a half does not.
        let start = sun.centre;
        assert!(
            !sun.follow(start + right * texel * 0.3),
            "a third of a texel is not a move"
        );
        assert!(
            !sun.follow(start + up * texel * 0.4 + right * texel * 0.2),
            "and neither is 0.4 of one the other way"
        );
        assert!(
            sun.follow(start + right * texel * 1.5),
            "a texel and a half is"
        );
        assert_eq!(
            sun.centre,
            snap(start + right * texel * 1.5),
            "by one whole texel"
        );

        let centre = sun.centre;
        assert_eq!(snap(centre), centre, "snap is idempotent");
        assert_eq!(
            Sun::new(centre).view_proj,
            sun.view_proj,
            "the same centre rebuilds the same matrix"
        );
    }

    /// Snapping along the light's axes must not let the box drift off the walker: it is
    /// re-derived from the position each frame, never accumulated.
    #[test]
    fn walking_a_long_way_does_not_drift() {
        let texel = 2.0 * RADIUS / RESOLUTION as f32;
        let (right, _) = light_axes();
        let mut sun = Sun::new(Vec3::ZERO);
        for step in 1..=2_000 {
            sun.follow(right * (step as f32 * 0.25));
        }
        let walked = 2_000.0 * 0.25;
        assert!(
            (right.dot(sun.centre) - walked).abs() <= texel,
            "the box trails the walker by at most one texel"
        );
    }

    /// The map must cover everything the render distance can put on screen — and say so,
    /// because outside it the lighting pass reads "lit" rather than "shadowed".
    #[test]
    fn the_map_covers_the_streamed_world() {
        let sun = Sun::new(Vec3::ZERO);
        assert!(covers(sun.shadow_coord(Vec3::new(0.0, 64.0, 0.0))));

        let (right, up) = light_axes();
        let inside = sun.centre + right * (RADIUS * 0.9) + up * (RADIUS * 0.9);
        assert!(covers(sun.shadow_coord(inside)), "in the corner of the box");

        let outside = sun.centre + right * (RADIUS * 3.0);
        assert!(!covers(sun.shadow_coord(outside)), "far outside the box");
    }

    /// The box has to be wide enough for the world we stream, and a texel small enough
    /// that a tree's shadow still reads as a *tree*.
    #[test]
    fn the_box_spans_the_render_distance() {
        let streamed = (crate::world::RENDER_RADIUS as f32 + 1.0) * crate::world::CHUNK_SIZE as f32;
        assert!(RADIUS >= streamed, "{RADIUS} blocks must cover {streamed}");
        let texel = 2.0 * RADIUS / RESOLUTION as f32;
        assert!(texel < 0.25, "a texel is {texel} blocks wide");
    }

    /// The shadow test on the CPU: is anything between this cell and the sun?
    ///
    /// This is what the shadow map does in hardware — march from the cell toward the sun,
    /// stopping at whatever is in the way — written out in Rust so a test can pin the *meaning*
    /// of the map to real geometry: which way shadows fall, and what casts one.
    fn in_shadow(mut solid: impl FnMut(i32, i32, i32) -> bool, cell: Vec3) -> bool {
        // A quarter of a block a step, so a block can never be stepped clean over.
        let step = direction() * 0.25;
        let mut probe = cell + step;
        for _ in 0..(4 * 160) {
            if solid(
                probe.x.floor() as i32,
                probe.y.floor() as i32,
                probe.z.floor() as i32,
            ) {
                return true;
            }
            probe += step;
        }
        false
    }

    /// A roof shades the ground beneath it — and the ground it shades is the ground the sun
    /// leans *away* from. That is the one thing a slip in the sign of the sun's direction would
    /// break, and almost nothing else would notice.
    #[test]
    fn a_roof_shades_the_ground_the_sun_leans_away_from() {
        // A 25x25 slab at y = 80 over open ground at y = 64. It has to be this wide: the sun
        // throws a shadow about half as far sideways as it falls, so at this height the
        // shadow of a *narrow* roof lands entirely clear of the roof itself.
        let roof =
            |x: i32, y: i32, z: i32| y == 80 && (-12..=12).contains(&x) && (-12..=12).contains(&z);
        let ground = Vec3::new(0.5, 64.5, 0.5);
        assert!(in_shadow(roof, ground), "straight under the roof");

        // How far the shadow is thrown: the drop to the ground, stepped along the way the
        // light travels.
        let sun = direction();
        let thrown = Vec3::new(-sun.x, 0.0, -sun.z) / sun.y * (80.0 - ground.y);
        assert!(
            in_shadow(roof, ground + thrown),
            "the shadow is thrown away from the sun, by {thrown:?}"
        );
        assert!(
            !in_shadow(roof, ground - thrown),
            "and nothing is thrown the other way"
        );

        assert!(
            !in_shadow(roof, Vec3::new(0.5, 81.5, 0.5)),
            "nothing is above the roof, so the roof itself is lit"
        );
    }

    /// A real generated oak shades the ground it stands on: the world's trees really are in the
    /// sun's way, canopy and all.
    #[test]
    fn a_generated_oak_shades_the_ground_it_stands_on() {
        let terrain = crate::world::Terrain::new(crate::world::DEFAULT_SEED);
        let size = crate::world::CHUNK_SIZE;
        // `oak_at` rather than `tree_height`: a trunk only goes in where the grass is dry, so
        // the raw dice would happily point at a drowned column with no tree on it at all.
        let (x, z) = (0..64)
            .flat_map(|x| (0..64).map(move |z| (x, z)))
            .find(|&(x, z)| terrain.oak_at(x, z).is_some())
            .expect("the default seed grows an oak near the origin");
        let (ground, _) = terrain.oak_at(x, z).expect("just found one");

        // Generate the chunks the shadow ray passes through, once each.
        let mut chunks = std::collections::HashMap::new();
        let mut solid = |x: i32, y: i32, z: i32| {
            if !(0..256).contains(&y) {
                return false; // above the sky, or below the world
            }
            let pos = (x.div_euclid(size), z.div_euclid(size));
            let chunk = chunks
                .entry(pos)
                .or_insert_with(|| terrain.generate_chunk(pos));
            let block = chunk.get(x.rem_euclid(size), y, z.rem_euclid(size));
            // Only what the shadow pass draws can cast a shadow: water and glass are blended
            // and belong to the other mesh. A leaf is cutout, so it is in the sun's way.
            !block.is_air() && !block.is_blended()
        };

        // The cell inside the foot of the trunk, which the canopy is above.
        let trunk = Vec3::new(x as f32 + 0.5, ground as f32 + 1.5, z as f32 + 0.5);
        assert!(
            in_shadow(&mut solid, trunk),
            "the oak at ({x}, {z}) should shade the ground it stands on"
        );
    }
}
