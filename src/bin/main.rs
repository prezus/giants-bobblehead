#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

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
use giants_bobblehead::board::{AMP_SETTLE_MS, AMP_TAIL_MS, DMA_BUF_BYTES, SAMPLE_RATE};
use giants_bobblehead::clips;

extern crate alloc;

// Last-played clip, stored as `index + 1` in RTC fast RAM so it survives the
// deep-sleep reset between button presses. 0 means "none yet" (a true power-on
// zeroes this), which lets the first pick be any clip; afterwards we avoid
// replaying the same clip twice in a row.
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut LAST_CLIP: u32 = 0;

// This creates a default app-descriptor required by the esp-idf bootloader.
esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

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
    let mut adc_config = AdcConfig::new();
    let mut battery_pin = adc_config.enable_pin(peripherals.GPIO35, Attenuation::_11dB);
    let mut adc = Adc::new(peripherals.ADC1, adc_config);

    // Discard one settling conversion, then average enough samples to smooth
    // the classic ESP32 ADC's noise.
    let _ = nb::block!(adc.read_oneshot(&mut battery_pin));
    let mut raw_sum = 0u32;
    for _ in 0..battery::SAMPLE_COUNT {
        raw_sum += nb::block!(adc.read_oneshot(&mut battery_pin)).unwrap_or(0) as u32;
    }
    let battery = Reading::from_raw((raw_sum / battery::SAMPLE_COUNT) as u16);
    info!(
        "battery: ~{} mV (raw {}, {:?})",
        battery.millivolts, battery.raw, battery.state
    );
    match battery.state {
        State::NotPresent => info!("no battery voltage detected; assuming USB power"),
        State::Critical => log::warn!(
            "battery below {} mV; skipping audio until recharged",
            battery::CRITICAL_MV
        ),
        State::Low => log::warn!("battery below {} mV; charge soon", battery::LOW_MV),
        State::Normal => {}
    }

    let mut button_pin = peripherals.GPIO27;
    if battery.state.allows_playback() {
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
        let i2s_tx = i2s
            .unwrap()
            .into_async()
            .i2s_tx
            .with_bclk(peripherals.GPIO8)
            .with_ws(peripherals.GPIO7)
            .with_dout(peripherals.GPIO14)
            .build(tx_descriptors);

        // --- Clip selection ---
        // Mix the hardware RNG with the RTC timer, which varies with the time
        // spent asleep. LAST_CLIP survives deep-sleep reset and prevents an
        // immediate repeat.
        let rng = Rng::new();
        let entropy = || rng.random() ^ (rtc.current_time_us() as u32);
        let pick_random = || {
            let e = entropy() as usize;
            let last = unsafe { LAST_CLIP };
            let index = if clips::COUNT <= 1 {
                0
            } else if last == 0 {
                e % clips::COUNT
            } else {
                let prev = (last - 1) as usize % clips::COUNT;
                (prev + 1 + e % (clips::COUNT - 1)) % clips::COUNT
            };
            unsafe { LAST_CLIP = index as u32 + 1 };
            index
        };

        let first_index = if unsafe { LAST_CLIP } == 0 {
            let i = clips::CLIPS
                .iter()
                .position(|c| c.name == "izzy-pine")
                .unwrap_or(0);
            unsafe { LAST_CLIP = i as u32 + 1 };
            i
        } else {
            pick_random()
        };
        info!(
            "playing clip [{}] {}",
            first_index,
            clips::CLIPS[first_index].name
        );
        let first = clips::CLIPS[first_index].pcm;
        let next_clip = || {
            let i = pick_random();
            info!("playing clip [{}] {}", i, clips::CLIPS[i].name);
            clips::CLIPS[i].pcm
        };

        // While awake, another button press interrupts the current clip and
        // starts a different one. Once a clip completes, shut the amp back down.
        amp_sd.set_high();
        Timer::after(Duration::from_millis(AMP_SETTLE_MS)).await;
        {
            let mut button = Input::new(
                button_pin.reborrow(),
                InputConfig::default().with_pull(Pull::Up),
            );
            audio::session(i2s_tx, tx_buffer, &mut button, first, next_clip).await;
        }
        Timer::after(Duration::from_millis(AMP_TAIL_MS)).await;
        amp_sd.set_low();
    }

    // --- Deep sleep until the next press ---
    // ext0 wakes on GPIO27's falling edge. Internal pulls don't hold reliably
    // through deep sleep, so also fit an external 10 kΩ from GPIO27 to 3V3 (see
    // board.rs).
    let ext0 = Ext0WakeupSource::new(button_pin, WakeupLevel::Low);
    info!("sleeping — press the button to play");
    Delay::new().delay_millis(50);
    rtc.sleep_deep(&[&ext0]);
}
