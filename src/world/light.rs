//! Minecraft-style voxel lighting, at levels 0–15.
//!
//! There are two channels, exactly as in Minecraft:
//!
//! * **Sky light** ([`sky_light`]): sunlight falls straight down at full strength, so every cell
//!   with open sky above it sits at [`MAX`], and light spreads from there one block at a time
//!   in all six directions, losing a level each step. Solid blocks stop it dead — which is why
//!   the world is bright in the open and fades to black under overhangs and inside caves.
//! * **Block light** ([`block_light`]): the same spread, but starting from the blocks that
//!   *emit* light — lava, and torches later — rather than from the sky. It is what makes a
//!   lava pool glow and light the rock around it.
//!
//! A cell's [`Chunk::light`] is the brighter of the two, which is the light the mesher bakes into
//! the faces that look into it. The two are kept apart all the way through, though, because they
//! differ in one buried way: sky light reaches a cell *around* whatever is nearby, so the sun's
//! shadow map decides how much of it lands, while block light is not sunlight at all and no
//! shadow may take it away. See `shaders/lighting.wgsl`.
//!
//! Light is only computed *within a single chunk*. Sunlight is vertical, so the only thing lost
//! is the sideways bleed where a shadow straddles a chunk border; a full cross-chunk light engine
//! is future work.

use std::collections::VecDeque;

use super::chunk::{Chunk, DEPTH, HEIGHT, WIDTH};

/// The brightest light level: full daylight from an open sky.
pub const MAX: u8 = 15;

/// Minecraft's lightmap curve, as a fraction of full daylight: 15 -> 1.0, 8 -> 0.22, 0 -> 0.
///
/// It is deliberately not linear. Falling away quickly with distance from the sky is what
/// makes an overhang read as an overhang and a cave read as dark, and it is why a *level* is
/// a level rather than a fraction.
///
/// This is where the curve lives: the deferred lighting pass used to apply it to a light
/// level read out of the G-buffer, but smooth lighting averages the light of the cells around
/// each *corner* of a face — and that averaging has to happen in brightness, after the curve,
/// or a gradient would step a whole level at a time. So the mesher applies it
/// ([`crate::world::mesher`]), the shaders only multiply, and there is one definition of the
/// curve rather than two that could drift.
pub fn brightness(level: u8) -> f32 {
    let f = f32::from(level) / f32::from(MAX);
    f / (4.0 - 3.0 * f)
}

/// [`brightness`] quantised the way a vertex carries it: 0-255, which is exactly the byte the
/// G-buffer stores per pixel — so an interpolated vertex brightness travels the whole pipeline
/// without losing anything on the way.
pub fn quantise(brightness: f32) -> u8 {
    (brightness * 255.0).round().clamp(0.0, 255.0) as u8
}

