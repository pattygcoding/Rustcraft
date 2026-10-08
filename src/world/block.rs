//! Block types.

use crate::gfx::mesh::BlockFaces;
use crate::gfx::texture::BlockTextures;

/// A block type. `#[repr(u8)]` keeps it one byte, so a chunk stays compact.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Block {
    Air,
    Grass,
    Dirt,
    Stone,
    Bedrock,
}

impl Block {
    /// Whether this block is a solid, fully-opaque cube.
    ///
    /// This drives face culling: a face is emitted only when the block behind it
    /// is *not* solid. (Cutout/blended blocks would need a finer rule later.)
    pub fn is_solid(self) -> bool {
        !matches!(self, Block::Air)
    }

    /// Whether the player can break this block.
    ///
    /// Bedrock is deliberately unbreakable, so you can't mine through the bottom
    /// of the world.
    pub fn is_breakable(self) -> bool {
        !matches!(self, Block::Air | Block::Bedrock)
    }

    /// Resolve this block's per-face texture layers from the loaded textures.
    ///
    /// Face order is `[+X, -X, +Y, -Y, +Z, -Z]`. Grass uses a different top
    /// (`grass_block_top`), side (`grass_block_side`) and bottom (`dirt`), while
    /// the rest use one texture on every face.
    pub fn faces(self, textures: &BlockTextures) -> BlockFaces {
        match self {
            Block::Grass => BlockFaces::column(
                textures.layer("grass_block_top"),
                textures.layer("grass_block_side"),
                textures.layer("dirt"),
            ),
            Block::Dirt => BlockFaces::uniform(textures.layer("dirt")),
            Block::Stone => BlockFaces::uniform(textures.layer("stone")),
            Block::Bedrock => BlockFaces::uniform(textures.layer("bedrock")),
            // Air is never meshed; layer 0 keeps the match total.
            Block::Air => BlockFaces::uniform(0),
        }
    }
}
