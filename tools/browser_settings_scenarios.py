#!/usr/bin/env python3
"""Focused Playwright QA scenario for the Roady Car settings overlay.

The game renders all UI (menu, pause screen, settings modal) into a canvas, so
this scenario never relies on DOM text selectors. Modal/state is inferred from:

  * exact ``localStorage`` v1 schema assertions after every adjustment
    (``roady_car_settings == "v2:<volume>:<muted>:<reduced_motion>:<leaderboard_initials>"``);
  * continued settings input -- an adjustment only takes effect while the modal
    is open in Menu/Paused, so a storage change proves the modal was open and
    the underlying state was preserved (no transition fired);
  * timed transitions -- the 3/2/1/GO countdown (~3.4s) marks Playing, and
    behavioral probes (Esc to pause/resume, Q to quit, Enter to start) plus
    "can the modal still open?" discriminate Menu vs Paused vs Playing.

Coverage:

  desktop (keyboard):
    - deterministic fresh schema + absent legacy key
    - menu open (O) and adjust Volume/Mute/ReducedMotion via arrows + Enter/Space
    - modal isolation: Enter/Space (menu start keys) are swallowed while open
    - close via Esc, Back row (Enter), and O, each confirmed by reopening
    - reload persistence of the exact v1 schema + app-loaded values
    - pause open/close/isolation (R/Q swallowed) with state discrimination

  mobile (touch):
    - deterministic fresh schema
    - touch opener (top-right) opens; menu's pending Playing transition canceled
    - tap each row's left/right/center: Volume -, Mute/ReducedMotion on/off/toggle
    - Back row closes; reopen confirms isolation and exact v1 schema
    - pause flow: touch start + pause, touch-open/adjust, Back, verify stays Paused

Strict failure on console errors, page errors, network failures, and HTTP >=400.

Requires Playwright for Python and a local Chrome (or ``--browser-channel
chromium`` for Playwright's bundled browser, as CI does)::

    python -m pip install playwright
    python tools/browser_settings_scenarios.py --url http://localhost:8080
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

try:  # Direct script execution puts tools/ on sys.path.
    from browser_scenarios import (
        FailureScreenshotRecorder,
        discard_pre_cleanup_screenshot,
        promote_pre_cleanup_screenshot,
    )
except ImportError:  # Package-style imports used by helper self-tests.
    from .browser_scenarios import (
        FailureScreenshotRecorder,
        discard_pre_cleanup_screenshot,
        promote_pre_cleanup_screenshot,
    )

DEFAULT_URL = "http://localhost:8080"
DEFAULT_OUT_DIR = "tools/scenarios/settings"
BOOT_TIMEOUT_MS = 120_000
STORAGE_ASSERT_TIMEOUT_MS = 60_000
STORAGE_POLL_INTERVAL_MS = 250
STORAGE_KEY = "roady_car_settings"
LEGACY_KEY = "roady_car_audio_muted"
DEFAULT_SCHEMA = "v2:100:0:0:"
# A fresh browser context has fresh sessionStorage. This marker makes the
# initial localStorage wipe one-shot, so later reloads genuinely verify
# persistence instead of being reset by the init script.
QA_MARKER = "__roady_car_settings_qa_fresh"
DESKTOP_VIEWPORT = {"width": 1440, "height": 900}
MOBILE_VIEWPORT = {"width": 844, "height": 390}
SCREENSHOT_POLICIES = {"all", "failure"}


def parse_screenshot_policy(value: str | None) -> str:
    """Parse ROADY_SCREENSHOTS without depending on Playwright or argparse."""
    policy = "all" if value is None else value
    if policy not in SCREENSHOT_POLICIES:
        raise ValueError(
            "ROADY_SCREENSHOTS must be either 'all' or 'failure' "
            f"(got {value!r})"
        )
    return policy


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run the focused Roady Car settings Playwright QA scenario."
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


def fresh_init_script() -> str:
    return f"""
