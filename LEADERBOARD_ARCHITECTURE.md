# Roady Car Cloudflare Leaderboard Architecture

**Status:** Approved design; implementation is intentionally phased.
**Security goal:** deter casual spam, replay, and automated abuse without claiming that an untrusted browser can prove an honest score.

## 1. Honest threat model

Roady Car's WebAssembly client is public and fully attacker-controlled. Any symmetric HMAC key embedded in the WASM can eventually be extracted or the signing path reused. Even so, Roady Car will require a build-injected client HMAC as an additional nuisance layer: it prevents completely unsigned raw API submissions and raises the effort above merely discovering the endpoint. It is **not** treated as proof of honest gameplay and never replaces the stronger controls below.

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
- A mandatory client HMAC injected from deployment secrets and checked on score submission, explicitly treated as extractable nuisance friction

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

Requires both the opaque Worker-issued session proof and `X-Roady-Client-Signature`, an unpadded base64url HMAC-SHA-256 signature created by the production client. The build key comes from the GitHub Actions secret `ROADY_LEADERBOARD_CLIENT_HMAC_KEY`; the Worker receives the matching runtime secret `LB_CLIENT_HMAC_KEY`.

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
  "round_duration_ms": 161400,
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
- Reconstruct and verify the exact canonical client-HMAC bytes; missing/invalid signatures are rejected, while documentation remains explicit that an extracted key can produce valid signatures
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

## 5. Client submission HMAC

Build secret: `ROADY_LEADERBOARD_CLIENT_HMAC_KEY`.
Worker secret: `LB_CLIENT_HMAC_KEY`.

The key is injected only during the production Rust/WASM build and installed separately as a Worker runtime secret. It must never appear in source, Trunk configuration, JavaScript glue, logs, or repository variables. The compiled WASM necessarily contains enough information to recover or reuse it; this is accepted because the HMAC is only an extra barrier against trivial direct API calls.

Canonical UTF-8 bytes use fixed field order, one ASCII LF separator, and no trailing LF:

```text
roady.v1.score
{sessionId}
{proof}
{name}
{condition}
{terminal_total}
{chickens}
{coins}
{objective_completed_0_or_1}
{max_combo}
{round_duration_ms}
{time_left_ms}
{game_over_reason}
{build}
{platform}
```

Integers are canonical base-10 without a plus sign or unnecessary leading zeroes. Names are normalized to uppercase `[A-Z0-9]{3,5}` before signing. Both client and Worker HMAC the exact bytes with SHA-256 and encode the 32-byte result as unpadded base64url. The Worker validates JSON first, rebuilds the canonical bytes from the validated values, decodes the signature, and performs a fixed-length XOR-accumulate comparison.

This check is mandatory, but an attacker who extracts the key can bypass it. Turnstile, one-time session proof, expiry, condition binding, rate limits, plausibility checks, and moderation remain mandatory and independent.

## 6. Session proof

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

## 7. D1 submission ordering

Do not unconditionally batch `UPDATE used` and `INSERT`, because a batch cannot conditionally skip the insert when the update affected zero rows.

Use:

1. Validate session, proof, telemetry, and limits.
2. `UPDATE sessions SET used=1 WHERE session_id=? AND used=0 AND expires_at>?`.
3. Require `meta.changes == 1`; otherwise return `409`.
4. Insert score.

If the insert fails after session consumption, return an error and require a new session. This is acceptable for the MVP and prevents replay duplicates.

## 8. Score validation

Hard invariants:

- `condition` is 0–4
- `terminal_total == chickens + coins`
- `max_combo` is 1–5
- `round_duration_ms` is a non-negative safe integer at most `1_800_000` ms (30 minutes); `time_left_ms` remains in its broad 0–120,000 ms sane range
- `terminal_total <= SCORE_CAPS[condition]`

The caps must be generous and derived from the shipped rules. A fresh round starts at 60s, ordinary coins add +1.5s each but clamp the *remaining clock* to 90s (`MAX_ROUND_TIME = 90.0` in `src/world.rs`), and the Time power-up adds +5s each clamped to a hard 99s remaining-time ceiling (`TIME_CAP = 99.0` in `src/pickups.rs`). Crucially, repeated pickups can extend **elapsed active play** well beyond 120 seconds even though `time_left_ms` cannot exceed the remaining-clock cap. Therefore 30 minutes (`1_800_000` ms) is a soft review threshold, while the hard duration bound is JavaScript safe-integer exactness. `time_left_ms` uses the shipped `99_000` ms remaining-clock ceiling. Provisional condition score caps are moderation thresholds: near/over-cap values are flagged, while hard score bounds enforce u32 component and checked aggregate arithmetic.

