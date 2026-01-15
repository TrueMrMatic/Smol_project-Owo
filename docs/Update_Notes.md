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
