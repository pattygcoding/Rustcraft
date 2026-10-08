//! Minecraft-style sky lighting, at levels 0–15.
//!
//! Sunlight falls straight down at full strength, so every cell with open sky
//! above it sits at [`MAX`]. From there light spreads one block at a time in all
//! six directions, **losing a level each step**, and solid blocks stop it dead —
//! which is why the world is bright in the open and fades to black under overhangs
//! and inside caves. A cell's level is also what the mesher bakes into the faces
//! that look into it, so the shading follows the light.
//!
//! Light is only computed *within a single chunk*. Sunlight is vertical, so the
//! only thing lost is the sideways bleed where a shadow straddles a chunk border;
//! a full cross-chunk light engine is future work.

use std::collections::VecDeque;

use super::chunk::{Chunk, DEPTH, HEIGHT, WIDTH};

/// The brightest light level: full daylight from an open sky.
pub const MAX: u8 = 15;

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
}
