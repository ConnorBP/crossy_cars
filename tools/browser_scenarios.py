#!/usr/bin/env python3
"""Repeatable browser QA scenario for Roady Car.

Requires Playwright for Python and, by default, locally installed Chrome::

    python -m pip install playwright
    python tools/browser_scenarios.py --url http://localhost:8080

Pass ``--browser-channel chromium`` (or set ``BROWSER_CHANNEL=chromium``) to
use Playwright's bundled Chromium, as CI does.

The scenario deliberately interacts through keyboard input, timing, the loading
DOM element, and localStorage. Game UI is rendered into a canvas, so it does not
use fragile selectors for game text.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import traceback
from pathlib import Path
from typing import Any, Callable


DEFAULT_URL = "http://localhost:8080"
DEFAULT_OUT_DIR = "tools/scenarios"
BOOT_TIMEOUT_MS = 120_000
# M toggles the shared Settings resource, which SettingsPlugin persists as the
# v1 schema string "v1:<volume>:<muted>:<reduced_motion>" (e.g. "v1:100:1:0")
# under roady_car_settings. The legacy roady_car_audio_muted key is migrated
# only when the schema is absent, so we wipe both to start deterministically.
SETTINGS_STORAGE_KEY = "roady_car_settings"
LEGACY_MUTE_STORAGE_KEY = "roady_car_audio_muted"
DEFAULT_SCHEMA = "v1:100:0:0"
MUTED_SCHEMA = "v1:100:1:0"
# A fresh browser context has fresh sessionStorage. This marker makes the
# initial localStorage wipe one-shot, so the later reload genuinely verifies
# persistence instead of being reset by the init script.
QA_SESSION_MARKER = "__roady_car_browser_qa_initialized"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run the repeatable Roady Car Playwright browser scenario."
    )
    parser.add_argument(
        "--url",
        default=DEFAULT_URL,
        help=f"Game URL (default: {DEFAULT_URL})",
    )
    parser.add_argument(
        "--out-dir",
        default=DEFAULT_OUT_DIR,
        help=f"Screenshot output directory (default: {DEFAULT_OUT_DIR})",
    )
    parser.add_argument(
        "--headed",
        action="store_true",
        help="Show the browser instead of running headless.",
    )
    parser.add_argument(
        "--browser-channel",
        default=os.environ.get("BROWSER_CHANNEL", "chrome"),
        help=(
            "Playwright Chromium channel (default: chrome, or BROWSER_CHANNEL). "
            "Use 'chromium' for Playwright's bundled browser."
        ),
    )
    return parser.parse_args()


def resolve_browser_channel(value: str) -> str | None:
    value = value.strip()
    if value.lower() in {"", "chromium", "playwright", "bundled"}:
        return None
    return value


def elapsed_ms(started_at: float) -> int:
    return round((time.monotonic() - started_at) * 1000)


def run_step(
    summary: dict[str, Any], name: str, operation: Callable[[], Any]
) -> Any:
    """Run and time one named operation while preserving its exception."""
    started_at = time.monotonic()
    step: dict[str, Any] = {"name": name, "status": "running"}
    summary["steps"].append(step)
    try:
        result = operation()
    except Exception as exc:
        step.update(
            status="failed",
            duration_ms=elapsed_ms(started_at),
            error=f"{type(exc).__name__}: {exc}",
        )
        raise
    step.update(status="passed", duration_ms=elapsed_ms(started_at))
    return result


def assert_condition(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def wait_for_boot(page: Any, url: str, *, navigate: bool = True) -> dict[str, Any]:
    """Wait for a canvas and for the retained #loading element to be hidden."""
    if navigate:
        page.goto(url, wait_until="domcontentloaded", timeout=BOOT_TIMEOUT_MS)

    loading = page.locator("#loading")
    loading.wait_for(state="attached", timeout=BOOT_TIMEOUT_MS)
    assert_condition(loading.count() == 1, "expected exactly one #loading element")

    page.locator("canvas").first.wait_for(state="attached", timeout=BOOT_TIMEOUT_MS)
    loading.wait_for(state="hidden", timeout=BOOT_TIMEOUT_MS)

    state = page.evaluate(
        """() => {
            const loading = document.querySelector('#loading');
            const canvas = document.querySelector('canvas');
            const style = loading ? getComputedStyle(loading) : null;
            const rect = canvas ? canvas.getBoundingClientRect() : null;
            return {
                loadingExists: Boolean(loading),
                loadingHiddenAttribute: loading ? loading.hidden : false,
                loadingDisplay: style ? style.display : null,
                canvasExists: Boolean(canvas),
                canvasWidth: rect ? rect.width : 0,
                canvasHeight: rect ? rect.height : 0,
            };
        }"""
    )
    assert_condition(state["loadingExists"], "#loading disappeared after boot")
    assert_condition(
        state["loadingHiddenAttribute"] and state["loadingDisplay"] == "none",
        "#loading exists but is not hidden after boot",
    )
    assert_condition(state["canvasExists"], "game canvas does not exist after boot")
    assert_condition(
        state["canvasWidth"] > 0 and state["canvasHeight"] > 0,
        "game canvas has no visible dimensions",
    )
    # Give the first Bevy update/render frames time to establish the menu.
    page.wait_for_timeout(500)
    return state


