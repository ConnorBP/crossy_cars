#!/usr/bin/env python3
"""Regression tests for the final deterministic PBR microdetail set."""
from __future__ import annotations

import hashlib
import importlib.util
import json
import math
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from PIL import Image

HERE = Path(__file__).resolve().parent
PROJECT = HERE.parent
SPEC = importlib.util.spec_from_file_location("build_pbr_details", HERE / "build_pbr_details.py")
assert SPEC and SPEC.loader
pbr = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = pbr
SPEC.loader.exec_module(pbr)


class GeneratorTests(unittest.TestCase):
    def test_fixed_seed_toroidal_noise_is_deterministic_seamless_and_two_octave(self) -> None:
        first = pbr.toroidal_isotropic_noise(12345)
        second = pbr.toroidal_isotropic_noise(12345)
        self.assertEqual(first, second)
        self.assertEqual(pbr.CYCLES, (24, 48))
        self.assertEqual(pbr.OCTAVE_WEIGHTS, (0.7, 0.3))
        self.assertEqual((min(first), max(first)), (0.0, 1.0))
        for y in range(pbr.SIZE):
            self.assertEqual(first[y * pbr.SIZE], first[y * pbr.SIZE + pbr.SIZE - 1])
        for x in range(pbr.SIZE):
            self.assertEqual(first[x], first[(pbr.SIZE - 1) * pbr.SIZE + x])

    def test_distinct_fields_are_not_reused(self) -> None:
        fields = [pbr.toroidal_isotropic_noise(seed) for seed in pbr.SEEDS.values()]
        hashes = {hashlib.sha256(bytes(round(value * 255) for value in field)).hexdigest() for field in fields}
        self.assertEqual(len(hashes), len(fields))

    def test_generator_is_byte_deterministic_and_writes_lf_constants(self) -> None:
        tracked = [pbr.MANIFEST, pbr.CONSTANTS] + [pbr.OUT / f"{name}.png" for name in (
            "concrete_albedo", "foliage_albedo", "traffic_paint_albedo", "traffic_paint_orm",
            "grass_normal", "grass_orm", "soil_orm",
        )]
        before = {path: path.read_bytes() for path in tracked}
        subprocess.run([sys.executable, str(HERE / "build_pbr_details.py")], check=True, stdout=subprocess.DEVNULL)
        self.assertEqual(before, {path: path.read_bytes() for path in tracked})
        constants = pbr.CONSTANTS.read_bytes()
        self.assertNotIn(b"\r", constants)
        self.assertTrue(constants.startswith(b"// @generated"))


class CommittedDerivativeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.directory = PROJECT / "assets" / "textures" / "pbr_detail"
        cls.manifest = json.loads((cls.directory / "manifest.json").read_text(encoding="utf-8"))

    def image(self, key: str) -> Image.Image:
        return Image.open(PROJECT / self.manifest["outputs"][key]["path"]).convert("RGBA")

    def test_manifest_and_files_are_hash_locked(self) -> None:
        self.assertEqual(self.manifest["schema"], "roady.pbr-detail.v5")
        self.assertEqual(self.manifest["generation"]["cycles"], [24, 48])
        self.assertEqual(self.manifest["generation"]["weights"], [0.7, 0.3])
        self.assertEqual(self.manifest["generation"]["runtime_repeat"], 2)
        for output in self.manifest["outputs"].values():
            path = PROJECT / output["path"]
            data = path.read_bytes()
            self.assertEqual(len(data), output["bytes"], path.name)
            self.assertEqual(hashlib.sha256(data).hexdigest(), output["sha256"], path.name)

    def test_every_derivative_has_exact_rgba_edges(self) -> None:
        for key in self.manifest["outputs"]:
            with self.image(key) as image:
                self.assertEqual(image.size, (256, 256))
                pbr.assert_exact_edges(image, key)

    def test_albedo_ranges_grayscale_means_and_generated_constants_are_exact(self) -> None:
        expected = {
            "concrete_albedo": (228, "CONCRETE_ALBEDO_LINEAR_MEAN"),
            "foliage_albedo": (236, "FOLIAGE_ALBEDO_LINEAR_MEAN"),
            "traffic_paint_albedo": (232, "TRAFFIC_PAINT_ALBEDO_LINEAR_MEAN"),
        }
        constants = pbr.CONSTANTS.read_text(encoding="utf-8")
        for key, (floor, constant) in expected.items():
            with self.image(key) as image:
                pbr.assert_exact_channels(image, ((floor, 255),) * 3 + ((255, 255),), key)
                pixels = list(image.getdata())
                self.assertTrue(all(r == g == b and a == 255 for r, g, b, a in pixels))
                mean = pbr.decoded_linear_mean(image)
            recorded = self.manifest["outputs"][key]["decoded_linear_mean"]
            self.assertLessEqual(abs(mean - recorded), 1e-12)
            line = next(line for line in constants.splitlines() if constant in line)
            generated = float(line.split("=")[1].strip().rstrip(";"))
            self.assertLessEqual(abs(mean - generated), 1e-7)

    def test_traffic_orm_and_generated_roughness_constant_are_exact(self) -> None:
        with self.image("traffic_paint_orm") as image:
            pbr.assert_exact_channels(image, ((250, 255), (220, 255), (255, 255), (255, 255)), "traffic")
            mean = pbr.linear_channel_mean(image, 1)
        constants = pbr.CONSTANTS.read_text(encoding="utf-8")
        line = next(line for line in constants.splitlines() if "TRAFFIC_PAINT_ROUGHNESS_LINEAR_MEAN" in line)
        generated = float(line.split("=")[1].strip().rstrip(";"))
        self.assertLessEqual(abs(mean - generated), 1e-7)

    def test_grass_and_soil_use_separate_bounded_fields(self) -> None:
        with self.image("grass_normal") as normal:
            metrics = pbr.normal_metrics(normal)
            self.assertLessEqual(metrics["length_error_max"], pbr.NORMAL_LENGTH_TOLERANCE)
            self.assertGreater(metrics["amplitude_mean"], 0.0)
            self.assertLessEqual(metrics["amplitude_max"], 0.035)
        with self.image("grass_orm") as grass, self.image("soil_orm") as soil:
            pbr.assert_exact_channels(grass, ((248, 255), (238, 255), (255, 255), (255, 255)), "grass")
            pbr.assert_exact_channels(soil, ((246, 255), (232, 255), (255, 255), (255, 255)), "soil")
            self.assertNotEqual(hashlib.sha256(grass.tobytes()).digest(), hashlib.sha256(soil.tobytes()).digest())
            self.assertNotEqual(list(zip(*grass.getdata()))[0], list(zip(*grass.getdata()))[1])
            self.assertNotEqual(list(zip(*soil.getdata()))[0], list(zip(*soil.getdata()))[1])

    def test_wood_and_legacy_roof_data_are_unchanged(self) -> None:
        expected = {
            "wood_normal": "b21c9425ac5d05104955b026ca8fbb7c1ad4b1ebea19676d60253f7a0e0db740",
            "wood_orm": "8906aaee4615d697161185e4c6f69de64a1cc33012cdeda225a0cbc09f0312f5",
            "concrete_normal": "5df8a179267f02c14e82f801e84c63e5cbd0f74daef870d39d9951314070dba4",
            "concrete_orm": "e3932426fed9fb8cbe83d010dad95960d7629dbad9d17596cc829585f600c2b6",
        }
        for key, digest in expected.items():
            self.assertEqual(self.manifest["outputs"][key]["sha256"], digest)


if __name__ == "__main__":
    unittest.main()
