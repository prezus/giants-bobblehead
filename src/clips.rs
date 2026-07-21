//! Embedded announcer sound bites.
//!
//! Clips are baked into the firmware with [`include_bytes!`] as **raw mono,
//! 16-bit signed, little-endian PCM** at [`crate::board::SAMPLE_RATE`] — no WAV
//! header. To add a real Giants clip, convert it and drop it in `assets/`:
//!
//! ```text
//! ffmpeg -i kruk_and_kuip.wav -ac 1 -ar 22050 -f s16le assets/kruk.pcm
//! ```
//!
//! then add a [`Clip`] entry below. Keep them short — flash is 8 MB but each
//! second of audio is ~44 KB.

/// One embedded sound bite: a name (for logging) and its raw PCM bytes.
pub struct Clip {
    /// Human-readable label, logged when the clip plays.
    pub name: &'static str,
    /// Raw mono i16 little-endian PCM at [`crate::board::SAMPLE_RATE`].
    pub pcm: &'static [u8],
}

/// All clips, played in round-robin order (see the RTC-persistent index in
/// `main.rs`). The `.pcm` files are generated from the `.mp3` sources in
/// `assets/` via `just convert`.
pub static CLIPS: &[Clip] = &[
    Clip {
        name: "bonds",
        pcm: include_bytes!("../assets/bonds.pcm"),
    },
    Clip {
        name: "pine",
        pcm: include_bytes!("../assets/pine.pcm"),
    },
];

/// Number of embedded clips.
pub const COUNT: usize = CLIPS.len();
