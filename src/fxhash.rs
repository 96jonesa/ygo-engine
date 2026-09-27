//! A small, fast, **deterministic** hasher for the port's maps.
//!
//! The standard `RandomState` is SipHash with a per-process random seed:
//! secure against collision attacks the engine will never face, slow on the
//! four-byte keys the engine actually hashes, and — the part that matters
//! for a differential harness — different every run, so any iteration over a
//! standard `HashMap` would be nondeterministic. This is the Fx hash used by
//! rustc: one multiply and a rotate per word, the same value every process.

use std::hash::{BuildHasherDefault, Hasher};

/// The multiplicative constant rustc's `FxHasher` uses on 64-bit targets.
const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

#[derive(Default, Clone, Copy)]
pub struct FxHasher {
    hash: u64,
}

impl FxHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let (chunks, rest) = bytes.as_chunks::<8>();
        for c in chunks {
            self.add(u64::from_le_bytes(*c));
        }
        if !rest.is_empty() {
            let mut buf = [0u8; 8];
            buf[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(buf));
        }
    }
    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(u64::from(i));
    }
    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add(u64::from(i));
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(u64::from(i));
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

pub type FxBuild = BuildHasherDefault<FxHasher>;
pub type FxHashMap<K, V> = std::collections::HashMap<K, V, FxBuild>;

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic across processes, and different for different keys —
    /// the two properties the maps need.
    #[test]
    fn the_hash_is_a_pure_function_of_the_key() {
        let h = |v: u32| {
            let mut x = FxHasher::default();
            x.write_u32(v);
            x.finish()
        };
        assert_eq!(h(0x1234), h(0x1234));
        assert_ne!(h(0x1234), h(0x1235));
        assert_eq!(
            h(7),
            (7u64).wrapping_mul(SEED),
            "one word: rotate of zero, xor, multiply"
        );
    }
}
