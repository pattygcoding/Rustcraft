//! The things a player can hold.
//!
//! A quick-access slot used to hold a [`Block`] and nothing else, which was enough while every
//! block was placed by hand. **Buckets are the items that break that**: a water bucket *places*
//! water but is not water, and an empty bucket is nothing at all until it has scooped something
//! up. So a slot holds an [`Item`] — either a block, which is drawn as a little cube and placed
//! as a block, or a bucket, which is drawn as a flat sprite and *used* on the world.
//!
//! The item is also where the two halves of the bucket rule meet: a full bucket knows the fluid
//! it pours, and [`Item::bucket_of`] is the fluid-to-bucket mapping the other way round, so the
//! two can never disagree about which bucket holds what.

use crate::world::Block;

/// Something a quick-access slot can hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    /// A block: placed as one, and drawn as a little cube.
    Block(Block),
    /// An empty bucket: use it on water or lava to fill it.
    Bucket,
    /// A bucket of water: use it to pour water out, leaving an empty bucket behind.
    WaterBucket,
    /// A bucket of lava: use it to pour lava out, leaving an empty bucket behind.
    LavaBucket,
}

impl From<Block> for Item {
    fn from(block: Block) -> Self {
        Item::Block(block)
    }
}

impl Item {
    /// The fluid this item **pours**, if it is a full bucket. This is what using it on the world
    /// places, and what the empty bucket it leaves behind used to hold.
    pub fn pours(self) -> Option<Block> {
        match self {
            Item::WaterBucket => Some(Block::Water),
            Item::LavaBucket => Some(Block::Lava),
            _ => None,
        }
    }

    /// The bucket a fluid **fills** — the inverse of [`Item::pours`], and the whole of what an
    /// empty bucket does with what it is held over.
    ///
    /// Every fluid cell is a *source* here, because nothing in this world flows: there is no
    /// half-empty water to refuse, which is why this needs no other test than "is it a fluid".
    pub fn bucket_of(fluid: Block) -> Option<Self> {
        match fluid {
            Block::Water => Some(Item::WaterBucket),
            Block::Lava => Some(Item::LavaBucket),
            _ => None,
        }
    }

    /// The block this item is, if it is one. The rest are used rather than placed.
    pub fn block(self) -> Option<Block> {
        match self {
            Item::Block(block) => Some(block),
            _ => None,
        }
    }

    /// The name of the sprite this item is drawn with, if it is drawn as a *sprite* rather than
    /// as a cube. [`Item::Block`] has none: a block is drawn as its six faces (`Block::faces`),
    /// which is what makes a slot read as a block rather than as a picture of one.
    pub fn sprite(self) -> Option<&'static str> {
        match self {
            Item::Block(_) => None,
            Item::Bucket => Some("bucket"),
            Item::WaterBucket => Some("water_bucket"),
            Item::LavaBucket => Some("lava_bucket"),
        }
    }

    /// The id this item is known by — the block's id for a block, and the sprite's name for a
    /// bucket, so `block.stone` and `item.water_bucket` are the two keys the language file is
    /// asked for (see [`crate::lang`]).
    pub fn key(self) -> &'static str {
        match self {
            Item::Block(block) => block.key(),
            Item::Bucket => "bucket",
            Item::WaterBucket => "water_bucket",
            Item::LavaBucket => "lava_bucket",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bucket rule, both ways round — and the property that matters: a full bucket pours
    /// exactly the fluid an empty one would have filled it from.
    #[test]
    fn a_bucket_pours_the_fluid_it_was_filled_from() {
        for fluid in [Block::Water, Block::Lava] {
            let full = Item::bucket_of(fluid).expect("a fluid has a bucket");
            assert_eq!(full.pours(), Some(fluid), "and pours it back out");
            assert_ne!(full, Item::Bucket, "a full bucket is not an empty one");
        }

        // An empty bucket pours nothing, and a block is not a fluid.
        assert_eq!(Item::Bucket.pours(), None);
        for block in [Block::Stone, Block::Glass, Block::Air, Block::Grass] {
            assert_eq!(
                Item::bucket_of(block),
                None,
                "{block:?} is not something a bucket holds"
            );
        }
    }

    /// A block item is placed; a bucket is drawn as a sprite and used instead. The two are told
    /// apart by these, so a bucket can never be "placed" as a cube and a block can never be
    /// drawn as a sprite.
    #[test]
    fn blocks_are_placed_and_buckets_are_used() {
        assert_eq!(Item::from(Block::Stone).block(), Some(Block::Stone));
        assert_eq!(Item::from(Block::Stone).sprite(), None, "drawn as a cube");

        for (bucket, sprite) in [
            (Item::Bucket, "bucket"),
            (Item::WaterBucket, "water_bucket"),
            (Item::LavaBucket, "lava_bucket"),
        ] {
            assert_eq!(bucket.block(), None, "a bucket is not a block");
            assert_eq!(bucket.sprite(), Some(sprite), "and is drawn as its sprite");
            assert_eq!(bucket.pours().is_some(), bucket != Item::Bucket);
        }
    }

    /// Every item has an id, and it is the one the language keys are built from: a block item
    /// borrows its block's, and a bucket is named after the sprite it is drawn with.
    #[test]
    fn every_item_has_an_id() {
        for block in Block::ALL {
            assert_eq!(
                Item::from(block).key(),
                block.key(),
                "{block:?} keeps its own id"
            );
        }
        for item in [Item::Bucket, Item::WaterBucket, Item::LavaBucket] {
            assert_eq!(item.key(), item.sprite().unwrap(), "{item:?}");
        }
    }

    /// Every sprite an item names is a texture that really exists.
    ///
    /// A block's texture is looked up the moment the world is drawn, so a renamed file fails
    /// loudly at once — but a **bucket is only drawn while the creative screen is open**, so a
    /// typo here would sit undetected until someone pressed `E`. This is the test that would
    /// catch it: the names the items ask for, against the files in `assets/textures/items`.
    #[test]
    fn every_item_sprite_is_a_texture_on_disk() {
        let dir = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/resources/assets/textures/items"
        ));
        for item in [Item::Bucket, Item::WaterBucket, Item::LavaBucket] {
            let name = item.sprite().expect("every bucket is drawn as a sprite");
            let path = dir.join(format!("{name}.png"));
            assert!(
                path.is_file(),
                "{} is missing, so the HUD could not draw {item:?}",
                path.display()
            );
        }
    }
}
