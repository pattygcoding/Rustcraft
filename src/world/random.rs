//! `java.util.Random`, ported.
//!
//! Minecraft's cave and ravine generators were written against this exact generator, and its
//! *idioms* — `nextInt(nextInt(40) + 1)`, `nextFloat() - nextFloat()` — shape the terrain as much
//! as the shape of the carvers do. Porting the generator rather than reaching for a different hash
//! is what lets the carvers keep Minecraft's constants and read like the source they came from:
//! [`super::carve`]'s dice draw the same numbers Minecraft's `MapGenBase` drew.
//!
//! It is forty lines and no dependency, which is why it lives here rather than in a crate. Three
//! of its behaviours are quirks worth naming:
//!
//! * **Forty-eight bits of state**, multiplied and added to on every draw, so the stream repeats
//!   after 2^48 steps and no sooner. Plenty for dice.
//! * **`next(bits)` keeps the *top* bits** of the new state, which is why the low bits of this
//!   generator are famously poor and the high bits are the ones worth using.
//! * **`next_int_bound` rejects as well as divides.** Scaling a 31-bit draw down to `bound` leaves
//!   the first `2^31 mod bound` values one draw likelier than the rest; Java throws those away and
//!   draws again, and so does this.

/// The multiplier and addend of the congruential step, and the mask that keeps 48 bits of it.
///
/// Written out in full because the step is a *product*: the top bits of a 48-bit number times a
/// 35-bit one are what `MASK` is for.
const MULTIPLIER: u64 = 0x0005_DEEC_E66D;
const ADDEND: u64 = 0x0B;
const MASK: u64 = (1 << 48) - 1;

/// A deterministic dice stream, seeded and stepped exactly as `java.util.Random` is.
#[derive(Clone, Debug)]
pub struct JavaRandom {
    state: u64,
}

impl JavaRandom {
    /// A stream seeded with `seed`.
    ///
    /// The seed is scrambled on the way in, as Java's constructor does, so that nearby seeds give
    /// unrelated streams — which matters here, because our seeds are hashed chunk coordinates.
    pub fn new(seed: i64) -> Self {
        Self {
            state: (seed as u64 ^ MULTIPLIER) & MASK,
        }
    }

    /// The top `bits` of the next state: Java's `next`.
    fn next(&mut self, bits: u32) -> u32 {
        self.state = self.state.wrapping_mul(MULTIPLIER).wrapping_add(ADDEND) & MASK;
        (self.state >> (48 - bits)) as u32
    }

    /// A draw from the whole `i32` range, sign included.
    pub fn next_int(&mut self) -> i32 {
        self.next(32) as i32
    }

