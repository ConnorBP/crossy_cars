#!/usr/bin/env python3
"""Build Roady's deterministic seamless PBR microdetail maps.

Color multipliers and the regenerated grass/soil data maps are source-free,
fixed-seed, toroidal value noise.  The audited plastic, concrete and wood data
maps are intentionally left byte-for-byte unchanged by this correction.
"""
from __future__ import annotations

import hashlib
import json
import math
import random
from pathlib import Path

from PIL import Image

SIZE = 256
OUT = Path(__file__).resolve().parents[1] / "assets" / "textures" / "pbr_detail"
MANIFEST = OUT / "manifest.json"
CONSTANTS = Path(__file__).resolve().parents[1] / "src" / "pbr_detail_constants.rs"
CYCLES = (24, 48)
OCTAVE_WEIGHTS = (0.7, 0.3)
REPEAT = 2
NORMAL_LENGTH_TOLERANCE = 0.01
NORMAL_AMPLITUDE_CEILING = 0.035

SEEDS = {
    "concrete_albedo": 0xC011C0DE,
    "foliage_albedo": 0xF011A6E,
    "traffic_paint_albedo": 0x7AFF1C,
    "traffic_ao": 0x7A0A0,
    "traffic_roughness": 0x7A0B0,
    "grass_height": 0x6A455,
    "grass_ao": 0x6A456,
    "grass_roughness": 0x6A457,
    "soil_ao": 0x5011A0,
    "soil_roughness": 0x5011B0,
}

ALBEDO_RANGES = {
    "concrete_albedo": (228, 255),
    "foliage_albedo": (236, 255),
    "traffic_paint_albedo": (232, 255),
}


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _smooth(value: float) -> float:
    return value * value * (3.0 - 2.0 * value)


def _periodic_value_octave(cycles: int, seed: int) -> list[float]:
    """Periodic value noise, D4-symmetrized to remove directional bias."""
    rng = random.Random(seed)
    lattice = [[rng.random() for _ in range(cycles)] for _ in range(cycles)]

    def sample(px: float, py: float) -> float:
        x0 = math.floor(px)
        y0 = math.floor(py)
        tx = _smooth(px - x0)
        ty = _smooth(py - y0)
        x0 %= cycles
        y0 %= cycles
        x1 = (x0 + 1) % cycles
        y1 = (y0 + 1) % cycles
        a = lattice[y0][x0] * (1.0 - tx) + lattice[y0][x1] * tx
        b = lattice[y1][x0] * (1.0 - tx) + lattice[y1][x1] * tx
        return a * (1.0 - ty) + b * ty

    values: list[float] = []
    period = float(SIZE - 1)
    for y in range(SIZE):
        py = y * cycles / period
        for x in range(SIZE):
            px = x * cycles / period
            # Averaging the four quarter-turn/reflection equivalents gives
            # each fixed realization equal X/Y and diagonal statistics.
            values.append(
                (sample(px, py) + sample(py, px) + sample(-px, py) + sample(py, -px))
                * 0.25
            )
    return values


def toroidal_isotropic_noise(seed: int) -> list[float]:
    octaves = [
        _periodic_value_octave(cycles, seed + index * 0x9E3779B1)
        for index, cycles in enumerate(CYCLES)
    ]
    mixed = [
        OCTAVE_WEIGHTS[0] * first + OCTAVE_WEIGHTS[1] * second
        for first, second in zip(*octaves, strict=True)
    ]
    lo, hi = min(mixed), max(mixed)
    span = hi - lo
    assert span > 0.0
    values = [(value - lo) / span for value in mixed]
    # Integer endpoint coordinates already agree; assign them explicitly so
    # PNG bytes at opposite edges and all four corners are exact.
    for y in range(SIZE):
        values[y * SIZE + SIZE - 1] = values[y * SIZE]
    for x in range(SIZE):
        values[(SIZE - 1) * SIZE + x] = values[x]
    return values


def quantize(values: list[float], floor: int, ceiling: int = 255) -> list[int]:
    result = [round(floor + value * (ceiling - floor)) for value in values]
    assert min(result) == floor and max(result) == ceiling
    return result


def albedo_map(seed: int, floor: int) -> Image.Image:
    values = quantize(toroidal_isotropic_noise(seed), floor)
    image = Image.new("RGBA", (SIZE, SIZE))
    image.putdata([(value, value, value, 255) for value in values])
    return image


def orm_map(ao_seed: int, roughness_seed: int, ao_floor: int, roughness_floor: int) -> Image.Image:
    ao = quantize(toroidal_isotropic_noise(ao_seed), ao_floor)
    roughness = quantize(toroidal_isotropic_noise(roughness_seed), roughness_floor)
    image = Image.new("RGBA", (SIZE, SIZE))
    image.putdata([(a, r, 255, 255) for a, r in zip(ao, roughness, strict=True)])
    return image


