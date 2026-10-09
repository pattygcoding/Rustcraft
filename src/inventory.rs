//! The player's inventory: nine quick-access slots, and the creative screen (`E`) that fills
//! them.
//!
//! A slot draws its block as a small 3D cube — the top and two sides, projected flat by
//! [`BlockIcon`] — except for cross sprites such as flowers, which are drawn as the flat
//! sprite they actually are. One [`Layout`] places the slots *and* answers what the cursor is
//! over, so what you see and what you click can never disagree. The screen also draws the
//! **crosshair** marking the middle of the view — which, since the raycast runs straight down
//! the view axis, is exactly the point a click acts on.

use std::time::{Duration, Instant};

use crate::font;
use crate::gfx::mesh::{self, BlockIcon, FULL_LIGHT, FaceStyle, GeometryData, MeshData};
use crate::gfx::texture::BlockTextures;
use crate::item::Item;
use crate::lang::Lang;
use crate::ui::{UNIT_SQUARE, push_bevel, push_rect};
use crate::world::Block;

// The rectangle the HUD's cells are placed in lives in `ui` because the pause menu places
// rectangles too. Re-exported rather than imported privately, because it is part of this
// module's own API: `Layout` hands one back, and `Inventory::mesh_data` fills them.
pub use crate::ui::Rect;

/// How many quick-access slots the player has, picked with the 1–9 keys.
pub const SLOTS: usize = 9;

/// What the slots hold before you change anything.
const DEFAULT_SLOTS: [Option<Item>; SLOTS] = [
    Some(Item::Block(Block::Grass)),
    Some(Item::Block(Block::Dirt)),
    Some(Item::Block(Block::Stone)),
    Some(Item::Block(Block::OakLog)),
    Some(Item::Block(Block::OakPlanks)),
    Some(Item::Block(Block::OakLeaves)),
    Some(Item::Block(Block::Poppy)),
    Some(Item::Block(Block::Dandelion)),
    Some(Item::Block(Block::Glass)),
];

/// Every item the creative screen offers, in the order it shows them.
///
/// **The fluids are not on it.** Water and lava are not blocks you can hold: they live in the two
/// buckets, and a bucket is the only thing that can put one into the world or take one out of it
/// — which is what the empty bucket is for. See [`crate::item`].
const PALETTE: [Item; 13] = [
    Item::Block(Block::Grass),
    Item::Block(Block::Dirt),
    Item::Block(Block::Stone),
    Item::Block(Block::OakLog),
    Item::Block(Block::OakPlanks),
    Item::Block(Block::OakLeaves),
    Item::Block(Block::Poppy),
    Item::Block(Block::Dandelion),
    Item::Block(Block::Glass),
    Item::WaterBucket,
    Item::LavaBucket,
    Item::Bucket,
    Item::Block(Block::Bedrock),
];

/// A cell's height in NDC, and the gap between cells as a multiple of it.
const CELL: f32 = 0.16;
/// How far apart cell centres are, as a multiple of [`CELL`].
const SPACING: f32 = 1.06;
/// How far the quick-access row sits above the bottom of the screen, in NDC.
///
/// The row does not move when the screen opens — it is the row you were already using — so
/// everything else is laid out *around* it rather than the other way round.
const BOTTOM_MARGIN: f32 = 0.06;
/// Cells per palette row.
///
/// Seven, so the grid fills the card — which is nine quick-access slots wide, because that row is
/// the widest thing on the screen and it does not move — without the palette looking stranded in
/// the middle of it. Thirteen entries then make two rows, with one cell to spare.
const COLUMNS: usize = 7;
/// How much of a cell a block icon may fill.
const ICON: f32 = 0.8;
/// How thick a slot's bevel is, in NDC — and the ring drawn round the cell under the cursor.
const BEVEL: f32 = 0.011;
/// The card's air all round its contents, the gap between the palette and the row below it, and
/// the height of the title bar across its top. Vertical NDC measures, like everything here.
const PADDING: f32 = 0.024;
const SECTION_GAP: f32 = 0.045;
const TITLE_BAR: f32 = 0.10;
/// How tall the title and the tooltip's text are drawn, in NDC — the two sizes of type on this
/// screen. A line is sized by its *height*: the glyph is five by seven, so the width follows.
const TITLE_TEXT: f32 = 0.052;
const TOOLTIP_TEXT: f32 = 0.038;
/// How long the caption above the quick-access row stays up after the thing in hand changes.
///
/// Minecraft's own "you are holding…" caption fades after a moment, and the *moment* is the point:
/// it names what you just picked without staying on screen to clutter it.
const LABEL_TIME: Duration = Duration::from_secs(2);
/// How far above the quick-access row that caption sits, in NDC.
const NOTICE_GAP: f32 = 0.02;
/// The air a tooltip's plate keeps around its line of text.
const TOOLTIP_PAD: f32 = 0.007;

