//! Carving caves and ravines out of the ground.
//!
//! Minecraft Beta's approach, and deliberately not another threshold on the density field: rather
//! than giving every cell a fractal chance of being hollow, the rock the terrain pass has *already
//! laid down* is cut into by a handful of deterministic **carvers** that walk through it. Two
//! kinds, and both of them **worms** — a point, a heading, and a radius that move and change as the
//! worm goes:
//!
//! * **Caves** ([`CAVE_GATE`] and friends) — winding tunnels. The heading drifts a little at every
//!   step, the radius swells in the middle and tapers at both ends, and part way along a fat worm
//!   forks into two thinner ones at right angles. A start in four is a **room** instead: a worm
//!   that barely steers, so it mills about in one place and hollows a chamber.
//! * **Ravines** ([`RAVINE_GATE`] and friends) — rare (one chunk in fifty), long, and nearly
//!   straight, with a heading close to horizontal but a carve stretched *vertically*. That stretch
//!   is the whole difference: the same worm flattened makes a tunnel, stretched makes a gash. The
//!   two legs it forks into at the end open it into a `Y`.
//!
//! ## Why this is seam-free
//!
//! A worm is a path through the *world*, and a chunk can only see the part of it that lies inside
//! itself. So the one thing that must never happen is for the same worm to be carved twice, by two
//! chunks, with different dice. Two rules make that impossible:
//!
//! * **A worm's dice come from its own origin and nothing else.** The origin chunk's *absolute*
//!   coordinates seed the stream, so the worm an origin grows is the same worm whichever chunk asks
//!   for it. (This is the one place we deliberately part company with the snippet this was ported
//!   from, which mixes in the *offset* from the chunk being carved — fine for Minecraft, where a
//!   chunk is carved in a single pass, but not for a streaming world.)
//! * **A worm's reach is bounded by [`RANGE`].** Caves stop once they are further from their origin
//!   than the steps they have left could carry them; ravines are shorter than that to begin with.
//!   So an origin more than `RANGE` chunks away cannot print one cell into the chunk being carved,
//!   and a chunk can find every worm that touches it by asking only the origins around it.
//!
//! Nothing is kept between chunks and nothing depends on the order they arrive in, which is what
//! [`Terrain::generate_chunk`](super::Terrain::generate_chunk) needs to stay a pure function of
//! `(seed, position)` — the property the whole streaming design rests on (see [`super::streaming`]).
//!
//! Every carved cell comes out as [`Block::Air`]: what a carver cuts is *emptiness*. Whatever is
//! still open at the bottom of the world becomes lava afterwards, in a pass of its own (see
//! `LAVA_LEVEL` in [`super::terrain`]), which is why neither carver below mentions lava at all.

use std::f32::consts::{FRAC_PI_2, TAU};
use std::f64::consts::PI;

use super::ChunkPos;
use super::block::Block;
use super::chunk::{DEPTH, HEIGHT, WIDTH};
use super::random::JavaRandom;

/// How many chunks around an origin its worms can reach into.
///
/// Eight is Minecraft's own reach, and it is *earned* here rather than copied: a cave gives up once
/// it is further from its origin than the steps it has left could carry it (see [`cave_node`],
/// which allows about 123 blocks, against `RANGE * 16` = 128), and a ravine is shorter outright.
/// `caves_and_ravines_stay_within_reach` pins both halves of that.
const RANGE: i32 = 8;

/// Only one candidate origin in this many grows a cave at all.
const CAVE_GATE: i32 = 15;

/// Upper bound on the worm starts one origin rolls. The count itself is a *nested* draw — a draw
/// bounded by a draw bounded by a draw — so a typical origin grows a handful of worms and an
/// unlucky one several times that, which is what makes cave systems cluster instead of being
/// sprinkled evenly. Minecraft's own carvers draw it the same way.
const STARTS: i32 = 40;

/// One start in this many is a wide **room** rather than a tunnel.
const ROOM_CHANCE: i32 = 4;

/// Tunnels that leave one start point, at headings drawn separately. A fan of ways in and out at a
/// single spot is what makes a cave a system rather than one corridor.
const TUNNELS: i32 = 3;

/// Steps in a cave worm before it forks or ends, as Minecraft's carvers had it:
/// `RANGE * 16 - 16` blocks of travel, a little short of the reach.
const CAVE_LENGTH: i32 = RANGE * 16 - 16;

/// A cave is this much wider than it is tall — Minecraft's caves are flattened the same way.
const CAVE_FLATTEN: f64 = 0.5;

/// One origin in this many grows ravines at all, and how many it grows when it does.
///
/// The gate is per **origin**, and there is one origin per chunk of the world, so it counts the
/// same as one ravine start per fifty chunks — which is what a player sees. [`CAVE_GATE`] is
/// deliberately a *larger* chance (one in fifteen) even though a ravine is the bigger feature: a
/// cave is a thin tunnel and a warren of them under every chunk is what an underground looks like,
/// where even a handful of ravines would turn the landscape into canyons.
const RAVINE_GATE: i32 = 50;
const RAVINE_TRIES: i32 = 1;

