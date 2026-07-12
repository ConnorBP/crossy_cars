"""Capture a screenshot of the running game for visual QA.

Usage: python tools/screenshot.py [url] [out.png]
Loads the app, presses Enter, drives forward a few seconds, screenshots.
"""
import sys
from playwright.sync_api import sync_playwright

URL = sys.argv[1] if len(sys.argv) > 1 else "http://localhost:8080"
OUT = sys.argv[2] if len(sys.argv) > 2 else "tools/qa_screenshot.png"

with sync_playwright() as p:
    browser = p.chromium.launch(channel="chrome", headless=True)
    ctx = browser.new_context(viewport={"width": 1280, "height": 720})
    page = ctx.new_page()
    page.goto(URL, wait_until="load", timeout=30000)
    page.wait_for_timeout(6000)          # boot + countdown start
    page.keyboard.press("Enter")          # start the round (countdown 3-2-1-GO)
    page.wait_for_timeout(4000)           # let countdown finish + car start
    # drive forward + weave to get into the world
    for k in ["KeyW"] * 6 + ["KeyA", "KeyW", "KeyD", "KeyW"]:
        page.keyboard.down(k); page.wait_for_timeout(450); page.keyboard.up(k); page.wait_for_timeout(80)
    page.wait_for_timeout(1500)
    page.screenshot(path=OUT, full_page=False)
    print(f"saved {OUT}")
    browser.close()
