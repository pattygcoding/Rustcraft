//! A tiny hand-rolled 5×7 bitmap font, drawn as flat screen-space quads.
//!
//! There is no font file and no glyph texture here. The HUD draws with
//! [`mesh::push_screen_plane`](crate::gfx::mesh::push_screen_plane) and
//! [`mesh::NO_TEXTURE`](crate::gfx::mesh::NO_TEXTURE), where the *tint* is the colour, so a
//! letter is just a handful of little tinted rectangles — which is why the pause menu can say
//! what its buttons do without adding an asset, a second pipeline or a second bind group. The
//! font simply rides the HUD geometry it is drawn into.
//!
//! The shapes are stored one `u64` per glyph: seven rows of five bits, the top row in the most
//! significant of the thirty-five, so `0b01110_10001_…` reads like the letter it draws. The
//! font covers `A`–`Z` (the register a five-pixel-wide grid does well), `0`–`9` and space;
//! anything else draws as a blank and still takes its place in the line.
//!
//! Two rules come from the rest of the HUD and are worth repeating, because they are what make
//! text legible at any window size:
//!
//! * **A line is sized by its height.** The glyph *is* five by seven, so growing the height
//!   grows the whole letter and the width follows.
//! * **A measure across the screen is divided by the aspect ratio**, so a letter stays square
//!   in *pixels* however the window is shaped — the same arithmetic that keeps an inventory
//!   cell square.

use crate::gfx::mesh::GeometryData;
use crate::ui::{Rect, push_rect};

/// A glyph's box, in font pixels, across.
pub const GLYPH_WIDTH: u32 = 5;
/// A glyph's box, in font pixels, down.
pub const GLYPH_HEIGHT: u32 = 7;

/// How far apart two glyph boxes sit, in font pixels: the glyph plus a one-pixel gap, so the
/// letters of a word do not run together.
const ADVANCE: f32 = (GLYPH_WIDTH + 1) as f32;

/// How many bits one glyph uses: seven rows of five.
const PACKED_BITS: u32 = GLYPH_WIDTH * GLYPH_HEIGHT;

/// Every glyph the font has, in the order it is searched — which is only ever a handful of
/// comparisons, for the few characters a label holds.
///
/// One line each: five bits per row, top row first. The rows are separated by `_` purely so the
/// shape stays visible in the source.
#[rustfmt::skip]
const GLYPHS: [(char, u64); 37] = [
    (' ', 0b00000_00000_00000_00000_00000_00000_00000),
    ('0', 0b01110_10001_10011_10101_11001_10001_01110),
    ('1', 0b00100_01100_00100_00100_00100_00100_01110),
    ('2', 0b01110_10001_00001_00010_00100_01000_11111),
    ('3', 0b11111_00010_00100_00010_00001_10001_01110),
    ('4', 0b00010_00110_01010_10010_11111_00010_00010),
    ('5', 0b11111_10000_11110_00001_00001_10001_01110),
    ('6', 0b00110_01000_10000_11110_10001_10001_01110),
    ('7', 0b11111_00001_00010_00100_01000_01000_01000),
    ('8', 0b01110_10001_10001_01110_10001_10001_01110),
    ('9', 0b01110_10001_10001_01111_00001_00010_01100),
    ('A', 0b01110_10001_10001_11111_10001_10001_10001),
    ('B', 0b11110_10001_10001_11110_10001_10001_11110),
    ('C', 0b01110_10001_10000_10000_10000_10001_01110),
    ('D', 0b11110_10001_10001_10001_10001_10001_11110),
    ('E', 0b11111_10000_10000_11110_10000_10000_11111),
    ('F', 0b11111_10000_10000_11110_10000_10000_10000),
    ('G', 0b01110_10001_10000_10111_10001_10001_01110),
    ('H', 0b10001_10001_10001_11111_10001_10001_10001),
    ('I', 0b11111_00100_00100_00100_00100_00100_11111),
    ('J', 0b00001_00001_00001_00001_10001_10001_01110),
    ('K', 0b10001_10010_10100_11000_10100_10010_10001),
    ('L', 0b10000_10000_10000_10000_10000_10000_11111),
    ('M', 0b10001_11011_10101_10001_10001_10001_10001),
    ('N', 0b10001_11001_10101_10011_10001_10001_10001),
    ('O', 0b01110_10001_10001_10001_10001_10001_01110),
    ('P', 0b11110_10001_10001_11110_10000_10000_10000),
    ('Q', 0b01110_10001_10001_10001_10101_10010_01101),
    ('R', 0b11110_10001_10001_11110_10100_10010_10001),
    ('S', 0b01111_10000_10000_01110_00001_00001_11110),
    ('T', 0b11111_00100_00100_00100_00100_00100_00100),
    ('U', 0b10001_10001_10001_10001_10001_10001_01110),
    ('V', 0b10001_10001_10001_10001_10001_01010_00100),
    ('W', 0b10001_10001_10001_10101_10101_11011_10001),
    ('X', 0b10001_10001_01010_00100_01010_10001_10001),
    ('Y', 0b10001_10001_01010_00100_00100_00100_00100),
    ('Z', 0b11111_00001_00010_00100_01000_10000_11111),
];

