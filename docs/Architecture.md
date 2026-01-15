# Architecture Overview

This project is a **hybrid C + Rust** pipeline. The C side owns the 3DS app lifecycle and hardware-facing APIs. The Rust side integrates a Ruffle-based core and emits a render command list that the 3DS backend consumes.

## Data flow (high level)

```
SWF → Ruffle core (Rust)
    → CommandList (Rust)
    → FFI bridge (C ABI)
    → 3DS renderer backend (C/Rust)
    → Framebuffer (top screen)
```

## Module responsibilities

### C (3DS app)
- Initializes graphics + console
- Handles input and SWF selection
- Calls into Rust via `bridge_*` symbols
- Displays HUD + bottom-screen log window

### Rust (bridge crate)
- Owns player lifecycle and tick loop
- Adapts Ruffle output into a compact `CommandList`
- Enforces caps + fallbacks for heavy operations
- Emits run logs + stage markers for debugging

## Debugging artifacts
Every run writes a **run bundle** with stage markers and status snapshots under:

`sdmc:/flash/_runs/<BUILD_ID>/<SWF_NAME>/`

See `docs/SD_Run_Artifacts.md` for the exact file list.

### Diagnostics flow
- Run bundles are reinitialized per SWF selection.
- Y writes multi-line diagnostic snapshots (last stage, cache stats, draw stats, warnings).
- X requests a one-frame command dump to correlate command lists with mesh usage.

## Performance constraints
- Avoid per-frame allocations in the render path
- Avoid per-frame filesystem writes
- Prefer linear-time operations with strict caps
