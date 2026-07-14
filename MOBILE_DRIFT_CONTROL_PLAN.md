# Mobile Drift Control Plan

Status: design only; implementation requires approval.

## Gesture

Keep Roady's position-independent touch roles:

- First eligible touch anywhere: drive owner and gas.
- Second touch held: existing brake/reverse behavior, unchanged.
- Quick release of the second touch while moving forward and steering: toggle drift on.
- Next second-touch press, drive-owner release, pause/freeze/state exit: drift off.

This avoids a fixed button and preserves the entire canvas as the touch target.

## Proposed thresholds

- Tap duration: at most 280 ms.
- Tap travel: at most 18 logical pixels.
- Steering magnitude: at least 0.35.
- Speed: strictly above the existing drift minimum.
- Exactly two eligible touches during the candidate tap; a third touch invalidates drift activation.

Canceled touches never count as taps.

## State machine

1. `Driving`: first touch owns driving.
2. `ActionHeld`: second touch immediately brakes/reverses. Track its identity, start time/position, and tap eligibility.
3. On valid quick release, with owner still live and speed/steer eligible: enter `DriftLatched`.
4. `DriftLatched`: touch contributes `Handbrake = true`; first touch continues normal drive/gas.
5. Any second-touch press immediately ends drift and becomes ordinary `ActionHeld`; braking always wins.
6. Owner release/cancel, pause, freeze, restart, Menu, or Game Over clears all drift gesture state.

Touch handbrake must OR with keyboard Shift; touch must never clear a physically held Shift key.

## HUD copy

Persistent instruction band:

- `1ST: DRAG TO DRIVE`
- `2ND: TAP DRIFT | HOLD BRAKE / REVERSE`

State feedback must use text as well as color:

- `TAP 2ND: DRIFT`
- `DRIFT ON | TAP 2ND TO END`
- `HOLDING: BRAKE / REVERSE`

Reduced Motion must retain textual drift feedback even when smoke or other effects are suppressed.

## Accidental-trigger safeguards

- Require uninterrupted drive-owner continuity.
- Require forward drift speed and meaningful steering.
- Reject long, moved, canceled, or three-finger candidates.
- A tap while already drifting is disarm-only and cannot re-arm on release.
- Preserve deterministic owner promotion, but clear drift before promotion.

## TDD acceptance

Pure and ECS tests must cover:

- Exact 280 ms and 18 px boundaries.
- Left/right steering symmetry and 0.35 threshold.
- Low-speed and straight quick taps remain ordinary brake pulses.
- Held second touch remains brake/reverse at the existing 0.15 speed boundary.
- Cancel, third touch, owner loss, pause/freeze/restart/state exit never leave drift active.
- Touch/keyboard handbrake merge is logical OR.
- Screen position and touch iteration order do not affect roles.
- Existing owner-promotion and multitouch tests remain green.
- Updated HUD remains disjoint at 844x390, 960x480, and desktop.

Browser QA should exercise valid toggle-on/toggle-off, long hold, low-speed, straight-line, moved, third-contact, canceled, owner-loss, pause/restart, and Reduced Motion cases using CDP multitouch.

## Accessibility follow-up

A later optional `Drift Assist` setting could engage at strong sustained steering for players who cannot perform timed taps. It is not part of the initial implementation.
