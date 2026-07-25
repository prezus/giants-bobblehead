//! Mono-to-stereo PCM expansion for the I2S transmit buffer.
//!
//! The MAX98357A takes a standard stereo I2S stream, but the embedded clips are
//! mono to halve the flash they occupy, so each mono sample is duplicated into
//! the left and right slots of a stereo frame as it is copied into the DMA
//! buffer.
//!
//! Kept free of `esp-hal` and of crate-internal imports so it can be unit-tested
//! on the host — see `just test`.

/// Bytes per stereo frame: two 16-bit channels.
pub const BYTES_PER_FRAME: usize = 4;

/// Bytes per mono sample: one 16-bit channel.
const BYTES_PER_SAMPLE: usize = 2;

/// Number of whole mono samples in a raw PCM buffer.
#[must_use]
pub const fn sample_count(pcm: &[u8]) -> usize {
    pcm.len() / BYTES_PER_SAMPLE
}

/// Fill `buf` with stereo frames expanded from mono `pcm`, starting at mono
/// sample `sample`. Returns the number of bytes written, always a whole number
/// of frames.
///
/// Returns 0 once `sample` reaches the end of `pcm`, or if `buf` cannot hold a
/// whole frame. Callers must treat 0 as "stop", never as "retry" — a retry loop
/// on a zero return spins without making progress.
///
/// # Panics
///
/// In debug builds, if `buf` is not a whole number of frames. Every buffer in
/// the DMA chain is 4-aligned ([`crate::board::DMA_BUF_BYTES`] is a multiple of 4
/// and this function only ever consumes whole frames), so a partial frame means
/// an upstream invariant broke rather than a case worth handling silently.
pub fn fill_stereo(buf: &mut [u8], pcm: &[u8], sample: usize) -> usize {
    debug_assert_eq!(
        buf.len() % BYTES_PER_FRAME,
        0,
        "DMA buffer must hold whole stereo frames"
    );

    let frames = buf.len() / BYTES_PER_FRAME;
    let n = core::cmp::min(frames, sample_count(pcm).saturating_sub(sample));

    for i in 0..n {
        let src = (sample + i) * BYTES_PER_SAMPLE;
        let lo = pcm[src];
        let hi = pcm[src + 1];
        let dst = i * BYTES_PER_FRAME;
        buf[dst] = lo; // left  low byte
        buf[dst + 1] = hi; // left  high byte
        buf[dst + 2] = lo; // right low byte
        buf[dst + 3] = hi; // right high byte
    }

    n * BYTES_PER_FRAME
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two mono samples: 0x1234 and 0x5678, little-endian.
    const MONO: &[u8] = &[0x34, 0x12, 0x78, 0x56];

    #[test]
    fn duplicates_each_mono_sample_into_both_channels() {
        let mut buf = [0u8; 8];
        assert_eq!(fill_stereo(&mut buf, MONO, 0), 8);
        assert_eq!(
            buf,
            [
                0x34, 0x12, 0x34, 0x12, // sample 0 in L and R
                0x78, 0x56, 0x78, 0x56, // sample 1 in L and R
            ]
        );
    }

    #[test]
    fn resumes_from_a_sample_offset() {
        let mut buf = [0u8; 4];
        assert_eq!(fill_stereo(&mut buf, MONO, 1), 4);
        assert_eq!(buf, [0x78, 0x56, 0x78, 0x56]);
    }

    #[test]
    fn writes_only_what_the_buffer_holds() {
        let mut buf = [0xAAu8; 4];
        // Buffer holds one frame; two samples remain.
        assert_eq!(fill_stereo(&mut buf, MONO, 0), 4);
        assert_eq!(buf, [0x34, 0x12, 0x34, 0x12]);
    }

    #[test]
    fn stops_at_the_end_of_the_clip() {
        let mut buf = [0xAAu8; 16];
        // Only two samples available, so only two frames written.
        assert_eq!(fill_stereo(&mut buf, MONO, 0), 8);
        // Remainder is left untouched for the caller to handle.
        assert_eq!(&buf[8..], &[0xAA; 8]);
    }

    #[test]
    fn returns_zero_when_the_clip_is_exhausted() {
        let mut buf = [0u8; 8];
        assert_eq!(fill_stereo(&mut buf, MONO, 2), 0);
    }

    /// Guards the spin-forever bug: an offset past the end must not underflow
    /// into a huge frame count.
    #[test]
    fn returns_zero_past_the_end_of_the_clip() {
        let mut buf = [0u8; 8];
        assert_eq!(fill_stereo(&mut buf, MONO, 999), 0);
    }

    #[test]
    fn returns_zero_for_an_empty_clip() {
        let mut buf = [0u8; 8];
        assert_eq!(fill_stereo(&mut buf, &[], 0), 0);
    }

    #[test]
    fn handles_an_empty_buffer() {
        assert_eq!(fill_stereo(&mut [], MONO, 0), 0);
    }

    #[test]
    fn ignores_a_trailing_odd_byte_in_the_clip() {
        let mut buf = [0u8; 8];
        let odd: &[u8] = &[0x34, 0x12, 0x99];
        assert_eq!(sample_count(odd), 1);
        assert_eq!(fill_stereo(&mut buf, odd, 0), 4);
    }

    #[test]
    #[should_panic(expected = "whole stereo frames")]
    fn rejects_a_misaligned_buffer_in_debug() {
        let mut buf = [0u8; 6];
        let _ = fill_stereo(&mut buf, MONO, 0);
    }
}
