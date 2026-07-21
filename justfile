# Giants Bobblehead — build & flash recipes for the ESP32 Feather V2.
#
# Every recipe sources the Espressif toolchain env (~/export-esp.sh) so the
# xtensa-esp32-elf-gcc linker is on PATH — no need to source it in your shell.

# Sourced before each cargo/espflash invocation.
esp := ". ~/export-esp.sh"

# Firmware paths.
elf_debug   := "target/xtensa-esp32-none-elf/debug/giants-bobblehead"
elf_release := "target/xtensa-esp32-none-elf/release/giants-bobblehead"

# Show available recipes.
default:
    @just --list

# Compile (debug).
build:
    {{esp}} && cargo build

# Compile (release — smaller, faster; use this for real deployment).
build-release:
    {{esp}} && cargo build --release

# Fast type-check, no codegen.
check:
    {{esp}} && cargo check

# Lint with clippy.
clippy:
    {{esp}} && cargo clippy

# Flash release build to the ESP32 over USB and open the serial monitor.
flash:
    {{esp}} && cargo run --release

# Flash debug build + monitor (bigger binary, faster build).
flash-debug:
    {{esp}} && cargo run

# Open the serial monitor without reflashing.
monitor:
    {{esp}} && espflash monitor

# Print firmware size breakdown (release).
size: build-release
    {{esp}} && xtensa-esp32-elf-size {{elf_release}}

# Preview a converted clip on your computer, e.g. `just play pine`.
# Raw PCM has no header, so ffmpeg wraps it (mono s16le @ 22050 Hz) into a WAV
# that macOS's afplay can play — quiet, no progress spam.
play name:
    #!/usr/bin/env sh
    set -e
    tmp="$(mktemp -t gb_play).wav"
    ffmpeg -hide_banner -v error -y -f s16le -ar 22050 -ac 1 -i "assets/{{name}}.pcm" "$tmp"
    afplay "$tmp"
    rm -f "$tmp"

# Regenerate PCM clips from the MP3 sources (mono, s16le, 22050 Hz).
convert:
    #!/usr/bin/env sh
    set -e
    for f in assets/*.mp3; do
        out="${f%.mp3}.pcm"
        echo "converting $f -> $out"
        ffmpeg -y -v error -i "$f" -ac 1 -ar 22050 -f s16le "$out"
    done

# Remove build artifacts.
clean:
    cargo clean
