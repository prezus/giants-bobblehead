//! LiPo voltage monitoring for the Adafruit ESP32 Feather V2.
//!
//! The board connects `BAT` to GPIO35 (ADC1 channel 7) through two 200 kΩ
//! resistors, so the ADC sees half the battery voltage.
//!
//! # Accuracy and calibration
//!
//! The original ESP32 ADC is not an ideal straight line through zero, especially
//! at 11 dB attenuation. `esp-hal` does not expose a calibration curve for this
//! chip, so [`Calibration`] ports Espressif's ADC1 11 dB line-fitting model:
//! prefer the chip's two-point eFuse values when present, otherwise use its
//! eFuse reference voltage (or the nominal 1100 mV fallback).
//! The coefficients and equations follow ESP-IDF v5.4.2's
//! `esp_adc_cal_legacy.c` implementation.
//!
//! Keeping the conversion pure lets the whole module remain free of `esp-hal`
//! and unit-tested on the host with `just test`.
//!
//! **These readings are coarse.** Treat them as good enough to refuse playback
//! on a flat battery, not as a state-of-charge display. See the README for the
//! recalibration procedure.

/// Number of ADC readings averaged at boot.
pub const SAMPLE_COUNT: u32 = 16;

/// The ESP32's nominal ADC reference voltage, and the value assumed when a chip
/// has no calibration burned into eFuse.
pub const NOMINAL_VREF_MV: u32 = 1100;

/// Millivolts per step in the eFuse `ADC_VREF` field.
const VREF_STEP_MV: u32 = 7;

/// Fixed-point scale used by Espressif's line-fitting coefficients.
const COEFF_A_SCALE: u32 = 65_536;

/// Half the fixed-point scale, used to round raw-to-voltage conversion.
const COEFF_A_ROUND: u32 = COEFF_A_SCALE / 2;

/// ESP-IDF ADC1 11 dB attenuation scale when characterizing from Vref.
const ADC1_VREF_ATTEN_SCALE: u32 = 196_602;

/// ESP-IDF ADC1 11 dB attenuation offset when characterizing from Vref.
const ADC1_VREF_ATTEN_OFFSET_MV: u32 = 142;

/// ESP-IDF ADC1 11 dB attenuation scale when using two-point calibration.
const ADC1_TP_ATTEN_SCALE: u32 = 224_310;

/// ESP-IDF ADC1 11 dB attenuation offset when using two-point calibration.
const ADC1_TP_ATTEN_OFFSET_MV: i32 = 54;

/// Nominal raw ADC1 code at 150 mV before applying the eFuse deviation.
const TP_LOW_RAW_OFFSET: i32 = 278;

/// Nominal raw ADC1 code at 850 mV before applying the eFuse deviation.
const TP_HIGH_RAW_OFFSET: i32 = 3265;

/// Voltage represented by the two calibration points.
const TP_LOW_MV: i32 = 150;
const TP_HIGH_MV: i32 = 850;

/// Millivolts represented by one two-point eFuse deviation step.
const TP_STEP: i32 = 4;

/// The Feather battery monitor divides BAT by two.
const BATTERY_DIVIDER: u32 = 2;

/// Below this, assume no battery is fitted and the board is running from USB.
///
/// Keep this threshold far below any plausible LiPo voltage. Treating every
/// value below the old 2.5 V threshold as "USB-only" made the policy
/// non-monotonic and could re-enable playback on a collapsed cell.
const BATTERY_PRESENT_MIN_MV: u16 = 1000;

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

/// ADC1 11 dB line-fitting coefficients.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Calibration {
    /// Fixed-point slope, scaled by 65536.
    pub coefficient_a: u32,
    /// Input-voltage intercept in millivolts.
    pub coefficient_b: u32,
}

impl Calibration {
    /// Build the ESP-IDF ADC1 11 dB line fit for a chip reference voltage.
    #[must_use]
    pub const fn from_vref(vref_mv: u32) -> Self {
        Self {
            coefficient_a: vref_mv * ADC1_VREF_ATTEN_SCALE / 4096,
            coefficient_b: ADC1_VREF_ATTEN_OFFSET_MV,
        }
    }

    /// Build the ESP-IDF ADC1 11 dB line fit from two-point eFuse fields.
    ///
    /// Returns `None` for malformed points. Callers should then fall back to
    /// [`Self::from_vref`].
    #[must_use]
    pub fn from_two_point(low_bits: u8, high_bits: u16) -> Option<Self> {
        if low_bits == 0 || high_bits == 0 {
            return None;
        }

        let low = TP_LOW_RAW_OFFSET + decode_twos_complement(u32::from(low_bits), 0x7f) * TP_STEP;
        let high =
            TP_HIGH_RAW_OFFSET + decode_twos_complement(u32::from(high_bits), 0x1ff) * TP_STEP;
        let delta_raw = high.checked_sub(low)?;
        if delta_raw <= 0 {
            return None;
        }

        let delta_mv = TP_HIGH_MV - TP_LOW_MV;
        let coefficient_a = (delta_mv * ADC1_TP_ATTEN_SCALE as i32 + delta_raw / 2) / delta_raw;
        let coefficient_b =
            TP_HIGH_MV - (delta_mv * high + delta_raw / 2) / delta_raw + ADC1_TP_ATTEN_OFFSET_MV;

        Some(Self {
            coefficient_a: u32::try_from(coefficient_a).ok()?,
            coefficient_b: u32::try_from(coefficient_b).ok()?,
        })
    }

