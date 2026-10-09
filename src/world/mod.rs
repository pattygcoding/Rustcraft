//! The voxel world.
//!
//! The world is split into **chunks** — 16 × 256 × 16 columns of blocks, just like
//! Minecraft — and only the chunks near the player are kept in memory. As the
//! player moves, chunks that fall out of range are dropped and chunks that come
//! into range are generated, so the world extends infinitely in every direction
//! while memory stays bounded.
//!
//! Generating those chunks is expensive — a debug-build chunk costs ~26 ms, most of it the noise and
//! the cave carvers — so it happens on
//! background threads ([`ChunkPool`], see [`streaming`]) and the frame loop only ever *collects*
//! what they have finished. Nothing here blocks on generation for the sake of a frame: what has
//! not arrived yet reads as air, exactly as an unloaded chunk does, so the gameplay code above
//! never has to know the difference.
//!
//! Each chunk is one mesh and therefore one draw call, and faces hidden between
//! adjacent solid blocks (including across chunk borders) are never emitted — see
//! [`mesh_chunk`].

mod block;
mod carve;
mod chunk;
mod light;
mod mesher;
mod random;
pub mod streaming;
mod terrain;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use glam::Vec3;

pub use block::Block;
pub use chunk::Chunk;
pub use mesher::mesh_chunk;
pub use streaming::{ChunkPool, GenerationStats};
pub use terrain::{DEFAULT_SEED, SEA_LEVEL, Terrain};

/// How many chunks out from the player's chunk stay loaded (a square patch, so
/// `(2 * RENDER_RADIUS + 1)²` chunks in total).
pub const RENDER_RADIUS: i32 = 6;

/// The position of a chunk, in chunk units.
pub type ChunkPos = (i32, i32);

/// The horizontal size of a chunk, in blocks.
pub const CHUNK_SIZE: i32 = chunk::WIDTH as i32;

/// A block hit by a [`World::raycast`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RayHit {
    /// Coordinates of the opaque block that was hit.
    pub block: [i32; 3],
    /// Unit axis of the face that was entered (e.g. `[0, 1, 0]` for the top).
    ///
    /// Add this to `block` to get the empty cell to place a new block into.
    pub normal: [i32; 3],
}

/// The chunk a world-space position falls in.
///
/// The one place a camera or player position becomes a chunk coordinate, so streaming and spawning
/// agree about which chunk the player is standing in by construction.
pub fn chunk_of(x: f32, z: f32) -> ChunkPos {
    (
        (x / chunk::WIDTH as f32).floor() as i32,
        (z / chunk::DEPTH as f32).floor() as i32,
    )
}

/// The set of nearby chunks making up the world around the player.
pub struct World {
    /// Loaded chunks, keyed by chunk coordinate.
    chunks: HashMap<ChunkPos, Chunk>,
    /// Chunks that still need (re)meshing: newly loaded ones, plus the neighbours
    /// whose culling changed because a chunk appeared or disappeared next to them.
    dirty: HashSet<ChunkPos>,
    /// The chunk the world is currently centred on.
    center: ChunkPos,
    /// The procedural generator, shared with the threads that run it.
    terrain: Arc<Terrain>,
    /// Chunks asked for and not yet arrived. Keeps a position from being asked for twice, and is
    /// what [`World::flush`] waits on.
    pending: HashSet<ChunkPos>,
    /// The threads that generate chunks while the frame loop gets on with drawing.
    pool: ChunkPool,
}

impl World {
    /// An empty world generated from `seed`, with its generator threads running. Call
    /// [`World::update`] to stream in the first chunks, and [`World::flush_around`] to wait for
    /// the ground under a position before standing on it.
    pub fn new(seed: u32) -> Self {
        let terrain = Arc::new(Terrain::new(seed));
        Self {
            chunks: HashMap::new(),
            dirty: HashSet::new(),
            // `i32::MIN` can never be a real chunk, so the first update always runs.
            center: (i32::MIN, i32::MIN),
            pool: ChunkPool::new(Arc::clone(&terrain)),
            terrain,
            pending: HashSet::new(),
        }
    }

