//! In-place shuffle order for the queue (Phase 17b).
//!
//! Shuffle is a *reorder*, not a hidden playback-order overlay (spec §6.1: the
//! queue view is the play order). Enabling it Fisher-Yates-shuffles the upcoming
//! tail of the queue; the played + currently-playing prefix stays put so the
//! shuffle only touches the future. The engine and the DB queue both apply the
//! *same* permutation this module produces, which is what keeps them lock-step
//! (the position-keyed invariant, playqueue.rs).
//!
//! Pure and dependency-free: a tiny SplitMix64 PRNG (the hand-rolled-difflib idiom
//! of `dedup.rs`, no `rand` crate) drives the shuffle, seeded by the caller so the
//! result is deterministic in tests. The permutation is `perm[new_index] =
//! old_index`, the shape [`crate::player::handle::PlayerCommand::ReorderQueue`]
//! and the worker's `reorder_queue_by_positions` both consume.

/// A minimal SplitMix64 generator: fast, allocation-free, good enough for
/// shuffling a play queue (not cryptographic). Seeded by the caller.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        // The canonical SplitMix64 step (constants from the reference impl).
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform value in `[0, n)` via Lemire's multiply-shift (no modulo bias
    /// worth caring about at queue sizes). `n` must be non-zero.
    fn below(&mut self, n: usize) -> usize {
        ((self.next_u64() as u128 * n as u128) >> 64) as usize
    }
}

/// The permutation to apply to a queue of `len` items to shuffle it in place,
/// leaving indices `[0, keep_prefix)` fixed (the played + currently-playing head)
/// and Fisher-Yates-shuffling `[keep_prefix, len)`. Returns `perm` with
/// `perm[new_index] = old_index`, always a permutation of `0..len`. `keep_prefix`
/// is clamped to `len`, so a `keep_prefix >= len` (nothing to shuffle) yields the
/// identity. `seed` makes it deterministic.
pub fn shuffle_order(len: usize, keep_prefix: usize, seed: u64) -> Vec<usize> {
    let mut perm: Vec<usize> = (0..len).collect();
    let start = keep_prefix.min(len);
    // Fisher-Yates over the tail [start, len): for i from the top down, swap with
    // a random j in [start, i].
    let mut rng = SplitMix64(seed);
    if len > start + 1 {
        for i in (start + 1..len).rev() {
            let span = i - start + 1; // size of [start, i]
            let j = start + rng.below(span);
            perm.swap(i, j);
        }
    }
    perm
}

/// Apply a `perm` (`new_index -> old_index`) to `items`, returning the reordered
/// vector. Shared by the engine's queue reorder so the permutation math lives in
/// one place. Panics only on a malformed perm (an out-of-range index), which the
/// generator above never produces.
pub fn apply_permutation<T: Clone>(items: &[T], perm: &[usize]) -> Vec<T> {
    perm.iter().map(|&old| items[old].clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn is_permutation(perm: &[usize], len: usize) -> bool {
        perm.len() == len && perm.iter().copied().collect::<HashSet<_>>().len() == len
    }

    #[test]
    fn result_is_always_a_permutation() {
        for len in [0, 1, 2, 5, 50] {
            for keep in [0, 1, len / 2] {
                let perm = shuffle_order(len, keep, 12345);
                assert!(is_permutation(&perm, len), "len={len} keep={keep}");
            }
        }
    }

    #[test]
    fn prefix_is_left_in_place() {
        let perm = shuffle_order(20, 5, 999);
        for (i, &old) in perm.iter().enumerate().take(5) {
            assert_eq!(old, i, "the kept prefix must stay identity");
        }
    }

    #[test]
    fn deterministic_per_seed() {
        assert_eq!(shuffle_order(30, 2, 42), shuffle_order(30, 2, 42));
        // A different seed almost surely differs (30! space).
        assert_ne!(shuffle_order(30, 2, 42), shuffle_order(30, 2, 43));
    }

    #[test]
    fn nothing_to_shuffle_is_identity() {
        // keep_prefix at/over the end, or a length-1 tail, cannot reorder.
        assert_eq!(shuffle_order(10, 10, 7), (0..10).collect::<Vec<_>>());
        assert_eq!(shuffle_order(10, 20, 7), (0..10).collect::<Vec<_>>());
        assert_eq!(shuffle_order(6, 5, 7), (0..6).collect::<Vec<_>>());
        assert_eq!(shuffle_order(0, 0, 7), Vec::<usize>::new());
    }

    #[test]
    fn actually_reorders_the_tail() {
        // With a big enough tail, some seed must move things off identity.
        let perm = shuffle_order(20, 0, 2024);
        assert_ne!(perm, (0..20).collect::<Vec<_>>(), "the tail should shuffle");
    }

    #[test]
    fn apply_permutation_reorders() {
        let items = vec!["a", "b", "c", "d"];
        // perm keeps 0, then reverses the rest.
        let perm = vec![0, 3, 2, 1];
        assert_eq!(apply_permutation(&items, &perm), vec!["a", "d", "c", "b"]);
    }
}
