# Roady Car Leaderboard Worker

A Cloudflare Worker backend for the Roady Car leaderboard. Implements the design
in [`../LEADERBOARD_ARCHITECTURE.md`](../LEADERBOARD_ARCHITECTURE.md): a cached
public board, Turnstile-backed short-lived sessions, canonical HMAC score
submission, one-time replay protection, per-IP rate limiting with hashed IP
attribution, validation/plausibility caps, moderation, and a scheduled cleanup
hook.

> **Threat model (read this first).** The Roady Car WASM client is public and
> attacker-controlled. The embedded client HMAC key is recoverable. This Worker
> provides defense in depth — **it does not prove scores are honest.** A
> determined attacker can fabricate plausible telemetry below the cap.
> Authoritative scoring requires server-side simulation (see
> `MULTIPLAYER_PLAN.md`). The client HMAC is nuisance friction only.

## Layout

```
leaderboard/
  package.json
  wrangler.toml          # placeholder D1 id + bindings (replace before deploy)
  tsconfig.json
  vitest.config.ts
  .dev.vars.example      # local dev secrets template (NEVER commit real values)
  .gitignore
  migrations/
    0001_init.sql        # sessions, scores, moderation_log
    0002_indexes.sql     # leaderboard + session expiry indexes
  src/
    index.ts             # router, endpoints, D1 queries, rate limiting, cron
    security.ts          # HMAC, canonical bytes, IP hash, constant-time compare
    validation.ts        # name normalization, score invariants, plausibility caps
    responses.ts         # JSON/CORS/error helpers; strict shared origins
    svg.ts               # accessible dark/gold SVG renderer + XML escaping
  vendor/
    cloudflare-game-common/ # local adapter for unpublished shared package
  test/
    index.test.ts        # pure helpers: canonical bytes, HMAC, XML escape, replay
    routes.test.ts       # fetch-route integration tests, including SVG/cache/CORS
    helpers.ts           # signers + in-memory D1 fake for endpoint ordering
```

## Prerequisites

- Node.js 22+ (Node 24 used in CI; the Web Crypto globals used by tests ship in
  Node 20+).
- Wrangler 4 (`npm install` brings it locally; no global install needed).
- A Cloudflare account with permission to create a D1 database and a Worker.

## Local development

```bash
# from leaderboard/
npm install

# 1. Create a local D1 database and apply migrations (uses wrangler.toml).
npx wrangler d1 create roady_leaderboard      # prints a database_id
#   paste the printed database_id into wrangler.toml, replacing the placeholder
npx wrangler d1 migrations apply roady_leaderboard --local

# 2. Configure local secrets for `wrangler dev`.
cp .dev.vars.example .dev.vars
#   edit .dev.vars and replace each REPLACE_* value with a random local value:
#     openssl rand -base64 32   # for LB_SESSION_HMAC_KEY, LB_IP_HASH_PEPPER
#     openssl rand -hex 32      # for LB_ADMIN_TOKEN
#   LB_TURNSTILE_SECRET=1x0000000000000000000000000000000AA is the documented
#   "always-pass" test secret; local dev uses it so no network Turnstile call
#   is required.
#   LB_CLIENT_HMAC_KEY can be any local string; it only needs to match the key
#   your local game build signs with.

# 3. Run the Worker locally.
npm run dev              # wrangler dev, default http://localhost:8787

# 4. Smoke test.
curl http://localhost:8787/healthz
curl 'http://localhost:8787/v1/leaderboard?condition=0&limit=10'
curl 'http://localhost:8787/v1/leaderboard.svg?condition=0&limit=10'
```

## Tests

```bash
npm test                 # vitest run — unit, route, and replay-ordering tests
npm run test:watch       # watch mode
npm run typecheck        # tsc --noEmit
```

The tests are pure unit tests (canonical byte construction, HMAC sign/verify
round-trips, name normalization, score validation invariants, plausibility
caps & moderation flagging, constant-time comparison, IP hashing) plus the
replay-sensitive one-time session claim exercised against an in-memory D1 fake
(`test/helpers.ts` `FakeD1`). They run in the default Node environment using
Node's global Web Crypto API; no Cloudflare bindings or Miniflare runtime are
required. An optional integration tier using
`@cloudflare/vitest-pool-workers` with a real Miniflare D1 can be layered on
later.

## Deploy

