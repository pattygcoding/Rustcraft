//! The player's inventory: nine quick-access slots, and the creative screen (`E`) that fills
//! them.
//!
//! A slot draws its block as a small 3D cube — the top and two sides, projected flat by
//! [`BlockIcon`] — except for cross sprites such as flowers, which are drawn as the flat
//! sprite they actually are. One [`Layout`] places the slots *and* answers what the cursor is
//! over, so what you see and what you click can never disagree. The screen also draws the
//! **crosshair** marking the middle of the view — which, since the raycast runs straight down
//! the view axis, is exactly the point a click acts on.

use crate::gfx::mesh::{
    self, BlockIcon, FULL_LIGHT, FaceStyle, GeometryData, MeshData, NO_TEXTURE,
};
use crate::gfx::texture::BlockTextures;
use crate::world::Block;

/// How many quick-access slots the player has, picked with the 1–9 keys.
pub const SLOTS: usize = 9;

/// What the slots hold before you change anything.
const DEFAULT_SLOTS: [Option<Block>; SLOTS] = [
    Some(Block::Grass),
    Some(Block::Dirt),
    Some(Block::Stone),
    Some(Block::OakLog),
    Some(Block::OakPlanks),
    Some(Block::OakLeaves),
    Some(Block::Poppy),
    Some(Block::Dandelion),
    Some(Block::Glass),
];

/// Every block the creative screen offers, in the order it shows them.
const PALETTE: [Block; 12] = [
    Block::Grass,
    Block::Dirt,
    Block::Stone,
    Block::OakLog,
    Block::OakPlanks,
    Block::OakLeaves,
    Block::Poppy,
    Block::Dandelion,
    Block::Glass,
    Block::Lava,
    Block::Water,
    Block::Bedrock,
];

/// A cell's height in NDC, and the gap between cells as a multiple of it.
const CELL: f32 = 0.16;
/// How far apart cell centres are, as a multiple of [`CELL`].
const SPACING: f32 = 1.06;
/// How far the quick-access row sits above the bottom of the screen, in NDC.
const BOTTOM_MARGIN: f32 = 0.06;
/// Cells per palette row.
const COLUMNS: usize = 4;
/// How much of a cell a block icon may fill.
const ICON: f32 = 0.8;
/// How thick the highlight around the hovered cell is, in NDC.
const OUTLINE: f32 = 0.01;

/// UI colours, packed `0xAARRGGBB` exactly as `Block::tint` packs block colours.
const PANEL_COLOR: u32 = 0xC8_10_10_10;
const CELL_COLOR: u32 = 0x38_FF_FF_FF;
const SELECTED_COLOR: u32 = 0x70_FF_FF_FF;
const OUTLINE_COLOR: u32 = 0xF0_FF_FF_FF;

/// The crosshair in the middle of the screen: how far its arms reach from the centre, the
/// hole they leave there, and how thick they are. All three are *vertical* NDC measures, like
/// [`OUTLINE`], so every horizontal one is divided by the aspect ratio to stay square on
/// screen. [`CROSSHAIR_EDGE`] is how far the dark underlay juts out past the white cross drawn
/// on top of it.
///
/// These are sizes in NDC rather than pixels, so the crosshair grows with the window the same
/// way the rest of the HUD does. At a 1920×1080 window that is an 19-pixel cross, a 2-pixel
/// line — thin enough to aim with, thick enough that no bar falls between pixel centres and
/// disappears, which a sub-pixel one does.
const CROSSHAIR_ARM: f32 = 0.022;
const CROSSHAIR_GAP: f32 = 0.007;
const CROSSHAIR_THICK: f32 = 0.005;
const CROSSHAIR_EDGE: f32 = 0.0016;

/// Nearly-opaque white over a half-black underlay: the white reads against dark stone, the
/// underlay against a bright sky or a sunlit beach.
const CROSSHAIR_COLOR: u32 = 0xE6_FF_FF_FF;
const CROSSHAIR_EDGE_COLOR: u32 = 0x66_00_00_00;

/// The four corners of a unit square, in `[top-left, top-right, bottom-right, bottom-left]`
/// order — what a plain UI quad is made of.
const UNIT_SQUARE: [([f32; 2], [f32; 2]); 4] = [
    ([-1.0, 1.0], [0.0, 0.0]),
    ([1.0, 1.0], [1.0, 0.0]),
    ([1.0, -1.0], [1.0, 1.0]),
    ([-1.0, -1.0], [0.0, 1.0]),
];

