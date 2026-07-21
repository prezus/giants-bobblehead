//! I2S audio playback to the MAX98357A amplifier.
//!
//! Clips live in flash, which the DMA engine can't read, so we copy each chunk
//! into the DMA buffer in RAM. The MAX98357A takes a standard stereo I2S stream,
//! so we expand our mono clips to stereo on the fly (same sample in L and R).

use esp_hal::Async;
use esp_hal::i2s::master::I2sTx;

/// Stream a whole mono-i16 PCM clip out over I2S, once, then return.
///
/// - `i2s_tx`: the async I2S transmitter, already wired to BCLK/WS/DOUT.
/// - `dma_buf`: the DMA transmit buffer from `dma_buffers!` (in RAM). Its length
///   must be a multiple of 4 (one stereo 16-bit frame).
/// - `pcm`: raw mono, 16-bit signed, little-endian samples at the board sample
///   rate.
///
/// Each DMA transfer is a separate one-shot, so with long clips there can be a
/// brief gap between chunks; making `dma_buf` larger reduces how often that
/// happens. For gapless playback, switch to `write_dma_circular_async`.
pub async fn play(i2s_tx: &mut I2sTx<'_, Async>, dma_buf: &mut [u8], pcm: &[u8]) {
    // Frames we can fit per DMA transfer (4 bytes per stereo frame).
    let frames_per_chunk = dma_buf.len() / 4;
    if frames_per_chunk == 0 {
        return;
    }
    // Whole mono samples available (2 bytes each).
    let total_samples = pcm.len() / 2;

    let mut sample = 0;
    while sample < total_samples {
        let n = core::cmp::min(frames_per_chunk, total_samples - sample);

        // Copy mono samples from flash into the RAM DMA buffer, duplicating
        // each into the left and right channels.
        for i in 0..n {
            let src = (sample + i) * 2;
            let lo = pcm[src];
            let hi = pcm[src + 1];
            let dst = i * 4;
            dma_buf[dst] = lo; // left  low byte
            dma_buf[dst + 1] = hi; // left  high byte
            dma_buf[dst + 2] = lo; // right low byte
            dma_buf[dst + 3] = hi; // right high byte
        }

        // Ignore transfer errors: a glitched chunk shouldn't abort the whole
        // clip, and there's nothing to recover to mid-playback.
        let _ = i2s_tx.write_dma_async(&mut dma_buf[..n * 4]).await;

        sample += n;
    }
}
