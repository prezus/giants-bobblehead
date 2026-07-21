//! Board configuration for the **Adafruit ESP32 Feather V2** (product 5400).
//!
//! # Wiring map
//!
//! Signal chain: `ESP32 --I2S--> MAX98357A amp (3006) --> 4Ω speaker (4445)`,
//! triggered by the IoT button (5666).
//!
//! | Feather pin | GPIO | Connects to                                   |
//! |-------------|------|-----------------------------------------------|
//! | D14         | 14   | MAX98357A **BCLK**                            |
//! | D15         | 15   | MAX98357A **LRC** (word select / WS)         |
//! | D32         | 32   | MAX98357A **DIN** (data)                     |
//! | D33         | 33   | MAX98357A **SD** (shutdown: HIGH = amp on)   |
//! | D27         | 27   | Button to **GND** (RTC-capable, ext0 wakeup) |
//! | 3V3 / GND   | —    | MAX98357A **Vin** / **GND**, speaker +/-      |
//!
//! ## Button pull-up (important for deep-sleep wakeup)
//! The button connects GPIO27 to GND (active-low), so the pin needs a pull-up
//! to read HIGH when idle. GPIO27 is RTC-capable (`RTC_GPIO17`) so ext0 can wake
//! the chip on a falling edge (`WakeupLevel::Low`). Add a **10 kΩ resistor from
//! GPIO27 to 3V3** for a reliable, low-leakage pull-up during deep sleep.
//!
//! ## Zero-wiring test option
//! The Feather's **onboard user button is GPIO38** and already has a hardware
//! pull-up, so you can prove out wake→play→sleep before wiring the external
//! button by setting [`BUTTON_ACTIVE_LOW`] accordingly and using GPIO38.

/// Playback sample rate (Hz). All embedded clips MUST be mono, 16-bit signed,
/// little-endian PCM at this rate. See [`crate::clips`].
pub const SAMPLE_RATE: u32 = 22_050;

/// Size of the I2S DMA transmit buffer, in bytes. Holds stereo 16-bit frames
/// (4 bytes/frame), so this is `DMA_BUF_BYTES / 4` frames per chunk. Must be a
/// multiple of 4.
pub const DMA_BUF_BYTES: usize = 32_000;

/// Milliseconds to wait after enabling the amp (SD high) before streaming,
/// letting the MAX98357A settle to avoid a click at the start of a clip.
pub const AMP_SETTLE_MS: u64 = 5;

/// Milliseconds to hold the amp on after the last sample drains, before pulling
/// SD low, so the tail of the clip isn't clipped.
pub const AMP_TAIL_MS: u64 = 30;