/// Steps in a ravine, and in each of the two legs it forks into.
const RAVINE_LENGTH: i32 = 64;
const RAVINE_LEG: i32 = 24;

/// Ravine walls are this many times as tall as the ravine is wide — the counterpart of
/// [`CAVE_FLATTEN`], and the whole reason a ravine reads as a gash rather than a tunnel.
const RAVINE_STRETCH: f64 = 2.0;

/// How far a ravine drifts *down* per step, in blocks: a ravine cuts down as it runs, so its far
/// end is well below the surface it started at and its floor reaches the lava.
const RAVINE_DESCENT: f64 = 0.25;

/// How far each leg turns off the main gash, in radians — the angle that opens a ravine into a `Y`.
const RAVINE_LEG_TURN: f32 = 1.1;

/// The block rectangle one carve pass may write into, inclusive on both ends.
///
/// Carvers work in world coordinates — a worm has no idea which chunk it is in — so an area is what
/// keeps a pass to its own chunk's cells. It is a **clip and never a decision**: the same worm,
/// offered a wider area, carves exactly the same cells inside the narrower one. That is the property
/// `a_worm_carves_the_same_cells_from_either_side_of_a_border` pins, and it is why the early-out in
/// [`carve_ellipsoid`] may be a mere bounding-box test.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Area {
    /// First and last `x` cell, inclusive.
    x: (i32, i32),
    /// First and last `z` cell, inclusive.
    z: (i32, i32),
}

impl Area {
    /// The cells of the chunk at `pos`.
    pub(crate) fn of_chunk(pos: ChunkPos) -> Self {
        Self::of_chunks(pos, pos)
    }

    /// The cells covered by the chunks from `min` to `max`, inclusive — the wider area a test
    /// carves to check that a chunk's own cells do not depend on the ones beside it.
    pub(crate) fn of_chunks(min: ChunkPos, max: ChunkPos) -> Self {
        Self {
            x: (
                min.0 * WIDTH as i32,
                max.0 * WIDTH as i32 + WIDTH as i32 - 1,
            ),
            z: (
                min.1 * DEPTH as i32,
                max.1 * DEPTH as i32 + DEPTH as i32 - 1,
            ),
        }
    }

    /// The `x` cells, as a plain pair, for the clip in [`carve_ellipsoid`].
    fn x_bounds(&self) -> (i32, i32) {
        self.x
    }

    /// The `z` cells, likewise.
    fn z_bounds(&self) -> (i32, i32) {
        self.z
    }

    /// Every origin chunk whose worms could reach into this area: `RANGE` chunks out from each of
    /// its edges.
    ///
    /// Asking more origins than strictly necessary costs a few dice draws each and nothing else, so
    /// the box is drawn generously — what matters is that nothing *inside* it is missed.
    fn origins(&self) -> impl Iterator<Item = ChunkPos> {
        let (x0, x1) = (chunk_of(self.x.0), chunk_of(self.x.1));
        let (z0, z1) = (chunk_of(self.z.0), chunk_of(self.z.1));
        (x0 - RANGE..=x1 + RANGE).flat_map(move |x| (z0 - RANGE..=z1 + RANGE).map(move |z| (x, z)))
    }
}

/// The chunk coordinate a world coordinate falls in, rounding towards minus infinity so the answer
/// is right west and north of the origin too.
fn chunk_of(w: i32) -> i32 {
    w.div_euclid(WIDTH as i32)
}

/// The seed-derived pair of odd multipliers Minecraft's carvers mix an origin's coordinates with,
/// so that origins far apart are as unrelated as origins close together.
#[derive(Clone, Copy, Debug)]
struct Dice {
    /// A per-world scramble, so two seeds do not put their caves in the same places.
    seed: i64,
    x: i64,
    z: i64,
}

impl Dice {
    /// The dice for one world.
    fn new(seed: u32) -> Self {
        let mut world = JavaRandom::new(i64::from(seed));
        // `nextLong() / 2 * 2 + 1` is Minecraft's trick for an odd number, and an odd number shares
        // no factor with the chunk coordinates it multiplies.
        let x = world.next_long() / 2 * 2 + 1;
        let z = world.next_long() / 2 * 2 + 1;
        Self {
            seed: world.next_long(),
            x,
            z,
        }
    }

    /// The stream for one origin, seeded from its **absolute** chunk coordinates.
    fn at(&self, origin: ChunkPos) -> JavaRandom {
        let mixed = i64::from(origin.0)
            .wrapping_mul(self.x)
            .wrapping_add(i64::from(origin.1).wrapping_mul(self.z));
        JavaRandom::new(mixed ^ self.seed)
    }
}

