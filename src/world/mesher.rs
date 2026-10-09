//! Chunk meshing with hidden-face culling and smooth lighting.
//!
//! For every solid block and each of its six faces, the face is emitted **only
//! if the neighbouring block is not solid**. Interior faces between adjacent
//! blocks (the vast majority of blocks in a chunk) are therefore never turned
//! into triangles or vertices — the single biggest win over "one quad per block".
//!
//! Neighbour lookups cross chunk borders via [`World::block`], so the faces where
//! two chunks meet are culled as well — only the world's outer shell is drawn.
//!
//! Each face is lit *per corner*, not per face: see [`corner_light`], which is
//! Minecraft's smooth lighting — the cells around a corner averaged in brightness,
//! and ambient occlusion where solid blocks sit beside it. That is what turns a
//! light's edge into a gradient and paints the creases where a wall meets a floor.
//!
//! The fluids are the blocks that are not a full cube: their surface stops short of the
//! top of the cell (see [`Block::surface_height`]), so a shoreline reads as a step down
//! into the water, and a lava pool as a step down into the lava. A fluid buried under
//! another fluid keeps the full height, so columns of sea have no seam in them.

use std::ops::{Add, Mul};

use crate::gfx::mesh::{self, MeshData};
use crate::gfx::texture::BlockTextures;

use super::World;
use super::block::Block;
use super::chunk::{Chunk, DEPTH, HEIGHT, WIDTH};
use super::light;

/// Neighbour offset per face, in `[+X, -X, +Y, -Y, +Z, -Z]` order to match
/// [`crate::gfx::mesh::CUBE_FACES`].
const NEIGHBOURS: [[i32; 3]; 6] = [
    [1, 0, 0],
    [-1, 0, 0],
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, 1],
    [0, 0, -1],
];

/// The face index a cross sprite is shaded under.
///
/// The shader's `face_shade` reads this to catch the light on each side of a block, and
/// a cross is drawn as north/south planes — so all four corners get the north/south
/// shade, the same on both planes and on both of their sides. A flower therefore looks
/// identical from every direction, which is exactly what a cross sprite is for.
const CROSS_FACE: usize = 4;

/// How much a corner is darkened by ambient occlusion, by the corner's 0–3 score from
/// [`corner_light`]: 3 is an open corner, 0 one where two solid blocks meet.
///
/// This one table is the whole of the ambient-occlusion look, so it is the knob to turn if the
/// creases read too harshly or too faintly. Minecraft's own is in the same spirit: gentle where
/// a single block stands beside a corner — the face's other corners are open, and the gradient
/// between them averages most of it back out — and pronounced where two walls meet, because
/// the corners either side of that one are shaded too.
const AO_SHADE: [f32; 4] = [0.55, 0.72, 0.87, 1.0];

