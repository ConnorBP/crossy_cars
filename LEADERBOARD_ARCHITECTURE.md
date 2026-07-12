# Roady Car Cloudflare Leaderboard Architecture

**Status:** Approved design; implementation is intentionally phased.  
**Security goal:** deter casual spam, replay, and automated abuse without claiming that an untrusted browser can prove an honest score.

## 1. Honest threat model

Roady Car's WebAssembly client is public and fully attacker-controlled. Any symmetric HMAC key embedded in the WASM can be extracted or the signing path can be reused. Therefore an embedded client secret is **not** a trust boundary and is omitted from the MVP.

The leaderboard will use defense in depth:

- Cloudflare Turnstile on session issuance
- Per-IP rate limits on sessions, submissions, and reads
- Short-lived Worker-signed bearer proofs
- One-time D1 sessions to prevent replay
- Condition-bound submissions
- Score invariants and generous plausibility caps
- Hashed IP attribution, never raw IP storage
- Moderation flags and hide/delete controls
- Cached public reads to limit cost and denial-of-service exposure

This prevents casual unsigned spam and makes automated abuse more expensive. It does **not** cryptographically prove the score was earned. A determined attacker can fabricate telemetry below the plausibility cap. Truly authoritative scoring requires server-side game simulation; see [MULTIPLAYER_PLAN.md](MULTIPLAYER_PLAN.md).

## 2. Service layout

Use a separate Cloudflare Worker named `roady-leaderboard` with:

- **D1** database `roady_leaderboard`
- **Cloudflare Rate Limiting bindings** for public reads, sessions, submissions, and authenticated rank reads
- Optional **KV** only for short-lived edge cache metadata; the Cache API is sufficient for top boards initially
- **Turnstile** verification through its siteverify endpoint
- Scheduled cleanup for expired sessions and old non-top scores

The static Roady Car site can remain on Cloudflare Pages. The leaderboard Worker should use a separate subdomain such as `leaderboard.example.com`, or a Workers route such as `/api/leaderboard/*` on a custom domain. CORS must allow only the production site origin and localhost during development.

## 3. D1 schema

```sql
CREATE TABLE sessions (
  session_id TEXT PRIMARY KEY,
  challenge TEXT NOT NULL,
  condition INTEGER NOT NULL CHECK(condition BETWEEN 0 AND 4),
  proof TEXT NOT NULL,
  issued_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  used INTEGER NOT NULL DEFAULT 0 CHECK(used IN (0, 1)),
  turnstile_verified INTEGER NOT NULL DEFAULT 0 CHECK(turnstile_verified IN (0, 1)),
  ip_hash TEXT NOT NULL
);

CREATE TABLE scores (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL,
  condition INTEGER NOT NULL CHECK(condition BETWEEN 0 AND 4),
  terminal_total INTEGER NOT NULL CHECK(terminal_total >= 0),
  chickens INTEGER NOT NULL CHECK(chickens >= 0),
  coins INTEGER NOT NULL CHECK(coins >= 0),
  objective_completed INTEGER NOT NULL CHECK(objective_completed IN (0, 1)),
  max_combo INTEGER NOT NULL CHECK(max_combo BETWEEN 1 AND 5),
  round_duration_ms INTEGER NOT NULL CHECK(round_duration_ms >= 0),
  time_left_ms INTEGER NOT NULL CHECK(time_left_ms >= 0),
  game_over_reason TEXT NOT NULL CHECK(game_over_reason IN ('time_up', 'wrecked')),
  build TEXT NOT NULL,
  platform TEXT NOT NULL CHECK(platform IN ('web', 'native')),
  session_id TEXT NOT NULL UNIQUE,
  submitted_at INTEGER NOT NULL,
  ip_hash TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'live' CHECK(status IN ('live', 'hidden', 'deleted')),
  moderation_note TEXT,
  FOREIGN KEY(session_id) REFERENCES sessions(session_id)
);

CREATE TABLE moderation_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  action TEXT NOT NULL,
  target_score_id INTEGER NOT NULL,
  admin TEXT NOT NULL,
  at INTEGER NOT NULL,
  note TEXT
);

CREATE INDEX idx_scores_global
  ON scores(status, terminal_total DESC, submitted_at ASC);
CREATE INDEX idx_scores_condition
  ON scores(condition, status, terminal_total DESC, submitted_at ASC);
CREATE INDEX idx_scores_submitted_at ON scores(submitted_at);
CREATE INDEX idx_sessions_expires_at ON sessions(expires_at);
```

Names are normalized server-side to 3–5 characters from `[A-Z0-9]`. Invalid names return `422`.

## 4. API

All responses use JSON. Errors use:

