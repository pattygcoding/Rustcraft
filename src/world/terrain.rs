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
//!
//! On top of the noise, a few columns grow an **oak**. Where the ground is dry grass
//! a deterministic 5-in-1000 roll plants a bare trunk of 4–6 logs — the base a canopy
//! will later sit on. The roll is a pure function of `(seed, x, z)`, so a tree never
//! flickers in and out as chunks stream around the player.

use noise::{Fbm, MultiFractal, NoiseFn, Perlin};

use super::ChunkPos;
use super::block::Block;
use super::carve;
use super::chunk::{Chunk, DEPTH, HEIGHT, WIDTH};

/// Seed used when none is supplied (see `RUSTCRAFT_SEED` in `main.rs`).
pub const DEFAULT_SEED: u32 = 1337;

/// Block height the terrain is centred on.
pub const BASE_HEIGHT: i32 = 64;

/// Sea level: any open space at or below this height fills with water.
pub const SEA_LEVEL: i32 = 62;

/// Lava level: any open space at or below this height fills with lava.
///
/// Minecraft's lava sea, which is why a cave that goes deep enough stops being something you can
/// walk down and becomes something that glows — and why the caves and ravines are cut *before* this
/// pass runs, so that the bottom of a ravine is a pool rather than a pit. See
/// [`Terrain::fill_lava_sea`].
pub const LAVA_LEVEL: i32 = 11;

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

/// The trunk of a vanilla oak is `StraightTrunkPlacer(4, 2, 0)`: four logs plus
/// `nextInt(3)`, so 4, 5 or 6.
const TRUNK_MIN: i32 = 4;
const TRUNK_MAX: i32 = 6;

/// How many columns in [`PER_MILLE`] grow an oak — 5 in 1000. Counting *per mille*
/// keeps the roll an integer comparison rather than a float one.
const TREE_CHANCE: u32 = 5;
const PER_MILLE: u32 = 1000;

/// How far an oak's leaves reach out from its trunk, in blocks. The widest canopy
/// rows are `2 * LEAF_RADIUS + 1 = 5` across, like Minecraft's oak.
const LEAF_RADIUS: i32 = 2;

