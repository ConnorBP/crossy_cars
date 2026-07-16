# Public v1 leaderboard archive — 2026-07-16T12:47:07Z

This directory preserves the public live Roady v1 leaderboard immediately before the gameplay-modes v2 implementation wave.

- Source: `https://roady-leaderboard.connor-postma.workers.dev/v1/leaderboard`
- Scope: public `status=live` API rows only
- Completion: every global/condition board was paginated with `limit=100` until a short page
- Ordering: `terminal_total DESC, submitted_at ASC, id ASC`
- Legacy condition IDs: `0` Standard, `1` Rush Hour, `2` Chicken Frenzy, `3` Stampede, `4` Glass Cannon
- Repository source commit: `cb6a872`

Raw page files are preserved byte-for-byte as returned by production. Combined files are deterministic, sorted-key JSON convenience copies. `manifest.json` records URLs, response sizes, SHA-256 hashes, API generation timestamps, and exclusions.

This archive contains no hidden/deleted moderation rows, sessions, proofs, IP hashes, secrets, or non-public D1 data.
