"""Headless startup-crash audit for the Bevy/wasm car game.

Launches Chrome via Playwright, loads the served app, collects console + page
errors, presses a key (to unlock audio + start gameplay), drives a bit, and
reports any panics / render errors / runtime errors.

Usage: python tools/browser_audit.py [url]   (default http://localhost:8080)
Exit 0 = no crash detected; Exit 1 = crash/error detected.
"""
import json
import re
import sys

from playwright.sync_api import sync_playwright

URL = sys.argv[1] if len(sys.argv) > 1 else "http://localhost:8080"

PANIC_RX = re.compile(
    r"panicked|RuntimeError|unreachable|__rust_abort|UnrecognizedFormat|"
    r"Validation Error|Caught rendering error|Quitting the application|"
    r"Result::unwrap\(\) on an `Err`",
    re.IGNORECASE,
)


def main() -> int:
    logs: list[tuple[str, str]] = []
    page_errors: list[str] = []
    crashed = False
    crash_reason = ""

    def scan() -> None:
        nonlocal crashed, crash_reason
        for typ, text in logs:
            if typ == "error" and PANIC_RX.search(text):
                crashed = True
                crash_reason = f"console error: {text[:300]}"
                return
        for e in page_errors:
            if PANIC_RX.search(e) or re.search(r"RuntimeError|unreachable", e, re.I):
                crashed = True
                crash_reason = f"pageerror: {e[:300]}"
                return

    with sync_playwright() as p:
        browser = p.chromium.launch(channel="chrome", headless=True)
        ctx = browser.new_context(viewport={"width": 1280, "height": 720})
        page = ctx.new_page()

        page.on("console", lambda msg: logs.append((msg.type, msg.text)))
        page.on("pageerror", lambda err: page_errors.append(str(err)))

        try:
            page.goto(URL, wait_until="load", timeout=30000)
            page.wait_for_timeout(6000)  # boot: GPU init, first frames, asset load
            scan()

            if not crashed:
                # Start gameplay (Enter) + drive to exercise systems (collision,
                # chunk recycling, audio). Keypress also resumes AudioContext.
                page.keyboard.press("Enter")
                page.wait_for_timeout(1500)
                for k in ["KeyW", "KeyW", "KeyA", "KeyW", "KeyD",
                          "KeyW", "KeyS", "KeyW", "KeyA", "KeyW"]:
                    page.keyboard.down(k)
                    page.wait_for_timeout(400)
                    page.keyboard.up(k)
                    page.wait_for_timeout(120)
                page.wait_for_timeout(2500)
                scan()
        except Exception as e:  # noqa: BLE001
            crashed = True
            crash_reason = f"audit harness error: {str(e)[:300]}"
        finally:
            browser.close()

    errors = [t for _, t in (x for x in logs if x[0] == "error")]
    warns = [t for _, t in (x for x in logs if x[0] == "warning")]

    def dedup(arr):
        return list(dict.fromkeys(arr))

    report = {
        "url": URL,
        "crashed": crashed,
        "crashReason": crash_reason,
        "errorCount": len(errors),
        "warnCount": len(warns),
        "pageErrorCount": len(page_errors),
        "errorSamples": [s[:200] for s in dedup(errors)[:6]],
        "warnSamples": [s[:160] for s in dedup(warns)[:4]],
        "pageErrors": [s[:200] for s in dedup(page_errors)[:4]],
    }
    print(json.dumps(report, indent=2))
    return 1 if crashed else 0


if __name__ == "__main__":
    sys.exit(main())
