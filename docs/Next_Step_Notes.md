# Next Step Notes — for the next patch after PATCH_001_PROTOCOL_INIT_v2

You are continuing from PATCH_001_PROTOCOL_INIT_v2 on top of BASELINE_000_BEFORE_BUG.

## Current state
- Baseline renderer loads redkanga.swf without the freeze introduced in later snapshots.
- Run logging + SD run bundles are in place with low overhead.
- Bottom-screen log window exists; verbosity can be cycled with SELECT.

## Inputs you will receive
- A run bundle zip from the user (copied from SD) containing boottrace/last_stage/warnings/status snapshots.
- Or compile errors from `make`.
- Possibly a SWF that fails.

## Next theme (ONLY ONE)
Implement **Solid triangle fills for shapes** (Step 2A), but do it safely:
- Start with non-AA solid fills only.
- Keep fallback to bounds when tessellation fails.
- Add caps + logging around heavy steps (only at verbosity 2).

## Acceptance criteria
- The user can load at least one complex SWF (including redkanga) without new freezes.
- Loading screen FPS must remain close to baseline (do not add per-frame filesystem writes).
- New fill rendering produces visible filled shapes (even if imperfect) and does not regress existing strokes/bounds rendering.

## Implementation checklist
- Add a single module for fill tessellation output and triangle submission (avoid scattering logic).
- Add stage markers for heavy steps using runlog::stage().
- Keep UI log volume low by default.

## Out of scope
- Do NOT refactor caches and renderer pipeline in the same patch.
- Do NOT add bitmap rendering or sprite masks yet.
