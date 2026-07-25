#![no_std]
#![no_main]

//! Giants Bobblehead soundboard — main control flow.
//!
//! The board spends almost all its life in deep sleep. Pressing the button is an
//! ext0 RTC wakeup that resets the chip; `main` runs from the top, samples the
//! battery, plays a clip when power is healthy, and goes back to sleep:
//!
//! ```text
//! deep sleep --button--> boot --> check battery --> amp on --> play --> amp off
//! ```

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::analog::adc::{Adc, AdcConfig, Attenuation};
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::dma_buffers;
use esp_hal::efuse;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::i2s::master::{Channels, Config as I2sConfig, DataFormat, I2s};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::rng::Rng;
use esp_hal::rtc_cntl::sleep::{Ext0WakeupSource, WakeupLevel};
use esp_hal::rtc_cntl::{Rtc, SocResetReason, reset_reason, wakeup_cause};
use esp_hal::system::Cpu;
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use log::info;

use giants_bobblehead::audio;
use giants_bobblehead::battery::{self, Reading, State};
use giants_bobblehead::board::{
    AMP_SETTLE_MS, AMP_TAIL_MS, DMA_BUF_BYTES, PRE_SLEEP_FLUSH_MS, SAMPLE_RATE,
};
use giants_bobblehead::clips;

// This creates a default app-descriptor required by the esp-idf bootloader.
esp_bootloader_esp_idf::esp_app_desc!();

/// Result of one boot-time battery sampling pass.
struct BatterySample {
    reading: Option<Reading>,
    failed_conversions: u32,
}