/// Whether the font has a glyph for `c`.
///
/// A test helper, and marked as one: the game draws whatever it is given, but a *test* can hold
/// every name in it to the font, which is how a stray hyphen or a lower-case letter is caught
/// rather than discovered as a hole in the middle of a word.
#[cfg(test)]
pub fn can_draw(c: char) -> bool {
    glyph(c).is_some()
}

/// The pixels of `c`, or `None` for a character the font has no glyph for.
///
/// Lower case is folded onto its upper-case glyph, so a caller need not shout: the register is
/// upper case because that is what a five-pixel-wide grid does well.
fn glyph(c: char) -> Option<u64> {
    let wanted = c.to_ascii_uppercase();
    GLYPHS
        .iter()
        .find(|(known, _)| *known == wanted)
        .map(|(_, packed)| *packed)
}

/// Is the pixel at `row` (from the top) and `column` (from the left) of `packed` set?
fn pixel_on(packed: u64, row: u32, column: u32) -> bool {
    packed & (1 << (PACKED_BITS - 1 - row * GLYPH_WIDTH - column)) != 0
}

/// How tall one font pixel is drawn when the glyph box is `height` NDC units tall.
fn pixel_size(height: f32) -> f32 {
    height / GLYPH_HEIGHT as f32
}

/// How wide `text` is drawn, in NDC, when it is `height` tall.
///
/// The last glyph's trailing gap is left out of the box, so a line centred on its width is
/// centred on its *ink* rather than a pixel to the right of it.
pub fn measure(text: &str, height: f32, aspect: f32) -> f32 {
    let count = text.chars().count() as f32;
    let ink = (count * ADVANCE - (ADVANCE - GLYPH_WIDTH as f32)).max(0.0);
    ink * pixel_size(height) / aspect
}

/// Append `text`, centred on `centre` and `height` NDC units tall, in `color` — packed
/// `0xAARRGGBB`, as [`crate::gfx::mesh::Vertex::tint`] is.
///
/// A character with no glyph is drawn as nothing but still advances, so a space or a stray
/// character keeps its place in the line rather than pulling the rest of it left.
pub fn push_text(
    geometry: &mut GeometryData,
    text: &str,
    centre: [f32; 2],
    height: f32,
    aspect: f32,
    color: u32,
) {
    let pixel = pixel_size(height);
    let step = pixel / aspect;
    // The box the line is centred on, walked left to right one glyph at a time.
    let mut x = centre[0] - measure(text, height, aspect) * 0.5;
    let top = centre[1] + height * 0.5;
    for c in text.chars() {
        if let Some(packed) = glyph(c) {
            push_glyph(geometry, packed, [x, top], pixel, aspect, color);
        }
        x += ADVANCE * step;
    }
}