def encode_normal(nx: float, ny: float) -> tuple[int, int, int, int]:
    inverse_length = 1.0 / math.sqrt(nx * nx + ny * ny + 1.0)
    return (
        round((nx * inverse_length * 0.5 + 0.5) * 255.0),
        round((ny * inverse_length * 0.5 + 0.5) * 255.0),
        round((inverse_length * 0.5 + 0.5) * 255.0),
        255,
    )


def normal_map(seed: int, max_slope: float) -> Image.Image:
    height = toroidal_isotropic_noise(seed)
    gradients: list[tuple[float, float]] = []
    maximum = 0.0
    period = SIZE - 1
    for y in range(SIZE):
        ym = (y - 1) % period
        yp = (y + 1) % period
        for x in range(SIZE):
            xm = (x - 1) % period
            xp = (x + 1) % period
            dx = (height[y * SIZE + xp] - height[y * SIZE + xm]) * 0.5
            dy = (height[yp * SIZE + x] - height[ym * SIZE + x]) * 0.5
            gradients.append((dx, dy))
            maximum = max(maximum, math.hypot(dx, dy))
    scale = max_slope / maximum
    slopes = [(-dx * scale, -dy * scale) for dx, dy in gradients]
    for y in range(SIZE):
        slopes[y * SIZE + SIZE - 1] = slopes[y * SIZE]
    for x in range(SIZE):
        slopes[(SIZE - 1) * SIZE + x] = slopes[x]
    image = Image.new("RGBA", (SIZE, SIZE))
    image.putdata([encode_normal(nx, ny) for nx, ny in slopes])
    return image


def srgb_to_linear(value: int) -> float:
    encoded = value / 255.0
    return encoded / 12.92 if encoded <= 0.04045 else ((encoded + 0.055) / 1.055) ** 2.4


def decoded_linear_mean(image: Image.Image) -> float:
    pixels = image.convert("RGBA").getdata()
    return sum(srgb_to_linear(pixel[0]) for pixel in pixels) / (SIZE * SIZE)


def linear_channel_mean(image: Image.Image, channel: int) -> float:
    return sum(pixel[channel] / 255.0 for pixel in image.convert("RGBA").getdata()) / (SIZE * SIZE)


def assert_exact_edges(image: Image.Image, label: str) -> None:
    pixels = list(image.convert("RGBA").getdata())
    for y in range(SIZE):
        assert pixels[y * SIZE] == pixels[y * SIZE + SIZE - 1], f"{label}: horizontal seam {y}"
    for x in range(SIZE):
        assert pixels[x] == pixels[(SIZE - 1) * SIZE + x], f"{label}: vertical seam {x}"


def assert_exact_channels(image: Image.Image, bounds: tuple[tuple[int, int], ...], label: str) -> None:
    channels = tuple(zip(*image.convert("RGBA").getdata()))
    actual = tuple((min(channel), max(channel)) for channel in channels)
    assert actual == bounds, f"{label}: {actual}, expected {bounds}"


def normal_metrics(image: Image.Image) -> dict[str, float]:
    vectors = [tuple(channel / 127.5 - 1.0 for channel in pixel[:3]) for pixel in image.convert("RGBA").getdata()]
    lengths = [math.sqrt(x * x + y * y + z * z) for x, y, z in vectors]
    amplitudes = [math.hypot(x, y) for x, y, _ in vectors]
    return {
        "length_error_max": max(abs(length - 1.0) for length in lengths),
        "amplitude_mean": sum(amplitudes) / len(amplitudes),
        "amplitude_max": max(amplitudes),
        "seam_max": 0.0,
    }


def save(image: Image.Image, name: str) -> dict[str, object]:
    path = OUT / name
    image.save(path, format="PNG", optimize=True, compress_level=9)
    data = path.read_bytes()
    return {
        "path": path.relative_to(OUT.parents[2]).as_posix(),
        "bytes": len(data),
        "sha256": sha256(data),
    }


def existing(name: str) -> dict[str, object]:
    path = OUT / name
    data = path.read_bytes()
    return {
        "path": path.relative_to(OUT.parents[2]).as_posix(),
        "bytes": len(data),
        "sha256": sha256(data),
        "generation": "preserved unchanged by final correction",
    }


