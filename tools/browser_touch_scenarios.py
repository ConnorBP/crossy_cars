#!/usr/bin/env python3
"""Strict mobile/touch browser scenario for Roady Car.

Uses Playwright touch emulation plus Chrome DevTools multi-touch dispatch.
State transitions are behaviorally probed through touch-accessible Settings and
exact localStorage changes. The scenario also asserts responsive canvas fit,
exercises every touch path, captures screenshots, and fails on browser/network/
runtime errors.
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


STORAGE_KEY = "roady_car_settings"
LEGACY_KEY = "roady_car_audio_muted"
INITIAL_SETTINGS = "v2:100:0:0:"
STORAGE_ASSERT_TIMEOUT_MS = 60_000
STORAGE_POLL_INTERVAL_MS = 250
UNCHANGED_ASSERT_WAIT_MS = 2_000


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


def read_settings(page: Any) -> str | None:
    return page.evaluate(f"localStorage.getItem({json.dumps(STORAGE_KEY)})")


def wait_for_exact_settings(page: Any, expected: str, checkpoint: str) -> None:
    try:
        page.wait_for_function(
            f"localStorage.getItem({json.dumps(STORAGE_KEY)}) === {json.dumps(expected)}",
            timeout=STORAGE_ASSERT_TIMEOUT_MS,
            polling=STORAGE_POLL_INTERVAL_MS,
        )
    except Exception as exc:  # noqa: BLE001 - replace opaque polling timeout
        actual = read_settings(page)
        raise AssertionError(
            f"{checkpoint}: expected exact settings {expected!r} within "
            f"{STORAGE_ASSERT_TIMEOUT_MS}ms, got {actual!r}"
        ) from exc

    actual = read_settings(page)
    if actual != expected:
        raise AssertionError(
            f"{checkpoint}: expected exact settings {expected!r}, got {actual!r}"
        )


def assert_settings_unchanged(page: Any, expected: str, checkpoint: str) -> None:
    page.wait_for_timeout(UNCHANGED_ASSERT_WAIT_MS)
    actual = read_settings(page)
    if actual != expected:
        raise AssertionError(
            f"{checkpoint}: settings changed during the conservative "
            f"{UNCHANGED_ASSERT_WAIT_MS}ms observation; expected {expected!r}, "
            f"got {actual!r}"
        )


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
            # Install deterministic settings before the first navigation so the
            # state probes below never depend on settings left by an earlier run.
            context.add_init_script(
                script=f"""