    /// Convert a raw ADC code into millivolts at the GPIO35 input.
    #[must_use]
    pub const fn input_mv(self, raw: u16) -> u32 {
        let scaled = (self.coefficient_a as u64 * raw as u64 + COEFF_A_ROUND as u64)
            / COEFF_A_SCALE as u64
            + self.coefficient_b as u64;
        if scaled > u32::MAX as u64 {
            u32::MAX
        } else {
            scaled as u32
        }
    }
}

/// Decode a two's-complement eFuse deviation with the given field mask.
fn decode_twos_complement(bits: u32, mask: u32) -> i32 {
    let bits = bits & mask;
    let sign = !(mask >> 1) & mask;
    if bits & sign == 0 {
        (bits & (mask >> 1)) as i32
    } else {
        -((((!bits) + 1) & (mask >> 1)) as i32)
    }
}

/// Nominal fallback calibration, useful in host tests.
pub const DEFAULT_CALIBRATION: Calibration = Calibration::from_vref(NOMINAL_VREF_MV);

/// Coarse battery condition derived from the boot-time voltage sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    /// GPIO35 does not show a usable battery voltage; assume USB-only power.
    ///
    /// This threshold is deliberately far below any plausible LiPo voltage so
    /// a depleted cell remains `Critical` rather than being mistaken for USB.
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
    /// `calibration` describes the ADC1 11 dB transfer curve.
    #[must_use]
    pub const fn from_raw(raw: u16, calibration: Calibration) -> Self {
        let scaled = calibration.input_mv(raw) as u64 * BATTERY_DIVIDER as u64;
        let millivolts = if scaled > u16::MAX as u64 {
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

    #[test]
    fn classifies_usb_only_reading() {
        assert_eq!(
            Reading::from_raw(0, DEFAULT_CALIBRATION).state,
            State::NotPresent
        );
    }

    #[test]
    fn no_battery_boundary_is_monotonic() {
        assert_eq!(
            Reading::from_raw(443, DEFAULT_CALIBRATION).state,
            State::NotPresent
        );
        assert_eq!(
            Reading::from_raw(444, DEFAULT_CALIBRATION).state,
            State::Critical
        );
    }

    #[test]
    fn suppresses_playback_for_critical_battery() {
        // Official nominal-Vref line fit: raw 1800 is about 3184 mV at BAT.
        let reading = Reading::from_raw(1800, DEFAULT_CALIBRATION);
        assert_eq!(reading.state, State::Critical);
        assert!(!reading.state.allows_playback());
    }

    #[test]
    fn does_not_mistake_a_collapsed_cell_for_usb_power() {
        // Raw 1313 is about 2400 mV at BAT: dangerously depleted, but nonzero.
        let reading = Reading::from_raw(1313, DEFAULT_CALIBRATION);
        assert_eq!(reading.state, State::Critical);
        assert!(!reading.state.allows_playback());
    }

    #[test]
    fn warns_but_plays_on_low_battery() {
        // Raw 2000 is about 3506 mV at BAT.
        let reading = Reading::from_raw(2000, DEFAULT_CALIBRATION);
        assert_eq!(reading.state, State::Low);
        assert!(reading.state.allows_playback());
    }

    #[test]
    fn accepts_charged_battery() {
        assert_eq!(
            Reading::from_raw(2306, DEFAULT_CALIBRATION).state,
            State::Normal
        );
    }

    #[test]
    fn nominal_vref_matches_espressif_coefficients() {
        assert_eq!(
            DEFAULT_CALIBRATION,
            Calibration {
                coefficient_a: 52_798,
                coefficient_b: 142,
            }
        );
    }

    #[test]
    fn matches_known_nominal_vref_voltage_vectors() {
        // These are independent vectors from Espressif's nominal-Vref line fit,
        // not values obtained by inverting this module's implementation.
        assert_eq!(
            Reading::from_raw(1934, DEFAULT_CALIBRATION).millivolts,
            3400
        );
        assert_eq!(
            Reading::from_raw(2058, DEFAULT_CALIBRATION).millivolts,
            3600
        );
        assert_eq!(
            Reading::from_raw(2430, DEFAULT_CALIBRATION).millivolts,
            4200
        );
    }

    #[test]
    fn critical_boundary_uses_calibrated_voltage() {
        assert_eq!(
            Reading::from_raw(1933, DEFAULT_CALIBRATION).state,
            State::Critical
        );
        assert_eq!(
            Reading::from_raw(1934, DEFAULT_CALIBRATION).state,
            State::Low
        );
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
    fn scales_line_fit_with_vref() {
        assert_eq!(Calibration::from_vref(1000).coefficient_a, 47_998);
        assert!(
            Calibration::from_vref(1205).coefficient_a > Calibration::from_vref(995).coefficient_a
        );
    }

    #[test]
    fn builds_two_point_calibration_from_known_codes() {
        assert_eq!(
            Calibration::from_two_point(1, 1),
            Some(Calibration {
                coefficient_a: 52_567,
                coefficient_b: 138,
            })
        );
    }

    #[test]
    fn rejects_absent_two_point_fields() {
        assert_eq!(Calibration::from_two_point(0, 1), None);
        assert_eq!(Calibration::from_two_point(1, 0), None);
    }

    #[test]
    fn decodes_signed_two_point_deviations() {
        let positive = Calibration::from_two_point(0b000_0011, 0b0_0000_0011).unwrap();
        let negative = Calibration::from_two_point(0b111_1101, 0b1_1111_1101).unwrap();
        assert_ne!(positive, negative);
    }

    /// A zero code must not be mistaken for a healthy battery.
    #[test]
    fn treats_zero_code_as_absent() {
        assert_eq!(
            Reading::from_raw(0, DEFAULT_CALIBRATION).state,
            State::NotPresent
        );
    }
}
