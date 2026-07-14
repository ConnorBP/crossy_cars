# Camera and Touch Smoothing Implementation Report

## Scope

This change smooths Roady's dynamic fixed-isometric gameplay camera and mobile drag steering. Camera rotation remains assigned only at camera spawn; follow, lead, collision shake, and touch input never rotate or tilt it.

## Camera

- Reduced full-speed directional lead from `0.80` to `0.42` NDC.
- Separated travel-direction damping (`2/s`) from lead damping (`3/s`). Both use exponential, delta-time-based smoothing.
- Replaced absolute-position anchor lerp with a capped anchor step (`18 world units/s`). Normal max-speed travel can be followed directly without building heavy lag, while exceptional collision pushout cannot move the anchor arbitrarily in one frame.
- Stores lead in screen/NDC space before converting it through the fixed ground projection.
- Caps lead slew independently by screen axis (`0.9 NDC/s` horizontal, `0.28 NDC/s` vertical). This specifically limits the apparent screen-vertical camera movement caused by lateral world lead during sharp turns.
- Direction smoothing follows the remaining shortest angle exponentially. It cannot overshoot and has deterministic handling for an exact 180-degree reversal.
- Teleport reset, fresh-round reset, pause/resume preservation, reduced-motion behavior, speed zoom, and collision shake remain intact.

Focused unit coverage in `src/camera.rs` includes:

- 90-degree-turn anchor and vertical-lead bounds.
- 30/60/120 fps direction equivalence.
- Monotonic lead release without overshoot/jitter.
- Collision pushout anchor cap.
- Fresh-round reset versus pause/resume preservation.
- Fixed lead projection and reduced lead extent.

## Mobile Touch

- Added a stateful virtual analog drag for the sticky direction owner.
- A drag must exceed 8px to engage and returns to zero at 6px, providing center deadzone hysteresis.
- Drag vectors are low-pass filtered with exponential delta-time damping (`12/s`).
- A bounded floating origin follows drags beyond 96px, retaining analog direction without requiring unlimited thumb travel.
- Owner acquisition initializes at the current finger location. Promotion or reacquisition therefore cannot inherit an old vector or kick steering.
- Owner eligibility is decided on acquisition and remains sticky until release. A viewport/orientation size change recenters the filter at the live finger without changing ownership, preventing a coordinate-rescale steering kick.
- The second touch's independent brake-then-reverse action semantics are unchanged.
- Touch world direction continues to use only the camera rotation's fixed projected ground basis. Camera follow translation and shake cannot affect it.
- Playing-state exit resets both owner and analog filter state; pause/resume acquires a clean owner after normal touch release.

Focused unit coverage in `src/touch.rs` includes:

- Exact 6px zero steering and 8px engagement hysteresis.
- Analog unequal diagonals.
- 30/60/120 fps filter equivalence.
- Monotonic filtering, no overshoot, and clean release reset.
- Bounded floating-origin recentering.
- Camera-translation-independent ground basis.
- Sticky ownership and clean filter recentering under orientation changes.
- Independent action brake/reverse behavior and owner promotion.

## Browser Scenarios and Documentation

- `tools/browser_scenarios.py` now captures a sharp-turn transition and settled frame for camera-motion comparison.
- `tools/browser_touch_scenarios.py` now changes to portrait, verifies exact canvas fit, pauses/resumes there, returns to landscape, and continues through the existing pause/resume/restart behavior.
- `README.md` documents deadzone hysteresis, bounded floating origin, and camera/touch QA checks.

## Validation Status

Per task constraints, no shell commands, builds, tests, package tools, browser automation, or Git commands were executed. The added tests and browser scenario changes are authored but intentionally not run in this worktree.
