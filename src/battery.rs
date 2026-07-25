//! LiPo voltage monitoring for the Adafruit ESP32 Feather V2.
//!
//! The board connects `BAT` to GPIO35 (ADC1 channel 7) through two 200 kΩ
//! resistors, so the ADC sees half the battery voltage. The original ESP32 ADC
//! is not precision-calibrated by `esp-hal`; these readings are suitable for
//! coarse low-battery decisions, not for reporting an exact state of charge.

/// Number of ADC readings averaged at boot.
pub const SAMPLE_COUNT: u32 = 16;

/// Maximum code from the ESP32's 12-bit ADC.
const ADC_MAX_RAW: u32 = 4095;

/// Approximate top of the ESP32 ADC input range at 11 dB attenuation.
const ADC_FULL_SCALE_MV: u32 = 2450;

/// The Feather battery monitor divides BAT by two.
const BATTERY_DIVIDER: u32 = 2;

/// Below this, assume no battery is fitted and the board is running from USB.
///
/// A battery low enough to produce this reading cannot reliably run the board.
const BATTERY_PRESENT_MIN_MV: u16 = 2500;

/// Skip audio below this voltage to avoid a high-current load on a depleted
/// LiPo. The Feather/battery protection remains the final safety cutoff.
pub const CRITICAL_MV: u16 = 3400;

/// Log a warning below this voltage, but still allow playback.
pub const LOW_MV: u16 = 3600;

/// Coarse battery condition derived from the boot-time voltage sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    /// GPIO35 does not show a usable battery voltage; assume USB-only power.
    NotPresent,
    /// A battery is present but too depleted for the amplifier load.
    Critical,
    /// Playback is allowed, but the battery should be charged soon.
    Low,
    /// Battery voltage is above the low-battery threshold.
    Normal,
}

impl State {
    /// Whether starting an audio playback session is allowed.
    pub const fn allows_playback(self) -> bool {
        !matches!(self, Self::Critical)
    }
}

/// One averaged battery-monitor result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Reading {
    /// Averaged raw 12-bit ADC code.
    pub raw: u16,
    /// Approximate battery voltage after accounting for the 2:1 divider.
    pub millivolts: u16,
    /// Coarse condition used by the playback policy.
    pub state: State,
}

impl Reading {
    /// Convert an averaged GPIO35 ADC code into voltage and a coarse condition.
    pub const fn from_raw(raw: u16) -> Self {
        let millivolts = ((raw as u32 * ADC_FULL_SCALE_MV * BATTERY_DIVIDER) / ADC_MAX_RAW) as u16;
        let state = if millivolts < BATTERY_PRESENT_MIN_MV {
            State::NotPresent
        } else if millivolts < CRITICAL_MV {
            State::Critical
        } else if millivolts < LOW_MV {
            State::Low
        } else {
            State::Normal
        };

        Self {
            raw,
            millivolts,
            state,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn raw_for_mv(millivolts: u32) -> u16 {
        ((millivolts * ADC_MAX_RAW) / (ADC_FULL_SCALE_MV * BATTERY_DIVIDER)) as u16
    }

    #[test]
    fn classifies_usb_only_reading() {
        assert_eq!(Reading::from_raw(raw_for_mv(2000)).state, State::NotPresent);
    }

    #[test]
    fn suppresses_playback_for_critical_battery() {
        let reading = Reading::from_raw(raw_for_mv(3200));
        assert_eq!(reading.state, State::Critical);
        assert!(!reading.state.allows_playback());
    }

    #[test]
    fn warns_but_plays_on_low_battery() {
        let reading = Reading::from_raw(raw_for_mv(3500));
        assert_eq!(reading.state, State::Low);
        assert!(reading.state.allows_playback());
    }

    #[test]
    fn accepts_charged_battery() {
        assert_eq!(Reading::from_raw(raw_for_mv(4000)).state, State::Normal);
    }
}
