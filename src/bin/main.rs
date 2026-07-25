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

/// Average `battery::SAMPLE_COUNT` conversions from the BAT/2 divider.
///
/// Failed conversions are skipped rather than folded in as zero, which would drag
/// a healthy battery toward `Critical` and silently disable audio. Returns `None`
/// if every conversion failed.
fn sample_battery<'d, PIN, ADCI>(
    adc: &mut Adc<'d, ADCI, esp_hal::Blocking>,
    pin: &mut esp_hal::analog::adc::AdcPin<PIN, ADCI>,
    full_scale_mv: u32,
) -> Option<Reading>
where
    PIN: esp_hal::analog::adc::AdcChannel,
    ADCI: esp_hal::analog::adc::RegisterAccess + 'd,
{
    // Discard one settling conversion before averaging.
    let _ = nb::block!(adc.read_oneshot(pin));

    let mut sum = 0u32;
    let mut taken = 0u32;
    for _ in 0..battery::SAMPLE_COUNT {
        match nb::block!(adc.read_oneshot(pin)) {
            Ok(raw) => {
                sum += u32::from(raw);
                taken += 1;
            }
            Err(()) => log::warn!("battery ADC conversion failed"),
        }
    }

    if taken == 0 {
        return None;
    }
    if taken < battery::SAMPLE_COUNT {
        log::warn!("battery: only {taken}/{} samples read", battery::SAMPLE_COUNT);
    }
    Some(Reading::from_raw((sum / taken) as u16, full_scale_mv))
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
    // The classic ESP32 has no HAL calibration curve, so derive the ADC's
    // full-scale voltage from this chip's own reference voltage in eFuse. A chip
    // with nothing burned reads 0, which decodes to the nominal 1100 mV.
    let vref_code: u8 = efuse::read_field_le(efuse::ADC_VREF);
    let vref_mv = battery::vref_from_efuse(vref_code);
    let full_scale_mv = battery::full_scale_mv(vref_mv);
    info!("adc: vref {vref_mv} mV (efuse {vref_code:#x}), full scale {full_scale_mv} mV");

    let mut adc_config = AdcConfig::new();
    let mut battery_pin = adc_config.enable_pin(peripherals.GPIO35, Attenuation::_11dB);
    let mut adc = Adc::new(peripherals.ADC1, adc_config);

    let battery = sample_battery(&mut adc, &mut battery_pin, full_scale_mv);
    match battery {
        Some(reading) => info!(
            "battery: ~{} mV (raw {}, {:?})",
            reading.millivolts, reading.raw, reading.state
        ),
        None => log::warn!("battery: no ADC samples; assuming USB power"),
    }
    match battery.map(|r| r.state) {
        Some(State::NotPresent) | None => {
            info!("no battery voltage detected; assuming USB power")
        }
        Some(State::Critical) => log::warn!(
            "battery below {} mV; skipping audio until recharged",
            battery::CRITICAL_MV
        ),
        Some(State::Low) => log::warn!("battery below {} mV; charge soon", battery::LOW_MV),
        Some(State::Normal) => {}
    }

    // A failed read shouldn't brick the soundboard; treat it as USB power.
    let allows_playback = battery.is_none_or(|r| r.state.allows_playback());

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
    // ext0 wakes on GPIO27's falling edge. Internal pulls don't hold reliably
    // through deep sleep, so also fit an external 10 kΩ from GPIO27 to 3V3 (see
    // board.rs).
    let ext0 = Ext0WakeupSource::new(button_pin, WakeupLevel::Low);
    info!("sleeping — press the button to play");
    Delay::new().delay_millis(PRE_SLEEP_FLUSH_MS);
    rtc.sleep_deep(&[&ext0]);
}