/// The four side neighbours of a cell, as `(dx, dz)`.
const SIDES: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// The four diagonals of a cell, as `(dx, dz)` — the corners of a canopy row, once
/// scaled by [`LEAF_RADIUS`].
const DIAGONALS: [(i32, i32); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];

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
    /// The world seed, kept for the decisions that are not noise (where oaks go).
    seed: u32,
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
            seed,
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

    /// The height of the oak that grows on the grass at `(x, z)`, in logs, or `None`
    /// where no tree is due.
    ///
    /// The dice are a pure function of `(seed, x, z)`, so a column generated twice —
    /// or generated by two callers — plants the same tree in the same place.
    /// [`TREE_CHANCE`] columns in [`PER_MILLE`] pass; the rest grow nothing. See
    /// [`TRUNK_MIN`] for the height range.
    pub fn tree_height(&self, x: i32, z: i32) -> Option<i32> {
        if self.tree_roll(x, z, 0) % PER_MILLE >= TREE_CHANCE {
            return None;
        }
        // A second draw off the *same* column, salted so it is independent of the
        // first: how tall a trunk is must not follow from whether it exists.
        let span = (TRUNK_MAX - TRUNK_MIN + 1) as u32;
        Some(TRUNK_MIN + (self.tree_roll(x, z, 0x51ED_2701) % span) as i32)
    }

    /// A hash of the seed and the column `(x, z)`, scrambled so that neighbouring
    /// columns come out unrelated rather than in a smooth noise pattern.
    fn tree_roll(&self, x: i32, z: i32, salt: u32) -> u32 {
        let mut h = self.seed ^ salt;
        h = mix(h ^ (x as u32).wrapping_mul(0x9E37_79B9));
        h = mix(h ^ (z as u32).wrapping_mul(0x85EB_CA6B));
        h
    }

    /// The oak that grows at `(x, z)`, if the dice plant one: the `y` of the grass the
    /// trunk stands on, and the trunk height in logs.
    ///
    /// Trees only take on dry grass, so `None` also covers the drowned columns whose
    /// cap is bare dirt. The cheap hash is asked *first*, so the ground is only
    /// sampled for the 5 columns in 1000 that actually grow something.
    pub fn oak_at(&self, x: i32, z: i32) -> Option<(i32, i32)> {
        let trunk = self.tree_height(x, z)?;
        let ground = self.surface(x, z);
        (ground >= SEA_LEVEL).then_some((ground, trunk))
    }

    /// Sprinkle the leaves of the oak at `(x, z)` into `chunk`.
    ///
    /// `origin` is `chunk`'s world corner, so an oak rooted just *outside* the chunk
    /// still plants the leaves that spill over the border into it: each chunk asks
    /// after every trunk within [`LEAF_RADIUS`] of it and draws the leaves that land
    /// inside, so the two halves of a tree straddling an edge add up.
    ///
    /// The shape is Minecraft's **oak leaf arrangement** — the very same one birch
    /// uses — four rows stacked on the trunk's top log:
    ///
    /// * one block *above* the log: five leaves, a `+` (the log's own cell and its
    ///   four sides) with the diagonals left empty;
    /// * the log's own row: its four sides, plus 1–3 of the diagonals;
    /// * the two rows below: `5 × 5` blocks with the four corners cut, the corners
    ///   themselves filled only now and then.
    ///
    /// The trunk is placed by the column pass; leaves go only into *air*, so they
    /// never cut into the log, the ground or the sea.
    fn place_canopy(&self, chunk: &mut Chunk, origin: (i32, i32), x: i32, z: i32) {
        let Some((ground, trunk)) = self.oak_at(x, z) else {
            return;
        };
        let top = ground + trunk;

        // The tree's own dice shape its canopy — a draw of their own, salted apart
        // from the trunk height already rolled above.
        let dice = self.tree_roll(x, z, 0x2545_F491);
        let diagonals = 1 + dice % 3; // 1, 2 or 3 of the trunk-top row's diagonals
        let corners_top = pick_corners(dice >> 4);
        let corners_bottom = pick_corners(dice >> 9);

        let mut leaf = |dx: i32, y: i32, dz: i32| {
            let (lx, lz) = (x + dx - origin.0, z + dz - origin.1);
            if !(0..WIDTH as i32).contains(&lx)
                || !(0..DEPTH as i32).contains(&lz)
                || !(0..HEIGHT as i32).contains(&y)
            {
                return;
            }
            if chunk.get(lx, y, lz) == Block::Air {
                chunk.set(lx as usize, y as usize, lz as usize, Block::OakLeaves);
            }
        };

        // The top row: a `+` — the cell above the log and its four sides.
        leaf(0, top + 1, 0);
        for (dx, dz) in SIDES {
            leaf(dx, top + 1, dz);
        }

        // The trunk-top row: the four sides, and 1–3 of the diagonals.
        for (dx, dz) in SIDES {
            leaf(dx, top, dz);
        }
        for (i, (dx, dz)) in DIAGONALS.into_iter().enumerate() {
            if (i as u32) < diagonals {
                leaf(dx, top, dz);
            }
        }

        // The two wide rows: everything within `LEAF_RADIUS` except the corners...
        for y in [top - 1, top - 2] {
            for dz in -LEAF_RADIUS..=LEAF_RADIUS {
                for dx in -LEAF_RADIUS..=LEAF_RADIUS {
                    if dx.abs() == LEAF_RADIUS && dz.abs() == LEAF_RADIUS {
                        continue;
                    }
                    leaf(dx, y, dz);
                }
            }
        }
        // ...which the dice fill in, now and then, at the row's four outer corners.
        for (i, (dx, dz)) in DIAGONALS
            .map(|(dx, dz)| (dx * LEAF_RADIUS, dz * LEAF_RADIUS))
            .into_iter()
            .enumerate()
        {
            if (i as u32) < corners_top {
                leaf(dx, top - 1, dz);
            }
            if (i as u32) < corners_bottom {
                leaf(dx, top - 2, dz);
            }
        }
    }

    /// Generate the chunk at `pos`.
    ///
    /// Four passes, in the order Minecraft ran them:
    ///
    /// 1. [`Terrain::fill_columns`] lays the rock down: bedrock, stone, soil, the grass or dirt cap,
    ///    the sea above it, and the oak trunks standing on dry grass.
    /// 2. [`Terrain::carve_rock`] cuts the caves and ravines into that rock, and floods whatever is
    ///    still open at the bottom of the world with lava.
    /// 3. [`Terrain::place_canopies`] hangs the leaves on the trunks the first pass planted.
    ///
    /// The carvers come *after* the trees because they cannot see what they are cutting into: wood
    /// is simply not [`Block::is_carveable`]. That leaves two quirks worth knowing about. A ravine
    /// will cut *around* an oak rather than through it, and a cave that opens the ground under one
    /// leaves the tree standing where it was — which is the lesser evil, since the alternative (trees
    /// after carvers) would leave canopies whose trunks no chunk could see to plant.
    pub fn generate_chunk(&self, pos: ChunkPos) -> Chunk {
        let mut chunk = Chunk::new();
        let x0 = pos.0 * WIDTH as i32;
        let z0 = pos.1 * DEPTH as i32;

        self.fill_columns(&mut chunk, x0, z0);
        self.carve_rock(&mut chunk, pos, x0, z0);
        self.place_canopies(&mut chunk, x0, z0);
        chunk
    }

    /// Fill every column: a bedrock floor, stone and soil up to the column's surface — capped with
    /// grass, or bare dirt where the ground lies under the sea — and the remaining open space below
    /// [`SEA_LEVEL`] flooded with water.
    ///
    /// Air the 3D noise carved out (overhangs and caves) is left empty: only the clear space above a
    /// column's surface floods, so the noise's caves stay dry. Dry grass grows an oak trunk where the
    /// dice say so; the canopy that turns it into a tree comes later, in [`Terrain::place_canopies`].
    fn fill_columns(&self, chunk: &mut Chunk, x0: i32, z0: i32) {
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

                // Dry grass grows an oak where the dice say so: a bare trunk, straight
                // up into open air (a grass cap always has open sky above it — that is
                // why it is grass — so the trunk never collides with anything). The
                // canopy that turns it into a tree comes later.
                if cap == Block::Grass
                    && chunk.get(x as i32, top + 1, z as i32) == Block::Air
                    && let Some(height) = self.tree_height(x0 + x as i32, z0 + z as i32)
                {
                    for y in 1..=height {
                        chunk.set(x, (top + y) as usize, z, Block::OakLog);
                    }
                }
            }
        }
    }

    /// Cut the caves and ravines into the rock, then flood the bottom of the world with lava.
    ///
    /// A carver works in world coordinates and does not know what it is cutting into, so the
    /// decision about what may be *replaced* is made here: stone and soil, and nothing else
    /// ([`Block::is_carveable`]). Bedrock keeps the floor of the world solid, the sea does not drain
    /// into a cave that opens beneath it, and an oak stands on through a ravine.
    fn carve_rock(&self, chunk: &mut Chunk, pos: ChunkPos, x0: i32, z0: i32) {
        carve::carve(self.seed, pos, &mut |wx, y, wz, block| {
            let (lx, lz) = (wx - x0, wz - z0);
            if !(0..WIDTH as i32).contains(&lx)
                || !(0..DEPTH as i32).contains(&lz)
                || !(0..HEIGHT as i32).contains(&y)
            {
                return;
            }
            if chunk.get(lx, y, lz).is_carveable() {
                chunk.set(lx as usize, y as usize, lz as usize, block);
            }
        });
        self.fill_lava_sea(chunk);
    }

    /// Whatever the carvers and the noise left open at or below [`LAVA_LEVEL`] becomes lava.
    ///
    /// This is Minecraft's lava sea, and it is a pass of its own so that neither carver has to know
    /// lava exists: they cut *emptiness*, and the bottom of the world is then filled in. It also
    /// catches what the noise hollowed out down there, so a cave that reaches the bottom of the
    /// world has a glow at the end of it rather than a floor to walk on — see
    /// [`crate::world::light::block_light`], which is what makes that glow reach up the tunnel.
    fn fill_lava_sea(&self, chunk: &mut Chunk) {
        for y in 0..=LAVA_LEVEL as usize {
            for z in 0..DEPTH {
                for x in 0..WIDTH {
                    if chunk.get(x as i32, y as i32, z as i32) == Block::Air {
                        chunk.set(x, y, z, Block::Lava);
                    }
                }
            }
        }
    }

    /// Hang the canopies on the trunks already standing in `chunk`.
    ///
    /// Leaves reach up to [`LEAF_RADIUS`] blocks out from their trunk, so a trunk just *outside*
    /// this chunk still sheds leaves into it: sweep a margin around the chunk and let every oak
    /// within reach fill in the leaves that land here. (The trunk itself can never spill — it is one
    /// block wide.)
    fn place_canopies(&self, chunk: &mut Chunk, x0: i32, z0: i32) {
        for wz in (z0 - LEAF_RADIUS)..(z0 + DEPTH as i32 + LEAF_RADIUS) {
            for wx in (x0 - LEAF_RADIUS)..(x0 + WIDTH as i32 + LEAF_RADIUS) {
                self.place_canopy(chunk, (x0, z0), wx, wz);
            }
        }
    }
}

