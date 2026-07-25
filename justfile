# Giants Bobblehead — build & flash recipes for the ESP32 Feather V2.
#
# Every recipe sources the Espressif toolchain env (~/export-esp.sh) so the
# xtensa-esp32-elf-gcc linker is on PATH — no need to source it in your shell.
#
# Note on comments: `just --list` shows only the LAST comment line above a
# recipe, so each recipe below keeps its summary on a single final line and puts
# any detail above a blank line.

# Sourced before each cargo/espflash invocation.
esp := ". ~/export-esp.sh"

# Firmware paths.
elf_release := "target/xtensa-esp32-none-elf/release/giants-bobblehead"

# Host-testable modules. These are compiled standalone by `just test`, which
# only works while they have no `use crate::...` imports and no esp-hal
# dependency. Adding either breaks the recipe — see src/lib.rs.
host_test_modules := "battery pcm selection"

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
    {{esp}} && cargo clippy -- -D warnings

# Check formatting without changing files.
fmt-check:
    cargo fmt --all -- --check

# Formatting, lint, host tests, and a release link — run before committing.
verify: fmt-check clippy test build-release

# `cargo test` can't work here: the default target is xtensa and the firmware is
# no_std with no test harness. Instead the pure modules are compiled standalone
# against std, at the same edition as the crate.
#
# Run the host-side unit tests.
test:
    #!/usr/bin/env sh
    set -e
    mkdir -p target/host-tests
    for m in {{host_test_modules}}; do
        echo "--- $m ---"
        rustc +stable --edition 2024 --test "src/$m.rs" -o "target/host-tests/$m"
        "target/host-tests/$m"
    done

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

# Raw PCM has no header, so ffmpeg wraps it (mono s16le @ 22050 Hz) into a WAV
# that macOS's afplay can play — quiet, no progress spam.
#
# Preview a converted clip, e.g. `just play izzy-pine`.
play name:
    #!/usr/bin/env sh
    set -e
    name={{quote(name)}}
    case "$name" in
        ""|*[!a-z0-9-]*)
            echo "clip name must contain only lowercase letters, digits, and hyphens" >&2
            exit 2
            ;;
    esac
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    ffmpeg -hide_banner -v error -y -f s16le -ar 22050 -ac 1 \
        -i "assets/$name.pcm" "$tmp/clip.wav"
    afplay "$tmp/clip.wav"

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
