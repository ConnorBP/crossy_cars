import importlib.util
import sys
import unittest
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

MODULE_PATH = Path(__file__).with_name("microtexture_review_capture.py")
SPEC = importlib.util.spec_from_file_location("microtexture_review_capture", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


def valid_metadata(focus):
    expected = MODULE.EXPECTED_PRIMITIVES[focus]
    return {
        "schema": "roady-microtexture-review-v4", "ready": True, "focus": focus,
        "sides": ["detail-off", "detail-on"],
        "camera": "matched-orthographic-grazing-isometric",
        "lighting": "shared-low-angle-key-fill",
        "assets": MODULE.EXPECTED_ASSETS[focus],
        "detail_maps": [f"map-{index}" for index in range(13)],
        "tuning": MODULE.EXPECTED_TUNING,
        "stages": MODULE.EXPECTED_STAGES,
        "primitives": {"expected_per_side": expected, "off_processed": expected,
                       "on_processed": expected, "pending": 0},
        "on_maps": {"albedo": 1, "normal": 1, "orm": 1},
        "cache": {"meshes": 1, "materials": 1,
                  "failed_meshes": 0, "stable_updates": 2},
    }


class MicrotextureReviewCaptureTests(unittest.TestCase):
    def test_review_url_adds_default_apartment_focus_and_preserves_query(self):
        query = parse_qs(urlsplit(MODULE.review_url("https://example.test/game?foo=bar")).query)
        self.assertEqual(query, {"foo": ["bar"], "microtexture_review": ["1"],
                                 "microtexture_focus": ["apartment"]})

    def test_review_url_selects_each_focus_and_rejects_unknown(self):
        for focus in MODULE.FOCUSES:
            query = parse_qs(urlsplit(MODULE.review_url("https://example.test", focus)).query)
            self.assertEqual(query["microtexture_focus"], [focus])
        with self.assertRaises(ValueError):
            MODULE.review_url("https://example.test", "wide")

    def test_normal_url_has_no_review_marker(self):
        query = parse_qs(urlsplit("https://example.test/game?foo=bar").query)
        self.assertNotIn("microtexture_review", query)
        self.assertNotIn("microtexture_focus", query)

    def test_traffic_contract_lists_all_five_glbs(self):
        traffic = MODULE.EXPECTED_ASSETS["traffic"]
        self.assertEqual(len(traffic), 5)
        self.assertEqual({Path(asset.split("#", 1)[0]).stem for asset in traffic},
                         {"npc_toy_sedan", "npc_toy_city_van", "npc_toy_hatchback",
                          "npc_toy_pickup", "npc_toy_suv"})

    def test_each_focus_accepts_complete_matched_metadata(self):
        for focus in MODULE.FOCUSES:
            MODULE.validate_metadata(valid_metadata(focus), focus)

    def test_metadata_rejects_pending_mismatch_maps_and_unstable_cache(self):
        for path, value in [(("primitives", "pending"), 1),
                            (("primitives", "on_processed"), 0),
                            (("on_maps", "albedo"), 0), (("on_maps", "normal"), 0),
                            (("on_maps", "orm"), 0),
                            (("cache", "stable_updates"), 1)]:
            metadata = valid_metadata("apartment")
            metadata[path[0]][path[1]] = value
            with self.assertRaises(RuntimeError):
                MODULE.validate_metadata(metadata, "apartment")

    def test_readiness_stages_require_runtime_bindings_and_matched_frame(self):
        self.assertIn("runtime-bindings-complete", MODULE.EXPECTED_STAGES)
        self.assertEqual(MODULE.EXPECTED_STAGES[-1], "matched-frame-ready")


if __name__ == "__main__":
    unittest.main()
