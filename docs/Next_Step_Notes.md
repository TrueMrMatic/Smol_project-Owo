# Next Step Notes — for the next patch after PATCH_009_MASKS

You are continuing from PATCH_009_MASKS on top of PATCH_008_TEXT_VECTOR.

## Current state
- Run bundles and LAST_RUN pointers are flash-only (`sdmc:/flash/_runs/...`).
- Runlog verbosity is locked to 2; no runtime toggle remains in the UI.
- Rectangle mode is removed; triangles are the only path, with improved fill winding for complex shapes.
- Bounds fallbacks are now counted/logged so missing mesh cases are visible.
- Bitmap transforms now use a textured triangle path (toggleable via `sdmc:/flash/renderer.cfg`).
- Strokes now tessellate into triangle strips with bounds-outline fallbacks.
- Vector text is routed through dedicated text draw commands with bounds fallbacks.
- Rect masks apply scissor clipping (toggleable via `masks_enabled`).
- Run logs reinitialize per SWF and snapshots now include last stage, cache stats, draw stats, and recent warnings.
- X triggers a one-frame command dump to correlate command lists with draw stats.

## Inputs you will receive
- A run bundle copied from `sdmc:/flash/_runs/...` with boottrace/last_stage/status snapshots.
- Possibly a SWF that still looks “rectangular” due to missing renderer coverage.

## Next theme (ONLY ONE)
Renderer coverage expansion — move beyond bounds rectangles:
1. **Shapes:** validate fill mesh coverage (reduce bounds fallbacks; inspect warnings).
2. **Bitmaps:** validate textured bitmap coverage and confirm config toggle behavior.
3. **Strokes:** validate stroke coverage and tune joins/caps as needed.
4. **Text:** render static text (vector glyphs or bitmap atlas).
5. **Masks:** basic clip masks (rect + simple shape masks) with safe fallbacks.

## Acceptance criteria
- Complex vector shapes render as triangles, not bounds rectangles.
- Bitmap fills render with transforms (no more unscaled blits only).
- Stroked outlines appear for common artwork (constant width at minimum).
- Static text is visible (even if using a single-font or atlas fallback).
- Masked content clips correctly in at least rect masks; unsupported masks degrade gracefully.

## Implementation checklist
- Inspect bounds fallback warnings and confirm fill meshes exist for common shapes.
- Extend render commands to carry bitmap transforms + UVs and add textured-triangle draw path.
- Add stroke meshes to the shape cache and draw them after fills.
- Choose a single text pipeline (vector glyphs or bitmap atlas) and implement minimal coverage.
- Add mask push/pop commands and a safe scissor-based fallback.

## Out of scope
- Audio, AVM work, or non-renderer refactors.
- Unbounded work or per-frame allocations in the render path.
