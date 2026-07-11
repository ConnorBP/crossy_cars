# Isometric Car Game

A minimal isometric car-driving game built with [Bevy](https://bevyengine.org) 0.19.
Runs natively **and** in the browser via WebAssembly (WebGL2).

## Controls

| Key | Action |
| --- | --- |
| `W` / `↑` | Accelerate |
| `S` / `↓` | Brake / reverse |
| `A` / `←` | Steer left |
| `D` / `→` | Steer right |

The camera is a fixed-offset orthographic (isometric) view that follows the car.

## Run natively

```sh
cargo run
```

## Run in the browser

Prerequisites (one time):

```sh
rustup target add wasm32-unknown-unknown
cargo install --locked trunk
```

Dev server with hot reload (opens `http://localhost:8080`):

```sh
trunk serve
```

Optimized web build (uses the `wasm-release` profile for a much smaller `.wasm`):

```sh
trunk build --release
# output is in dist/ — host it with any static server, e.g.:
# npx serve dist
```

## How it works

- `src/main.rs` — the whole game: orthographic isometric camera, checkerboard
  ground, a car (body + cabin + wheels) driven with arcade physics, and a
  camera-follow system.
- `index.html` — minimal Trunk page with a full-viewport canvas.
- `Cargo.toml` — Bevy with the `webgl2` feature (required for WASM) and size
  profiles for web releases.