/// How many of a canopy row's four corners carry a leaf: usually none, rarely all
/// four — which is how often Minecraft fills them (it leaves the corners bare far
/// more often than not).
fn pick_corners(dice: u32) -> u32 {
    match dice % 16 {
        0..=7 => 0,
        8..=11 => 1,
        12..=13 => 2,
        14 => 3,
        _ => 4,
    }
}

/// MurmurHash3's 32-bit finalizer: scrambles `h` so that nearby inputs give
/// unrelated outputs. That avalanche is what makes a hash usable as dice — without
/// it, columns one block apart would roll almost the same number.
fn mix(mut h: u32) -> u32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::light;

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
        let mut intact = 0;
        for z in 0..DEPTH {
            for x in 0..WIDTH {
                let (x, z) = (x as i32, z as i32);
                let surface = terrain.surface(x, z);
                // The floor of the world is bedrock however deep the carvers went above it.
                assert_eq!(chunk.get(x, 0, z), Block::Bedrock, "the floor of the world");
                // Grass above the waterline, bare dirt on the seabed — unless a cave or a ravine
                // opened this very column, which is how a cave gets its way in from the surface.
                let cap = if surface < SEA_LEVEL {
                    Block::Dirt
                } else {
                    Block::Grass
                };
                match chunk.get(x, surface, z) {
                    block if block == cap => intact += 1,
                    Block::Air | Block::Lava => {}
                    opened => panic!("{opened:?} where {cap:?} or a carved-out column should be"),
                }
                // Just above the ground: the sea where it is low, and open air where
                // it is high — or the foot of an oak trunk that is due to grow there.
                let above = if surface < SEA_LEVEL {
                    Block::Water
                } else if terrain.tree_height(x, z).is_some() {
                    Block::OakLog
                } else {
                    Block::Air
                };
                assert_eq!(chunk.get(x, surface + 1, z), above, "just above the ground");
                // The flood never reaches past sea level; only open sky is up here.
                assert_eq!(chunk.get(x, 200, z), Block::Air, "sky above sea level");
            }
        }
        // Carvers open the odd column and no more: the ground is still ground.
        assert!(
            intact > (WIDTH * DEPTH) * 9 / 10,
            "only {intact} of {} columns kept their surface",
            WIDTH * DEPTH
        );
    }

    #[test]
    fn low_ground_is_flooded_up_to_sea_level_but_caves_stay_dry() {
        let terrain = Terrain::new(DEFAULT_SEED);
        // Carvers open the odd column, so this asks for a seabed that is *still* a seabed: what is
        // under test is the flood and the caves the noise made, not what a ravine did to the floor.
        let (x, z, ground) = (0..256)
            .step_by(3)
            .flat_map(|x| (0..256).step_by(3).map(move |z| (x, z)))
            .filter(|&(x, z)| terrain.surface(x, z) < SEA_LEVEL)
            .find_map(|(x, z)| {
                let ground = terrain.surface(x, z);
                (block_at(&terrain, x, ground, z) == Block::Dirt).then_some((x, z, ground))
            })
            .expect("the world should have some ground below sea level");

        let chunk =
            terrain.generate_chunk((x.div_euclid(WIDTH as i32), z.div_euclid(DEPTH as i32)));
        let (lx, lz) = (x.rem_euclid(WIDTH as i32), z.rem_euclid(DEPTH as i32));

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
        // A carved column is no longer a surface at all, so this asks for one the carvers left
        // alone — what is under test is which cap the *terrain* pass laid down.
        let surface_block = |(x, z): (i32, i32)| block_at(&terrain, x, terrain.surface(x, z), z);
        let ground = |dry: bool| {
            (0..256)
                .step_by(3)
                .flat_map(|x| (0..256).step_by(3).map(move |z| (x, z)))
                .filter(|&(x, z)| {
                    let surface = terrain.surface(x, z);
                    if dry {
                        surface > SEA_LEVEL
                    } else {
                        surface < SEA_LEVEL
                    }
                })
                .find(|&(x, z)| surface_block((x, z)).is_carveable())
                .expect("the world should have ground the carvers left alone")
        };

        assert_eq!(
            surface_block(ground(true)),
            Block::Grass,
            "{:?} is dry ground",
            ground(true)
        );
        assert_eq!(
            surface_block(ground(false)),
            Block::Dirt,
            "{:?} is drowned ground",
            ground(false)
        );
    }

    /// Every column of a `side` × `side` patch that grows a tree.
    fn tree_columns(terrain: &Terrain, side: i32) -> Vec<(i32, i32)> {
        let mut columns = Vec::new();
        for x in 0..side {
            for z in 0..side {
                if terrain.tree_height(x, z).is_some() {
                    columns.push((x, z));
                }
            }
        }
        columns
    }

    #[test]
    fn about_five_columns_in_a_thousand_grow_a_tree() {
        let terrain = Terrain::new(DEFAULT_SEED);
        let side = 500;
        let trees = tree_columns(&terrain, side).len();
        let rate = trees as f64 / (side * side) as f64;
        assert!(
            (rate - 0.005).abs() < 0.002,
            "expected about 5 trees in 1000, got {trees} in {}",
            side * side
        );
    }

    #[test]
    fn oak_trunks_vary_in_height_like_vanilla() {
        let terrain = Terrain::new(DEFAULT_SEED);
        let heights: std::collections::BTreeSet<i32> = tree_columns(&terrain, 500)
            .into_iter()
            .filter_map(|(x, z)| terrain.tree_height(x, z))
            .collect();
        assert_eq!(
            heights.into_iter().collect::<Vec<_>>(),
            vec![TRUNK_MIN, TRUNK_MIN + 1, TRUNK_MAX],
            "vanilla oaks are 4, 5 or 6 logs tall"
        );
    }

    #[test]
    fn the_same_seed_plants_the_same_trees() {
        let a = Terrain::new(DEFAULT_SEED);
        let b = Terrain::new(DEFAULT_SEED);
        let other = Terrain::new(DEFAULT_SEED + 1);

        // Same seed, same trees — a chunk rebuilt as it streams back in looks
        // identical, and a tree straddling a border is agreed on by both chunks.
        assert_eq!(tree_columns(&a, 300), tree_columns(&b, 300));
        // A different seed moves them.
        assert_ne!(tree_columns(&a, 300), tree_columns(&other, 300));
    }

    #[test]
    fn every_due_oak_is_planted_as_a_trunk_on_the_grass() {
        let terrain = Terrain::new(DEFAULT_SEED);
        let mut trunks = 0;
        for cx in 0..4 {
            for cz in 0..4 {
                let chunk = terrain.generate_chunk((cx, cz));
                for lz in 0..DEPTH as i32 {
                    for lx in 0..WIDTH as i32 {
                        let (x, z) = (cx * WIDTH as i32 + lx, cz * DEPTH as i32 + lz);
                        let ground = terrain.surface(x, z);
                        // Logs in the column just above its surface.
                        let logs: Vec<i32> = (1..=TRUNK_MAX + 2)
                            .map(|dy| ground + dy)
                            .filter(|&y| chunk.get(lx, y, lz) == Block::OakLog)
                            .collect();

                        // The seabed is bare dirt, so nothing can be planted there.
                        if ground < SEA_LEVEL {
                            assert!(logs.is_empty(), "an oak grew under the sea at ({x}, {z})");
                            continue;
                        }

                        assert!(
                            matches!(chunk.get(lx, ground, lz), Block::Grass | Block::Air),
                            "dry ground: {:?} at ({x}, {z})",
                            chunk.get(lx, ground, lz)
                        );
                        match terrain.tree_height(x, z) {
                            Some(height) => {
                                // A straight trunk of exactly `height` logs, one per
                                // block from the grass up, and clear air above it.
                                let expected: Vec<i32> =
                                    (1..=height).map(|dy| ground + dy).collect();
                                assert_eq!(logs, expected, "trunk at ({x}, {z})");
                                trunks += 1;
                            }
                            None => assert!(
                                logs.is_empty(),
                                "an oak grew at ({x}, {z}) that the dice never called"
                            ),
                        }
                    }
                }
            }
        }
        assert!(
            trunks > 0,
            "16 chunks of this seed should grow several oaks"
        );
    }

    /// The block at a world position, read from whichever chunk holds it — so a leaf
    /// that spilled over a chunk border is found just the same.
    fn block_at(terrain: &Terrain, x: i32, y: i32, z: i32) -> Block {
        terrain
            .generate_chunk((x.div_euclid(WIDTH as i32), z.div_euclid(DEPTH as i32)))
            .get(x.rem_euclid(WIDTH as i32), y, z.rem_euclid(DEPTH as i32))
    }

    /// An oak with no other trunk within `2 * LEAF_RADIUS` blocks — so its canopy is
    /// unmistakably its own — that also stands within [`LEAF_RADIUS`] of a chunk
    /// border, so some of its leaves are forced to spill into the next chunk.
    fn lone_edge_oak(terrain: &Terrain) -> (i32, i32) {
        let clear = 2 * LEAF_RADIUS;
        for x in 0..256_i32 {
            if x.rem_euclid(WIDTH as i32) > 1 {
                continue;
            }
            for z in 0..256_i32 {
                if terrain.tree_height(x, z).is_none() {
                    continue;
                }
                let loner = (-clear..=clear).all(|dx| {
                    (-clear..=clear).all(|dz| {
                        (dx, dz) == (0, 0) || terrain.tree_height(x + dx, z + dz).is_none()
                    })
                });
                if loner {
                    return (x, z);
                }
            }
        }
        panic!("no lone oak near a chunk border in the sampled patch");
    }

    #[test]
    fn an_oak_canopy_is_minecrafts_oak_leaf_arrangement() {
        let terrain = Terrain::new(DEFAULT_SEED);
        let (x, z) = lone_edge_oak(&terrain);
        let trunk = terrain.tree_height(x, z).unwrap();
        let ground = terrain.surface(x, z);
        let top = ground + trunk;
        let leaf =
            |dx: i32, y: i32, dz: i32| block_at(&terrain, x + dx, y, z + dz) == Block::OakLeaves;

        // The trunk runs intact up through the canopy — leaves never cut into it.
        for y in (ground + 1)..=top {
            assert_eq!(block_at(&terrain, x, y, z), Block::OakLog, "log at y = {y}");
        }

        // One above the top log: a `+` of five — the cell above the log and its four
        // sides — with no diagonals.
        assert!(leaf(0, top + 1, 0), "the leaf above the log");
        for (dx, dz) in SIDES {
            assert!(leaf(dx, top + 1, dz), "top row side ({dx}, {dz})");
        }
        for (dx, dz) in DIAGONALS {
            assert!(!leaf(dx, top + 1, dz), "the top row has no diagonals");
        }

        // The trunk-top row: its four sides are leaves.
        for (dx, dz) in SIDES {
            assert!(leaf(dx, top, dz), "trunk-top side ({dx}, {dz})");
        }

        // The two wide rows: every cell within `LEAF_RADIUS` is a leaf bar the
        // corners, which the dice may or may not fill.
        let mut corners = 0;
        for y in [top - 1, top - 2] {
            for dz in -LEAF_RADIUS..=LEAF_RADIUS {
                for dx in -LEAF_RADIUS..=LEAF_RADIUS {
                    if (dx, dz) == (0, 0) {
                        continue; // the trunk
                    }
                    if dx.abs() == LEAF_RADIUS && dz.abs() == LEAF_RADIUS {
                        corners += leaf(dx, y, dz) as i32;
                    } else {
                        assert!(leaf(dx, y, dz), "wide row cell ({dx}, {dz}) at y = {y}");
                    }
                }
            }
        }
        // Four corners per row, at most.
        assert!(corners <= 8, "too many corners filled: {corners}");

        // The canopy reaches no further out, nothing sits above it, and below it the
        // trunk is bare.
        assert!(
            !leaf(LEAF_RADIUS + 1, top - 1, 0),
            "leaves stay within LEAF_RADIUS"
        );
        assert_eq!(
            block_at(&terrain, x, top + 2, z),
            Block::Air,
            "sky above the canopy"
        );
        assert_eq!(
            block_at(&terrain, x, top - 3, z),
            Block::OakLog,
            "bare trunk below"
        );

        // The tree stands right by the border, so reading the far edge of its widest
        // row took a *different* chunk — the canopy crosses the border whole.
        assert_eq!(
            block_at(&terrain, x - LEAF_RADIUS, top - 1, z),
            Block::OakLeaves,
            "the canopy continues into the neighbouring chunk"
        );
    }

    /// The carvers ran, and ran *before* the lava: every cell a carver offered is no longer rock,
    /// and nothing below [`LAVA_LEVEL`] is still open.
    ///
    /// This is the wiring test — the carve pass and the lava pass both inside
    /// [`Terrain::generate_chunk`], in that order — and it is deliberately phrased in terms of what
    /// a carver *offered* rather than what it cut, so that a change to the order shows up here
    /// rather than as a seam thirty chunks from spawn.
    #[test]
    fn the_carvers_cut_the_rock_and_the_lava_fills_the_bottom() {
        let terrain = Terrain::new(DEFAULT_SEED);
        let pos = (0, 0);
        let chunk = terrain.generate_chunk(pos);
        let (x0, z0) = (pos.0 * WIDTH as i32, pos.1 * DEPTH as i32);
        let mut offered = 0;
        carve::carve(terrain.seed, pos, &mut |x, y, z, block| {
            assert_eq!(
                block,
                Block::Air,
                "a carver cuts emptiness and nothing else"
            );
            let here = chunk.get(x - x0, y, z - z0);
            assert!(
                !here.is_carveable(),
                "({x}, {y}, {z}) was carved and is still {here:?}"
            );
            assert!(
                !(here == Block::Air && y <= LAVA_LEVEL),
                "({x}, {y}, {z}) is open below the lava level"
            );
            offered += 1;
        });
        assert!(offered > 0, "the chunk has no carves to check at all");
    }

    /// Below [`LAVA_LEVEL`] nothing is open and there is lava to find; above it, the caves are
    /// still caves.
    #[test]
    fn the_lava_sea_fills_the_bottom_of_the_world() {
        let terrain = Terrain::new(DEFAULT_SEED);
        let (mut open_below, mut lava, mut open_above) = (0, 0, 0);
        for cx in 0..3 {
            for cz in 0..3 {
                let chunk = terrain.generate_chunk((cx, cz));
                for y in 0..HEIGHT {
                    for z in 0..DEPTH {
                        for x in 0..WIDTH {
                            let block = chunk.get(x as i32, y as i32, z as i32);
                            if block == Block::Air {
                                if y as i32 <= LAVA_LEVEL {
                                    open_below += 1;
                                } else {
                                    open_above += 1;
                                }
                            }
                            if block == Block::Lava {
                                lava += 1;
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(
            open_below, 0,
            "{open_below} cells are still open under the lava"
        );
        assert!(
            lava > 0,
            "no lava anywhere — is LAVA_LEVEL under the terrain?"
        );
        assert!(open_above > 0, "no caves above the lava at all");
    }

    /// Bedrock is not [`Block::is_carveable`], so the floor of the world survives whatever the
    /// carvers do above it.
    #[test]
    fn the_floor_of_the_world_survives_the_carvers() {
        let terrain = Terrain::new(DEFAULT_SEED);
        for cx in 0..3 {
            for cz in 0..3 {
                let chunk = terrain.generate_chunk((cx, cz));
                for z in 0..DEPTH {
                    for x in 0..WIDTH {
                        assert_eq!(
                            chunk.get(x as i32, 0, z as i32),
                            Block::Bedrock,
                            "the floor of chunk ({cx}, {cz})"
                        );
                    }
                }
            }
        }
    }

    /// A lava lake lights the cave above it *in its own channel*, and the sky light down there is
    /// nothing at all — which is the difference between a glow and a dark hole.
    ///
    /// This is the whole chain in one test: the carvers hollow a way down, [`LAVA_LEVEL`] fills it,
    /// [`Chunk::relight`] spreads the block light out of it, and the result reaches the cell above
    /// the lava at full strength less one step.
    #[test]
    fn a_lava_lake_lights_the_cave_above_it() {
        let terrain = Terrain::new(DEFAULT_SEED);
        let lit = (0..3)
            .flat_map(|cx| (0..3).map(move |cz| (cx, cz)))
            .find_map(|pos| {
                let mut chunk = terrain.generate_chunk(pos);
                let (x0, z0) = (pos.0 * WIDTH as i32, pos.1 * DEPTH as i32);
                let cell = (0..WIDTH as i32)
                    .flat_map(|x| (0..DEPTH as i32).map(move |z| (x, z)))
                    .flat_map(|(x, z)| (1..=20_i32).map(move |y| (x, y, z)))
                    .find(|&(x, y, z)| {
                        chunk.get(x, y, z) == Block::Lava && chunk.get(x, y + 1, z) == Block::Air
                    })?;
                chunk.relight();
                Some((chunk, (x0, z0), cell))
            });
        let (chunk, (x0, z0), (x, y, z)) =
            lit.expect("no lava with open air above it in 3 × 3 chunks");
        let cell = (x, y + 1, z);
        let place = (x0 + x, y + 1, z0 + z);

        assert_eq!(
            chunk.block_light(cell.0, cell.1, cell.2),
            light::MAX - 1,
            "the cell above a lava lake at {place:?} should be lit by it"
        );
        assert_eq!(
            chunk.sky_light(cell.0, cell.1, cell.2),
            0,
            "no sunlight reaches {place:?}, so the glow is all there is"
        );
    }
}