/// What the screen calls itself, across the top of the card.
const TITLE: &str = "CREATIVE";

/// The screen's colours, packed `0xAARRGGBB` exactly as `Block::tint` packs block colours.
///
/// The card is near-opaque charcoal with a lighter rim, and the title bar is a shade lighter
/// again, so the heading reads as a header rather than as more of the panel. A **slot** is
/// *recessed*: its fill is darker than the card around it and its bevel is dark along the top and
/// left where the hollow turns away from the light — which is the whole difference between a grid
/// of slots and a grid of translucent squares. The slot in hand is the same hollow with its bevel
/// lit up.
const PANEL_COLOR: u32 = 0xF2_14_16_1C;
const RIM_DARK: u32 = 0xFF_08_09_0C;
const RIM_LIGHT: u32 = 0xFF_46_50_60;
const TITLE_COLOR: u32 = 0xFF_24_29_36;
const TITLE_INK: u32 = 0xFF_E8_EC_F4;
const SLOT_COLOR: u32 = 0xFF_0B_0C_10;
const SLOT_DARK: u32 = 0xA0_00_00_00;
const SLOT_LIGHT: u32 = 0x50_96_A2_B4;
const SELECTED_COLOR: u32 = 0xFF_3C_48_5A;
const SELECTED_LIGHT: u32 = 0xFF_B0_BE_D0;
/// The ring round the cell under the cursor, and the pale wash inside it.
const HOVER_RING: u32 = 0xE8_FF_FF_FF;
const HOVER_WASH: u32 = 0x1C_FF_FF_FF;
/// The tooltip: a dark plate with light ink.
const TOOLTIP_COLOR: u32 = 0xFA_07_08_0B;
const TOOLTIP_INK: u32 = 0xFF_E8_EC_F4;

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

/// The player's items, and the screen that fills them.
pub struct Inventory {
    slots: [Option<Item>; SLOTS],
    selected: usize,
    open: bool,
    /// The item riding the cursor, picked up from the palette.
    carried: Option<Item>,
    /// Cursor position in NDC. Only meaningful while the screen is open.
    cursor: [f32; 2],
    /// The cell under the cursor, if any.
    hovered: Option<Hovered>,
    /// When the caption above the quick-access row stops being drawn. It is set every time the
    /// thing in hand changes, and is what makes that caption a *notice* rather than a label.
    label_until: Option<Instant>,
}