```bash
# 1. Create the remote D1 database (one-time).
npx wrangler d1 create roady_leaderboard
#   paste the printed database_id into wrangler.toml, replacing the placeholder

# 2. Apply migrations to the remote database.
npx wrangler d1 migrations apply roady_leaderboard --remote

# 3. Install runtime secrets (NEVER write these to source or .dev.vars for prod).
npx wrangler secret put LB_SESSION_HMAC_KEY      # openssl rand -base64 32
npx wrangler secret put LB_IP_HASH_PEPPER       # openssl rand -base64 32
npx wrangler secret put LB_ADMIN_TOKEN          # openssl rand -hex 32
npx wrangler secret put LB_TURNSTILE_SECRET     # from Cloudflare dashboard widget
npx wrangler secret put LB_CLIENT_HMAC_KEY      # MUST match the production
                                                #   ROADY_LEADERBOARD_CLIENT_HMAC_KEY
                                                #   build secret

# 4. Deploy the Worker.
npm run deploy           # wrangler deploy

# 5. (Optional) Enable scheduled cleanup by uncommenting [triggers] in
#    wrangler.toml after reviewing the retention policy in §15.
```

## Configuration

### `wrangler.toml`

- **`database_id` is a placeholder.** Replace it with the output of
  `wrangler d1 create roady_leaderboard` before `dev` or `deploy`.
- **Rate limit bindings** (`RATE_LIMIT_READ/SESSION/SUBMIT/RANK`) use the
  Cloudflare Rate Limiting `unsafe.bindings` form. Each is an independent
  anonymous pool. The per-IP keys are constructed in `src/index.ts`.
- **Non-secret vars** (`ALLOWED_ORIGINS`, `BUILD`, `SCORE_CAPS_JSON`) are set
  inline. Edit `ALLOWED_ORIGINS` to your production origin.

### Secrets (installed via `wrangler secret put`, never in source)

| Secret | Purpose |
|---|---|
| `LB_SESSION_HMAC_KEY` | HMAC-SHA-256 key for Worker-issued session proofs (≥32 random bytes, base64). |
| `LB_IP_HASH_PEPPER` | Pepper mixed with client IP before SHA-256 for `ip_hash` (≥32 random bytes, base64). |
| `LB_ADMIN_TOKEN` | Bearer token for `POST /v1/admin/scores/:id/hide` and `DELETE /v1/admin/scores/:id`. |
| `LB_TURNSTILE_SECRET` | Cloudflare Turnstile secret (from the dashboard widget). |
| `LB_CLIENT_HMAC_KEY` | Client submission HMAC key. **Must match** the production build secret `ROADY_LEADERBOARD_CLIENT_HMAC_KEY`. |

### Non-secret vars

| Var | Example |
|---|---|
| `ALLOWED_ORIGINS` | `https://car.segfault.site,https://roady-car.pages.dev,http://localhost:8080` |
| `BUILD` | `0.1.0` |
| `SCORE_CAPS_JSON` | `{"0":3000,"1":3000,"2":4000,"3":3000,"4":6000}` |

## API summary

See `../LEADERBOARD_ARCHITECTURE.md §4` for the authoritative spec.

| Method | Path | Notes |
|---|---|---|
| `GET` | `/healthz` | `{ ok, build, time }`. |
| `GET` | `/v1/leaderboard` | `condition`, `limit` (1–100, default 25), `offset`. Cached (`public, max-age=30, s-maxage=60, stale-while-revalidate=120`). Only `status='live'`. 30/min/IP. |
| `GET` | `/v1/leaderboard.svg` | Generated accessible fixed-width SVG. Optional `condition` (0–4) and `limit` (1–25, default 10). Same live-score ordering as JSON; variable height and an empty state. Cached (`public, max-age=60, s-maxage=300, stale-while-revalidate=600`). Uses the read rate limit. |
| `POST` | `/v1/session` | `{ condition, turnstileToken }` → `{ sessionId, challenge, condition, expiresAt, proof }`. 5-minute TTL. Turnstile required. 3/min/IP. |
| `POST` | `/v1/scores` | Requires session `proof` + `X-Roady-Client-Signature` (unpadded base64url HMAC-SHA-256 over canonical bytes). One-time session claim; replay → `409`. 5/min/IP. |
| `GET` | `/v1/me/rank?sessionId=` | Requires a *used* session. `private, no-store`. 60/min/IP. |
| `POST` | `/v1/admin/scores/:id/hide` | `Authorization: Bearer <LB_ADMIN_TOKEN>`. Writes `moderation_log`. |
| `DELETE` | `/v1/admin/scores/:id` | `Authorization: Bearer <LB_ADMIN_TOKEN>`. Writes `moderation_log`. |

### Embeddable SVG leaderboard

The Worker endpoint is `/v1/leaderboard.svg`. Production exposes it at:

```text
https://car.segfault.site/api/leaderboard.svg
https://car.segfault.site/api/leaderboard.svg?condition=2&limit=10
```

The production hostname must attach the `roady-leaderboard` Worker to the exact
public route `car.segfault.site/api/leaderboard.svg` (see
[`../DEPLOYMENT.md`](../DEPLOYMENT.md)). The Worker accepts that public path as
an alias of its canonical `/v1/leaderboard.svg` route without redirecting, so
image bytes remain cacheable and independent of the request Origin. Cache
entries contain no CORS headers; allowed-origin CORS is reapplied separately on
every response, including cache hits.

