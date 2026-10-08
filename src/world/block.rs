//! Block types.

use crate::gfx::mesh::{self, BlockFaces};
use crate::gfx::texture::BlockTextures;

/// Minecraft's default (plains) water colour, slightly transparent, applied over
/// the greyscale water texture. Packed as `0xAARRGGBB`.
const WATER_TINT: u32 = 0x993F76E4;

/// A block type. `#[repr(u8)]` keeps it one byte, so a chunk stays compact.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Block {
    Air,
    Grass,
    Dirt,
    Stone,
    Bedrock,
    Water,
}

impl Block {
    /// Whether this block is air, and so draws nothing.
    pub fn is_air(self) -> bool {
        matches!(self, Block::Air)
    }

    /// A fully opaque cube: it hides whatever is behind it and stops light dead.
    pub fn is_opaque(self) -> bool {
        matches!(
            self,
            Block::Grass | Block::Dirt | Block::Stone | Block::Bedrock
        )
    }

    /// A see-through block, drawn in the second (blended) pass. Water, for now.
    pub fn is_translucent(self) -> bool {
        matches!(self, Block::Water)
    }

    /// Whether light can spread into this block: air and water, but not opaque
    /// cubes.
    pub fn lets_light_through(self) -> bool {
        !self.is_opaque()
    }

    /// Whether this block stops a column's free sunlight, so light can only reach
    /// below it by spreading. Opaque cubes and water both do, which is what makes
    /// the sea dim one level at a time with depth.
    pub fn blocks_sky(self) -> bool {
        !self.is_air()
    }

    /// Whether `self` hides the face of `other` that touches it.
    ///
    /// An opaque neighbour hides every face. A translucent one hides only faces of
    /// its own kind, so a body of water has no faces between its own cells while
    /// the ground beneath it is still drawn through the water.
    pub fn hides_face_of(self, other: Block) -> bool {
        self.is_opaque() || (self.is_translucent() && self == other)
    }

    /// Whether the player can break this block.
    ///
    /// Bedrock is deliberately unbreakable, so you can't mine through the bottom
    /// of the world. Water is not targetable in the first place (see
    /// [`crate::world::World::raycast`]), so it never gets here.
    pub fn is_breakable(self) -> bool {
        self.is_opaque() && !matches!(self, Block::Bedrock)
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
            // Still water for now; flowing water would use `water_flow`.
            Block::Water => BlockFaces::uniform(textures.layer("water_still")),
            // Air is never meshed; layer 0 keeps the match total.
            Block::Air => BlockFaces::uniform(0),
        }
    }

    /// The colour multiplied over this block's textures, as `0xAARRGGBB`.
    ///
    /// Minecraft tints some blocks in code so a single greyscale texture can serve
    /// several of them. Water's texture is greyscale, so it takes [`WATER_TINT`] —
    /// blue, with the alpha of a translucent block. Everything else is opaque
    /// white, which leaves the texture alone.
    pub fn tint(self) -> u32 {
        match self {
            Block::Water => WATER_TINT,
            _ => mesh::UNTINTED,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_water_is_tinted() {
        assert_eq!(Block::Water.tint(), WATER_TINT, "water is blue");
        for block in [
            Block::Air,
            Block::Grass,
            Block::Dirt,
            Block::Stone,
            Block::Bedrock,
        ] {
            assert_eq!(block.tint(), mesh::UNTINTED, "{block:?} is untinted");
        }
    }

    #[test]
    fn the_water_tint_is_blue_rather_than_grey() {
        let [r, g, b] = [
            (WATER_TINT >> 16) & 0xFF,
            (WATER_TINT >> 8) & 0xFF,
            WATER_TINT & 0xFF,
        ];
        assert!(b > r && b > g, "blue should dominate: #{WATER_TINT:06X}");
    }

    #[test]
    fn the_water_tint_is_translucent() {
        let alpha = (WATER_TINT >> 24) & 0xFF;
        assert!(
            alpha < 0xFF,
            "water alpha should let light through: {alpha}"
        );
        assert_eq!(mesh::UNTINTED >> 24, 0xFF, "everything else is opaque");
    }

    #[test]
    fn water_is_translucent_and_terrain_is_opaque() {
        assert!(Block::Water.is_translucent());
        assert!(!Block::Water.is_opaque());
        assert!(Block::Water.lets_light_through(), "light enters water");
        assert!(
            Block::Water.blocks_sky(),
            "but it does stop the free sunlight"
        );

        for block in [Block::Grass, Block::Dirt, Block::Stone, Block::Bedrock] {
            assert!(block.is_opaque(), "{block:?} is opaque");
            assert!(!block.is_translucent());
            assert!(!block.lets_light_through());
        }

        assert!(Block::Air.is_air());
        assert!(Block::Air.lets_light_through());
        assert!(!Block::Air.blocks_sky());
    }

    #[test]
    fn faces_are_culled_only_by_what_really_hides_them() {
        // An opaque neighbour hides any face.
        assert!(Block::Stone.hides_face_of(Block::Stone));
        assert!(Block::Stone.hides_face_of(Block::Water));
        // Water hides other water — no faces inside the sea — but not stone, so the
        // seabed still shows through the water.
        assert!(Block::Water.hides_face_of(Block::Water));
        assert!(!Block::Water.hides_face_of(Block::Stone));
        // Air hides nothing.
        assert!(!Block::Air.hides_face_of(Block::Stone));
        assert!(!Block::Air.hides_face_of(Block::Water));
    }
}
