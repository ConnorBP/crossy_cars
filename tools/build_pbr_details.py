#!/usr/bin/env python3
"""Build Roady's tiny toy-town PBR detail library from free-use source maps.

Reads selected entries directly from the user's source ZIPs, attenuates normal
relief, packs AO/roughness/metallic channels, and writes only 256px derivatives.
The original 4K maps are never copied into the repository.
"""
from __future__ import annotations

import hashlib
import json
import math
from dataclasses import dataclass
from io import BytesIO
from pathlib import Path
from zipfile import ZipFile

from PIL import Image, ImageFile, ImageOps

SIZE = 256
ROOT = Path(r"E:/DEVELOPER/PBR_MATERIALS")
OUT = Path(__file__).resolve().parents[1] / "assets" / "textures" / "pbr_detail"
MANIFEST = OUT / "manifest.json"


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
        0.20,
        205,
        232,
    ),
    "concrete": Family(
        "concrete_wall_01_4k.zip",
        "concrete_wall_01_4k/concrete_wall_01_normal_gl_4k.png",
        "concrete_wall_01_4k/concrete_wall_01_roughness_4k.png",
        "concrete_wall_01_4k/concrete_wall_01_ambient_occlusion_4k.png",
        0.32,
        210,
        228,
    ),
    "wood": Family(
        "wood_01_4k.zip",
        "wood_01_4k/wood_01_normal_gl_4k.png",
        "wood_01_4k/wood_01_roughness_4k.png",
        "wood_01_4k/wood_01_ambient_occlusion_4k.png",
        0.27,
        205,
        228,
    ),
    "grass": Family(
        "whispy-grass-meadow-bl.zip",
        "whispy-grass-meadow-bl/wispy-grass-meadow_normal-ogl.png",
        "whispy-grass-meadow-bl/wispy-grass-meadow_roughness.png",
        "whispy-grass-meadow-bl/wispy-grass-meadow_ao.png",
        0.22,
        220,
        232,
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


def normalized_luminance(image: Image.Image) -> list[float]:
    # Several 4K sources are 16-bit grayscale. Converting those directly to
    # `L` clips values above 255 and produces an almost uniform white map.
    # Resize in float space, then normalize the resulting sampled range.
    sampled = image.convert("F").resize((SIZE, SIZE), Image.Resampling.LANCZOS)
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
    """Make a seamless 256px tile from a recovered 128px source sample."""
    sample = image.convert("L").resize((SIZE // 2, SIZE // 2), Image.Resampling.LANCZOS)
    tile = Image.new("L", (SIZE, SIZE))
    tile.paste(sample, (0, 0))
    tile.paste(ImageOps.mirror(sample), (SIZE // 2, 0))
    tile.paste(ImageOps.flip(sample), (0, SIZE // 2))
    tile.paste(ImageOps.flip(ImageOps.mirror(sample)), (SIZE // 2, SIZE // 2))
    return tile


def soil_orm(smudge: Image.Image) -> Image.Image:
    values = normalized_luminance(mirrored_tile(smudge))
    output = Image.new("RGBA", (SIZE, SIZE))
    output.putdata(
        [
            (
                round(232 + value * 23),
                round(210 + value * 45),
                255,
                255,
            )
            for value in values
        ]
    )
    return output


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
        normal = attenuated_normal(decode(normal_bytes), family.normal_strength)
        orm = packed_orm(
            decode(ao_bytes),
            decode(roughness_bytes),
            family.ao_floor,
            family.roughness_floor,
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
            "normal_strength": family.normal_strength,
            "roughness_floor": family.roughness_floor,
            "ao_floor": family.ao_floor,
        }

    smudge_path = ROOT / "Smudges01_4K.png"
    smudge_bytes = smudge_path.read_bytes()
    smudge = decode(smudge_bytes, recover_truncated=True)
    outputs["soil_orm"] = save(soil_orm(smudge), "soil_orm.png")
    sources["soil"] = {
        "source": smudge_path.name,
        "source_sha256": sha256(smudge_bytes),
        "recovery": "Pillow LOAD_TRUNCATED_IMAGES=true; complete 4096x4096 RGB decoded",
        "tiling": "128px Lanczos sample mirrored in both axes into a seamless 256px tile",
        "roughness_floor": 210,
        "ao_floor": 232,
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
