#!/usr/bin/env python3
"""Capture Roady's deterministic production-world review PNG and JSON.

The release app must already be served from `dist` by a static HTTP server.
Do not use `trunk serve`, which can overwrite release output with reload code.
This tool never builds the project. Install Playwright with:
  python -m pip install playwright && playwright install chromium

Example:
  python tools/map_review_capture.py --url http://127.0.0.1:8080 \
      --png artifacts/world-review.png --json artifacts/world-review.json
"""

from __future__ import annotations

import argparse
import json
import os
import struct
import time
import zlib
from pathlib import Path
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit

from playwright.sync_api import sync_playwright


def review_url(base: str) -> str:
    parts = urlsplit(base)
    query = dict(parse_qsl(parts.query, keep_blank_values=True))
    query["world_review"] = "1"
    return urlunsplit((parts.scheme, parts.netloc, parts.path, urlencode(query), parts.fragment))


def screenshot_variance(png: bytes) -> int:
    """Return sampled RGB range from an 8-bit RGB/RGBA Playwright PNG."""
    if png[:8] != b"\x89PNG\r\n\x1a\n":
        raise RuntimeError("Playwright returned a non-PNG screenshot")
    offset, width, height, color_type, compressed = 8, 0, 0, 0, bytearray()
    while offset < len(png):
        length = struct.unpack(">I", png[offset : offset + 4])[0]
        kind = png[offset + 4 : offset + 8]
        data = png[offset + 8 : offset + 8 + length]
        offset += 12 + length
        if kind == b"IHDR":
            width, height, depth, color_type = struct.unpack(">IIBB", data[:10])
            if depth != 8 or color_type not in (2, 6):
                raise RuntimeError("unsupported screenshot PNG pixel format")
        elif kind == b"IDAT":
            compressed.extend(data)
        elif kind == b"IEND":
            break
    channels = 3 if color_type == 2 else 4
    stride = width * channels
    raw = zlib.decompress(compressed)
    previous = bytearray(stride)
    low, high, cursor = 765, 0, 0
    row_step = max(1, height // 64)
    column_step = max(1, width // 64)
    for y in range(height):
        filter_type = raw[cursor]
        cursor += 1
        encoded = raw[cursor : cursor + stride]
        cursor += stride
        row = bytearray(stride)
        for x, value in enumerate(encoded):
            left = row[x - channels] if x >= channels else 0
            above = previous[x]
            upper_left = previous[x - channels] if x >= channels else 0
            if filter_type == 0:
                predictor = 0
            elif filter_type == 1:
                predictor = left
            elif filter_type == 2:
                predictor = above
            elif filter_type == 3:
                predictor = (left + above) // 2
            elif filter_type == 4:
                estimate = left + above - upper_left
                distances = (abs(estimate - left), abs(estimate - above), abs(estimate - upper_left))
                predictor = (left, above, upper_left)[distances.index(min(distances))]
            else:
                raise RuntimeError("unsupported screenshot PNG filter")
            row[x] = (value + predictor) & 0xFF
        if y % row_step == 0:
            for x in range(0, width, column_step):
                base = x * channels
                luminance = row[base] + row[base + 1] + row[base + 2]
                low, high = min(low, luminance), max(high, luminance)
        previous = row
    return high - low


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://127.0.0.1:8080/")
    parser.add_argument("--png", type=Path, default=Path("artifacts/world-review.png"))
    parser.add_argument("--json", type=Path, default=Path("artifacts/world-review.json"))
    parser.add_argument("--width", type=int, default=1600)
    parser.add_argument("--height", type=int, default=1200)
    parser.add_argument("--timeout-ms", type=int, default=120_000)
    parser.add_argument(
        "--browser-channel",
        default=os.environ.get("BROWSER_CHANNEL", "chrome"),
        help="Chrome channel, or 'chromium' for Playwright's bundled browser",
    )
    args = parser.parse_args()
    if args.width <= 0 or args.height <= 0:
        parser.error("--width and --height must be positive")

    args.png.parent.mkdir(parents=True, exist_ok=True)
    args.json.parent.mkdir(parents=True, exist_ok=True)

    with sync_playwright() as playwright:
        channel = args.browser_channel.strip()
        launch = {"headless": True}
        if channel.lower() not in {"", "chromium", "playwright", "bundled"}:
            launch["channel"] = channel
        browser = playwright.chromium.launch(**launch)
        page = browser.new_page(
            viewport={"width": args.width, "height": args.height},
            device_scale_factor=1,
            reduced_motion="reduce",
        )
        console_errors: list[str] = []
        page_errors: list[str] = []
        page.on(
            "console",
            lambda message: console_errors.append(message.text)
            if message.type == "error"
            else None,
        )
        page.on("pageerror", lambda error: page_errors.append(str(error)))
        page.goto(review_url(args.url), wait_until="domcontentloaded", timeout=args.timeout_ms)
        page.wait_for_function(
            "document.documentElement.dataset.roadyWorldReviewReady === 'true' "
            "&& typeof window.__ROADY_WORLD_REVIEW__ === 'string'",
            timeout=args.timeout_ms,
        )
        page.wait_for_function(
            "!document.getElementById('loading') || document.getElementById('loading').hidden",
            timeout=args.timeout_ms,
        )
        canvas = page.locator("canvas").first
        canvas.wait_for(state="visible", timeout=args.timeout_ms)
        box = canvas.bounding_box()
        dimensions = canvas.evaluate("c => ({width: c.width, height: c.height})")
        if (
            not box
            or box["width"] <= 0
            or box["height"] <= 0
            or dimensions["width"] <= 0
            or dimensions["height"] <= 0
        ):
            raise RuntimeError("Roady canvas is not visible with nonzero dimensions")

        raw = page.evaluate("window.__ROADY_WORLD_REVIEW__")
        metadata = json.loads(raw)
        if metadata.get("schema") != "roady-world-review-v1" or not metadata.get("ready"):
            raise RuntimeError("unexpected or incomplete Roady world-review metadata")

        # The Rust marker means ECS/metadata ready only. Own render readiness
        # here: wait for several presented frames, require nonblank pixels, and
        # require stable CSS/backing dimensions. GPU output is not required to
        # be byte-identical: rasterization and PNG encoding may vary slightly.
        deadline = time.monotonic() + args.timeout_ms / 1000
        stable_geometry = None
        stable_frames = 0
        observed_frames = 0
        final_png = None
        while time.monotonic() < deadline:
            page.evaluate("() => new Promise(r => requestAnimationFrame(r))")
            png = canvas.screenshot()
            current_box = canvas.bounding_box()
            current_dimensions = canvas.evaluate("c => ({width: c.width, height: c.height})")
            visible_nonzero = (
                current_box
                and current_box["width"] > 0
                and current_box["height"] > 0
                and current_dimensions["width"] > 0
                and current_dimensions["height"] > 0
            )
            # Inspect actual screenshot pixels: a uniform clear-color frame is
            # never accepted as capture-ready.
            nonblank = visible_nonzero and screenshot_variance(png) > 8
            geometry = (
                round(current_box["width"], 3) if current_box else 0,
                round(current_box["height"], 3) if current_box else 0,
                current_dimensions["width"],
                current_dimensions["height"],
            )
            observed_frames += 1
            if nonblank and geometry == stable_geometry:
                stable_frames += 1
            elif nonblank:
                stable_geometry, stable_frames = geometry, 1
            else:
                stable_geometry, stable_frames = None, 0
            if stable_frames >= 3 and observed_frames >= 3:
                final_png = png
                break
        if final_png is None:
            raise RuntimeError("canvas never reached a stable, nonblank rendered state")
        if console_errors or page_errors:
            raise RuntimeError(
                "world-review browser errors: "
                + json.dumps(
                    {"console": console_errors[:6], "page": page_errors[:6]},
                    sort_keys=True,
                )
            )

        args.png.write_bytes(final_png)
        args.json.write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
        browser.close()

    print(f"captured {args.png} and {args.json}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