/// The player's items, and the screen that fills them.
pub struct Inventory {
    slots: [Option<Block>; SLOTS],
    selected: usize,
    open: bool,
    /// The block riding the cursor, picked up from the palette.
    carried: Option<Block>,
    /// Cursor position in NDC. Only meaningful while the screen is open.
    cursor: [f32; 2],
    /// The palette cell under the cursor, if any.
    hovered: Option<usize>,
}

impl Default for Inventory {
    fn default() -> Self {
        Self::new()
    }
}

impl Inventory {
    /// A fresh inventory: the default slots, nothing carried, the screen closed.
    pub fn new() -> Self {
        Self {
            slots: DEFAULT_SLOTS,
            selected: 0,
            open: false,
            carried: None,
            cursor: [0.0, 0.0],
            hovered: None,
        }
    }

    /// Is the creative screen open?
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open or close the creative screen. Closing drops whatever the cursor was carrying.
    pub fn set_open(&mut self, open: bool) {
        self.open = open;
        if !open {
            self.carried = None;
            self.hovered = None;
        }
    }

    /// The block that would be placed right now, if the selected slot holds one.
    pub fn selected(&self) -> Option<Block> {
        self.slots[self.selected]
    }

    /// The index of the selected slot.
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// The cursor position, quantised to whole steps. The renderer compares this to decide
    /// whether to rebuild the HUD, and floats cannot be compared for equality.
    pub fn cursor_key(&self) -> [i32; 2] {
        [
            (self.cursor[0] * 1024.0) as i32,
            (self.cursor[1] * 1024.0) as i32,
        ]
    }

    /// Move the selection by `delta` slots, wrapping around.
    pub fn scroll(&mut self, delta: i32) {
        self.selected = (self.selected as i32 + delta).rem_euclid(SLOTS as i32) as usize;
    }

    /// Point the cursor at `point` (NDC) and note which cell it is over.
    ///
    /// `aspect` must be the viewport aspect the drawing uses, or the highlight would land a
    /// cell off.
    pub fn set_cursor(&mut self, point: [f32; 2], aspect: f32) {
        self.cursor = point;
        self.hovered = if self.open {
            Layout::new(aspect, PALETTE.len()).palette_at(point)
        } else {
            None
        };
    }

    /// The 1–9 keys. In the world they choose a slot; while the screen is open they put the
    /// block under the cursor into that slot, the way Minecraft does. `key` is 1-based.
    pub fn number_key(&mut self, key: usize) {
        let Some(index) = key.checked_sub(1).filter(|index| *index < SLOTS) else {
            return;
        };
        match (self.open, self.hovered) {
            (true, Some(hovered)) => self.slots[index] = Some(PALETTE[hovered]),
            _ => self.selected = index,
        }
    }

    /// Handle a click at `point` (NDC): pick a block up off the palette, or drop the one
    /// being carried into a slot.
    pub fn click(&mut self, point: [f32; 2], aspect: f32) {
        if !self.open {
            return;
        }
        let layout = Layout::new(aspect, PALETTE.len());
        if let Some(index) = layout.palette_at(point) {
            // Picking a block up; doing it again with one in hand swaps what you hold.
            self.carried = Some(PALETTE[index]);
        } else if let Some(index) = layout.slot_at(point) {
            match self.carried.take() {
                Some(block) => self.slots[index] = Some(block),
                None => self.selected = index,
            }
        }
    }

    /// Build the HUD geometry (already in clip space) for the given aspect ratio.
    pub fn mesh_data(&self, textures: &BlockTextures, aspect: f32) -> MeshData {
        let mut data = MeshData::default();
        let layout = Layout::new(aspect, PALETTE.len());

        if self.open {
            push_rect(&mut data.opaque, layout.panel(), PANEL_COLOR);
            for (index, &block) in PALETTE.iter().enumerate() {
                let cell = layout
                    .palette_rect(index)
                    .expect("a rect per palette entry");
                push_rect(&mut data.opaque, cell, CELL_COLOR);
                push_block(&mut data.opaque, textures, block, cell);
            }
            if let Some(hovered) = self.hovered {
                let cell = layout
                    .palette_rect(hovered)
                    .expect("hovered is a palette entry");
                push_outline(&mut data.opaque, cell, OUTLINE_COLOR, aspect);
            }
        }

        // The quick-access row is on screen whether or not the screen is open.
        for index in 0..SLOTS {
            let cell = layout.slot_rect(index);
            let color = if index == self.selected {
                SELECTED_COLOR
            } else {
                CELL_COLOR
            };
            push_rect(&mut data.opaque, cell, color);
            if let Some(block) = self.slots[index] {
                push_block(&mut data.opaque, textures, block, cell);
            }
        }

        // The crosshair marks the centre of the view, and so the target of the raycast a click
        // runs: always on screen in the world, and hidden while the creative screen is open,
        // since the cursor is free then and nothing is being aimed at.
        if !self.open {
            push_crosshair(&mut data.opaque, aspect);
        }

        // Whatever is riding the cursor, drawn last so it sits on top of everything.
        if let Some(block) = self.carried {
            let cell = Rect::new(self.cursor, [CELL / aspect, CELL * 0.5]);
            push_block(&mut data.opaque, textures, block, cell);
        }

        data
    }
}

