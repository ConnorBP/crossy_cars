# Credits

## Engine

Roady Car is built with [Bevy 0.19](https://bevyengine.org/). Bevy is dual-licensed under the [MIT License](https://github.com/bevyengine/bevy/blob/v0.19.0/LICENSE-MIT) or the [Apache License 2.0](https://github.com/bevyengine/bevy/blob/v0.19.0/LICENSE-APACHE).

## Environment maps

The following files were sourced unmodified from Bevy v0.19's `assets/environment_maps` directory:

- `assets/environment_maps/pisa_diffuse_rgb9e5_zstd.ktx2`
- `assets/environment_maps/pisa_specular_rgb9e5_zstd.ktx2`

The Pisa environment was originally published by HDRI Haven, now [Poly Haven](https://polyhaven.com/), and is available under [CC0](https://creativecommons.org/publicdomain/zero/1.0/).

## Audio

The procedural WAV files are original generated assets, produced with Python's standard-library `wave`, `math`, and `struct` functionality:

- `assets/audio/ambient.wav`
- `assets/audio/click.wav`
- `assets/audio/coin.wav`
- `assets/audio/crash.wav`
- `assets/audio/engine.wav`
- `assets/audio/hit.wav`

They do not contain third-party recordings or samples.

## Models and textures

The car, traffic, characters, animals, buildings, props, particles, and other meshes are assembled procedurally from Bevy primitives or generated geometry. Surface textures and normal maps are generated in code. No third-party model or surface-texture assets are used; the credited Pisa environment maps above are the only externally sourced image assets.