def hold_keys(page: Any, keys: list[str], duration_ms: int) -> None:
    """Hold keys concurrently and always attempt to release every pressed key."""
    pressed: list[str] = []
    try:
        for key in keys:
            page.keyboard.down(key)
            pressed.append(key)
        page.wait_for_timeout(duration_ms)
    finally:
        for key in reversed(pressed):
            try:
                page.keyboard.up(key)
            except Exception:
                # Preserve the original action failure (for example, a crashed page).
                pass


def run_scenario(args: argparse.Namespace, summary: dict[str, Any]) -> None:
    out_dir = Path(args.out_dir).expanduser().resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    summary["out_dir"] = str(out_dir)

    # Import here so a missing dependency can still produce the JSON report
    # (and after mkdir so the requested artifact directory always exists).
    from playwright.sync_api import sync_playwright

    browser = None
    context = None
    cleanup_errors: list[str] = summary["cleanup_errors"]

    with sync_playwright() as playwright:
        try:
            # Local runs exercise installed Chrome by default; CI can select
            # Playwright's installed Chromium without changing local behavior.
            browser_channel = resolve_browser_channel(args.browser_channel)
            launch_options: dict[str, Any] = {"headless": not args.headed}
            if browser_channel is not None:
                launch_options["channel"] = browser_channel
            browser = playwright.chromium.launch(**launch_options)
            context = browser.new_context(
                viewport={"width": 1440, "height": 900},
                device_scale_factor=1,
            )

            # Ensure M always performs false -> true by starting from the
            # deterministic fresh schema, including when a previous QA run left
            # the origin with persisted settings. Wiping both the v1 schema and
            # the legacy mute bit guarantees the app boots unmuted at v1:100:0:0.
            # sessionStorage makes this initialization one-shot, so the later
            # reload genuinely verifies persistence instead of being reset.
            context.add_init_script(
                script=f"""
                    (() => {{
                        try {{
                            if (sessionStorage.getItem({json.dumps(QA_SESSION_MARKER)}) !== '1') {{
                                localStorage.removeItem({json.dumps(SETTINGS_STORAGE_KEY)});
                                localStorage.removeItem({json.dumps(LEGACY_MUTE_STORAGE_KEY)});
                                sessionStorage.setItem({json.dumps(QA_SESSION_MARKER)}, '1');
                            }}
                        }} catch (_) {{}}
                    }})();
                """
            )

            page = context.new_page()
            page.set_default_timeout(30_000)
            page.set_default_navigation_timeout(BOOT_TIMEOUT_MS)
            scenario_started = time.monotonic()

            def timestamp() -> int:
                return elapsed_ms(scenario_started)

            def on_console(message: Any) -> None:
                if message.type == "error":
                    try:
                        location = message.location
                    except Exception:
                        location = None
                    summary["console_errors"].append(
                        {
                            "at_ms": timestamp(),
                            "text": message.text,
                            "location": location,
                        }
                    )

            def on_page_error(error: Any) -> None:
                summary["page_errors"].append(
                    {
                        "at_ms": timestamp(),
                        "message": str(error),
                        "stack": getattr(error, "stack", None),
                    }
                )

            def on_request_failed(request: Any) -> None:
                summary["network_failures"].append(
                    {
                        "at_ms": timestamp(),
                        "method": request.method,
                        "url": request.url,
                        "failure": request.failure,
                    }
                )

            def on_response(response: Any) -> None:
                if response.status >= 400:
                    summary["http_errors"].append(
                        {
                            "at_ms": timestamp(),
                            "status": response.status,
                            "url": response.url,
                        }
                    )

            page.on("console", on_console)
            page.on("pageerror", on_page_error)
            page.on("requestfailed", on_request_failed)
            page.on("response", on_response)

            def screenshot(filename: str) -> None:
                path = out_dir / filename
                page.screenshot(path=str(path), full_page=True)
                summary["screenshots"].append(str(path))

            boot_state = run_step(
                summary, "load_and_wait_for_boot", lambda: wait_for_boot(page, args.url)
            )
            summary["boot"] = boot_state
            run_step(summary, "capture_boot", lambda: screenshot("00_boot_menu.png"))

            # Enter starts a fresh round. Samples are based on elapsed wall time,
            # not canvas text selectors: 3/2/1 transition near 0/1/2 seconds and
            # GO appears near 3 seconds for its short punch animation.
            countdown_started = time.monotonic()
            run_step(summary, "start_round_with_enter", lambda: page.keyboard.press("Enter"))

            def capture_at(target_seconds: float, filename: str) -> None:
                remaining_ms = round(
                    max(0.0, target_seconds - (time.monotonic() - countdown_started))
                    * 1000
                )
                if remaining_ms:
                    page.wait_for_timeout(remaining_ms)
                screenshot(filename)

            run_step(
                summary,
                "capture_initial_countdown_3",
                lambda: capture_at(0.25, "01_countdown_3.png"),
            )
            run_step(
                summary,
                "capture_initial_countdown_2",
                lambda: capture_at(1.15, "02_countdown_2.png"),
            )
            run_step(
                summary,
                "capture_initial_countdown_1",
                lambda: capture_at(2.15, "03_countdown_1.png"),
            )
            run_step(
                summary,
                "wait_for_and_capture_go",
                lambda: capture_at(3.05, "04_countdown_go.png"),
            )
            # GO releases input at ~3s; wait until its punch overlay has completed.
            run_step(
                summary,
                "finish_initial_countdown",
                lambda: page.wait_for_timeout(
                    round(
                        max(0.0, 3.40 - (time.monotonic() - countdown_started))
                        * 1000
                    )
                ),
            )

            run_step(summary, "drive_w_12_seconds", lambda: hold_keys(page, ["w"], 12_000))
            run_step(summary, "capture_after_w", lambda: screenshot("05_after_w_12s.png"))
            run_step(
                summary,
                "drive_w_and_a_8_seconds",
                lambda: hold_keys(page, ["w", "a"], 8_000),
            )
            run_step(
                summary,
                "capture_after_w_and_a",
                lambda: screenshot("06_after_wa_8s.png"),
            )

            def pause_and_capture() -> None:
                page.keyboard.press("Escape")
                page.wait_for_timeout(300)
                screenshot("07_paused.png")

            run_step(summary, "pause_and_capture", pause_and_capture)
            run_step(
                summary,
                "resume_from_pause",
                lambda: (page.keyboard.press("Escape"), page.wait_for_timeout(300)),
            )

            def pause_restart_and_capture(iteration: int) -> None:
                page.keyboard.press("Escape")
                page.wait_for_timeout(250)
                page.keyboard.press("r")
                # R routes Paused -> Menu -> fresh Playing. At 450ms the fresh
                # countdown is visibly active, without depending on canvas text.
                page.wait_for_timeout(450)
                screenshot(f"{7 + iteration:02d}_restart_{iteration}_countdown.png")
                # Allow 3/2/1/GO and the GO punch to complete before the next input.
                page.wait_for_timeout(3_050)

            run_step(
                summary,
                "pause_r_fresh_restart_1",
                lambda: pause_restart_and_capture(1),
            )
            run_step(
                summary,
                "pause_r_fresh_restart_2",
                lambda: pause_restart_and_capture(2),
            )

            def toggle_mute() -> str:
                # The app may have already persisted the default schema on its
                # first update; either null or the fresh default is unmuted.
                before = page.evaluate(
                    f"localStorage.getItem({json.dumps(SETTINGS_STORAGE_KEY)})"
                )
                assert_condition(
                    before in (None, DEFAULT_SCHEMA),
                    f"mute precondition should be {DEFAULT_SCHEMA!r} or absent, got {before!r}",
                )
                page.keyboard.press("m")
                # M flips Settings.muted; SettingsPlugin persists the full v1
                # schema. Only the muted bit changes (volume/reduced-motion stay).
                page.wait_for_function(
                    f"localStorage.getItem({json.dumps(SETTINGS_STORAGE_KEY)}) === {json.dumps(MUTED_SCHEMA)}"
                )
                after = page.evaluate(
                    f"localStorage.getItem({json.dumps(SETTINGS_STORAGE_KEY)})"
                )
                assert_condition(
                    after == MUTED_SCHEMA,
                    f"M did not persist muted schema {MUTED_SCHEMA!r}, got {after!r}",
                )
                return after

            summary["mute_after_toggle"] = run_step(
                summary, "toggle_mute_and_assert_storage", toggle_mute
            )

            def reload_and_check_persistence() -> str:
                # Waiting for DOM/canvas alone is insufficient: Trunk's module
                # can create the replacement document while the large WASM
                # response is still streaming. Closing Chrome at that point
                # fabricates a pageerror/network failure. Track the reload's
                # WASM response explicitly and wait for its body to finish.
                with page.expect_response(
                    lambda response: response.url.endswith("_bg.wasm"),
                    timeout=BOOT_TIMEOUT_MS,
                ) as wasm_response_info:
                    page.reload(wait_until="domcontentloaded", timeout=BOOT_TIMEOUT_MS)
                # `expect_response` proves the fresh document requested WASM;
                # `wait_for_boot` proves that response compiled far enough to
                # create a new Bevy canvas. Finish with network-idle instead of
                # Response.finished(), whose Playwright wrapper can leave an
                # internal future pending during browser shutdown.
                _wasm_response = wasm_response_info.value
                wait_for_boot(page, args.url, navigate=False)
                page.wait_for_load_state("networkidle", timeout=BOOT_TIMEOUT_MS)
                page.wait_for_timeout(500)
                persisted = page.evaluate(
                    f"localStorage.getItem({json.dumps(SETTINGS_STORAGE_KEY)})"
                )
                assert_condition(
                    persisted == MUTED_SCHEMA,
                    f"mute preference did not survive reload: expected {MUTED_SCHEMA!r}, got {persisted!r}",
                )
                return persisted

            summary["mute_after_reload"] = run_step(
                summary, "reload_and_assert_mute_persists", reload_and_check_persistence
            )
            run_step(
                summary,
                "capture_final",
                lambda: screenshot("10_final_after_reload.png"),
            )

            # Let asynchronous console/page errors queued by the final frame arrive.
            page.wait_for_timeout(300)
            assert_condition(
                not summary["console_errors"],
                f"observed {len(summary['console_errors'])} console.error message(s)",
            )
            assert_condition(
                not summary["page_errors"],
                f"observed {len(summary['page_errors'])} pageerror event(s)",
            )
            assert_condition(
                not summary["network_failures"],
                f"observed {len(summary['network_failures'])} failed request(s)",
            )
            assert_condition(
                not summary["http_errors"],
                f"observed {len(summary['http_errors'])} HTTP error response(s)",
            )
        finally:
            if context is not None:
                try:
                    context.close()
                except Exception as exc:
                    cleanup_errors.append(f"context.close: {type(exc).__name__}: {exc}")
            if browser is not None:
                try:
                    browser.close()
                except Exception as exc:
                    cleanup_errors.append(f"browser.close: {type(exc).__name__}: {exc}")


