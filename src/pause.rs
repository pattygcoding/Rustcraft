//! The pause menu: the screen `Esc` opens, offering **Continue** and **Exit**.
//!
//! A pause is two things at once, and they are deliberately split. It **takes the cursor** — a
//! menu you cannot click is not a menu — and it **freezes the world**, so nothing moves under
//! the screen you are reading. `App` does both of those, because only it owns the window and
//! the camera; this module knows merely what the menu *is*: which buttons it offers, where they
//! sit, and which one the cursor is over. Keeping the layout here is what stops the drawing and
//! the hit test from drifting apart — the same trick the inventory's `Layout` plays.
//!
//! The screen is drawn as flat quads, with the labels coming from the tiny bitmap font in
//! [`crate::font`]. There is no font file and no new pipeline, so the menu is just more HUD
//! geometry in the pass the crosshair and the quick-access row already use.

use crate::font;
use crate::gfx::mesh::GeometryData;
use crate::ui::{Rect, push_rect};

/// The menu's title.
const TITLE: &str = "PAUSED";
/// The line under the buttons, naming the key that does the same job as *Continue*.
const HINT: &str = "ESC TO RESUME";

/// The buttons, in the order they are drawn — top to bottom.
pub const ACTIONS: [Action; 2] = [Action::Continue, Action::Exit];
/// How many buttons that is, for arrays that have one slot per button.
const BUTTONS: usize = ACTIONS.len();

/// How tall the panel is, in NDC — and, through [`PANEL_RATIO`], how wide.
const PANEL_HEIGHT: f32 = 0.62;
/// The panel's width as a multiple of its height. Both are divided by the aspect ratio where
/// they are horizontal, so the panel keeps its *shape* in pixels however the window is sized.
const PANEL_RATIO: f32 = 1.30;

/// Where the title sits down the panel, how tall it is, and the same for the hint line. All
/// vertical NDC, so their widths follow from the aspect ratio and the letters stay square.
const TITLE_Y: f32 = 0.215;
const TITLE_HEIGHT: f32 = 0.10;
const HINT_Y: f32 = -0.225;
const HINT_HEIGHT: f32 = 0.04;

/// Buttons: how tall, how much of the panel's width they fill, where the first one's centre
/// sits, and how far below it the second one's does.
const BUTTON_HEIGHT: f32 = 0.12;
const BUTTON_FILL: f32 = 0.85;
const BUTTON_TOP: f32 = 0.05;
const BUTTON_STEP: f32 = 0.135;
/// How tall a button's label is drawn. A little under half the button, so the text breathes.
const LABEL_HEIGHT: f32 = 0.055;

/// UI colours, packed `0xAARRGGBB` exactly as `Block::tint` and the inventory's are: the tint
/// *is* the colour for a `NO_TEXTURE` quad, and it blends, so the alpha is real.
const SCRIM_COLOR: u32 = 0xB0_00_00_00;
const PANEL_COLOR: u32 = 0xE8_14_14_14;
const BUTTON_COLOR: u32 = 0x50_FF_FF_FF;
const BUTTON_HOVER_COLOR: u32 = 0xA0_FF_FF_FF;
const TITLE_COLOR: u32 = 0xFF_FF_FF_FF;
const HINT_COLOR: u32 = 0x90_FF_FF_FF;

/// What a button does.
///
/// The menu reports which one was clicked and nothing more: what *Continue* or *Exit* means —
/// handing the cursor back, or stopping the event loop — is the app's business, not the
/// screen's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Close the menu and go back to the world.
    Continue,
    /// Leave the game.
    Exit,
}

impl Action {
    /// The button's label, in the font's upper case.
    pub fn label(self) -> &'static str {
        match self {
            Action::Continue => "CONTINUE",
            Action::Exit => "EXIT",
        }
    }

    /// Where the button sits in the panel, counting down from the top.
    fn index(self) -> usize {
        match self {
            Action::Continue => 0,
            Action::Exit => 1,
        }
    }
}

/// Where the menu sits on screen, in NDC.
///
/// Drawing and hit-testing both read this, so a button is clickable exactly where it was drawn.
#[derive(Clone, Debug)]
pub struct Layout {
    panel: Rect,
    buttons: [Rect; BUTTONS],
    title: [f32; 2],
    hint: [f32; 2],
}

impl Layout {
    /// Lay the menu out for a viewport of `aspect`.
    ///
    /// Everything is measured down the screen and divided by the aspect ratio where it is
    /// across, so the panel, the buttons and the letters keep their shape in *pixels* whatever
    /// the window is — the rule the inventory's square cells follow too.
    pub fn new(aspect: f32) -> Self {
        let aspect = aspect.max(1e-3);
        let panel_half = [
            PANEL_HEIGHT * 0.5 * PANEL_RATIO / aspect,
            PANEL_HEIGHT * 0.5,
        ];
        let buttons = std::array::from_fn(|index| {
            let centre = [0.0, BUTTON_TOP - index as f32 * BUTTON_STEP];
            Rect::new(centre, [panel_half[0] * BUTTON_FILL, BUTTON_HEIGHT * 0.5])
        });
        Self {
            panel: Rect::new([0.0, 0.0], panel_half),
            buttons,
            title: [0.0, TITLE_Y],
            hint: [0.0, HINT_Y],
        }
    }