def write_constants(means: dict[str, float]) -> None:
    text = """// @generated by tools/build_pbr_details.py; do not edit.\n\
// Decoded means are derived from the checked-in PNG bytes.\n\n\
pub(crate) const CONCRETE_ALBEDO_LINEAR_MEAN: f32 = {concrete:.12f};\n\
pub(crate) const FOLIAGE_ALBEDO_LINEAR_MEAN: f32 = {foliage:.12f};\n\
pub(crate) const TRAFFIC_PAINT_ALBEDO_LINEAR_MEAN: f32 = {traffic:.12f};\n\
pub(crate) const TRAFFIC_PAINT_ROUGHNESS_LINEAR_MEAN: f32 = {roughness:.12f};\n\
""".format(**means)
    CONSTANTS.write_text(text, encoding="utf-8", newline="\n")


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    outputs: dict[str, dict[str, object]] = {}
    means: dict[str, float] = {}

    for key, (floor, ceiling) in ALBEDO_RANGES.items():
        image = albedo_map(SEEDS[key], floor)
        assert_exact_edges(image, key)
        assert_exact_channels(image, ((floor, ceiling),) * 3 + ((255, 255),), key)
        mean = decoded_linear_mean(image)
        constant_key = {"concrete_albedo": "concrete", "foliage_albedo": "foliage", "traffic_paint_albedo": "traffic"}[key]
        means[constant_key] = mean
        outputs[key] = {
            **save(image, f"{key}.png"),
            "decoded_linear_mean": round(mean, 12),
            "compensation_reciprocal": round(1.0 / mean, 12),
        }

    traffic_orm = orm_map(SEEDS["traffic_ao"], SEEDS["traffic_roughness"], 250, 220)
    assert_exact_edges(traffic_orm, "traffic_paint_orm")
    assert_exact_channels(traffic_orm, ((250, 255), (220, 255), (255, 255), (255, 255)), "traffic_paint_orm")
    roughness_mean = linear_channel_mean(traffic_orm, 1)
    means["roughness"] = roughness_mean
    outputs["traffic_paint_orm"] = {
        **save(traffic_orm, "traffic_paint_orm.png"),
        "roughness_linear_mean": round(roughness_mean, 12),
        "roughness_compensation_reciprocal": round(1.0 / roughness_mean, 12),
    }

    grass_normal = normal_map(SEEDS["grass_height"], 0.024)
    grass_metrics = normal_metrics(grass_normal)
    assert_exact_edges(grass_normal, "grass_normal")
    assert grass_metrics["length_error_max"] <= NORMAL_LENGTH_TOLERANCE
    assert grass_metrics["amplitude_max"] <= NORMAL_AMPLITUDE_CEILING
    outputs["grass_normal"] = {**save(grass_normal, "grass_normal.png"), "normal_validation": {key: round(value, 6) for key, value in grass_metrics.items()}}

    grass_orm = orm_map(SEEDS["grass_ao"], SEEDS["grass_roughness"], 248, 238)
    soil_orm = orm_map(SEEDS["soil_ao"], SEEDS["soil_roughness"], 246, 232)
    for key, image, bounds in [
        ("grass_orm", grass_orm, ((248, 255), (238, 255), (255, 255), (255, 255))),
        ("soil_orm", soil_orm, ((246, 255), (232, 255), (255, 255), (255, 255))),
    ]:
        assert_exact_edges(image, key)
        assert_exact_channels(image, bounds, key)
        outputs[key] = save(image, f"{key}.png")

    for key in ("plastic_normal", "plastic_orm", "concrete_normal", "concrete_orm", "wood_normal", "wood_orm"):
        outputs[key] = existing(f"{key}.png")

    write_constants(means)
    manifest = {
        "schema": "roady.pbr-detail.v5",
        "license": "Source-free generated noise; preserved legacy derivatives retain prior user-confirmed free-use provenance",
        "resolution": [SIZE, SIZE],
        "color_spaces": {"albedo": "sRGB", "normal_and_orm": "linear"},
        "generator": Path(__file__).name,
        "generation": {
            "algorithm": "fixed-seed toroidal isotropic D4-symmetrized value noise",
            "cycles": list(CYCLES),
            "weights": list(OCTAVE_WEIGHTS),
            "runtime_repeat": REPEAT,
            "seam": "opposite RGBA edges and corners are byte-exact",
            "seeds": SEEDS,
        },
        "orm_channels": {"r": "ambient occlusion multiplier", "g": "roughness multiplier", "b": "metallic multiplier (255)", "a": 255},
        "families": {
            "concrete_lightstone": {"albedo_sRGB": [228, 255], "repeat": 2, "facade_maps": ["albedo", "orm"], "facade_normal": None},
            "foliage": {"albedo_sRGB": [236, 255], "repeat": 2},
            "traffic_paint": {"albedo_sRGB": [232, 255], "repeat": 2, "orm_ranges": {"ao": [250, 255], "roughness": [220, 255], "metallic": [255, 255], "alpha": [255, 255]}, "normal": "plastic_normal.png, requested/audited strength 0.035 (8-bit decoded amplitude <= 0.0356)"},
            "grass": {"normal_amplitude_max": round(grass_metrics["amplitude_max"], 6), "normal_ceiling": 0.035, "orm_ranges": {"ao": [248, 255], "roughness": [238, 255]}},
            "soil": {"orm_ranges": {"ao": [246, 255], "roughness": [232, 255]}},
            "wood_and_roofs": "unchanged",
        },
        "outputs": outputs,
    }
    MANIFEST.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="\n")
    print(json.dumps(manifest, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
