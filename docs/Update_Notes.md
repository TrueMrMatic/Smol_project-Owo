# Update Notes — PATCH_001_PROTOCOL_INIT_v2

Build ID: PATCH_001_PROTOCOL_INIT_v2  
Base: BASELINE_000_BEFORE_BUG  
Theme: Protocol scaffolding + **low-overhead** run logging (restore loading FPS)

## Summary
- Replaced the initial run logger with a **buffered, rate-limited** logger to avoid SD I/O destroying FPS.
- `last_stage.txt` updates are now **rate-limited** and **forced only when entering heavy phases**, instead of being written every frame.
- Added a bottom-screen log window that shows **important boottrace lines in real time**, without spamming a heartbeat.
- Logs are created **automatically when launching a SWF** (no need to press Y to get files).

## Why loading became slow in v1
v1 wrote `last_stage.txt` every frame and flushed every boottrace line immediately to SD.  
That causes heavy filesystem churn on 3DS and drops FPS during loading.

## Changed files
- rust/bridge/src/runlog.rs
- rust/bridge/src/engine/mod.rs
- rust/bridge/src/ffi/exports.rs
- rust/bridge/src/lib.rs
- source/main.c
- FIRST_PROMPT.txt
- docs/Update_Notes.md
- docs/Next_Step_Notes.md

## Behavior changes
- Boottrace files are written automatically under:
  - Primary: `sdmc:/flash/_runs/PATCH_001_PROTOCOL_INIT_v2/<swf>/`
  - Fallback: `sdmc:/3ds/ruffle3ds_runs/...`
  - The active run dir is also written to `sdmc:/3ds/ruffle3ds_last_run.txt`
- Bottom screen shows a rolling window of important log lines.
- SELECT cycles runlog verbosity (0/1/2).

## Risks / Watch-outs
- Verbosity 2 can still slow loading if the SWF registers lots of shapes (expected). Use it only when diagnosing.

- Added stage markers around shape registration/tessellation (shape id visible in last_stage.txt).

# Update Notes — PATCH_002_UI_SNAPSHOT_SAFETY

Build ID: PATCH_002_UI_SNAPSHOT_SAFETY  
Base: PATCH_001_PROTOCOL_INIT_v2  
Theme: Bottom-screen UX rework + snapshot safety

## Summary
- Reworked the bottom-screen layout for SWF playback: fixed control legend, log window, notice line, warning line, and HUD line.
- Added a snapshot cooldown and on-screen notice to avoid accidental rapid SD writes.
- Y now writes a status snapshot only (no command dump spam) to reduce crash risk.

## Changed files
- source/main.c
- docs/Update_Notes.md
- docs/Next_Step_Notes.md

## Behavior changes
- Bottom screen now always shows controls + a dedicated log window + status lines.
- Pressing **Y** shows a short “snapshot saved” notice and is rate-limited.
- Command dumps are no longer triggered by Y during normal playback.

## Risks / Watch-outs
- If command dumps are needed, we will add a dedicated hotkey in a future patch.

# Update Notes — PATCH_003_UI_Y_HARD_FIX

Build ID: PATCH_003_UI_Y_HARD_FIX  
Base: PATCH_002_UI_SNAPSHOT_SAFETY  
Theme: Y snapshot hard-fix + remove rectangle mode + UI polish

## Summary
- Moved snapshot writes to a deferred queue flushed during the engine tick to avoid SD I/O on the input path.
- Removed rectangle-only render mode; triangle rendering is now the sole path.
- Tweaked the bottom-screen layout with a separator and simplified controls text.

## Changed files
- rust/bridge/src/runlog.rs
- rust/bridge/src/engine/mod.rs
- rust/bridge/src/ffi/exports.rs
- rust/bridge/src/ruffle_adapter/threed_backend.rs
- source/main.c
- docs/Update_Notes.md
- docs/Next_Step_Notes.md

## Behavior changes
- Pressing **Y** queues a snapshot; it is flushed asynchronously during ticks.
- HUD no longer shows mode flags (mR/mT); only triangle rendering remains.

## Risks / Watch-outs
- Snapshot writes are rate-limited; if you press Y repeatedly, snapshots may be queued and flushed over time.

# Update Notes — PATCH_004_FLASH_ONLY_LOGS

Build ID: PATCH_004_FLASH_ONLY_LOGS  
Base: PATCH_003_UI_Y_HARD_FIX  
Theme: Flash-only run artifacts + verbosity lock

## Summary
- Locked runlog verbosity to level 2 and removed the SELECT verbosity control from the UI.
- Ensured run bundle artifacts and LAST_RUN pointers are written only under `sdmc:/flash/`.

## Changed files
- rust/bridge/src/runlog.rs
- rust/bridge/src/ffi/exports.rs
- source/main.c
- docs/Update_Notes.md
- docs/Next_Step_Notes.md

## Behavior changes
- Run artifacts are now written only under `sdmc:/flash/_runs/...` (no fallback writes to `sdmc:/3ds/...`).
- Verbosity is fixed at 2; no user-facing verbosity toggle remains.

## Risks / Watch-outs
- If the `sdmc:/flash/` folder is missing or unwritable, run bundle writes will fail instead of falling back elsewhere.

# Update Notes — PATCH_005_SHAPE_FILL_MESHES

Build ID: PATCH_005_SHAPE_FILL_MESHES  
Base: PATCH_004_FLASH_ONLY_LOGS  
Theme: Shape fill mesh correctness + fallback visibility

## Summary
- Oriented fill/holes winding before triangulation to improve earcut success on complex shapes.
- Added counters + warnings for missing/invalid fill meshes and bounds-rect fallbacks.
- Hardened the executor so bounds fallbacks happen only when mesh data is missing or invalid.

