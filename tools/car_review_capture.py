#!/usr/bin/env python3
"""Capture the opt-in Roady car-review studio from deterministic viewpoints.

The web app must already be served (this tool deliberately does not build it):

    python tools/car_review_capture.py --url http://127.0.0.1:8080 \
        --output-dir review/car-iteration2

Requires Playwright (``pip install playwright && playwright install chromium``)
and Pillow. Every image is named with its view label; ``manifest.txt`` records
the same labels. Browser/page errors and blank or nearly uniform captures make
the command fail rather than silently producing unusable review evidence.
"""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit

try:
    from PIL import Image, ImageStat
    from playwright.sync_api import Error as PlaywrightError
    from playwright.sync_api import sync_playwright
except ImportError as exc:  # pragma: no cover - actionable command-line failure
    raise SystemExit(
        "car review capture requires Pillow and Playwright; install with "
        "`pip install pillow playwright && playwright install chromium`"
    ) from exc


VIEWS = (
    "front",
    "rear",
    "left",
    "right",
    "front_left",
    "front_right",
    "rear_left",
    "rear_right",
)


def review_url(base: str, view: str) -> str:
    parts = urlsplit(base)
    query = dict(parse_qsl(parts.query, keep_blank_values=True))
    query.update(car_review="1", car_view=view)
    return urlunsplit((parts.scheme, parts.netloc, parts.path, urlencode(query), parts.fragment))


def validate_capture(path: Path) -> None:
    with Image.open(path) as image:
        image = image.convert("RGB")
        if image.width < 320 or image.height < 240:
            raise RuntimeError(f"{path}: capture is unexpectedly small ({image.size})")
        stat = ImageStat.Stat(image.resize((160, 100)))
        extrema = image.getextrema()
        spans = [high - low for low, high in extrema]
        # A failed WebGL canvas is commonly transparent/black or one flat clear
        # color. Require both useful tonal range and spatial variance.
        if max(spans) < 24 or max(stat.var) < 35.0:
            raise RuntimeError(
                f"{path}: rejected blank/nearly uniform capture "
                f"(channel spans={spans}, variance={stat.var})"
            )


def capture(args: argparse.Namespace) -> None:
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    manifest: list[str] = []

    with sync_playwright() as playwright:
        channel = args.browser_channel.strip()
        launch = {"headless": not args.headed}
        if channel.lower() not in {"", "chromium", "playwright", "bundled"}:
            launch["channel"] = channel
        browser = playwright.chromium.launch(**launch)
        try:
            for view in VIEWS:
                page = browser.new_page(
                    viewport={"width": args.width, "height": args.height},
                    device_scale_factor=1,
                )
                errors: list[str] = []
                page.on(
                    "console",
                    lambda message, errors=errors: errors.append(
                        f"console.{message.type}: {message.text}"
                    )
                    if message.type == "error"
                    else None,
                )
                page.on("pageerror", lambda error, errors=errors: errors.append(f"pageerror: {error}"))
                url = review_url(args.url, view)
                try:
                    response = page.goto(url, wait_until="domcontentloaded", timeout=args.timeout_ms)
                    if response is not None and not response.ok:
                        raise RuntimeError(f"{view}: HTTP {response.status} loading {url}")
                    page.wait_for_selector("canvas", state="visible", timeout=args.timeout_ms)
                    # Wait for Bevy startup/assets and several stable render frames.
                    page.wait_for_timeout(args.settle_ms)
                    canvas = page.locator("canvas").first
                    path = output / f"car_review__{view}.png"
                    canvas.screenshot(path=str(path), animations="disabled")
                    if errors:
                        raise RuntimeError(f"{view}: browser errors:\n  " + "\n  ".join(errors))
                    validate_capture(path)
                    manifest.append(f"{view}\t{path.name}\t{url}")
                    print(f"captured {view:>11}: {path}")
                finally:
                    page.close()
        finally:
            browser.close()

    (output / "manifest.txt").write_text(
        "label\tfile\turl\n" + "\n".join(manifest) + "\n", encoding="utf-8"
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://127.0.0.1:8080", help="already-served game URL")
    parser.add_argument("--output-dir", type=Path, default=Path("car_review"))
    parser.add_argument("--width", type=int, default=960)
    parser.add_argument("--height", type=int, default=720)
    parser.add_argument("--settle-ms", type=int, default=1800)
    parser.add_argument("--timeout-ms", type=int, default=20000)
    parser.add_argument("--headed", action="store_true", help="show Chromium while capturing")
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
        print(f"car review capture failed: {exc}", file=sys.stderr)
        raise SystemExit(1) from exc
