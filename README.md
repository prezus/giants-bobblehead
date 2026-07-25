# giants-bobblehead

A battery-powered **ESP32 soundboard** in Rust: press a button, hear a Giants
announcer sound bite, and the board drops straight back into deep sleep. It is
built `no_std` on [`esp-hal`](https://github.com/esp-rs/esp-hal), monitors its
LiPo at boot, and shuts down the amplifier between plays.

> **Status: not yet validated on hardware.** The firmware builds and its pure
> logic is unit-tested, but the assembled device has not been bench-tested. In
> particular the ported ADC calibration must be checked against a multimeter
> before relying on the battery thresholds — see
> [Battery monitoring](#charging-and-battery-monitoring) and
> [Hardware validation still required](#hardware-validation-still-required).

## Hardware

| Part | Product | Role |
|------|---------|------|
| Adafruit **ESP32 Feather V2** | [5400](https://www.adafruit.com/product/5400) | MCU (Xtensa dual-core, 8 MB flash, 2 MB PSRAM) |
| **MAX98357A** I2S amp | [3006](https://www.adafruit.com/product/3006) | I2S DAC + 3 W class-D amplifier |
| Mono enclosed speaker 3 W 4 Ω | [4445](https://www.adafruit.com/product/4445) | Speaker |
| IoT Button + NeoPixel BFF | [5666](https://www.adafruit.com/product/5666) | Trigger button (its NeoPixel is not used) |
| Single-cell LiPo, JST PH2.0 | e.g. Qimoo 503450 3.7 V 1000 mAh | Power |

> **Note:** the IoT Button (5666) is a "BFF" for QT Py / Xiao, so it won't stack
> on the Feather — we just use its button + GND pads and wire them over.

### Wiring

Signal chain: `ESP32 --I2S--> MAX98357A --> speaker`, triggered by the button.

| Feather pin | GPIO | Connects to |
|-------------|------|-------------|
| TX | 8 | MAX98357A **BCLK** |
| RX | 7 | MAX98357A **LRC** (word select) |
| D14 | 14 | MAX98357A **DIN** |
| D33 | 33 | MAX98357A **SD** (shutdown — HIGH = amp on) |
| D27 | 27 | Button **A2** pad (active-low trigger) |
| — | 2 | *On-board only:* NeoPixel / STEMMA QT power, held LOW |
| A13 | 35 | *On-board only:* BAT/2 via the Feather's divider |
| BAT | — | MAX98357A **Vin** |
| GND | — | MAX98357A **GND** and **GAIN**, button **GND**, speaker **–** |
| 3V3 | — | 10 kΩ pull-up to GPIO27 (see below) |
| — | — | MAX98357A **+/−** → speaker **+/−** |
| — | — | 10 kΩ from MAX98357A **SD** to **GND** |

GPIO2 and GPIO35 need no external wiring; they are listed because the firmware
drives or reads them, so anything repurposing those pins will conflict. The same
table lives in `src/board.rs` as rustdoc — keep the two in sync.

**Button pull-up:** GPIO27 is driven active-low (button to GND). The firmware
enables an internal pull-up, but for a reliable, low-leakage hold through deep
sleep, fit an external **10 kΩ resistor from GPIO27 to 3V3**.

**Amplifier pulldown:** Fit an external **10 kΩ resistor from MAX98357A SD to
GND**. GPIO33 overrides it during playback; the resistor guarantees the amp
stays shut down while the ESP32 is resetting or sleeping with high-impedance
GPIOs.

**Battery power:** The amp must use **BAT**, not USB or 3V3. BAT remains powered
when USB-C is unplugged and is within the MAX98357A's supply range for a
single-cell LiPo.

### Charging and battery monitoring

Connect a protected 3.7 V LiPo to the Feather's JST socket. The Feather hardware
automatically:

- powers the board from either USB-C or the battery;
- charges the battery whenever USB-C is connected; and
- lights the onboard `CHG` LED while charging.

Charging is hardware-controlled; the firmware does not change the charge rate
or termination behavior.

The Feather also has a built-in pair of 200 kΩ resistors that presents half of
the BAT voltage on **GPIO35 / ADC1**. On every boot the firmware discards one
settling sample, averages up to 16 readings (skipping any that fail), and
applies this policy:

| Approximate battery voltage | Behavior |
|-----------------------------|----------|
| Below 1.0 V | Treat as no battery / USB-only operation, play normally |
| 1.0–3.4 V | Log a critical warning, skip audio, return to sleep |
| 3.4–3.6 V | Play normally and log a charge-soon warning |
| 3.6 V and above | Play normally |

The no-battery threshold is deliberately far below any plausible LiPo voltage.
A collapsed cell in the 1.0–3.4 V range therefore fails closed instead of being
mistaken for USB power. If every ADC conversion fails, playback is also skipped;
a USB-only board with a working monitor still reads near zero and plays.

#### Calibrating the voltage scale

**This is the one step you must do before trusting any of the thresholds above.**

`esp-hal` provides no ADC calibration curve for the original ESP32, so
`src/battery.rs` ports Espressif's ADC1 11 dB line-fitting model. It prefers the
chip's two-point eFuse calibration when present; otherwise it uses the eFuse
`ADC_VREF` value, or the nominal 1100 mV fallback. Unlike an ideal attenuation
ratio, this model includes the measured slope and non-zero intercept.

To calibrate, flash and watch the log:

```
adc: Vref calibration, vref 1100 mV, coefficients (52798, 142)
battery: ~4200 mV (raw 2430, 0/16 conversions failed); normal
```

Measure BAT with a multimeter and compare. If a board still needs a one-point
correction, keep the logged intercept and solve the fixed-point slope:

```
coefficient_a = ((measured_BAT_mV / 2 - coefficient_b) * 65536) / raw
```

Override the selected `battery::Calibration` in `src/bin/main.rs` with the
measured coefficient. For higher confidence, measure several points across
3.2–4.2 V and verify the line rather than fitting a single sample. The host
tests use independent Espressif reference vectors, so do not rewrite those
vectors to make a board-specific correction pass.

The reading is also taken **before** the amplifier is enabled, so it is an
open-circuit voltage. The MAX98357A draws close to 900 mA at peak, which on a
1000 mAh cell is nearly 1C and will sag noticeably — a battery that passes the
check unloaded may still droop under load. Treat these numbers as coarse.

## How it works

The ESP32 spends most of its time in deep sleep. Pressing the button is an
**ext0 RTC wakeup** that resets the chip; `main` runs from the top, checks the
battery, plays when power is healthy, and sleeps again:

```
deep sleep --button--> boot --> sample BAT --> amp on --> play --> amp off --> deep sleep
```

The first cold boot plays `izzy-pine` (`clips::FIRST_BOOT`); later clips are
selected randomly without immediate repeats. The last clip index lives in RTC
fast RAM across deep-sleep resets. While awake, another button press interrupts
the current clip and starts a different one. When a clip finishes uninterrupted,
GPIO33 shuts down the amp and the board returns to deep sleep.

> **`esp-hal` is pinned exactly** (`=1.1.1`) because that RTC-fast retention
> relies on an upstream implementation detail: `RtcSleepConfig::deep()` intends to
> power down both RTC memories, but only the `slowmem` write actually happens —
> the `fastmem` one is commented out under a `TODO`. See `clips::LAST_CLIP`. After
> any `esp-hal` bump, press the button twice and confirm the log names two
> different clips.

The Feather itself is specified around **80–100 µA** from LiPo in deep sleep.
The finished assembly will draw more depending on the amplifier shutdown
current, battery-monitor divider, pull-ups, and battery protection board.

### Source layout

- `src/lib.rs` — crate root; declares the modules below
- `src/board.rs` — pin map, sample rate, and timing constants
- `src/battery.rs` — ADC conversion and low-battery policy *(host-tested)*
- `src/pcm.rs` — mono→stereo expansion into the DMA buffer *(host-tested)*
- `src/selection.rs` — which clip plays next, and the no-repeat rule *(host-tested)*
- `src/clips.rs` — embedded sound bites (`include_bytes!`) and the RTC-persistent last-played marker
- `src/audio.rs` — circular I2S streaming and press-to-interrupt playback
- `src/bin/main.rs` — wake → battery check → play → sleep control flow
- `build.rs` — adds esp-hal's linker script and friendlier linker errors
- `partitions.csv` — custom partition table (see [Adding sound bites](#adding-sound-bites))

`battery.rs`, `pcm.rs` and `selection.rs` are deliberately free of `esp-hal` and
of `use crate::...` imports so they can be compiled standalone against std for
testing. Keep them that way or `just test` stops working.

## Building & flashing

This targets the ESP32 (Xtensa), so it needs the Espressif Rust toolchain,
installed with [`espup`](https://github.com/esp-rs/espup):

```sh
cargo install espup
espup install          # installs the `esp` toolchain, rust-src, and the GCC linker
```

`espup` writes `~/export-esp.sh`, which puts `xtensa-esp32-elf-gcc` on `PATH`.
`rust-toolchain.toml` selects the `esp` toolchain; the
`xtensa-esp32-none-elf` target and the `build-std` settings (which require the
`rust-src` component) come from `.cargo/config.toml`.

Then use the [`justfile`](./justfile) — every recipe sources `~/export-esp.sh`
for you, so you don't need to source anything yourself:

```sh
just             # list recipes
just build       # compile (debug)
just check       # fast type-check
just clippy      # lint
just test        # host-side unit tests (battery + pcm + selection)
just verify      # clippy + test, run before committing
just flash       # flash release build + serial monitor over USB-C
just monitor     # open the serial monitor without reflashing
just size        # firmware size breakdown
just convert     # regenerate .pcm from the .mp3 sources
just play NAME   # preview a clip on your computer, e.g. `just play izzy-pine`
```

`just flash` runs `espflash flash --monitor` with the custom partition table
(see `.cargo/config.toml`), so you'll see the log output and can watch the
wake→play→sleep cycle.

If you'd rather use `cargo` directly, `source ~/export-esp.sh` first (add it to
your shell profile to make it automatic), then `cargo build` /
`cargo run --release`.

`cargo test` does **not** work: the default target is xtensa and the firmware is
`no_std` with no test harness. `just test` compiles the pure modules standalone
against std instead.

## Adding sound bites

Clips are raw **mono, 16-bit signed, little-endian PCM at 22 050 Hz** (no WAV
header), embedded with `include_bytes!`.

1. Drop an **MP3** in `assets/` — use a lowercase, hyphenated name.
2. Run `just convert` to generate the matching `.pcm`. Both files are committed,
   so a fresh clone builds without `ffmpeg`.
3. Add a `Clip` entry to `CLIPS` in `src/clips.rs`.
4. `just play NAME` to preview, `just size` to check the budget.

`just convert` only looks at `assets/*.mp3`. To use a source in another format,
convert it by hand with the same flags:

```sh
ffmpeg -i kruk_and_kuip.wav -ac 1 -ar 22050 -f s16le assets/kruk.pcm
```

### Size budget

The app image lives in the **6 MB `factory` partition** defined by
`partitions.csv` — not the full 8 MB of flash. The default espflash table would
only give the app about 1 MB, which is why the custom table exists; the cargo
runner passes it automatically.

Each second of audio is ~44 KB and the code is well under 100 KB, so the ceiling
is roughly **140 seconds of audio in total**. The three clips currently shipped
use about 77 s (~3.3 MB), leaving ~60 s of headroom. `bonds` and `bumgarner` are
long highlight calls at roughly 40 s and 33 s; trim them if hardware testing
shows that awake-time, battery-life, or interaction latency is unacceptable.

## Hardware validation still required

- **Validate the ADC calibration**: compare the logged GPIO35 voltage with a
  multimeter and adjust the line-fit coefficient if needed (see
  [Calibrating the voltage scale](#calibrating-the-voltage-scale)). Until this is
  done the battery thresholds are guesses.
- **Confirm no-repeat selection survives deep sleep**: press twice, check the log
  names two different clips.
- Measure deep-sleep current with the amp SD pulldown fitted.
- Verify battery-only playback with the amp powered from BAT.
- Verify the GPIO27 external pull-up holds reliably through deep sleep.
