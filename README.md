# Ruffle 3DS (Trash chatbot prototype)

Hybrid **C (3DS homebrew)** + **Rust (Ruffle-based core + renderer bridge)** Flash player prototype.

This repo is intentionally optimized for *incremental renderer bring-up* on a very constrained device. The current focus is correctness + robustness with aggressive safeguards against unbounded work.

## Quick start

### Prerequisites
- devkitARM / devkitPro toolchain (3DS)
- `cargo-3ds` + nightly toolchain
- A 3DS with homebrew access

### Build
```sh
make
```

The build pipeline compiles the Rust bridge as a staticlib and links it into the 3DS app via the top-level `Makefile`.

### Run
1. Copy the generated `.3dsx` to your SD card.
2. Place SWFs on the SD card (any location).
3. Launch the app, pick a SWF, and monitor the bottom-screen logs.

## Documentation index
- `docs/Project_Guide.md` — renderer-first roadmap + rules
- `docs/Workflow.md` — iteration protocol and feedback format
- `docs/Architecture.md` — data flow and module responsibilities
- `docs/Regression_Suite.md` — practical regression checks
- `docs/SD_Run_Artifacts.md` — run bundle contents and paths

## Goals (short-term)
- Robust pipeline + instrumentation (no silent freezes)
- Solid triangle fills (safe + capped)
- Clear fallbacks for missing features

## Non-goals (for now)
- Perfect fidelity
- Full feature parity with desktop Ruffle