/// Where the inventory sits on screen, in NDC.
///
/// Drawing and hit-testing both read this, so a cell is always clickable exactly where it was
/// drawn.
#[derive(Clone, Debug)]
pub struct Layout {
    panel: Rect,
    palette: Vec<Rect>,
    slots: [Rect; SLOTS],
}

/// A rectangle in NDC: its centre and half-size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    /// Centre of the rectangle.
    pub centre: [f32; 2],
    /// Half-width and half-height.
    pub half: [f32; 2],
}

impl Rect {
    /// A rectangle from its centre and half-size.
    pub const fn new(centre: [f32; 2], half: [f32; 2]) -> Self {
        Self { centre, half }
    }

    /// Is `point` inside?
    pub fn contains(&self, point: [f32; 2]) -> bool {
        (point[0] - self.centre[0]).abs() <= self.half[0]
            && (point[1] - self.centre[1]).abs() <= self.half[1]
    }
}

impl Layout {
    /// Lay the inventory out for a viewport of `aspect`, offering `count` palette entries.
    ///
    /// Cells are square *on screen*, so a cell's width in NDC is its height divided by the
    /// aspect ratio — the trick the quick-access row has always used.
    pub fn new(aspect: f32, count: usize) -> Self {
        let half = [CELL * 0.5 / aspect, CELL * 0.5];
        let step = [CELL * SPACING / aspect, CELL * SPACING];
        // Where cell `index` of a row of `total` goes, measured from the middle of the row.
        let across =
            |index: usize, total: usize| step[0] * (index as f32 + 0.5 - total as f32 * 0.5);

        let row_y = -1.0 + half[1] + BOTTOM_MARGIN;
        let slots = std::array::from_fn(|index| Rect::new([across(index, SLOTS), row_y], half));

        // The palette fills upwards from one step above the row, left to right.
        let rows = count.div_ceil(COLUMNS).max(1);
        let palette_bottom = row_y + step[1];
        let palette: Vec<Rect> = (0..count)
            .map(|index| {
                let (column, row) = (index % COLUMNS, index / COLUMNS);
                let y = palette_bottom + step[1] * (rows - 1 - row) as f32;
                Rect::new([across(column, COLUMNS), y], half)
            })
            .collect();

        // The panel covers whichever row is wider, with a little air all around.
        let width = half[0] + step[0] * COLUMNS.max(SLOTS) as f32 * 0.5;
        let bottom = row_y - half[1] * 1.2;
        let top = palette_bottom + step[1] * (rows - 1) as f32 + half[1] * 1.2;
        let panel = Rect::new([0.0, (bottom + top) * 0.5], [width, (top - bottom) * 0.5]);

        Self {
            panel,
            palette,
            slots,
        }
    }

    /// The panel behind everything.
    pub fn panel(&self) -> Rect {
        self.panel
    }

    /// The box palette entry `index` occupies.
    pub fn palette_rect(&self, index: usize) -> Option<Rect> {
        self.palette.get(index).copied()
    }

    /// The box quick-access slot `index` occupies.
    pub fn slot_rect(&self, index: usize) -> Rect {
        self.slots[index.min(SLOTS - 1)]
    }

    /// Which palette entry is under `point`, if any.
    pub fn palette_at(&self, point: [f32; 2]) -> Option<usize> {
        self.palette.iter().position(|rect| rect.contains(point))
    }

    /// Which quick-access slot is under `point`, if any.
    pub fn slot_at(&self, point: [f32; 2]) -> Option<usize> {
        self.slots.iter().position(|rect| rect.contains(point))
    }
}

