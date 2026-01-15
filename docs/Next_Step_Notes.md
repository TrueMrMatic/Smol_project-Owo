# Next Step Notes — for the next patch after PATCH_003_UI_Y_HARD_FIX

You are continuing from PATCH_003_UI_Y_HARD_FIX on top of PATCH_002_UI_SNAPSHOT_SAFETY.

## Current state
- Run logging + SD run bundles are in place with low overhead.
- Bottom screen shows controls, a fixed log window, a notice line, and HUD status.
- Snapshot hotkey (Y) queues an async snapshot flush to avoid input-path stalls.
- Rectangle render mode is removed; triangles are the only path.

## Inputs you will receive
- A run bundle zip from the user (copied from SD) containing boottrace/last_stage/warnings/status snapshots.
- Or compile errors from `make`.
- Possibly a SWF that fails.

## Next theme (ONLY ONE)
Validate snapshot stability + collect run bundles from real hardware:
- Confirm Y no longer crashes and run bundles are written reliably.
- Verify the new bottom-screen UI remains stable across SWF loads.
- Capture a run bundle for at least one heavy SWF.

## Acceptance criteria
- Y snapshot never crashes and always creates a `status_snapshot` entry.
- The bottom-screen UI stays readable without scrolling/flicker during playback.
- A run bundle from hardware is available for analysis.

## Implementation checklist
- Confirm UI lines are stable and no long prints cause scroll.
- Ensure snapshot cooldown feedback is visible.
- Make sure run bundle paths still match `SD_Run_Artifacts.md`.

## Out of scope
- Do NOT add new rendering features in the same patch.
- Do NOT change runlog formats unless necessary.
