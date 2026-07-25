//! I2S audio playback to the MAX98357A amplifier.
//!
//! Clips live in flash, which the DMA engine can't read, so we stream each clip
//! through a RAM DMA buffer. The MAX98357A takes a standard stereo I2S stream,
//! so we expand our mono clips to stereo on the fly (same sample in L and R).
//!
//! Playback uses one **continuous (circular) DMA transfer** for the whole awake
//! session rather than a fresh transfer per clip. One-shot-per-chunk hangs on
//! this chip after the first transfer; and the circular transfer consumes the
//! I2S driver (no way to hand it back), so a single long-lived transfer is also
//! what lets us play several clips in a row — needed for button interrupts.

use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Timer};
use esp_hal::Async;
use esp_hal::gpio::Input;
use esp_hal::i2s::master::I2sTx;

/// Debounce window after a button press before we watch for the next one, so
/// contact bounce doesn't skip several clips at once.
const DEBOUNCE_MS: u64 = 40;

/// Fill `buf` with stereo frames expanded from mono `pcm` starting at mono
/// sample `sample`. Returns the number of bytes written (whole frames only).
fn fill_stereo(buf: &mut [u8], pcm: &[u8], sample: usize) -> usize {
    let total_samples = pcm.len() / 2;
    let frames = buf.len() / 4; // 4 bytes per stereo 16-bit frame
    let n = core::cmp::min(frames, total_samples - sample);
    for i in 0..n {
        let src = (sample + i) * 2;
        let lo = pcm[src];
        let hi = pcm[src + 1];
        let dst = i * 4;
        buf[dst] = lo; // left  low byte
        buf[dst + 1] = hi; // left  high byte
        buf[dst + 2] = lo; // right low byte
        buf[dst + 3] = hi; // right high byte
    }
    n * 4
}

/// Run one awake playback session on a single continuous DMA transfer.
///
/// Plays `first`, then loops: a **button press interrupts the current clip and
/// starts the next one** from `next_clip`. Returns once a clip plays all the way
/// through without being interrupted — the caller can then arm the button as an
/// ext0 wake source and deep-sleep.
///
/// - `i2s_tx` / `dma_buf`: the I2S transmitter and its `dma_buffers!` buffer,
///   both consumed by the circular transfer for the session's lifetime.
/// - `button`: the trigger, already configured active-low. `wait_for_falling_edge`
///   fires on a press.
/// - `next_clip`: returns the raw PCM of the next clip to play on each press.
pub async fn session(
    i2s_tx: I2sTx<'static, Async>,
    dma_buf: &'static mut [u8],
    button: &mut Input<'_>,
    first: &'static [u8],
    mut next_clip: impl FnMut() -> &'static [u8],
) {
    let buf_len = dma_buf.len();
    let mut xfer = match i2s_tx.write_dma_circular_async(dma_buf) {
        Ok(x) => x,
        Err(e) => {
            log::warn!("i2s start failed: {e:?}");
            return;
        }
    };

    let mut pcm = first;
    loop {
        let total_samples = pcm.len() / 2;
        let mut sample = 0usize;

        // Stream this clip, but let a button press cut it short. `select` polls
        // both: the press fires independently of `available()`, so interrupts
        // are near-instant (only the ~one-buffer of already-queued audio still
        // plays out before the next clip is heard).
        let stream = async {
            while sample < total_samples {
                match xfer.push_with(|buf| fill_stereo(buf, pcm, sample)).await {
                    Ok(n) => sample += n / 4, // bytes / 4 = frames = mono samples
                    Err(e) => {
                        log::warn!("i2s push failed at sample {sample}: {e:?}");
                        break;
                    }
                }
            }
        };

        if let Either::First(()) = select(stream, button.wait_for_falling_edge()).await {
            // Clip played out on its own. Push one buffer of silence so the
            // circular DMA doesn't loop the tail, then return to deep-sleep.
            let mut flushed = 0usize;
            while flushed < buf_len {
                match xfer
                    .push_with(|buf| {
                        let n = core::cmp::min(buf.len(), buf_len - flushed);
                        buf[..n].fill(0);
                        n
                    })
                    .await
                {
                    Ok(n) => flushed += n,
                    Err(_) => break,
                }
            }
            return;
        }

        // Button pressed: debounce, then start the next clip. A held button
        // won't re-trigger until it's released and pressed again (no new
        // falling edge), so we don't need to wait for release here.
        Timer::after(Duration::from_millis(DEBOUNCE_MS)).await;
        pcm = next_clip();
    }
}
