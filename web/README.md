# Rustboy Advance — WebAssembly Frontend

Static web frontend for Rustboy Advance (`gba-frontend-web`), powered by Rust + `wasm-bindgen`, Canvas 2D pixel presentation, `AudioWorklet` dynamic-rate resampling, Gamepad API, touch controls, and IndexedDB persistence.

## Prerequisites

1. **Rust `wasm32-unknown-unknown` target**:
   ```sh
   rustup target add wasm32-unknown-unknown
   ```
2. **`wasm-bindgen-cli`** matching the `wasm-bindgen` version in `Cargo.lock` (`0.2.130`):
   ```sh
   cargo install wasm-bindgen-cli --version 0.2.130 --locked
   ```
   *(Or download the matching prebuilt binary from the `wasm-bindgen` `0.2.130` GitHub release.)*

## Build WASM Package

Run from the repository root:

```sh
cargo build --target wasm32-unknown-unknown --release -p gba-frontend-web
wasm-bindgen --target web --out-dir web/pkg target/wasm32-unknown-unknown/release/gba_frontend_web.wasm
```

This generates `web/pkg/gba_frontend_web.js` and `web/pkg/gba_frontend_web_bg.wasm` (ignored by git and built automatically in CI).

## Run Local Dev Server

Browsers require an HTTP server for ES modules, WebAssembly streaming compilation, and `AudioWorklet` (do not open `index.html` over `file://`).

Using Node.js:

```sh
npx serve web
```

Or using Python 3:

```sh
python -m http.server 8080 --directory web
```

Then open `http://localhost:8080` (or the port printed by `serve`).

## Default Controls

| GBA Input | Keyboard | Gamepad |
|-----------|----------|---------|
| D-Pad | Arrow Keys | D-Pad / Left Stick |
| A / B | `Z` / `X` | South / East (swappable in Settings) |
| L / R | `A` / `S` | LB / RB (or LT / RT) |
| Start / Select | `Enter` / `Backspace` | Start / Back |
| Pause / Resume | `P` or `Space` | — |
| Fast Forward | `F` | — |
| Mute Toggle | `M` | — |