    /// Stream chunks so the loaded patch is centred on the player's chunk, and collect any chunks
    /// the generator threads have finished since the last call.
    ///
    /// `player_x`/`player_z` are world-space coordinates. Cheap to call every frame: collecting is
    /// a few non-blocking polls, and the streaming work happens only when the player crossed a
    /// chunk boundary. Returns how many chunks arrived, for the frame-timing log.
    pub fn update(&mut self, player_x: f32, player_z: f32) -> usize {
        let arrived = self.collect();

        let center = chunk_of(player_x, player_z);
        if center == self.center {
            return arrived;
        }
        self.center = center;

        // Drop chunks that fell out of range; the neighbours that remain must be
        // re-meshed because they lost a neighbour.
        let removed: Vec<ChunkPos> = self
            .chunks
            .keys()
            .copied()
            .filter(|&pos| !self.in_range(pos))
            .collect();
        for pos in removed {
            self.chunks.remove(&pos);
            self.dirty.remove(&pos);
            for n in neighbours(pos) {
                if self.in_range(n) {
                    self.dirty.insert(n);
                }
            }
        }

        // The player is not coming back for the chunks they walked away from, so let the threads
        // off that work — and forget it here too, or a caller waiting for the patch would wait for
        // chunks that will never be generated.
        self.pool.discard(|pos| within_range(pos, center));
        self.pending.retain(|&pos| within_range(pos, center));

        // Ask for the chunks that came into range, nearest first: the threads take them off the
        // front of the queue, so the world fills inwards from the player.
        let wanted = missing_chunks(center, |pos| self.chunks.contains_key(&pos), &self.pending);
        for pos in &wanted {
            self.pending.insert(*pos);
        }
        log::debug!(
            "streamed around {center:?}: {} loaded, {} requested, {} dirty",
            self.chunks.len(),
            wanted.len(),
            self.dirty.len()
        );
        self.pool.request(wanted);

        arrived
    }

    /// Wait for every chunk that has been asked for to arrive, blocking the caller.
    ///
    /// Tests use this to get a whole patch generated before asserting on it. The game never does:
    /// it waits only for the ground the player stands on ([`World::flush_around`]), because waiting
    /// for a whole patch is exactly the startup freeze that background generation exists to avoid.
    #[cfg(test)]
    pub fn flush(&mut self) {
        self.wait_for(|world| world.pending.is_empty());
    }

    /// Wait until `pos` and the eight chunks around it are loaded, so the ground under the player
    /// is real before the first frame — and so the chunk they stand in has neighbours to be
    /// meshed against.
    ///
    /// Stops early if there is nothing left in flight.
    pub fn flush_around(&mut self, pos: ChunkPos) {
        self.wait_for(|world| world.neighbourhood_ready(pos));
    }

    /// Whether `pos` and all eight chunks around it have arrived.
    fn neighbourhood_ready(&self, pos: ChunkPos) -> bool {
        (pos.0 - 1..=pos.0 + 1)
            .flat_map(|x| (pos.1 - 1..=pos.1 + 1).map(move |z| (x, z)))
            .all(|pos| self.chunks.contains_key(&pos))
    }

    /// Collect finished chunks until `ready` is satisfied or nothing is left in flight, blocking
    /// on the generator threads in between — which is what makes it a *wait*.
    fn wait_for(&mut self, mut ready: impl FnMut(&Self) -> bool) {
        while !ready(self) {
            if self.pending.is_empty() {
                return;
            }
            match self.pool.recv() {
                Some((pos, chunk)) => self.accept(pos, chunk),
                None => return,
            }
        }
    }

    /// Take everything the generator threads have finished, without blocking.
    ///
    /// Returns how many chunks arrived, which is what the frame-timing log reports.
    fn collect(&mut self) -> usize {
        let arrived = self.pool.poll();
        let count = arrived.len();
        for (pos, chunk) in arrived {
            self.accept(pos, chunk);
        }
        count
    }

