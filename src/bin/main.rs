#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

//! Giants Bobblehead soundboard — main control flow.
//!
//! The board spends almost all its life in deep sleep (~10 µA). Pressing the
//! button is an ext0 RTC wakeup that resets the chip; `main` runs from the top
//! each time, plays the next clip, and goes back to sleep:
//!
//! ```text
//! deep sleep --button--> boot --> enable amp --> stream clip --> amp off --> deep sleep
//! ```

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::dma_buffers;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::i2s::master::{Channels, Config as I2sConfig, DataFormat, I2s};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::rtc_cntl::sleep::{Ext0WakeupSource, WakeupLevel};
use esp_hal::rtc_cntl::{Rtc, SocResetReason, reset_reason, wakeup_cause};
use esp_hal::system::Cpu;
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use log::info;

use giants_bobblehead::audio;
use giants_bobblehead::board::{AMP_SETTLE_MS, AMP_TAIL_MS, DMA_BUF_BYTES, SAMPLE_RATE};
use giants_bobblehead::clips;

extern crate alloc;

// Round-robin clip index, kept in RTC fast RAM so it survives the deep-sleep
// reset between button presses. Zeroed only on a true power-on.
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut CLIP_INDEX: u32 = 0;

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
    info!("reset reason: {:?}, wake cause: {:?}", reason, wakeup_cause());

    // Amp shutdown control: start LOW (amp off) so we don't draw current or pop
    // while I2S is still being set up.
    let mut amp_sd = Output::new(peripherals.GPIO33, Level::Low, OutputConfig::default());

    // --- Set up I2S transmit to the MAX98357A ---
    let (_, _, tx_buffer, tx_descriptors) = dma_buffers!(0, DMA_BUF_BYTES);
    let i2s = I2s::new(
        peripherals.I2S0,
        peripherals.DMA_I2S0,
        I2sConfig::new_tdm_philips()
            .with_sample_rate(Rate::from_hz(SAMPLE_RATE))
            .with_data_format(DataFormat::Data16Channel16)
            .with_channels(Channels::STEREO),
    )
    .unwrap()
    .into_async();
    let mut i2s_tx = i2s
        .i2s_tx
        .with_bclk(peripherals.GPIO14)
        .with_ws(peripherals.GPIO15)
        .with_dout(peripherals.GPIO32)
        .build(tx_descriptors);

    // --- Pick the next clip (round-robin, persisted across deep sleep) ---
    let index = {
        // SAFETY: single-threaded startup; no other reference to CLIP_INDEX
        // exists this early in boot.
        let current = unsafe { CLIP_INDEX } as usize % clips::COUNT;
        unsafe { CLIP_INDEX = (current as u32).wrapping_add(1) };
        current
    };
    let clip = &clips::CLIPS[index];
    info!("playing clip [{}] {}", index, clip.name);

    // --- Play: enable amp, stream, let the tail drain, disable amp ---
    amp_sd.set_high();
    Timer::after(Duration::from_millis(AMP_SETTLE_MS)).await;

    audio::play(&mut i2s_tx, tx_buffer, clip.pcm).await;

    Timer::after(Duration::from_millis(AMP_TAIL_MS)).await;
    amp_sd.set_low();

    // --- Arm the button and go back to deep sleep ---
    // Button wires GPIO27 -> GND (active low). Configure an internal pull-up so
    // the pad reads HIGH when idle; ext0 then wakes us on the falling edge.
    // NOTE: for reliable, low-leakage hold through deep sleep, also fit an
    // external 10 kΩ pull-up from GPIO27 to 3V3 (see board.rs).
    let mut button = peripherals.GPIO27;
    let idle = Input::new(button.reborrow(), InputConfig::default().with_pull(Pull::Up));
    drop(idle);

    let ext0 = Ext0WakeupSource::new(button, WakeupLevel::Low);

    info!("sleeping — press the button to play");
    Delay::new().delay_millis(50);
    rtc.sleep_deep(&[&ext0]);
}