## Changed files
- rust/bridge/src/ruffle_adapter/tessellate.rs
- rust/bridge/src/ruffle_adapter/threed_backend.rs
- rust/bridge/src/render/cache/shapes.rs
- rust/bridge/src/render/executor.rs
- rust/bridge/src/runlog.rs
- docs/Update_Notes.md
- docs/Next_Step_Notes.md

## Behavior changes
- Shape fill meshes now normalize winding (outer vs holes) before earcut.
- Missing/invalid fill meshes emit warnings (rate-limited) and increment counters when falling back to bounds.

## Risks / Watch-outs
- If a shape registers with malformed contours, the executor will still fall back to bounds; warnings are logged to help identify offenders.

# Update Notes — PATCH_006_BITMAP_TRANSFORMS

Build ID: PATCH_006_BITMAP_TRANSFORMS  
Base: PATCH_005_SHAPE_FILL_MESHES  
Theme: Bitmap transforms + textured triangles (gated)

## Summary
- Extended bitmap render commands to carry transforms, UVs, and color transforms.
- Added a textured-triangle raster path (nearest-neighbor) for transformed bitmap fills.
- Introduced a runtime config toggle (`sdmc:/flash/renderer.cfg`) to enable/disable textured bitmaps.

## Changed files
- rust/bridge/src/render/frame.rs
- rust/bridge/src/render/device/mod.rs
- rust/bridge/src/render/device/fb3ds.rs
- rust/bridge/src/render/executor.rs
- rust/bridge/src/ruffle_adapter/threed_backend.rs
- rust/bridge/src/util/config.rs
- rust/bridge/src/util/mod.rs
- rust/bridge/src/runlog.rs
- docs/Project_Guide.md
- docs/Update_Notes.md
- docs/Next_Step_Notes.md

## Behavior changes
- Bitmap fills can now render with transforms (scale/rotate) via textured triangles.
- If `textured_bitmaps` is disabled in `renderer.cfg`, only identity/axis-aligned blits render.

## Risks / Watch-outs
- The textured path is heavier than blitting; disable via config if performance regresses.

# Update Notes — PATCH_007_STROKE_MESHES

Build ID: PATCH_007_STROKE_MESHES  
Base: PATCH_006_BITMAP_TRANSFORMS  
Theme: Stroke tessellation + fallback visibility

## Summary
- Added constant-width stroke tessellation (butt caps, miter joins) alongside fills.
- Stored stroke meshes separately in the shape cache and emitted stroke draw commands.
- Added bounds-only stroke fallbacks with logging/counters when stroke meshes are missing.

## Changed files
- rust/bridge/src/ruffle_adapter/tessellate.rs
- rust/bridge/src/render/cache/shapes.rs
- rust/bridge/src/render/frame.rs
- rust/bridge/src/render/executor.rs
- rust/bridge/src/ruffle_adapter/threed_backend.rs
- rust/bridge/src/runlog.rs
- docs/Update_Notes.md
- docs/Next_Step_Notes.md

## Behavior changes
- Strokes render as filled triangle strips with constant width (no caps beyond butt).
- Missing or invalid stroke meshes fall back to a bounds-only outline.

## Risks / Watch-outs
- Miter joins can spike width on sharp angles; check logs if you see large stroke artifacts.

# Update Notes — PATCH_008_TEXT_VECTOR

Build ID: PATCH_008_TEXT_VECTOR  
Base: PATCH_007_STROKE_MESHES  
Theme: Vector text routing + fallback

## Summary
- Treat vector glyphs as shape fills and emit distinct text draw commands for profiling.
- Added a text fill fallback that draws a bounds rectangle when glyph tessellation is missing/invalid.

## Changed files
- rust/bridge/src/ruffle_adapter/threed_backend.rs
- rust/bridge/src/render/frame.rs
- rust/bridge/src/render/cache/shapes.rs
- rust/bridge/src/render/executor.rs
- rust/bridge/src/runlog.rs
- docs/Update_Notes.md
- docs/Next_Step_Notes.md

## Behavior changes
- Vector text uses dedicated text commands (`DrawTextSolidFill`) for easier profiling.
- When glyph tessellation fails, a low-cost bounds fill is rendered instead.

## Risks / Watch-outs
- Text detection is heuristic (shape id + fill-only); verify on real SWFs and adjust if needed.

# Update Notes — PATCH_009_MASKS

Build ID: PATCH_009_MASKS  
Base: PATCH_008_TEXT_VECTOR  
Theme: Rect mask scissoring (gated)

## Summary
- Added mask render commands and a rect-only scissor implementation for basic masks.
- Emitted rect mask commands from the backend and ignored unsupported mask cases with warnings.
- Added a runtime toggle (`masks_enabled`) in `sdmc:/flash/renderer.cfg`.

## Changed files
- rust/bridge/src/render/frame.rs
- rust/bridge/src/render/device/mod.rs
- rust/bridge/src/render/device/fb3ds.rs
- rust/bridge/src/render/executor.rs
- rust/bridge/src/ruffle_adapter/threed_backend.rs
- rust/bridge/src/util/config.rs
- rust/bridge/src/runlog.rs
- docs/Project_Guide.md
- docs/Update_Notes.md
- docs/Next_Step_Notes.md

## Behavior changes
- Rect masks apply a scissor rectangle to subsequent draw calls.
- Unsupported masks log warnings and render without masking.

## Risks / Watch-outs
- Only axis-aligned rect masks are supported; rotated/shape masks are ignored.
