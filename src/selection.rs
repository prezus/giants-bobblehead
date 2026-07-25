//! Which clip to play next.
//!
//! Kept free of `esp-hal` and of crate-internal imports so it can be unit-tested
//! on the host — see `just test`. The RTC-persistent storage this
//! feeds from lives in [`crate::clips`].
//!
//! The "last played" value is stored biased by one so that zero can mean "no
//! clip yet"; a true power-on zeroes RTC memory, and that must be
//! distinguishable from "clip 0 played last". [`decode_last`] and
//! [`encode_last`] own that convention so it isn't restated at each call site.

/// Decode a stored, one-biased clip marker into an index.
///
/// Returns `None` for the zero sentinel, meaning nothing has played yet.
#[must_use]
pub const fn decode_last(stored: u32, count: usize) -> Option<usize> {
    if stored == 0 || count == 0 {
        None
    } else {
        Some((stored - 1) as usize % count)
    }
}

/// Encode a clip index for storage, biased by one so zero stays free.
#[must_use]
pub const fn encode_last(index: usize) -> u32 {
    index as u32 + 1
}

/// Pick the next clip index, avoiding an immediate repeat.
///
/// `stored` is the one-biased marker from RTC memory (0 = nothing played yet),
/// `entropy` any random value, and `count` the number of available clips.
///
/// When something played last, the result is drawn uniformly from the `count - 1`
/// clips that are *not* the previous one: offsetting from `prev + 1` over a range
/// of `count - 1` can never land back on `prev`.
#[must_use]
pub const fn pick_next(stored: u32, entropy: u32, count: usize) -> usize {
    if count <= 1 {
        return 0;
    }
    let e = entropy as usize;
    match decode_last(stored, count) {
        None => e % count,
        Some(prev) => (prev + 1 + e % (count - 1)) % count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn treats_zero_as_nothing_played_yet() {
        assert_eq!(decode_last(0, 3), None);
    }

    #[test]
    fn round_trips_an_index_through_storage() {
        for index in 0..3 {
            assert_eq!(decode_last(encode_last(index), 3), Some(index));
        }
    }

    #[test]
    fn never_repeats_the_previous_clip() {
        let count = 3;
        for prev in 0..count {
            let stored = encode_last(prev);
            for entropy in 0..64 {
                let next = pick_next(stored, entropy, count);
                assert_ne!(next, prev, "prev={prev} entropy={entropy} repeated");
                assert!(next < count, "prev={prev} entropy={entropy} out of range");
            }
        }
    }

    /// Which of `count` indices `pick_next` produces across a sweep of entropy.
    fn reachable(stored: u32, count: usize) -> [bool; 8] {
        let mut seen = [false; 8];
        for entropy in 0..64 {
            seen[pick_next(stored, entropy, count)] = true;
        }
        seen
    }

    #[test]
    fn can_reach_every_other_clip() {
        let count = 3;
        let prev = 0;
        let seen = reachable(encode_last(prev), count);
        assert!(!seen[prev], "picked the previous clip");
        for index in 1..count {
            assert!(seen[index], "clip {index} unreachable after prev={prev}");
        }
    }

    #[test]
    fn first_pick_can_be_any_clip() {
        let count = 3;
        let seen = reachable(0, count);
        for index in 0..count {
            assert!(seen[index], "clip {index} unreachable on first pick");
        }
    }

    #[test]
    fn stays_in_range_for_a_single_clip() {
        for entropy in 0..8 {
            assert_eq!(pick_next(0, entropy, 1), 0);
            assert_eq!(pick_next(encode_last(0), entropy, 1), 0);
        }
    }

    /// `CLIPS` being empty is a compile error in `clips.rs`, but the arithmetic
    /// here must not divide by zero even so.
    #[test]
    fn survives_a_zero_count() {
        assert_eq!(pick_next(0, 7, 0), 0);
        assert_eq!(decode_last(1, 0), None);
    }
}
