# Roady Car startup performance audit

Measured 2026-07-16 against production `00485d0` with cache-disabled headless Chromium. The measurements explain the previously exposed but temporarily unresponsive Menu.

## Cold-load profile

| Phase | Observed |
|---|---:|
| HTML shell | 73-192 ms normally |
| Compressed WASM transfer | 7.29 MiB, 0.62-0.98 s |
| Decoded WASM | 23.52 MiB |
| HTML complete to canvas | 0.93-1.26 s |
| Prominent assets complete | 5.21-5.86 s |
| Specular IBL | 6.79 MiB, about 3.48-4.02 s |
| Total network | about 16.38 MiB / 39 requests |

WASM and the specular IBL represent about 86% of cold transfer. V8 WASM compilation was only about 77 ms in the sampled trace; renderer, world preparation, GPU upload, and commit tasks were substantially larger. Production also constructs the complete populated 5x5 world synchronously before presenting the Menu.

## Root defect

The HTML shell previously hid as soon as a canvas existed and two browser animation frames elapsed. Canvas creation did not prove that Startup systems, the initial grid, the responsive Menu, or the imported player car were ready. This exposed controls while the main/render threads were still preparing the scene.

## Implemented readiness contract

The shell now remains visible until Bevy publishes `data-roady-ready="playable"`. Production publishes that marker only after two consecutive complete updates where:

- the current state is Menu;
- the primary window exists;
- a 3D gameplay camera exists;
- the expected `GridConfig.count^2` block roots exist;
- the responsive Menu composition exists; and
- the imported player car has completed its named-node bindings.

JavaScript then waits two animation frames before fading the splash. `TrunkApplicationStarted` changes the truthful status from "Downloading the game..." to "Preparing the toy town..." but cannot dismiss it. A 60-second semantic-readiness watchdog provides a failure fallback; canvas existence cannot suppress that error.

## Deferred optimization wave

The next dedicated startup optimization should remain image- and measurement-gated:

1. Generate a smaller specular IBL candidate and compare lighting before replacing the current map.
2. Replace broad Bevy defaults with an audited explicit feature set to shrink WASM.
3. Add fingerprinted immutable caching for generated WASM/JS and versioned copied assets.
4. Evaluate central-first staged world construction without changing topology or gameplay readiness.
5. Defer traffic/audio requests only if a new waterfall shows they remain material after the dominant fixes.

Traffic GLBs and audio together are only about 1.1 MiB and were intentionally not deferred in the current wave; their lifecycle/order risk outweighs that limited gain. The IBL was also not deferred without a controlled visual fallback because a late lighting transition would undermine the reviewed toy-town presentation.