/// Append a solid rectangle: no texture at all, just the colour.
fn push_rect(geometry: &mut GeometryData, rect: Rect, color: u32) {
    let style = FaceStyle {
        layer: NO_TEXTURE,
        light: [FULL_LIGHT; 4],
        tint: color,
        height: 1.0,
    };
    mesh::push_screen_plane(
        &mut geometry.vertices,
        &mut geometry.indices,
        rect.centre,
        rect.half,
        UNIT_SQUARE,
        style,
    );
}

/// Append the highlight around `rect`: four thin bars. The sides are thinner in NDC than the
/// top and bottom, because the viewport is wider than it is tall.
fn push_outline(geometry: &mut GeometryData, rect: Rect, color: u32, aspect: f32) {
    let thin = [OUTLINE * 0.5 / aspect, OUTLINE * 0.5];
    let (x, y) = (rect.half[0], rect.half[1]);
    for (offset, half) in [
        ([0.0, y], [x + thin[0], thin[1]]),
        ([0.0, -y], [x + thin[0], thin[1]]),
        ([-x, 0.0], [thin[0], y + thin[1]]),
        ([x, 0.0], [thin[0], y + thin[1]]),
    ] {
        let centre = [rect.centre[0] + offset[0], rect.centre[1] + offset[1]];
        push_rect(geometry, Rect::new(centre, half), color);
    }
}

/// The four bars of a crosshair centred on the screen: up, down, right, left. Each starts
/// `gap` out from the centre and reaches `arm`, and is `thick` wide — all given in vertical
/// NDC, so the horizontal measures are divided by `aspect` and the cross comes out square on
/// screen however wide the window is.
///
/// The middle of the cross is deliberately left empty: that is the pixel being aimed at, and
/// covering it would hide the very thing the crosshair is there to show.
fn crosshair_bars(aspect: f32, arm: f32, gap: f32, thick: f32) -> [Rect; 4] {
    let half_length = (arm - gap) * 0.5;
    let offset = (arm + gap) * 0.5;
    let (half_thin_x, half_thin_y) = (thick * 0.5 / aspect, thick * 0.5);
    [
        Rect::new([0.0, offset], [half_thin_x, half_length]),
        Rect::new([0.0, -offset], [half_thin_x, half_length]),
        Rect::new([offset / aspect, 0.0], [half_length / aspect, half_thin_y]),
        Rect::new([-offset / aspect, 0.0], [half_length / aspect, half_thin_y]),
    ]
}

/// Append the crosshair: a dark cross with the white cross drawn over it.
///
/// The dark one is a little longer and thicker, so a hairline of dark shows all round the
/// white — which is what keeps a white crosshair legible against a bright sky or sunlit sand.
/// It goes down first, so the white wins the middle of every bar. Both layers leave the same
/// hole in the middle, so the pixel being aimed at stays clear.
fn push_crosshair(geometry: &mut GeometryData, aspect: f32) {
    let layers = [
        // The underlay, a hair larger all round...
        (
            CROSSHAIR_ARM + CROSSHAIR_EDGE,
            CROSSHAIR_GAP,
            CROSSHAIR_THICK + CROSSHAIR_EDGE * 2.0,
            CROSSHAIR_EDGE_COLOR,
        ),
        // ...and the white cross on top of it.
        (
            CROSSHAIR_ARM,
            CROSSHAIR_GAP,
            CROSSHAIR_THICK,
            CROSSHAIR_COLOR,
        ),
    ];
    for (arm, gap, thick, color) in layers {
        for bar in crosshair_bars(aspect, arm, gap, thick) {
            push_rect(geometry, bar, color);
        }
    }
}

