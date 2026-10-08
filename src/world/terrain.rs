//! Procedural terrain generation.
//!
//! The world is built the way modern Minecraft builds it: a **noise field**, not a
//! heightmap. Every point `(x, y, z)` gets a *density*, and a block is solid
//! wherever that density is positive:
//!
//! ```text
//! density(x, y, z) = (surface_target(x, z) - y) + DETAIL * noise_at(x, y, z)
//! ```
//!
//! * `surface_target` is where the ground would like to be, a **2D** multi-octave
//!   Perlin field: a low-frequency *continent* layer lifted by a *relief* layer,
//!   so the world has broad plains and mountain ranges.
//! * `noise_at` is a **combined multi-octave 3D Perlin** field in `[-1, 1]`. Being
//!   3D, it varies vertically as well as horizontally, so instead of a smooth sheet
//!   it carves **overhangs, cliffs and caves**.
//! * the `DETAIL` amplitude sets how far that 3D noise can push the surface, and
//!   therefore how deep those features go.
//!
//! `(surface_target - y)` makes the density fall by one per block going up, so more
//! than `DETAIL` blocks either side of the target the answer is certain: solid
//! below, air above. Only a narrow band around the target is ever sampled — and
//! within it the field is sampled every [`STEP`] blocks and linearly interpolated
//! in between, exactly like Minecraft's density noise.
//!
//! Everything is a pure function of `(seed, x, y, z)`, so neighbouring chunks agree
//! at their borders and the same seed always rebuilds the same world.

use noise::{Fbm, MultiFractal, NoiseFn, Perlin};

use super::ChunkPos;
use super::block::Block;
use super::chunk::{Chunk, DEPTH, HEIGHT, WIDTH};

/// Seed used when none is supplied (see `RUSTCRAFT_SEED` in `main.rs`).
pub const DEFAULT_SEED: u32 = 1337;

/// Block height the terrain is centred on.
pub const BASE_HEIGHT: i32 = 64;

/// Sea level: any open space at or below this height fills with water.
pub const SEA_LEVEL: i32 = 62;

/// Relief (half the height range) of open plains and of mountain ranges, in blocks.
const MIN_HILLS: f32 = 8.0;
const MAX_HILLS: f32 = 44.0;

/// How far the 3D noise can push the surface up or down, in blocks — i.e. how far
/// the world can overhang or hollow itself out.
///
/// This has to beat the `1`-per-block fall of the density below for the field to
/// ever turn back upwards and carve an overhang, and the 3D noise only changes by
/// about `0.02` per block, so it needs to be generous.
const DETAIL: f32 = 35.0;

/// The 3D noise's `y` is stretched by this factor, so it varies faster vertically
/// than horizontally. That vertical variation is what grows overhangs and caves
/// instead of a smooth sheet. (Minecraft does the same by sampling its density
/// noise at `(x/4, y, z/4)`.)
const Y_STRETCH: f32 = 2.0;

/// Half-height of the band sampled around the target surface. Must be at least
/// [`DETAIL`] so the real surface is always inside it.
const WINDOW: i32 = 40;

/// Vertical spacing (in blocks) of the density samples; the gaps are interpolated.
const STEP: i32 = 4;

/// Number of density samples down a column, and of blocks in the sampled window.
const SPAN: i32 = 2 * WINDOW;
const SAMPLES: usize = (SPAN / STEP + 1) as usize;
const WINDOW_LEN: usize = (SPAN + 1) as usize;

/// How many blocks below the surface are soil (dirt) rather than stone.
const SOIL_DEPTH: i32 = 3;

/// The sampled window has to fit inside a chunk's column.
const _: () = assert!(BASE_HEIGHT + WINDOW < HEIGHT as i32);

/// The shape of the world, as a noise field.
pub struct Terrain {
    /// Low-frequency 3D component of the combined noise.
    rough: Fbm<Perlin>,
    /// Higher-frequency 3D component of the combined noise.
    detail: Fbm<Perlin>,
    /// Low-frequency 2D layer: where the continents and oceans of rock sit.
    continent: Fbm<Perlin>,
    /// Low-frequency 2D layer deciding plains versus mountains.
    relief: Fbm<Perlin>,
}