    /// The panel behind everything.
    pub fn panel(&self) -> Rect {
        self.panel
    }

    /// The box `action`'s button occupies.
    pub fn button(&self, action: Action) -> Rect {
        self.buttons[action.index()]
    }

    /// Which button is under `point`, if any.
    pub fn action_at(&self, point: [f32; 2]) -> Option<Action> {
        ACTIONS
            .iter()
            .copied()
            .find(|&action| self.button(action).contains(point))
    }

    /// Where the title's centre sits.
    pub fn title(&self) -> [f32; 2] {
        self.title
    }

    /// Where the hint's centre sits.
    pub fn hint(&self) -> [f32; 2] {
        self.hint
    }
}

/// The pause menu: whether it is up, and what the cursor is over.
///
/// Nothing here touches the world — a paused game and a running one differ outside this type —
/// so the whole screen is a pure function of its own state and the viewport's aspect ratio,
/// which is what makes it testable without a window.
#[derive(Debug)]
pub struct PauseMenu {
    open: bool,
    /// Cursor position in NDC. Only meaningful while the menu is open.
    cursor: [f32; 2],
    /// The button under the cursor, if any.
    hovered: Option<Action>,
}

impl Default for PauseMenu {
    fn default() -> Self {
        Self::new()
    }
}

impl PauseMenu {
    /// A menu that is not showing.
    pub fn new() -> Self {
        Self {
            open: false,
            cursor: [0.0, 0.0],
            hovered: None,
        }
    }

    /// Is the menu up?
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Show or hide the menu.
    ///
    /// Hiding forgets what the cursor was over, so a highlight cannot survive into the next time
    /// the menu opens on a cursor that has since moved elsewhere.
    pub fn set_open(&mut self, open: bool) {
        self.open = open;
        if !open {
            self.hovered = None;
        }
    }

    /// The button under the cursor, if any. This is what the renderer watches to know whether
    /// the highlight has moved.
    pub fn hovered(&self) -> Option<Action> {
        self.hovered
    }

    /// Point the cursor at `point` (NDC) and note which button it is over.
    ///
    /// `aspect` must be the viewport aspect the drawing uses, or the highlight would land on a
    /// button that is not the one under the mouse.
    pub fn set_cursor(&mut self, point: [f32; 2], aspect: f32) {
        self.cursor = point;
        self.hovered = if self.open {
            Layout::new(aspect).action_at(point)
        } else {
            None
        };
    }

    /// Handle a click at `point` (NDC): the button it landed on, if any.
    pub fn click(&mut self, point: [f32; 2], aspect: f32) -> Option<Action> {
        if !self.open {
            return None;
        }
        Layout::new(aspect).action_at(point)
    }

