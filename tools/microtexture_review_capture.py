#!/usr/bin/env python3
"""Capture one matched microtexture detail OFF/ON focus view.

The app must already be served. Requires Pillow and Playwright:
  pip install pillow playwright && playwright install chromium
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit

try:
    from PIL import Image, ImageStat
    from playwright.sync_api import Error as PlaywrightError
    from playwright.sync_api import sync_playwright
except ImportError as exc:
    raise SystemExit(
        "microtexture review capture requires Pillow and Playwright; install with "
        "`pip install pillow playwright && playwright install chromium`"
    ) from exc

FOCUSES = ("apartment", "materials", "traffic")
EXPECTED_ASSETS = {
    "apartment": [
        "models/world/isometric/apartment_modern_balconies.glb#Scene0",
    ],
    "materials": [
        "models/world/isometric/house_cottage_gabled.glb#Scene0",
        "models/world/isometric/tree_urban_blocky.glb#Scene0",
        "runtime:ground-grass",
        "runtime:ground-soil",
    ],
    "traffic": [
        "models/traffic/toy/npc_toy_sedan.glb#Scene0",
        "models/traffic/toy/npc_toy_city_van.glb#Scene0",
        "models/traffic/toy/npc_toy_hatchback.glb#Scene0",
        "models/traffic/toy/npc_toy_pickup.glb#Scene0",
        "models/traffic/toy/npc_toy_suv.glb#Scene0",
    ],
}
EXPECTED_PRIMITIVES = {"apartment": 10, "materials": 14, "traffic": 71}
EXPECTED_TUNING = {
    "concrete_albedo_srgb": [228, 255], "concrete_repeat": 2,
    "concrete_maps": ["albedo", "orm"],
    "concrete_normal": "none (authored facade geometry only)",
    "foliage_albedo_srgb": [236, 255], "foliage_repeat": 2,
    "foliage_scope": "closed Leaf only; Planter Green excluded",
    "traffic_albedo_srgb": [232, 255],
    "traffic_orm_ranges": [[250, 255], [220, 255], [255, 255], [255, 255]],
    "traffic_repeat": 2, "traffic_normal": "plastic_normal strength 0.035",
    "traffic_exclusions": ["buildings", "player", "accent", "glass", "lights",
                           "trim", "tires", "authored-texture-slots"],
}
EXPECTED_STAGES = [
    "assets-loaded",
    "scenes-instantiated",
    "detail-maps-loaded",
    "runtime-bindings-complete",
    "matched-frame-ready",
]


def review_url(base: str, focus: str = "apartment") -> str:
    if focus not in FOCUSES:
        raise ValueError(f"unknown microtexture focus: {focus}")
    parts = urlsplit(base)
    query = dict(parse_qsl(parts.query, keep_blank_values=True))
    query["microtexture_review"] = "1"
    query["microtexture_focus"] = focus
    return urlunsplit((parts.scheme, parts.netloc, parts.path, urlencode(query), parts.fragment))


def validate_metadata(metadata: dict, focus: str) -> None:
    primitives = metadata.get("primitives", {})
    on_maps = metadata.get("on_maps", {})
    cache = metadata.get("cache", {})
    expected = EXPECTED_PRIMITIVES[focus]
    valid_cache_counts = all(
        isinstance(cache.get(key), int) and cache[key] >= 0
        for key in ("meshes", "materials", "failed_meshes")
    )
    if (
        metadata.get("schema") != "roady-microtexture-review-v4"
        or metadata.get("ready") is not True
        or metadata.get("focus") != focus
        or metadata.get("sides") != ["detail-off", "detail-on"]
        or metadata.get("camera") != "matched-orthographic-grazing-isometric"
        or metadata.get("lighting") != "shared-low-angle-key-fill"
        or metadata.get("assets") != EXPECTED_ASSETS[focus]
        or metadata.get("stages") != EXPECTED_STAGES
        or len(metadata.get("detail_maps", [])) != 13
        or metadata.get("tuning") != EXPECTED_TUNING
        or primitives.get("expected_per_side") != expected
        or primitives.get("off_processed") != expected
        or primitives.get("on_processed") != expected
        or primitives.get("pending") != 0
        or not isinstance(on_maps.get("albedo"), int)
        or on_maps["albedo"] < 0
        or (focus != "traffic" and on_maps["albedo"] <= 0)
        or not isinstance(on_maps.get("normal"), int)
        or on_maps["normal"] <= 0
        or not isinstance(on_maps.get("orm"), int)
        or on_maps["orm"] <= 0
        or not valid_cache_counts
        or cache.get("stable_updates") != 2
    ):
        raise RuntimeError(f"unexpected {focus} microtexture-review metadata: {metadata}")


def validate_capture(path: Path) -> None:
    with Image.open(path) as image:
        image = image.convert("RGB")
        if image.width < 640 or image.height < 480:
            raise RuntimeError(f"capture is unexpectedly small: {image.size}")
        stat = ImageStat.Stat(image.resize((160, 100)))
        spans = [high - low for low, high in image.getextrema()]
        if max(spans) < 24 or max(stat.var) < 35.0:
            raise RuntimeError(
                f"rejected blank/nearly uniform capture (spans={spans}, variance={stat.var})"
            )


def capture(args: argparse.Namespace) -> None:
    args.png.parent.mkdir(parents=True, exist_ok=True)
    args.json.parent.mkdir(parents=True, exist_ok=True)
    url = review_url(args.url, args.focus)
    with sync_playwright() as playwright:
        channel = args.browser_channel.strip()
        launch = {"headless": not args.headed}
        if channel.lower() not in {"", "chromium", "playwright", "bundled"}:
            launch["channel"] = channel
        browser = playwright.chromium.launch(**launch)
        try:
            context = browser.new_context(
                viewport={"width": args.width, "height": args.height},
                device_scale_factor=args.dpr,
                reduced_motion="reduce",
            )
            page = context.new_page()
            errors: list[str] = []
            page.on(
                "console",
                lambda message: errors.append(f"console.{message.type}: {message.text}")
                if message.type == "error" else None,
            )
            page.on("pageerror", lambda error: errors.append(f"pageerror: {error}"))
            response = page.goto(url, wait_until="domcontentloaded", timeout=args.timeout_ms)
            if response is not None and not response.ok:
                raise RuntimeError(f"HTTP {response.status} loading {url}")
            page.wait_for_function(
                "document.documentElement.dataset.roadyMicrotextureReviewReady === 'true' "
                "&& typeof window.__ROADY_MICROTEXTURE_REVIEW__ === 'string'",
                timeout=args.timeout_ms,
            )
            page.wait_for_function(
                "!document.getElementById('loading') || document.getElementById('loading').hidden",
                timeout=args.timeout_ms,
            )
            canvas = page.locator("canvas").first
            canvas.wait_for(state="visible", timeout=args.timeout_ms)
            page.wait_for_timeout(args.settle_ms)
            metadata = json.loads(page.evaluate("window.__ROADY_MICROTEXTURE_REVIEW__"))
            validate_metadata(metadata, args.focus)
            canvas.screenshot(path=str(args.png), animations="disabled")
            if errors:
                raise RuntimeError("browser errors:\n  " + "\n  ".join(errors))
            validate_capture(args.png)
            args.json.write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
            print(f"captured microtexture {args.focus} A/B review: {args.png}")
        finally:
            browser.close()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://127.0.0.1:8080/")
    parser.add_argument("--focus", choices=FOCUSES, default="apartment")
    parser.add_argument("--png", type=Path, default=Path("artifacts/microtexture-review.png"))
    parser.add_argument("--json", type=Path, default=Path("artifacts/microtexture-review.json"))
    parser.add_argument("--width", type=int, default=1600)
    parser.add_argument("--height", type=int, default=900)
    parser.add_argument("--dpr", type=float, default=1.0)
    parser.add_argument("--settle-ms", type=int, default=1200)
    parser.add_argument("--timeout-ms", type=int, default=45000)
    parser.add_argument("--headed", action="store_true")
    parser.add_argument(
        "--browser-channel", default=os.environ.get("BROWSER_CHANNEL", "chrome"),
        help="Chrome channel, or 'chromium' for Playwright's bundled browser",
    )
    args = parser.parse_args()
    if (args.width < 640 or args.height < 480 or args.dpr <= 0
            or args.settle_ms < 0 or args.timeout_ms <= 0):
        parser.error("invalid viewport, DPR, settle time, or timeout")
    return args


if __name__ == "__main__":
    try:
        capture(parse_args())
    except (OSError, RuntimeError, ValueError, PlaywrightError) as exc:
        print(f"microtexture review capture failed: {exc}", file=sys.stderr)
        raise SystemExit(1) from exc
