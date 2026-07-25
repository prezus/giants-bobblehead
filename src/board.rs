//! Board configuration for the **Adafruit ESP32 Feather V2** (product 5400).
//!
//! # Wiring map
//!
//! Signal chain: `ESP32 --I2S--> MAX98357A amp (3006) --> 4Ω speaker (4445)`,
//! triggered by the IoT button (5666).
//!
//! | Feather pin | GPIO | Connects to                                   |
//! |-------------|------|-----------------------------------------------|
//! | TX          | 8    | MAX98357A **BCLK**                            |
//! | RX          | 7    | MAX98357A **LRC** (word select / WS)         |
//! | D14         | 14   | MAX98357A **DIN** (data)                     |
//! | D33         | 33   | MAX98357A **SD** (amp enable)                |
//! | D27         | 27   | 5666 button **A2** pad (ext0 wakeup)         |
//! | BAT         | —    | MAX98357A **Vin** (battery-backed supply)    |
//! | GND         | —    | Amp **GND/GAIN**, button **GND**, speaker    |
//! | 3V3         | —    | 10 kΩ pull-up to GPIO27 (see below)          |
//! | —           | —    | Speaker **+/–** across MAX98357A **+/–**     |
//! | —           | —    | 10 kΩ from MAX98357A **SD** to **GND**       |
//!
//! ## Trigger button (external 5666 IoT button → GPIO27)
//! Playback is triggered by the **Adafruit 5666 IoT button**. Its **A2** pad
//! connects GPIO27 to GND when pressed (active-low), and each press wakes the
//! chip from deep sleep via ext0 (`WakeupLevel::Low`) to play a **random** clip.
//! GPIO27 is RTC-capable (`RTC_GPIO17`).
//!
//! The 5666 has **no on-board pull-up**, so the firmware enables GPIO27's
//! internal pull-up to read HIGH when idle. Internal pulls don't hold reliably
//! through deep sleep, so also fit an external **10 kΩ from GPIO27 to 3V3** for
//! a stable, low-leakage hold — without it, wakeups can be flaky.
//!
//! The Feather's onboard **SW38 button (GPIO38)** is reserved as a bench-test
//! trigger (polled while awake, not a deep-sleep wake source).
//!
//! ## Battery power and amp shutdown
//! Power the MAX98357A from **BAT**, not USB, so audio works when USB-C is
//! disconnected. GPIO33 drives **SD** high only during playback. Fit an
//! external **10 kΩ pulldown from SD to GND** so the amp remains shut down while
//! the ESP32 resets and while its GPIOs are high-impedance in deep sleep.
//!
//! ## Battery monitor
//! The Feather's built-in two-resistor divider connects BAT/2 to **GPIO35
//! (ADC1)**. Firmware averages that input at boot, warns below 3.6 V, and skips
//! audio below 3.4 V. No external monitor wiring is required.

/// Playback sample rate (Hz). All embedded clips MUST be mono, 16-bit signed,
/// little-endian PCM at this rate. See [`crate::clips`].
pub const SAMPLE_RATE: u32 = 22_050;

/// Size of the I2S DMA transmit buffer, in bytes. Holds stereo 16-bit frames
/// (4 bytes/frame), so this is `DMA_BUF_BYTES / 4` frames per chunk. Must be a
/// multiple of 4.
pub const DMA_BUF_BYTES: usize = 32_000;

/// Milliseconds to wait after enabling the amplifier before streaming audio.
pub const AMP_SETTLE_MS: u64 = 5;

/// Milliseconds to leave the amp enabled after the final silence is queued.
pub const AMP_TAIL_MS: u64 = 30;
