"""Headless runtime audit for the Bevy/wasm car game.

Loads the served app, collects console and page errors, starts gameplay, and
exercises driving. Every ``console.error`` or ``pageerror`` is a failure.

Usage: python tools/browser_audit.py [url] [--browser-channel chrome]
Exit 0 = no error detected; Exit 1 = browser, page, or harness error detected.
"""
import argparse
import json
import os
import re
import sys
from urllib.parse import urlsplit

DEFAULT_URL = "http://localhost:8080"
DEFAULT_BROWSER_CHANNEL = "chrome"

PANIC_RX = re.compile(
    r"panicked|RuntimeError|unreachable|__rust_abort|UnrecognizedFormat|"
    r"Validation Error|Caught rendering error|Quitting the application|"
    r"Result::unwrap\(\) on an `Err`",
    re.IGNORECASE,
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Audit the Roady Car browser build.")
    parser.add_argument("url", nargs="?", default=DEFAULT_URL)
    parser.add_argument(
        "--browser-channel",
        default=os.environ.get("BROWSER_CHANNEL", DEFAULT_BROWSER_CHANNEL),
        help=(
            "Playwright Chromium channel (default: chrome, or BROWSER_CHANNEL). "
            "Use 'chromium' to use Playwright's bundled browser."
        ),
    )
    return parser.parse_args()


def resolve_browser_channel(value: str) -> str | None:
    """Map a friendly Chromium value to Playwright's channel-less launch."""
    value = value.strip()
    if value.lower() in {"", "chromium", "playwright", "bundled"}:
        return None
    return value


def main() -> int:
    args = parse_args()
    browser_channel = resolve_browser_channel(args.browser_channel)
    logs: list[tuple[str, str]] = []
    page_errors: list[str] = []
    v3_write_requests: list[str] = []
    crashed = False
    crash_reason = ""

    def scan() -> None:
        nonlocal crashed, crash_reason
        # CI is intentionally strict: panic matching remains useful for the
        # reason text, but no console.error or pageerror is ever ignored.
        errors = [text for typ, text in logs if typ == "error"]
        if errors:
            crashed = True
            text = errors[0]
            kind = "panic/runtime console error" if PANIC_RX.search(text) else "console error"
            crash_reason = f"{kind}: {text[:300]}"
            return
        if page_errors:
            crashed = True
            text = page_errors[0]
            kind = "panic/runtime pageerror" if PANIC_RX.search(text) else "pageerror"
            crash_reason = f"{kind}: {text[:300]}"

    browser = None
    try:
        # Import inside the guarded block so dependency/launch failures still
        # produce the same machine-readable JSON report as page failures.
        from playwright.sync_api import sync_playwright

        with sync_playwright() as p:
            launch_options = {"headless": True}
            if browser_channel is not None:
                launch_options["channel"] = browser_channel
            browser = p.chromium.launch(**launch_options)
            ctx = browser.new_context(viewport={"width": 1280, "height": 720})
            page = ctx.new_page()

            page.on("console", lambda msg: logs.append((msg.type, msg.text)))
            page.on("pageerror", lambda err: page_errors.append(str(err)))
            page.on(
                "request",
                lambda request: v3_write_requests.append(request.url)
                if request.method == "POST" and urlsplit(request.url).path.startswith("/v3/")
                else None,
            )

            page.goto(args.url, wait_until="load", timeout=30000)
            page.wait_for_timeout(6000)  # boot: GPU init, first frames, asset load
            scan()

            if not crashed:
                # Start gameplay (Enter) + drive in multiple directions to
                # exercise the 2D grid recycling in ALL four directions.
                # Capability is fail-closed in ordinary audit builds, so the
                # contractual fallback Enter starts Casual Cluck Hunt.
                page.keyboard.press("Enter")
                page.wait_for_timeout(4000)  # let the 3-2-1-GO countdown finish
                # forward, turn left, forward, turn right, forward, reverse,
                # turn around, forward — covers -Z, -X, +X, +Z, -Z.
                for k in [
                    "KeyW", "KeyW", "KeyW",
                    "KeyA", "KeyA",
                    "KeyW", "KeyW", "KeyW",
                    "KeyD", "KeyD",
                    "KeyW", "KeyW",
                    "KeyS", "KeyS",
                    "KeyA", "KeyA", "KeyA",
                    "KeyW", "KeyW", "KeyW",
                    "KeyD", "KeyW", "KeyW",
                ]:
                    page.keyboard.down(k)
                    page.wait_for_timeout(450)
                    page.keyboard.up(k)
                    page.wait_for_timeout(100)
                page.wait_for_timeout(2500)
                if v3_write_requests:
                    crashed = True
                    crash_reason = f"Casual audit emitted v3 writes: {v3_write_requests[:3]}"
                scan()

            # Close while Playwright's event loop is still alive. Closing in
            # the outer finally after leaving `sync_playwright()` fabricates an
            # "event loop is closed" cleanup failure on otherwise clean runs.
            ctx.close()
            browser.close()
            browser = None
    except Exception as e:  # noqa: BLE001
        crashed = True
        crash_reason = f"audit harness error: {str(e)[:300]}"
    finally:
        if browser is not None:
            try:
                browser.close()
            except Exception as e:  # noqa: BLE001
                crashed = True
                if not crash_reason:
                    crash_reason = f"browser cleanup error: {str(e)[:300]}"
        # Include errors delivered at the end of the final wait/close. Do not
        # replace a harness reason, which is generally the more actionable one.
        if not crash_reason:
            scan()
        elif any(typ == "error" for typ, _ in logs) or page_errors:
            crashed = True

    errors = [text for typ, text in logs if typ == "error"]
    warns = [text for typ, text in logs if typ == "warning"]

    def dedup(arr):
        return list(dict.fromkeys(arr))

    report = {
        "url": args.url,
        "browser": {
            "engine": "chromium",
            "channel": browser_channel or "playwright-chromium",
        },
        "crashed": crashed,
        "crashReason": crash_reason,
        "errorCount": len(errors),
        "warnCount": len(warns),
        "pageErrorCount": len(page_errors),
        "errorSamples": [s[:200] for s in dedup(errors)[:6]],
        "warnSamples": [s[:160] for s in dedup(warns)[:4]],
        "pageErrors": [s[:200] for s in dedup(page_errors)[:4]],
        "v3WriteRequests": v3_write_requests,
    }
    print(json.dumps(report, indent=2))
    return 1 if crashed else 0


if __name__ == "__main__":
    sys.exit(main())
