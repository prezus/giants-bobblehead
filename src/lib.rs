#![no_std]
//! Giants Bobblehead — a low-power ESP32 soundboard.
//!
//! Press the button, hear a Giants announcer sound bite, then the board drops
//! straight back into deep sleep. See [`board`] for the wiring map.

pub mod audio;
pub mod board;
pub mod clips;