/// Build mesh data for `chunk`, whose minimum corner is at world-space `origin`.
///
/// `world` is used to sample neighbouring chunks, so shared faces between chunks
/// are culled too.
pub fn mesh_chunk(
    chunk: &Chunk,
    origin: [i32; 3],
    world: &World,
    textures: &BlockTextures,
) -> MeshData {
    let mut data = MeshData::default();

    for y in 0..chunk.height() {
        for z in 0..DEPTH {
            for x in 0..WIDTH {
                let block = chunk.get(x as i32, y as i32, z as i32);
                if block.is_air() {
                    continue;
                }

                let faces = block.faces(textures);
                let tint = block.tint();
                // Almost every block fills its cell exactly. Water does not: its
                // surface sits a hair low, so a shoreline shows a step down into the
                // water. Water stacked under more water keeps the full height, so a
                // column of it stays seamless and only the exposed surface sits low.
                let above = neighbour_block(chunk, origin, world, x as i32, y as i32 + 1, z as i32);
                let top = top_height(block, above);
                // Blocks that need alpha *blending* (water) go to the blended pass;
                // everything else — cutout blocks like glass and leaves included —
                // belongs to the depth-writing pass, so each is drawn its own way.
                let geometry = if block.is_blended() {
                    &mut data.translucent
                } else {
                    &mut data.opaque
                };

                let (wx, wy, wz) = (
                    origin[0] + x as i32,
                    origin[1] + y as i32,
                    origin[2] + z as i32,
                );
                let block_origin = [wx as f32, wy as f32, wz as f32];

                // A cross sprite is not a cube: it is two planes cutting through its cell
                // on the diagonal. It is drawn from every side and nothing ever hides it
                // (see `Block::hides_face_of`), so it skips the per-face culling below —
                // and it is lit by the light of its *own* cell, since its planes do not
                // look *into* anything.
                if block.is_cross() {
                    // A cross sprite is lit flat, by the light of its *own* cell: its planes look into
                    // nothing, so there are no corners for smooth lighting to average and nothing
                    // for occlusion to shade. That one number goes into both channels, which the
                    // shader reads as "lit as if the sun reached it": in the open that is exactly
                    // the old full-daylight shading, and beside a lava pool the flower glows.
                    let brightness = light::quantise(light::brightness(world.light(wx, wy, wz)));
                    let light = mesh::pack_light(brightness, brightness, CROSS_FACE);
                    let style = mesh::FaceStyle {
                        layer: faces.layers[CROSS_FACE],
                        light: [light; 4],
                        tint,
                        height: top,
                    };
                    for plane in mesh::CROSS_PLANES {
                        mesh::push_plane(
                            &mut geometry.vertices,
                            &mut geometry.indices,
                            block_origin,
                            plane,
                            style,
                        );
                    }
                    continue;
                }

                for (face, [dx, dy, dz]) in NEIGHBOURS.iter().copied().enumerate() {
                    let (nx, ny, nz) = (x as i32 + dx, y as i32 + dy, z as i32 + dz);
                    // Skip faces the neighbour hides — an opaque block hides every
                    // face, water only hides other water (see `Block::hides_face_of`).
                    if neighbour_block(chunk, origin, world, nx, ny, nz).hides_face_of(block) {
                        continue;
                    }

                    // The face is lit per corner — the cells that meet around each one,
                    // averaged in brightness and shaded by any solid block among them.
                    // That is smooth lighting; see `corner_light`.
                    let light =
                        face_light(chunk, origin, world, [x as i32, y as i32, z as i32], face);
                    mesh::push_face(
                        &mut geometry.vertices,
                        &mut geometry.indices,
                        block_origin,
                        face,
                        mesh::FaceStyle {
                            layer: faces.layers[face],
                            light,
                            tint,
                            height: top,
                        },
                    );
                }
            }
        }
    }

    data
}

/// How far `block`'s faces reach above its floor, as a fraction of a block, given the
/// `block` sitting on top of it.
///
/// Everything but the fluids fills its cell exactly. A fluid stops short of the top (see
/// [`Block::surface_height`]) — but only where it is *exposed*: a fluid buried under more fluid
/// keeps the full height, so a column of water (or of lava) has no seam in it, and only the
/// surface you can see sits low. (A fluid under a ceiling still stops short, as in Minecraft.)
fn top_height(block: Block, above: Block) -> f32 {
    let surface = block.surface_height();
    if surface < 1.0 && !above.is_fluid() {
        surface
    } else {
        1.0
    }
}

/// Whether chunk-local `(nx, ny, nz)` is inside this chunk, and so can be read from it
/// directly rather than through the world.
fn in_chunk(nx: i32, ny: i32, nz: i32) -> bool {
    (0..WIDTH as i32).contains(&nx)
        && (0..HEIGHT as i32).contains(&ny)
        && (0..DEPTH as i32).contains(&nz)
}

/// The block at chunk-local `(nx, ny, nz)`.
///
/// Inside the chunk we read directly; outside it we go through the world, which
/// covers both neighbouring chunks and the empty space beyond the world edge.
fn neighbour_block(
    chunk: &Chunk,
    origin: [i32; 3],
    world: &World,
    nx: i32,
    ny: i32,
    nz: i32,
) -> Block {
    if in_chunk(nx, ny, nz) {
        chunk.get(nx, ny, nz)
    } else {
        world.block(origin[0] + nx, origin[1] + ny, origin[2] + nz)
    }
}