Names and all other interpolated values are XML-escaped. The SVG includes
`role="img"`, a `<title>`, a `<desc>`, a generated timestamp, and explicit
empty-board messaging.

### Canonical client HMAC bytes (`§5`)

```
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

Fixed field order, one ASCII LF separator, no trailing LF. Integers are
canonical base-10 (no `+`, no leading zeros). Names normalized to uppercase
`[A-Z0-9]{3,5}` **before** signing. HMAC-SHA-256 → unpadded base64url. The
Worker validates JSON first, rebuilds the canonical bytes from validated
values, decodes the signature, and compares with fixed-length XOR
accumulation.

### Canonical session proof bytes (`§6`)

```
roady.v1.session
{sessionId}
{challenge}
{condition}
{expiresAt}
```

### Submission ordering (`§7`)

1. Validate session, proof, telemetry, and limits.
2. `UPDATE sessions SET used=1 WHERE session_id=? AND used=0 AND expires_at>?`.
3. Require `meta.changes == 1`; otherwise `409`.
4. Insert score (failure after consumption requires a new session).

### Plausibility caps (`§8`)

Hard invariants: `condition ∈ 0–4`, `terminal_total == chickens + coins`,
`max_combo ∈ 1–5`, sane duration/time ranges, `terminal_total <= cap`.
`round_duration_ms` must be a non-negative safe integer and is accepted through
`1_800_000` ms (30 minutes), inclusive: repeated time pickups can extend
elapsed play beyond 120 seconds even though the remaining clock is capped.
`time_left_ms` uses the shipped 99-second remaining-clock cap. Provisional
condition caps are moderation thresholds, not mathematical ceilings: near-cap
and above-cap scores remain live with deterministic review notes. Hard score
bounds enforce u32 component/aggregate arithmetic. **Near-cap** (≥80% of cap) scores are
accepted and flagged for moderation via `moderation_note`.

### Privacy (`§9`)

`ip_hash = base64url(SHA-256(clientIP + LB_IP_HASH_PEPPER))`. Raw IPs are never
stored. Rotate the pepper carefully — rotation breaks historical correlation
but not existing leaderboard rows.

## Additive ranked v3 service

Rules v3 is isolated under `/v3` and uses migrations `0005_ranked_v2.sql`
(the approved byte-exact v2 migration reserved by the contract) followed by
`0006_ranked_v3.sql`. Migration 0006 creates only `_v3` tables and indexes.
The public capability route is `GET /v3/capabilities`; its checked-in gate is
off and production remains disabled until every parity/deployment item in
`docs/RULES_V3_DROWNED_PROTOCOL_ADDENDUM.md` section 9 has independent release
evidence.

Install the following **additional, independent** production secrets:

```bash
npx wrangler secret put LB_V3_PROOF_HMAC_KEY
npx wrangler secret put LB_V3_SEED_ENCRYPTION_KEY       # unpadded base64url, exactly 32 decoded bytes
npx wrangler secret put LB_V3_SEED_KEY_ID               # e.g. v3.seed.prod.1
npx wrangler secret put LB_V3_EVIDENCE_CAPABILITY_KEY
npx wrangler secret put LB_V3_CLIENT_HMAC_KEYS_JSON     # {"v3.client.1":"..."}; overlap IDs while rotating
```

`LB_V3_SEED_ENCRYPTION_KEY` is AES-256-GCM material. Sessions store only the
12-byte IV, 32-byte seed ciphertext plus 16-byte tag, key ID, and commitments.
Retain every referenced encryption key until no stored started session needs
it; started sessions have no completion TTL. The proof and evidence capability
keys must be random and independent of all v1/v2 keys. The client HMAC remains
recoverable nuisance friction and accepted key IDs should overlap during a
controlled rotation.

`ROADY_V3_RANKED_ENABLED` is a non-secret upper-bound switch. Missing, malformed,
or anything except exact lowercase `true` is disabled. Keep it `false` while
applying and checking migrations, artifacts, workerd replay, smoke tests, and
two uncached production probes. Disabling stops new sessions but intentionally
does not invalidate already-started scores/evidence. Never enable production
by changing the source default or bypassing the code-level parity latch.

V3 endpoint summary: capabilities; session issuance/start; one-time pending
score submission; evidence upload/replay; category leaderboard; personal rank;
and isolated restoration/hide/delete moderation routes. All JSON writes reject
unknown fields and use the addendum's 16 KiB/512 KiB bounds.

## Provisioning blockers (must resolve before deploy)

1. **D1 `database_id` is a placeholder in `wrangler.toml`.** Run
   `npx wrangler d1 create roady_leaderboard` and paste the printed id before
   any `dev`/`deploy`. Without this, both local and remote operations fail.
2. **Runtime secrets are not installed.** All five secrets
   (`LB_SESSION_HMAC_KEY`, `LB_IP_HASH_PEPPER`, `LB_ADMIN_TOKEN`,
   `LB_TURNSTILE_SECRET`, `LB_CLIENT_HMAC_KEY`) must be installed via
   `wrangler secret put`. The Worker reads them from bindings; missing secrets
   cause session issuance / submission / moderation to fail.
3. **`LB_CLIENT_HMAC_KEY` must match the production build secret
   `ROADY_LEADERBOARD_CLIENT_HMAC_KEY`.** The Pages build injects the latter
   into the WASM client (see `.github/workflows/deploy-cloudflare-pages.yml`);
   the Worker must receive the same value as a runtime secret, or every
   submission is rejected with `invalid_signature`.
4. **Turnstile widget must exist.** Create a Cloudflare Turnstile widget in the
   dashboard and put its secret in `LB_TURNSTILE_SECRET`; the corresponding
   site key goes in the game client. Local dev uses the documented
   always-pass test secret `1x0000000000000000000000000000000AA`.
5. **Deployment token permissions.** Deploying this Worker requires a token
   with **Workers Scripts Edit** and **D1 Edit**. The existing Pages workflow
   token is separate (Pages Edit) and is **not** sufficient. The static site
   (`roady-car` Pages) and this Worker should use least-privilege, separate
   tokens/workflows. Configure routes/custom domains once in the dashboard.
6. **`ALLOWED_ORIGINS` must list your production origin.** The default
   (`https://car.segfault.site,https://roady-car.pages.dev,http://localhost:8080`) is a starting point;
   update it to the real production site origin so CORS allows submissions.
