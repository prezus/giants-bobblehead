# giants-bobblehead

A battery-powered **ESP32 soundboard** in Rust: press a button, hear a Giants
announcer sound bite, and the board drops straight back into deep sleep. It is
built `no_std` on [`esp-hal`](https://github.com/esp-rs/esp-hal), monitors its
LiPo at boot, and shuts down the amplifier between plays.

## Hardware

| Part | Product | Role |
|------|---------|------|
| Adafruit **ESP32 Feather V2** | [5400](https://www.adafruit.com/product/5400) | MCU (Xtensa dual-core, 8 MB flash, 2 MB PSRAM) |
| **MAX98357A** I2S amp | [3006](https://www.adafruit.com/product/3006) | I2S DAC + 3 W class-D amplifier |
| Mono enclosed speaker 3 W 4 Ω | [4445](https://www.adafruit.com/product/4445) | Speaker |
| IoT Button + NeoPixel BFF | [5666](https://www.adafruit.com/product/5666) | Trigger button (NeoPixel unused for now) |

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
| BAT | — | MAX98357A **Vin** |
| GND | — | MAX98357A **GND** and **GAIN**, button **GND** |
| — | — | MAX98357A **+/−** → speaker **+/−** |

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
settling sample, averages 16 readings, and applies this policy:

| Approximate battery voltage | Behavior |
|-----------------------------|----------|
| Below 2.5 V | Treat as no battery / USB-only operation |
| 2.5–3.4 V | Log a critical warning, skip audio, return to sleep |
| 3.4–3.6 V | Play normally and log a charge-soon warning |
| 3.6 V and above | Play normally |

The classic ESP32 ADC is noisy and the current HAL does not apply per-device
calibration, so the voltage is intentionally approximate. Verify the logged
reading against a multimeter before relying on the thresholds in a finished
unit. This is a runtime power policy, not a substitute for a protected LiPo or
the Feather's hardware protection.

## How it works

The ESP32 spends most of its time in deep sleep. Pressing the button is an
**ext0 RTC wakeup** that resets the chip; `main` runs from the top, checks the
battery, plays when power is healthy, and sleeps again:

```
deep sleep --button--> boot --> sample BAT --> amp on --> play --> amp off --> deep sleep
```

The first cold boot plays `izzy-pine`; later clips are selected randomly without
immediate repeats. The last clip index lives in RTC RAM across deep-sleep
resets. While awake, another button press interrupts the current clip and starts
a different one. When a clip finishes uninterrupted, GPIO33 shuts down the amp
and the board returns to deep sleep.

The Feather itself is specified around **80–100 µA** from LiPo in deep sleep.
The finished assembly will draw more depending on the amplifier shutdown
current, battery-monitor divider, pull-ups, and battery protection board.

### Source layout

- `src/board.rs` — pin map, sample rate, and timing constants
- `src/battery.rs` — ADC conversion and low-battery policy
- `src/clips.rs` — embedded sound bites (`include_bytes!`)
- `src/audio.rs` — circular I2S streaming and press-to-interrupt playback
- `src/bin/main.rs` — wake → battery check → play → sleep control flow

## Building & flashing

This targets the ESP32 (Xtensa), so it needs the Espressif Rust toolchain. The
`esp` toolchain is already installed here (via [`espup`](https://github.com/esp-rs/espup)),
and the pinned toolchain + `xtensa-esp32-none-elf` target are selected
automatically via `rust-toolchain.toml` and `.cargo/config.toml`.

Use the [`justfile`](./justfile) — every recipe sources the Espressif env
(`~/export-esp.sh`) so the GCC linker is on `PATH`; you don't need to source
anything yourself:

```sh
just             # list recipes
just build       # compile (debug)
just flash       # flash release build + serial monitor over USB-C
just monitor     # open the serial monitor without reflashing
just size        # firmware size breakdown
just test-battery # host-side battery policy tests
```

`just flash` runs `espflash flash --monitor` (see `.cargo/config.toml`), so
you'll see the log output and can watch the wake→play→sleep cycle.

If you'd rather use `cargo` directly, `source ~/export-esp.sh` first (add it to
your shell profile to make it automatic), then `cargo build` / `cargo run --release`.

## Adding real sound bites

Clips are raw **mono, 16-bit signed, little-endian PCM at 22 050 Hz** (no WAV
header). Convert an audio file and drop it in `assets/`:

```sh
ffmpeg -i kruk_and_kuip.wav -ac 1 -ar 22050 -f s16le assets/kruk.pcm
```

Then add an entry to `CLIPS` in `src/clips.rs`. Keep clips short — each second of
audio is ~44 KB, and 8 MB flash holds plenty of them.

## Hardware validation still required

- Compare the logged GPIO35 voltage with a multimeter and tune the ADC scale or
  thresholds if necessary.
- Measure deep-sleep current with the amp SD pulldown fitted.
- Verify battery-only playback with the amp powered from BAT.
- Verify the GPIO27 external pull-up holds reliably through deep sleep.
