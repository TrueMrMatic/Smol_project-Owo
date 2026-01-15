# Workflow Protocol

This project is optimized for **tight hardware feedback loops** on a constrained platform. To keep iterations reliable and avoid performance regressions, follow this protocol exactly.

## Patch discipline
- **One theme per patch.** (No mixed refactors + new features.)
- **Avoid unbounded work** in the render path.
- **All heavy steps must be capped** or degrade gracefully.
- **Run bundles are mandatory** for every run.

## Before coding (per patch)
1. State the **single theme** of the patch.
2. Declare what is **explicitly out of scope**.
3. Define the **acceptance criteria**.

## Implementation checklist
- [ ] Stage markers added for any heavy step.
- [ ] A fallback path exists (bounds-only or no-op rendering).
- [ ] Any new allocations are bounded or reused.
- [ ] No per-frame filesystem writes (unless explicitly required).

## Hardware test protocol
1. Copy the `.3dsx` to the SD card.
2. Launch and select the SWF.
3. During loading, press **Y** once to write a status snapshot.
4. Let it run for at least 60 seconds or until the first frame.
5. If it freezes, power-cycle, then preserve the run bundle.

## Reporting back
Provide:
- The full **run bundle** zip (`sdmc:/flash/_runs/<BUILD_ID>/<SWF_NAME>/`).
- A short description of the **visible behavior** (freeze, blank, partial render).
- The **SWF name + size**.

## Recommended feedback format
```
SWF:
Behavior:
Did it freeze? (Y/N)
Time to first frame:
Notes:
```

