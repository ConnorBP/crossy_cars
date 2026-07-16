#!/usr/bin/env python3
"""Archive every public live v1 leaderboard page without private D1 data."""
from __future__ import annotations

import hashlib
import json
import time
import urllib.parse
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

BASE = "https://roady-leaderboard.connor-postma.workers.dev/v1/leaderboard"
ORIGIN = "https://car.segfault.site"
PAGE_SIZE = 100
ROOT = Path(__file__).resolve().parents[1]


def fetch(url: str) -> bytes:
    request = urllib.request.Request(
        url,
        headers={"Origin": ORIGIN, "User-Agent": "Roady-v1-public-archive/1"},
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        if response.status != 200:
            raise RuntimeError(f"{url}: HTTP {response.status}")
        if "application/json" not in response.headers.get("Content-Type", ""):
            raise RuntimeError(f"{url}: unexpected content type")
        return response.read()


def canonical_json(value: object) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()


def main() -> None:
    captured = datetime.now(timezone.utc).replace(microsecond=0)
    slug = captured.strftime("%Y-%m-%dT%H-%M-%SZ")
    out = ROOT / "docs" / "data" / "leaderboard-v1" / slug
    out.mkdir(parents=True, exist_ok=False)
    boards = []

    for condition in [None, 0, 1, 2, 3, 4]:
        label = "global" if condition is None else f"condition-{condition}"
        entries = []
        pages = []
        offset = 0
        while True:
            query = {"limit": PAGE_SIZE, "offset": offset}
            if condition is not None:
                query["condition"] = condition
            url = f"{BASE}?{urllib.parse.urlencode(query)}"
            raw = fetch(url)
            parsed = json.loads(raw)
            page_entries = parsed.get("entries")
            if not isinstance(page_entries, list):
                raise RuntimeError(f"{url}: entries is not a list")
            expected_condition = "global" if condition is None else condition
            if parsed.get("condition") != expected_condition:
                raise RuntimeError(f"{url}: condition mismatch")
            for index, entry in enumerate(page_entries):
                expected_rank = offset + index + 1
                if entry.get("rank") != expected_rank:
                    raise RuntimeError(f"{url}: rank {entry.get('rank')} != {expected_rank}")
            filename = f"{label}-offset-{offset:06}.json"
            (out / filename).write_bytes(raw)
            pages.append(
                {
                    "file": filename,
                    "url": url,
                    "offset": offset,
                    "entries": len(page_entries),
                    "bytes": len(raw),
                    "sha256": hashlib.sha256(raw).hexdigest(),
                    "generatedAt": parsed.get("generatedAt"),
                }
            )
            entries.extend(page_entries)
            if len(page_entries) < PAGE_SIZE:
                break
            offset += PAGE_SIZE
            time.sleep(2.2)
        combined_file = f"{label}-combined.json"
        combined = {
            "apiVersion": "v1",
            "condition": expected_condition,
            "capturedAt": captured.isoformat().replace("+00:00", "Z"),
            "source": BASE,
            "entryCount": len(entries),
            "entries": entries,
        }
        combined_bytes = canonical_json(combined)
        (out / combined_file).write_bytes(combined_bytes)
        boards.append(
            {
                "board": label,
                "condition": expected_condition,
                "entryCount": len(entries),
                "pageCount": len(pages),
                "combinedFile": combined_file,
                "combinedSha256": hashlib.sha256(combined_bytes).hexdigest(),
                "pages": pages,
            }
        )
        time.sleep(2.2)

    manifest = {
        "schema": "roady-public-v1-leaderboard-archive.v1",
        "capturedAt": captured.isoformat().replace("+00:00", "Z"),
        "sourceBaseUrl": BASE,
        "requestOrigin": ORIGIN,
        "pageSize": PAGE_SIZE,
        "completionRule": "Each board was paginated until the API returned fewer than 100 entries.",
        "scope": "Public status=live rows exposed by GET /v1/leaderboard only.",
        "excluded": [
            "hidden/deleted scores",
            "sessions and proofs",
            "IP hashes",
            "moderation logs and notes",
            "admin restorations metadata not exposed by the public API",
            "secrets and rate-limit identifiers",
        ],
        "legacyConditions": [
            {"id": 0, "name": "Standard"},
            {"id": 1, "name": "Rush Hour"},
            {"id": 2, "name": "Chicken Frenzy"},
            {"id": 3, "name": "Stampede"},
            {"id": 4, "name": "Glass Cannon"},
        ],
        "ordering": "terminal_total DESC, submitted_at ASC, id ASC",
        "repositorySourceCommit": "cb6a872",
        "boards": boards,
    }
    manifest_bytes = canonical_json(manifest)
    (out / "manifest.json").write_bytes(manifest_bytes)
    readme = f"""# Public v1 leaderboard archive — {manifest['capturedAt']}

This directory preserves the public live Roady v1 leaderboard immediately before the gameplay-modes v2 implementation wave.

- Source: `{BASE}`
- Scope: public `status=live` API rows only
- Completion: every global/condition board was paginated with `limit=100` until a short page
- Ordering: `{manifest['ordering']}`
- Legacy condition IDs: `0` Standard, `1` Rush Hour, `2` Chicken Frenzy, `3` Stampede, `4` Glass Cannon
- Repository source commit: `cb6a872`

Raw page files are preserved byte-for-byte as returned by production. Combined files are deterministic, sorted-key JSON convenience copies. `manifest.json` records URLs, response sizes, SHA-256 hashes, API generation timestamps, and exclusions.

This archive contains no hidden/deleted moderation rows, sessions, proofs, IP hashes, secrets, or non-public D1 data.
"""
    (out / "README.md").write_text(readme, encoding="utf-8", newline="\n")
    print(out)
    print(json.dumps({board["board"]: board["entryCount"] for board in boards}, indent=2))


if __name__ == "__main__":
    main()
