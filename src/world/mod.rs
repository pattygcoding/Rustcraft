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

use glam::Vec3;

pub use block::Block;
pub use chunk::Chunk;
pub use mesher::mesh_chunk;

/// How many chunks out from the player's chunk stay loaded (a square patch, so
/// `(2 * RENDER_RADIUS + 1)²` chunks in total).
pub const RENDER_RADIUS: i32 = 6;

/// The position of a chunk, in chunk units.
pub type ChunkPos = (i32, i32);

/// A block hit by a [`World::raycast`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RayHit {
    /// Coordinates of the solid block that was hit.
    pub block: [i32; 3],
    /// Unit axis of the face that was entered (e.g. `[0, 1, 0]` for the top).
    ///
    /// Add this to `block` to get the empty cell to place a new block into.
    pub normal: [i32; 3],
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

    /// Cast a ray through the voxel grid and return the first solid block it hits
    /// within `max_distance`, along with the face it was entered through.
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
            if self.block(voxel[0], voxel[1], voxel[2]).is_solid() {
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

    #[test]
    fn raycast_finds_the_first_solid_block() {
        let mut world = World::new();
        world.update(0.0, 0.0);

        // Looking straight down from above the surface (grass tops at y = 65),
        // entering the top face.
        let hit = world
            .raycast(Vec3::new(0.5, 70.0, 0.5), Vec3::new(0.0, -1.0, 0.0), 10.0)
            .expect("should hit the ground");
        assert_eq!(hit.block, [0, 65, 0]);
        assert_eq!(hit.normal, [0, 1, 0], "entered through the top face");

        // Looking straight up hits nothing.
        assert_eq!(
            world.raycast(Vec3::new(0.5, 70.0, 0.5), Vec3::new(0.0, 1.0, 0.0), 10.0),
            None
        );
        // Something too far away is out of reach.
        assert_eq!(
            world.raycast(Vec3::new(0.5, 200.0, 0.5), Vec3::new(0.0, -1.0, 0.0), 10.0),
            None
        );
    }

    #[test]
    fn breaking_a_block_clears_it_and_marks_the_chunk_dirty() {
        let mut world = World::new();
        world.update(0.0, 0.0);
        world.take_dirty(usize::MAX);

        world.set_block(1, 65, 1, Block::Air);

        assert_eq!(world.block(1, 65, 1), Block::Air);
        // An interior block only dirties its own chunk.
        assert_eq!(world.take_dirty(usize::MAX), vec![(0, 0)]);
    }
}