Telemetry such as duration, objective completion, combo, client timestamp, or build is advisory and forgeable. It may generate moderation flags but is not proof.

## 8.1 Validation v2: mechanics, evidence, and moderation

The remaining clock is capped, but elapsed play is not: streamed coins and repeatable Time pickups can replenish the timer indefinitely. Therefore 30 minutes is a **soft review threshold**, not a hard rejection. Durations are hard-bounded only by JavaScript safe-integer/canonical-signing exactness. A high historical peak combo with a low terminal score is also review evidence rather than impossibility because later critter penalties can reduce the chicken score bucket.

Accepted exceptional rows receive deterministic notes such as `review:v1:long-duration`, `review:v1:near-cap`, `review:v1:over-cap`, or `review:v1:implausible-combo`. Authentication, Turnstile, proof/HMAC, condition binding, rate limits, session expiry, one-time claim, replay rejection, and u32 aggregate arithmetic remain strict.

Shipped mechanics live in the pure `roady-score-rules` Rust crate. `rules/roady-rules.v1.json` is generated from that typed source and byte-compared in tests. The game consumes the native crate; a later thin `wasm32-unknown-unknown` adapter can let the TypeScript Worker execute the same rules without rewriting HTTP, D1, or security code.

A bounded event ledger can replay score/time arithmetic and improve moderation/debugging, but client telemetry remains forgeable. It is consistency evidence, not authoritative anti-cheat; authoritative competitive scoring still requires server simulation.

## 8.2 Administrative restoration

`POST /v1/admin/scores/restore` is authenticated with `LB_ADMIN_TOKEN` and reserved for evidence-backed historical recovery. It creates an already-used, unverified `admin_restore:` synthetic session, a visibly sourced score row, an `admin_restorations` provenance row, and a `moderation_log` entry in one idempotent D1 batch. Screenshot-proven and synthetic fields are persisted separately with an evidence SHA-256 and reason. Restored rows must never be represented as Turnstile-verified player submissions.

## 9. Rate limiting and privacy

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

## 10. Turnstile

Turnstile is required on `POST /v1/session` for the public MVP, not postponed to a later hardening phase. The Worker verifies the token with Cloudflare siteverify and optionally the requesting IP. Failed verification returns `422 turnstile_failed`.

The game should request a token only when the player chooses to submit a score, not on every page load.

## 11. CORS

Allow only configured origins, for example:

- Production Roady Car origin
- `http://localhost:8080` in development

Set `Vary: Origin`. Handle `OPTIONS` with allowed methods and `Content-Type, Authorization`. Do not use `Access-Control-Allow-Origin: *` for submission endpoints.

## 12. Client integration

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

## 13. Exact-byte reference and response authentication

The corrected loader reference contributes a useful byte-discipline pattern:

- `scriptsSubscribed.ts` calls `JSON.stringify` once, signs that exact string's UTF-8 bytes, and sends the same string.
- `dataDownloadCore.ts` signs exact raw plaintext bytes or exact `IV || ciphertext` bytes when transport encoding is enabled.

Roady Car follows the serialize-once/canonical-bytes principle for score-submission HMACs. It does not copy the loader's AES-CTR transport encoding or IP/HWID-bound session design: symmetric transport keys are recoverable from public WASM, client-asserted HWID is spoofable, and exact IP pinning is brittle under mobile networks and NAT.

The MVP does not add `X-Response-Signature`; HTTPS authenticates live responses, while a symmetric response-verification key in WASM would also permit forgery after extraction. If independently verifiable offline snapshots are needed later, use asymmetric response signatures with a Worker-only Ed25519/P-256 private key and pinned public key in the game. Sign the exact returned bytes, include freshness/expiry and key ID in the body, use unpadded base64url, verify `response.arrayBuffer()` before parsing, and expose the signature header through CORS.

## 14. Worker repository structure

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
- `LB_CLIENT_HMAC_KEY` (must match production build secret `ROADY_LEADERBOARD_CLIENT_HMAC_KEY`)

Non-secret vars:

- `ALLOWED_ORIGINS`
- `SCORE_CAPS_JSON`
- `BUILD`