/// Append `block`'s icon, filling [`ICON`] of `slot`.
///
/// A cube gets a small 3D cube — its top and two sides, each shaded by which side it is, just
/// as in the world. A cross sprite (a flower) gets its flat texture instead, because that is
/// what it is.
fn push_block(geometry: &mut GeometryData, textures: &BlockTextures, block: Block, slot: Rect) {
    let faces = block.faces(textures);
    let tint = block.tint();
    let centre = slot.centre;
    let half = [slot.half[0] * ICON, slot.half[1] * ICON];

    if block.is_cross() {
        let style = FaceStyle {
            layer: faces.layers[2],
            light: [FULL_LIGHT; 4],
            tint,
            height: 1.0,
        };
        mesh::push_screen_plane(
            &mut geometry.vertices,
            &mut geometry.indices,
            centre,
            half,
            UNIT_SQUARE,
            style,
        );
        return;
    }

    // The icon is 1.0 wide and `height` tall, so scale it to fit the box while keeping that
    // shape.
    let icon = BlockIcon::new();
    let scale = (2.0 * half[0]).min(2.0 * half[1] / icon.height);
    for (face, corners) in icon.faces {
        let style = FaceStyle {
            layer: faces.layers[face],
            // Full daylight carrying *this face's* index: the HUD shader turns that into the
            // same directional shading the world gives that side of a block. All four corners
            // alike, since an icon is not lit — only shaded by which way it faces.
            light: [mesh::pack_light(255, 0, face); 4],
            tint,
            height: 1.0,
        };
        mesh::push_screen_plane(
            &mut geometry.vertices,
            &mut geometry.indices,
            centre,
            [scale * 0.5, scale * 0.5],
            corners,
            style,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_number_keys_choose_a_slot_in_the_world() {
        let mut inventory = Inventory::new();
        inventory.number_key(3);
        assert_eq!(inventory.selected_index(), 2, "the third slot");

        // Scrolling wraps, both ways.
        inventory.number_key(9);
        inventory.scroll(1);
        assert_eq!(inventory.selected_index(), 0);
        inventory.scroll(-1);
        assert_eq!(inventory.selected_index(), SLOTS - 1);

        // Out-of-range keys are ignored.
        inventory.number_key(0);
        inventory.number_key(10);
        assert_eq!(inventory.selected_index(), SLOTS - 1);
    }

    #[test]
    fn the_number_keys_stock_a_slot_from_the_palette() {
        let mut inventory = Inventory::new();
        inventory.set_open(true);
        let layout = Layout::new(1.0, PALETTE.len());

        // Hover the water entry (second from the end) and press 1.
        let water = layout.palette_rect(PALETTE.len() - 2).unwrap();
        inventory.set_cursor(water.centre, 1.0);
        inventory.number_key(1);

        assert_eq!(inventory.selected(), Some(Block::Water));
    }

    #[test]
    fn clicking_picks_a_block_up_and_drops_it_into_a_slot() {
        let mut inventory = Inventory::new();
        inventory.set_open(true);
        let layout = Layout::new(1.0, PALETTE.len());

        // Pick bedrock up off the palette...
        let bedrock = layout.palette_rect(PALETTE.len() - 1).unwrap();
        inventory.click(bedrock.centre, 1.0);
        // ...and drop it into the fifth slot.
        inventory.click(layout.slot_rect(4).centre, 1.0);
        inventory.number_key(5);

        assert_eq!(inventory.selected(), Some(Block::Bedrock));
    }

    #[test]
    fn the_crosshair_leaves_the_centre_of_the_screen_clear() {
        let aspect = 16.0 / 9.0;
        for bar in crosshair_bars(aspect, CROSSHAIR_ARM, CROSSHAIR_GAP, CROSSHAIR_THICK) {
            // No bar covers the point being aimed at, and none runs off screen.
            assert!(
                !bar.contains([0.0, 0.0]),
                "a crosshair bar covers the centre: {bar:?}"
            );
            assert!(
                bar.centre[0].abs() + bar.half[0] <= 1.0
                    && bar.centre[1].abs() + bar.half[1] <= 1.0,
                "a crosshair bar runs off screen: {bar:?}"
            );
        }
    }

    #[test]
    fn the_crosshair_cross_is_square_on_screen() {
        // The arms are as long as they are wide, in pixels rather than in NDC, at any aspect.
        for aspect in [1.0, 4.0 / 3.0, 16.0 / 9.0, 21.0 / 9.0] {
            let [up, down, right, left] =
                crosshair_bars(aspect, CROSSHAIR_ARM, CROSSHAIR_GAP, CROSSHAIR_THICK);
            let size = |rect: Rect| (rect.half[0] * 2.0 * aspect, rect.half[1] * 2.0);
            let (up_w, up_h) = size(up);
            let (right_w, right_h) = size(right);
            assert!(
                (up_w - right_h).abs() < 1e-6,
                "thicknesses differ at {aspect}"
            );
            assert!((up_h - right_w).abs() < 1e-6, "lengths differ at {aspect}");
            assert_eq!(down.centre[1], -up.centre[1], "down mirrors up");
            assert_eq!(left.centre[0], -right.centre[0], "left mirrors right");
        }
    }

    #[test]
    fn the_crosshair_is_big_enough_to_see() {
        // A bar thinner than a pixel can fall between pixel centres and vanish entirely — which
        // is exactly what the first, thinner crosshair did, and why this test exists. One NDC
        // unit is half the viewport, so a bar's size in pixels follows from the aspect ratio.
        let (width, height) = (1920.0, 1080.0);
        let bars = crosshair_bars(
            width / height,
            CROSSHAIR_ARM,
            CROSSHAIR_GAP,
            CROSSHAIR_THICK,
        );
        let mut span = 0.0_f32;
        for bar in bars {
            let (w, h) = (bar.half[0] * 2.0 * width, bar.half[1] * 2.0 * height);
            let (thin, long) = if w < h { (w, h) } else { (h, w) };
            assert!(
                thin >= 1.5,
                "a bar is a sub-pixel {thin} wide and could vanish"
            );
            assert!(
                long >= 6.0,
                "a bar is only {long} pixels long — too short to aim with"
            );
            span = span.max(w.max(h));
        }
        assert!(span >= 12.0, "the crosshair is only {span} pixels across");
    }

    #[test]
    fn the_crosshair_puts_the_white_cross_over_the_dark_one() {
        let mut geometry = GeometryData::default();
        push_crosshair(&mut geometry, 16.0 / 9.0);
        assert_eq!(geometry.vertices.len(), 32, "four bars, twice over");
        // The underlay is drawn first and the white second, so the white wins where they meet.
        assert_eq!(geometry.vertices[0].tint, CROSSHAIR_EDGE_COLOR);
        assert_eq!(geometry.vertices.last().unwrap().tint, CROSSHAIR_COLOR);
    }

    #[test]
    fn closing_the_screen_drops_what_the_cursor_carried() {
        let mut inventory = Inventory::new();
        inventory.set_open(true);
        let layout = Layout::new(1.0, PALETTE.len());
        inventory.click(layout.palette_rect(0).unwrap().centre, 1.0);
        assert!(inventory.carried.is_some(), "picked a block up");

        inventory.set_open(false);
        assert!(inventory.carried.is_none());
        assert!(!inventory.is_open());
    }

    #[test]
    fn the_layout_is_shared_by_drawing_and_hit_testing() {
        let layout = Layout::new(16.0 / 9.0, PALETTE.len());
        for index in 0..PALETTE.len() {
            let cell = layout.palette_rect(index).unwrap();
            assert_eq!(layout.palette_at(cell.centre), Some(index));
            assert!(
                layout.panel().contains(cell.centre),
                "palette cell {index} is on the panel"
            );
        }
        for index in 0..SLOTS {
            let cell = layout.slot_rect(index);
            assert_eq!(layout.slot_at(cell.centre), Some(index));
            assert!(
                layout.panel().contains(cell.centre),
                "slot {index} is on the panel"
            );
        }

        // A point clear of every cell hits nothing.
        let cell = layout.palette_rect(0).unwrap();
        let away = [cell.centre[0] - cell.half[0] * 3.0, cell.centre[1]];
        assert_eq!(layout.palette_at(away), None);
    }

    #[test]
    fn nothing_falls_off_the_screen() {
        // Every landscape viewport must fit the panel, every palette cell and every slot
        // inside NDC, whatever the size.
        for aspect in [1.0, 4.0 / 3.0, 16.0 / 9.0, 21.0 / 9.0] {
            let layout = Layout::new(aspect, PALETTE.len());
            let on_screen = |rect: Rect| {
                rect.centre[0].abs() + rect.half[0] <= 1.0
                    && rect.centre[1].abs() + rect.half[1] <= 1.0
            };
            assert!(on_screen(layout.panel()), "the panel runs off at {aspect}");
            for index in 0..PALETTE.len() {
                let cell = layout.palette_rect(index).unwrap();
                assert!(on_screen(cell), "palette cell {index} runs off at {aspect}");
            }
            for index in 0..SLOTS {
                let cell = layout.slot_rect(index);
                assert!(on_screen(cell), "slot {index} runs off at {aspect}");
            }
        }
    }

    #[test]
    fn cells_are_square_on_screen() {
        let aspect = 16.0 / 9.0;
        let layout = Layout::new(aspect, PALETTE.len());
        for rect in [layout.slot_rect(0), layout.palette_rect(0).unwrap()] {
            let (width, height) = (rect.half[0] * 2.0 * aspect, rect.half[1] * 2.0);
            assert!(
                (width - height).abs() < 1e-6,
                "not square: {width} by {height}"
            );
        }
    }
}
