#!/usr/bin/env python3
"""Focused Playwright QA for the responsive Roady Car product menu.

The game UI is canvas-rendered, so this scenario uses audited viewport-relative
coordinates and behavioral/pixel oracles rather than DOM text selectors.  It
covers mouse at 1440x900 and touch at 844x390 with Ranked disabled, verifies a
stable menu over consecutive rendered frames, selects Casual Right Of Way,
cycles the five Casual conditions, presses the explicit DRIVE target, and
proves Playing by showing that the otherwise-working Settings storage path is
inaccessible.

Requires Playwright and either local Chrome (the default) or Playwright's
bundled Chromium::

    python tools/browser_menu_scenarios.py --url http://localhost:8080
    python tools/browser_menu_scenarios.py --browser-channel chromium

Set ``ROADY_SCREENSHOTS=failure`` to omit checkpoint screenshots.  A failing
run always attempts to write ``failure.png`` and exits nonzero.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import time
import traceback
from pathlib import Path
from typing import Any, Callable

try:
    from browser_v3_fixture import install_disabled_capability
except ImportError:
    from .browser_v3_fixture import install_disabled_capability

try:  # Direct execution puts tools/ on sys.path.
    from browser_scenarios import (
        FailureScreenshotRecorder,
        discard_pre_cleanup_screenshot,
        ignorable_request_failure,
        is_v3_write,
        promote_pre_cleanup_screenshot,
    )
except ImportError:  # Package-style import used by helper tests.
    from .browser_scenarios import (
        FailureScreenshotRecorder,
        discard_pre_cleanup_screenshot,
        ignorable_request_failure,
        is_v3_write,
        promote_pre_cleanup_screenshot,
    )


DEFAULT_URL = "http://localhost:8080"
DEFAULT_OUT_DIR = "tools/scenarios/menu"
BOOT_TIMEOUT_MS = 120_000
DEFAULT_TIMEOUT_MS = 60_000
SETTINGS_KEY = "roady_car_settings"
LEGACY_SETTINGS_KEY = "roady_car_audio_muted"
# Reduced motion makes the menu-control clips deterministic without changing
# product focus, condition selection, pointer behavior, or game-state routing.
INITIAL_SETTINGS = "v2:100:0:1:"
CALIBRATED_SETTINGS = "v2:90:0:1:"
SCREENSHOT_POLICIES = {"all", "failure"}

DESKTOP = {
    "name": "desktop_mouse",
    "viewport": {"width": 1440, "height": 900},
    "touch": False,
    "stable_clip": {"x": 410, "y": 684, "width": 620, "height": 100},
    "condition_clip": {"x": 596, "y": 330, "width": 250, "height": 260},
    "casual_right_of_way": (877, 761),
    "previous": (270, 451),
    "next": (1170, 451),
    "drive": (720, 850),
    "settings": ((1361, 35), (400, 299), (720, 603)),
}

TOUCH = {
    "name": "touch_844x390",
    "viewport": {"width": 844, "height": 390},
    "touch": True,
    "stable_clip": {"x": 172, "y": 256, "width": 500, "height": 70},
    "condition_clip": {"x": 321, "y": 158, "width": 205, "height": 92},
    "casual_right_of_way": (548, 309),
    "previous": (122, 205),
    "next": (721, 205),
    "drive": (422, 348),
    "settings": ((777, 27), (169, 91), (422, 275)),
}


def parse_screenshot_policy(value: str | None) -> str:
    policy = "all" if value is None else value
    if policy not in SCREENSHOT_POLICIES:
        raise ValueError(
            "ROADY_SCREENSHOTS must be either 'all' or 'failure' "
            f"(got {value!r})"
        )
    return policy


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run the Roady Car mouse/touch product-menu Playwright scenario."
    )
    parser.add_argument(
        "--url", default=DEFAULT_URL, help=f"Game URL (default: {DEFAULT_URL})"
    )
    parser.add_argument(
        "--out-dir",
        default=DEFAULT_OUT_DIR,
        help=f"Screenshot output directory (default: {DEFAULT_OUT_DIR})",
    )
    parser.add_argument(
        "--headed", action="store_true", help="Show the browser instead of running headless."
    )
    parser.add_argument(
        "--browser-channel",
        default=os.environ.get("BROWSER_CHANNEL", "chrome"),
        help=(
            "Playwright Chromium channel (default: chrome, or BROWSER_CHANNEL). "
            "Use 'chromium' for Playwright's bundled browser."
        ),
    )
    parsed = parser.parse_args()
    try:
        parsed.screenshot_policy = parse_screenshot_policy(
            os.environ.get("ROADY_SCREENSHOTS")
        )
    except ValueError as exc:
        parser.error(str(exc))
    return parsed


def resolve_browser_channel(value: str) -> str | None:
    value = value.strip()
    if value.lower() in {"", "chromium", "playwright", "bundled"}:
        return None
    return value


def elapsed_ms(started_at: float) -> int:
    return round((time.monotonic() - started_at) * 1000)


def assert_condition(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def run_step(
    section: dict[str, Any], name: str, operation: Callable[[], Any]
) -> Any:
    started = time.monotonic()
    entry: dict[str, Any] = {"name": name, "status": "running"}
    section["steps"].append(entry)
    try:
        result = operation()
    except Exception as exc:
        entry.update(
            status="failed",
            duration_ms=elapsed_ms(started),
            error=f"{type(exc).__name__}: {exc}",
        )
        raise
    entry.update(status="passed", duration_ms=elapsed_ms(started))
    return result


def initial_storage_script() -> str:
    return f"""
