//! The voxel world.
//!
//! The world is split into **chunks** — 16 × 256 × 16 columns of blocks, just like
//! Minecraft — and only the chunks near the player are kept in memory. As the
//! player moves, chunks that fall out of range are dropped and chunks that come
//! into range are generated, so the world extends infinitely in every direction
//! while memory stays bounded.
//!
//! Each chunk is one mesh and therefore one draw call, and faces hidden between
//! adjacent solid blocks (including across chunk borders) are never emitted — see
//! [`mesh_chunk`].

mod block;
mod chunk;
mod mesher;

use std::collections::{HashMap, HashSet};

use block::Block;

pub use chunk::Chunk;
pub use mesher::mesh_chunk;

/// How many chunks out from the player's chunk stay loaded (a square patch, so
/// `(2 * RENDER_RADIUS + 1)²` chunks in total).
pub const RENDER_RADIUS: i32 = 6;

/// The position of a chunk, in chunk units.
pub type ChunkPos = (i32, i32);

/// The set of nearby chunks making up the world around the player.
pub struct World {
    /// Loaded chunks, keyed by chunk coordinate.
    chunks: HashMap<ChunkPos, Chunk>,
    /// Chunks that still need (re)meshing: newly loaded ones, plus the neighbours
    /// whose culling changed because a chunk appeared or disappeared next to them.
    dirty: HashSet<ChunkPos>,
    /// The chunk the world is currently centred on.
    center: ChunkPos,
}

impl World {
    /// An empty world. Call [`World::update`] to stream in the first chunks.
    pub fn new() -> Self {
        Self {
            chunks: HashMap::new(),
            dirty: HashSet::new(),
            // `i32::MIN` can never be a real chunk, so the first update always runs.
            center: (i32::MIN, i32::MIN),
        }
    }

    /// Stream chunks so the loaded patch is centred on the player's chunk.
    ///
    /// `player_x`/`player_z` are world-space coordinates. Cheap to call every
    /// frame: it does nothing unless the player crossed a chunk boundary.
    pub fn update(&mut self, player_x: f32, player_z: f32) {
        let center = (
            (player_x / chunk::WIDTH as f32).floor() as i32,
            (player_z / chunk::DEPTH as f32).floor() as i32,
        );
        if center == self.center {
            return;
        }
        self.center = center;

        let r = RENDER_RADIUS;
        let in_range = |x: i32, z: i32| (x - center.0).abs() <= r && (z - center.1).abs() <= r;

        // Drop chunks that fell out of range; the neighbours that remain must be
        // re-meshed because they lost a neighbour.
        let removed: Vec<ChunkPos> = self
            .chunks
            .keys()
            .copied()
            .filter(|&(x, z)| !in_range(x, z))
            .collect();
        for pos in removed {
            self.chunks.remove(&pos);
            self.dirty.remove(&pos);
            for n in neighbours(pos) {
                if in_range(n.0, n.1) {
                    self.dirty.insert(n);
                }
            }
        }

        // Generate the chunks that came into range; they and their neighbours
        // must be meshed.
        for x in (center.0 - r)..=(center.0 + r) {
            for z in (center.1 - r)..=(center.1 + r) {
                let pos = (x, z);
                if self.chunks.contains_key(&pos) {
                    continue;
                }
                self.chunks.insert(pos, Chunk::superflat());
                self.dirty.insert(pos);
                for n in neighbours(pos) {
                    if in_range(n.0, n.1) {
                        self.dirty.insert(n);
                    }
                }
            }
        }

        log::debug!(
            "streamed around {center:?}: {} chunks loaded, {} dirty",
            self.chunks.len(),
            self.dirty.len()
        );
    }

    /// Take up to `limit` chunks that still need meshing, nearest to the player
    /// first, removing them from the dirty set.
    pub fn take_dirty(&mut self, limit: usize) -> Vec<ChunkPos> {
        let mut candidates: Vec<ChunkPos> = self
            .dirty
            .iter()
            .copied()
            .filter(|pos| self.chunks.contains_key(pos))
            .collect();
        candidates.sort_by_key(|&(x, z)| (x - self.center.0).abs() + (z - self.center.1).abs());
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

    /// How many chunks are currently loaded.
    pub fn loaded_count(&self) -> usize {
        self.chunks.len()
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

/// The four horizontally adjacent chunk coordinates.
///
/// Only these affect face culling (four of the mesher's six face directions are
/// horizontal), so only they need re-meshing when the neighbourhood changes.
fn neighbours((x, z): ChunkPos) -> [ChunkPos; 4] {
    [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One chunk of world movement, in world units.
    const ONE_CHUNK: f32 = chunk::WIDTH as f32;

    #[test]
    fn loads_a_square_patch() {
        let mut world = World::new();
        world.update(0.0, 0.0);
        let side = (2 * RENDER_RADIUS + 1) as usize;
        assert_eq!(world.loaded_count(), side * side);
    }

    #[test]
    fn moving_replaces_far_chunks_with_near_ones() {
        let mut world = World::new();
        world.update(0.0, 0.0);
        let before = world.loaded_count();

        // Move one chunk east.
        world.update(ONE_CHUNK, 0.0);

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
        let mut world = World::new();
        world.update(0.0, 0.0);
        // Drain the initial load.
        world.take_dirty(usize::MAX);

        world.update(ONE_CHUNK, 0.0);
        let dirty = world.take_dirty(usize::MAX);

        // Only the changed neighbourhood needs meshing — a small fraction of the
        // whole loaded patch.
        assert!(!dirty.is_empty());
        assert!(dirty.len() < world.loaded_count());
    }

    #[test]
    fn block_lookups_respect_chunk_boundaries() {
        let mut world = World::new();
        world.update(0.0, 0.0);
        // Inside a loaded chunk: grass on top (y = 65), bedrock bottom.
        assert_eq!(world.block(0, 65, 0), Block::Grass);
        assert_eq!(world.block(0, 0, 0), Block::Bedrock);
        assert_eq!(world.block(0, 66, 0), Block::Air);
        // Negative coordinates land in the chunk to the west, not the wrong one.
        assert_eq!(world.block(-1, 65, -1), Block::Grass);
    }
}
