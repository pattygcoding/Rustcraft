//! The block hotbar: which blocks you can place, and which one is selected.
//!
//! Scroll the mouse wheel to change the selection. The renderer draws a HUD row of
//! block icons (see [`Hotbar::mesh_data`]) with the selected slot enlarged, so you
//! can always see what you are about to place.

use crate::gfx::mesh::{MeshData, Vertex};
use crate::gfx::texture::BlockTextures;
use crate::world::Block;

/// Height of a hotbar slot, in NDC units.
const SLOT_HEIGHT: f32 = 0.16;
/// Gap between slot centres, as a multiple of [`SLOT_HEIGHT`].
const SLOT_SPACING: f32 = 1.15;
/// How much bigger the selected slot is drawn.
const SELECTED_SCALE: f32 = 1.25;
/// Distance of the slot row from the bottom of the screen, in NDC units.
const BOTTOM_MARGIN: f32 = 0.06;

/// The row of placeable blocks, with one selected.
pub struct Hotbar {
    blocks: Vec<Block>,
    selected: usize,
}

impl Default for Hotbar {
    fn default() -> Self {
        Self::new()
    }
}

impl Hotbar {
    /// The default hotbar (one slot per placeable block).
    pub fn new() -> Self {
        Self {
            blocks: vec![Block::Grass, Block::Dirt, Block::Stone, Block::Bedrock],
            selected: 0,
        }
    }

    /// The block that would be placed right now.
    pub fn selected(&self) -> Block {
        self.blocks[self.selected]
    }

    /// The index of the selected slot (used to detect changes).
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// Move the selection by `delta` slots, wrapping around.
    pub fn scroll(&mut self, delta: i32) {
        let count = self.blocks.len() as i32;
        if count > 0 {
            self.selected = (self.selected as i32 + delta).rem_euclid(count) as usize;
        }
    }

    /// Build the HUD geometry (already in clip space) for the given aspect ratio.
    pub fn mesh_data(&self, textures: &BlockTextures, aspect: f32) -> MeshData {
        let mut data = MeshData::default();
        let count = self.blocks.len() as f32;
        if count == 0.0 {
            return data;
        }

        // Slots are square on screen: NDC x is `aspect` times narrower than y.
        let half_height = SLOT_HEIGHT * 0.5;
        let half_width = half_height / aspect;
        let spacing = SLOT_HEIGHT * SLOT_SPACING;
        let row_width = spacing * count;
        let y = -1.0 + half_height + BOTTOM_MARGIN;

        for (i, &block) in self.blocks.iter().enumerate() {
            let x = -row_width * 0.5 + spacing * (i as f32 + 0.5);
            let scale = if i == self.selected {
                SELECTED_SCALE
            } else {
                1.0
            };
            // Use the block's top texture as its icon.
            let layer = block.faces(textures).layers[2];
            push_icon(
                &mut data,
                [x, y],
                [half_width * scale, half_height * scale],
                layer,
            );
        }

        data.opaque_index_count = data.indices.len() as u32;
        data
    }
}

/// Append a textured screen-space quad centred at `center` (in NDC).
fn push_icon(data: &mut MeshData, center: [f32; 2], half: [f32; 2], layer: u32) {
    let base = data.vertices.len() as u32;
    // (corner offset, uv) — top-left first so the icon is upright.
    let corners = [
        ([-1.0, 1.0], [0.0, 0.0]),
        ([1.0, 1.0], [1.0, 0.0]),
        ([1.0, -1.0], [1.0, 1.0]),
        ([-1.0, -1.0], [0.0, 1.0]),
    ];
    for (corner, uv) in corners {
        data.vertices.push(Vertex {
            position: [
                center[0] + corner[0] * half[0],
                center[1] + corner[1] * half[1],
                0.0,
            ],
            uv,
            layer,
        });
    }
    data.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_cycles_and_wraps() {
        let mut hotbar = Hotbar::new();
        let first = hotbar.selected();

        // Scrolling back wraps to the last slot, and forward returns.
        hotbar.scroll(-1);
        assert_ne!(hotbar.selected(), first);
        hotbar.scroll(1);
        assert_eq!(hotbar.selected(), first);

        // A full loop comes back to the same block.
        let count = hotbar.blocks.len() as i32;
        hotbar.scroll(count + 2);
        hotbar.scroll(-2);
        assert_eq!(hotbar.selected(), first);
    }
}