def main() -> int:
    args = parse_args()
    overall_started = time.monotonic()
    summary: dict[str, Any] = {
        "scenario": "wave_f_repeatable_browser_qa",
        "status": "running",
        "url": args.url,
        "browser": {
            "engine": "chromium",
            "channel": resolve_browser_channel(args.browser_channel)
            or "playwright-chromium",
            "headed": args.headed,
        },
        "out_dir": str(Path(args.out_dir).expanduser()),
        "steps": [],
        "screenshots": [],
        "console_errors": [],
        "page_errors": [],
        "network_failures": [],
        "http_errors": [],
        "cleanup_errors": [],
    }

    exit_code = 0
    try:
        run_scenario(args, summary)
        # Recheck after browser/context shutdown so even a late event captured
        # while closing cannot accidentally leave the run marked as passed.
        assert_condition(
            not summary["console_errors"],
            f"observed {len(summary['console_errors'])} console.error message(s)",
        )
        assert_condition(
            not summary["page_errors"],
            f"observed {len(summary['page_errors'])} pageerror event(s)",
        )
        assert_condition(
            not summary["network_failures"],
            f"observed {len(summary['network_failures'])} failed request(s)",
        )
        assert_condition(
            not summary["http_errors"],
            f"observed {len(summary['http_errors'])} HTTP error response(s)",
        )
        if summary["cleanup_errors"]:
            raise RuntimeError("browser cleanup reported errors")
        summary["status"] = "passed"
    except Exception as exc:
        exit_code = 1
        summary["status"] = "failed"
        summary["failure"] = {
            "type": type(exc).__name__,
            "message": str(exc),
            "traceback": traceback.format_exc(),
        }
    finally:
        summary["duration_ms"] = elapsed_ms(overall_started)
        print(json.dumps(summary, indent=2, sort_keys=True, default=str), flush=True)

    return exit_code


if __name__ == "__main__":
    sys.exit(main())