/// The cell the cursor is over, while the screen is open: a palette entry to take, or one of the
/// quick-access slots to put it in.
///
/// The screen marks either one — with a ring, and with a tooltip that says what it holds — and the
/// `1`–`9` keys read the *palette* half of it, so the two are told apart rather than both being
/// "an index".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hovered {
    Palette(usize),
    Slot(usize),
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
            // Nothing has been picked yet, so nothing is being announced.
            label_until: None,
        }
    }

    /// Note that the thing **in hand** just changed, which is what puts the caption above the
    /// quick-access row up for [`LABEL_TIME`].
    ///
    /// Called from every path that changes it — a slot key, the wheel, a pick, a bucket emptying
    /// itself into the world — and from nowhere else, so the caption means exactly "this is what
    /// you are holding now" and never appears for a change that did not happen.
    fn announce(&mut self) {
        self.label_until = Some(Instant::now() + LABEL_TIME);
    }

    /// Whether the caption is still up: whether what is in hand changed less than [`LABEL_TIME`]
    /// ago.
    pub fn announcing(&self, now: Instant) -> bool {
        self.label_until.is_some_and(|until| now < until)
    }

    /// The caption to draw above the quick-access row at `now`: what is in hand, while it is still
    /// news.
    ///
    /// `None` while the creative screen is open, because the palette sits exactly where the
    /// caption would go — the screen names things with its tooltips instead.
    fn caption<'a>(&'a self, lang: &'a Lang, now: Instant) -> Option<&'a str> {
        if self.open || !self.announcing(now) {
            return None;
        }
        lang.label_of_item(self.selected()?)
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

    /// The item the selected slot holds — the block that would be placed, the bucket that would
    /// be poured or filled, or nothing at all.
    pub fn selected(&self) -> Option<Item> {
        self.slots[self.selected]
    }

    /// Put `item` in the selected slot, whatever was there before.
    ///
    /// What a bucket does to itself: pouring one out leaves an empty bucket in hand, and filling
    /// an empty one leaves the full bucket — both of which are exactly the moment a caption naming
    /// what you are holding is worth showing.
    pub fn set_selected(&mut self, item: Item) {
        self.slots[self.selected] = Some(item);
        self.announce();
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
        self.announce();
    }

    /// Point the cursor at `point` (NDC) and note which cell it is over.
    ///
    /// `aspect` must be the viewport aspect the drawing uses, or the highlight would land a
    /// cell off.
    pub fn set_cursor(&mut self, point: [f32; 2], aspect: f32) {
        self.cursor = point;
        self.hovered = if !self.open {
            None
        } else {
            let layout = Layout::new(aspect, PALETTE.len());
            layout
                .palette_at(point)
                .map(Hovered::Palette)
                .or_else(|| layout.slot_at(point).map(Hovered::Slot))
        };
    }

    /// The 1–9 keys. In the world they choose a slot; while the screen is open they put the
    /// item under the cursor into that slot, the way Minecraft does. `key` is 1-based.
    pub fn number_key(&mut self, key: usize) {
        let Some(index) = key.checked_sub(1).filter(|index| *index < SLOTS) else {
            return;
        };
        match (self.open, self.hovered) {
            (true, Some(Hovered::Palette(hovered))) => {
                self.slots[index] = Some(PALETTE[hovered]);
                // Stocking the slot you are holding changes what you are holding.
                if index == self.selected {
                    self.announce();
                }
            }
            _ => {
                self.selected = index;
                self.announce();
            }
        }
    }

    /// **Pick block**: make `item` the one you are holding, the way Minecraft's pick-block does.
    ///
    /// A slot that already holds it is *selected* — so the quick-access row is a set of things you
    /// have, and reaching for one of them does not disturb what the others hold — and otherwise
    /// the picked item replaces whatever the selected slot had. Either way what is in hand is what
    /// you were looking at, which is the whole of what a pick is for.
    ///
    /// Nothing is consumed: this is a creative inventory, so everything is always in it to be
    /// picked. The caller hands over what a raycast has already found — a block, or the bucket a
    /// fluid comes in.
    pub fn pick_item(&mut self, item: Item) {
        match self.slots.iter().position(|slot| *slot == Some(item)) {
            Some(index) => self.selected = index,
            None => self.slots[self.selected] = Some(item),
        }
        self.announce();
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
                Some(item) => self.slots[index] = Some(item),
                None => self.selected = index,
            }
            self.announce();
        }
    }

    /// Build the HUD geometry (already in clip space) for the given aspect ratio.
    ///
    /// `aiming` says whether the world is being aimed at, which is when the crosshair is drawn.
    /// The screen knows its own state but not what else is standing over the world, so its
    /// caller — which can see every screen at once — is the one that answers.
    ///
    /// `lang` names the things it draws — the tooltip and the caption — and `now` is the clock the
    /// caption is measured against, handed in rather than read here so that the screen stays a
    /// pure function of its arguments (see [`Inventory::caption`]).
    pub fn mesh_data(
        &self,
        textures: &BlockTextures,
        aspect: f32,
        aiming: bool,
        lang: &Lang,
        now: Instant,
    ) -> MeshData {
        let mut data = MeshData::default();
        let layout = Layout::new(aspect, PALETTE.len());
        let geometry = &mut data.opaque;

        if self.open {
            // The card. A near-opaque plate with a bevel rim, a title bar across the top whose
            // own bevel makes it read as a raised header, and the screen's name in it — which is
            // what turns a grid of squares into a *screen*.
            push_bevel(
                geometry,
                layout.panel(),
                aspect,
                BEVEL * 1.4,
                PANEL_COLOR,
                RIM_DARK,
                RIM_LIGHT,
            );
            let title = layout.title();
            push_bevel(
                geometry,
                title,
                aspect,
                BEVEL,
                TITLE_COLOR,
                RIM_DARK,
                RIM_LIGHT,
            );
            font::push_text(geometry, TITLE, title.centre, TITLE_TEXT, aspect, TITLE_INK);

            // Everything on offer, each in its own recessed slot.
            for (index, &item) in PALETTE.iter().enumerate() {
                let cell = layout
                    .palette_rect(index)
                    .expect("a rect per palette entry");
                push_slot(geometry, cell, aspect, false);
                push_item(geometry, textures, item, cell, aspect);
            }
        }

        // The quick-access row is on screen whether or not the screen is open — the same nine
        // slots, in the same place, so opening the screen does not move the thing in your hand.
        for index in 0..SLOTS {
            let cell = layout.slot_rect(index);
            push_slot(geometry, cell, aspect, index == self.selected);
            if let Some(item) = self.slots[index] {
                push_item(geometry, textures, item, cell, aspect);
            }
        }

        // The ring round whatever the cursor is over, drawn after every cell so it sits on top of
        // them, and then that cell's name in a tooltip.
        if let Some(hovered) = self.hovered {
            let cell = match hovered {
                Hovered::Palette(index) => layout.palette_rect(index),
                Hovered::Slot(index) => Some(layout.slot_rect(index)),
            };
            if let Some(cell) = cell {
                push_hover(geometry, cell, aspect);
                if let Some(item) = self.hovered_item() {
                    // The file names it; a missing label falls back to the id, the way Minecraft
                    // prints the raw key rather than nothing at all.
                    let label = lang.label_of_item(item).unwrap_or(item.key());
                    push_tooltip(geometry, self.cursor, label, aspect);
                }
            }
        }

        // In the world, the thing just picked is *named* above the row, and only for a moment —
        // Minecraft's caption, which says what you are holding now and then gets out of the way.
        if let Some(label) = self.caption(lang, now) {
            let row = layout.slot_rect(0);
            let above = row.centre[1] + row.half[1] + NOTICE_GAP;
            push_notice(
                geometry,
                [0.0, above + plate_half(label, aspect)[1]],
                label,
                aspect,
            );
        }

        // The crosshair marks the centre of the view, and so the point a block click acts on:
        // always on screen in the world, and gone the moment a screen stands between the player
        // and it — the cursor is free then, and nothing is being aimed at.
        if aiming {
            push_crosshair(geometry, aspect);
        }

        // Whatever is riding the cursor, drawn last so it sits on top of everything.
        if let Some(item) = self.carried {
            let cell = Rect::new(self.cursor, [CELL / aspect, CELL * 0.5]);
            push_item(geometry, textures, item, cell, aspect);
        }

        data
    }

    /// The item under the cursor, if the cursor is over a cell that holds something — what the
    /// tooltip prints.
    fn hovered_item(&self) -> Option<Item> {
        match self.hovered? {
            Hovered::Palette(index) => PALETTE.get(index).copied(),
            Hovered::Slot(index) => self.slots.get(index).copied().flatten(),
        }
    }
}