/// Both light levels at chunk-local `(nx, ny, nz)`, read the same way — and for the same reason:
/// most of the cells a face is lit by are inside this chunk, where reading one is cheap, and
/// only a face on the border has to go out through the world.
/// Both come back together because a face's corners need both: the sky's brightness is the
/// ambient half of the light, and the block light's is the part no shadow may take away.
fn neighbour_light(
    chunk: &Chunk,
    origin: [i32; 3],
    world: &World,
    nx: i32,
    ny: i32,
    nz: i32,
) -> (u8, u8) {
    if in_chunk(nx, ny, nz) {
        (chunk.sky_light(nx, ny, nz), chunk.block_light(nx, ny, nz))
    } else {
        let (wx, wy, wz) = (origin[0] + nx, origin[1] + ny, origin[2] + nz);
        (world.sky_light(wx, wy, wz), world.block_light(wx, wy, wz))
    }
}

// --- Smooth lighting -------------------------------------------------------------
//
// A face is not lit by one cell but *per corner*: for each of its four corners, the four cells
// that meet there are averaged in brightness, and the corner is darkened by ambient occlusion
// where solid blocks stand beside it. That is Minecraft's smooth lighting, and it is what
// softens an overhang's edge into a gradient and paints the creases in a doorway.

/// One cell of the plane a face's corners sample: how bright it is, and whether it is a solid
/// block that *shades* those corners instead of lighting them.
#[derive(Clone, Copy, Debug)]
struct CornerCell {
    /// The cell's light as brightness, i.e. after the curve ([`light::brightness`]), in both
    /// channels.
    brightness: Brightness,
    /// A full opaque cube, which casts ambient occlusion into the corners it touches.
    occludes: bool,
}

/// The two light channels of a cell as brightnesses, so the smoothing can average both with one
/// piece of arithmetic and only part them again when it packs the vertex.
#[derive(Clone, Copy, Debug, Default)]
struct Brightness {
    /// Sky light: the sun that reached here *around* whatever is nearby.
    sky: f32,
    /// Block light: what the blocks that emit light put here (lava, and torches later).
    block: f32,
}

impl Brightness {
    /// The brightness of a cell at `sky`/`block` light levels.
    fn of(sky: u8, block: u8) -> Self {
        Self {
            sky: light::brightness(sky),
            block: light::brightness(block),
        }
    }
}

impl Add for Brightness {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            sky: self.sky + other.sky,
            block: self.block + other.block,
        }
    }
}

impl Mul<f32> for Brightness {
    type Output = Self;

    fn mul(self, factor: f32) -> Self {
        Self {
            sky: self.sky * factor,
            block: self.block * factor,
        }
    }
}

/// The two axes perpendicular to a face, as indices into `[x, y, z]`: the plane the face lies
/// in, and so the plane the cells that light it sit in.
///
/// [`NEIGHBOURS`] lists the faces `[+X, -X, +Y, -Y, +Z, -Z]`, so a face's own axis is simply
/// `face / 2` and the other two follow it round.
fn face_plane(face: usize) -> (usize, usize) {
    let axis = face / 2;
    ((axis + 1) % 3, (axis + 2) % 3)
}

/// The 3×3 grid of cells a face's corners sample, as chunk-local coordinates: `grid[iu][iv]`
/// is the cell `iu - 1` along axis `u` and `iv - 1` along axis `v` from `front`, where `(u, v)`
/// are the face's two in-plane axes ([`face_plane`]). `grid[1][1]` is `front` itself.
fn corner_grid(front: [i32; 3], face: usize) -> [[[i32; 3]; 3]; 3] {
    let (u, v) = face_plane(face);
    std::array::from_fn(|iu| {
        std::array::from_fn(|iv| {
            let mut at = front;
            at[u] += iu as i32 - 1;
            at[v] += iv as i32 - 1;
            at
        })
    })
}

