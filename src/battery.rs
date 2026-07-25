//! LiPo voltage monitoring for the Adafruit ESP32 Feather V2.
//!
//! The board connects `BAT` to GPIO35 (ADC1 channel 7) through two 200 kΩ
//! resistors, so the ADC sees half the battery voltage.
//!
//! # Accuracy, and why this module takes a full-scale argument
//!
//! Converting a raw ADC code to millivolts needs to know the input voltage that
//! produces the maximum code. `esp-hal` provides no calibration curve for the
//! *original* ESP32 (only for the S2/S3 and the RISC-V parts), so we derive that
//! full-scale value ourselves from the reference voltage — see [`full_scale_mv`]
//! and [`vref_from_efuse`] — and the caller passes it to [`Reading::from_raw`].
//!
//! Keeping the conversion pure like this has two benefits: the whole module has
//! no `esp-hal` dependency and so can be unit-tested on the host (see
//! `just test-battery`), and the full-scale value can be recalibrated per board
//! without touching the policy logic.
//!
//! **These readings are coarse.** Treat them as good enough to refuse playback
//! on a flat battery, not as a state-of-charge display. See the README for the
//! recalibration procedure.

/// Number of ADC readings averaged at boot.
pub const SAMPLE_COUNT: u32 = 16;

/// Maximum code from the ESP32's 12-bit ADC.
const ADC_MAX_RAW: u32 = 4095;

/// The ESP32's nominal ADC reference voltage, and the value assumed when a chip
/// has no calibration burned into eFuse.
pub const NOMINAL_VREF_MV: u32 = 1100;

/// Millivolts per step in the eFuse `ADC_VREF` field.
const VREF_STEP_MV: u32 = 7;

/// Input attenuation for `Attenuation::_11dB`, as a voltage ratio × 1000.
///
/// 11 dB is a voltage ratio of `10^(11/20) ≈ 3.548`.
///
/// This is the least certain number in the module. Espressif originally labelled
/// this setting 11 dB and later relabelled the same hardware setting 12 dB
/// (ratio ≈ 3.98) after measuring it, so the true figure sits somewhere in that
/// ~12% band, and per-chip spread widens it further. Recalibrate against a
/// multimeter rather than trusting the nominal value.
const ATTEN_11DB_RATIO_MILLI: u32 = 3548;

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

/// Decode the ESP32's 5-bit eFuse `ADC_VREF` field into a reference voltage.
///
/// The field stores a deviation from [`NOMINAL_VREF_MV`]: bit 4 is the sign and
/// bits 0..=3 the magnitude, in [`VREF_STEP_MV`] steps. A chip with no
/// calibration burned reads 0, which yields the nominal voltage.
pub const fn vref_from_efuse(code: u8) -> u32 {
    let magnitude = (code & 0x0F) as u32 * VREF_STEP_MV;
    if code & 0x10 != 0 {
        NOMINAL_VREF_MV - magnitude
    } else {
        NOMINAL_VREF_MV + magnitude
    }
}

/// The input voltage that produces [`ADC_MAX_RAW`], for a given reference
/// voltage at 11 dB attenuation.
///
/// Note this is the *extrapolated* full scale. The ESP32's input cannot exceed
/// its supply, so the top of this range is not physically reachable — but it is
/// the correct denominator for converting codes in the usable range.
pub const fn full_scale_mv(vref_mv: u32) -> u32 {
    vref_mv * ATTEN_11DB_RATIO_MILLI / 1000
}

/// Full scale for a chip with no eFuse calibration, useful as a fallback and in
/// tests.
pub const DEFAULT_FULL_SCALE_MV: u32 = full_scale_mv(NOMINAL_VREF_MV);