/// Where the inventory sits on screen, in NDC.
///
/// Drawing and hit-testing both read this, so a cell is always clickable exactly where it was
/// drawn.
#[derive(Clone, Debug)]
pub struct Layout {
    panel: Rect,
    title: Rect,
    palette: Vec<Rect>,
    slots: [Rect; SLOTS],
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

        // The quick-access row is pinned exactly where it sits in the world; every other part of
        // the screen is laid out *around* it, so opening the screen moves nothing that was
        // already on it.
        let row_y = -1.0 + half[1] + BOTTOM_MARGIN;
        let slots = std::array::from_fn(|index| Rect::new([across(index, SLOTS), row_y], half));

        // The palette fills upwards from a step *and a gap* above the row, left to right.
        let rows = count.div_ceil(COLUMNS).max(1);
        let palette_bottom = row_y + step[1] + SECTION_GAP;
        let palette: Vec<Rect> = (0..count)
            .map(|index| {
                let (column, row) = (index % COLUMNS, index / COLUMNS);
                let y = palette_bottom + step[1] * (rows - 1 - row) as f32;
                Rect::new([across(column, COLUMNS), y], half)
            })
            .collect();

        // The card wraps the lot: a band of padding all round it, and a title bar across the top.
        let content_top = palette_bottom + step[1] * (rows - 1) as f32 + half[1];
        let content_bottom = row_y - half[1];
        let width = half[0] + PADDING / aspect + step[0] * COLUMNS.max(SLOTS) as f32 * 0.5;
        let bottom = content_bottom - PADDING;
        let top = content_top + PADDING + TITLE_BAR;
        let panel = Rect::new([0.0, (bottom + top) * 0.5], [width, (top - bottom) * 0.5]);
        // The bar spans the panel's own width, inset by the same air the cells keep from its edge.
        let bar_half = TITLE_BAR * 0.5;
        let title = Rect::new([0.0, top - bar_half], [width - PADDING / aspect, bar_half]);