7. **V3 secrets, migration parity, and production gate are intentionally unresolved.**
   Provision the five v3 secrets above, apply migrations 0005 then 0006, run
   local/remote D1 and workerd replay checks, and collect the addendum section 9
   evidence. Ranked v3 must remain disabled until a separate review changes the
   code-level parity latch.
8. **Rate limit bindings use the `unsafe.bindings` form.** Confirm your
   Wrangler version / account supports anonymous rate-limit namespaces, or
   switch to named namespaces. Session and submit bindings fail closed when
   missing or errored. Public reads tolerate an absent binding for standalone
   tests/unsupported runtimes, but a configured binding error fails closed.

## Validation v2 and administrative restoration

Elapsed play has no mechanics-derived finite ceiling because coins and Time pickups can repeatedly replenish the remaining clock. The Worker therefore accepts safe-integer durations above 30 minutes and adds `review:v1:long-duration`; it no longer rejects them. Low terminal score after a high peak combo is similarly flagged rather than rejected because critter penalties can reduce score later. Near/over-cap scores remain live with `review:v1:near-cap` / `review:v1:over-cap`. Structural u32 arithmetic, condition/reason IDs, proof/signature/session binding, replay protection, and rate limits remain strict.

The pure Rust crate `crates/roady-score-rules` owns score/time mechanics and generates `rules/roady-rules.v1.json`. CI byte-compares the generated manifest and tests the game workspace. A future WASM adapter will execute the same crate inside the TypeScript Worker.

Authenticated `POST /v1/admin/scores/restore` supports idempotent evidence-backed restoration after migration `0004_admin_restorations.sql`. The exact payload separates screenshot-proven `known` fields from inferred `synthetic` fields and includes a reason plus evidence SHA-256. The Worker creates an already-used/unverified synthetic session and persists provenance/audit rows. This route must never be used as a normal submission bypass.

## Security notes

- **Never commit `.dev.vars` or real secrets.** `.gitignore` excludes them.
- The client HMAC key in WASM is **recoverable**; it only raises the bar above
  trivial unsigned API calls. Do not rely on it for integrity.
- The MVP does **not** add response signatures (`§13`); HTTPS authenticates
  live responses. A symmetric response key in WASM would be forgeable after
  extraction. If offline-verifiable snapshots are needed later, use asymmetric
  (Ed25519/P-256) signatures with a Worker-only private key.
- Raw IP addresses are never stored; only `ip_hash` is persisted.
- Shared Worker primitives are vendored under
  `vendor/cloudflare-game-common` until the separate package is published.
  See its README for source provenance and synchronization policy.
- `ALLOWED_ORIGINS` entries must be canonical exact HTTP(S) origins (for
  example `https://car.segfault.site`, with no trailing slash/path/wildcard).
  One malformed entry invalidates the full list and configuration fails closed.
- Body and free-text limits are measured as encoded UTF-8 bytes, not JavaScript
  UTF-16 code units.