/// Carve the caves and ravines reaching the chunk at `pos`, offering every carved cell to `set` in
/// **world** coordinates.
///
/// `set` is called for cells the carve is pointless in as well — air, water, the foot of an oak —
/// because a carver cannot see what it is cutting into. Deciding what may be *replaced* is the
/// caller's job: see `Terrain::generate_chunk`, which cuts stone and soil and nothing else, so a
/// cave can never eat the sea, the bedrock, or a tree.
pub(crate) fn carve(seed: u32, pos: ChunkPos, set: &mut impl FnMut(i32, i32, i32, Block)) {
    carve_area(seed, Area::of_chunk(pos), set);
}

/// Carve every cell of `area`, which may be several chunks wide: the carvers themselves never need
/// more than one chunk, but a test does, to be able to look at a whole worm at once.
pub(crate) fn carve_area(seed: u32, area: Area, set: &mut impl FnMut(i32, i32, i32, Block)) {
    let dice = Dice::new(seed);
    for origin in area.origins() {
        carve_origin(origin, area, set, &dice);
    }
}

/// Carve the caves and ravines that grew at one origin.
///
/// Split out from [`carve_area`] so that a test can ask a single origin how far its worms wandered,
/// which is the guarantee the neighbourhood scheme rests on.
fn carve_origin(
    origin: ChunkPos,
    area: Area,
    set: &mut impl FnMut(i32, i32, i32, Block),
    dice: &Dice,
) {
    let mut rand = dice.at(origin);
    // The two carvers share one stream at an origin: a ravine's dice are then as unrelated to that
    // origin's caves as any two draws are, and there is one place to look when a seed misbehaves.
    carve_caves(&mut rand, origin, area, set);
    carve_ravines(&mut rand, origin, area, set);
}

/// Where a worm is, how fat it is, and which way it is pointing: everything that changes as it
/// walks, gathered up so the walkers can take one argument instead of six.
#[derive(Clone, Copy, Debug)]
struct Head {
    x: f64,
    y: f64,
    z: f64,
    /// Half-width of the carve at the worm's fattest, in blocks.
    thickness: f32,
    /// Heading in the horizontal plane, in radians: which way is "forward".
    yaw: f32,
    /// Climb or dive, in radians: positive is up.
    pitch: f32,
}

/// The cave starts one origin rolls, and the worms they grow.
fn carve_caves(
    rand: &mut JavaRandom,
    origin: ChunkPos,
    area: Area,
    set: &mut impl FnMut(i32, i32, i32, Block),
) {
    // A nested draw, so most origins that pass the gate grow one worm and the odd one grows a
    // dozen — which is what makes cave systems cluster instead of being sprinkled evenly.
    let span = rand.next_int_bound(STARTS) + 1;
    let span = rand.next_int_bound(span) + 1;
    let starts = rand.next_int_bound(span);
    if rand.next_int_bound(CAVE_GATE) != 0 {
        return;
    }
    for _ in 0..starts {
        // The start point: anywhere in the origin chunk, at any height in the world but biased
        // towards the bottom, since there is nothing worth carving up in the sky.
        let height = rand.next_int_bound(120) + 8;
        let head = Head {
            x: f64::from(origin.0 * WIDTH as i32 + rand.next_int_bound(WIDTH as i32)) + 0.5,
            y: f64::from(rand.next_int_bound(height)) + 0.5,
            z: f64::from(origin.1 * DEPTH as i32 + rand.next_int_bound(DEPTH as i32)) + 0.5,
            thickness: 1.0 + rand.next_float() * 6.0,
            yaw: 0.0,
            pitch: 0.0,
        };

        if rand.next_int_bound(ROOM_CHANCE) == 0 {
            // A room: one worm, steering itself almost not at all, so it mills about in one place
            // and hollows a chamber instead of boring a corridor.
            cave_node(
                JavaRandom::new(rand.next_long()),
                origin,
                area,
                head,
                true,
                set,
            );
        }
        for _ in 0..TUNNELS {
            // Each tunnel leaves the same point on a heading of its own — a fan of them is what
            // turns one start into a little system rather than a single corridor.
            let tunnel = Head {
                yaw: rand.next_float() * TAU,
                pitch: (rand.next_float() - 0.5) * 2.0 / 8.0,
                ..head
            };
            cave_node(
                JavaRandom::new(rand.next_long()),
                origin,
                area,
                tunnel,
                false,
                set,
            );
        }
    }
}