    /// A draw from `0..bound`, uniformly — `bound` must be positive.
    pub fn next_int_bound(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "bound must be positive");
        // A power of two needs no rejection: the high bits of a uniform draw are already uniform.
        if bound & (bound - 1) == 0 {
            return ((i64::from(bound) * i64::from(self.next(31))) >> 31) as i32;
        }
        loop {
            let bits = self.next(31) as i32;
            let value = bits % bound;
            // Reject the tail that would make the low values likelier than the high ones. The test
            // overflows exactly when `bits` fell in that tail, which is why it is a *wrapping*
            // arithmetic rather than a comparison against a constant.
            if bits.wrapping_sub(value).wrapping_add(bound - 1) >= 0 {
                return value;
            }
        }
    }

    /// A draw from `[0, 1)`, at Java's `float` precision: 24 bits, so every result is exactly
    /// representable and `1.0` itself can never come up.
    ///
    /// Dividing by `2^24` rather than multiplying by its reciprocal would be Java's own arithmetic,
    /// and both are *exact* here — `2^24` is a power of two, so the reciprocal is a power of two too
    /// and no bit is lost either way. The multiplication is what a step-by-step worm can afford.
    pub fn next_float(&mut self) -> f32 {
        self.next(24) as f32 * (1.0 / (1 << 24) as f32)
    }

    /// A draw from `[0, 1)` at `double` precision: 53 bits, from two draws, the way Java builds it.
    ///
    /// Nothing in the carvers wants more than `float` precision, so this is the one draw without a
    /// caller outside the tests — part of what *a port of `java.util.Random`* means, and what the
    /// next generator (ore veins, flower patches) will reach for.
    #[allow(
        dead_code,
        reason = "part of the ported generator, waiting for its first user"
    )]
    pub fn next_double(&mut self) -> f64 {
        let high = u64::from(self.next(26));
        let low = u64::from(self.next(27));
        ((high << 27) + low) as f64 / (1u64 << 53) as f64
    }

    /// A draw from the whole `i64` range, which is what Minecraft seeds its sub-generators with.
    pub fn next_long(&mut self) -> i64 {
        let high = i64::from(self.next_int());
        let low = i64::from(self.next_int());
        (high << 32).wrapping_add(low)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The expected values below came from the published generator — a 48-bit congruential step,
    /// top bits out — worked through by an independent implementation, and they agree with the
    /// values `java.util.Random` is widely documented to produce.
    #[test]
    fn next_int_matches_java() {
        assert_eq!(JavaRandom::new(0).next_int(), -1155484576);
        assert_eq!(JavaRandom::new(1).next_int(), -1155869325);
        assert_eq!(JavaRandom::new(42).next_int(), -1170105035);
    }

    #[test]
    fn next_double_and_next_float_match_java() {
        assert_eq!(JavaRandom::new(0).next_double(), 0.730967787376657);
        assert_eq!(JavaRandom::new(42).next_double(), 0.7275636800328681);
        assert_eq!(JavaRandom::new(0).next_float(), 0.730_967_76);
        assert_eq!(JavaRandom::new(42).next_float(), 0.727_563_7);
    }

    /// The rejection loop is the part of `next_int_bound` that is easy to get wrong — it eats a
    /// variable number of draws, so getting it wrong throws the whole stream out of step — and the
    /// power-of-two shortcut takes a *different* path through the same generator, which is why both
    /// are pinned. A non-power-of-two bound is checked twice: once from a fresh stream and once
    /// carried on from a previous one.
    #[test]
    fn next_int_bound_matches_java_across_a_stream() {
        let mut rng = JavaRandom::new(1337);
        let sixteens: Vec<i32> = (0..10).map(|_| rng.next_int_bound(16)).collect();
        assert_eq!(sixteens, [10, 2, 11, 13, 14, 15, 14, 12, 2, 10]);

        let mut rng = JavaRandom::new(42);
        let hundreds: Vec<i32> = (0..10).map(|_| rng.next_int_bound(100)).collect();
        assert_eq!(hundreds, [30, 63, 48, 84, 70, 25, 5, 18, 19, 93]);
        let sevens: Vec<i32> = (0..6).map(|_| rng.next_int_bound(7)).collect();
        assert_eq!(sevens, [3, 4, 0, 0, 1, 3]);
    }

    /// Whatever it draws, it stays inside what it promised: `bound` exclusive, `[0, 1)` exclusive,
    /// and a power-of-two bound not special-cased into a different range.
    #[test]
    fn every_draw_stays_in_its_range() {
        let mut rng = JavaRandom::new(-9_876_543_210);
        for _ in 0..1_000 {
            let bound = rng.next_int_bound(1_000) + 1;
            let drawn = rng.next_int_bound(bound);
            assert!((0..bound).contains(&drawn), "{drawn} out of 0..{bound}");
            let unit = rng.next_float();
            assert!((0.0..1.0).contains(&unit), "{unit} out of [0, 1)");
            let unit = rng.next_double();
            assert!((0.0..1.0).contains(&unit), "{unit} out of [0, 1)");
        }
        assert_eq!(rng.next_int_bound(1), 0, "a bound of one has one answer");
    }

    /// A stream is a function of its seed and nothing else, and two nearby seeds do not walk in
    /// step — which is what lets a carver be restarted from a seed at any moment.
    #[test]
    fn a_stream_is_reproducible_and_seeds_are_independent() {
        let draw = |seed| {
            let mut rng = JavaRandom::new(seed);
            (0..8).map(|_| rng.next_int()).collect::<Vec<_>>()
        };
        assert_eq!(draw(1337), draw(1337));
        assert_ne!(draw(1337), draw(1338), "seeds must not walk in step");
    }
}
