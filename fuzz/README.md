# Fuzzing Rustboy Advance (`cargo-fuzz`)

The `fuzz/` directory defines an independent Cargo workspace (`[workspace]` in `fuzz/Cargo.toml`) so normal workspace builds (`cargo test --workspace`) do not require a nightly compiler or libFuzzer linker.

## Targets

| Target | Entrypoint | What It Fuzzes |
|--------|------------|----------------|
| `fuzz_load_state` | `fuzz_targets/fuzz_load_state.rs` | `GbaCore::load_state(&[u8])` deserialization, fastmem rebuild, and state validation against malformed/arbitrary bytes |
| `fuzz_disasm` | `fuzz_targets/fuzz_disasm.rs` | `gba_debugger::disassemble_arm` and `gba_debugger::disassemble_thumb` across arbitrary 32-bit/16-bit instruction encodings and PCs |
| `fuzz_rom_boot` | `fuzz_targets/fuzz_rom_boot.rs` | `GbaCore::new(None, rom)` header/save-type detection and 4 frames of execution with arbitrary ROM payloads (up to 32 MiB) |

## Prerequisites

`cargo-fuzz` requires a **nightly** Rust toolchain (and on Linux/macOS a C/C++ linker with libFuzzer/ASan; on Windows, MSVC + Clang/LLVM sanitizers or WSL2 Ubuntu).

```bash
rustup toolchain install nightly
cargo install cargo-fuzz
```

## Running

Run any target from the repository root (pass `--fuzz-dir fuzz` or run inside `fuzz/`):

```bash
# Fuzz save-state deserialization
cargo +nightly fuzz run --fuzz-dir fuzz fuzz_load_state

# Fuzz ARM & Thumb disassemblers
cargo +nightly fuzz run --fuzz-dir fuzz fuzz_disasm

# Fuzz cartridge parsing & early boot execution (with 60s time limit)
cargo +nightly fuzz run --fuzz-dir fuzz fuzz_rom_boot -- -max_total_time=60
```

Corpus and crash artifacts are written to `fuzz/corpus/` and `fuzz/artifacts/` (both gitignored).

To verify type-checking without running libFuzzer:

```bash
cargo check --manifest-path fuzz/Cargo.toml
```