```json
{
  "error": {
    "code": "rate_limited",
    "message": "Too many requests",
    "requestId": "..."
  }
}
```

### `POST /v1/session`

Creates a five-minute submission session.

Request:

```json
{
  "condition": 0,
  "turnstileToken": "..."
}
```

Response:

```json
{
  "sessionId": "base64url-random-id",
  "challenge": "base64url-random-challenge",
  "condition": 0,
  "expiresAt": 1760000000000,
  "proof": "base64url-worker-hmac"
}
```

Controls:

- Turnstile required in production
- Limit: 3/minute/IP
- No cache
- Session ID and challenge generated with `crypto.getRandomValues`

### `POST /v1/scores`

Request:

```json
{
  "sessionId": "...",
  "proof": "...",
  "name": "AAA",
  "condition": 0,
  "terminal_total": 42,
  "chickens": 30,
  "coins": 12,
  "objective_completed": true,
  "max_combo": 4,
  "round_duration_ms": 65000,
  "time_left_ms": 0,
  "game_over_reason": "time_up",
  "build": "0.1.0",
  "platform": "web"
}
```

Response `201`:

```json
{
  "inserted": true,
  "rank": 7,
  "condition": 0,
  "total": 42,
  "submittedAt": 1760000000000
}
```

Controls:

- Limit: 5/minute/IP
- Look up `session_id`; require unused, unexpired, matching condition
- Verify opaque Worker HMAC proof
- Require `terminal_total == chickens + coins`
- Require total below the condition plausibility cap
- Validate name/build/platform/reason/max combo/ranges
- Mark session used before inserting; if insert fails, player obtains a new session
- Duplicate/replayed session returns `409`

### `GET /v1/leaderboard`

Query:

- `condition=0..4`, or omitted for global
- `limit=1..100`, default 25
- `offset>=0`

Response:

```json
{
  "condition": "global",
  "entries": [
    {
      "rank": 1,
      "name": "AAA",
      "score": 120,
      "condition": 2,
      "submittedAt": 1760000000000
    }
  ],
  "generatedAt": 1760000000000
}
```

Controls:

- Public limit: 30/minute/IP
- `Cache-Control: public, max-age=30, s-maxage=60, stale-while-revalidate=120`
- Cache key includes API version, condition, limit, and offset
- Only `status='live'`

### `GET /v1/me/rank`

Requires a **used** session ID from a successful submission. Returns the player's entry and nearby ranks. Limit 60/minute/IP. `Cache-Control: private, no-store`.

### Moderation endpoints

- `POST /v1/admin/scores/:id/hide`
- `DELETE /v1/admin/scores/:id`

Require `Authorization: Bearer <LB_ADMIN_TOKEN>`. Every action writes `moderation_log`.

### `GET /healthz`

Returns `{ "ok": true, "build": "...", "time": ... }`.

## 5. Session proof

Worker secret: `LB_SESSION_HMAC_KEY` (at least 32 random bytes, base64).

Canonical UTF-8 bytes:

```text
roady.v1.session
{sessionId}
{challenge}
{condition}
{expiresAt}
```

HMAC algorithm: HMAC-SHA-256 via Workers Web Crypto.

The Worker returns the base64url signature as an opaque bearer proof. On score submission it recomputes and compares the expected signature using XOR accumulation across equal-length byte arrays.

The proof is bound to one session, condition, and expiry. TLS, five-minute expiry, and single-use storage reduce interception/replay risk. It does not prove honest gameplay.

## 6. D1 submission ordering

Do not unconditionally batch `UPDATE used` and `INSERT`, because a batch cannot conditionally skip the insert when the update affected zero rows.

Use:

1. Validate session, proof, telemetry, and limits.
2. `UPDATE sessions SET used=1 WHERE session_id=? AND used=0 AND expires_at>?`.
3. Require `meta.changes == 1`; otherwise return `409`.
4. Insert score.

If the insert fails after session consumption, return an error and require a new session. This is acceptable for the MVP and prevents replay duplicates.

## 7. Score validation

Hard invariants:

- `condition` is 0–4
- `terminal_total == chickens + coins`
- `max_combo` is 1–5
- `round_duration_ms` and `time_left_ms` are in broad sane ranges
- `terminal_total <= SCORE_CAPS[condition]`

The caps must be generous and derived from the shipped rules: up to 90 seconds, 5× combo, objective bonus, condition/event bonuses, and maximum plausible pickups. Scores near the cap should be flagged for moderation rather than rejected merely for being high. Only above-cap values are hard rejected.

Telemetry such as duration, objective completion, combo, client timestamp, or build is advisory and forgeable. It may generate moderation flags but is not proof.

## 8. Rate limiting and privacy

Recommended limits:

