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
