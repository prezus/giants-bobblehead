#![no_std]
//! Giants Bobblehead — a low-power ESP32 soundboard.
//!
//! Press the button, hear a Giants announcer sound bite, then the board drops
//! straight back into deep sleep. See [`board`] for the wiring map.
//!
//! # Module layout
//!
//! [`battery`], [`pcm`] and [`selection`] are deliberately free of `esp-hal` and
//! of crate-internal imports, so they can be compiled and unit-tested on the host
//! (`just test`). The hardware-touching logic lives in [`audio`] and [`clips`],
//! and [`board`] holds the pin map and timing constants.

pub mod audio;
pub mod battery;
pub mod board;
pub mod clips;
pub mod pcm;
pub mod selection;