/// Carve one cave worm — or a branch, or a room: the walk is the same, and `room` only says how
/// hard it steers and whether it may fork.
///
/// Every step is one block along the heading, and every step carves an ellipsoid. The radius swells
/// to `1.5 + thickness` in the middle and tapers to `1.5` at either end, so a tunnel has a mouth
/// rather than a wall, and stands `CAVE_FLATTEN` times as tall as it is wide.
fn cave_node(
    mut rand: JavaRandom,
    origin: ChunkPos,
    area: Area,
    head: Head,
    room: bool,
    set: &mut impl FnMut(i32, i32, i32, Block),
) {
    // How far this worm gets to walk: near a full `CAVE_LENGTH`, sometimes a good deal less, so
    // worms come in a range of lengths rather than all the same.
    let count = CAVE_LENGTH - rand.next_int_bound(CAVE_LENGTH / 4);
    // A room is a tunnel that starts half way along — Minecraft's own trick for making one, and the
    // reason it never forks.
    let mut node = if room { count / 2 } else { 0 };
    // Where a fat worm forks into two, part way along. One fork only, or the world would fill with
    // branches of branches.
    let fork_at = rand.next_int_bound(count / 2) + count / 4;
    // A steep worm damps its pitch faster, so it plunges instead of levelling out.
    let damping = if rand.next_int_bound(6) == 0 {
        0.92
    } else {
        0.7
    };

    let Head {
        mut x,
        mut y,
        mut z,
        thickness,
        mut yaw,
        mut pitch,
    } = head;
    let mut yaw_drift = 0.0_f32;
    let mut pitch_drift = 0.0_f32;

    while node < count {
        node += 1;
        // One block forward, along the heading.
        let (sin_pitch, cos_pitch) = (f64::from(pitch).sin(), f64::from(pitch).cos());
        x += f64::from(yaw).cos() * cos_pitch;
        y += sin_pitch;
        z += f64::from(yaw).sin() * cos_pitch;

        // Then steer. The pitch is damped towards level, the heading drifts, and the drift itself
        // wanders — a random walk of a random walk, which is what makes a worm bend smoothly
        // instead of jittering from step to step.
        pitch *= damping;
        pitch += pitch_drift * 0.1;
        yaw += yaw_drift * 0.1;
        pitch_drift *= 0.9;
        yaw_drift *= 0.75;
        pitch_drift += (rand.next_float() - rand.next_float()) * rand.next_float() * 2.0;
        yaw_drift += (rand.next_float() - rand.next_float()) * rand.next_float() * 4.0;

        if node == fork_at && !room && thickness > 1.0 {
            // A fork: two thinner worms leaving at right angles to the parent, which ends here. The
            // children carry the rest of the way, and that is what makes a cave a *network*.
            let (fork, fork_pitch) = (thickness * 0.5, pitch / 3.0);
            for turn in [FRAC_PI_2, -FRAC_PI_2] {
                cave_node(
                    JavaRandom::new(rand.next_long()),
                    origin,
                    area,
                    Head {
                        x,
                        y,
                        z,
                        thickness: fork,
                        yaw: yaw + turn,
                        pitch: fork_pitch,
                    },
                    false,
                    set,
                );
            }
            return;
        }
        // One step in four the worm rolls no carve at all, which leaves the odd solid bump between
        // two passes of the same worm.
        if !room && rand.next_int_bound(4) == 0 {
            continue;
        }
        // Give up once the worm is further from its origin than the steps it has left could carry
        // it: that bound is what lets a chunk ask only the origins within `RANGE` and still see
        // every worm that reaches it. (Minecraft's carvers carry the same test, and it is what
        // makes eight chunks the right reach: `CAVE_LENGTH` is a little short of `RANGE * 16`.)
        let (dx, dz) = (
            x - f64::from(origin.0 * WIDTH as i32 + WIDTH as i32 / 2),
            z - f64::from(origin.1 * DEPTH as i32 + DEPTH as i32 / 2),
        );
        let left = f64::from(count - node);
        let reach = f64::from(thickness) + 2.0 + 16.0;
        if dx * dx + dz * dz - left * left > reach * reach {
            return;
        }

        let swell = 1.5 + (f64::from(node) / f64::from(count) * PI).sin() * f64::from(thickness);
        carve_ellipsoid(set, area, x, y, z, swell, swell * CAVE_FLATTEN);
    }
}

/// The ravine starts one origin rolls, and the gashes they cut.
fn carve_ravines(
    rand: &mut JavaRandom,
    origin: ChunkPos,
    area: Area,
    set: &mut impl FnMut(i32, i32, i32, Block),
) {
    // One chunk in fifty. Rare enough that finding a ravine is an event, and that a *chunk* of them
    // is a landmark you remember where you left it.
    if rand.next_int_bound(RAVINE_GATE) != 0 {
        return;
    }
    for _ in 0..RAVINE_TRIES {
        // A start height drawn the same way a cave's is, so a ravine is as likely to break the
        // surface as a cave is to stay under it.
        let height = rand.next_int_bound(120) + 8;
        let head = Head {
            x: f64::from(origin.0 * WIDTH as i32 + rand.next_int_bound(WIDTH as i32)) + 0.5,
            y: f64::from(rand.next_int_bound(height)) + 0.5,
            z: f64::from(origin.1 * DEPTH as i32 + rand.next_int_bound(DEPTH as i32)) + 0.5,
            thickness: 1.0 + rand.next_float() * 6.0,
            yaw: rand.next_float() * TAU,
            // Nearly level: a ravine runs *across* the landscape. What makes it deep is the stretch
            // applied to its carve, and what makes it descend is [`RAVINE_DESCENT`] — not a dive.
            pitch: (rand.next_float() - 0.5) / 8.0,
        };
        ravine_node(
            JavaRandom::new(rand.next_long()),
            area,
            head,
            RAVINE_LENGTH,
            set,
        );
    }
}

