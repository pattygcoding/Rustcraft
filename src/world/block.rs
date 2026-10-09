//! Block types.

use super::light;
use crate::gfx::mesh::{self, BlockFaces};
use crate::gfx::texture::BlockTextures;

/// Minecraft's default (plains) water colour, slightly transparent, applied over
/// the greyscale water texture. Packed as `0xAARRGGBB`.
const WATER_TINT: u32 = 0x993F76E4;

/// Minecraft's default (plains) foliage colour, applied over the greyscale oak-leaf
/// texture. Packed as `0xAARRGGBB`; fully opaque, because leaves are cut out rather
/// than blended (see [`Block::is_blended`]).
const FOLIAGE_TINT: u32 = 0xFF77AB2F;

/// How high a fluid's surface reaches, as a fraction of a block (Minecraft's 14/16).
///
/// Water and lava deliberately do not quite fill their cell, so a shoreline shows a step
/// down to the surface instead of joining the land flush. See [`Block::surface_height`].
const FLUID_SURFACE: f32 = 0.875;

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
    Lava,
    Glass,
    OakLog,
    OakPlanks,
    OakLeaves,
    Poppy,
    Dandelion,
}

impl Block {
    /// Whether this block is air, and so draws nothing.
    pub fn is_air(self) -> bool {
        matches!(self, Block::Air)
    }

    /// Whether this block is a *liquid*: water or lava.
    ///
    /// Both are the blocks that fill their cell only part way up ([`Block::surface_height`]),
    /// that a ray passes straight through ([`Block::is_targetable`]), and that a placed block
    /// replaces ([`Block::is_replaceable`]). What separates them is light and looks: water is
    /// see-through and dims the sunlight, while lava is opaque and lights the dark.
    pub fn is_fluid(self) -> bool {
        matches!(self, Block::Water | Block::Lava)
    }

    /// How much light this block *emits* of its own, at levels 0–15.
    ///
    /// Lava is the only source for now, at full strength; everything else is dark. This is the
    /// hook block light spreads from (see [`crate::world::light::block_light`]) and the one a
    /// torch will share.
    pub fn light(self) -> u8 {
        match self {
            Block::Lava => light::MAX,
            _ => 0,
        }
    }

    /// Whether this block is a *cross sprite*: two quads crossed at right angles rather
    /// than a cube, the way Minecraft draws flowers (see [`mesh::CROSS_PLANES`]).
    pub fn is_cross(self) -> bool {
        matches!(self, Block::Poppy | Block::Dandelion)
    }

    /// Whether this block fills its cell with cube geometry, so that its faces lie flush
    /// against a neighbour's.
    ///
    /// Air has no geometry at all, and a cross sprite's planes cut through the middle of
    /// its cell at an angle, touching neither the cell's walls nor a neighbour's face.
    /// Only a full cube can hide — or be hidden by — a flush face; see
    /// [`Block::hides_face_of`].
    pub fn is_full_cube(self) -> bool {
        !self.is_air() && !self.is_cross()
    }

    /// A full opaque cube: it hides whatever is behind it and stops light dead.
    ///
    /// Lava is on this list: you cannot see through it, and neither sunlight nor block light
    /// spreads *into* it — light spreads out of it instead (see [`Block::light`]).
    pub fn is_opaque(self) -> bool {
        matches!(
            self,
            Block::Grass
                | Block::Dirt
                | Block::Stone
                | Block::Bedrock
                | Block::OakLog
                | Block::OakPlanks
                | Block::Lava
        )
    }

    /// A see-through block, whether blended (water) or cut out (glass). Either way it
    /// hides only faces of its own kind, so there are none inside a body of water or
    /// a wall of glass while what lies behind them is still drawn.
    pub fn is_translucent(self) -> bool {
        matches!(self, Block::Water | Block::Glass)
    }

