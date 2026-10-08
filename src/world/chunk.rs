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
    /// One past the highest non-air block; lets meshing skip the empty space above.
    height: usize,
}

impl Chunk {
    /// An all-air chunk.
    pub fn new() -> Self {
        Self {
            blocks: vec![Block::Air; VOLUME],
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

    /// A superflat chunk: 1 bedrock, 59 stone, 5 dirt, then grass on top (y = 65).
    pub fn superflat() -> Self {
        let mut chunk = Self::new();
        for z in 0..DEPTH {
            for x in 0..WIDTH {
                chunk.set(x, 0, z, Block::Bedrock);
                for y in 1..60 {
                    chunk.set(x, y, z, Block::Stone);
                }
                for y in 60..65 {
                    chunk.set(x, y, z, Block::Dirt);
                }
                chunk.set(x, 65, z, Block::Grass);
            }
        }
        chunk
    }
}

/// Index into the flat block array (`[y][z][x]` order).
fn index(x: usize, y: usize, z: usize) -> usize {
    (y * DEPTH + z) * WIDTH + x
}