    /// File a chunk a generator thread has finished, unless the player has moved on.
    ///
    /// A chunk arriving changes what its neighbours cull, so both it and its neighbours are
    /// marked for re-meshing.
    fn accept(&mut self, pos: ChunkPos, chunk: Chunk) {
        self.pending.remove(&pos);
        if !self.in_range(pos) {
            return;
        }
        self.chunks.insert(pos, chunk);
        self.dirty.insert(pos);
        for n in neighbours(pos) {
            if self.in_range(n) {
                self.dirty.insert(n);
            }
        }
    }

    /// Whether `pos` is within [`RENDER_RADIUS`] of the chunk the world is centred on.
    fn in_range(&self, pos: ChunkPos) -> bool {
        within_range(pos, self.center)
    }

    /// What the generator threads have done so far, for the frame-timing log.
    pub fn generation_stats(&self) -> GenerationStats {
        self.pool.stats()
    }

    /// Take up to `limit` chunks that still need meshing, nearest to the player
    /// first, removing them from the dirty set.
    ///
    /// A chunk that is loaded but whose in-range neighbours have not arrived yet is *left* in the
    /// set: meshing now would draw a wall where the neighbour's blocks will be, and the seam would
    /// have to be meshed away again a moment later. Waiting is cheaper than drawing it.
    pub fn take_dirty(&mut self, limit: usize) -> Vec<ChunkPos> {
        let mut candidates: Vec<ChunkPos> = self
            .dirty
            .iter()
            .copied()
            .filter(|&pos| can_mesh(pos, self.center, |n| self.chunks.contains_key(&n)))
            .collect();
        candidates.sort_by_key(|&pos| distance(pos, self.center));
        candidates.truncate(limit);
        for pos in &candidates {
            self.dirty.remove(pos);
        }
        candidates
    }

    /// Whether a chunk is currently loaded.
    pub fn is_loaded(&self, pos: ChunkPos) -> bool {
        self.chunks.contains_key(&pos)
    }