GitHub deployment credentials are separate from Worker runtime secrets. The GitHub token needs Workers Scripts Edit, D1 Edit, and Account Settings Read; runtime secrets are installed into the Worker through `wrangler secret put` and never exposed to the game client. The committed `wrangler.toml` keeps a placeholder D1 `database_id`; the deployment workflow ([`.github/workflows/deploy-cloudflare-leaderboard.yml`](.github/workflows/deploy-cloudflare-leaderboard.yml)) injects the real id from the GitHub variable `CLOUDFLARE_D1_DATABASE_ID` into the CI checkout copy at deploy time. See [DEPLOYMENT.md](DEPLOYMENT.md) for the exact variables, secrets, and permissions.

## 15. Retention and moderation

Scheduled cleanup should:

- Delete expired sessions
- Keep at least the top 1000 live scores per condition
- Keep recent submissions for 90 days
- Hide or archive older non-top entries
- Enforce row-count guards before storage becomes expensive

Flag suspicious entries for review rather than automatically deleting plausible high scores.

## 16. Phases

1. **Worker skeleton:** health endpoint, CORS, D1 migrations.
2. **Read-only board:** public cached global/per-condition GET.
3. **Secure submissions:** Turnstile session issuance, Worker proof, one-time D1 write, plausibility caps, rank endpoint.
4. **Moderation:** hide/delete/admin log.
5. **Game integration:** initials UI, fetch/menu display, asynchronous Game Over submission, offline cache.
6. **Hardening:** retention cron, moderation flags, soak/cost tests.

## 17. Non-goals

- No real-time multiplayer in the leaderboard Worker
- No authoritative Roady Car simulation
- No account/OAuth system in MVP
- No claim of tamper-proof scores
- No claim that the mandatory embedded client HMAC proves honest gameplay

## 18. Deployment relationship

Roady Car has **two independent deployment workflows**, each with its own least-privilege Cloudflare token. See [DEPLOYMENT.md](DEPLOYMENT.md) for the full operational walkthrough, exact GitHub variables/secrets, and one-time Cloudflare setup.

- **Static site** — [`deploy-cloudflare-pages.yml`](.github/workflows/deploy-cloudflare-pages.yml) builds the WASM and uploads `dist/` to a Cloudflare Pages Direct Upload project. Token scope: `Account` / `Cloudflare Pages` / `Edit`.
- **Leaderboard Worker** — [`deploy-cloudflare-leaderboard.yml`](.github/workflows/deploy-cloudflare-leaderboard.yml) typechecks and tests the Worker, applies D1 migrations, deploys the Worker, installs runtime secrets, and smoke-tests `/healthz` and `/v1/leaderboard`. Token scope: `Account` / `Workers Scripts` / `Edit`, `Account` / `D1` / `Edit`, `Account` / `Account Settings` / `Read`.

The leaderboard workflow does **not** assume the D1 database, Turnstile widget, or Workers subdomain already exist. It reads the D1 database id and deployed URL from GitHub **variables** (`CLOUDFLARE_D1_DATABASE_ID`, `LEADERBOARD_BASE_URL`) and the runtime secrets from GitHub **secrets** (`LB_SESSION_HMAC_KEY`, `LB_IP_HASH_PEPPER`, `LB_ADMIN_TOKEN`, `LB_TURNSTILE_SECRET`, `ROADY_LEADERBOARD_CLIENT_HMAC_KEY` installed as Worker `LB_CLIENT_HMAC_KEY`, plus `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`). If any are missing it fails with an actionable `::error::` checklist before touching Cloudflare. Secret values are never echoed: each is piped over stdin to `wrangler secret put`.

The committed `leaderboard/wrangler.toml` keeps a placeholder D1 `database_id`; the workflow substitutes the real id into the CI checkout copy at deploy time (via `sed`), so no source edit is needed and the public repo stays decoupled from a specific Cloudflare account.

`ROADY_LEADERBOARD_CLIENT_HMAC_KEY` is the single shared nuisance client-HMAC key: the Pages workflow embeds it into the WASM at build time and the leaderboard workflow installs it as the Worker's `LB_CLIENT_HMAC_KEY` runtime secret, so the two are guaranteed identical when both workflows read the same GitHub secret.

A custom domain or Workers route can serve the site and API cleanly without granting broad DNS permissions to CI; configure routes/custom domains once in the Cloudflare dashboard and put the resulting URL in `LEADERBOARD_BASE_URL`.