/// The smoothed light of a face's four corners, packed for its vertices.
///
/// `grid` is the plane of cells around the face, centred on the open cell it looks into
/// (`grid[1][1]`), laid out as [`corner_grid`] does. This is the whole of Minecraft's smooth
/// lighting, and it has two halves:
///
/// * **Averaging.** A corner is lit by the four cells that meet around it — the front cell,
///   the two beside it, and the one diagonally between them — averaged in *brightness*.
///   Averaging light *levels* would step a whole level at a time; averaging the curve's output
///   is what gives a gradient rather than a staircase across a face.
/// * **Ambient occlusion.** A solid cell shades the corner it touches, and two beside each
///   other — a wall meeting a wall — shade it hardest, which is the soft dark crease in a
///   doorway. A solid cell contributes the *front* cell's brightness rather than its own (a
///   solid block holds no light of its own), so a wall beside sunlit ground darkens the crease
///   without dimming the whole face.
///
/// Corners come back in the order the face's own corners do ([`mesh::CUBE_FACES`]:
/// top-left, top-right, bottom-right, bottom-left) — each is worked out from where that corner
/// sits on the face, so a corner's light always lands on that corner.
fn corner_light(grid: [[CornerCell; 3]; 3], face: usize) -> [u32; 4] {
    let front = grid[1][1].brightness;
    // What a cell contributes to the average: its own brightness, unless it is solid — in
    // which case it lights nothing and stands in at the front cell's brightness instead. Both
    // channels are treated alike here; only the lighting pass parts them.
    let lit = |iu: usize, iv: usize| {
        let cell = grid[iu][iv];
        if cell.occludes {
            front
        } else {
            cell.brightness
        }
    };

    let (u, v) = face_plane(face);
    mesh::CUBE_FACES[face].map(|(corner, _)| {
        // This corner's side of the front cell, as a grid index: 0 is -1 along that axis,
        // 2 is +1.
        let (iu, iv) = (
            1 + usize::from(corner[u] > 0.0),
            1 + usize::from(corner[v] > 0.0),
        );

        let (shaded_u, shaded_v) = (grid[iu][1].occludes, grid[1][iv].occludes);
        let occluders = u8::from(shaded_u) + u8::from(shaded_v) + u8::from(grid[iu][iv].occludes);
        // Two solid cells beside each other leave the corner no light at all; otherwise each
        // one costs it a step, and three is open ground under open sky.
        let score = if shaded_u && shaded_v {
            0
        } else {
            3 - occluders
        };

        let average = (front + lit(iu, 1) + lit(1, iv) + lit(iu, iv)) * 0.25;
        let shade = average * AO_SHADE[score as usize];
        mesh::pack_light(
            light::quantise(shade.sky),
            light::quantise(shade.block),
            face,
        )
    })
}