/// Carve one ravine: a long, nearly straight worm whose carve is stretched vertically, so that
/// what would be a tunnel standing on its side becomes a gash with walls.
///
/// No `origin` here, unlike [`cave_node`]: a ravine is short enough to stay inside [`RANGE`] of the
/// origin that grew it however it wanders, so it has no reach to give up on.
fn ravine_node(
    mut rand: JavaRandom,
    area: Area,
    head: Head,
    count: i32,
    set: &mut impl FnMut(i32, i32, i32, Block),
) {
    let count = count - rand.next_int_bound(count / 4);
    let fork_at = rand.next_int_bound(count / 2) + count / 4;

    let Head {
        mut x,
        mut y,
        mut z,
        thickness,
        mut yaw,
        mut pitch,
    } = head;
    let mut yaw_drift = 0.0_f32;
    let mut node = 0;

    while node < count {
        node += 1;
        let (sin_pitch, cos_pitch) = (f64::from(pitch).sin(), f64::from(pitch).cos());
        x += f64::from(yaw).cos() * cos_pitch;
        y += sin_pitch + RAVINE_DESCENT;
        z += f64::from(yaw).sin() * cos_pitch;

        // A ravine barely turns. The heading drifts five times more weakly than a cave's and the
        // pitch is damped hard, so the gash stays straight where a tunnel would meander.
        pitch *= 0.98;
        yaw += yaw_drift * 0.05;
        yaw_drift *= 0.9;
        yaw_drift += (rand.next_float() - rand.next_float()) * rand.next_float() * 2.0;

        if node == fork_at && thickness > 1.0 {
            // The `Y`: each leg carries on at a fair angle to the main gash, thinner and shorter.
            // Two of them, because that is what a ravine's far end looks like from above.
            let leg = thickness * 0.5;
            for turn in [RAVINE_LEG_TURN, -RAVINE_LEG_TURN] {
                ravine_node(
                    JavaRandom::new(rand.next_long()),
                    area,
                    Head {
                        x,
                        y,
                        z,
                        thickness: leg,
                        yaw: yaw + turn,
                        pitch: pitch / 2.0,
                    },
                    RAVINE_LEG,
                    set,
                );
            }
            return;
        }

        let swell = 1.5 + (f64::from(node) / f64::from(count) * PI).sin() * f64::from(thickness);
        carve_ellipsoid(set, area, x, y, z, swell, swell * RAVINE_STRETCH);
    }
}

/// Cut one ellipsoid out of the ground: the shape a worm carves at each step of its walk.
///
/// A cell is judged by its **centre**, which is Minecraft's own convention and what keeps a carve
/// symmetric about the grid rather than a quarter of a block off it.
///
/// Two things here are chosen for the *debug* build, because this is the loop a chunk's generation
/// time is spent in and the renderer waits on those worker threads:
///
/// * the bounding box is **clipped to `area`** before a single cell is looked at, so the cells
///   outside this chunk are never visited (and `area` is never re-tested per cell); and
/// * the ellipsoid test is **scaled** rather than divided — `(dx/h)² + (dy/v)² + (dz/h)² < 1`
///   multiplied through by `h²v²` is the same test with three multiplications instead of three
///   divisions.
fn carve_ellipsoid(
    set: &mut impl FnMut(i32, i32, i32, Block),
    area: Area,
    x: f64,
    y: f64,
    z: f64,
    horizontal: f64,
    vertical: f64,
) {
    let (ax0, ax1) = area.x_bounds();
    let (az0, az1) = area.z_bounds();
    // A worm spends most of its length outside the chunk being carved, and this is the test that
    // spends nothing on those steps: no cell, no lerp, no lookup.
    let (x0, x1) = (
        ((x - horizontal).floor() as i32).max(ax0),
        ((x + horizontal).floor() as i32).min(ax1),
    );
    let (z0, z1) = (
        ((z - horizontal).floor() as i32).max(az0),
        ((z + horizontal).floor() as i32).min(az1),
    );
    if x1 < x0 || z1 < z0 {
        return;
    }
    // Clipped to the world's height too: a worm may wander above or below it, and there is nothing
    // there to carve.
    let y0 = ((y - vertical).floor() as i32).max(0);
    let y1 = ((y + vertical).floor() as i32).min(HEIGHT as i32 - 1);

    let (h_squared, v_squared) = (horizontal * horizontal, vertical * vertical);
    let limit = h_squared * v_squared;

    for cz in z0..=z1 {
        let dz = f64::from(cz) + 0.5 - z;
        let along = dz * dz * v_squared;
        for cx in x0..=x1 {
            let dx = f64::from(cx) + 0.5 - x;
            let across = along + dx * dx * v_squared;
            for cy in y0..=y1 {
                let dy = f64::from(cy) + 0.5 - y;
                if across + dy * dy * h_squared < limit {
                    set(cx, cy, cz, Block::Air);
                }
            }
        }
    }
}