| Endpoint | Limit |
|---|---:|
| Public leaderboard | 30/min/IP |
| Create session | 3/min/IP |
| Submit score | 5/min/IP |
| Used-session rank | 60/min/IP |

Store:

```text
ip_hash = base64url(SHA-256(clientIP + LB_IP_HASH_PEPPER))
```

Never store raw IP addresses. Rotate the pepper carefully; rotation breaks historical correlation but not leaderboard rows.

## 9. Turnstile

Turnstile is required on `POST /v1/session` for the public MVP, not postponed to a later hardening phase. The Worker verifies the token with Cloudflare siteverify and optionally the requesting IP. Failed verification returns `422 turnstile_failed`.

The game should request a token only when the player chooses to submit a score, not on every page load.

## 10. CORS

Allow only configured origins, for example:

- Production Roady Car origin
- `http://localhost:8080` in development

Set `Vary: Origin`. Handle `OPTIONS` with allowed methods and `Content-Type, Authorization`. Do not use `Access-Control-Allow-Origin: *` for submission endpoints.

## 11. Client integration

Add a separate Bevy `LeaderboardPlugin`; keep `PersistPlugin` as the offline/local record system.

At `OnEnter(GameState::GameOver)`:

1. Snapshot terminal `Score`, `ActiveModifier`, `GameOverReason`, `ActiveObjective`, peak combo, and active duration.
2. Enter initials/submission UI without blocking Game Over navigation.
3. Request a Turnstile-backed session.
4. Submit asynchronously.
5. Show rank or an offline/retry status.

Fetch global/per-condition boards on Menu entry. Cache the last successful response locally for offline display. Network errors must never block gameplay, menu navigation, or local best persistence.

Initials UI:

- 3–5 uppercase ASCII letters/digits
- Keyboard typing/backspace/Enter/Escape
- Touch A–Z/0–9 grid and submit/skip buttons
- While initials UI is active, regular Game Over restart/menu input is suspended

Use raw `web-sys` Fetch bindings instead of `reqwest` to preserve WASM size. The required web-sys features are `Headers`, `Request`, `RequestInit`, `Response`, `AbortController`, and `AbortSignal` in addition to existing `Window`/`Storage`.

## 12. Worker repository structure

```text
leaderboard/
  package.json
  wrangler.toml
  .dev.vars.example
  migrations/
    0001_init.sql
    0002_indexes.sql
  src/
    index.ts
    security.ts
    validation.ts
    responses.ts
  test/
    index.test.ts
    helpers.ts
  vitest.config.ts
```

Secrets:

- `LB_SESSION_HMAC_KEY`
- `LB_IP_HASH_PEPPER`
- `LB_ADMIN_TOKEN`
- `LB_TURNSTILE_SECRET`

Non-secret vars:

- `ALLOWED_ORIGINS`
- `SCORE_CAPS_JSON`
- `BUILD`

GitHub deployment credentials are separate from Worker runtime secrets. The GitHub token needs Workers Scripts Edit and D1 Edit; runtime secrets are installed into the Worker through Wrangler and never exposed to the game client.

## 13. Retention and moderation

Scheduled cleanup should:

- Delete expired sessions
- Keep at least the top 1000 live scores per condition
- Keep recent submissions for 90 days
- Hide or archive older non-top entries
- Enforce row-count guards before storage becomes expensive

Flag suspicious entries for review rather than automatically deleting plausible high scores.

## 14. Phases

1. **Worker skeleton:** health endpoint, CORS, D1 migrations.
2. **Read-only board:** public cached global/per-condition GET.
3. **Secure submissions:** Turnstile session issuance, Worker proof, one-time D1 write, plausibility caps, rank endpoint.
4. **Moderation:** hide/delete/admin log.
5. **Game integration:** initials UI, fetch/menu display, asynchronous Game Over submission, offline cache.
6. **Hardening:** retention cron, moderation flags, soak/cost tests.

## 15. Non-goals

- No real-time multiplayer in the leaderboard Worker
- No authoritative Roady Car simulation
- No account/OAuth system in MVP
- No claim of tamper-proof scores
- No embedded client HMAC as a hard gate

## 16. Deployment relationship

The current Cloudflare Pages workflow is blocked because the available token lacks **Cloudflare Pages Edit**, and the `roady-car` Pages project does not yet exist. The leaderboard Worker requires a separate token or expanded token with Workers Scripts Edit and D1 Edit.

Prefer separate deployment workflows and least-privilege tokens:

- Static Pages workflow: Pages Edit
- Leaderboard Worker workflow: Workers Scripts Edit + D1 Edit

A custom domain can route the site and API cleanly without granting broad DNS permissions to CI; configure routes/custom domains once in the Cloudflare dashboard.