(() => {{
    try {{
        localStorage.setItem(
            {json.dumps(STORAGE_KEY)}, {json.dumps(INITIAL_SETTINGS)}
        );
        localStorage.removeItem({json.dumps(LEGACY_KEY)});
    }} catch (_) {{}}
}})();
"""
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
                "e => { const r=e.getBoundingClientRect(); return "
                "{left:r.left,top:r.top,width:r.width,height:r.height}; }"
            )
            if abs(rect["width"] - 844) > 1 or abs(rect["height"] - 390) > 1:
                raise AssertionError(f"canvas did not fit mobile viewport: {rect}")
            summary["canvas"] = rect

            wait_for_exact_settings(page, INITIAL_SETTINGS, "deterministic boot")

            def settings_tap(fx: float, fy: float) -> None:
                page.touchscreen.tap(
                    rect["left"] + fx * rect["width"],
                    rect["top"] + fy * rect["height"],
                )
                page.wait_for_timeout(170)

            def settings_volume_minus(expected: str, checkpoint: str) -> None:
                """Open Settings, tap Volume -, close Back, and require the change."""
                # These are the mobile row coordinates proven by
                # browser_settings_scenarios.py: opener, Volume left, Back.
                settings_tap(0.88, 0.12)
                settings_tap(0.20, 0.26)
                settings_tap(0.50, 0.77)
                # Assert only after Back so a failed checkpoint cannot leave a
                # successfully opened modal covering the game.
                wait_for_exact_settings(page, expected, checkpoint)

            def settings_volume_minus_must_not_change(
                expected: str, checkpoint: str
            ) -> None:
                """Try the same touch path where Settings must be inaccessible."""
                settings_tap(0.88, 0.12)
                settings_tap(0.20, 0.26)
                settings_tap(0.50, 0.77)
                # Two seconds allows delayed frame/event processing to expose an
                # accidental modal opening or settings mutation.
                assert_settings_unchanged(page, expected, checkpoint)

            shot("00_mobile_menu.png")

            # Any touch starts Menu, activates touch controls, and unlocks audio.
            page.touchscreen.tap(422, 195)
            page.wait_for_timeout(3_800)
            settings_volume_minus_must_not_change(
                "v2:100:0:0:",
                "after touch start: Playing must reject Settings adjustment",
            )
            shot("01_touch_hud.png")

            # The first eligible touch owns direction regardless of position:
            # begin on the right, drag up-left, then add action on the left.
            cdp = context.new_cdp_session(page)
            drive = {"x": 735, "y": 325, "radiusX": 8, "radiusY": 8, "force": 1, "id": 1}
            cdp.send(
                "Input.dispatchTouchEvent",
                {"type": "touchStart", "touchPoints": [drive]},
            )
            page.wait_for_timeout(600)
            drive["x"] = 690
            drive["y"] = 265
            cdp.send(
                "Input.dispatchTouchEvent",
                {"type": "touchMove", "touchPoints": [drive]},
            )
            page.wait_for_timeout(1_000)
            action = {"x": 100, "y": 325, "radiusX": 8, "radiusY": 8, "force": 1, "id": 2}
            cdp.send(
                "Input.dispatchTouchEvent",
                {"type": "touchStart", "touchPoints": [drive, action]},
            )
            page.wait_for_timeout(800)
            cdp.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})
            page.wait_for_timeout(400)
            shot("02_after_multitouch.png")

            # Pause zone; Settings is touch-accessible only because state is
            # Paused. The helper closes the modal through its Back row.
            page.touchscreen.tap(422, 25)
            page.wait_for_timeout(500)
            shot("03_paused.png")
            settings_volume_minus(
                "v2:90:0:0:",
                "first pause: Paused must allow Settings volume 100 -> 90",
            )

            # Left third resumes. Playing must make the identical Settings touch
            # sequence inert, preserving the exact schema.
            page.touchscreen.tap(100, 195)
            page.wait_for_timeout(500)
            settings_volume_minus_must_not_change(
                "v2:90:0:0:",
                "after resume: Playing must reject Settings adjustment",
            )
            shot("04_resumed.png")

            # Middle third performs restart through Menu and a fresh countdown.
            page.touchscreen.tap(422, 25)
            page.wait_for_timeout(350)
            page.touchscreen.tap(422, 195)
            page.wait_for_timeout(800)
            shot("05_touch_restart_countdown.png")
            page.wait_for_timeout(3_200)
            settings_volume_minus_must_not_change(
                "v2:90:0:0:",
                "after restart countdown: Playing must reject Settings adjustment",
            )

            # Pause once more, adjust while Paused, and close Back before the
            # right-third action returns to Menu. Menu then allows one final
            # touch-only adjustment, whose modal is also closed through Back.
            page.touchscreen.tap(422, 25)
            page.wait_for_timeout(350)
            settings_volume_minus(
                "v2:80:0:0:",
                "second pause: Paused must allow Settings volume 90 -> 80",
            )
            page.touchscreen.tap(760, 195)
            page.wait_for_timeout(700)
            settings_volume_minus(
                "v2:70:0:0:",
                "right-third return: Menu must allow Settings volume 80 -> 70",
            )
            shot("06_touch_menu.png")

            # Distinguish Menu from Paused rather than treating both as the
            # same Settings-accessible state. In Menu this touch starts a round;
            # if the preceding right-third transition failed and state remained
            # Paused, the same touch merely returns to Menu. Settings must be
            # inaccessible after the fresh countdown only in the correct path.
            page.touchscreen.tap(760, 195)
            page.wait_for_timeout(3_800)
            settings_volume_minus_must_not_change(
                "v2:70:0:0:",
                "menu proof: a touch must start Playing, not leave Paused/Menu",
            )
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