    /// The loaded chunk at `pos`, if any.
    pub fn chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&pos)
    }

    /// World-space minimum corner of the chunk at `pos`.
    pub fn origin_of(&self, pos: ChunkPos) -> [i32; 3] {
        [pos.0 * chunk::WIDTH as i32, 0, pos.1 * chunk::DEPTH as i32]
    }

    /// The block at world-space `(wx, wy, wz)`, or [`Block::Air`] outside the
    /// generated world (either above/below it, or in an unloaded chunk).
    pub fn block(&self, wx: i32, wy: i32, wz: i32) -> Block {
        if wy < 0 || wy >= chunk::HEIGHT as i32 {
            return Block::Air;
        }
        let pos = (
            wx.div_euclid(chunk::WIDTH as i32),
            wz.div_euclid(chunk::DEPTH as i32),
        );
        let Some(chunk) = self.chunks.get(&pos) else {
            return Block::Air;
        };
        chunk.get(
            wx.rem_euclid(chunk::WIDTH as i32),
            wy,
            wz.rem_euclid(chunk::DEPTH as i32),
        )
    }

    /// The **sky light** level (0–15) at world `(wx, wy, wz)`.
    ///
    /// Unloaded cells read full daylight ([`light::MAX`]): light does not cross
    /// chunk borders, so assuming the outside is lit keeps the world's edge bright
    /// rather than ringed in black.
    pub fn sky_light(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        if wy < 0 {
            return 0;
        }
        if wy >= chunk::HEIGHT as i32 {
            return light::MAX;
        }
        let pos = (
            wx.div_euclid(chunk::WIDTH as i32),
            wz.div_euclid(chunk::DEPTH as i32),
        );
        match self.chunks.get(&pos) {
            Some(chunk) => chunk.sky_light(
                wx.rem_euclid(chunk::WIDTH as i32),
                wy,
                wz.rem_euclid(chunk::DEPTH as i32),
            ),
            None => light::MAX,
        }
    }

    /// The **block light** level (0–15) at world `(wx, wy, wz)`.
    ///
    /// Unloaded cells read **0**, the opposite of the sky light above: a chunk that has not
    /// arrived has no lava in it, and pretending the world's edge glowed would put a ring of
    /// light around the render distance.
    pub fn block_light(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        if wy < 0 || wy >= chunk::HEIGHT as i32 {
            return 0;
        }
        let pos = (
            wx.div_euclid(chunk::WIDTH as i32),
            wz.div_euclid(chunk::DEPTH as i32),
        );
        match self.chunks.get(&pos) {
            Some(chunk) => chunk.block_light(
                wx.rem_euclid(chunk::WIDTH as i32),
                wy,
                wz.rem_euclid(chunk::DEPTH as i32),
            ),
            None => 0,
        }
    }

    /// The **light** level (0–15) at world `(wx, wy, wz)`: the brighter of the sky light and the
    /// block light there.
    ///
    /// This is the light a face looking into the cell is drawn with, so it is what the mesher
    /// asks for when it wants one number rather than the two channels — a flower, say, which has
    /// no corners to smooth and is lit flat by its own cell.
    pub fn light(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        if wy < 0 {
            return 0;
        }
        if wy >= chunk::HEIGHT as i32 {
            return light::MAX;
        }
        let pos = (
            wx.div_euclid(chunk::WIDTH as i32),
            wz.div_euclid(chunk::DEPTH as i32),
        );
        match self.chunks.get(&pos) {
            Some(chunk) => chunk.light(
                wx.rem_euclid(chunk::WIDTH as i32),
                wy,
                wz.rem_euclid(chunk::DEPTH as i32),
            ),
            // Unloaded cells read as the open sky, as they always have.
            None => light::MAX,
        }
    }

    /// Set the block at world-space `(wx, wy, wz)`, marking the affected chunk(s)
    /// for re-meshing.
    ///
    /// A block on a chunk border also touches the neighbouring chunk (its culling
    /// changed there), so that neighbour is marked too.
    pub fn set_block(&mut self, wx: i32, wy: i32, wz: i32, block: Block) {
        if wy < 0 || wy >= chunk::HEIGHT as i32 {
            return;
        }
        let pos = (
            wx.div_euclid(chunk::WIDTH as i32),
            wz.div_euclid(chunk::DEPTH as i32),
        );
        let lx = wx.rem_euclid(chunk::WIDTH as i32) as usize;
        let lz = wz.rem_euclid(chunk::DEPTH as i32) as usize;

        {
            let Some(chunk) = self.chunks.get_mut(&pos) else {
                return;
            };
            chunk.set(lx, wy as usize, lz, block);
            // The edit opens up or shadows the blocks around it, so redo the light.
            chunk.relight();
        }
        self.dirty.insert(pos);

        // Mark any neighbour that shares a border face with the edited block.
        let border_neighbours = [
            (lx == 0).then_some((pos.0 - 1, pos.1)),
            (lx == chunk::WIDTH - 1).then_some((pos.0 + 1, pos.1)),
            (lz == 0).then_some((pos.0, pos.1 - 1)),
            (lz == chunk::DEPTH - 1).then_some((pos.0, pos.1 + 1)),
        ];
        for n in border_neighbours.into_iter().flatten() {
            if self.chunks.contains_key(&n) {
                self.dirty.insert(n);
            }
        }
    }

    /// Cast a ray through the voxel grid and return the first block it hits within
    /// `max_distance`, along with the face it was entered through — the first block a
    /// ray can actually *hit* (see [`Block::is_targetable`]).
    ///
    /// Air and water are both see-through to a ray, so it passes straight through them:
    /// looking at the sea aims at its bed rather than the water, while glass and leaves
    /// are aimed at directly.
    ///
    /// Uses the Amanatides–Woo grid-traversal (a "DDA"): it walks from voxel to
    /// voxel along the ray rather than sampling at fixed steps, so it never skips
    /// a block however thin the angle.
    pub fn raycast(&self, origin: Vec3, direction: Vec3, max_distance: f32) -> Option<RayHit> {
        let dir = direction.normalize_or_zero();
        if dir == Vec3::ZERO {
            return None;
        }

        let mut voxel = [
            origin.x.floor() as i32,
            origin.y.floor() as i32,
            origin.z.floor() as i32,
        ];
        let step = [
            dir.x.signum() as i32,
            dir.y.signum() as i32,
            dir.z.signum() as i32,
        ];
        // Distance along the ray to cross one whole voxel on each axis.
        let t_delta = [
            if dir.x != 0.0 {
                (1.0 / dir.x).abs()
            } else {
                f32::INFINITY
            },
            if dir.y != 0.0 {
                (1.0 / dir.y).abs()
            } else {
                f32::INFINITY
            },
            if dir.z != 0.0 {
                (1.0 / dir.z).abs()
            } else {
                f32::INFINITY
            },
        ];
        // Distance along the ray to the next voxel boundary on each axis.
        let mut t_max = [
            next_boundary(origin.x, voxel[0], dir.x, t_delta[0]),
            next_boundary(origin.y, voxel[1], dir.y, t_delta[1]),
            next_boundary(origin.z, voxel[2], dir.z, t_delta[2]),
        ];

        let mut normal = [0, 0, 0];
        let mut distance = 0.0;
        while distance <= max_distance {
            if self.block(voxel[0], voxel[1], voxel[2]).is_targetable() {
                return Some(RayHit {
                    block: voxel,
                    normal,
                });
            }
            // Step into whichever neighbouring voxel the ray reaches first; the
            // face we enter is the opposite of the step direction.
            if t_max[0] <= t_max[1] && t_max[0] <= t_max[2] {
                normal = [-step[0], 0, 0];
                voxel[0] += step[0];
                distance = t_max[0];
                t_max[0] += t_delta[0];
            } else if t_max[1] <= t_max[2] {
                normal = [0, -step[1], 0];
                voxel[1] += step[1];
                distance = t_max[1];
                t_max[1] += t_delta[1];
            } else {
                normal = [0, 0, -step[2]];
                voxel[2] += step[2];
                distance = t_max[2];
                t_max[2] += t_delta[2];
            }
        }
        None
    }

    /// How many chunks are currently loaded.
    pub fn loaded_count(&self) -> usize {
        self.chunks.len()
    }

    /// The `y` of the topmost solid block at world `(x, z)`, whether or not that
    /// chunk happens to be loaded.
    pub fn surface_height(&self, x: i32, z: i32) -> i32 {
        self.terrain.surface(x, z)
    }
}