/// The smooth lighting for every corner of one face of the block at chunk-local `block`,
/// ready for [`mesh::FaceStyle::light`].
fn face_light(
    chunk: &Chunk,
    origin: [i32; 3],
    world: &World,
    block: [i32; 3],
    face: usize,
) -> [u32; 4] {
    let [dx, dy, dz] = NEIGHBOURS[face];
    // The open cell the face looks into. Note that it is never opaque — an opaque neighbour
    // would have hidden the face (see `Block::hides_face_of`) — so its light is a real one.
    let front = [block[0] + dx, block[1] + dy, block[2] + dz];
    let grid = corner_grid(front, face).map(|row| {
        row.map(|[nx, ny, nz]| {
            let (sky, block) = neighbour_light(chunk, origin, world, nx, ny, nz);
            CornerCell {
                brightness: Brightness::of(sky, block),
                occludes: neighbour_block(chunk, origin, world, nx, ny, nz).is_opaque(),
            }
        })
    });
    corner_light(grid, face)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_exposed_fluid_stops_short_of_a_full_block() {
        let water = Block::Water.surface_height();
        assert!(water < 1.0, "water really is short: {water}");

        // The surface you can see: it sits low, so the shore shows a step down.
        assert_eq!(top_height(Block::Water, Block::Air), water);
        // Still short under a ceiling, as in Minecraft...
        assert_eq!(top_height(Block::Water, Block::Stone), water);
        // ...but buried under more water it fills its cell, keeping the sea seamless.
        assert_eq!(top_height(Block::Water, Block::Water), 1.0);

        // Lava is a fluid too, so its pool reads as a step down in the same way.
        let lava = Block::Lava.surface_height();
        assert_eq!(top_height(Block::Lava, Block::Air), lava);
        assert_eq!(top_height(Block::Lava, Block::Stone), lava);
        assert_eq!(
            top_height(Block::Lava, Block::Lava),
            1.0,
            "no seam in a pool"
        );

        // Everything else fills its cell, whatever is above it.
        assert_eq!(top_height(Block::Stone, Block::Air), 1.0);
        assert_eq!(top_height(Block::Stone, Block::Water), 1.0);
        assert_eq!(top_height(Block::Dirt, Block::Stone), 1.0);
        assert_eq!(top_height(Block::Grass, Block::Air), 1.0);
    }

    /// A face's plane of cells with every one of them open under `level` of sky light.
    fn open(level: u8) -> [[CornerCell; 3]; 3] {
        let cell = CornerCell {
            brightness: Brightness::of(level, 0),
            occludes: false,
        };
        [[cell; 3]; 3]
    }

    /// The same, but lit *only* by a block source: no daylight at all, `level` of block light.
    fn lava_lit(level: u8) -> [[CornerCell; 3]; 3] {
        let cell = CornerCell {
            brightness: Brightness::of(0, level),
            occludes: false,
        };
        [[cell; 3]; 3]
    }

    /// `grid` with the cell at `(iu, iv)` a solid block. Its own brightness is 0, as a solid
    /// block's is — which is exactly what the smoothing has to ignore.
    fn walled(mut grid: [[CornerCell; 3]; 3], iu: usize, iv: usize) -> [[CornerCell; 3]; 3] {
        grid[iu][iv] = CornerCell {
            brightness: Brightness::default(),
            occludes: true,
        };
        grid
    }

    /// The sky brightness byte of each corner, for readable assertions.
    fn bytes(corners: [u32; 4]) -> [u8; 4] {
        corners.map(|light| (light & 255) as u8)
    }

    /// The block brightness byte of each corner.
    fn block_bytes(corners: [u32; 4]) -> [u8; 4] {
        corners.map(|light| ((light >> 8) & 255) as u8)
    }

    #[test]
    fn a_face_is_lit_by_the_cells_in_its_own_plane() {
        let block = [4, 5, 6];
        for (face, [dx, dy, dz]) in NEIGHBOURS.iter().copied().enumerate() {
            let front = [block[0] + dx, block[1] + dy, block[2] + dz];
            let axis = face / 2;
            let (u, v) = face_plane(face);

            for row in corner_grid(front, face) {
                for at in row {
                    // Every cell is in the plane of the face...
                    assert_eq!(at[axis], front[axis], "face {face} leaves its own plane");
                    // ...and within one cell of the front cell in the other two axes, so the
                    // 3x3 really is the front cell and the eight around it.
                    assert!(
                        (at[u] - front[u]).abs() <= 1 && (at[v] - front[v]).abs() <= 1,
                        "face {face} strays from the front cell: {at:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn open_ground_is_lit_exactly_as_before_smooth_lighting() {
        // Every corner of a face out in the open averages four equally bright cells and is
        // shaded by nothing, so it comes out at full brightness — the very value the mesher
        // packed for the whole face before there was any smoothing. That is what keeps smooth
        // lighting a change to *shaded* faces rather than a rewrite of the picture.
        for face in 0..6 {
            assert_eq!(
                corner_light(open(light::MAX), face),
                [mesh::pack_light(255, 0, face); 4],
                "face {face}"
            );
        }
    }

    #[test]
    fn a_cave_lit_by_lava_keeps_the_glow_in_its_own_channel() {
        // Pitch dark — the sun reaches nothing here — with a block source filling the plane.
        // The sky byte must stay 0 (the sun really is not here) while the block byte comes out
        // at full strength: that is what lets the lighting pass keep the glow through the sun's
        // shadow, and it is why the two are not simply added up in the mesher.
        for face in 0..6 {
            let corners = corner_light(lava_lit(light::MAX), face);
            assert_eq!(bytes(corners), [0; 4], "no daylight in the cave");
            assert_eq!(block_bytes(corners), [255; 4], "and lava's light untouched");
        }
    }

    #[test]
    fn a_wall_shades_lavas_light_out_of_the_corner_beside_it_too() {
        // Ambient occlusion is geometry, not light, so it darkens both channels alike — a
        // crease in a lava-lit room is a crease in whichever light is shining.
        let corners = corner_light(walled(lava_lit(light::MAX), 2, 1), 2);
        assert_eq!(bytes(corners), [0; 4], "still no daylight");
        let block = block_bytes(corners);
        assert_eq!(block[0], 255, "the open corners are lit fully");
        assert!(block[2] < 255 && block[3] < 255, "and the crease is darker");
        assert!(block[2] > 0, "but not black");
    }

    #[test]
    fn a_wall_shades_only_the_corners_beside_it() {
        // Face 2 is a top face, so its plane is (u, v) = (z, x): a wall at grid[2][1] stands
        // off the +z side of the front cell.
        let corners = bytes(corner_light(walled(open(light::MAX), 2, 1), 2));

        // Only the two corners on the wall's side darken...
        assert_eq!(
            corners
                .iter()
                .filter(|brightness| **brightness == 255)
                .count(),
            2,
            "only the corners beside the wall darken: {corners:?}"
        );
        // ...and they are shaded a little, not blanked: the wall's own light (0) never leaks
        // into the average, so this is a crease rather than a hole.
        for shaded in [corners[2], corners[3]] {
            assert!(
                (170..255).contains(&shaded),
                "a crease, not a hole: {shaded}"
            );
        }
    }

    #[test]
    fn two_walls_meeting_make_the_darkest_corner() {
        // A wall off the +z side and another off the +x side: the corner where they meet is
        // the darkest of the four, and the one diagonally away from them is untouched.
        let mut grid = walled(open(light::MAX), 2, 1);
        grid = walled(grid, 1, 2);
        let corners = bytes(corner_light(grid, 2));

        let darkest = (0..4)
            .min_by_key(|corner| corners[*corner])
            .expect("four corners");
        assert_eq!(darkest, 2, "CUBE_FACES[2] lists the +z, +x corner third");
        assert!(corners[darkest] < corners[0], "darker than the open corner");
        assert!(corners[darkest] > 0, "and still not pitch black");
        assert_eq!(corners[0], 255, "the corner away from both walls is open");
    }

    #[test]
    fn the_corner_that_is_lit_is_the_corner_the_face_lists() {
        // Light only the +u, +v cell: the corner that brightens must be the one CUBE_FACES
        // lists with both of those axes positive, or the shading would run backwards across
        // the face — and every face's two axes are ordered differently.
        for face in 0..6 {
            let mut grid = open(0);
            grid[2][2] = CornerCell {
                brightness: Brightness::of(light::MAX, 0),
                occludes: false,
            };
            let corners = corner_light(grid, face);
            let (u, v) = face_plane(face);
            let expected = mesh::CUBE_FACES[face]
                .iter()
                .position(|(corner, _)| corner[u] > 0.0 && corner[v] > 0.0)
                .expect("a corner with both axes positive");

            let lit = (0..4)
                .max_by_key(|corner| corners[*corner] & 255)
                .expect("four corners");
            assert_eq!(lit, expected, "face {face}");
        }
    }

    /// A stone floor with `fill` in the cell beside `(7, 1, 8)` — the same scene the two tests
    /// below read their corners out of, so only what is being tested differs.
    fn floor_with(fill: Block) -> Chunk {
        let mut chunk = Chunk::new();
        for z in 0..DEPTH {
            for x in 0..WIDTH {
                chunk.set(x, 0, z, Block::Stone);
            }
            chunk.set(8, 1, z, fill);
            chunk.set(8, 2, z, fill);
        }
        chunk.relight();
        chunk
    }

    /// The corners of the floor's top face beside the filled column. Face 2's corners run
    /// `(x-, z-), (x+, z-), (x+, z+), (x-, z+)`, so 1 and 2 are the two against it.
    fn corners_beside(fill: Block) -> [u8; 4] {
        // Nothing streamed into the world, so cells outside the chunk read as open sky.
        let world = World::new(0);
        bytes(face_light(
            &floor_with(fill),
            [0, 0, 0],
            &world,
            [7, 0, 8],
            2,
        ))
    }

    #[test]
    fn a_floor_face_darkens_where_a_wall_stands_beside_it() {
        // Real cells out of a real chunk, through the same `face_light` the mesher calls: the
        // corners against the wall are shaded and the ones away from it are untouched, which is
        // the crease you see along the bottom of a wall.
        let corners = corners_beside(Block::Stone);
        assert_eq!(corners[0], 255, "the corner away from the wall is open");
        assert_eq!(corners[3], 255, "and so is the other one");
        assert!(
            corners[1] < 255 && corners[2] < 255,
            "the corners against the wall are shaded: {corners:?}"
        );
    }

    #[test]
    fn a_face_is_dimmed_by_the_light_of_a_cell_beside_it() {
        // Water instead of a wall. Water lets light through and casts no occlusion at all, so
        // the only thing that can dim these corners is the *light* of the cells themselves —
        // and a water cell holds one level less than the air beside it. This is the averaging
        // half of smooth lighting on its own, with nothing to shade the corner.
        let corners = corners_beside(Block::Water);
        assert_eq!(corners[0], 255, "the open corners keep full daylight");
        assert_eq!(corners[3], 255);
        assert!(
            corners[1] < 255 && corners[1] > 200,
            "the corners towards the water are dimmed, and only a little: {corners:?}"
        );
    }

    /// A roofed stone room in a chunk — pitch black inside — with `fill` in the cell at
    /// `(8, 1, 8)`. Laid out like [`floor_with`], but with the sun shut out so the only light
    /// inside is whatever a block emits.
    fn roofed_room_with(fill: Block) -> Chunk {
        let mut chunk = Chunk::new();
        for z in 0..DEPTH {
            for x in 0..WIDTH {
                chunk.set(x, 0, z, Block::Stone);
                chunk.set(x, 6, z, Block::Stone);
            }
        }
        chunk.set(8, 1, 8, fill);
        chunk.relight();
        chunk
    }

    /// The four corners of the floor's top face beside the cell at `(8, 1, z)`, through the same
    /// `face_light` the mesher calls.
    fn floor_corners_beside_room(fill: Block, z: i32) -> [u32; 4] {
        let world = World::new(0);
        face_light(&roofed_room_with(fill), [0, 0, 0], &world, [7, 0, z], 2)
    }

    #[test]
    fn a_lava_block_lights_the_room_it_stands_in() {
        // The floor beside a lava block, and the floor seven blocks away: the sky channel knows
        // nothing in either case (the room is roofed) while the block channel is bright near the
        // lava and dimmer far from it. A block light that did not fall off — or did not reach the
        // vertex at all — would show up here.
        let near = floor_corners_beside_room(Block::Lava, 8);
        let far = floor_corners_beside_room(Block::Lava, 1);

        assert_eq!(bytes(near), [0; 4], "no daylight under a roof");
        assert_eq!(bytes(far), [0; 4]);
        let (near, far) = (block_bytes(near), block_bytes(far));
        let brightest = |corners: [u8; 4]| corners.into_iter().max().unwrap_or(0);
        assert!(
            brightest(near) > 150,
            "the floor beside the lava is lit well: {near:?}"
        );
        assert!(
            brightest(far) < brightest(near),
            "and the light falls off with distance: {far:?} vs {near:?}"
        );
    }

    #[test]
    fn a_room_with_no_source_in_it_stays_dark() {
        // The same room with a plain stone block where the lava was: nothing emits, so both
        // channels are empty. This is the guarantee that adding block light changed nothing
        // about a world with no lava in it.
        let corners = floor_corners_beside_room(Block::Stone, 8);
        assert_eq!(bytes(corners), [0; 4], "no daylight under a roof");
        assert_eq!(block_bytes(corners), [0; 4], "and nothing emitting");
    }
}
