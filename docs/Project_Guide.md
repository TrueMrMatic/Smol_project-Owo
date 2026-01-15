# Project Guide (Renderer-focused)

This project is a hybrid **C (3DS app)** + **Rust (Ruffle-based core + renderer bridge)** Flash player prototype.

This guide is intentionally **renderer-focused** for now. Other subsystems (audio, input, AVM) are out of scope until the renderer is reliable.

## Golden rules
1. **One theme per iteration.** No mixed refactors + new features in the same patch.
2. **Never introduce unbounded work** in the render path (no O(n²) grouping without caps; no per-frame allocations without limits).
3. Any new heavy behavior must be **toggleable at runtime** via an SD config file or build flag.
4. All runs must emit a **run bundle** under `sdmc:/flash/_runs/<BUILD_ID>/<SWF_NAME>/`.

### Runtime config
Runtime toggles live in `sdmc:/flash/renderer.cfg` (simple `key=value` lines).

Current keys:
- `textured_bitmaps=1|0` — enable/disable transformed bitmap rendering.
- `masks_enabled=1|0` — enable/disable mask scissor application.

## Current renderer status
- Displays basic frames via a `CommandList` emitted from Ruffle to the 3DS backend.
- Supports a minimal subset of shape drawing (bounds rectangles and/or triangle fill meshes depending on mode).
- Not feature-complete: strokes, gradients, bitmaps, text, masks, blend modes, filters are incomplete or missing.

## Debug controls
- **Y**: write a multi-line diagnostic snapshot (last stage, cache stats, draw stats, recent warnings).
- **X**: request a one-shot command dump for the next frame (for correlating command lists).
- **L (hold)**: wireframe overlay for triangle edges.

## Roadmap to a practical Flash renderer on 3DS
The goal is not perfect fidelity first; it's **robustness** + **incremental coverage**.

### Phase A — Robust pipeline + instrumentation (must never regress)
- Stage markers for every render step (tick → render → submit → pull frame → rasterize).
- Watchdog / cap system for expensive steps (tessellation, triangulation, cache growth).
- Fail-soft fallbacks (bounds-only rendering when tessellation fails).
- Run bundles auto-written to SD (boottrace, last stage, status snapshots, warnings).

### Phase B — Shapes (fills) correctness
1. Solid triangle fills for arbitrary paths (including holes where feasible).
2. Coordinate transforms: matrix concatenation, world→screen mapping correctness.
3. Culling/clipping and bounds sanity.

### Phase C — Bitmaps
1. Decode embedded bitmaps; upload/copy to CPU cache.
2. Draw bitmap fills with transforms (no filtering first; nearest-neighbor).
3. Optional: simple bilinear filter if performance allows.

### Phase D — Strokes
- Basic strokes: constant width, joins, caps.
- Then miter/round joins approximations.

### Phase E — Text (basic)
- Render static text as vector shapes or bitmap glyph atlases (depending on Ruffle path).
- Start with single font, no hinting.

### Phase F — Masks / blend modes / filters (later)
- Masks (clip paths)
- Blend modes
- Drop shadow / blur (optional)

## What counts as “done enough”
- A reasonable set of real-world SWFs loads without freezing.
- Missing features degrade gracefully (fallback), never hard-freeze.
- Logs always allow pinpointing the stage and last processed item.