/// Carve **only** the caves over `area`.
///
/// The two carvers are separate functions and are kept reachable apart so that a test can measure
/// one of them without the other: how much ground each cuts, and what shape it leaves.
#[cfg(test)]
fn carve_caves_area(seed: u32, area: Area, set: &mut impl FnMut(i32, i32, i32, Block)) {
    let dice = Dice::new(seed);
    for origin in area.origins() {
        carve_caves(&mut dice.at(origin), origin, area, set);
    }
}

/// Carve **only** the ravines over `area`; see [`carve_caves_area`].
#[cfg(test)]
fn carve_ravines_area(seed: u32, area: Area, set: &mut impl FnMut(i32, i32, i32, Block)) {
    let dice = Dice::new(seed);
    for origin in area.origins() {
        carve_ravines(&mut dice.at(origin), origin, area, set);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    /// A patch of chunks big enough to hold a ravine (one origin in fifty grows one, so a 4 × 4
    /// patch expects a little over one) and small enough to keep the tests quick.
    const PATCH: i32 = 4;

    /// A bigger patch for the ravine tests: a ravine is ~90 blocks long and one origin in fifty
    /// grows one, so asking about 4 × 4 chunks can easily come up empty — and a rate needs enough
    /// world behind it to mean something.
    const RAVINE_PATCH: i32 = 8;

    type Cells = HashSet<(i32, i32, i32)>;

    /// The cells one pass carves, as a set — the form two passes can be compared in.
    fn carved_cells(seed: u32, area: Area) -> Cells {
        let mut cells = Cells::new();
        carve_area(seed, area, &mut |x, y, z, block| {
            // A carver cuts emptiness and nothing else; if that ever stops being true, every shape
            // test below is measuring something it does not think it is.
            assert_eq!(block, Block::Air, "a carver cut something other than air");
            cells.insert((x, y, z));
        });
        cells
    }

    /// Only the caves of a patch, and only the ravines — one carver at a time, so a test can ask
    /// what each of them is doing on its own.
    fn caves_in(seed: u32, area: Area) -> Cells {
        let mut cells = Cells::new();
        carve_caves_area(seed, area, &mut |x, y, z, _| {
            cells.insert((x, y, z));
        });
        cells
    }

    fn ravines_in(seed: u32, area: Area) -> Cells {
        let mut cells = Cells::new();
        carve_ravines_area(seed, area, &mut |x, y, z, _| {
            cells.insert((x, y, z));
        });
        cells
    }

    /// The carved cells lying inside one chunk.
    fn in_chunk(cells: &Cells, chunk: ChunkPos) -> Cells {
        let (x0, z0) = (chunk.0 * WIDTH as i32, chunk.1 * DEPTH as i32);
        let (x1, z1) = (x0 + WIDTH as i32 - 1, z0 + DEPTH as i32 - 1);
        cells
            .iter()
            .copied()
            .filter(|&(x, _, z)| (x0..=x1).contains(&x) && (z0..=z1).contains(&z))
            .collect()
    }

    /// The patch, as an area, and a wider one around it that no worm can fill.
    fn patch() -> Area {
        Area::of_chunks((0, 0), (PATCH - 1, PATCH - 1))
    }

    /// The wider patch the ravine tests carve, for the same reason.
    fn ravine_patch() -> Area {
        Area::of_chunks((0, 0), (RAVINE_PATCH - 1, RAVINE_PATCH - 1))
    }

    #[test]
    fn the_same_seed_carves_the_same_caves() {
        let area = Area::of_chunk((0, 0));
        assert_eq!(carved_cells(7, area), carved_cells(7, area));
        assert_ne!(
            carved_cells(7, area),
            carved_cells(8, area),
            "a different seed must put its caves somewhere else"
        );
    }

    /// The seam test, and the reason the carvers are built the way they are: a chunk's own cells are
    /// the same whether or not the chunks beside it were carved in the same pass.
    ///
    /// A worm that stopped at a chunk edge, an origin left out of the neighbourhood, or a clip that
    /// leaked would all show up here — and *nowhere else*, because carving a chunk on its own looks
    /// perfectly reasonable however badly it is clipped. It is the test that keeps streaming
    /// seamless: the chunks either side of a border are generated by different threads, at
    /// different times, and a cave that disagreed about the cells in between would leave a wall
    /// standing in the middle of a tunnel.
    #[test]
    fn a_worm_carves_the_same_cells_from_either_side_of_a_border() {
        for seed in [7_u32, 1337, 0xDEAD_BEEF] {
            for chunk in [(0, 0), (1, 0), (0, 1), (-2, 3)] {
                let alone = carved_cells(seed, Area::of_chunk(chunk));
                let together = in_chunk(
                    &carved_cells(
                        seed,
                        Area::of_chunks((chunk.0 - 2, chunk.1 - 2), (chunk.0 + 2, chunk.1 + 2)),
                    ),
                    chunk,
                );
                assert!(!alone.is_empty(), "the patch should have caves to compare");
                assert_eq!(alone, together, "chunk {chunk:?} was carved differently");
            }
        }
    }

    /// A pass writes its own chunk and no other: the neighbouring chunk's cells are somebody else's
    /// business, and carving them twice is at best wasted work.
    #[test]
    fn a_pass_writes_only_inside_its_own_area() {
        let area = Area::of_chunks((0, 0), (1, 1));
        for (x, y, z) in carved_cells(1337, area) {
            assert!((0..32).contains(&x), "x = {x} left the area");
            assert!((0..32).contains(&z), "z = {z} left the area");
            assert!((0..HEIGHT as i32).contains(&y), "y = {y} left the world");
        }
    }

    /// The longest *unbroken* vertical run of carved cells through any one column: how deep a
    /// carver cuts at its deepest.
    ///
    /// Unbroken, because a column may be crossed by two unrelated tunnels at different heights and
    /// that is not one deep cut. It is the shape a player sees when they look down a shaft.
    fn deepest_column(cells: &Cells) -> i32 {
        let mut columns: HashMap<(i32, i32), Vec<i32>> = HashMap::new();
        for &(x, y, z) in cells {
            columns.entry((x, z)).or_default().push(y);
        }
        columns
            .into_values()
            .map(|mut heights| {
                heights.sort_unstable();
                let (mut best, mut run) = (0, 0);
                let mut previous = i32::MIN;
                for y in heights {
                    run = if y == previous + 1 { run + 1 } else { 1 };
                    previous = y;
                    best = best.max(run);
                }
                best
            })
            .max()
            .unwrap_or(0)
    }

    /// How far, in chunks, the cells of one worm stray from the origin that grew it.
    fn farthest_chunk(cells: &Cells, origin: ChunkPos) -> i32 {
        cells
            .iter()
            .map(|&(x, _, z)| {
                (chunk_of(x) - origin.0)
                    .abs()
                    .max((chunk_of(z) - origin.1).abs())
            })
            .max()
            .unwrap_or(0)
    }

    /// Which of the two carvers to grow.
    #[derive(Clone, Copy, Debug)]
    enum Carver {
        Caves,
        Ravines,
    }

    impl Carver {
        /// Grow this carver at one origin, offering every cell it cuts.
        fn grow(
            self,
            seed: u32,
            origin: ChunkPos,
            area: Area,
            set: &mut impl FnMut(i32, i32, i32, Block),
        ) {
            let mut rand = Dice::new(seed).at(origin);
            match self {
                Carver::Caves => carve_caves(&mut rand, origin, area, set),
                Carver::Ravines => carve_ravines(&mut rand, origin, area, set),
            }
        }
    }

    /// The first origin that grows anything, and the cells it cut.
    ///
    /// Both carvers are gated — one origin in [`CAVE_GATE`] grows a cave, one in [`RAVINE_GATE`] a
    /// ravine — so a test that wants one has to go and look for it rather than expect the first
    /// origin it tries to have one.
    fn first_carver(seed: u32, carver: Carver, area: Area) -> (ChunkPos, Cells) {
        for x in 0..64 {
            for z in 0..64 {
                let origin = (x, z);
                let mut cells = Cells::new();
                carver.grow(seed, origin, area, &mut |cx, cy, cz, _| {
                    cells.insert((cx, cy, cz));
                });
                if !cells.is_empty() {
                    return (origin, cells);
                }
            }
        }
        panic!("no {carver:?} grew in a 64 × 64 sweep of origins");
    }

    /// Every worm stays within `RANGE` chunks of the origin that grew it.
    ///
    /// This is the other half of the seam argument (see the module docs): a chunk asks only the
    /// origins around it, and that is sound *because* nothing can reach in from further out. The
    /// area carved below is deliberately two chunks wider than the claim, so a worm that overran
    /// would have somewhere to overrun into — and be caught for it.
    #[test]
    fn caves_and_ravines_stay_within_reach() {
        let reach = RANGE + 2;
        let (mut travelled, mut cut) = (0, 0);
        for seed in [1_u32, 1337, 0xDEAD_BEEF] {
            // A sweep rather than a handful of origins, because most origins grow nothing at all.
            for x in 0..12 {
                for z in 0..12 {
                    let origin = (x, z);
                    let area = Area::of_chunks(
                        (origin.0 - reach, origin.1 - reach),
                        (origin.0 + reach, origin.1 + reach),
                    );
                    let mut wander = |wx: i32, y: i32, wz: i32, _: Block| {
                        let out = (chunk_of(wx) - origin.0)
                            .abs()
                            .max((chunk_of(wz) - origin.1).abs());
                        travelled = travelled.max(out);
                        cut += 1;
                        assert!(
                            out <= RANGE,
                            "a worm from {origin:?} reached ({wx}, {y}, {wz}), {out} chunks out"
                        );
                    };
                    Carver::Caves.grow(seed, origin, area, &mut wander);
                    Carver::Ravines.grow(seed, origin, area, &mut wander);
                }
            }
        }
        assert!(cut > 0, "no worms grew at all — the test proves nothing");
        assert!(
            travelled >= 2,
            "nothing travelled further than {travelled} chunks, so the reach went untested"
        );
    }

    /// A ravine is a shaft, not a dent: it cuts deeper than a cave does.
    ///
    /// The two carvers differ in one number — the stretch applied to the carve — and this is what
    /// that number buys. A cave is [`CAVE_FLATTEN`] times as tall as it is wide, so however thick a
    /// cave worm grows it can never cut a tall column; a ravine is stretched the other way, and a
    /// thick one cuts a shaft more than twice the depth of the widest cave above it.
    #[test]
    fn a_ravine_cuts_deeper_than_a_cave_does() {
        let across = Area::of_chunks((-RANGE, -RANGE), (RANGE, RANGE));
        let (_, ravine) = first_carver(1337, Carver::Ravines, across);
        // A whole patch of caves, not one worm: the claim is that a ravine beats the *best* a cave
        // carver manages, which makes it a claim about the two carvers rather than about two worms.
        let cave = deepest_column(&caves_in(1337, patch()));

        let ravine = deepest_column(&ravine);
        assert!(ravine >= 20, "the deepest ravine column is only {ravine}");
        assert!(
            ravine > cave,
            "a ravine ({ravine}) must out-cut a cave ({cave})"
        );
    }

    /// Ravines are the rare carver: compared over the same landscape, caves cut several times the
    /// ground ravines do.
    ///
    /// Which is what stops the world reading as one huge canyon — and the reason the gate numbers in
    /// this module are what they are. Rates are compared *per chunk*, so the two carvers are asked
    /// about the same amount of world rather than the same number of origins.
    #[test]
    fn ravines_are_rarer_than_caves() {
        let caves = caves_in(1337, patch()).len() as f64 / f64::from(PATCH * PATCH);
        let ravines =
            ravines_in(1337, ravine_patch()).len() as f64 / f64::from(RAVINE_PATCH * RAVINE_PATCH);
        assert!(caves > 0.0, "the patch has no caves at all");
        assert!(ravines > 0.0, "the patch has no ravines at all");
        assert!(
            ravines * 3.0 < caves,
            "{ravines:.0} ravine cells a chunk against {caves:.0} cave cells is not rare enough"
        );
    }

    /// Caves leave most of the rock alone: they thread it rather than hollow it, so the ground the
    /// terrain pass laid down is still ground, and a cave is something you find rather than the
    /// absence of anything to find.
    #[test]
    fn caves_leave_most_of_the_rock_alone() {
        let area = patch();
        let band = 1..=60;
        let volume = (PATCH * WIDTH as i32) * (PATCH * DEPTH as i32) * band.clone().count() as i32;
        let cut = caves_in(1337, area)
            .iter()
            .filter(|&&(_, y, _)| band.contains(&y))
            .count() as i32;
        let fraction = f64::from(cut) / f64::from(volume);
        assert!(
            (0.005..0.25).contains(&fraction),
            "{cut} of {volume} cells cut is {:.1}%, not the threading a cave should be",
            fraction * 100.0
        );
    }

    /// A worm is the *same worm* to every chunk in the neighbourhood: seeded from its origin, it
    /// cuts the same cells whoever asks for them.
    ///
    /// `a_worm_carves_the_same_cells_from_either_side_of_a_border` checks what a caller *keeps*;
    /// this checks the worm itself, by cutting it once across a wide area and then once into each of
    /// the chunks it passes through, and finding the two agree cell for cell in every one of them.
    /// (Those chunks are asked with a *narrow* area, exactly as a chunk being generated would be.)
    #[test]
    fn a_worm_is_the_same_worm_to_every_chunk() {
        let across = Area::of_chunks((-RANGE, -RANGE), (RANGE, RANGE));
        for seed in [7_u32, 1337] {
            let (origin, whole) = first_carver(seed, Carver::Caves, across);

            let mut chunks: Vec<ChunkPos> = whole
                .iter()
                .map(|&(x, _, z)| (chunk_of(x), chunk_of(z)))
                .collect();
            chunks.sort_unstable();
            chunks.dedup();
            assert!(
                chunks.len() > 1,
                "the worm should reach more than one chunk"
            );

            for chunk in chunks {
                let mut here = Cells::new();
                Carver::Caves.grow(seed, origin, Area::of_chunk(chunk), &mut |x, y, z, _| {
                    here.insert((x, y, z));
                });
                let expected: Cells = whole
                    .iter()
                    .copied()
                    .filter(|&(x, _, z)| (chunk_of(x), chunk_of(z)) == chunk)
                    .collect();
                assert_eq!(here, expected, "chunk {chunk:?} saw a different worm");
            }
            assert!(
                farthest_chunk(&whole, origin) >= 2,
                "the worm barely left its origin, so this proves little"
            );
        }
    }
}
