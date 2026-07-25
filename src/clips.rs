//! Embedded announcer sound bites, and the RTC-persistent record of which one
//! played last.
//!
//! Clips are baked into the firmware with [`include_bytes!`] as **raw mono,
//! 16-bit signed, little-endian PCM** at [`crate::board::SAMPLE_RATE`] — no WAV
//! header. To add a clip, drop an MP3 in `assets/`, run `just convert` to
//! generate the matching `.pcm`, then add a [`Clip`] entry below.
//!
//! # Size budget
//!
//! Clips are linked into the app image, which lives in the **6 MB** `factory`
//! partition (see `partitions.csv`) — *not* the full 8 MB of flash. Each second
//! of audio is ~44 KB, and the code itself is well under 100 KB, so the practical
//! ceiling is about **140 seconds of audio in total**. `just size` reports what
//! the current build uses.
//!
//! The picking logic lives in [`crate::selection`], which is kept pure so it can
//! be host-tested; this module owns the storage it reads from.

use portable_atomic::{AtomicU32, Ordering};

use crate::selection;

/// One embedded sound bite: a name (for logging) and its raw PCM bytes.
#[derive(Clone, Copy, Debug)]
pub struct Clip {
    /// Human-readable label, logged when the clip plays.
    pub name: &'static str,
    /// Raw mono i16 little-endian PCM at [`crate::board::SAMPLE_RATE`].
    pub pcm: &'static [u8],
}

/// All clips. A button press plays one at random, never the same one twice in a
/// row — see [`crate::selection::pick_next`].
///
/// The `.pcm` files are generated from the `.mp3` sources in `assets/` by
/// `just convert`; both are committed so a clone builds without `ffmpeg`.
pub static CLIPS: &[Clip] = &[
    Clip {
        name: "bonds",
        pcm: include_bytes!("../assets/bonds.pcm"),
    },
    Clip {
        name: "izzy-pine",
        pcm: include_bytes!("../assets/izzy-pine.pcm"),
    },
    Clip {
        name: "bumgarner",
        pcm: include_bytes!("../assets/bumgarner.pcm"),
    },
];

/// Number of embedded clips.
pub const COUNT: usize = CLIPS.len();

/// The clip played on a true power-on, before any random pick has happened.
///
/// Kept deterministic so plugging the board in gives a predictable result. The
/// `const` assertion below fails the build if this stops pointing at the clip it
/// names, so reordering [`CLIPS`] can't silently change first-boot behaviour.
pub const FIRST_BOOT: usize = 1;

const _: () = assert!(!CLIPS.is_empty(), "CLIPS must contain at least one clip");
const _: () = assert!(
    FIRST_BOOT < COUNT,
    "FIRST_BOOT must be a valid index into CLIPS"
);
const _: () = assert!(
    str_eq(CLIPS[FIRST_BOOT].name, "izzy-pine"),
    "FIRST_BOOT must point at izzy-pine; update it or the doc comment above"
);

/// `str` equality usable in a `const` assertion.
const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Which clip played last, stored one-biased so 0 means "none yet".
///
/// Lives in RTC fast RAM so it survives the deep-sleep reset between presses; a
/// true power-on zeroes it, which lets the first pick be [`FIRST_BOOT`].
///
/// # Retention depends on an esp-hal implementation detail
///
/// `RtcSleepConfig::deep()` *intends* to power down both RTC memories, but in
/// esp-hal 1.1.1 (`src/rtc_cntl/sleep/esp32.rs`) only the `slowmem_pd_en` write
/// actually happens — the `fastmem_pd_en` line is commented out under an upstream
/// `TODO`. So `rtc_fast` is retained and `rtc_slow` is *not*, which is the
/// opposite of what you would expect and the reason this uses `rtc_fast`.
///
/// If that TODO is ever resolved, this silently reads 0 on every wake and every
/// press replays [`FIRST_BOOT`]. `esp-hal` is pinned exactly in `Cargo.toml` for
/// this reason; re-test "press twice, hear two different clips" after any bump.
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static LAST_CLIP: AtomicU32 = AtomicU32::new(0);

/// The index of the clip that played most recently, or `None` after a power-on.
#[must_use]
pub fn last_played() -> Option<usize> {
    let stored = LAST_CLIP.load(Ordering::Relaxed);
    selection::decode_last(stored, COUNT)
}

/// Record `index` as the most recently played clip.
pub fn set_last_played(index: usize) {
    LAST_CLIP.store(selection::encode_last(index), Ordering::Relaxed);
}

/// Pick the next clip, avoiding an immediate repeat, and record the choice.
///
/// `entropy` should be freshly random on each call.
pub fn advance(entropy: u32) -> &'static Clip {
    let mut stored = LAST_CLIP.load(Ordering::Relaxed);
    loop {
        let index = selection::pick_next(stored, entropy, COUNT);
        let encoded = selection::encode_last(index);
        match LAST_CLIP.compare_exchange_weak(stored, encoded, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => return &CLIPS[index],
            Err(current) => stored = current,
        }
    }
}
