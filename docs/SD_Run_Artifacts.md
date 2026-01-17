# SD Run Artifacts

Every run creates a folder:

`sdmc:/flash/_runs/<BUILD_ID>/<SWF_NAME>/`

Files written:

- `build_info.txt`  
  Build id, base id, timestamp, and SWF path.
- `boottrace.txt`  
  High-level boot and heartbeat logs (always flushed).
- `last_stage.txt`  
  Frequently-updated single-line marker for “where we are” (useful on freezes).
- `status_snapshot.txt`  
  Appended multi-line diagnostic snapshots when the user presses **Y** (last stage, cache stats, draw stats, recent warnings).
- `warnings.txt`  
  Renderer warnings, caps hit, recoveries (may be empty in early iterations).

Optional (future iterations):
- `metrics.csv` (fps/frame time, cache sizes)
- `top.bmp`, `bottom.bmp` on screenshot hotkey

When a freeze happens, zip the entire `<BUILD_ID>` folder and share it back to the AI.

Notes:
- Run bundles reinitialize per SWF selection (new folder per SWF).
- Use **X** to request a one-frame command dump for deeper command-list correlation.
- Boottrace may include `shape_cache_evict` lines when the shape cache exceeds its budget.