/// Average `battery::SAMPLE_COUNT` conversions from the BAT/2 divider.
///
/// Failed conversions are skipped rather than folded in as zero, which would drag
/// a healthy battery toward `Critical` and silently disable audio. The returned
/// sample also records how many conversions failed so the caller can emit one
/// complete status line.
fn sample_battery<'d, PIN, ADCI>(
    adc: &mut Adc<'d, ADCI, esp_hal::Blocking>,
    pin: &mut esp_hal::analog::adc::AdcPin<PIN, ADCI>,
    calibration: battery::Calibration,
) -> BatterySample
where
    PIN: esp_hal::analog::adc::AdcChannel,
    ADCI: esp_hal::analog::adc::RegisterAccess + 'd,
{
    // Discard one settling conversion before averaging.
    let _ = nb::block!(adc.read_oneshot(pin));

    let mut sum = 0u32;
    let mut taken = 0u32;
    for _ in 0..battery::SAMPLE_COUNT {
        if let Ok(raw) = nb::block!(adc.read_oneshot(pin)) {
            sum += u32::from(raw);
            taken += 1;
        }
    }

    BatterySample {
        reading: sum
            .checked_div(taken)
            .map(|average| Reading::from_raw(average as u16, calibration)),
        failed_conversions: battery::SAMPLE_COUNT - taken,
    }
}

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // Nothing in this crate allocates, but `esp-rtos` is built with the
    // `esp-alloc` feature and needs a global allocator to link. The size is
    // whatever fits in RAM reclaimed from the bootloader, and could likely be
    // cut a long way — it has not been tuned against measured usage.
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 98768);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    let mut rtc = Rtc::new(peripherals.LPWR);

    // Why did we wake? Handy while bringing the hardware up.
    let reason = reset_reason(Cpu::ProCpu).unwrap_or(SocResetReason::ChipPowerOn);
    info!(
        "reset reason: {:?}, wake cause: {:?}",
        reason,
        wakeup_cause()
    );

    // Shut down external loads immediately. GPIO2 controls the Feather's
    // NeoPixel/STEMMA regulator; GPIO33 drives the MAX98357A SD input. The
    // external 10 kΩ SD pulldown keeps the amp off through reset/deep sleep.
    let _peripheral_power = Output::new(peripherals.GPIO2, Level::Low, OutputConfig::default());
    let mut amp_sd = Output::new(peripherals.GPIO33, Level::Low, OutputConfig::default());

    // --- Battery monitor (GPIO35 = BAT/2, ADC1 channel 7) ---
    // `esp-hal` does not expose the classic ESP32 calibration curve. Port
    // Espressif's ADC1 11 dB line fit here: prefer two-point eFuse data, then
    // fall back to the chip Vref (or nominal 1100 mV when Vref is unburned).
    let vref_code: u8 = efuse::read_field_le(efuse::ADC_VREF);
    let vref_mv = battery::vref_from_efuse(vref_code);
    let block3_reserved: u8 = efuse::read_field_le(efuse::BLK3_PART_RESERVE);
    let adc1_tp_low: u8 = efuse::read_field_le(efuse::ADC1_TP_LOW);
    let adc1_tp_high: u16 = efuse::read_field_le(efuse::ADC1_TP_HIGH);
    let adc2_tp_low: u8 = efuse::read_field_le(efuse::ADC2_TP_LOW);
    let adc2_tp_high: u16 = efuse::read_field_le(efuse::ADC2_TP_HIGH);
    let two_point_present = block3_reserved != 0
        && adc1_tp_low != 0
        && adc1_tp_high != 0
        && adc2_tp_low != 0
        && adc2_tp_high != 0;
    let (calibration, calibration_source) = if two_point_present {
        match battery::Calibration::from_two_point(adc1_tp_low, adc1_tp_high) {
            Some(calibration) => (calibration, "two-point eFuse"),
            None => (battery::Calibration::from_vref(vref_mv), "Vref fallback"),
        }
    } else {
        (battery::Calibration::from_vref(vref_mv), "Vref")
    };
    info!(
        "adc: {calibration_source} calibration, vref {vref_mv} mV, coefficients ({}, {})",
        calibration.coefficient_a, calibration.coefficient_b
    );

    let mut adc_config = AdcConfig::new();
    let mut battery_pin = adc_config.enable_pin(peripherals.GPIO35, Attenuation::_11dB);
    let mut adc = Adc::new(peripherals.ADC1, adc_config);

    let battery = sample_battery(&mut adc, &mut battery_pin, calibration);
    let allows_playback = match battery.reading {
        None => {
            log::warn!(
                "battery: all {} ADC conversions failed; skipping audio",
                battery::SAMPLE_COUNT
            );
            false
        }
        Some(reading) => {
            match reading.state {
                State::NotPresent => {
                    info!(
                        "battery: ~{} mV (raw {}, {}/{} conversions failed); assuming USB power",
                        reading.millivolts,
                        reading.raw,
                        battery.failed_conversions,
                        battery::SAMPLE_COUNT
                    );
                }
                State::Critical => {
                    log::warn!(
                        "battery: ~{} mV (raw {}, {}/{} conversions failed); below {} mV, skipping audio",
                        reading.millivolts,
                        reading.raw,
                        battery.failed_conversions,
                        battery::SAMPLE_COUNT,
                        battery::CRITICAL_MV
                    );
                }
                State::Low => {
                    log::warn!(
                        "battery: ~{} mV (raw {}, {}/{} conversions failed); below {} mV, charge soon",
                        reading.millivolts,
                        reading.raw,
                        battery.failed_conversions,
                        battery::SAMPLE_COUNT,
                        battery::LOW_MV
                    );
                }
                State::Normal => {
                    info!(
                        "battery: ~{} mV (raw {}, {}/{} conversions failed); normal",
                        reading.millivolts,
                        reading.raw,
                        battery.failed_conversions,
                        battery::SAMPLE_COUNT
                    );
                }
            }
            reading.state.allows_playback()
        }
    };

    let mut button_pin = peripherals.GPIO27;
    if allows_playback {
        // --- Set up I2S transmit to the MAX98357A ---
        let (_, _, tx_buffer, tx_descriptors) = dma_buffers!(0, DMA_BUF_BYTES);
        let i2s = I2s::new(
            peripherals.I2S0,
            peripherals.DMA_I2S0,
            I2sConfig::new_tdm_philips()
                .with_sample_rate(Rate::from_hz(SAMPLE_RATE))
                .with_data_format(DataFormat::Data16Channel16)
                .with_channels(Channels::STEREO),
        );

        // Don't panic on a bad I2S config: a panic never reaches the deep-sleep
        // call below, so the board would sit awake draining the battery.
        match i2s {
            Ok(i2s) => {
                let i2s_tx = i2s
                    .into_async()
                    .i2s_tx
                    .with_bclk(peripherals.GPIO8)
                    .with_ws(peripherals.GPIO7)
                    .with_dout(peripherals.GPIO14)
                    .build(tx_descriptors);

                // --- Clip selection ---
                // Mix the hardware RNG with the RTC timer, which varies with the
                // time spent asleep. `clips` keeps the last index in RTC RAM so
                // it survives the deep-sleep reset and prevents a repeat.
                let rng = Rng::new();
                let entropy = || rng.random() ^ (rtc.current_time_us() as u32);

                let first = match clips::last_played() {
                    // True power-on: play a known clip so plugging in is
                    // predictable.
                    None => {
                        clips::set_last_played(clips::FIRST_BOOT);
                        &clips::CLIPS[clips::FIRST_BOOT]
                    }
                    Some(_) => clips::advance(entropy()),
                };
                info!("playing clip {}", first.name);

                let next_clip = || {
                    let clip = clips::advance(entropy());
                    info!("playing clip {}", clip.name);
                    clip.pcm
                };

                // While awake, another button press interrupts the current clip
                // and starts a different one. Once a clip completes, shut the amp
                // back down.
                amp_sd.set_high();
                Timer::after(Duration::from_millis(AMP_SETTLE_MS)).await;
                {
                    let mut button = Input::new(
                        button_pin.reborrow(),
                        InputConfig::default().with_pull(Pull::Up),
                    );
                    audio::session(i2s_tx, tx_buffer, &mut button, first.pcm, next_clip).await;
                }
                Timer::after(Duration::from_millis(AMP_TAIL_MS)).await;
                amp_sd.set_low();
            }
            Err(e) => log::error!("i2s config rejected, skipping playback: {e:?}"),
        }
    }

    // --- Deep sleep until the next press ---
    // ext0 wakes while GPIO27 is low. Internal pulls don't hold reliably through
    // deep sleep, so also fit an external 10 kΩ from GPIO27 to 3V3 (see
    // board.rs). Entering sleep while the active-low button is still held would
    // immediately wake and reboot the board. Flush logs first, then make the
    // release check the final action before arming ext0.
    info!("sleeping — press the button to play");
    Delay::new().delay_millis(PRE_SLEEP_FLUSH_MS);
    {
        let mut button = Input::new(
            button_pin.reborrow(),
            InputConfig::default().with_pull(Pull::Up),
        );
        if button.is_low() {
            info!("waiting for button release before sleep");
            button.wait_for_high().await;
        }
    }

    let ext0 = Ext0WakeupSource::new(button_pin, WakeupLevel::Low);
    rtc.sleep_deep(&[&ext0]);
}
