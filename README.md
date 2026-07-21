# giants-bobblehead

A low-power **ESP32 soundboard** in Rust: press a button, hear a Giants
announcer sound bite, and the board drops straight back into deep sleep. Built
`no_std` on [`esp-hal`](https://github.com/esp-rs/esp-hal) so it can sit on a
LiPo for months and wake only when the button is pressed.

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
| D14 | 14 | MAX98357A **BCLK** |
| D15 | 15 | MAX98357A **LRC** (word select) |
| D32 | 32 | MAX98357A **DIN** |
| D33 | 33 | MAX98357A **SD** (shutdown — HIGH = amp on) |
| D27 | 27 | Button → **GND** (RTC-capable, ext0 wakeup) |
| 3V3 | — | MAX98357A **Vin** |
| GND | — | MAX98357A **GND**, button, speaker − |
| — | — | MAX98357A **+/−** → speaker **+/−** |

**Button pull-up:** GPIO27 is driven active-low (button to GND). The firmware
enables an internal pull-up, but for a reliable, low-leakage hold through deep
sleep, fit an external **10 kΩ resistor from GPIO27 to 3V3**.

**Zero-wiring first test:** the Feather's onboard user button is **GPIO38** and
already has a hardware pull-up — handy to prove out wake→play→sleep before the
external button is wired. (Change the pin in `src/bin/main.rs`.)

## How it works

The ESP32 sits in **deep sleep** (~10 µA). Pressing the button is an **ext0 RTC
wakeup** that resets the chip; `main` runs from the top, plays the next clip, and
sleeps again:

```
deep sleep --button--> boot --> amp on --> stream clip over I2S --> amp off --> deep sleep
```

Clips cycle round-robin via a counter kept in RTC RAM that survives the reset.
The amp's `SD` pin is held low except during playback so it draws no idle current.

### Source layout

- `src/board.rs` — pin map, sample rate, and timing constants
- `src/clips.rs` — embedded sound bites (`include_bytes!`)
- `src/audio.rs` — I2S streaming (flash→RAM copy, mono→stereo expansion)
- `src/bin/main.rs` — wake → play → sleep control flow

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

## Roadmap / not yet done

- Real Giants announcer clips (currently a placeholder arpeggio tone)
- NeoPixel status flash on wake (BFF LED / onboard NeoPixel on GPIO0)
- Gapless playback via `write_dma_circular_async` (current chunked one-shot may
  have tiny gaps between DMA transfers on long clips)
- On-hardware verification of ext0 pull-up hold through deep sleep