    /// Whether a **carver** may cut this block away: **stone and soil**, the rock the caves and
    /// ravines are cut through.
    ///
    /// Deliberately a short list, because it is the whole of what a carver is allowed to touch:
    ///
    /// * **Bedrock** is left alone, so the floor of the world stays solid however deep a worm goes.
    /// * **The fluids** are not carved. A cave opening into the sea does not drain it — nothing
    ///   flows in this world — and lava is a *source*, not something to be tunnelled through.
    /// * **Wood and flowers** are not rock. A ravine cuts around an oak rather than through it, and
    ///   a tree whose ground a cave opens keeps standing (see [`Terrain::generate_chunk`]).
    ///
    /// Air is not carveable either, which is why a worm that wanders through open sky costs a few
    /// dice draws and changes nothing.
    ///
    /// [`Terrain::generate_chunk`]: super::Terrain::generate_chunk
    pub fn is_carveable(self) -> bool {
        matches!(self, Block::Stone | Block::Dirt | Block::Grass)
    }

    /// Whether this block needs the alpha-*blended* pass — the one drawn last, back to
    /// front, without writing depth.
    ///
    /// This is Minecraft's **translucent** render type: water and glass. Blending is the
    /// only way a texture can carry alpha of its own — a faintly tinted pane, say —
    /// because cutout can only keep or throw away a whole texel. Water's alpha is 60%,
    /// and glass's is whatever its texture says.
    ///
    /// Oak leaves are the other see-through kind, **cut out**: their texels are only ever
    /// alpha 0 or 1, so the shader's `discard` handles them in the *opaque* pass instead,
    /// which also writes depth — so a leaf canopy occludes what is behind it and needs no
    /// ordering at all. See [`Block::hides_face_of`].
    pub fn is_blended(self) -> bool {
        matches!(self, Block::Water | Block::Glass)
    }

    /// Whether light can spread into this block: air, water, leaves and glass — but
    /// not a full opaque cube.
    pub fn lets_light_through(self) -> bool {
        !self.is_opaque()
    }

    /// Whether this block stops a column's free sunlight, so light can only reach
    /// below it by spreading.
    ///
    /// Nothing you can see straight through from above does — air, glass, a flower —
    /// while everything else does, water and leaves included: that is what makes the sea
    /// dim a level at a time with depth, and a canopy dapple the ground beneath it.
    pub fn blocks_sky(self) -> bool {
        !matches!(
            self,
            Block::Air | Block::Glass | Block::Poppy | Block::Dandelion
        )
    }

    /// Whether `self` hides the face of `other` that touches it.
    ///
    /// An opaque neighbour hides every face. A see-through one hides only faces of its
    /// own kind, so a body of water (or a wall of glass) has no faces between its own
    /// cells while the ground behind it is still drawn. Leaves hide nothing at all:
    /// their texture has holes in it, so a leaf face is kept even against another
    /// leaf, and culling those would let you see straight through the canopy.
    ///
    /// A face can only be hidden when it lies flush against the neighbour, which is only
    /// true of a block that fills its cell — so nothing ever hides the planes of a cross
    /// sprite, and a flower stays visible against the wall beside it.
    pub fn hides_face_of(self, other: Block) -> bool {
        if !other.is_full_cube() {
            return false;
        }
        self.is_opaque() || (self.is_translucent() && self == other)
    }

    /// Whether the player can break this block.
    ///
    /// Bedrock is deliberately unbreakable, so you can't mine through the bottom of
    /// the world. The fluids are never targeted in the first place (see
    /// [`Block::is_targetable`]), so they never get here — and a placed block is how
    /// you get rid of lava, exactly as it is how you clear water.
    pub fn is_breakable(self) -> bool {
        !matches!(self, Block::Air | Block::Bedrock) && !self.is_fluid()
    }

    /// Whether a ray can *hit* this block — see [`crate::world::World::raycast`].
    ///
    /// Everything but the fluids, so glass, leaves, a log and the ground itself can all
    /// be aimed at. A fluid is skipped so that looking at water aims at its bed, and
    /// looking at lava aims at the rock behind it, which is also what lets you place
    /// blocks *into* either.
    pub fn is_targetable(self) -> bool {
        !self.is_air() && !self.is_fluid()
    }

    /// Whether a block placed against this cell replaces it: air and the fluids are free
    /// space, and so is a flower, which a placed block simply clears away. Anything else
    /// is in the way.
    ///
    /// Deliberately *not* the inverse of [`Block::is_targetable`]: a flower is both
    /// aimable and replaceable.
    pub fn is_replaceable(self) -> bool {
        self.is_air() || self.is_fluid() || self.is_cross()
    }

