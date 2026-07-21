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

# Remove build artifacts.
clean:
    cargo clean
