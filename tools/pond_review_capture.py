#!/usr/bin/env python3
"""Capture Roady's isolated deterministic pond-review tableau and metadata.

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
        "pond review capture requires Pillow and Playwright; install with "
        "`pip install pillow playwright && playwright install chromium`"
    ) from exc


def review_url(base: str, motion: str) -> str:
    parts = urlsplit(base)
    query = dict(parse_qsl(parts.query, keep_blank_values=True))
    query["pond_review"] = "1"
    if motion == "normal":
        query["pond_motion"] = "1"
    else:
        query.pop("pond_motion", None)
    return urlunsplit((parts.scheme, parts.netloc, parts.path, urlencode(query), parts.fragment))


def validate_capture(path: Path) -> None:
    with Image.open(path) as image:
        image = image.convert("RGB")
        if image.width < 320 or image.height < 240:
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
    url = review_url(args.url, args.motion)
    with sync_playwright() as playwright:
        channel = args.browser_channel.strip()
        launch = {"headless": not args.headed}
        if channel.lower() not in {"", "chromium", "playwright", "bundled"}:
            launch["channel"] = channel
        browser = playwright.chromium.launch(**launch)
        try:
            page = browser.new_page(
                viewport={"width": args.width, "height": args.height},
                device_scale_factor=1,
                reduced_motion="reduce" if args.motion == "reduced" else "no-preference",
            )
            errors: list[str] = []
            page.on(
                "console",
                lambda message: errors.append(f"console.{message.type}: {message.text}")
                if message.type == "error"
                else None,
            )
            page.on("pageerror", lambda error: errors.append(f"pageerror: {error}"))
            response = page.goto(url, wait_until="domcontentloaded", timeout=args.timeout_ms)
            if response is not None and not response.ok:
                raise RuntimeError(f"HTTP {response.status} loading {url}")
            page.wait_for_function(
                "document.documentElement.dataset.roadyPondReviewReady === 'true' "
                "&& typeof window.__ROADY_POND_REVIEW__ === 'string'",
                timeout=args.timeout_ms,
            )
            canvas = page.locator("canvas").first
            canvas.wait_for(state="visible", timeout=args.timeout_ms)
            page.wait_for_timeout(args.settle_ms)
            metadata = json.loads(page.evaluate("window.__ROADY_POND_REVIEW__"))
            expected_materials = ["GardenOval", "ReedMarsh", "FarmReservoir"]
            expected_stages = ["pre-entry", "splash-rings", "mid-sink", "sunk"]
            if (
                metadata.get("schema") != "roady-pond-review-v1"
                or not metadata.get("ready")
                or metadata.get("motion") != args.motion
                or metadata.get("materials") != expected_materials
                or metadata.get("stages") != expected_stages
            ):
                raise RuntimeError(f"unexpected pond-review metadata: {metadata}")
            canvas.screenshot(path=str(args.png), animations="disabled")
            if errors:
                raise RuntimeError("browser errors:\n  " + "\n  ".join(errors))
            validate_capture(args.png)
            args.json.write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
            print(f"captured pond review: {args.png}")
        finally:
            browser.close()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://127.0.0.1:8080/")
    parser.add_argument("--png", type=Path, default=Path("artifacts/pond-review.png"))
    parser.add_argument("--json", type=Path, default=Path("artifacts/pond-review.json"))
    parser.add_argument("--width", type=int, default=1280)
    parser.add_argument("--height", type=int, default=720)
    parser.add_argument("--settle-ms", type=int, default=1800)
    parser.add_argument("--timeout-ms", type=int, default=30000)
    parser.add_argument("--motion", choices=("reduced", "normal"), default="reduced")
    parser.add_argument("--headed", action="store_true")
    parser.add_argument(
        "--browser-channel",
        default=os.environ.get("BROWSER_CHANNEL", "chrome"),
        help="Chrome channel, or 'chromium' for Playwright's bundled browser",
    )
    args = parser.parse_args()
    if args.width < 320 or args.height < 240 or args.settle_ms < 0 or args.timeout_ms <= 0:
        parser.error("invalid viewport, settle time, or timeout")
    return args


if __name__ == "__main__":
    try:
        capture(parse_args())
    except (OSError, RuntimeError, PlaywrightError) as exc:
        print(f"pond review capture failed: {exc}", file=sys.stderr)
        raise SystemExit(1) from exc