    /// How high this block's top reaches, as a fraction of a full block.
    ///
    /// Almost everything fills its cell exactly. The fluids stop short at
    /// [`FLUID_SURFACE`], so a surface sits a little below the land beside it: a
    /// shoreline reads as a step *down* into the water, and a lava pool as a step down
    /// into the lava.
    pub fn surface_height(self) -> f32 {
        if self.is_fluid() { FLUID_SURFACE } else { 1.0 }
    }

    /// Resolve this block's per-face texture layers from the loaded textures.
    ///
    /// Face order is `[+X, -X, +Y, -Y, +Z, -Z]`. Grass uses a different top
    /// (`grass_block_top`), side (`grass_block_side`) and bottom (`dirt`), and an oak
    /// log is bark on the four sides with rings (`oak_log_top`) on its top *and*
    /// bottom. The rest use one texture on every face.
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
            // Lava is opaque, so it is the one fluid the opaque pass draws. Still lava for
            // now; flowing lava would use `lava_flow`.
            Block::Lava => BlockFaces::uniform(textures.layer("lava_still")),
            Block::Glass => BlockFaces::uniform(textures.layer("glass")),
            Block::OakLog => BlockFaces::column(
                textures.layer("oak_log_top"),
                textures.layer("oak_log"),
                textures.layer("oak_log_top"),
            ),
            Block::OakPlanks => BlockFaces::uniform(textures.layer("oak_planks")),
            Block::OakLeaves => BlockFaces::uniform(textures.layer("oak_leaves")),
            // Flowers are cross sprites: every face is the same texture, and the mesher
            // draws [`mesh::CROSS_PLANES`] instead of a cube.
            Block::Poppy => BlockFaces::uniform(textures.layer("poppy")),
            Block::Dandelion => BlockFaces::uniform(textures.layer("dandelion")),
            // Air is never meshed; layer 0 keeps the match total.
            Block::Air => BlockFaces::uniform(0),
        }
    }

    /// The colour multiplied over this block's textures, as `0xAARRGGBB`.
    ///
    /// Minecraft tints some blocks in code so that a single greyscale texture can
    /// serve several of them. Water's texture is greyscale, so it takes [`WATER_TINT`]
    /// — blue, with the alpha of a blended block — and oak leaves take
    /// [`FOLIAGE_TINT`], green. Everything else is opaque white, which leaves the
    /// sampled texture alone; lava's is already orange, so it needs no tint at all.
    pub fn tint(self) -> u32 {
        match self {
            Block::Water => WATER_TINT,
            Block::OakLeaves => FOLIAGE_TINT,
            _ => mesh::UNTINTED,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_greyscale_textures_are_tinted_in_code() {
        assert_eq!(Block::Water.tint(), WATER_TINT, "water is blue");
        assert_eq!(Block::OakLeaves.tint(), FOLIAGE_TINT, "leaves are green");
        for block in [
            Block::Air,
            Block::Grass,
            Block::Dirt,
            Block::Stone,
            Block::Bedrock,
            Block::Lava,
            Block::Glass,
            Block::OakLog,
            Block::OakPlanks,
            Block::Poppy,
            Block::Dandelion,
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
    fn the_foliage_tint_is_green_rather_than_grey() {
        let [r, g, b] = [
            (FOLIAGE_TINT >> 16) & 0xFF,
            (FOLIAGE_TINT >> 8) & 0xFF,
            FOLIAGE_TINT & 0xFF,
        ];
        assert!(g > r && g > b, "green should dominate: #{FOLIAGE_TINT:06X}");
        assert_eq!(FOLIAGE_TINT >> 24, 0xFF, "leaves are cut out, not blended");
    }

    #[test]
    fn see_through_blocks_are_water_glass_and_leaves() {
        // Water blends: its alpha is a real 60%.
        assert!(Block::Water.is_translucent());
        assert!(Block::Water.is_blended(), "water is the blended pass");
        assert!(!Block::Water.is_opaque());
        assert!(Block::Water.lets_light_through(), "light enters water");
        assert!(Block::Water.blocks_sky(), "but it stops the free sunlight");

        // Glass is the other translucent block — the same render type as water, as in
        // Minecraft. Its texture is only clear or solid today, but a faintly tinted pane
        // will blend rather than vanish.
        assert!(Block::Glass.is_translucent());
        assert!(
            Block::Glass.is_blended(),
            "glass is the translucent kind too"
        );
        assert!(!Block::Glass.is_opaque());
        assert!(Block::Glass.lets_light_through());
        // Being truly transparent, it does not dim the sun at all.
        assert!(!Block::Glass.blocks_sky(), "a glass roof lights the room");

        // Leaves are the *cutout* kind instead, and shade whatever is under them.
        assert!(!Block::OakLeaves.is_opaque());
        assert!(!Block::OakLeaves.is_translucent());
        assert!(!Block::OakLeaves.is_blended());
        assert!(Block::OakLeaves.lets_light_through());
        assert!(Block::OakLeaves.blocks_sky(), "leaves shade the ground");

        for block in [
            Block::Grass,
            Block::Dirt,
            Block::Stone,
            Block::Bedrock,
            Block::OakLog,
            Block::OakPlanks,
        ] {
            assert!(block.is_opaque(), "{block:?} is opaque");
            assert!(!block.is_translucent());
            assert!(!block.is_blended());
            assert!(!block.lets_light_through());
            assert!(block.blocks_sky());
        }

        assert!(Block::Air.is_air());
        assert!(Block::Air.lets_light_through());
        assert!(!Block::Air.blocks_sky());
    }

    #[test]
    fn a_flower_is_a_light_passing_cross_sprite() {
        for flower in [Block::Poppy, Block::Dandelion] {
            assert!(flower.is_cross(), "{flower:?} is a cross sprite");
            assert!(!flower.is_full_cube(), "and so not a cube");
            assert!(!flower.is_opaque());
            assert!(!flower.is_translucent());
            assert!(!flower.is_blended(), "cut out, not blended");
            assert!(flower.lets_light_through());
            assert!(!flower.blocks_sky(), "the sun goes straight past it");
        }

        // The cross sprites are the only blocks that are not cubes.
        for block in [Block::Grass, Block::Stone, Block::Glass, Block::OakLeaves] {
            assert!(!block.is_cross());
            assert!(block.is_full_cube(), "{block:?} fills its cell");
        }
        assert!(!Block::Air.is_full_cube(), "air has no geometry at all");
    }

    #[test]
    fn a_fluid_sits_lower_than_a_full_block() {
        let surface = Block::Water.surface_height();
        assert!(surface < 1.0, "water stops short of the top: {surface}");
        assert!(surface > 0.5, "but it is still mostly full: {surface}");
        // Lava is a fluid too, so its surface sits at exactly the same height: a lava pool
        // reads as a step down into it, just as a shoreline does.
        assert_eq!(
            Block::Lava.surface_height(),
            surface,
            "lava sits level with it"
        );

        for block in [
            Block::Grass,
            Block::Dirt,
            Block::Stone,
            Block::Bedrock,
            Block::Glass,
            Block::OakLog,
            Block::OakPlanks,
            Block::OakLeaves,
        ] {
            assert_eq!(block.surface_height(), 1.0, "{block:?} fills its cell");
        }
    }

    #[test]
    fn lava_is_the_light_source_and_everything_else_is_dark() {
        assert_eq!(
            Block::Lava.light(),
            light::MAX,
            "lava is as bright as it goes"
        );
        for block in [
            Block::Air,
            Block::Grass,
            Block::Dirt,
            Block::Stone,
            Block::Bedrock,
            Block::Water,
            Block::Glass,
            Block::OakLog,
            Block::OakPlanks,
            Block::OakLeaves,
            Block::Poppy,
            Block::Dandelion,
        ] {
            assert_eq!(block.light(), 0, "{block:?} emits nothing");
        }
    }

    #[test]
    fn lava_is_opaque_and_takes_the_fluids_place_in_the_sandbox() {
        // Opaque: nothing spreads light *into* it, and no faces are drawn inside a pool.
        assert!(Block::Lava.is_opaque());
        assert!(!Block::Lava.lets_light_through());
        assert!(
            Block::Lava.blocks_sky(),
            "an opaque block cuts the sunlight off"
        );
        // It is not see-through, so it is drawn by the opaque pass, like stone.
        assert!(!Block::Lava.is_blended());
        assert!(!Block::Lava.is_translucent());
        assert!(Block::Lava.is_full_cube());
        // A liquid to gameplay: a ray passes through it, and a placed block replaces it.
        assert!(Block::Lava.is_fluid() && Block::Water.is_fluid());
        assert!(!Block::Lava.is_targetable());
        assert!(Block::Lava.is_replaceable());
        assert!(!Block::Lava.is_breakable(), "place a block over it instead");
        for block in [Block::Stone, Block::Glass, Block::Grass] {
            assert!(!block.is_fluid(), "{block:?} is not a liquid");
        }
    }

    #[test]
    fn lava_hides_faces_like_the_opaque_block_it_is() {
        assert!(Block::Lava.hides_face_of(Block::Stone));
        assert!(
            Block::Lava.hides_face_of(Block::Lava),
            "no faces inside a pool"
        );
        assert!(Block::Stone.hides_face_of(Block::Lava));
        assert!(Block::Lava.hides_face_of(Block::Water));
        // Water hides only its own kind, so the lava under a shallow pool is still drawn
        // through the water above it.
        assert!(!Block::Water.hides_face_of(Block::Lava));
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
        // Glass does the same, so a wall of glass has no seam running through it.
        assert!(Block::Glass.hides_face_of(Block::Glass));
        assert!(!Block::Glass.hides_face_of(Block::Stone));
        assert!(Block::Stone.hides_face_of(Block::Glass));
        // Leaves hide nothing: their texture has holes, so the faces must stay or you
        // would see straight through the canopy.
        assert!(!Block::OakLeaves.hides_face_of(Block::OakLeaves));
        assert!(!Block::OakLeaves.hides_face_of(Block::Stone));
        assert!(Block::Stone.hides_face_of(Block::OakLeaves));
        // Air hides nothing.
        assert!(!Block::Air.hides_face_of(Block::Stone));
        assert!(!Block::Air.hides_face_of(Block::Water));
        // Nothing hides a cross sprite's planes — not even the wall it grows against —
        // because they do not lie flush against anything.
        for flower in [Block::Poppy, Block::Dandelion] {
            assert!(
                !Block::Stone.hides_face_of(flower),
                "a wall would erase the flower"
            );
            assert!(!Block::Glass.hides_face_of(flower));
            assert!(!flower.hides_face_of(Block::Stone));
            assert!(
                !flower.hides_face_of(flower),
                "two flowers side by side both stay"
            );
        }
    }

    #[test]
    fn everything_but_the_fluids_and_bedrock_can_be_mined() {
        for block in [
            Block::Grass,
            Block::Dirt,
            Block::Stone,
            Block::OakLog,
            Block::OakPlanks,
            Block::OakLeaves,
            Block::Glass,
            Block::Poppy,
            Block::Dandelion,
        ] {
            assert!(block.is_breakable(), "{block:?} can be mined");
        }
        // The fluids are aimed through rather than at, so they never come up for breaking;
        // a placed block is how you clear either of them.
        for block in [Block::Air, Block::Water, Block::Lava, Block::Bedrock] {
            assert!(!block.is_breakable(), "{block:?} cannot be mined");
        }
    }

    #[test]
    fn a_ray_can_hit_anything_but_the_fluids() {
        for block in [
            Block::Grass,
            Block::Dirt,
            Block::Stone,
            Block::Bedrock,
            Block::Glass,
            Block::OakLog,
            Block::OakPlanks,
            Block::OakLeaves,
        ] {
            assert!(block.is_targetable(), "{block:?} can be aimed at");
            assert!(!block.is_replaceable(), "{block:?} is in the way");
        }

        // Air and the fluids are the free space: aimable at nothing, replaceable by a
        // placed block.
        for block in [Block::Air, Block::Water, Block::Lava] {
            assert!(!block.is_targetable(), "{block:?} is not aimed at");
            assert!(block.is_replaceable(), "{block:?} is free space");
        }

        // A flower is the block those two questions part company on: you can aim at it,
        // *and* a placed block clears it away.
        for flower in [Block::Poppy, Block::Dandelion] {
            assert!(flower.is_targetable(), "{flower:?} can be aimed at");
            assert!(flower.is_replaceable(), "{flower:?} is free space too");
        }
    }
}