/// The six directions light spreads in, as `(dx, dy, dz)`.
const SPREAD: [(i32, i32, i32); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

/// Compute the sky light of every cell in `chunk`, as a fresh `[y][z][x]` array.
pub fn sky_light(chunk: &Chunk) -> Vec<u8> {
    let surface = surface_map(chunk);

    // 1. Sunlight: every cell above its column's surface sees open sky.
    let mut light = vec![0u8; WIDTH * HEIGHT * DEPTH];
    for (x, column) in surface.iter().enumerate() {
        for (z, &top) in column.iter().enumerate() {
            for y in (top + 1)..HEIGHT as i32 {
                light[index(x, y as usize, z)] = MAX;
            }
        }
    }

    // 2. Spread. A lit cell can only brighten its neighbours where it touches
    //    something darker. Sunlight falls straight down, so with no cross-chunk
    //    light the only such places are beside a *taller* neighbouring column —
    //    which is where the flood starts, keeping the open sky untouched.
    let mut queue: VecDeque<(usize, usize, usize)> = VecDeque::new();
    for (x, column) in surface.iter().enumerate() {
        for (z, &top) in column.iter().enumerate() {
            let tallest = sides(x, z)
                .map(|(nx, nz)| surface[nx][nz])
                .max()
                .unwrap_or(-1);
            // Seed the cell just above this column's surface as well: when that
            // surface is water, this is the cell that pours light *into* the sea.
            for y in (top + 1)..=(top + 1).max(tallest) {
                queue.push_back((x, y as usize, z));
            }
        }
    }

    while let Some((x, y, z)) = queue.pop_front() {
        let level = light[index(x, y, z)];
        if level <= 1 {
            continue;
        }
        for (dx, dy, dz) in SPREAD {
            let Some((nx, ny, nz)) = in_bounds(x as i32 + dx, y as i32 + dy, z as i32 + dz) else {
                continue;
            };
            // Opaque blocks stop light dead; air and water let it through, which is
            // how the sea dims a level at a time with depth.
            if !chunk
                .get(nx as i32, ny as i32, nz as i32)
                .lets_light_through()
            {
                continue;
            }
            let cell = index(nx, ny, nz);
            if light[cell] < level - 1 {
                light[cell] = level - 1;
                queue.push_back((nx, ny, nz));
            }
        }
    }

    light
}

/// Compute the block light of every cell in `chunk`, as a fresh `[y][z][x]` array.
///
/// Block light comes from blocks that *emit* it — lava today, torches later (see
/// [`Block::light`]) — and once it is out it spreads exactly as sunlight does: one block at a
/// time in all six directions, **losing a level each step**, and stopped dead by anything
/// opaque. So a lone lava block lights the air beside it at 14, the air beyond that at 13, and
/// so on: a pool is a bright source with a falloff, not a flat glow.
///
/// Lava is itself opaque, so its light pours *out* of it rather than through it. The pool's own
/// cell holds 15 and its neighbours hold 14, which is what matters: a face is only ever lit by
/// the cells it can look into, and never by an opaque one.
pub fn block_light(chunk: &Chunk) -> Vec<u8> {
    let mut light = vec![0u8; WIDTH * HEIGHT * DEPTH];
    let mut queue: VecDeque<(usize, usize, usize)> = VecDeque::new();

    // Every source starts at its own level, and the flood works outwards from there.
    for x in 0..WIDTH {
        for y in 0..HEIGHT {
            for z in 0..DEPTH {
                let level = chunk.get(x as i32, y as i32, z as i32).light();
                if level > 0 {
                    light[index(x, y, z)] = level;
                    queue.push_back((x, y, z));
                }
            }
        }
    }

    while let Some((x, y, z)) = queue.pop_front() {
        let level = light[index(x, y, z)];
        if level <= 1 {
            continue;
        }
        for (dx, dy, dz) in SPREAD {
            let Some((nx, ny, nz)) = in_bounds(x as i32 + dx, y as i32 + dy, z as i32 + dz) else {
                continue;
            };
            // Opaque blocks stop block light too — a stone wall keeps a lava pool's glow on
            // its own side of it.
            if !chunk
                .get(nx as i32, ny as i32, nz as i32)
                .lets_light_through()
            {
                continue;
            }
            let cell = index(nx, ny, nz);
            if light[cell] < level - 1 {
                light[cell] = level - 1;
                queue.push_back((nx, ny, nz));
            }
        }
    }

    light
}

/// The `y` of the highest solid block in each column, or `-1` for an empty column.
fn surface_map(chunk: &Chunk) -> [[i32; DEPTH]; WIDTH] {
    let mut surface = [[-1i32; DEPTH]; WIDTH];
    for (x, column) in surface.iter_mut().enumerate() {
        for (z, top) in column.iter_mut().enumerate() {
            for y in (0..chunk.height() as i32).rev() {
                if chunk.get(x as i32, y, z as i32).blocks_sky() {
                    *top = y;
                    break;
                }
            }
        }
    }
    surface
}

/// The in-chunk horizontal neighbours of `(x, z)`, as `(nx, nz)`.
fn sides(x: usize, z: usize) -> impl Iterator<Item = (usize, usize)> {
    [(1, 0), (-1, 0), (0, 1), (0, -1)]
        .into_iter()
        .filter_map(move |(dx, dz)| {
            let (nx, nz) = (x as i32 + dx, z as i32 + dz);
            ((0..WIDTH as i32).contains(&nx) && (0..DEPTH as i32).contains(&nz))
                .then_some((nx as usize, nz as usize))
        })
}

/// Clamp `(x, y, z)` to the chunk, returning `None` if it falls outside.
fn in_bounds(x: i32, y: i32, z: i32) -> Option<(usize, usize, usize)> {
    if (0..WIDTH as i32).contains(&x)
        && (0..HEIGHT as i32).contains(&y)
        && (0..DEPTH as i32).contains(&z)
    {
        Some((x as usize, y as usize, z as usize))
    } else {
        None
    }
}

/// Index into the flat light array (`[y][z][x]` order, matching [`Chunk`]).
fn index(x: usize, y: usize, z: usize) -> usize {
    (y * DEPTH + z) * WIDTH + x
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::Block;

    #[test]
    fn open_sky_is_full_daylight() {
        let mut chunk = Chunk::new();
        chunk.relight();
        assert_eq!(chunk.light(0, 200, 0), MAX);
        assert_eq!(chunk.light(15, 1, 15), MAX);
    }

    #[test]
    fn light_fades_away_from_the_lit_edge_and_stops_at_solids() {
        let mut chunk = Chunk::new();
        // Solid ground with a one-block-thick roof over the `x < 8` half.
        for z in 0..DEPTH {
            for x in 0..WIDTH {
                chunk.set(x, 0, z, Block::Stone);
                if x < 8 {
                    chunk.set(x, 8, z, Block::Stone);
                }
            }
        }
        chunk.relight();

        // Above the roof, and out in the open, it is full daylight.
        assert_eq!(chunk.light(0, 9, 8), MAX);
        assert_eq!(chunk.light(12, 4, 8), MAX);

        // Under the roof it dims with distance from the open edge at x = 8...
        let near = chunk.light(7, 4, 8);
        let far = chunk.light(1, 4, 8);
        assert!(near < MAX, "under the roof is not full daylight: {near}");
        assert!(far < near, "light falls off inland: {far} vs {near}");

        // Solid blocks hold no light.
        assert_eq!(chunk.light(0, 0, 8), 0);
        assert_eq!(chunk.light(0, 8, 8), 0);
    }

    #[test]
    fn a_sealed_cavity_stays_dark() {
        let mut chunk = Chunk::new();
        // A box sealed on all six sides, so the inside never sees the sky.
        for z in 0..DEPTH {
            for x in 0..WIDTH {
                chunk.set(x, 0, z, Block::Stone);
                chunk.set(x, 5, z, Block::Stone);
                if x == 0 || x == WIDTH - 1 || z == 0 || z == DEPTH - 1 {
                    for y in 0..6 {
                        chunk.set(x, y, z, Block::Stone);
                    }
                }
            }
        }
        chunk.relight();

        // The air inside is cut off from daylight...
        assert_eq!(chunk.light(8, 2, 8), 0);
        // ...while the sky above the roof is not.
        assert_eq!(chunk.light(8, 10, 8), MAX);
    }

    #[test]
    fn light_dims_with_depth_through_water() {
        let mut chunk = Chunk::new();
        // Ground at y = 0, then water from y = 1 up to y = 10, open sky above.
        for z in 0..DEPTH {
            for x in 0..WIDTH {
                chunk.set(x, 0, z, Block::Stone);
                for y in 1..=10 {
                    chunk.set(x, y, z, Block::Water);
                }
            }
        }
        chunk.relight();

        // The sea surface catches the daylight and loses a level per block down.
        assert_eq!(chunk.light(8, 11, 8), MAX, "air above the water");
        assert_eq!(chunk.light(8, 10, 8), MAX - 1, "just under the surface");
        assert_eq!(chunk.light(8, 9, 8), MAX - 2);
        assert!(
            chunk.light(8, 1, 8) < chunk.light(8, 9, 8),
            "light should keep falling with depth: {} vs {}",
            chunk.light(8, 1, 8),
            chunk.light(8, 9, 8)
        );
    }

    #[test]
    fn a_glass_roof_lights_the_room_but_leaves_shade_it() {
        // Two identical rooms, each with a one-block-thick roof: glass over one, leaves
        // over the other.
        let mut glass = Chunk::new();
        let mut leaves = Chunk::new();
        for z in 0..DEPTH {
            for x in 0..WIDTH {
                glass.set(x, 0, z, Block::Stone);
                leaves.set(x, 0, z, Block::Stone);
                glass.set(x, 8, z, Block::Glass);
                leaves.set(x, 8, z, Block::OakLeaves);
            }
        }
        glass.relight();
        leaves.relight();

        // Glass is transparent to the sun, so the room below keeps full daylight...
        assert_eq!(glass.light(8, 4, 8), MAX, "under a glass roof");
        // ...while leaves stop the sunlight and pass it on a level at a time, so the
        // room under them is shaded instead.
        let shaded = leaves.light(8, 4, 8);
        assert!(shaded < MAX, "leaves should shade the ground: {shaded}");
        assert!(shaded > 0, "but not shut the light out entirely: {shaded}");
    }

    #[test]
    fn a_flower_does_not_shade_what_is_around_it() {
        let mut chunk = Chunk::new();
        for z in 0..DEPTH {
            for x in 0..WIDTH {
                chunk.set(x, 0, z, Block::Stone);
                chunk.set(x, 1, z, Block::Poppy);
            }
        }
        chunk.relight();

        // A flower is transparent to the sun, so the cell it stands in — and the air
        // above it — keep full daylight. Only a *solid* block would cast shade.
        assert_eq!(chunk.light(8, 1, 8), MAX, "the flower's own cell");
        assert_eq!(chunk.light(8, 2, 8), MAX, "the air above it");
    }

    #[test]
    fn the_curve_matches_minecrafts_lightmap() {
        // The three points that define the shape: full daylight, a dim room, and black.
        assert_eq!(brightness(MAX), 1.0);
        assert_eq!(brightness(0), 0.0);
        let eight = brightness(8);
        assert!(
            (eight - 0.22).abs() < 0.005,
            "8 is about a fifth of the light: {eight}"
        );
    }

    #[test]
    fn brightness_rises_with_the_level() {
        for level in 1..=MAX {
            assert!(
                brightness(level) > brightness(level - 1),
                "level {level} is brighter than {}",
                level - 1
            );
        }
    }

    /// A dark chunk — a stone floor with a roof over it — with `fill` where the light is.
    fn dark_room(fill: Block) -> Chunk {
        let mut chunk = Chunk::new();
        for z in 0..DEPTH {
            for x in 0..WIDTH {
                chunk.set(x, 0, z, Block::Stone);
                chunk.set(x, 8, z, Block::Stone);
            }
        }
        chunk.set(8, 1, 8, fill);
        chunk.relight();
        chunk
    }

    #[test]
    fn lava_lights_the_cells_around_it_a_level_at_a_time() {
        let chunk = dark_room(Block::Lava);

        // Nothing the sun does gets in here, so every level below is the lava's own.
        assert_eq!(chunk.sky_light(8, 4, 8), 0, "the room is roofed");
        assert_eq!(chunk.block_light(8, 1, 8), MAX, "lava is a full source");
        assert_eq!(chunk.block_light(8, 2, 8), MAX - 1, "one block out");
        assert_eq!(chunk.block_light(8, 3, 8), MAX - 2, "two blocks out");
        // ...and it keeps fading with distance, which is what makes a pool a source with a
        // falloff rather than a flat glow.
        let near = chunk.block_light(8, 4, 8);
        let far = chunk.block_light(8, 7, 8);
        assert!(near < MAX - 2, "the light keeps falling: {near}");
        assert!(
            far < near,
            "and is dimmer far from the lava: {far} vs {near}"
        );
        assert!(far > 0, "but it does reach the roof");
    }

    #[test]
    fn an_opaque_wall_keeps_block_light_on_its_own_side() {
        // A solid slab of stone right through the room, from the floor to the roof, with the
        // lava on its western side. Block light has to stop dead at it — a stone wall is not
        // glass, and the glow does not leak through a wall into the next room.
        let mut chunk = Chunk::new();
        for z in 0..DEPTH {
            for x in 0..WIDTH {
                chunk.set(x, 0, z, Block::Stone);
                chunk.set(x, 8, z, Block::Stone);
                if x == 8 {
                    for y in 1..=7 {
                        chunk.set(x, y, z, Block::Stone);
                    }
                }
            }
        }
        chunk.set(2, 1, 8, Block::Lava);
        chunk.relight();

        assert!(
            chunk.block_light(7, 4, 8) > 0,
            "the lava's own side of the wall is lit"
        );
        assert_eq!(
            chunk.block_light(9, 4, 8),
            0,
            "and the far side is pitch dark"
        );
    }

    #[test]
    fn the_world_is_dark_in_block_light_until_something_emits() {
        // The regression guard for the whole feature: with no source anywhere, the block light
        // channel is empty in every cell, so the light a cell reads is exactly the sky light it
        // read before there was a second channel.
        let mut chunk = Chunk::new();
        for z in 0..DEPTH {
            for x in 0..WIDTH {
                chunk.set(x, 0, z, Block::Stone);
                chunk.set(x, 1, z, Block::Poppy);
                chunk.set(x, 3, z, Block::Glass);
            }
        }
        chunk.relight();

        for y in 0..12 {
            for z in 0..DEPTH {
                for x in 0..WIDTH {
                    let (x, y, z) = (x as i32, y, z as i32);
                    assert_eq!(
                        chunk.block_light(x, y, z),
                        0,
                        "nothing emits at ({x}, {y}, {z})"
                    );
                    assert_eq!(
                        chunk.light(x, y, z),
                        chunk.sky_light(x, y, z),
                        "so a cell reads exactly its sky light"
                    );
                }
            }
        }
    }

    #[test]
    fn a_fluid_passes_block_light_but_is_not_a_source() {
        // Water is a fluid too, and it takes a level off the light the way it does the sun's,
        // but it emits nothing of its own. Only lava does.
        let mut chunk = Chunk::new();
        for z in 0..DEPTH {
            for x in 0..WIDTH {
                chunk.set(x, 0, z, Block::Stone);
                chunk.set(x, 8, z, Block::Stone);
                chunk.set(x, 3, z, Block::Water);
            }
        }
        chunk.set(8, 1, 8, Block::Lava);
        chunk.relight();

        assert_eq!(chunk.block_light(8, 3, 8), MAX - 2, "water dims it a step");
        assert_eq!(
            chunk.block_light(8, 3, 8),
            chunk.block_light(8, 2, 8) - 1,
            "exactly as the sun is dimmed entering the sea"
        );

        // And the same room with water where the lava was stays dark: a fluid is not a source.
        let water = dark_room(Block::Water);
        assert_eq!(water.block_light(8, 1, 8), 0);
        assert_eq!(water.block_light(8, 3, 8), 0);
    }

    #[test]
    fn every_light_level_keeps_its_own_place_in_a_byte() {
        // Smooth lighting averages brightnesses, so a level has to survive as its own byte or
        // two levels would average into the same value and the gradient would flatten.
        let bytes: Vec<u8> = (0..=MAX).map(|level| quantise(brightness(level))).collect();
        let mut unique = bytes.clone();
        unique.dedup();
        assert_eq!(unique.len(), bytes.len(), "one byte each: {bytes:?}");

        assert_eq!(quantise(1.0), 255, "full daylight");
        assert_eq!(quantise(0.0), 0, "pitch dark");
        assert_eq!(quantise(2.0), 255, "and nothing overflows");
        assert_eq!(quantise(-1.0), 0, "nor underflows");
    }
}