impl Terrain {
    /// Build the terrain for `seed`. The same seed always yields the same world.
    pub fn new(seed: u32) -> Self {
        Self {
            // 3D layers. Each octave doubles the frequency and halves the weight,
            // so every octave contributes equally to how fast the field changes.
            rough: Fbm::<Perlin>::new(seed)
                .set_octaves(2)
                .set_frequency(1.0 / 90.0)
                .set_persistence(0.5),
            detail: Fbm::<Perlin>::new(seed.wrapping_add(0x9E37_79B9))
                .set_octaves(2)
                .set_frequency(1.0 / 36.0)
                .set_persistence(0.5),
            // 2D layers, offset so they are unrelated shapes.
            continent: Fbm::<Perlin>::new(seed.wrapping_add(0x85EB_CA6B))
                .set_octaves(5)
                .set_frequency(1.0 / 180.0)
                .set_persistence(0.5),
            relief: Fbm::<Perlin>::new(seed.wrapping_add(0xC2B2_AE35))
                .set_octaves(3)
                .set_frequency(1.0 / 320.0)
                .set_persistence(0.5),
        }
    }

    /// The combined multi-octave 3D Perlin noise at `(x, y, z)`, always in
    /// **`[-1, 1]`**.
    ///
    /// `-1` is the deepest hollow the noise can make, `+1` the tallest bump. Both
    /// layers are in `[-1, 1]` and the weights sum to 1, so the blend is too.
    pub fn noise_at(&self, x: f32, y: f32, z: f32) -> f32 {
        let point = [x as f64, (y * Y_STRETCH) as f64, z as f64];
        let rough = self.rough.get(point);
        let detail = self.detail.get(point);
        (rough * 0.5 + detail * 0.5) as f32
    }

    /// How many blocks of hills this column gets: a little out on the plains, a lot
    /// up in the mountains.
    fn hills(&self, x: f32, z: f32) -> f32 {
        let factor = self.relief.get([x as f64, z as f64]) as f32; // [-1, 1]
        let plains_to_peaks = 0.5 + 0.5 * factor; // [0, 1]
        MIN_HILLS + (MAX_HILLS - MIN_HILLS) * plains_to_peaks
    }

    /// The block height the ground aims for at `(x, z)`, before 3D detail.
    pub fn surface_target(&self, x: f32, z: f32) -> f32 {
        let continent = self.continent.get([x as f64, z as f64]) as f32; // [-1, 1]
        BASE_HEIGHT as f32 + continent * self.hills(x, z)
    }

    /// The terrain density at `(x, y, z)` for a column whose target is `target`:
    /// positive inside solid ground, negative in the air, measured in blocks.
    fn density_at(&self, x: f32, y: f32, z: f32, target: f32) -> f32 {
        (target - y) + DETAIL * self.noise_at(x, y, z)
    }

    /// Sample one column: which of the [`WINDOW_LEN`] blocks centred on the target
    /// surface are solid, and the `y` of the lowest of them.
    ///
    /// The density is sampled every [`STEP`] blocks and linearly interpolated in
    /// between, the way Minecraft's density noise is — cheap, and the field is
    /// smooth enough that the difference is invisible.
    pub fn solid_column(&self, x: i32, z: i32) -> ([bool; WINDOW_LEN], i32) {
        let (x, z) = (x as f32, z as f32);
        let target = self.surface_target(x, z);
        let base = target.round() as i32 - WINDOW;

        let mut density = [0.0f32; SAMPLES];
        for (k, sample) in density.iter_mut().enumerate() {
            let y = (base + k as i32 * STEP) as f32;
            *sample = self.density_at(x, y, z, target);
        }

        let mut solid = [false; WINDOW_LEN];
        for (i, is_solid) in solid.iter_mut().enumerate() {
            let k = (i / STEP as usize).min(SAMPLES - 1);
            let t = (i % STEP as usize) as f32 / STEP as f32;
            let next = density[(k + 1).min(SAMPLES - 1)];
            *is_solid = density[k] * (1.0 - t) + next * t > 0.0;
        }
        (solid, base)
    }

