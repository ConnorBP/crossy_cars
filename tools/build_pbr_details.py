#!/usr/bin/env python3
"""Build Roady's tiny toy-town PBR detail library from free-use source maps.

Reads selected entries directly from the user's source ZIPs, builds restrained
normal relief, packs AO/roughness/metallic channels, and writes only 256px
derivatives. The original 4K maps are never copied into the repository.
"""
from __future__ import annotations

import hashlib
import json
import math
from dataclasses import dataclass
from io import BytesIO
from pathlib import Path
from zipfile import ZipFile

from PIL import Image, ImageFile, ImageFilter, ImageOps

SIZE = 256
ROOT = Path(r"E:/DEVELOPER/PBR_MATERIALS")
OUT = Path(__file__).resolve().parents[1] / "assets" / "textures" / "pbr_detail"
MANIFEST = OUT / "manifest.json"
PLASTIC_NORMAL_BLUR_RADIUS = 1.5
PLASTIC_NORMAL_HEIGHT_STRENGTH = 4.0
NORMAL_LENGTH_TOLERANCE = 0.01

# Tangent-plane amplitude (sqrt(x*x + y*y)) ranges are deliberately advisory:
# they catch accidentally flat or harsh maps while allowing source-map variation.
NORMAL_AMPLITUDE_ADVISORS = {
    "plastic": {"mean": (0.035, 0.055), "max": (0.22, 0.32), "seam_max": 0.0},
    "concrete": {"mean": (0.015, 0.025), "max": (0.14, 0.17), "seam_max": 0.07},
    "wood": {"mean": (0.038, 0.050), "max": (0.24, 0.27), "seam_max": 0.23},
    "grass": {"mean": (0.032, 0.045), "max": (0.14, 0.17), "seam_max": 0.20},
}


@dataclass(frozen=True)
class Family:
    archive: str
    normal: str
    roughness: str
    ao: str
    normal_strength: float
    roughness_floor: int
    ao_floor: int


FAMILIES = {
    "plastic": Family(
        "scuffed-plastic-1-bl.zip",
        "scuffed-plastic-1-bl/scuffed-plastic-normal.png",
        "scuffed-plastic-1-bl/scuffed-plastic-rough.png",
        "scuffed-plastic-1-bl/scuffed-plastic-ao.png",
        0.0,  # The plastic normal is derived from AO; see plastic_normal().
        185,
        205,
    ),
    "concrete": Family(
        "concrete_wall_01_4k.zip",
        "concrete_wall_01_4k/concrete_wall_01_normal_gl_4k.png",
        "concrete_wall_01_4k/concrete_wall_01_roughness_4k.png",
        "concrete_wall_01_4k/concrete_wall_01_ambient_occlusion_4k.png",
        0.75,
        185,
        195,
    ),
    "wood": Family(
        "wood_01_4k.zip",
        "wood_01_4k/wood_01_normal_gl_4k.png",
        "wood_01_4k/wood_01_roughness_4k.png",
        "wood_01_4k/wood_01_ambient_occlusion_4k.png",
        0.45,
        180,
        195,
    ),
    "grass": Family(
        "whispy-grass-meadow-bl.zip",
        "whispy-grass-meadow-bl/wispy-grass-meadow_normal-ogl.png",
        "whispy-grass-meadow-bl/wispy-grass-meadow_roughness.png",
        "whispy-grass-meadow-bl/wispy-grass-meadow_ao.png",
        0.45,
        200,
        205,
    ),
}


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def read_entry(archive: Path, entry: str) -> bytes:
    with ZipFile(archive) as bundle:
        return bundle.read(entry)


def decode(data: bytes, *, recover_truncated: bool = False) -> Image.Image:
    previous = ImageFile.LOAD_TRUNCATED_IMAGES
    ImageFile.LOAD_TRUNCATED_IMAGES = recover_truncated
    try:
        image = Image.open(BytesIO(data))
        image.load()
        return image
    finally:
        ImageFile.LOAD_TRUNCATED_IMAGES = previous


def downscale(image: Image.Image, mode: str) -> Image.Image:
    return image.convert(mode).resize((SIZE, SIZE), Image.Resampling.LANCZOS)


def attenuated_normal(image: Image.Image, strength: float) -> Image.Image:
    source = downscale(image, "RGB")
    output = Image.new("RGBA", source.size)
    pixels = []
    for red, green, _blue in source.getdata():
        x = (red / 127.5 - 1.0) * strength
        y = (green / 127.5 - 1.0) * strength
        z = math.sqrt(max(0.0, 1.0 - x * x - y * y))
        pixels.append(
            (
                round((x * 0.5 + 0.5) * 255.0),
                round((y * 0.5 + 0.5) * 255.0),
                round((z * 0.5 + 0.5) * 255.0),
                255,
            )
        )
    output.putdata(pixels)
    return output


