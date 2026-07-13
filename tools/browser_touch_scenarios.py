#!/usr/bin/env python3
"""Strict mobile/touch browser scenario for Roady Car.

Uses Playwright touch emulation plus Chrome DevTools multi-touch dispatch.
Canvas-rendered state transitions are covered by Rust pure tests; this scenario
asserts responsive canvas fit, exercises every touch path, captures screenshots,
and fails on browser/network/runtime errors.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import traceback
from pathlib import Path
from typing import Any


def args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:8080")
    parser.add_argument("--out-dir", default="tools/scenarios/touch")
    parser.add_argument(
        "--browser-channel",
        default=os.environ.get("BROWSER_CHANNEL", "chrome"),
    )
    return parser.parse_args()


def channel(value: str) -> str | None:
    return None if value.lower() in {"", "chromium", "playwright", "bundled"} else value


def main() -> int:
    options = args()
    out_dir = Path(options.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    summary: dict[str, Any] = {
        "scenario": "roady_car_touch",
        "url": options.url,
        "status": "running",
        "screenshots": [],
        "console_errors": [],
        "page_errors": [],
        "network_failures": [],
        "http_errors": [],
    }
    started = time.monotonic()

    try:
        from playwright.sync_api import sync_playwright

        with sync_playwright() as playwright:
            launch = {"headless": True}
            selected = channel(options.browser_channel)
            if selected:
                launch["channel"] = selected
            browser = playwright.chromium.launch(**launch)
            context = browser.new_context(
                viewport={"width": 844, "height": 390},
                has_touch=True,
                is_mobile=True,
                device_scale_factor=1,
            )
            page = context.new_page()
            page.on(
                "console",
                lambda message: summary["console_errors"].append(message.text)
                if message.type == "error"
                else None,
            )
            page.on("pageerror", lambda error: summary["page_errors"].append(str(error)))
            page.on(
                "requestfailed",
                lambda request: None
                if request.failure == "net::ERR_ABORTED" and "/v1/leaderboard" in request.url
                else summary["network_failures"].append(
                    {"url": request.url, "failure": request.failure}
                ),
            )
            page.on(
                "response",
                lambda response: summary["http_errors"].append(
                    {"url": response.url, "status": response.status}
                )
                if response.status >= 400
                else None,
            )

            def shot(name: str) -> None:
                path = out_dir / name
                page.screenshot(path=str(path))
                summary["screenshots"].append(str(path))

            page.goto(options.url, wait_until="load", timeout=120_000)
            canvas = page.locator("canvas").first
            canvas.wait_for(state="visible", timeout=120_000)
            page.locator("#loading").wait_for(state="hidden", timeout=120_000)
            page.wait_for_timeout(800)
            rect = canvas.evaluate(
                "e => { const r=e.getBoundingClientRect(); return {width:r.width,height:r.height}; }"
            )
            if abs(rect["width"] - 844) > 1 or abs(rect["height"] - 390) > 1:
                raise AssertionError(f"canvas did not fit mobile viewport: {rect}")
            summary["canvas"] = rect
            shot("00_mobile_menu.png")

            # Any touch starts Menu, activates touch controls, and unlocks audio.
            page.touchscreen.tap(422, 195)
            page.wait_for_timeout(3_800)
            shot("01_touch_hud.png")

            # Multi-touch steering + GO, followed by an explicit release.
            cdp = context.new_cdp_session(page)
            touches = [
                {"x": 80, "y": 335, "radiusX": 8, "radiusY": 8, "force": 1, "id": 1},
                {"x": 790, "y": 335, "radiusX": 8, "radiusY": 8, "force": 1, "id": 2},
            ]
            cdp.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": touches})
            page.wait_for_timeout(1_600)
            touches[0]["x"] = 25
            cdp.send("Input.dispatchTouchEvent", {"type": "touchMove", "touchPoints": touches})
            page.wait_for_timeout(800)
            cdp.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})
            page.wait_for_timeout(400)
            shot("02_after_multitouch.png")

            # Pause zone; left third resumes.
            page.touchscreen.tap(422, 25)
            page.wait_for_timeout(500)
            shot("03_paused.png")
            page.touchscreen.tap(100, 195)
            page.wait_for_timeout(500)
            shot("04_resumed.png")

            # Middle third performs restart through Menu and a fresh countdown.
            page.touchscreen.tap(422, 25)
            page.wait_for_timeout(350)
            page.touchscreen.tap(422, 195)
            page.wait_for_timeout(800)
            shot("05_touch_restart_countdown.png")
            page.wait_for_timeout(3_200)

            # Right third returns to Menu.
            page.touchscreen.tap(422, 25)
            page.wait_for_timeout(350)
            page.touchscreen.tap(760, 195)
            page.wait_for_timeout(700)
            shot("06_touch_menu.png")
            page.wait_for_timeout(300)

            for key in ("console_errors", "page_errors", "network_failures", "http_errors"):
                if summary[key]:
                    raise AssertionError(f"{key}: {summary[key]}")

            context.close()
            browser.close()
            summary["status"] = "passed"
    except Exception as exc:  # noqa: BLE001
        summary["status"] = "failed"
        summary["failure"] = {
            "type": type(exc).__name__,
            "message": str(exc),
            "traceback": traceback.format_exc(),
        }
    finally:
        summary["duration_ms"] = round((time.monotonic() - started) * 1000)
        print(json.dumps(summary, indent=2, sort_keys=True))

    return 0 if summary["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