    /// The `y` of the topmost solid block at world `(x, z)`.
    pub fn surface(&self, x: i32, z: i32) -> i32 {
        let (solid, base) = self.solid_column(x, z);
        match solid.iter().rposition(|&s| s) {
            Some(i) => base + i as i32,
            // All air: the block just under the sampled window is the surface.
            None => base - 1,
        }
    }

    /// Generate the chunk at `pos`: a bedrock floor, then every column filled with
    /// stone and soil up to its surface — capped with grass, or with bare dirt where
    /// the ground lies under the sea — and any remaining open space below
    /// [`SEA_LEVEL`] flooded with water. Air the 3D noise carved out (overhangs and
    /// caves) is left empty: only the clear space above a column's surface floods,
    /// so caves stay dry.
    pub fn generate_chunk(&self, pos: ChunkPos) -> Chunk {
        let mut chunk = Chunk::new();
        let x0 = pos.0 * WIDTH as i32;
        let z0 = pos.1 * DEPTH as i32;

        for z in 0..DEPTH {
            for x in 0..WIDTH {
                let (solid, base) = self.solid_column(x0 + x as i32, z0 + z as i32);
                let top = match solid.iter().rposition(|&s| s) {
                    Some(i) => base + i as i32,
                    None => base - 1,
                };

                chunk.set(x, 0, z, Block::Bedrock);
                for y in 1..top {
                    // Below the sampled window the ground is solid; inside it the
                    // noise decides, leaving pockets where it carved them out.
                    let filled = y < base || solid[(y - base) as usize];
                    if !filled {
                        continue;
                    }
                    let block = if y + SOIL_DEPTH >= top {
                        Block::Dirt
                    } else {
                        Block::Stone
                    };
                    chunk.set(x, y as usize, z, block);
                }
                // Grass only grows out of the water: the seabed is bare dirt.
                let cap = if top < SEA_LEVEL {
                    Block::Dirt
                } else {
                    Block::Grass
                };
                chunk.set(x, top as usize, z, cap);
                // Flood up to sea level, turning low ground into ocean. Only the
                // space *above* the surface floods, so caves stay dry.
                for y in (top + 1)..=SEA_LEVEL {
                    chunk.set(x, y as usize, z, Block::Water);
                }
            }
        }
        chunk
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combined_3d_noise_stays_within_minus_one_and_one() {
        let terrain = Terrain::new(1);
        for x in (-240..240).step_by(13) {
            for y in (0..160).step_by(11) {
                for z in (-240..240).step_by(17) {
                    let n = terrain.noise_at(x as f32, y as f32, z as f32);
                    assert!(
                        (-1.0..=1.0).contains(&n),
                        "noise {n} at ({x}, {y}, {z}) left the [-1, 1] range"
                    );
                }
            }
        }
    }

    #[test]
    fn the_noise_varies_with_height_not_just_x_and_z() {
        // The point of 3D noise: the field is not a heightmap, so some columns have
        // air chewed out below their topmost solid block.
        let terrain = Terrain::new(1);
        let mut pockets = 0;
        for x in 0..192 {
            for z in 0..192 {
                let (solid, _) = terrain.solid_column(x, z);
                let top = solid.iter().rposition(|&s| s).unwrap();
                if solid[..top].iter().any(|&s| !s) {
                    pockets += 1;
                }
            }
        }
        assert!(pockets > 0, "3D noise should carve air pockets, found none");
    }

    #[test]
    fn terrain_has_real_relief() {
        let terrain = Terrain::new(1);
        let (mut min, mut max) = (i32::MAX, i32::MIN);
        for x in (0..640).step_by(8) {
            for z in (0..640).step_by(8) {
                let surface = terrain.surface(x, z);
                min = min.min(surface);
                max = max.max(surface);
            }
        }
        assert!(max - min >= 30, "expected real hills, got {min}..={max}");
    }

    #[test]
    fn the_surface_stays_inside_the_sampled_window() {
        let terrain = Terrain::new(1);
        for x in (0..256).step_by(5) {
            for z in (0..256).step_by(7) {
                let target = terrain.surface_target(x as f32, z as f32);
                let surface = terrain.surface(x, z) as f32;
                assert!(
                    (surface - target).abs() <= WINDOW as f32 + 1.0,
                    "surface {surface} escaped the window around target {target}"
                );
            }
        }
    }

    #[test]
    fn generation_is_deterministic_per_seed() {
        let a = Terrain::new(7);
        let b = Terrain::new(7);
        let other = Terrain::new(8);

        for i in 0..64 {
            assert_eq!(a.surface(i, i * 3), b.surface(i, i * 3));
        }
        assert!(
            (0..64).any(|i| a.surface(i, i * 3) != other.surface(i, i * 3)),
            "a different seed should give different terrain"
        );
    }

    #[test]
    fn chunk_columns_run_from_bedrock_to_the_surface() {
        let terrain = Terrain::new(1);
        let chunk = terrain.generate_chunk((0, 0));
        for z in 0..DEPTH {
            for x in 0..WIDTH {
                let (x, z) = (x as i32, z as i32);
                let surface = terrain.surface(x, z);
                assert_eq!(chunk.get(x, 0, z), Block::Bedrock);
                // Grass above the waterline, bare dirt on the seabed.
                let cap = if surface < SEA_LEVEL {
                    Block::Dirt
                } else {
                    Block::Grass
                };
                assert_eq!(chunk.get(x, surface, z), cap, "surface block");
                // Above the ground: sea water up to sea level, then air.
                let above = if surface < SEA_LEVEL {
                    Block::Water
                } else {
                    Block::Air
                };
                assert_eq!(chunk.get(x, surface + 1, z), above, "just above the ground");
                assert_eq!(chunk.get(x, surface.max(SEA_LEVEL) + 1, z), Block::Air);
            }
        }
    }

    #[test]
    fn low_ground_is_flooded_up_to_sea_level_but_caves_stay_dry() {
        let terrain = Terrain::new(DEFAULT_SEED);
        let (x, z) = (0..256)
            .step_by(3)
            .flat_map(|x| (0..256).step_by(3).map(move |z| (x, z)))
            .find(|&(x, z)| terrain.surface(x, z) < SEA_LEVEL)
            .expect("the world should have some ground below sea level");

        let chunk =
            terrain.generate_chunk((x.div_euclid(WIDTH as i32), z.div_euclid(DEPTH as i32)));
        let (lx, lz) = (x.rem_euclid(WIDTH as i32), z.rem_euclid(DEPTH as i32));
        let ground = terrain.surface(x, z);

        // The seabed is bare dirt, not grass.
        assert_eq!(chunk.get(lx, ground, lz), Block::Dirt, "drowned ground");
        // Flooded from just above the ground right up to the sea...
        assert_eq!(chunk.get(lx, ground + 1, lz), Block::Water);
        assert_eq!(chunk.get(lx, SEA_LEVEL, lz), Block::Water);
        // ...and not above it.
        assert_eq!(chunk.get(lx, SEA_LEVEL + 1, lz), Block::Air);
        // Nothing below the ground is water: the noise's caves stay dry.
        for y in 1..ground {
            assert_ne!(
                chunk.get(lx, y, lz),
                Block::Water,
                "flooded a cave at y={y}"
            );
        }
    }

    #[test]
    fn the_surface_is_grass_above_the_waterline_and_dirt_below_it() {
        let terrain = Terrain::new(DEFAULT_SEED);
        let high = (0..256)
            .step_by(3)
            .flat_map(|x| (0..256).step_by(3).map(move |z| (x, z)))
            .find(|&(x, z)| terrain.surface(x, z) > SEA_LEVEL)
            .expect("the world should have ground above sea level");
        let low = (0..256)
            .step_by(3)
            .flat_map(|x| (0..256).step_by(3).map(move |z| (x, z)))
            .find(|&(x, z)| terrain.surface(x, z) < SEA_LEVEL)
            .expect("the world should have ground below sea level");

        let surface_block = |(x, z): (i32, i32)| {
            let chunk =
                terrain.generate_chunk((x.div_euclid(WIDTH as i32), z.div_euclid(DEPTH as i32)));
            chunk.get(
                x.rem_euclid(WIDTH as i32),
                terrain.surface(x, z),
                z.rem_euclid(DEPTH as i32),
            )
        };

        assert_eq!(surface_block(high), Block::Grass, "{high:?} is dry ground");
        assert_eq!(surface_block(low), Block::Dirt, "{low:?} is drowned ground");
    }
}
