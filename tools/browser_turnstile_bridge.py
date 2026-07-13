#!/usr/bin/env python3
"""Focused browser tests for the Turnstile bridge embedded in index.html.

The test loads only the inline leaderboard bridge into a synthetic page, so it
needs Playwright/Chromium but does not need a Trunk server or WASM build::

    python tools/browser_turnstile_bridge.py --browser-channel chromium
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path
from typing import Any


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Test the bounded Turnstile JS bridge")
    parser.add_argument(
        "--browser-channel",
        default=os.environ.get("BROWSER_CHANNEL", "chrome"),
        help="Chrome channel, or 'chromium' for Playwright's bundled browser",
    )
    parser.add_argument("--headed", action="store_true")
    return parser.parse_args()


def browser_channel(value: str) -> str | None:
    if value.strip().lower() in {"", "chromium", "playwright", "bundled"}:
        return None
    return value.strip()


def leaderboard_bridge_script() -> str:
    index_path = Path(__file__).resolve().parents[1] / "index.html"
    source = index_path.read_text(encoding="utf-8")
    marker = "Roady Car leaderboard JS bridge"
    marker_at = source.find(marker)
    if marker_at < 0:
        raise AssertionError(f"leaderboard bridge marker missing from {index_path}")
    script_at = source.find("<script>", marker_at)
    script_end = source.find("</script>", script_at)
    if script_at < 0 or script_end < 0:
        raise AssertionError(f"leaderboard bridge script missing from {index_path}")
    return source[script_at + len("<script>") : script_end]


def assert_equal(actual: Any, expected: Any, context: str) -> None:
    if actual != expected:
        raise AssertionError(f"{context}: expected {expected!r}, got {actual!r}")


def run(page: Any) -> list[str]:
    passed: list[str] = []

    exposed = page.evaluate(
        """() => ({
            get: typeof window.roadyLeaderboard.getTurnstileToken,
            cancel: typeof window.roadyLeaderboard.cancelTurnstileRequests
        })"""
    )
    assert_equal(exposed, {"get": "function", "cancel": "function"}, "public API")
    passed.append("api_exposed")

    # A widget that never invokes any callback must settle at the bounded
    # timeout and synchronously clean both widget and temporary container.
    timeout = page.evaluate(
        """async () => {
            window.__roadyTurnstileTimeoutMs = 20;
            const removed = [];
            window.turnstile = {
                render: () => "timeout-widget",
                remove: id => removed.push(id),
            };
            const result = await window.roadyLeaderboard.getTurnstileToken("site-key");
            return {
                result,
                removed,
                containers: document.querySelectorAll("[data-roady-turnstile-request]").length,
            };
        }"""
    )
    assert_equal(timeout["result"], {"ok": False, "error": "Challenge timed out"}, "timeout result")
    assert_equal(timeout["removed"], ["timeout-widget"], "timeout widget cleanup")
    assert_equal(timeout["containers"], 0, "timeout container cleanup")
    passed.append("no_callback_timeout")

    # Cancellation snapshots all active requests. Every promise settles and
    # every request independently removes its widget/container.
    cancellation = page.evaluate(
        """async () => {
            window.__roadyTurnstileTimeoutMs = 10000;
            let nextWidget = 1;
            const removed = [];
            window.turnstile = {
                render: () => "cancel-widget-" + nextWidget++,
                remove: id => removed.push(id),
            };
            const first = window.roadyLeaderboard.getTurnstileToken("site-key");
            const second = window.roadyLeaderboard.getTurnstileToken("site-key");
            window.roadyLeaderboard.cancelTurnstileRequests();
            const results = await Promise.all([first, second]);
            return {
                results,
                removed,
                containers: document.querySelectorAll("[data-roady-turnstile-request]").length,
            };
        }"""
    )
    cancelled = {"ok": False, "error": "Challenge cancelled"}
    assert_equal(cancellation["results"], [cancelled, cancelled], "cancel results")
    assert_equal(
        cancellation["removed"],
        ["cancel-widget-1", "cancel-widget-2"],
        "cancel widget cleanup",
    )
    assert_equal(cancellation["containers"], 0, "cancel container cleanup")
    passed.append("cancellation")

    # A callback arriving after cancellation must not resolve again or repeat
    # cleanup. Invoke every callback to exercise their common settle path.
    late = page.evaluate(
        """async () => {
            window.__roadyTurnstileTimeoutMs = 10000;
            const removed = [];
            let options;
            window.turnstile = {
                render: (_container, value) => { options = value; return "late-widget"; },
                remove: id => removed.push(id),
            };
            const pending = window.roadyLeaderboard.getTurnstileToken("site-key");
            window.roadyLeaderboard.cancelTurnstileRequests();
            const result = await pending;
            options.callback("too-late");
            options["error-callback"]();
            options["expired-callback"]();
            await new Promise(resolve => setTimeout(resolve, 30));
            return {
                result,
                removed,
                containers: document.querySelectorAll("[data-roady-turnstile-request]").length,
            };
        }"""
    )
    assert_equal(late["result"], cancelled, "late callback result")
    assert_equal(late["removed"], ["late-widget"], "late callback idempotent cleanup")
    assert_equal(late["containers"], 0, "late callback container cleanup")
    passed.append("late_callback_no_double_cleanup")

    # Cancellation can happen re-entrantly inside render, before render has
    # returned its widget ID. The returned ID must then be removed immediately.
    during_render = page.evaluate(
        """async () => {
            window.__roadyTurnstileTimeoutMs = 10000;
            const removed = [];
            window.turnstile = {
                render: () => {
                    window.roadyLeaderboard.cancelTurnstileRequests();
                    return "returned-after-cancel";
                },
                remove: id => removed.push(id),
            };
            const result = await window.roadyLeaderboard.getTurnstileToken("site-key");
            return {
                result,
                removed,
                containers: document.querySelectorAll("[data-roady-turnstile-request]").length,
            };
        }"""
    )
    assert_equal(during_render["result"], cancelled, "cancel during render result")
    assert_equal(
        during_render["removed"],
        ["returned-after-cancel"],
        "cancel during render widget cleanup",
    )
    assert_equal(during_render["containers"], 0, "cancel during render container cleanup")
    passed.append("cancel_before_render_returns")

    # Repeated synchronous completions exercise the render-return race while
    # confirming monotonic request IDs and that no temporary DOM accumulates.
    repeated = page.evaluate(
        """async () => {
            window.__roadyTurnstileTimeoutMs = 10000;
            const requestIds = [];
            const removed = [];
            let nextWidget = 1;
            window.turnstile = {
                render: (container, options) => {
                    requestIds.push(Number(container.dataset.roadyTurnstileRequest));
                    const widget = "repeat-widget-" + nextWidget++;
                    options.callback("token-" + widget);
                    return widget;
                },
                remove: id => removed.push(id),
            };
            const results = [];
            for (let i = 0; i < 25; i++) {
                results.push(await window.roadyLeaderboard.getTurnstileToken("site-key"));
            }
            return {
                allOk: results.every(value => value.ok && value.token),
                requestIds,
                uniqueIds: new Set(requestIds).size,
                increasing: requestIds.every((id, index) => index === 0 || id > requestIds[index - 1]),
                removed,
                containers: document.querySelectorAll("[data-roady-turnstile-request]").length,
            };
        }"""
    )
    assert_equal(repeated["allOk"], True, "repeated callback results")
    assert_equal(repeated["uniqueIds"], 25, "repeated unique request IDs")
    assert_equal(repeated["increasing"], True, "monotonic request IDs")
    assert_equal(len(repeated["removed"]), 25, "repeated widget cleanup count")
    assert_equal(repeated["containers"], 0, "repeated requests leave zero containers")
    passed.append("repeated_requests_zero_containers")

    return passed


def main() -> int:
    options = parse_args()
    summary: dict[str, Any] = {"status": "running", "tests": []}
    try:
        from playwright.sync_api import sync_playwright

        with sync_playwright() as playwright:
            launch: dict[str, Any] = {"headless": not options.headed}
            selected_channel = browser_channel(options.browser_channel)
            if selected_channel is not None:
                launch["channel"] = selected_channel
            browser = playwright.chromium.launch(**launch)
            try:
                page = browser.new_page()
                page.set_content(
                    "<!doctype html><html><body><script>"
                    + leaderboard_bridge_script()
                    + "</script></body></html>"
                )
                summary["tests"] = run(page)
            finally:
                browser.close()
        summary["status"] = "passed"
    except Exception as exc:  # noqa: BLE001 - report focused test failures
        summary["status"] = "failed"
        summary["error"] = f"{type(exc).__name__}: {exc}"

    print(json.dumps(summary, indent=2, sort_keys=True))
    return 0 if summary["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