    /// Append the menu's geometry (already in clip space) to a HUD geometry set.
    pub fn push_geometry(&self, geometry: &mut GeometryData, aspect: f32) {
        let layout = Layout::new(aspect);

        // A scrim over the whole screen first — one quad the size of NDC — so the world behind
        // is dimmed rather than merely covered, and the panel then reads as a card laid over it.
        push_rect(geometry, Rect::new([0.0, 0.0], [1.0, 1.0]), SCRIM_COLOR);
        push_rect(geometry, layout.panel(), PANEL_COLOR);
        font::push_text(
            geometry,
            TITLE,
            layout.title(),
            TITLE_HEIGHT,
            aspect,
            TITLE_COLOR,
        );

        for action in ACTIONS {
            let button = layout.button(action);
            let color = if self.hovered == Some(action) {
                BUTTON_HOVER_COLOR
            } else {
                BUTTON_COLOR
            };
            push_rect(geometry, button, color);
            font::push_text(
                geometry,
                action.label(),
                button.centre,
                LABEL_HEIGHT,
                aspect,
                TITLE_COLOR,
            );
        }

        font::push_text(
            geometry,
            HINT,
            layout.hint(),
            HINT_HEIGHT,
            aspect,
            HINT_COLOR,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two buttons are laid out and hit-tested from the same [`Layout`], so what you see
    /// and what you click cannot disagree — the property the inventory's layout is built on too.
    #[test]
    fn every_button_is_clickable_exactly_where_it_is_drawn() {
        let layout = Layout::new(16.0 / 9.0);
        for (index, action) in ACTIONS.iter().copied().enumerate() {
            assert_eq!(action.index(), index, "ACTIONS and Action::index disagree");
            let button = layout.button(action);
            assert_eq!(layout.action_at(button.centre), Some(action));
        }

        // A point clear of every button hits nothing.
        let away = [0.0, layout.hint()[1]];
        assert_eq!(layout.action_at(away), None);
    }

    #[test]
    fn the_buttons_do_not_overlap_each_other() {
        let layout = Layout::new(16.0 / 9.0);
        for action in ACTIONS {
            let other = if action == Action::Continue {
                Action::Exit
            } else {
                Action::Continue
            };
            assert!(
                !layout.button(action).contains(layout.button(other).centre),
                "{action:?} covers {other:?}"
            );
        }
    }

    #[test]
    fn nothing_falls_off_the_screen() {
        for aspect in [1.0, 4.0 / 3.0, 16.0 / 9.0, 21.0 / 9.0] {
            let layout = Layout::new(aspect);
            let on_screen = |rect: Rect| {
                rect.centre[0].abs() + rect.half[0] <= 1.0
                    && rect.centre[1].abs() + rect.half[1] <= 1.0
            };
            assert!(on_screen(layout.panel()), "the panel runs off at {aspect}");
            for action in ACTIONS {
                assert!(
                    on_screen(layout.button(action)),
                    "{action:?} runs off at {aspect}"
                );
            }

            // The text is sized in *pixels*, so a square aspect is where a line is
            // proportionally widest in NDC — and so where it would run past its own box.
            let panel_width = layout.panel().half[0] * 2.0;
            for line in [TITLE, HINT] {
                let height = if line == TITLE {
                    TITLE_HEIGHT
                } else {
                    HINT_HEIGHT
                };
                let width = font::measure(line, height, aspect);
                assert!(width < panel_width, "{line:?} is wider than the panel");
            }
            for action in ACTIONS {
                let width = font::measure(action.label(), LABEL_HEIGHT, aspect);
                assert!(
                    width < layout.button(action).half[0] * 2.0,
                    "{action:?}'s label is wider than its button"
                );
            }
        }
    }

    #[test]
    fn every_line_the_menu_draws_has_ink() {
        // A character the font lacks draws nothing, so a typo in a label would show up as a
        // word with a hole in it — or, for a whole line, as no line at all.
        for line in [TITLE, HINT, Action::Continue.label(), Action::Exit.label()] {
            let mut geometry = GeometryData::default();
            font::push_text(
                &mut geometry,
                line,
                [0.0, 0.0],
                0.1,
                16.0 / 9.0,
                0xFFFF_FFFF,
            );
            assert!(!geometry.vertices.is_empty(), "{line:?} drew nothing");
        }
    }

    #[test]
    fn the_menu_starts_shut_and_forgets_its_highlight_when_it_closes() {
        let mut menu = PauseMenu::new();
        assert!(!menu.is_open(), "the game does not start paused");
        // Nothing is hovered while it is shut, wherever the cursor happens to be.
        let layout = Layout::new(16.0 / 9.0);
        menu.set_cursor(layout.button(Action::Exit).centre, 16.0 / 9.0);
        assert_eq!(menu.hovered(), None);

        menu.set_open(true);
        menu.set_cursor(layout.button(Action::Exit).centre, 16.0 / 9.0);
        assert_eq!(menu.hovered(), Some(Action::Exit));

        menu.set_open(false);
        assert_eq!(menu.hovered(), None, "the highlight outlived the menu");
    }

    #[test]
    fn clicking_a_button_reports_it_and_clicking_elsewhere_does_not() {
        let aspect = 16.0 / 9.0;
        let layout = Layout::new(aspect);
        let mut menu = PauseMenu::new();

        // A shut menu swallows nothing: that click belongs to the world.
        assert_eq!(
            menu.click(layout.button(Action::Continue).centre, aspect),
            None
        );

        menu.set_open(true);
        assert_eq!(
            menu.click(layout.button(Action::Continue).centre, aspect),
            Some(Action::Continue)
        );
        assert_eq!(
            menu.click(layout.button(Action::Exit).centre, aspect),
            Some(Action::Exit)
        );
        assert_eq!(menu.click([0.95, 0.95], aspect), None);
    }

    #[test]
    fn the_whole_menu_is_drawn_on_screen() {
        // Every quad the menu makes — scrim, panel, buttons and every letter — has to land
        // inside NDC, or part of the screen would be showing something it should not.
        for aspect in [1.0, 4.0 / 3.0, 16.0 / 9.0, 21.0 / 9.0] {
            let mut geometry = GeometryData::default();
            PauseMenu::new().push_geometry(&mut geometry, aspect);
            assert!(!geometry.vertices.is_empty());
            for vertex in &geometry.vertices {
                assert!(
                    vertex.position[0].abs() <= 1.0 && vertex.position[1].abs() <= 1.0,
                    "a vertex at {:?} runs off screen at {aspect}",
                    vertex.position
                );
            }
        }
    }

    #[test]
    fn the_drawing_is_a_scrim_a_panel_two_buttons_and_some_letters() {
        let mut geometry = GeometryData::default();
        PauseMenu::new().push_geometry(&mut geometry, 16.0 / 9.0);

        // Two quads for the scrim and the panel, two more for the buttons, and a run of quads
        // per glyph on top. Whole quads only: four vertices each, two triangles apiece.
        assert!(
            geometry.vertices.len() >= 16,
            "the chrome alone is four quads"
        );
        assert_eq!(geometry.vertices.len() % 4, 0, "a quad is four vertices");
        assert_eq!(geometry.indices.len(), geometry.vertices.len() / 4 * 6);
    }
}
