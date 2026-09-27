//! The duel's random source: Xoshiro256**.
//!
//! Ported bit for bit, because a differential harness needs the two engines
//! to make the *same* random choices, not merely both to be random.
//!
//! ## `DUEL_PSEUDO_SHUFFLE` does not make a duel deterministic
//!
//! It is easy to read the flag as "no randomness", and this project's
//! configuration sets it. What it actually does is narrower: `field::shuffle`
//! skips the shuffle **only for the deck**.
//!
//! ```text
//! if(location == LOCATION_HAND || !is_flag(DUEL_PSEUDO_SHUFFLE)) { ...shuffle... }
//! ```
//!
//! The hand is shuffled regardless. Card sequence within a hand is
//! observable — it is how cards are addressed — so two engines that disagree
//! about the RNG disagree about which card a selection refers to, even under
//! pseudo-shuffle. The reference also rolls for coin tosses, dice and
//! excavation.
//!
//! So the generator is part of the specification, not part of the test
//! harness.

/// `RNG::Xoshiro256StarStar`.
///
/// The upstream algorithm, unchanged. Rust's wrapping arithmetic is spelled
/// explicitly where C++'s `uint64_t` wraps silently — that is the only
/// difference, and it is a difference in notation rather than in result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Xoshiro256StarStar {
    s: [u64; 4],
}

impl Default for Xoshiro256StarStar {
    /// An all-zero state is a *fixed point* of this generator — it emits
    /// zero forever — so the default is a fixed non-zero state rather than
    /// `[0; 4]`. Any duel that cares about its rolls should pass a seed.
    fn default() -> Self {
        Self::new([0x2545_F491_4F6C_DD1D, 1, 2, 3])
    }
}

impl Xoshiro256StarStar {
    pub const fn new(state: [u64; 4]) -> Self {
        Self { s: state }
    }

    /// A generator from one word, its state expanded by [`expand_seed`].
    /// For callers that hold a single integer seed — a solver's per-sample
    /// or per-rollout draw — where the duel itself takes four words.
    pub fn from_u64(seed: u64) -> Self {
        Self::new(expand_seed(seed))
    }

    pub const fn min() -> u64 {
        0
    }

    pub const fn max() -> u64 {
        u64::MAX
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;

        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];

        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);

        result
    }

    /// `duel::get_next_integer` — a uniform integer in `[low, high]`.
    ///
    /// The rejection loop is the reference's and is kept exactly, because
    /// *how many times it draws* is observable: a port that used a different
    /// unbiasing scheme would consume a different number of values and every
    /// later roll would diverge, even though both are uniform.
    ///
    /// Note the comparison is `n <= lim`, not `n < lim`. That asymmetry
    /// changes which draws are rejected, and therefore the whole sequence.
    pub fn next_integer(&mut self, low: i32, high: i32) -> i32 {
        debug_assert!(low <= high);
        let range = (i64::from(high) - i64::from(low) + 1) as u64;
        let lim = Self::max() % range;
        let mut n;
        loop {
            n = self.next();
            if n > lim {
                break;
            }
        }
        ((n % range) as i64 + i64::from(low)) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference states of the upstream algorithm produce known values.
    /// Pinned so a transcription slip in the shifts or rotations shows up
    /// here rather than as a divergent shuffle a thousand games later.
    #[test]
    fn the_generator_matches_the_published_algorithm() {
        let mut r = Xoshiro256StarStar::new([1, 2, 3, 4]);
        // Computed from the published xoshiro256** reference implementation
        // with the same starting state.
        let got: Vec<u64> = (0..4).map(|_| r.next()).collect();
        assert_eq!(got, vec![11520u64, 0, 1509978240, 1215971899390074240,]);
    }

    /// The state advances even when the output does not look like it has.
    #[test]
    fn a_zero_output_is_not_a_stuck_generator() {
        let mut r = Xoshiro256StarStar::new([1, 2, 3, 4]);
        let before = r.clone();
        r.next();
        assert_ne!(r, before);
    }

    /// An all-zero state emits zero forever, which is why the default is
    /// not `[0; 4]`.
    #[test]
    fn the_all_zero_state_is_a_fixed_point() {
        let mut zeroed = Xoshiro256StarStar::new([0; 4]);
        assert_eq!(zeroed.next(), 0);
        assert_eq!(zeroed.next(), 0);

        let mut default = Xoshiro256StarStar::default();
        assert_ne!(default.next(), 0, "the default is not that state");
    }

    #[test]
    fn bounds_are_the_full_range_of_a_u64() {
        assert_eq!(Xoshiro256StarStar::min(), 0);
        assert_eq!(Xoshiro256StarStar::max(), u64::MAX);
    }

    /// A single-value range needs no draw to be uniform, but the reference
    /// still draws — and the count of draws is what a differential harness
    /// depends on.
    #[test]
    fn a_single_value_range_still_consumes_a_draw() {
        let mut r = Xoshiro256StarStar::new([1, 2, 3, 4]);
        let before = r.clone();
        assert_eq!(r.next_integer(7, 7), 7);
        assert_ne!(r, before, "a draw was consumed");
    }

    #[test]
    fn values_stay_inside_the_range() {
        let mut r = Xoshiro256StarStar::new([0x1234, 0x5678, 0x9abc, 0xdef0]);
        for _ in 0..200 {
            let v = r.next_integer(-3, 9);
            assert!((-3..=9).contains(&v), "{v} out of range");
        }
    }
}

/// SplitMix64 over `seed`, four outputs: the xoshiro authors' recommended
/// way to fill a state from one word, so that nearby seeds give unrelated
/// states (a plain `[seed, 0, 0, 0]` would start every generator in the
/// same thin corner of the state space).
pub fn expand_seed(seed: u64) -> [u64; 4] {
    let mut x = seed;
    let mut next = || {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    [next(), next(), next(), next()]
}

#[cfg(test)]
mod seed_tests {
    use super::*;

    /// **SplitMix64, pinned.** The first output for seed 0 is the
    /// reference sequence's (Steele, Lea, Flood 2014), and nearby seeds
    /// give unrelated states.
    #[test]
    fn expand_seed_is_split_mix_64() {
        let s0 = expand_seed(0);
        assert_eq!(s0[0], 0xE220_A839_7B1D_CDAF);
        assert_eq!(s0[1], 0x6E78_9E6A_A1B9_65F4);
        let s1 = expand_seed(1);
        assert!(s0.iter().zip(&s1).all(|(a, b)| a != b));
        assert_ne!(
            Xoshiro256StarStar::from_u64(0).next(),
            Xoshiro256StarStar::from_u64(1).next()
        );
    }
}
