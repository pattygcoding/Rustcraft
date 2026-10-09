//! A 16 × 256 × 16 column of blocks.

use super::block::Block;

/// Chunk size along X and Z (a Minecraft chunk is 16×16).
pub const WIDTH: usize = 16;
pub const DEPTH: usize = 16;
/// Chunk height along Y.
pub const HEIGHT: usize = 256;

const VOLUME: usize = WIDTH * HEIGHT * DEPTH;

/// A vertical column of blocks, indexed `[y][z][x]`.
pub struct Chunk {
    blocks: Vec<Block>,
    /// Sky light level (0–15) per cell, `[y][z][x]` like `blocks`; see [`Chunk::relight`].
    sky_light: Vec<u8>,
    /// Block light level (0–15) per cell, in the same layout: the light of the blocks that emit
    /// it, spread outwards (see [`super::light::block_light`]).
    block_light: Vec<u8>,
    /// One past the highest non-air block; lets meshing skip the empty space above.
    height: usize,
}

impl Chunk {
    /// An all-air chunk.
    pub fn new() -> Self {
        Self {
            blocks: vec![Block::Air; VOLUME],
            sky_light: vec![0; VOLUME],
            block_light: vec![0; VOLUME],
            height: 0,
        }
    }

    /// The block at `(x, y, z)`, or [`Block::Air`] if outside the chunk.
    ///
    /// Out-of-bounds reads returning air means a lone chunk renders its outer
    /// faces (and, once neighbouring chunks exist, that the mesher can sample
    /// across chunk borders).
    pub fn get(&self, x: i32, y: i32, z: i32) -> Block {
        if x < 0 || y < 0 || z < 0 || x >= WIDTH as i32 || y >= HEIGHT as i32 || z >= DEPTH as i32 {
            return Block::Air;
        }
        self.blocks[index(x as usize, y as usize, z as usize)]
    }

    /// Set the block at `(x, y, z)`. Coordinates must be in bounds.
    pub fn set(&mut self, x: usize, y: usize, z: usize, block: Block) {
        self.blocks[index(x, y, z)] = block;
        if !matches!(block, Block::Air) {
            self.height = self.height.max(y + 1);
        }
    }

    /// One past the highest non-air block.
    pub fn height(&self) -> usize {
        self.height
    }

    /// The light level (0–15) at `(x, y, z)`: the brighter of the sky light and the block
    /// light there, which is the light a face looking into this cell is drawn with.
    ///
    /// Reads `0` outside the chunk, in every direction: beyond the chunk's own columns and
    /// above or below the world.
    pub fn light(&self, x: i32, y: i32, z: i32) -> u8 {
        self.sky_light(x, y, z).max(self.block_light(x, y, z))
    }

    /// The **sky light** level (0–15) at `(x, y, z)`, or `0` outside the chunk.
    ///
    /// Sunlight that found its way here *around* whatever is nearby — the ambient half of the
    /// light, and the half the sun's shadow map then adds to.
    pub fn sky_light(&self, x: i32, y: i32, z: i32) -> u8 {
        if x < 0 || y < 0 || z < 0 || x >= WIDTH as i32 || y >= HEIGHT as i32 || z >= DEPTH as i32 {
            return 0;
        }
        self.sky_light[index(x as usize, y as usize, z as usize)]
    }

    /// The **block light** level (0–15) at `(x, y, z)`, or `0` outside the chunk.
    ///
    /// Light emitted by a block rather than by the sky — a lava pool, and later a torch. It is
    /// nobody's shadow to take away, which is why the lighting pass keeps it in its own channel
    /// (see `shaders/lighting.wgsl`).
    pub fn block_light(&self, x: i32, y: i32, z: i32) -> u8 {
        if x < 0 || y < 0 || z < 0 || x >= WIDTH as i32 || y >= HEIGHT as i32 || z >= DEPTH as i32 {
            return 0;
        }
        self.block_light[index(x as usize, y as usize, z as usize)]
    }

    /// Recompute this chunk's sky light and block light from its blocks.
    ///
    /// Call this after generating the chunk, and after any edit: digging a hole lets light in,
    /// placing a block casts a shadow, and placing a *lava* block lights the room.
    pub fn relight(&mut self) {
        self.sky_light = super::light::sky_light(self);
        self.block_light = super::light::block_light(self);
    }
}

/// Index into the flat block array (`[y][z][x]` order).
fn index(x: usize, y: usize, z: usize) -> usize {
    (y * DEPTH + z) * WIDTH + x
}