(() => {{
    try {{
        localStorage.setItem(
            {json.dumps(SETTINGS_KEY)}, {json.dumps(INITIAL_SETTINGS)}
        );
        localStorage.removeItem({json.dumps(LEGACY_SETTINGS_KEY)});
    }} catch (_) {{}}
}})();
"""


def read_settings(page: Any) -> str | None:
    return page.evaluate(f"localStorage.getItem({json.dumps(SETTINGS_KEY)})")


def wait_for_settings(page: Any, expected: str, checkpoint: str) -> None:
    try:
        page.wait_for_function(
            f"localStorage.getItem({json.dumps(SETTINGS_KEY)}) === {json.dumps(expected)}",
            timeout=DEFAULT_TIMEOUT_MS,
            polling=200,
        )
    except Exception as exc:  # Replace an opaque Playwright polling timeout.
        raise AssertionError(
            f"{checkpoint}: expected settings {expected!r}, got {read_settings(page)!r}"
        ) from exc
    actual = read_settings(page)
    assert_condition(
        actual == expected,
        f"{checkpoint}: expected settings {expected!r}, got {actual!r}",
    )


def wait_for_boot(page: Any, url: str, viewport: dict[str, int]) -> dict[str, Any]:
    page.goto(url, wait_until="domcontentloaded", timeout=BOOT_TIMEOUT_MS)
    loading = page.locator("#loading")
    loading.wait_for(state="attached", timeout=BOOT_TIMEOUT_MS)
    assert_condition(loading.count() == 1, "expected exactly one #loading element")
    canvas = page.locator("canvas").first
    canvas.wait_for(state="attached", timeout=BOOT_TIMEOUT_MS)
    loading.wait_for(state="hidden", timeout=BOOT_TIMEOUT_MS)
    # This signal is published only after two complete semantic Menu updates.
    page.wait_for_function(
        "document.documentElement.dataset.roadyReady === 'playable'",
        timeout=BOOT_TIMEOUT_MS,
    )
    state = page.evaluate(
        """() => {
            const loading = document.querySelector('#loading');
            const canvas = document.querySelector('canvas');
            const style = loading ? getComputedStyle(loading) : null;
            const rect = canvas ? canvas.getBoundingClientRect() : null;
            return {
                loadingExists: Boolean(loading),
                loadingHidden: Boolean(loading && loading.hidden),
                loadingDisplay: style ? style.display : null,
                ready: document.documentElement.dataset.roadyReady || null,
                canvasRect: rect ? {
                    left: rect.left, top: rect.top,
                    width: rect.width, height: rect.height
                } : null,
            };
        }"""
    )
    assert_condition(state["loadingExists"], "#loading disappeared after boot")
    assert_condition(
        state["loadingHidden"] and state["loadingDisplay"] == "none",
        "#loading exists but is not hidden after boot",
    )
    assert_condition(state["ready"] == "playable", "playable readiness was not published")
    rect = state["canvasRect"]
    assert_condition(rect is not None, "game canvas does not exist after boot")
    assert_condition(
        abs(rect["left"]) <= 1
        and abs(rect["top"]) <= 1
        and abs(rect["width"] - viewport["width"]) <= 1
        and abs(rect["height"] - viewport["height"]) <= 1,
        f"canvas does not fill {viewport['width']}x{viewport['height']}: {rect}",
    )
    page.wait_for_timeout(700)
    wait_for_settings(page, INITIAL_SETTINGS, "deterministic boot")
    return state


def frame_digest(page: Any, clip: dict[str, int]) -> str:
    # Synchronize to a browser frame before sampling the exact rendered pixels.
    page.evaluate(
        "() => new Promise(resolve => requestAnimationFrame(() => resolve()))"
    )
    return hashlib.sha256(page.screenshot(clip=clip)).hexdigest()


def assert_menu_stable(page: Any, clip: dict[str, int]) -> list[str]:
    digests = [frame_digest(page, clip) for _ in range(3)]
    assert_condition(
        len(set(digests)) == 1,
        "menu controls changed over three consecutive rendered frames: "
        f"{[value[:12] for value in digests]}",
    )
    return digests


def attach_error_listeners(
    page: Any, summary: dict[str, Any], view_name: str, started_at: float
) -> None:
    def timestamp() -> int:
        return elapsed_ms(started_at)

    def on_console(message: Any) -> None:
        if message.type == "error":
            try:
                location = message.location
            except Exception:
                location = None
            summary["console_errors"].append(
                {
                    "view": view_name,
                    "at_ms": timestamp(),
                    "text": message.text,
                    "location": location,
                }
            )

    def on_page_error(error: Any) -> None:
        summary["page_errors"].append(
            {
                "view": view_name,
                "at_ms": timestamp(),
                "message": str(error),
                "stack": getattr(error, "stack", None),
            }
        )

    def on_request_failed(request: Any) -> None:
        if ignorable_request_failure(request.method, request.url, request.failure):
            return
        summary["network_failures"].append(
            {
                "view": view_name,
                "at_ms": timestamp(),
                "method": request.method,
                "url": request.url,
                "failure": request.failure,
            }
        )

    def on_request(request: Any) -> None:
        if is_v3_write(request.method, request.url):
            summary["v3_write_requests"].append(
                {
                    "view": view_name,
                    "at_ms": timestamp(),
                    "method": request.method,
                    "url": request.url,
                }
            )

    def on_response(response: Any) -> None:
        if response.status >= 400:
            summary["http_errors"].append(
                {
                    "view": view_name,
                    "at_ms": timestamp(),
                    "status": response.status,
                    "url": response.url,
                }
            )

    page.on("console", on_console)
    page.on("pageerror", on_page_error)
    page.on("requestfailed", on_request_failed)
    page.on("request", on_request)
    page.on("response", on_response)


def assert_no_errors(summary: dict[str, Any]) -> None:
    for key in (
        "console_errors",
        "page_errors",
        "network_failures",
        "http_errors",
        "v3_write_requests",
        "cleanup_errors",
    ):
        assert_condition(not summary[key], f"{key}: {summary[key]}")


def run_view(
    page: Any,
    spec: dict[str, Any],
    options: argparse.Namespace,
    section: dict[str, Any],
    out_dir: Path,
) -> None:
    name = spec["name"]

    def shot(filename: str) -> None:
        if options.screenshot_policy == "failure":
            return
        path = out_dir / filename
        page.screenshot(path=str(path), full_page=True)
        section["screenshots"].append(str(path))

    def point(point_name: str) -> tuple[int, int]:
        return spec[point_name]

    def activate(point_name: str, settle_ms: int = 300) -> None:
        x, y = point(point_name)
        if spec["touch"]:
            page.touchscreen.tap(x, y)
        else:
            page.mouse.click(x, y)
        page.wait_for_timeout(settle_ms)

    def held_activate(point_name: str, hold_ms: int = 850) -> None:
        x, y = point(point_name)
        if spec["touch"]:
            cdp = page.context.new_cdp_session(page)
            touch = {"x": x, "y": y, "radiusX": 8, "radiusY": 8, "force": 1, "id": 91}
            cdp.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [touch]})
            page.wait_for_timeout(hold_ms)
            cdp.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})
            cdp.detach()
        else:
            page.mouse.move(x, y)
            page.mouse.down()
            page.wait_for_timeout(hold_ms)
            page.mouse.up()
        page.wait_for_timeout(300)

    section["boot"] = run_step(
        section,
        "wait_for_boot_and_loading_hidden",
        lambda: wait_for_boot(page, options.url, spec["viewport"]),
    )
    section["stable_frame_digests"] = run_step(
        section,
        "menu_stable_over_three_frames",
        lambda: assert_menu_stable(page, spec["stable_clip"]),
    )
    run_step(section, "capture_menu", lambda: shot(f"{name}_00_menu.png"))

    # Calibrate the Settings negative oracle while it is reachable in Menu.
    # Opener -> Volume left -> Back must persist exactly one 10-point change.
    def calibrate_settings_oracle() -> None:
        for coordinate in spec["settings"]:
            if spec["touch"]:
                page.touchscreen.tap(*coordinate)
            else:
                page.mouse.click(*coordinate)
            page.wait_for_timeout(180)
        wait_for_settings(page, CALIBRATED_SETTINGS, f"{name} Settings calibration")

    run_step(section, "calibrate_settings_storage_oracle", calibrate_settings_oracle)

    # Remove mouse-hover pixels from the before/after focus oracle.
    if not spec["touch"]:
        page.mouse.move(300, 100)
        page.wait_for_timeout(120)
    before_focus = frame_digest(page, spec["stable_clip"])
    activate("casual_right_of_way")
    if not spec["touch"]:
        page.mouse.move(300, 100)
        page.wait_for_timeout(120)
    after_focus = frame_digest(page, spec["stable_clip"])
    assert_condition(
        before_focus != after_focus,
        f"{name}: Casual Right Of Way click/tap did not change product focus pixels",
    )
    section["casual_right_of_way_selected"] = True
    run_step(
        section,
        "capture_casual_right_of_way",
        lambda: shot(f"{name}_01_casual_right_of_way.png"),
    )

    # Starting on Standard, five Next activations visit Rush Hour, Chicken
    # Frenzy, Stampede, Glass Cannon, and wrapped Standard: all five conditions
    # are reached through the arrow control and the wrap is explicit.
    initial_condition = frame_digest(page, spec["condition_clip"])
    condition_digests: list[str] = []

    def cycle_all_conditions() -> None:
        for index in range(1, 6):
            activate("next")
            digest = frame_digest(page, spec["condition_clip"])
            condition_digests.append(digest)
            shot(f"{name}_{index + 1:02d}_condition.png")
        assert_condition(
            len(set(condition_digests)) == 5,
            f"{name}: arrow cycle did not render five distinct conditions: "
            f"{[value[:12] for value in condition_digests]}",
        )
        assert_condition(
            condition_digests[-1] == initial_condition,
            f"{name}: five Next activations did not wrap to Standard",
        )

    run_step(section, "next_arrow_through_all_five_conditions", cycle_all_conditions)
    section["condition_digests"] = condition_digests

    # Regression: holding the arrow through menu rebuilds must still advance
    # exactly once, never shake/cycle continuously beneath a held pointer.
    def held_arrow_advances_once() -> None:
        held_activate("next")  # Standard -> Rush Hour exactly once
        after_hold = frame_digest(page, spec["condition_clip"])
        assert_condition(
            after_hold == condition_digests[0],
            f"{name}: held Next did not advance exactly once",
        )
        activate("previous")  # Rush Hour -> Standard
        assert_condition(
            frame_digest(page, spec["condition_clip"]) == initial_condition,
            f"{name}: held-arrow cleanup did not restore Standard",
        )

    run_step(section, "held_next_advances_exactly_once", held_arrow_advances_once)

    # Also exercise the other visible arrow and restore Standard before DRIVE.
    def previous_arrow_round_trip() -> None:
        activate("previous")  # Standard -> Glass Cannon
        glass = frame_digest(page, spec["condition_clip"])
        assert_condition(
            glass == condition_digests[-2],
            f"{name}: Previous did not wrap Standard to Glass Cannon",
        )
        activate("next")  # Glass Cannon -> Standard
        restored = frame_digest(page, spec["condition_clip"])
        assert_condition(restored == initial_condition, f"{name}: Standard was not restored")

    run_step(section, "previous_arrow_wrap_and_restore", previous_arrow_round_trip)

    run_step(section, "activate_explicit_drive", lambda: activate("drive", 100))
    # 3/2/1/GO completes at about 3.4s.  The Settings opener and its rows are
    # available in Menu and Paused but not Playing.  Replaying the calibrated
    # pointer path after the countdown must therefore leave storage unchanged.
    page.wait_for_timeout(3_700)

    def prove_playing_with_negative_settings_oracle() -> None:
        before = read_settings(page)
        assert_condition(
            before == CALIBRATED_SETTINGS,
            f"{name}: pre-oracle settings changed unexpectedly: {before!r}",
        )
        for coordinate in spec["settings"]:
            if spec["touch"]:
                page.touchscreen.tap(*coordinate)
            else:
                page.mouse.click(*coordinate)
            page.wait_for_timeout(180)
        # A conservative observation window catches delayed pointer/frame work.
        page.wait_for_timeout(1_500)
        after = read_settings(page)
        assert_condition(
            after == before,
            f"{name}: Settings remained accessible after DRIVE; "
            f"expected Playing storage {before!r}, got {after!r}",
        )
        section["playing_storage_oracle"] = {
            "calibrated_in_menu": CALIBRATED_SETTINGS,
            "after_drive": after,
            "unchanged": True,
        }

    run_step(
        section,
        "prove_playing_via_settings_storage_negative_oracle",
        prove_playing_with_negative_settings_oracle,
    )
    run_step(section, "capture_playing", lambda: shot(f"{name}_07_playing.png"))
    page.wait_for_timeout(300)  # Drain queued runtime/browser events.


def run_scenario(options: argparse.Namespace, summary: dict[str, Any]) -> None:
    out_dir = Path(options.out_dir).expanduser().resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    summary["out_dir"] = str(out_dir)

    # Checkpoint images are optional, but failure.png is mandatory best effort.
    recorder = FailureScreenshotRecorder("failure", out_dir, summary["screenshots"])
    from playwright.sync_api import sync_playwright

    browser = None
    active_context = None
    active_page = None
    playwright_instance = None

    def capture_failure() -> None:
        recorder.capture(active_page, active_context, browser)

    try:
        playwright_instance = sync_playwright().start()
        launch: dict[str, Any] = {"headless": not options.headed}
        selected_channel = resolve_browser_channel(options.browser_channel)
        if selected_channel is not None:
            launch["channel"] = selected_channel
        browser = playwright_instance.chromium.launch(**launch)
        scenario_started = time.monotonic()

        for spec in (DESKTOP, TOUCH):
            section = summary[spec["name"]]
            context = None
            page = None
            try:
                context = browser.new_context(
                    viewport=spec["viewport"],
                    has_touch=spec["touch"],
                    is_mobile=spec["touch"],
                    device_scale_factor=1,
                )
                active_context = context
                context.add_init_script(script=initial_storage_script())
                install_disabled_capability(context)
                page = context.new_page()
                active_page = page
                page.set_default_timeout(DEFAULT_TIMEOUT_MS)
                page.set_default_navigation_timeout(BOOT_TIMEOUT_MS)
                attach_error_listeners(page, summary, spec["name"], scenario_started)
                run_view(page, spec, options, section, out_dir)
                assert_no_errors(summary)
            except Exception:
                capture_failure()
                raise
            finally:
                primary_failure = sys.exc_info()[0] is not None
                if context is not None:
                    recorder.snapshot_before_cleanup(page, context, browser)
                    try:
                        context.close()
                    except Exception as exc:
                        summary["cleanup_errors"].append(
                            f"{spec['name']} context.close: {type(exc).__name__}: {exc}"
                        )
                        capture_failure()
                        if not primary_failure:
                            raise

        # Closing is part of the strict error surface; late events must not turn
        # into a successful report merely because the pages have gone away.
        browser_failure: tuple[Exception, Any] | None = None
        try:
            browser.close()
            browser = None
        except Exception as exc:
            summary["cleanup_errors"].append(
                f"browser.close: {type(exc).__name__}: {exc}"
            )
            capture_failure()
            browser_failure = (exc, exc.__traceback__)
        try:
            playwright_instance.stop()
            playwright_instance = None
        except Exception as exc:
            summary["cleanup_errors"].append(
                f"playwright.stop: {type(exc).__name__}: {exc}"
            )
            capture_failure()
            if browser_failure is None:
                browser_failure = (exc, exc.__traceback__)
        if browser_failure is not None:
            exc, tb = browser_failure
            raise exc.with_traceback(tb)
        assert_no_errors(summary)
    except Exception:
        recorder.snapshot_before_cleanup(active_page, active_context, browser)
        capture_failure()
        raise
    finally:
        # Best effort for failures before the normal close block.
        if browser is not None:
            try:
                browser.close()
            except Exception as exc:
                summary["cleanup_errors"].append(
                    f"emergency browser.close: {type(exc).__name__}: {exc}"
                )
                capture_failure()
        if playwright_instance is not None:
            try:
                playwright_instance.stop()
            except Exception as exc:
                summary["cleanup_errors"].append(
                    f"emergency playwright.stop: {type(exc).__name__}: {exc}"
                )
                capture_failure()


def main() -> int:
    options = parse_args()
    started = time.monotonic()
    summary: dict[str, Any] = {
        "scenario": "roady_car_product_menu",
        "status": "running",
        "url": options.url,
        "browser": {
            "engine": "chromium",
            "channel": resolve_browser_channel(options.browser_channel)
            or "playwright-chromium",
            "headed": options.headed,
        },
        "screenshot_policy": options.screenshot_policy,
        "screenshots": [],
        "desktop_mouse": {"steps": [], "screenshots": []},
        "touch_844x390": {"steps": [], "screenshots": []},
        "console_errors": [],
        "page_errors": [],
        "network_failures": [],
        "http_errors": [],
        "v3_write_requests": [],
        "cleanup_errors": [],
    }
    out_dir = Path(options.out_dir).expanduser().resolve()

    exit_code = 0
    try:
        run_scenario(options, summary)
        assert_no_errors(summary)
        summary["status"] = "passed"
        discard_pre_cleanup_screenshot("failure", out_dir)
    except Exception as exc:
        exit_code = 1
        promote_pre_cleanup_screenshot("failure", out_dir, summary["screenshots"])
        summary["status"] = "failed"
        summary["failure"] = {
            "type": type(exc).__name__,
            "message": str(exc),
            "traceback": traceback.format_exc(),
        }
    finally:
        summary["duration_ms"] = elapsed_ms(started)
        print(json.dumps(summary, indent=2, sort_keys=True, default=str), flush=True)

    return exit_code


if __name__ == "__main__":
    sys.exit(main())