(() => {{
    try {{
        if (sessionStorage.getItem({json.dumps(QA_MARKER)}) !== '1') {{
            localStorage.removeItem({json.dumps(STORAGE_KEY)});
            localStorage.removeItem({json.dumps(LEGACY_KEY)});
            sessionStorage.setItem({json.dumps(QA_MARKER)}, '1');
        }}
    }} catch (_) {{}}
}})();
"""


def read_storage(page: Any) -> str | None:
    return page.evaluate(f"localStorage.getItem({json.dumps(STORAGE_KEY)})")


def assert_storage(
    page: Any, expected: str, *, timeout: int = STORAGE_ASSERT_TIMEOUT_MS
) -> None:
    """Poll localStorage until it equals the exact v1 schema string."""
    page.wait_for_function(
        f"localStorage.getItem({json.dumps(STORAGE_KEY)}) === {json.dumps(expected)}",
        timeout=timeout,
        polling=STORAGE_POLL_INTERVAL_MS,
    )
    actual = read_storage(page)
    assert_condition(
        actual == expected,
        f"storage: expected {expected!r}, got {actual!r}",
    )


def assert_storage_unchanged(page: Any, expected: str, *, wait_ms: int = 350) -> None:
    """Negative assertion: storage must NOT have changed (modal stayed closed)."""
    page.wait_for_timeout(wait_ms)
    actual = read_storage(page)
    assert_condition(
        actual == expected,
        f"storage changed unexpectedly: expected {expected!r}, got {actual!r}",
    )


def wait_for_boot(page: Any, url: str, *, navigate: bool = True) -> dict[str, Any]:
    """Wait for the retained #loading element to hide and a canvas to appear."""
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
                canvasRect: rect
                    ? {left: rect.left, top: rect.top, width: rect.width, height: rect.height}
                    : null,
            };
        }"""
    )
    assert_condition(state["loadingExists"], "#loading disappeared after boot")
    assert_condition(
        state["loadingHiddenAttribute"] and state["loadingDisplay"] == "none",
        "#loading exists but is not hidden after boot",
    )
    assert_condition(state["canvasExists"], "game canvas does not exist after boot")
    rect = state["canvasRect"]
    assert_condition(
        rect and rect["width"] > 0 and rect["height"] > 0,
        "game canvas has no visible dimensions",
    )
    page.wait_for_timeout(500)
    return state


def reload_preserving_wasm(page: Any, url: str) -> None:
    """Reload, waiting for the fresh WASM response to compile into a new canvas."""
    with page.expect_response(
        lambda response: response.url.endswith("_bg.wasm"),
        timeout=BOOT_TIMEOUT_MS,
    ) as wasm_info:
        page.reload(wait_until="domcontentloaded", timeout=BOOT_TIMEOUT_MS)
    _wasm = wasm_info.value
    wait_for_boot(page, url, navigate=False)
    page.wait_for_load_state("networkidle", timeout=BOOT_TIMEOUT_MS)
    page.wait_for_timeout(400)


def attach_error_listeners(page: Any, summary: dict[str, Any], started_at: float) -> None:
    def ts() -> int:
        return elapsed_ms(started_at)

    def on_console(message: Any) -> None:
        if message.type == "error":
            try:
                location = message.location
            except Exception:
                location = None
            summary["console_errors"].append(
                {"at_ms": ts(), "text": message.text, "location": location}
            )

    def on_page_error(error: Any) -> None:
        summary["page_errors"].append(
            {"at_ms": ts(), "message": str(error), "stack": getattr(error, "stack", None)}
        )

    def on_request_failed(request: Any) -> None:
        if request.failure == "net::ERR_ABORTED" and "/v1/leaderboard" in request.url:
            return
        summary["network_failures"].append(
            {
                "at_ms": ts(),
                "method": request.method,
                "url": request.url,
                "failure": request.failure,
            }
        )

    def on_response(response: Any) -> None:
        if response.status >= 400:
            summary["http_errors"].append(
                {"at_ms": ts(), "status": response.status, "url": response.url}
            )

    page.on("console", on_console)
    page.on("pageerror", on_page_error)
    page.on("requestfailed", on_request_failed)
    page.on("response", on_response)


def assert_no_errors(summary: dict[str, Any]) -> None:
    assert_condition(
        not summary["console_errors"],
        f"observed {len(summary['console_errors'])} console.error message(s): "
        f"{summary['console_errors']}",
    )
    assert_condition(
        not summary["page_errors"],
        f"observed {len(summary['page_errors'])} pageerror event(s): "
        f"{summary['page_errors']}",
    )
    assert_condition(
        not summary["network_failures"],
        f"observed {len(summary['network_failures'])} failed request(s): "
        f"{summary['network_failures']}",
    )
    assert_condition(
        not summary["http_errors"],
        f"observed {len(summary['http_errors'])} HTTP error response(s): "
        f"{summary['http_errors']}",
    )


def run_step(steps: list[dict[str, Any]], name: str, fn: Callable[[], Any]) -> Any:
    """Run and time one named operation, recording its outcome in the steps log."""
    started = time.monotonic()
    entry: dict[str, Any] = {"name": name, "status": "running"}
    steps.append(entry)
    try:
        result = fn()
    except Exception as exc:
        entry.update(
            status="failed",
            duration_ms=elapsed_ms(started),
            error=f"{type(exc).__name__}: {exc}",
        )
        raise
    entry.update(status="passed", duration_ms=elapsed_ms(started))
    return result


def setup_context(
    playwright: Any,
    browser: Any,
    summary: dict[str, Any],
    started_at: float,
    *,
    mobile: bool,
) -> tuple[Any, Any]:
    context = browser.new_context(
        viewport=MOBILE_VIEWPORT if mobile else DESKTOP_VIEWPORT,
        has_touch=mobile,
        is_mobile=mobile,
        device_scale_factor=1,
    )
    # Wipe settings/legacy keys once per context so every run starts from the
    # deterministic fresh schema, while leaving reloads untouched.
    context.add_init_script(script=fresh_init_script())
    page = context.new_page()
    page.set_default_timeout(60_000)
    page.set_default_navigation_timeout(BOOT_TIMEOUT_MS)
    attach_error_listeners(page, summary, started_at)
    return context, page


def run_desktop(
    page: Any, args: argparse.Namespace, summary: dict[str, Any], out_dir: Path
) -> None:
    sect = summary["desktop"]
    steps: list[dict[str, Any]] = sect["steps"]
    shots: list[str] = sect["screenshots"]

    def shot(name: str) -> None:
        if args.screenshot_policy == "failure":
            return
        path = out_dir / name
        page.screenshot(path=str(path), full_page=True)
        shots.append(str(path))

    def press(key: str, settle: int = 90) -> None:
        page.keyboard.press(key)
        page.wait_for_timeout(settle)

    run_step(steps, "boot", lambda: wait_for_boot(page, args.url))

    def fresh_schema() -> None:
        schema = read_storage(page)
        # A fresh context has no schema and no legacy mute bit. The app may have
        # already persisted the default on its first update; either is the
        # deterministic fresh state. The legacy key must never be (re)created.
        assert_condition(
            schema in (None, DEFAULT_SCHEMA),
            f"fresh schema not default/null: {schema!r}",
        )
        legacy = page.evaluate(f"localStorage.getItem({json.dumps(LEGACY_KEY)})")
        assert_condition(
            legacy is None,
            f"legacy mute key should be absent on fresh load: {legacy!r}",
        )

    run_step(steps, "fresh_schema", fresh_schema)
    run_step(steps, "shot_00_menu", lambda: shot("00_desktop_menu.png"))

    # A. Open with O and adjust every row via keyboard.
    def menu_open_and_adjust() -> None:
        press("o")  # open; selection resets to Volume
        press("ArrowLeft")  # volume 100 -> 90
        assert_storage(page, "v2:90:0:0:")
        press("ArrowRight")  # volume 90 -> 100
        assert_storage(page, "v2:100:0:0:")
        press("ArrowLeft")  # volume 100 -> 90
        assert_storage(page, "v2:90:0:0:")
        press("ArrowDown")  # selection -> Mute
        press("ArrowRight")  # mute -> On
        assert_storage(page, "v2:90:1:0:")
        press("ArrowLeft")  # mute -> Off
        assert_storage(page, "v2:90:0:0:")
        press("Enter")  # toggle mute -> On
        assert_storage(page, "v2:90:1:0:")
        press("ArrowDown")  # selection -> ReducedMotion
        press("ArrowRight")  # reduced motion -> On
        assert_storage(page, "v2:90:1:1:")
        press("ArrowLeft")  # reduced motion -> Off
        assert_storage(page, "v2:90:1:0:")
        press("Space")  # toggle reduced motion -> On
        assert_storage(page, "v2:90:1:1:")
        shot("01_desktop_settings_open.png")

    run_step(steps, "menu_open_and_adjust_rows", menu_open_and_adjust)

    # B. Modal isolation: the menu's start keys (Enter/Space) are swallowed while
    # the modal owns focus. A volume change after each key proves the modal is
    # still open over Menu (a transition to Playing would close the modal and
    # make the adjustment a no-op).
    def menu_modal_isolation() -> None:
        press("ArrowUp")
        press("ArrowUp")  # ReducedMotion -> Mute -> Volume
        press("Enter")  # would start a round if not isolated
        press("ArrowLeft")  # volume 90 -> 80
        assert_storage(page, "v2:80:1:1:")
        press("Space")  # would start a round if not isolated
        press("ArrowLeft")  # volume 80 -> 70
        assert_storage(page, "v2:70:1:1:")

    run_step(steps, "menu_modal_isolation", menu_modal_isolation)

    # C. Close via Escape, then confirm still in Menu by reopening.
    def menu_close_escape() -> None:
        press("Escape")  # close
        press("o")  # reopen (Menu/Paused allow the modal; Playing does not)
        press("ArrowLeft")  # volume 70 -> 60
        assert_storage(page, "v2:60:1:1:")

    run_step(steps, "menu_close_escape", menu_close_escape)

    # D. Close via the Back row (Enter/Space), then confirm still in Menu.
    def menu_close_back_row() -> None:
        press("ArrowDown")
        press("ArrowDown")
        press("ArrowDown")
        press("ArrowDown")  # Volume -> Mute -> ReducedMotion -> Name -> Back
        press("Enter")  # Back row closes the modal
        press("o")  # reopen
        press("ArrowLeft")  # volume 60 -> 50
        assert_storage(page, "v2:50:1:1:")

    run_step(steps, "menu_close_back_row", menu_close_back_row)

    # E. Close via O, then confirm still in Menu.
    def menu_close_o() -> None:
        press("o")  # close
        press("o")  # reopen
        press("ArrowLeft")  # volume 50 -> 40
        assert_storage(page, "v2:40:1:1:")
        press("Escape")  # close (clean state before reload)

    run_step(steps, "menu_close_o", menu_close_o)
    run_step(steps, "shot_02_after_menu", lambda: shot("02_desktop_after_menu.png"))

    # F. Reload persistence: the exact v1 schema survives and the app loads it.
    def reload_persistence() -> None:
        reload_preserving_wasm(page, args.url)
        assert_storage(page, "v2:40:1:1:")
        press("o")  # reopen
        press("ArrowLeft")  # volume 40 -> 30 (proves app loaded 40, not default 100)
        assert_storage(page, "v2:30:1:1:")
        press("Escape")  # close

    run_step(steps, "reload_persistence", reload_persistence)
    run_step(steps, "shot_03_after_reload", lambda: shot("03_desktop_after_reload.png"))

    # G. Pause context: open/close/isolation plus state discrimination.
    def enter_playing_and_pause() -> None:
        press("Enter")  # Menu -> Playing (countdown)
        page.wait_for_timeout(3_700)  # 3/2/1/GO + punch, past InputFrozen
        press("Escape")  # Playing -> Paused
        page.wait_for_timeout(300)

    run_step(steps, "enter_playing_and_pause", enter_playing_and_pause)

    def pause_open() -> None:
        press("o")  # open in Paused
        press("ArrowLeft")  # volume 30 -> 20
        assert_storage(page, "v2:20:1:1:")
        shot("04_desktop_paused_settings.png")

    run_step(steps, "pause_open", pause_open)

    def pause_isolation() -> None:
        # R would restart (Paused -> Menu -> Playing); the modal can never open
        # in Playing, so a successful adjustment proves R was swallowed.
        press("r")
        press("ArrowLeft")  # volume 20 -> 10
        assert_storage(page, "v2:10:1:1:")
        # Q would quit to Menu; the modal can still open in Menu, so the
        # adjustment alone is ambiguous and is disambiguated in the next step.
        press("q")
        press("ArrowRight")  # volume 10 -> 20
        assert_storage(page, "v2:20:1:1:")

    run_step(steps, "pause_isolation", pause_isolation)

    def pause_close_and_discriminate() -> None:
        # Close, then prove we are still in Paused (not Menu): Enter is a no-op
        # in Paused but starts a round from Menu. Reopening the modal after
        # Enter therefore proves the state is still Paused, so Q above was
        # isolated. (If Q had quit to Menu, Enter would start a round to Playing
        # and the modal could not reopen; the assertion would fail.)
        press("Escape")  # close modal
        page.wait_for_timeout(150)
        press("Enter")  # Paused: no-op (Menu: would start a round)
        page.wait_for_timeout(150)
        press("o")  # reopen (proves still Paused)
        press("ArrowRight")  # volume 20 -> 30
        assert_storage(page, "v2:30:1:1:")

    run_step(steps, "pause_close_and_discriminate", pause_close_and_discriminate)

    def pause_resume_and_repause() -> None:
        press("Escape")  # close modal (it was open)
        page.wait_for_timeout(150)
        press("Escape")  # Paused -> Playing (resume)
        page.wait_for_timeout(600)
        press("Escape")  # Playing -> Paused
        page.wait_for_timeout(300)
        press("o")  # reopen
        press("ArrowRight")  # volume 30 -> 40
        assert_storage(page, "v2:40:1:1:")

    run_step(steps, "pause_resume_and_repause", pause_resume_and_repause)

    def pause_quit_to_menu() -> None:
        press("o")  # close
        page.wait_for_timeout(120)
        press("q")  # Paused -> Menu
        page.wait_for_timeout(400)
        press("o")  # reopen in Menu
        press("ArrowRight")  # volume 40 -> 50
        assert_storage(page, "v2:50:1:1:")
        press("Escape")  # close

    run_step(steps, "pause_quit_to_menu", pause_quit_to_menu)

    # Drain any asynchronously queued console/page errors before the final check.
    page.wait_for_timeout(300)
    sect["final_storage"] = read_storage(page)
    run_step(steps, "shot_05_final", lambda: shot("05_desktop_final.png"))


def run_mobile(
    page: Any, args: argparse.Namespace, summary: dict[str, Any], out_dir: Path
) -> None:
    sect = summary["mobile"]
    steps: list[dict[str, Any]] = sect["steps"]
    shots: list[str] = sect["screenshots"]

    def shot(name: str) -> None:
        if args.screenshot_policy == "failure":
            return
        path = out_dir / name
        page.screenshot(path=str(path), full_page=True)
        shots.append(str(path))

    boot_state = run_step(steps, "boot", lambda: wait_for_boot(page, args.url))
    rect = boot_state["canvasRect"]
    # Touch normalization uses the window size, so the canvas must fill the
    # mobile viewport for the fractional tap coordinates to map correctly.
    assert_condition(
        abs(rect["width"] - MOBILE_VIEWPORT["width"]) <= 1
        and abs(rect["height"] - MOBILE_VIEWPORT["height"]) <= 1,
        f"canvas did not fit mobile viewport: {rect}",
    )

    def fresh_schema() -> None:
        schema = read_storage(page)
        assert_condition(
            schema in (None, DEFAULT_SCHEMA),
            f"fresh schema not default/null: {schema!r}",
        )
        legacy = page.evaluate(f"localStorage.getItem({json.dumps(LEGACY_KEY)})")
        assert_condition(
            legacy is None,
            f"legacy mute key should be absent on fresh load: {legacy!r}",
        )

    run_step(steps, "fresh_schema", fresh_schema)
    run_step(steps, "shot_00_menu", lambda: shot("00_mobile_menu.png"))

    def tap(fx: float, fy: float, settle: int = 170) -> None:
        x = rect["left"] + fx * rect["width"]
        y = rect["top"] + fy * rect["height"]
        page.touchscreen.tap(x, y)
        page.wait_for_timeout(settle)

    # Touch row bands: Volume .20..33, Mute .33..45, Reduced .45..57,
    # Name .57..70, Back .70..84. Opener matches x .67..97, y .03..18.
    def touch_open_and_adjust() -> None:
        tap(0.92, 0.07)  # opener -> open (pending Menu->Playing canceled)
        tap(0.20, 0.234)  # Volume left: 100 -> 90
        assert_storage(page, "v2:90:0:0:")
        tap(0.80, 0.234)  # Volume right: 90 -> 100
        assert_storage(page, "v2:100:0:0:")
        tap(0.80, 0.352)  # Mute right: -> On
        assert_storage(page, "v2:100:1:0:")
        tap(0.20, 0.352)  # Mute left: -> Off
        assert_storage(page, "v2:100:0:0:")
        tap(0.50, 0.352)  # Mute center: toggle -> On
        assert_storage(page, "v2:100:1:0:")
        tap(0.80, 0.469)  # ReducedMotion right: -> On
        assert_storage(page, "v2:100:1:1:")
        tap(0.20, 0.469)  # ReducedMotion left: -> Off
        assert_storage(page, "v2:100:1:0:")
        tap(0.50, 0.469)  # ReducedMotion center: toggle -> On
        assert_storage(page, "v2:100:1:1:")
        shot("01_mobile_settings_open.png")

    run_step(steps, "touch_open_and_adjust", touch_open_and_adjust)

    def touch_back_and_reopen() -> None:
        tap(0.50, 0.704)  # Back row -> close (still Menu)
        tap(0.92, 0.07)  # reopen
        tap(0.20, 0.234)  # Volume left: 100 -> 90 (proves reopened)
        assert_storage(page, "v2:90:1:1:")
        shot("02_mobile_settings_reopened.png")
        tap(0.50, 0.704)  # Back -> close
        # After close, the opener tap is consumed by the modal again (not a
        # menu start); confirm the menu never transitioned by reopening once
        # more and verifying storage is still reachable through the modal.
        tap(0.92, 0.07)  # reopen
        tap(0.20, 0.234)  # Volume left: 90 -> 80 (still Menu + modal)
        assert_storage(page, "v2:80:1:1:")
        tap(0.50, 0.704)  # Back -> close
        shot("03_mobile_closed.png")

    run_step(steps, "touch_back_and_reopen", touch_back_and_reopen)

    # H. Pause context: touch start/pause, touch-open/adjust, Back, and a
    # discrimination probe that proves the modal closed over Paused (not Menu).
    def touch_start_and_pause() -> None:
        tap(0.50, 0.80)  # any non-opener tap starts Menu -> Playing (countdown)
        page.wait_for_timeout(3_700)  # 3/2/1/GO + punch, past InputFrozen
        tap(0.50, 0.10)  # top-center PAUSE zone: Playing -> Paused
        page.wait_for_timeout(300)
        shot("04_mobile_paused.png")

    run_step(steps, "touch_start_and_pause", touch_start_and_pause)

    def pause_touch_open_and_adjust() -> None:
        tap(0.92, 0.07)  # opener -> open over Paused (pending Menu transition canceled)
        tap(0.20, 0.234)  # Volume left: 80 -> 70 (pending resume transition canceled)
        assert_storage(page, "v2:70:1:1:")
        shot("05_mobile_paused_settings.png")

    run_step(steps, "pause_touch_open_and_adjust", pause_touch_open_and_adjust)

    def pause_touch_back_and_verify_paused() -> None:
        tap(0.50, 0.704)  # Back row -> close (stays Paused; pending Restart canceled)
        # Discriminate Paused from Menu: Enter is a no-op in Paused but starts a
        # round from Menu. Reopening the modal after Enter proves the state is
        # still Paused. (If Back had slipped to Menu, Enter would start a round
        # to Playing and the modal could not reopen; the assertion would fail.)
        page.keyboard.press("Enter")  # Paused: no-op (Menu: would start a round)
        page.wait_for_timeout(150)
        tap(0.92, 0.07)  # reopen (proves still Paused, not Playing)
        tap(0.20, 0.234)  # Volume left: 70 -> 60 (proves modal reopened)
        assert_storage(page, "v2:60:1:1:")
        tap(0.50, 0.704)  # Back -> close (clean final state, still Paused)
        shot("06_mobile_paused_verified.png")

    run_step(steps, "pause_touch_back_and_verify_paused", pause_touch_back_and_verify_paused)

    page.wait_for_timeout(300)
    sect["final_storage"] = read_storage(page)


def run_scenario(args: argparse.Namespace, summary: dict[str, Any]) -> None:
    out_dir = Path(args.out_dir).expanduser().resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    summary["out_dir"] = str(out_dir)

    browser = None
    active_context = None
    active_page = None
    failure_paths: list[str] = summary["screenshots"]
    recorder = FailureScreenshotRecorder(args.screenshot_policy, out_dir, failure_paths)

    # Import after recorder construction so stale private/failure artifacts are
    # cleared even when Playwright itself is unavailable.
    from playwright.sync_api import sync_playwright

    def capture_failure_screenshot() -> None:
        """Try every live page and fall back to the last pre-cleanup image."""
        recorder.capture(active_page, active_context, browser)

    playwright_instance = None
    try:
        playwright_instance = sync_playwright().start()
        playwright = playwright_instance
        try:
            launch: dict[str, Any] = {"headless": not args.headed}
            channel = resolve_browser_channel(args.browser_channel)
            if channel is not None:
                launch["channel"] = channel
            browser = playwright.chromium.launch(**launch)
            overall = time.monotonic()

            dctx = None
            dpage = None
            try:
                dctx, dpage = setup_context(
                    playwright, browser, summary, overall, mobile=False
                )
            except Exception:
                # setup_context may fail after creating a context/page but before
                # returning it; discover those resources from the browser.
                contexts = list(browser.contexts)
                if contexts:
                    active_context = contexts[-1]
                    pages = list(active_context.pages)
                    active_page = pages[-1] if pages else None
                    recorder.snapshot_before_cleanup(active_page, active_context, browser)
                raise
            active_context = dctx
            active_page = dpage
            try:
                run_desktop(dpage, args, summary, out_dir)
                assert_no_errors(summary)
            except Exception:
                capture_failure_screenshot()
                raise
            finally:
                primary_failure_active = sys.exc_info()[0] is not None
                recorder.snapshot_before_cleanup(dpage, dctx, browser)
                try:
                    dctx.close()
                except Exception as exc:
                    summary["cleanup_errors"].append(
                        f"desktop context.close: {type(exc).__name__}: {exc}"
                    )
                    capture_failure_screenshot()
                    if not primary_failure_active:
                        raise

            mctx = None
            mpage = None
            try:
                mctx, mpage = setup_context(
                    playwright, browser, summary, overall, mobile=True
                )
            except Exception:
                contexts = list(browser.contexts)
                if contexts:
                    active_context = contexts[-1]
                    pages = list(active_context.pages)
                    active_page = pages[-1] if pages else None
                    recorder.snapshot_before_cleanup(active_page, active_context, browser)
                raise
            active_context = mctx
            active_page = mpage
            try:
                run_mobile(mpage, args, summary, out_dir)
                assert_no_errors(summary)
            except Exception:
                capture_failure_screenshot()
                raise
            finally:
                primary_failure_active = sys.exc_info()[0] is not None
                recorder.snapshot_before_cleanup(mpage, mctx, browser)
                try:
                    mctx.close()
                except Exception as exc:
                    summary["cleanup_errors"].append(
                        f"mobile context.close: {type(exc).__name__}: {exc}"
                    )
                    capture_failure_screenshot()
                    if not primary_failure_active:
                        raise
        finally:
            primary_failure_active = sys.exc_info()[0] is not None
            browser_failure: tuple[Exception, Any] | None = None
            if browser is not None:
                try:
                    browser.close()
                except Exception as exc:
                    summary["cleanup_errors"].append(
                        f"browser.close: {type(exc).__name__}: {exc}"
                    )
                    capture_failure_screenshot()
                    browser_failure = (exc, exc.__traceback__)
            if playwright_instance is not None:
                try:
                    playwright_instance.stop()
                except Exception as exc:
                    summary["cleanup_errors"].append(
                        f"playwright.stop: {type(exc).__name__}: {exc}"
                    )
                    capture_failure_screenshot()
                    if browser_failure is None:
                        browser_failure = (exc, exc.__traceback__)

            if not primary_failure_active:
                if browser_failure is not None:
                    capture_failure_screenshot()
                    exc, tb = browser_failure
                    raise exc.with_traceback(tb)
                try:
                    assert_no_errors(summary)
                except Exception:
                    capture_failure_screenshot()
                    raise
            else:
                capture_failure_screenshot()
    finally:
        # Also covers setup failures before either per-context try/finally.
        if sys.exc_info()[0] is not None:
            recorder.snapshot_before_cleanup(active_page, active_context, browser)
            capture_failure_screenshot()


def main() -> int:
    args = parse_args()
    overall = time.monotonic()
    summary: dict[str, Any] = {
        "scenario": "roady_car_settings",
        "status": "running",
        "url": args.url,
        "browser": {
            "engine": "chromium",
            "channel": resolve_browser_channel(args.browser_channel)
            or "playwright-chromium",
            "headed": args.headed,
        },
        "out_dir": str(Path(args.out_dir).expanduser()),
        "screenshots": [],
        "desktop": {"steps": [], "screenshots": [], "final_storage": None},
        "mobile": {"steps": [], "screenshots": [], "final_storage": None},
        "console_errors": [],
        "page_errors": [],
        "network_failures": [],
        "http_errors": [],
        "cleanup_errors": [],
    }

    exit_code = 0
    try:
        run_scenario(args, summary)
        # Recheck after browser/context shutdown so a late event captured while
        # closing cannot leave the run marked as passed.
        assert_no_errors(summary)
        summary["status"] = "passed"
        discard_pre_cleanup_screenshot(
            args.screenshot_policy, Path(args.out_dir).expanduser().resolve()
        )
    except Exception as exc:
        exit_code = 1
        promote_pre_cleanup_screenshot(
            args.screenshot_policy,
            Path(args.out_dir).expanduser().resolve(),
            summary["screenshots"],
        )
        summary["status"] = "failed"
        summary["failure"] = {
            "type": type(exc).__name__,
            "message": str(exc),
            "traceback": traceback.format_exc(),
        }
    finally:
        summary["duration_ms"] = elapsed_ms(overall)
        print(json.dumps(summary, indent=2, sort_keys=True, default=str), flush=True)

    return exit_code


if __name__ == "__main__":
    sys.exit(main())
