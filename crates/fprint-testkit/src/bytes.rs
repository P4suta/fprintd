// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A byte source, and the seeded generator that drives it at `cargo test` time.

/// A stream of bytes a generator draws from.
///
/// One required method, so an adapter over some other stream is a few lines. That is the whole
/// reason the trait exists: the same [`gen`](crate::gen) functions run from [`Lcg`] under
/// `cargo test` and from a fuzzer's input in `fuzz/`, and neither tool appears here.
pub trait ByteSource {
    /// Fill `buf` completely. A finite source repeats, pads or wraps — it may not leave `buf`
    /// short, because a generator's shape must not depend on how much input is left.
    fn fill(&mut self, buf: &mut [u8]);

    /// One byte.
    fn u8(&mut self) -> u8 {
        let mut b = [0u8; 1];
        self.fill(&mut b);
        b[0]
    }

    /// Four bytes, little-endian.
    fn u32(&mut self) -> u32 {
        let mut b = [0u8; 4];
        self.fill(&mut b);
        u32::from_le_bytes(b)
    }

    /// A value in `lo..=hi`. Panics if `lo > hi`.
    ///
    /// Modulo-biased, and deliberately so: a rejection loop would make the number of bytes drawn
    /// depend on their values, and a fuzzer's corpus would then be sensitive to where a generator
    /// happened to reject. The bias does not matter to any caller here — none is sampling a
    /// distribution, they are covering a range.
    fn in_range(&mut self, lo: i32, hi: i32) -> i32 {
        assert!(lo <= hi, "in_range({lo}, {hi}): empty range");
        // In `i64` throughout: the widest range spans 2^32, which no `i32` step can hold, and the
        // offset alone reaches `u32::MAX`.
        let span = i64::from(hi) - i64::from(lo) + 1;
        let offset = (u64::from(self.u32()) % span as u64) as i64;
        (i64::from(lo) + offset) as i32
    }

    /// True with probability about `n / d`. Panics if `d` is zero.
    fn ratio(&mut self, n: u32, d: u32) -> bool {
        assert!(d > 0, "ratio(_, 0): empty denominator");
        self.u32() % d < n
    }
}

/// A seeded linear congruential generator — the deterministic source for `cargo test`.
///
/// No RNG crate: the constants are the same ones this workspace's corpora generators roll, so a
/// test's inputs are reproducible from a `u64` on any machine, and a failure message can carry the
/// seed rather than the input.
pub struct Lcg {
    seed: u64,
    state: u64,
}

impl Lcg {
    /// A generator for `seed`.
    #[must_use]
    pub fn new(seed: u64) -> Lcg {
        Lcg { seed, state: seed }
    }

    /// The seed this was built from, for a failure message.
    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    fn next(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.state
    }
}

impl ByteSource for Lcg {
    fn fill(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(8) {
            let word = self.next().to_le_bytes();
            chunk.copy_from_slice(&word[..chunk.len()]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_bytes() {
        let mut a = Lcg::new(42);
        let mut b = Lcg::new(42);
        let (mut x, mut y) = ([0u8; 64], [0u8; 64]);
        a.fill(&mut x);
        b.fill(&mut y);
        assert_eq!(x, y);
        assert_eq!(a.seed(), 42);
    }

    #[test]
    fn different_seeds_diverge() {
        let (mut a, mut b) = (Lcg::new(1), Lcg::new(2));
        let (mut x, mut y) = ([0u8; 64], [0u8; 64]);
        a.fill(&mut x);
        b.fill(&mut y);
        assert_ne!(x, y);
    }

    #[test]
    fn fill_covers_lengths_that_are_not_a_multiple_of_eight() {
        // The chunk loop writes the last partial word by slice length; a short copy would panic or
        // leave a tail unwritten.
        for len in 0..24usize {
            let mut buf = vec![0xAAu8; len];
            Lcg::new(7).fill(&mut buf);
            assert_eq!(buf.len(), len);
        }
    }

    #[test]
    fn in_range_stays_in_range() {
        let mut lcg = Lcg::new(9);
        for _ in 0..2000 {
            let v = lcg.in_range(-5, 5);
            assert!((-5..=5).contains(&v), "{v} outside -5..=5");
        }
        // A single-value range is a range, not an error.
        assert_eq!(lcg.in_range(3, 3), 3);
        // The widest range spans 2^32 and its offset reaches u32::MAX. Neither fits an i32, so a
        // step taken there wraps and the result lands outside the range it was asked for.
        for _ in 0..2000 {
            let _ = lcg.in_range(i32::MIN, i32::MAX);
            let v = lcg.in_range(i32::MIN, 0);
            assert!(v <= 0, "{v} above 0");
            let v = lcg.in_range(0, i32::MAX);
            assert!(v >= 0, "{v} below 0");
        }
    }

    #[test]
    fn ratio_bounds_are_never_and_always() {
        let mut lcg = Lcg::new(11);
        for _ in 0..200 {
            assert!(!lcg.ratio(0, 4), "0/4 must never fire");
            assert!(lcg.ratio(4, 4), "4/4 must always fire");
        }
    }
}