def normalized_luminance(image: Image.Image, size: tuple[int, int] = (SIZE, SIZE)) -> list[float]:
    # Several 4K sources are 16-bit grayscale. Converting those directly to
    # `L` clips values above 255 and produces an almost uniform white map.
    # Resize in float space, then normalize the resulting sampled range.
    sampled = image.convert("F").resize(size, Image.Resampling.LANCZOS)
    values = list(sampled.getdata())
    lo, hi = min(values), max(values)
    span = max(1.0, hi - lo)
    return [(value - lo) / span for value in values]


def packed_orm(ao: Image.Image, roughness: Image.Image, ao_floor: int, rough_floor: int) -> Image.Image:
    ao_values = normalized_luminance(ao)
    rough_values = normalized_luminance(roughness)
    output = Image.new("RGBA", (SIZE, SIZE))
    output.putdata(
        [
            (
                round(ao_floor + value_ao * (255 - ao_floor)),
                round(rough_floor + value_rough * (255 - rough_floor)),
                255,
                255,
            )
            for value_ao, value_rough in zip(ao_values, rough_values, strict=True)
        ]
    )
    return output


def mirrored_tile(image: Image.Image) -> Image.Image:
    """Make a seamless 256px tile from a normalized 128px source sample."""
    sample_size = (SIZE // 2, SIZE // 2)
    sample = Image.new("L", sample_size)
    sample.putdata([round(value * 255.0) for value in normalized_luminance(image, sample_size)])
    tile = Image.new("L", (SIZE, SIZE))
    tile.paste(sample, (0, 0))
    tile.paste(ImageOps.mirror(sample), (SIZE // 2, 0))
    tile.paste(ImageOps.flip(sample), (0, SIZE // 2))
    tile.paste(ImageOps.flip(ImageOps.mirror(sample)), (SIZE // 2, SIZE // 2))
    return tile


def plastic_normal(ao: Image.Image) -> Image.Image:
    """Derive a subtle, seamless tangent-space normal from scuffed-plastic AO."""
    height = mirrored_tile(ao).filter(ImageFilter.GaussianBlur(PLASTIC_NORMAL_BLUR_RADIUS))
    values = list(height.getdata())
    vectors: list[tuple[float, float, float]] = []
    for y in range(SIZE):
        for x in range(SIZE):
            # Central finite differences wrap around the tile. Mirrored endpoint
            # gradients are pinned to zero so opposite output edges match exactly.
            dx = (values[y * SIZE + (x + 1) % SIZE] - values[y * SIZE + (x - 1) % SIZE]) / 510.0
            dy = (values[((y + 1) % SIZE) * SIZE + x] - values[((y - 1) % SIZE) * SIZE + x]) / 510.0
            if x == 0 or x == SIZE - 1:
                dx = 0.0
            if y == 0 or y == SIZE - 1:
                dy = 0.0
            nx = -dx * PLASTIC_NORMAL_HEIGHT_STRENGTH
            ny = -dy * PLASTIC_NORMAL_HEIGHT_STRENGTH
            inverse_length = 1.0 / math.sqrt(nx * nx + ny * ny + 1.0)
            vectors.append((nx * inverse_length, ny * inverse_length, inverse_length))

    output = Image.new("RGBA", (SIZE, SIZE))
    output.putdata(
        [
            (
                round((x * 0.5 + 0.5) * 255.0),
                round((y * 0.5 + 0.5) * 255.0),
                round((z * 0.5 + 0.5) * 255.0),
                255,
            )
            for x, y, z in vectors
        ]
    )
    return output


def soil_orm(smudge: Image.Image) -> Image.Image:
    values = normalized_luminance(mirrored_tile(smudge))
    output = Image.new("RGBA", (SIZE, SIZE))
    output.putdata(
        [
            (
                round(205 + value * 50),
                round(185 + value * 70),
                255,
                255,
            )
            for value in values
        ]
    )
    return output


def assert_exact_channels(image: Image.Image, bounds: tuple[tuple[int, int], ...], label: str) -> None:
    channels = tuple(zip(*image.convert("RGBA").getdata()))
    actual = tuple((min(channel), max(channel)) for channel in channels)
    assert actual == bounds, f"{label} channel bounds {actual}, expected {bounds}"


def normal_metrics(image: Image.Image) -> dict[str, float]:
    pixels = list(image.convert("RGBA").getdata())
    vectors = [tuple(channel / 127.5 - 1.0 for channel in pixel[:3]) for pixel in pixels]
    lengths = [math.sqrt(x * x + y * y + z * z) for x, y, z in vectors]
    amplitudes = [math.hypot(x, y) for x, y, _z in vectors]
    seam_deltas = []
    for y in range(SIZE):
        left, right = vectors[y * SIZE], vectors[y * SIZE + SIZE - 1]
        seam_deltas.append(math.dist(left, right))
    for x in range(SIZE):
        top, bottom = vectors[x], vectors[(SIZE - 1) * SIZE + x]
        seam_deltas.append(math.dist(top, bottom))
    return {
        "length_error_max": max(abs(length - 1.0) for length in lengths),
        "amplitude_mean": sum(amplitudes) / len(amplitudes),
        "amplitude_max": max(amplitudes),
        "seam_max": max(seam_deltas),
    }


def assert_normal(image: Image.Image, label: str) -> dict[str, object]:
    metrics = normal_metrics(image)
    advisor = NORMAL_AMPLITUDE_ADVISORS[label]
    assert all(math.isfinite(value) for value in metrics.values()), f"{label} normal has non-finite metrics"
    assert metrics["length_error_max"] <= NORMAL_LENGTH_TOLERANCE, f"{label} normal is not normalized: {metrics}"
    for metric in ("mean", "max"):
        value = metrics[f"amplitude_{metric}"]
        low, high = advisor[metric]
        assert low <= value <= high, f"{label} normal amplitude {metric} {value} outside [{low}, {high}]"
    assert metrics["seam_max"] <= advisor["seam_max"], f"{label} normal seam exceeds advisor: {metrics}"
    return {
        **{key: round(value, 6) for key, value in metrics.items()},
        "advisors": {
            "amplitude_mean": list(advisor["mean"]),
            "amplitude_max": list(advisor["max"]),
            "length_error_max": NORMAL_LENGTH_TOLERANCE,
            "seam_max": advisor["seam_max"],
        },
    }


def save(image: Image.Image, name: str) -> dict[str, object]:
    path = OUT / name
    image.save(path, format="PNG", optimize=True, compress_level=9)
    data = path.read_bytes()
    return {"path": path.relative_to(OUT.parents[2]).as_posix(), "bytes": len(data), "sha256": sha256(data)}


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    outputs: dict[str, dict[str, object]] = {}
    sources: dict[str, dict[str, object]] = {}

    for name, family in FAMILIES.items():
        archive = ROOT / family.archive
        archive_bytes = archive.read_bytes()
        normal_bytes = read_entry(archive, family.normal)
        roughness_bytes = read_entry(archive, family.roughness)
        ao_bytes = read_entry(archive, family.ao)
        ao = decode(ao_bytes)
        normal = (
            plastic_normal(ao)
            if name == "plastic"
            else attenuated_normal(decode(normal_bytes), family.normal_strength)
        )
        orm = packed_orm(ao, decode(roughness_bytes), family.ao_floor, family.roughness_floor)
        normal_metrics_checked = assert_normal(normal, name)
        assert_exact_channels(
            orm,
            ((family.ao_floor, 255), (family.roughness_floor, 255), (255, 255), (255, 255)),
            f"{name} ORM",
        )
        outputs[f"{name}_normal"] = save(normal, f"{name}_normal.png")
        outputs[f"{name}_orm"] = save(orm, f"{name}_orm.png")
        sources[name] = {
            "archive": family.archive,
            "archive_sha256": sha256(archive_bytes),
            "entries": {
                "normal": {"path": family.normal, "sha256": sha256(normal_bytes)},
                "roughness": {"path": family.roughness, "sha256": sha256(roughness_bytes)},
                "ao": {"path": family.ao, "sha256": sha256(ao_bytes)},
            },
            "normal_generation": (
                {
                    "method": "central finite differences of mirrored, normalized AO height",
                    "gaussian_blur_radius": PLASTIC_NORMAL_BLUR_RADIUS,
                    "height_strength": PLASTIC_NORMAL_HEIGHT_STRENGTH,
                }
                if name == "plastic"
                else {"method": "attenuated source normal", "strength": family.normal_strength}
            ),
            "normal_validation": normal_metrics_checked,
            "roughness_floor": family.roughness_floor,
            "ao_floor": family.ao_floor,
        }
        # Keep the v1 field for consumers of existing manifests. Plastic has no
        # source-normal attenuation strength because its normal is AO-derived.
        if name != "plastic":
            sources[name]["normal_strength"] = family.normal_strength

    smudge_path = ROOT / "Smudges01_4K.png"
    smudge_bytes = smudge_path.read_bytes()
    smudge = decode(smudge_bytes, recover_truncated=True)
    soil = soil_orm(smudge)
    assert_exact_channels(soil, ((205, 255), (185, 255), (255, 255), (255, 255)), "soil ORM")
    outputs["soil_orm"] = save(soil, "soil_orm.png")
    sources["soil"] = {
        "source": smudge_path.name,
        "source_sha256": sha256(smudge_bytes),
        "recovery": "Pillow LOAD_TRUNCATED_IMAGES=true; complete 4096x4096 RGB decoded",
        "tiling": "128px Lanczos sample mirrored in both axes into a seamless 256px tile",
        "roughness_floor": 185,
        "ao_floor": 205,
    }

    manifest = {
        "schema": "roady.pbr-detail.v1",
        "license": "User-confirmed free to use",
        "resolution": [SIZE, SIZE],
        "color_space": "linear",
        "orm_channels": {"r": "ambient occlusion", "g": "roughness multiplier", "b": "metallic multiplier (255)", "a": 255},
        "generator": Path(__file__).name,
        "sources": sources,
        "outputs": outputs,
    }
    MANIFEST.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="\n")
    print(json.dumps(manifest, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
