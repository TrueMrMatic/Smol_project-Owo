# Regression Suite (Practical)

Because the test SWFs are user-provided and not curated, we use a pragmatic suite:

## Always
1. App boots and file selector works.
2. Loading screen appears and the HUD updates.
3. No freeze longer than 90 seconds without `last_stage.txt` changing.
4. `sdmc:/flash/_runs/<BUILD_ID>/<SWF_NAME>/` is created and contains the required files.

## For each SWF in user's usual order
- Run for 60 seconds (or until the first rendered frame).
- Press **Y** once during loading to create a `status_snapshot` entry.
- Confirm the `status_snapshot` line includes `shape_grouping totals ... unsupported_fills=` so unsupported fill paints are tracked.
- Optionally press **X** once to request a one-frame command dump.
- If a freeze happens, power-cycle and preserve the run bundle.

## Must-not-regress for redkanga
- It must not hard-freeze in loading.
- If rendering is incomplete, it must still progress with fallbacks instead of stalling.