impl Default for World {
    /// A world with the default terrain [`DEFAULT_SEED`].
    fn default() -> Self {
        Self::new(DEFAULT_SEED)
    }
}

/// The chunks in range that are neither loaded nor already being generated, nearest to `center`
/// first — the order they are worth generating in.
///
/// Pure, so the streaming decision can be tested without threads: `is_loaded` and `pending` are the
/// two questions that decide whether a position is still wanted.
fn missing_chunks(
    center: ChunkPos,
    is_loaded: impl Fn(ChunkPos) -> bool,
    pending: &HashSet<ChunkPos>,
) -> Vec<ChunkPos> {
    let mut wanted: Vec<ChunkPos> = Vec::new();
    for x in (center.0 - RENDER_RADIUS)..=(center.0 + RENDER_RADIUS) {
        for z in (center.1 - RENDER_RADIUS)..=(center.1 + RENDER_RADIUS) {
            let pos = (x, z);
            if !is_loaded(pos) && !pending.contains(&pos) {
                wanted.push(pos);
            }
        }
    }
    // Nearest first, so the world fills inwards from the player rather than one arbitrary corner
    // at a time. `sort_by_key` is stable, so equal distances keep the (row-major) order above.
    wanted.sort_by_key(|&pos| distance(pos, center));
    wanted
}

