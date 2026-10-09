//! Screen-space UI primitives shared by the HUD's screens.
//!
//! Every screen the player sees is built the same way: flat quads of colour — and, for an
//! inventory slot, a block icon — placed in NDC (clip space) by
//! [`mesh::push_screen_plane`](crate::gfx::mesh::push_screen_plane). Two pieces of that more
//! than one screen needs are a rectangle and the call that appends one, so they live here
//! rather than in whichever screen happened to need them first: the quick-access row and the
//! creative screen ([`crate::inventory`]) and the pause menu ([`crate::pause`]) all place
//! rectangles, and the bitmap font ([`crate::font`]) draws every letter out of them.

use crate::gfx::mesh::{self, FULL_LIGHT, FaceStyle, GeometryData, NO_TEXTURE};

/// A rectangle in NDC: its centre and half-size.
///
/// The half-size is what is stored, rather than the corners, because placing a cell is then a
/// matter of naming its middle — which is what the drawing *and* the hit test both want, and
/// why one [`Rect`] can serve for both without the two drifting apart.
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

/// The four corners of a unit square, in `[top-left, top-right, bottom-right, bottom-left]`
/// order — what a plain UI quad is made of.
pub const UNIT_SQUARE: [([f32; 2], [f32; 2]); 4] = [
    ([-1.0, 1.0], [0.0, 0.0]),
    ([1.0, 1.0], [1.0, 0.0]),
    ([1.0, -1.0], [1.0, 1.0]),
    ([-1.0, -1.0], [0.0, 1.0]),
];

/// Append a solid rectangle: no texture at all, just the colour.
pub fn push_rect(geometry: &mut GeometryData, rect: Rect, color: u32) {
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

/// Append a rectangle with a **bevel** around it: the fill, then a border whose two long sides
/// take different colours.
///
/// It is the trick that makes a rectangle read as *recessed* or as *raised* rather than as a flat
/// square: a hollow catches the light on the side away from it, so an inset slot is dark along
/// its top and left and light along its bottom and right — and a raised button is the same the
/// other way round. Passing one colour twice gives a plain ring, which is what a highlight is.
///
/// The horizontal bars are thinner in NDC than the vertical ones, for the reason every horizontal
/// measure in the HUD is divided by the aspect ratio: the viewport is wider than it is tall, so
/// equal NDC is not equally square on screen.
pub fn push_bevel(
    geometry: &mut GeometryData,
    rect: Rect,
    aspect: f32,
    thickness: f32,
    fill: u32,
    dark: u32,
    light: u32,
) {
    push_rect(geometry, rect, fill);
    let thin = [thickness * 0.5 / aspect, thickness * 0.5];
    let (x, y) = (rect.half[0], rect.half[1]);
    for (offset, half, color) in [
        // Dark above and to the left, light below and to the right.
        ([0.0, y], [x + thin[0], thin[1]], dark),
        ([-x, 0.0], [thin[0], y + thin[1]], dark),
        ([0.0, -y], [x + thin[0], thin[1]], light),
        ([x, 0.0], [thin[0], y + thin[1]], light),
    ] {
        let centre = [rect.centre[0] + offset[0], rect.centre[1] + offset[1]];
        push_rect(geometry, Rect::new(centre, half), color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rectangle_contains_its_middle_and_not_its_edges() {
        let rect = Rect::new([0.25, -0.5], [0.1, 0.2]);
        assert!(rect.contains([0.25, -0.5]), "the centre is inside");
        assert!(rect.contains([0.35, -0.7]), "a corner is inside");
        assert!(!rect.contains([0.36, -0.5]), "just past the right edge");
        assert!(!rect.contains([0.25, -0.71]), "just below the bottom edge");
    }

    #[test]
    fn a_rectangle_is_four_vertices_and_two_triangles() {
        let mut geometry = GeometryData::default();
        push_rect(
            &mut geometry,
            Rect::new([0.0, 0.0], [1.0, 1.0]),
            0xFFFF_FFFF,
        );
        assert_eq!(geometry.vertices.len(), 4);
        assert_eq!(geometry.indices.len(), 6);
    }

    /// A bevel is the fill plus four bars, and the bars hang *outside* the rectangle's edge by
    /// half their thickness — so the border sits on the rectangle's own line rather than inside
    /// it, and a bevelled cell is exactly as big as a plain one.
    #[test]
    fn a_bevel_is_a_fill_and_four_bars_that_straddle_the_edge() {
        let mut geometry = GeometryData::default();
        let rect = Rect::new([0.2, -0.1], [0.05, 0.05]);
        push_bevel(
            &mut geometry,
            rect,
            16.0 / 9.0,
            0.01,
            0x11_11_11_11,
            0x22_22_22_22,
            0x33_33_33_33,
        );
        assert_eq!(geometry.vertices.len(), 20, "five rectangles");
        assert_eq!(geometry.indices.len(), 30, "two triangles each");

        let reaches = |axis: usize| {
            geometry
                .vertices
                .iter()
                .map(|v| v.position[axis])
                .fold(f32::MIN, f32::max)
        };
        // A vertical bar is half a thickness (0.005) past the top and bottom edges; a horizontal
        // one is half a thickness *in NDC* divided by the aspect — the same number of pixels.
        assert!((reaches(1) - (rect.centre[1] + rect.half[1] + 0.005)).abs() < 1e-6);
        assert!((reaches(0) - (rect.centre[0] + rect.half[0] + 0.005 / (16.0 / 9.0))).abs() < 1e-6);
    }
}