        Self {
            panel,
            title,
            palette,
            slots,
        }
    }

    /// The panel behind everything.
    pub fn panel(&self) -> Rect {
        self.panel
    }

    /// The title bar across the top of the panel.
    pub fn title(&self) -> Rect {
        self.title
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

/// Append one slot: the recessed box a cell's icon sits in, lit up if it is the slot in hand.
fn push_slot(geometry: &mut GeometryData, cell: Rect, aspect: f32, selected: bool) {
    let (fill, light) = if selected {
        (SELECTED_COLOR, SELECTED_LIGHT)
    } else {
        (SLOT_COLOR, SLOT_LIGHT)
    };
    push_bevel(geometry, cell, aspect, BEVEL, fill, SLOT_DARK, light);
}

/// Append the ring round the cell under the cursor: a bright border over a pale wash, so the cell
/// lifts out of the grid without hiding what is in it.
fn push_hover(geometry: &mut GeometryData, cell: Rect, aspect: f32) {
    push_bevel(
        geometry, cell, aspect, BEVEL, HOVER_WASH, HOVER_RING, HOVER_RING,
    );
}

/// How big a line of HUD text's plate is: the line, plus a little air all round it.
///
/// The width follows from the *text* — a line is sized by its height, and every horizontal measure
/// is divided by the aspect ratio (see [`crate::font`]) — so the plate always fits what it holds,
/// however long the name.
fn plate_half(text: &str, aspect: f32) -> [f32; 2] {
    [
        font::measure(text, TOOLTIP_TEXT, aspect) * 0.5 + TOOLTIP_PAD / aspect,
        TOOLTIP_TEXT * 0.5 + TOOLTIP_PAD,
    ]
}

/// A plate of that size, centred on `point` and slid back inside the screen if it would overhang —
/// so a cell in the corner, or a long word, still gets a readable label.
fn plate_at(point: [f32; 2], text: &str, aspect: f32) -> Rect {
    let half = plate_half(text, aspect);
    Rect::new(
        [
            point[0].clamp(-1.0 + half[0], 1.0 - half[0]),
            point[1].clamp(-1.0 + half[1], 1.0 - half[1]),
        ],
        half,
    )
}

/// Where a tooltip's plate goes for a cursor at `point`: just below and to the right of it, which
/// is out of the way of what is being pointed at.
fn tooltip_rect(point: [f32; 2], text: &str, aspect: f32) -> Rect {
    let half = plate_half(text, aspect);
    plate_at(
        [point[0] + half[0] * 1.6, point[1] - half[1] * 1.6],
        text,
        aspect,
    )
}

/// Append a line of HUD text on a dark plate of its own size.
fn push_plate(geometry: &mut GeometryData, plate: Rect, text: &str, aspect: f32) {
    push_bevel(
        geometry,
        plate,
        aspect,
        BEVEL * 0.6,
        TOOLTIP_COLOR,
        RIM_DARK,
        RIM_LIGHT,
    );
    font::push_text(
        geometry,
        text,
        plate.centre,
        TOOLTIP_TEXT,
        aspect,
        TOOLTIP_INK,
    );
}

/// Append a tooltip: what is under the cursor, named on a plate beside it.
fn push_tooltip(geometry: &mut GeometryData, cursor: [f32; 2], text: &str, aspect: f32) {
    push_plate(geometry, tooltip_rect(cursor, text, aspect), text, aspect);
}

/// Append the caption naming what is in hand, centred above the quick-access row — Minecraft's
/// "you are holding…", which the screen puts up for a moment whenever that changes.
fn push_notice(geometry: &mut GeometryData, centre: [f32; 2], text: &str, aspect: f32) {
    push_plate(geometry, plate_at(centre, text, aspect), text, aspect);
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

/// The box a block icon may fill, as a half-size in NDC: [`ICON`] of the slot, which is itself
/// square in pixels.
fn icon_box(slot: Rect) -> [f32; 2] {
    [slot.half[0] * ICON, slot.half[1] * ICON]
}

/// The half-size a cube icon's corners are scaled by, so the **whole** block fits its slot.
///
/// [`BlockIcon`]'s corners span ±0.5 across and ±`height`/2 down, and the icon is 1.0 wide and
/// `height` tall in the same units — but a slot is square in **pixels** while the NDC it is
/// drawn in is not, because a measure across the screen is `aspect` times a measure down it.
/// So the two axes take different scales, just enough to keep the cube's shape (the adjustment
/// every square cell and the crosshair's bars make), and the icon is then sized so that its
/// height fills the box. Getting that wrong is what squashed the icons flat: one scale on both
/// axes stretched the cube across the screen and crushed its sides to a sliver, so that only
/// its top face read as a block at all.
fn icon_half(slot: Rect, height: f32, aspect: f32) -> [f32; 2] {
    let bounds = icon_box(slot);
    // Room for the icon across the slot is the box's width; room down it is the box's height
    // divided by the icon's own height and by the aspect ratio, because a vertical NDC unit is
    // `aspect` times as tall in pixels as a horizontal one is wide. Whichever fits first wins.
    let across = (2.0 * bounds[0]).min(2.0 * bounds[1] / (height * aspect));
    [across, across * aspect]
}

/// Append `item`'s icon, filling [`ICON`] of `slot`.
///
/// A **block** gets a 3D cube — its top and two sides, each shaded by which side it is, just as in
/// the world — or, if it is a cross sprite like a flower, its flat texture. An **item** gets that
/// flat sprite too, because that is what it is: a bucket is a picture of a bucket, drawn the same
/// way a flower is, from the layer its [`Item::sprite`] names.
fn push_item(
    geometry: &mut GeometryData,
    textures: &BlockTextures,
    item: Item,
    slot: Rect,
    aspect: f32,
) {
    // A sprite item: one quad, filling the box the cube would have filled.
    if let Some(name) = item.sprite() {
        let style = FaceStyle {
            layer: textures.layer(name),
            light: [FULL_LIGHT; 4],
            tint: mesh::UNTINTED,
            height: 1.0,
        };
        mesh::push_screen_plane(
            &mut geometry.vertices,
            &mut geometry.indices,
            slot.centre,
            icon_box(slot),
            UNIT_SQUARE,
            style,
        );
        return;
    }

    let Some(block) = item.block() else {
        return;
    };
    let faces = block.faces(textures);
    let tint = block.tint();
    let centre = slot.centre;

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
            icon_box(slot),
            UNIT_SQUARE,
            style,
        );
        return;
    }

    let icon = BlockIcon::new();
    let half = icon_half(slot, icon.height, aspect);
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
            half,
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

        // Hover the water-bucket entry and press 1.
        let water = PALETTE
            .iter()
            .position(|item| *item == Item::WaterBucket)
            .expect("the palette offers a water bucket");
        let cell = layout.palette_rect(water).unwrap();
        inventory.set_cursor(cell.centre, 1.0);
        inventory.number_key(1);

        assert_eq!(inventory.selected(), Some(Item::WaterBucket));
    }

    #[test]
    fn clicking_picks_an_item_up_and_drops_it_into_a_slot() {
        let mut inventory = Inventory::new();
        inventory.set_open(true);
        let layout = Layout::new(1.0, PALETTE.len());

        // Pick the empty bucket up off the palette — an *item*, not a block...
        let bucket = PALETTE
            .iter()
            .position(|item| *item == Item::Bucket)
            .expect("the palette offers an empty bucket");
        let cell = layout.palette_rect(bucket).unwrap();
        inventory.click(cell.centre, 1.0);
        // ...and drop it into the fifth slot.
        inventory.click(layout.slot_rect(4).centre, 1.0);
        inventory.number_key(5);

        assert_eq!(inventory.selected(), Some(Item::Bucket));
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
        // Every landscape viewport must fit the panel, the title bar across its top, every
        // palette cell and every slot inside NDC, whatever the size.
        for aspect in [1.0, 4.0 / 3.0, 16.0 / 9.0, 21.0 / 9.0] {
            let layout = Layout::new(aspect, PALETTE.len());
            let on_screen = |rect: Rect| {
                rect.centre[0].abs() + rect.half[0] <= 1.0
                    && rect.centre[1].abs() + rect.half[1] <= 1.0
            };
            assert!(on_screen(layout.panel()), "the panel runs off at {aspect}");
            assert!(
                on_screen(layout.title()),
                "the title bar runs off at {aspect}"
            );
            // ...and the heading really does fit inside the card it is written on.
            assert!(
                layout.panel().contains(layout.title().centre),
                "the title bar is off the panel at {aspect}"
            );
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

    /// A slot is drawn as a bevel — the box, then its rim — and the slot in hand is the same box
    /// in different colours, which is the whole of how "selected" is shown.
    #[test]
    fn a_slot_is_a_bevelled_box_and_the_slot_in_hand_is_the_lit_one() {
        let cell = Rect::new([0.0, -0.8], [0.05, 0.08]);
        let mut plain = GeometryData::default();
        push_slot(&mut plain, cell, 16.0 / 9.0, false);
        let mut selected = GeometryData::default();
        push_slot(&mut selected, cell, 16.0 / 9.0, true);

        // Five rectangles each — the fill and four bars.
        assert_eq!(plain.vertices.len(), 20);
        assert_eq!(selected.vertices.len(), 20);
        // Every vertex of a slot is inside the cell, since the rim straddles its edge by half a
        // bevel and no more.
        for vertex in &plain.vertices {
            assert!(
                (vertex.position[0] - cell.centre[0]).abs() <= cell.half[0] + BEVEL / 2.0 + 1e-6
                    && (vertex.position[1] - cell.centre[1]).abs()
                        <= cell.half[1] + BEVEL / 2.0 + 1e-6
            );
        }
        // ...and the two are visibly different boxes.
        assert_ne!(plain.vertices[0].tint, selected.vertices[0].tint);
    }

    /// A tooltip keeps its plate on screen wherever the cursor is, and is always wide enough for
    /// the line it holds — a name in the corner still gets a readable label.
    #[test]
    fn a_tooltip_stays_on_screen_and_fits_its_text() {
        let text = "WATER BUCKET";
        for aspect in [1.0, 16.0 / 9.0] {
            for point in [
                [0.0, 0.0],
                [-0.99, -0.99],
                [0.99, 0.99],
                [-0.99, 0.99],
                [0.99, -0.99],
            ] {
                let plate = tooltip_rect(point, text, aspect);
                assert!(
                    plate.centre[0].abs() + plate.half[0] <= 1.0
                        && plate.centre[1].abs() + plate.half[1] <= 1.0,
                    "the tooltip for {point:?} runs off screen at {aspect}"
                );
                assert!(
                    plate.half[0] * 2.0 >= font::measure(text, TOOLTIP_TEXT, aspect),
                    "the plate is narrower than the line it holds"
                );
                // It is a plate with the line on it: five rectangles and some type.
                let mut geometry = GeometryData::default();
                push_tooltip(&mut geometry, point, text, aspect);
                assert!(geometry.vertices.len() > 20, "no text on the tooltip");
            }
        }
    }

    /// The caption is a plate with the name on it, centred on the point it is given — which is what
    /// puts it in the middle of the screen, above the row.
    #[test]
    fn the_caption_is_a_plate_with_the_name_on_it() {
        let mut geometry = GeometryData::default();
        push_notice(&mut geometry, [0.0, -0.6], "Water Bucket", 16.0 / 9.0);
        assert!(geometry.vertices.len() > 20, "no plate, or no text on it");

        let span = |axis: usize| {
            let values = geometry.vertices.iter().map(|v| v.position[axis]);
            (
                values.clone().fold(f32::MAX, f32::min),
                values.fold(f32::MIN, f32::max),
            )
        };
        let (left, right) = span(0);
        assert!(
            ((left + right) * 0.5).abs() < 0.01,
            "the caption is not centred: {left}..{right}"
        );
        // ...and it stands above the row, not on it.
        let (bottom, _) = span(1);
        assert!(bottom > -0.7, "the caption is down at the row");
    }

    /// The caption names what is in hand, from the language file, for a moment after it changes —
    /// and not at all while the creative screen is up, because the palette sits where it would go.
    #[test]
    fn the_caption_names_what_is_in_hand_and_then_goes_away() {
        let lang = Lang::load(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/resources/assets/lang/en.json"
        ));
        let mut inventory = Inventory::new();

        // Nothing has been picked, so nothing is announced.
        assert!(!inventory.announcing(Instant::now()));
        assert_eq!(inventory.caption(&lang, Instant::now()), None);

        // Choosing a slot announces what it holds — the label straight out of the file.
        inventory.number_key(3);
        let picked = Instant::now();
        assert!(inventory.announcing(picked));
        assert!(
            inventory.announcing(picked + LABEL_TIME / 2),
            "it stays up for a moment"
        );
        assert_eq!(inventory.caption(&lang, picked), Some("Stone"));
        assert_eq!(
            inventory.caption(&lang, picked + LABEL_TIME / 2),
            Some("Stone")
        );

        // ...and then it is gone.
        assert!(!inventory.announcing(picked + LABEL_TIME * 2));
        assert_eq!(inventory.caption(&lang, picked + LABEL_TIME * 2), None);

        // A bucket emptying itself into the world is the other thing worth announcing.
        inventory.set_selected(Item::WaterBucket);
        assert_eq!(
            inventory.caption(&lang, Instant::now()),
            Some("Water Bucket")
        );

        // The screen has its tooltips instead, so the caption stands down while it is open.
        inventory.set_open(true);
        inventory.scroll(1);
        let scrolled = Instant::now();
        assert!(inventory.announcing(scrolled));
        assert_eq!(
            inventory.caption(&lang, scrolled),
            None,
            "the palette sits where the caption would go"
        );
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

    /// A slot holds a *whole* block: the icon keeps the cube's shape and fills the slot.
    ///
    /// This is the test that pins what the display actually shows. The icon used to be drawn at
    /// half that size, with one scale on both axes — which stretched the cube across the screen
    /// and crushed its sides to a sliver, so a slot read as a block's top face and little else.
    #[test]
    fn a_block_icon_fills_its_slot_and_keeps_its_shape() {
        for aspect in [1.0, 4.0 / 3.0, 16.0 / 9.0, 21.0 / 9.0] {
            let slot = Layout::new(aspect, PALETTE.len()).slot_rect(0);
            let icon = BlockIcon::new();
            let half = icon_half(slot, icon.height, aspect);

            // What gets drawn: the corners span ±0.5 across and ±height/2 down, scaled by `half`
            // (see `BlockIcon` and `push_screen_plane`).
            let drawn = [half[0], icon.height * half[1]];
            let bounds = icon_box(slot);
            let box_size = [2.0 * bounds[0], 2.0 * bounds[1]];

            // Square in pixels: the drawn width, once the aspect ratio is taken out, is the
            // icon's own width — a `height`th of its height.
            assert!(
                (half[1] - half[0] * aspect).abs() < 1e-6,
                "the icon is stretched at {aspect}: {half:?}"
            );
            // The block's height fills the slot, and its width stays inside it.
            assert!(
                (drawn[1] - box_size[1]).abs() < 1e-6,
                "the icon does not fill its slot at {aspect}"
            );
            assert!(
                drawn[0] <= box_size[0] + 1e-6,
                "the icon is wider than its slot at {aspect}"
            );
            // ...and it is centred, so nothing hangs out of one side.
            assert!(drawn[0] > 0.0 && drawn[1] > 0.0);
        }
    }

    #[test]
    fn a_block_icon_is_big_enough_to_read() {
        // A cell is 0.16 NDC tall, so a 1920×1080 window gives an 86-pixel cell and an icon 69
        // of them tall — where the half-scaled one was 22, its sides crushed under the top face.
        let (width, height) = (1920.0_f32, 1080.0_f32);
        let aspect = width / height;
        let slot = Layout::new(aspect, PALETTE.len()).slot_rect(0);
        let icon = BlockIcon::new();
        let half = icon_half(slot, icon.height, aspect);

        let icon_height = icon.height * half[1] * height * 0.5;
        let cell_height = slot.half[1] * 2.0 * height;
        assert!(
            icon_height >= 60.0,
            "a block icon is only {icon_height} pixels tall"
        );
        assert!(
            icon_height <= cell_height,
            "a block icon ({icon_height} px) runs out of its cell ({cell_height} px)"
        );
    }

    /// Pick-block (the wheel button) makes what is being looked at the thing in hand: a slot that
    /// already holds it is selected, and one that does not replaces the selected slot.
    #[test]
    fn picking_an_item_selects_it_or_puts_it_in_the_selected_slot() {
        let mut inventory = Inventory::new();
        let log = Item::Block(Block::OakLog);

        // An item the row already holds: picking it *selects* that slot, leaving the others be.
        inventory.number_key(1);
        assert_eq!(
            inventory.selected(),
            Some(Item::Block(Block::Grass)),
            "slot 1 holds grass"
        );
        inventory.pick_item(log);
        assert_eq!(inventory.selected(), Some(log));
        assert_eq!(
            inventory.selected_index(),
            3,
            "which is the slot holding it"
        );
        // ...and that slot had it already, so nothing else moved.
        inventory.number_key(1);
        assert_eq!(inventory.selected(), Some(Item::Block(Block::Grass)));

        // One the row does *not* hold takes the selected slot's place, so what is in hand is
        // always what was just looked at. A bucket is an item like any other here.
        inventory.number_key(7);
        assert_eq!(inventory.selected(), Some(Item::Block(Block::Poppy)));
        inventory.pick_item(Item::WaterBucket);
        assert_eq!(inventory.selected_index(), 6, "the same slot");
        assert_eq!(inventory.selected(), Some(Item::WaterBucket));
        // Picking it again is now the easy case: a slot already holds it.
        inventory.number_key(1);
        inventory.pick_item(Item::WaterBucket);
        assert_eq!(inventory.selected_index(), 6);
        assert_eq!(inventory.selected(), Some(Item::WaterBucket));
    }

    /// The creative screen offers **no bare fluid**. Water and lava are held in buckets and
    /// nowhere else, which is what makes a bucket the only way to put one into the world or take
    /// one out of it — the reason the two block entries became two bucket entries.
    #[test]
    fn the_palette_offers_fluids_only_in_buckets() {
        for item in PALETTE {
            if let Some(block) = item.block() {
                assert!(
                    !block.is_fluid(),
                    "{block:?} is on the palette as a block of its own"
                );
            }
        }
        for bucket in [Item::Bucket, Item::WaterBucket, Item::LavaBucket] {
            assert!(
                PALETTE.contains(&bucket),
                "{bucket:?} is missing from the palette"
            );
        }
    }
}