/// Whether a chunk can be meshed yet: it must be loaded, and so must every *in-range* chunk beside
/// it.
///
/// A face between two chunks is culled, so meshing against a neighbour that has not arrived would
/// draw a wall where the neighbour's blocks are about to be. A neighbour that is *out* of range is
/// a different matter: it will never be generated at all, so the world's edge is meant to show its
/// faces — that is exactly the shell the current patch has always drawn.
fn can_mesh(pos: ChunkPos, center: ChunkPos, is_loaded: impl Fn(ChunkPos) -> bool) -> bool {
    is_loaded(pos)
        && neighbours(pos)
            .into_iter()
            .all(|n| !within_range(n, center) || is_loaded(n))
}

/// Whether `pos` is within [`RENDER_RADIUS`] of `center` on both axes.
fn within_range(pos: ChunkPos, center: ChunkPos) -> bool {
    (pos.0 - center.0).abs() <= RENDER_RADIUS && (pos.1 - center.1).abs() <= RENDER_RADIUS
}

/// Manhattan distance between two chunk positions, in chunks — how generation and meshing are
/// ordered, so both work inwards from the player.
fn distance(from: ChunkPos, to: ChunkPos) -> i32 {
    (from.0 - to.0).abs() + (from.1 - to.1).abs()
}

/// The four horizontally adjacent chunk coordinates.
///
/// Only these affect face culling (four of the mesher's six face directions are
/// horizontal), so only they need re-meshing when the neighbourhood changes.
fn neighbours((x, z): ChunkPos) -> [ChunkPos; 4] {
    [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)]
}