/// Coarse battery condition derived from the boot-time voltage sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    /// GPIO35 does not show a usable battery voltage; assume USB-only power.
    ///
    /// Note this also catches a battery so far gone that it reads below
    /// [`BATTERY_PRESENT_MIN_MV`]; such a cell is indistinguishable from no
    /// cell at all, and playback is allowed on the assumption that USB is
    /// supplying the current.
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
    #[must_use]
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
    ///
    /// `full_scale_mv` is the input that would produce the maximum ADC code —
    /// see [`full_scale_mv`] for how to derive it.
    #[must_use]
    pub const fn from_raw(raw: u16, full_scale_mv: u32) -> Self {
        let scaled = raw as u32 * full_scale_mv * BATTERY_DIVIDER / ADC_MAX_RAW;
        let millivolts = if scaled > u16::MAX as u32 {
            u16::MAX
        } else {
            scaled as u16
        };

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

    /// The raw code a given battery voltage would produce, at default scaling.
    const fn raw_for_mv(millivolts: u32) -> u16 {
        ((millivolts * ADC_MAX_RAW) / (DEFAULT_FULL_SCALE_MV * BATTERY_DIVIDER)) as u16
    }

    fn state_at(millivolts: u32) -> State {
        Reading::from_raw(raw_for_mv(millivolts), DEFAULT_FULL_SCALE_MV).state
    }

    #[test]
    fn classifies_usb_only_reading() {
        assert_eq!(state_at(2000), State::NotPresent);
    }

    #[test]
    fn suppresses_playback_for_critical_battery() {
        let reading = Reading::from_raw(raw_for_mv(3200), DEFAULT_FULL_SCALE_MV);
        assert_eq!(reading.state, State::Critical);
        assert!(!reading.state.allows_playback());
    }

    #[test]
    fn warns_but_plays_on_low_battery() {
        let reading = Reading::from_raw(raw_for_mv(3500), DEFAULT_FULL_SCALE_MV);
        assert_eq!(reading.state, State::Low);
        assert!(reading.state.allows_playback());
    }

    #[test]
    fn accepts_charged_battery() {
        assert_eq!(state_at(4000), State::Normal);
    }

    /// A fully charged cell must classify as `Normal` and be allowed to play.
    ///
    /// This is the test that pins the scaling down: feeding
    /// [`ATTEN_11DB_RATIO_MILLI`] the *recommended input range* ceiling (2450 mV)
    /// rather than the full-scale voltage under-reads by ~20%, which lands 4.2 V
    /// in `Critical` and silently disables audio on a healthy battery.
    #[test]
    fn full_charge_is_normal_and_plays() {
        let reading = Reading::from_raw(raw_for_mv(4200), DEFAULT_FULL_SCALE_MV);
        assert_eq!(reading.state, State::Normal);
        assert!(reading.state.allows_playback());
    }

    #[test]
    fn round_trips_voltage_within_rounding_error() {
        for mv in [3000_u32, 3500, 3700, 4200] {
            let got = Reading::from_raw(raw_for_mv(mv), DEFAULT_FULL_SCALE_MV).millivolts;
            let diff = got.abs_diff(mv as u16);
            assert!(diff <= 4, "{mv} mV round-tripped to {got} mV");
        }
    }

    #[test]
    fn decodes_uncalibrated_efuse_as_nominal_vref() {
        assert_eq!(vref_from_efuse(0), NOMINAL_VREF_MV);
    }

    #[test]
    fn decodes_signed_efuse_vref_deviations() {
        // bit 4 clear => positive deviation, set => negative.
        assert_eq!(vref_from_efuse(0b0_0011), NOMINAL_VREF_MV + 21);
        assert_eq!(vref_from_efuse(0b1_0011), NOMINAL_VREF_MV - 21);
        // Extremes of the 4-bit magnitude.
        assert_eq!(vref_from_efuse(0b0_1111), NOMINAL_VREF_MV + 105);
        assert_eq!(vref_from_efuse(0b1_1111), NOMINAL_VREF_MV - 105);
    }

    #[test]
    fn scales_full_scale_with_vref() {
        assert_eq!(full_scale_mv(1000), 3548);
        assert!(full_scale_mv(1205) > full_scale_mv(995));
    }

    /// A zero code must not be mistaken for a healthy battery.
    #[test]
    fn treats_zero_code_as_absent() {
        assert_eq!(
            Reading::from_raw(0, DEFAULT_FULL_SCALE_MV).state,
            State::NotPresent
        );
    }
}