/// Append one glyph as a few tinted rectangles: one per run of lit pixels in each row.
///
/// Merging a row's pixels into runs is what keeps a line of text cheap — an `E` is four quads
/// rather than the eighteen a rectangle per pixel would cost — and the HUD mesh is rebuilt only
/// when the screen it draws actually changes.
fn push_glyph(
    geometry: &mut GeometryData,
    packed: u64,
    top_left: [f32; 2],
    pixel: f32,
    aspect: f32,
    color: u32,
) {
    let half = [pixel * 0.5 / aspect, pixel * 0.5];
    for row in 0..GLYPH_HEIGHT {
        let mut column = 0;
        while column < GLYPH_WIDTH {
            if !pixel_on(packed, row, column) {
                column += 1;
                continue;
            }
            // Take the whole run before placing anything, so one quad covers it.
            let first = column;
            while column < GLYPH_WIDTH && pixel_on(packed, row, column) {
                column += 1;
            }
            let run = (column - first) as f32;
            let centre = [
                top_left[0] + (first as f32 + run * 0.5) * pixel / aspect,
                top_left[1] - (row as f32 + 0.5) * pixel,
            ];
            push_rect(geometry, Rect::new(centre, [half[0] * run, half[1]]), color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The glyph table is hand-written, so the two ways of getting it wrong are a row that
    /// spills past its five bits and a letter with no ink at all (which would draw nothing).
    #[test]
    fn every_glyph_fits_its_box_and_has_ink_in_it() {
        for (c, packed) in GLYPHS {
            assert_eq!(
                packed >> PACKED_BITS,
                0,
                "{c:?} has bits above its five-by-seven box"
            );
            if c != ' ' {
                assert_ne!(packed, 0, "{c:?} has no ink at all");
            }
        }
    }

    #[test]
    fn a_glyph_is_read_the_way_it_is_written() {
        // 'A' as it appears in the table: `.###.` over `#...#`.
        let a = glyph('A').expect("the font has an A");
        assert!(!pixel_on(a, 0, 0), "the top-left corner is clear");
        for column in 1..4 {
            assert!(
                pixel_on(a, 0, column),
                "'A' has a top bar at column {column}"
            );
        }
        assert!(pixel_on(a, 1, 0) && pixel_on(a, 1, 4), "'A' has both legs");
        assert!(!pixel_on(a, 1, 2), "'A' is hollow between its legs");
    }

    #[test]
    fn lower_case_folds_onto_upper_case() {
        assert_eq!(glyph('a'), glyph('A'));
        assert!(glyph('7').is_some(), "the font has digits too");
        assert_eq!(glyph('~'), None, "the font has no tilde");
    }

    #[test]
    fn a_line_is_centred_on_its_ink() {
        // 'H' inks its whole box, so three of them span exactly three advances less the
        // trailing gap: the middle of the vertices is the point they were centred on.
        let mut geometry = GeometryData::default();
        push_text(
            &mut geometry,
            "HHH",
            [0.3, -0.1],
            0.14,
            16.0 / 9.0,
            0xFFFF_FFFF,
        );
        let xs = || geometry.vertices.iter().map(|v| v.position[0]);
        let (min, max) = (xs().fold(f32::MAX, f32::min), xs().fold(f32::MIN, f32::max));
        assert!(
            ((min + max) * 0.5 - 0.3).abs() < 1e-4,
            "the line is not centred"
        );
        let expected = measure("HHH", 0.14, 16.0 / 9.0);
        assert!(
            (max - min - expected).abs() < 1e-4,
            "the line is not its own width"
        );
    }

    #[test]
    fn text_is_sized_by_its_height_and_stays_square() {
        // One glyph is five pixels wide, so its ink is five sevenths of its height — in NDC,
        // once the aspect ratio is taken back out.
        for aspect in [1.0, 4.0 / 3.0, 16.0 / 9.0, 21.0 / 9.0] {
            let width = measure("I", 0.28, aspect);
            assert!(
                (width * aspect - 0.28 * GLYPH_WIDTH as f32 / GLYPH_HEIGHT as f32).abs() < 1e-6,
                "a glyph is not square in pixels at {aspect}"
            );
            // A longer line is wider, and a line of nothing is as wide as nothing.
            assert!(measure("II", 0.28, aspect) > width);
            assert_eq!(measure("", 0.28, aspect), 0.0);
        }
    }

    #[test]
    fn a_character_the_font_lacks_draws_nothing() {
        let mut geometry = GeometryData::default();
        push_text(&mut geometry, "~", [0.0, 0.0], 0.1, 1.0, 0xFFFF_FFFF);
        assert!(
            geometry.vertices.is_empty(),
            "a missing glyph drew something"
        );
    }

    #[test]
    fn a_line_is_a_run_of_whole_quads() {
        let mut geometry = GeometryData::default();
        push_text(
            &mut geometry,
            "PAUSED",
            [0.0, 0.0],
            0.1,
            16.0 / 9.0,
            0xFFFF_FFFF,
        );
        assert!(!geometry.vertices.is_empty(), "nothing was drawn");
        assert_eq!(geometry.vertices.len() % 4, 0, "a quad is four vertices");
        assert_eq!(
            geometry.indices.len(),
            geometry.vertices.len() / 4 * 6,
            "every quad is two triangles"
        );
    }
}