/// Distance along the ray from `coord` to the next voxel boundary on one axis.
fn next_boundary(coord: f32, voxel: i32, dir: f32, t_delta: f32) -> f32 {
    if dir > 0.0 {
        (voxel as f32 + 1.0 - coord) * t_delta
    } else if dir < 0.0 {
        (coord - voxel as f32) * t_delta
    } else {
        f32::INFINITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One chunk of world movement, in world units.
    const ONE_CHUNK: f32 = chunk::WIDTH as f32;

    #[test]
    fn loads_a_square_patch() {
        let mut world = World::new(DEFAULT_SEED);
        world.update(0.0, 0.0);
        // Chunks are generated on other threads, so wait for them: nothing is loaded until it
        // arrives.
        world.flush();
        let side = (2 * RENDER_RADIUS + 1) as usize;
        assert_eq!(world.loaded_count(), side * side);
    }

    #[test]
    fn moving_replaces_far_chunks_with_near_ones() {
        let mut world = World::new(DEFAULT_SEED);
        world.update(0.0, 0.0);
        world.flush();
        let before = world.loaded_count();

        // Move one chunk east.
        world.update(ONE_CHUNK, 0.0);
        world.flush();

        // The loaded set keeps its size; far-west chunks are dropped and
        // far-east chunks are generated in their place.
        assert_eq!(world.loaded_count(), before);
        assert!(
            world.is_loaded((RENDER_RADIUS + 1, 0)),
            "east edge generated"
        );
        assert!(!world.is_loaded((-RENDER_RADIUS, 0)), "west edge dropped");
        assert!(world.is_loaded((0, 0)), "nearby chunk kept");
    }

    #[test]
    fn streamed_chunks_are_marked_dirty() {
        let mut world = World::new(DEFAULT_SEED);
        world.update(0.0, 0.0);
        world.flush();
        // Drain the initial load.
        world.take_dirty(usize::MAX);

        world.update(ONE_CHUNK, 0.0);
        world.flush();
        let dirty = world.take_dirty(usize::MAX);

        // Only the changed neighbourhood needs meshing — a small fraction of the
        // whole loaded patch.
        assert!(!dirty.is_empty());
        assert!(dirty.len() < world.loaded_count());
    }

    #[test]
    fn block_lookups_respect_chunk_boundaries() {
        let mut world = World::new(DEFAULT_SEED);
        world.update(0.0, 0.0);
        world.flush();
        // Inside a loaded chunk: the surface block on top, bedrock at the bottom.
        // Grass above the waterline, bare dirt where the ground is under the sea.
        let top = world.surface_height(0, 0);
        let cap = if top < SEA_LEVEL {
            Block::Dirt
        } else {
            Block::Grass
        };
        assert_eq!(world.block(0, top, 0), cap);
        assert_eq!(world.block(0, 0, 0), Block::Bedrock);
        // Above the ground: sea water when the ground is low, and otherwise open
        // air — unless an oak trunk is rooted there instead.
        let above = world.block(0, top + 1, 0);
        if top < SEA_LEVEL {
            assert_eq!(above, Block::Water, "the sea floods low ground");
        } else {
            assert!(
                matches!(above, Block::Air | Block::OakLog),
                "dry ground is open sky or the foot of an oak, got {above:?}"
            );
        }
        // Negative coordinates land in the chunk to the west, not the wrong one.
        let west = world.surface_height(-1, -1);
        let west_cap = if west < SEA_LEVEL {
            Block::Dirt
        } else {
            Block::Grass
        };
        assert_eq!(world.block(-1, west, -1), west_cap);
    }

    #[test]
    fn raycast_finds_the_first_opaque_block() {
        let mut world = World::new(DEFAULT_SEED);
        world.update(0.0, 0.0);
        world.flush();

        // Looking straight down from above the highest block in the column, entering
        // its top face. Any sea in the way is see-through, so the ray lands on the
        // ground beneath it — or on an oak trunk standing on the ground, which is why
        // the column's top is found rather than assumed.
        let mut top = world.surface_height(0, 0);
        while world.block(0, top + 1, 0).is_targetable() {
            top += 1;
        }
        let above = (top + 5) as f32 + 0.5;
        let hit = world
            .raycast(Vec3::new(0.5, above, 0.5), Vec3::new(0.0, -1.0, 0.0), 10.0)
            .expect("should hit the ground");
        assert_eq!(hit.block, [0, top, 0]);
        assert_eq!(hit.normal, [0, 1, 0], "entered through the top face");

        // Looking straight up hits nothing.
        assert_eq!(
            world.raycast(Vec3::new(0.5, above, 0.5), Vec3::new(0.0, 1.0, 0.0), 10.0),
            None
        );
        // Something too far away is out of reach (terrain never reaches y = 200).
        assert_eq!(
            world.raycast(Vec3::new(0.5, 200.0, 0.5), Vec3::new(0.0, -1.0, 0.0), 10.0),
            None
        );
    }

    #[test]
    fn breaking_a_block_clears_it_and_marks_the_chunk_dirty() {
        let mut world = World::new(DEFAULT_SEED);
        world.update(0.0, 0.0);
        world.flush();
        world.take_dirty(usize::MAX);

        let top = world.surface_height(1, 1);
        world.set_block(1, top, 1, Block::Air);

        assert_eq!(world.block(1, top, 1), Block::Air);
        // An interior block only dirties its own chunk.
        assert_eq!(world.take_dirty(usize::MAX), vec![(0, 0)]);
    }
    #[test]
    fn flushing_waits_for_every_chunk_that_was_asked_for() {
        let mut world = World::new(DEFAULT_SEED);
        world.update(0.0, 0.0);
        world.flush();

        let side = (2 * RENDER_RADIUS + 1) as usize;
        assert_eq!(world.loaded_count(), side * side);
        assert!(world.pending.is_empty(), "nothing left in flight");
    }

    #[test]
    fn flushing_around_a_position_only_waits_for_the_ground_under_it() {
        let mut world = World::new(DEFAULT_SEED);
        world.update(0.0, 0.0);
        world.flush_around((0, 0));

        // The chunk the player stands in, and the eight sharing a face or a corner with it, are
        // real — which is what the camera needs before it stands on the surface.
        for x in -1..=1 {
            for z in -1..=1 {
                assert!(world.is_loaded((x, z)), "({x}, {z}) should be loaded");
            }
        }
    }

    #[test]
    fn a_missing_patch_is_asked_for_nearest_first() {
        let center = (3, -2);
        let loaded: HashSet<ChunkPos> = [(3, -2), (4, -2)].into_iter().collect();
        let pending: HashSet<ChunkPos> = [(2, -2)].into_iter().collect();

        let wanted = missing_chunks(center, |pos| loaded.contains(&pos), &pending);

        let side = (2 * RENDER_RADIUS + 1) as usize;
        assert_eq!(
            wanted.len(),
            side * side - loaded.len() - pending.len(),
            "in range, and neither loaded nor already being generated"
        );
        assert!(wanted.iter().all(|&pos| within_range(pos, center)));
        assert!(
            !wanted.contains(&(4, -2)) && !wanted.contains(&(2, -2)),
            "loaded and in-flight chunks are not asked for again"
        );
        assert_eq!(
            distance(wanted[0], center),
            1,
            "the centre and its neighbour are loaded and in flight, so the four chunks beside them come first"
        );
        for pair in wanted.windows(2) {
            assert!(
                distance(pair[0], center) <= distance(pair[1], center),
                "{:?} should not be asked for before {:?}",
                pair[1],
                pair[0]
            );
        }
    }

    #[test]
    fn a_chunk_waits_for_its_neighbours_before_it_is_meshed() {
        let center = (0, 0);
        let corner: HashSet<ChunkPos> = [(0, 0), (1, 0)].into_iter().collect();
        assert!(
            !can_mesh((0, 0), center, |pos| corner.contains(&pos)),
            "three of its sides are still generating"
        );

        // Out-of-range neighbours are never waited for: the world's edge shows its faces, because
        // nothing is ever going to be generated beyond it.
        let edge = (RENDER_RADIUS, 0);
        let loaded: HashSet<ChunkPos> = [
            edge,
            (RENDER_RADIUS - 1, 0),
            (RENDER_RADIUS, -1),
            (RENDER_RADIUS, 1),
        ]
        .into_iter()
        .collect();
        assert!(can_mesh(edge, center, |pos| loaded.contains(&pos)));
    }

    #[test]
    fn take_dirty_withholds_chunks_whose_neighbours_are_missing() {
        let mut world = World::new(DEFAULT_SEED);
        // Hand the world chunks directly: no generator threads involved, so the decision under
        // test is the only thing happening.
        world.center = (0, 0);
        for pos in [(0, 0), (1, 0)] {
            world.chunks.insert(pos, Chunk::new());
            world.dirty.insert(pos);
        }

        assert!(
            world.take_dirty(usize::MAX).is_empty(),
            "meshing now would draw a wall where the neighbour's blocks will be"
        );

        // Fill in the rest of the three-by-three, and both are ready to be drawn.
        for x in -1..=2 {
            for z in -1..=1 {
                world.chunks.insert((x, z), Chunk::new());
            }
        }
        assert_eq!(world.take_dirty(usize::MAX), vec![(0, 0), (1, 0)]);
    }

    #[test]
    fn a_chunk_arriving_dirties_itself_and_the_chunks_it_shares_a_face_with() {
        let mut world = World::new(DEFAULT_SEED);
        world.center = (0, 0);
        world.dirty.clear();

        world.accept((2, 3), Chunk::new());

        assert!(world.is_loaded((2, 3)));
        assert!(!world.pending.contains(&(2, 3)));
        let expected: HashSet<ChunkPos> = [(2, 3), (1, 3), (3, 3), (2, 2), (2, 4)]
            .into_iter()
            .collect();
        assert_eq!(world.dirty, expected);

        // A chunk that arrives after the player has moved on is dropped, and dirties nothing.
        world.dirty.clear();
        world.center = (100, 100);
        world.accept((50, 50), Chunk::new());
        assert!(
            !world.is_loaded((50, 50)),
            "out-of-range arrivals are thrown away"
        );
        assert!(world.dirty.is_empty());
    }
}
