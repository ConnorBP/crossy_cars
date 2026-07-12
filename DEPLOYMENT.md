# Roady Car deployment

Roady Car ships **two independent production targets**, each with its own
GitHub Actions workflow, Cloudflare token, and rollout cadence:

| Target | Platform | Workflow | Token scope |
| --- | --- | --- | --- |
| Static game site (`dist/`) | Cloudflare **Pages** | [`deploy-cloudflare-pages.yml`](.github/workflows/deploy-cloudflare-pages.yml) | Pages: Edit |
| Leaderboard API | Cloudflare **Worker + D1** | [`deploy-cloudflare-leaderboard.yml`](.github/workflows/deploy-cloudflare-leaderboard.yml) | Workers Scripts: Edit, D1: Edit, Account Settings: Read |

The two workflows are deliberately separate so each token is least-privilege
and the leaderboard backend can be rolled forward or rolled back without
rebuilding the WASM site. Both run only on pushes to `master` (the Pages
workflow on every push, the leaderboard workflow only when `leaderboard/**`
or its own workflow file changes) and via manual **Run workflow** dispatch.
Pull requests never deploy and never see deployment secrets.

The leaderboard backend is specified in [LEADERBOARD_ARCHITECTURE.md](LEADERBOARD_ARCHITECTURE.md)
and implemented under [`leaderboard/`](leaderboard). This document covers the
operational setup for both targets.

---

## Part A — Leaderboard Worker (Cloudflare Workers + D1)

The leaderboard is a Cloudflare Worker named `roady-leaderboard` backed by a
D1 database `roady_leaderboard`, Cloudflare Rate Limiting bindings, Turnstile
verification, and HMAC-gated score submission. The workflow typechecks and
unit-tests the Worker, applies D1 migrations, deploys the Worker, installs its
runtime secrets, and smoke-tests `/healthz` and `/v1/leaderboard`.

### A.1 One-time Cloudflare setup

These resources are created once in the Cloudflare dashboard / CLI. They are
not created by the workflow because a fresh repository has none of them yet;
the workflow instead reads their identifiers from GitHub variables/secrets and
fails with actionable diagnostics if anything is missing.

1. **Create the D1 database** (run locally with Wrangler auth, not in CI):

   ```sh
   cd leaderboard
   npx wrangler@4 d1 create roady_leaderboard
   ```

   Wrangler prints a `database_id`. Copy it — it goes into the GitHub variable
   `CLOUDFLARE_D1_DATABASE_ID`. The committed
   [`leaderboard/wrangler.toml`](leaderboard/wrangler.toml) keeps a placeholder
   `database_id`; the workflow patches the CI checkout copy with the real id at
   deploy time so no source edit is needed and the repo stays decoupled from a
   specific account. (A D1 id is not secret, but keeping it in a variable rather
   than source avoids coupling public deploys to a particular account.)

2. **Create a Turnstile widget.** In the Cloudflare dashboard open
   **Turnstile → Add site**, register the production site origin, and copy the
   **secret key** into GitHub secret `LB_TURNSTILE_SECRET`. For local
   development only, the always-pass test secret
   `1x0000000000000000000000000000000AA` works without network; never use it in
   production. The Worker rejects session creation with `422 turnstile_failed`
   when verification fails or the secret is missing/placeholder.

3. **Create the Workers subdomain and production routes** so the Worker is
   reachable at stable URLs. A `*.workers.dev` subdomain is created the first
   time you enable it for the account; custom routes are configured under
   **Workers & Pages → roady-leaderboard → Settings → Domains & Routes**.

   For the public embeddable board, add this required Worker route mapping on
   the `segfault.site` zone (the zone must be active on Cloudflare DNS):

   | Public URL | Required Worker route | Worker handler |
   | --- | --- | --- |
   | `https://car.segfault.site/api/leaderboard.svg` | `car.segfault.site/api/leaderboard.svg` | `/api/leaderboard.svg` alias of `/v1/leaderboard.svg` |

   Select the `roady-leaderboard` Worker for that route. Use the exact path
   (no trailing `*` is needed for this endpoint); query strings such as
   `?condition=2&limit=10` still match. Ensure `car.segfault.site` has a proxied
   DNS record so the route can execute. Do not configure an origin redirect:
   the Worker should directly return the cacheable SVG bytes.

   Put the Worker's API base URL (for example
   `https://roady-leaderboard.<subdomain>.workers.dev`) into GitHub variable
   `LEADERBOARD_BASE_URL`; the workflow uses it for health and JSON API smoke
   tests. The route above separately provides the public SVG URL.

### A.2 API token for the leaderboard workflow

In Cloudflare open **My Profile → API Tokens → Create Token → Create Custom
Token** and grant exactly:

- **Permissions**
  - `Account` / `Workers Scripts` / `Edit` — deploy the Worker and install secrets
  - `Account` / `D1` / `Edit` — apply migrations to the remote database
  - `Account` / `Account Settings` / `Read` — resolve the account id
- **Account Resources:** include the account that owns the Worker and D1 database

This token is **separate** from the Pages token. Do not reuse a Pages-only
token; it lacks Workers/D1 Edit and the workflow will fail at the migrate/deploy
step. Store it as GitHub secret `CLOUDFLARE_API_TOKEN`.

### A.3 GitHub variables (non-secret)

Add these under **Settings → Secrets and variables → Actions → Variables**:

| Variable | Purpose | Example |
| --- | --- | --- |
| `CLOUDFLARE_D1_DATABASE_ID` | Real D1 database id from `wrangler d1 create`. Replaces the placeholder in `wrangler.toml` at deploy time. | `a1b2c3d4-…` |
| `LEADERBOARD_BASE_URL` | Deployed Worker URL used by the post-deploy smoke test (`/healthz` and `/v1/leaderboard`). No trailing slash. | `https://roady-leaderboard.example.workers.dev` |

### A.4 GitHub secrets

Add these under **Settings → Secrets and variables → Actions → Secrets**. The
workflow never echoes secret values; it pipes them over stdin to
`wrangler secret put` and only checks that each is non-empty.

| Secret | Used for | How to generate |
| --- | --- | --- |
| `CLOUDFLARE_API_TOKEN` | Authenticating Wrangler in CI | Cloudflare API token (§A.2) |
| `CLOUDFLARE_ACCOUNT_ID` | Cloudflare account id | Dashboard account overview |
| `LB_SESSION_HMAC_KEY` | Worker-issued session-proof HMAC key (≥32 random bytes, base64) | `openssl rand -base64 32` |
| `LB_IP_HASH_PEPPER` | Pepper mixed with client IP before SHA-256 hashing (≥32 random bytes, base64) | `openssl rand -base64 32` |
| `LB_ADMIN_TOKEN` | Bearer token for moderation endpoints | `openssl rand -hex 32` |
| `LB_TURNSTILE_SECRET` | Cloudflare Turnstile secret key for the widget | Turnstile dashboard |
| `ROADY_LEADERBOARD_CLIENT_HMAC_KEY` | Nuisance client submission HMAC key (≥32 random bytes). Also injected into the production WASM build by the Pages workflow. Installed into the Worker as `LB_CLIENT_HMAC_KEY`; the two **must be identical**. | `openssl rand -base64 32` |

`ROADY_LEADERBOARD_CLIENT_HMAC_KEY` is the single source of truth for the
client/Worker shared key: the leaderboard workflow installs it under the
Worker binding name `LB_CLIENT_HMAC_KEY`, and the Pages workflow embeds it
into the WASM at build time. Using one secret for both guarantees they match.

### A.5 GitHub permissions

The workflow declares only:

```yaml
permissions:
  contents: read
```

It needs no `id-token: write` (Cloudflare is authenticated with an API token,
not OIDC), no `packages: write`, and no write access to the repository.
Environment protection rules can be added to the `production` environment in
repository settings if deployment approval is desired; the secrets/variables
above are repository-scoped.

### A.6 What the workflow does, in order

1. **Install, typecheck, test** — `npm ci`, `npm run typecheck` (`tsc --noEmit`),
   `npm test` (`vitest run`) inside `leaderboard/`. A failing typecheck or test
   aborts before any Cloudflare call.
2. **Verify prerequisites** — checks that `CLOUDFLARE_D1_DATABASE_ID` is set and
   not the placeholder, that `LEADERBOARD_BASE_URL` is set, and that every
   required secret is non-empty. If anything is missing it prints an `::error::`
   block listing exactly what to create and exits 1. Secret values are never
   printed.
3. **Inject D1 id** — `sed` substitutes the placeholder in the CI copy of
   `leaderboard/wrangler.toml` with `$CLOUDFLARE_D1_DATABASE_ID`. The committed
   file is untouched.
4. **Apply D1 migrations** — `wrangler d1 migrations apply roady_leaderboard --remote`.
   Idempotent: Wrangler tracks applied migrations in a `d1_migrations` table.
5. **Deploy Worker** — `wrangler deploy` creates the Worker on first run and
   updates it thereafter, including the D1 binding and the rate-limit bindings
   declared under `[[unsafe.bindings]]`.
6. **Install secrets** — pipes each secret over stdin to `wrangler secret put`.
   Re-runs are idempotent and keep the Worker in sync with rotated GitHub
   secrets.
7. **Smoke test** — `curl GET /healthz` (expects `{"ok":true,...}`) and
   `curl GET /v1/leaderboard?limit=1` (expects an `entries` array). Both must
   return HTTP 200 or the workflow fails with diagnostics. Session/score
   endpoints are not exercised because they require a browser-issued Turnstile
   token and client HMAC signature.

### A.7 Running the workflow

Push a change under `leaderboard/**` (or to the workflow file itself) to
`master`, or open **Actions → Deploy Cloudflare Leaderboard Worker → Run
workflow**. Watch the **Verify deployment prerequisites** step first — if any
variable/secret is missing it stops the run with a complete checklist before
touching Cloudflare.

### A.8 Public SVG route verification

After configuring the route, verify both the direct Worker handler and public
mapping. The response must be SVG and advertise the long edge-cache policy:

```sh
curl -sS -D - -o /dev/null 'https://car.segfault.site/api/leaderboard.svg?limit=10'
# Content-Type: image/svg+xml
# Cache-Control: public, max-age=60, s-maxage=300, stale-while-revalidate=600
```

Supported parameters are optional `condition=0..4` and `limit=1..25` (default
10). It uses the same `status='live'`, score-descending, earliest-tie ordering
as the JSON leaderboard and the same public-read rate-limit binding.

### A.9 Local leaderboard development

```sh
cd leaderboard
npm install
cp .dev.vars.example .dev.vars   # fill in random LOCAL values; never commit .dev.vars
npm run db:migrate:local         # apply migrations to the local Miniflare D1
npm run dev                      # wrangler dev on http://localhost:8787
npm test                         # vitest unit tests
npm run typecheck                # tsc --noEmit
```

`.dev.vars` holds local-only secrets; the committed `.dev.vars.example` uses
the Turnstile always-pass test key. Never put production secrets in `.dev.vars`.

### A.10 Troubleshooting

| Symptom | Likely cause / fix |
| --- | --- |
| Prerequisite step fails | A variable/secret is missing or `CLOUDFLARE_D1_DATABASE_ID` is still the placeholder. The step log lists exactly what to add. |
| `d1 migrations apply` fails | The database id is wrong, the database was deleted, or the token lacks D1:Edit. Re-run `wrangler d1 list` and confirm `CLOUDFLARE_D1_DATABASE_ID`. |
| `wrangler deploy` fails on `unsafe.bindings` rate-limit bindings | The account may not have Cloudflare Rate Limiting enabled for the binding type. Confirm the bindings in `wrangler.toml` and that rate limiting is available on the account. |
| `/healthz` smoke test fails | The Worker did not deploy, `LEADERBOARD_BASE_URL` is wrong, or the Worker threw on startup. Check `wrangler tail`. |
| `/v1/leaderboard` smoke test fails (non-200) | The D1 binding is missing, migrations did not apply, or the `DB` binding name in `wrangler.toml` is wrong. Check `wrangler d1 list`, the migrations step, and `wrangler tail`. |
| `https://car.segfault.site/api/leaderboard.svg` returns the site/404 | The required exact Worker route `car.segfault.site/api/leaderboard.svg` is absent, assigned to the wrong Worker, or the hostname's DNS record is not proxied. Check **Domains & Routes** and Cloudflare DNS. |
| `turnstile_failed` on session creation | `LB_TURNSTILE_SECRET` is missing, still a placeholder, or the wrong widget secret. |
| Score submission `401 invalid_signature` | `ROADY_LEADERBOARD_CLIENT_HMAC_KEY` (installed as Worker `LB_CLIENT_HMAC_KEY`) does not match the key embedded in the deployed WASM. Rebuild the site and re-run the leaderboard workflow so both use the same GitHub secret. |

---

## Part B — Static game site (Cloudflare Pages)

The game itself is a static WASM site deployed as a Cloudflare Pages **Direct
Upload** project by [`deploy-cloudflare-pages.yml`](.github/workflows/deploy-cloudflare-pages.yml).

### B.1 Create the Pages project

```sh
npx wrangler@4 login
npx wrangler@4 pages project create roady-car --production-branch main
```

This creates a Direct Upload Pages project named `roady-car` with `main` as the
production-branch label. To use another name, replace `roady-car` here and set
the repository variable `CLOUDFLARE_PAGES_PROJECT` to the same value.

Alternatively, in the dashboard select **Workers & Pages → Create application →
Pages → Upload assets**. Do not connect Cloudflare's Git integration; GitHub
Actions performs the builds and uploads.

### B.2 Pages API token

In Cloudflare open **My Profile → API Tokens → Create Token → Create Custom
Token**:

- **Permissions:** `Account` / `Cloudflare Pages` / `Edit`
- **Account Resources:** include the account that owns the Pages project

This token is separate from the leaderboard token. Store it as
`CLOUDFLARE_API_TOKEN` — **note**: GitHub secrets are namespaced per workflow
file, but a repository secret named `CLOUDFLARE_API_TOKEN` is shared. If you
need different tokens for Pages and the leaderboard, use environment-scoped
secrets (one per `environment`) or distinct secret names and reference the
correct one in each workflow. The shipped workflows both reference
`secrets.CLOUDFLARE_API_TOKEN`, so either use a single token with the union of
both permission sets, or fork the secret into environment-scoped values.

### B.3 Pages GitHub secrets/variables

Secrets:

- `CLOUDFLARE_API_TOKEN` — Pages: Edit token (see scoping note above)
- `CLOUDFLARE_ACCOUNT_ID` — owning Cloudflare account id
- `ROADY_LEADERBOARD_CLIENT_HMAC_KEY` — random ≥32-byte nuisance client signing
  key injected into production WASM; the same value is installed into the
  leaderboard Worker as `LB_CLIENT_HMAC_KEY` by the leaderboard workflow

Variables:

- `CLOUDFLARE_PAGES_PROJECT` — Pages project name (defaults to `roady-car`)
- `LEADERBOARD_BASE_URL` — public Worker base URL embedded as `LEADERBOARD_API_URL`
- `LB_TURNSTILE_SITE_KEY` — public Turnstile widget site key embedded in the web client

If either leaderboard variable is absent, the client intentionally degrades to
an unavailable/read-only state rather than attempting a broken submission.

### B.4 Deploy the site

Push to `master`, or open **Actions → Deploy to Cloudflare Pages → Run
workflow**. The workflow builds and validates `dist/`, uploads it as an
artifact, then runs:

```sh
npx wrangler@4 pages deploy dist --project-name "$PROJECT_NAME" --branch main
```

The canonical game URL is **https://car.segfault.site** (with the Pages default
URL available as a fallback); each deployment also gets
an immutable preview URL in the workflow log.

### B.5 Local production build

```sh
rustup target add wasm32-unknown-unknown
cargo install --locked trunk --version 0.21.14
trunk build --release --cargo-profile wasm-release
python tools/check_release.py
```

To upload manually:

```sh
npx wrangler@4 pages deploy dist --project-name roady-car --branch main
```

### B.6 Custom domain

In the dashboard open **Workers & Pages → the project → Custom domains → Set
up a custom domain**, enter the hostname, and follow the DNS prompts. For a
domain already on Cloudflare DNS, Cloudflare can add the required record. For
external DNS, add the CNAME target Cloudflare shows and complete any ownership
verification.

### B.7 Base-path caveat

The Trunk public URL is relative (`./`), which works for the Pages root and
keeps emitted asset links relative when the site is served below a subpath. If
hosting or routing changes, preserve that relative base or deliberately update
the public URL and verify asset, worker, and browser-navigation paths before
deploying.

---

## Effective round-time cap (score-plausibility reference)

The score-plausibility caps in `leaderboard/wrangler.toml` (`SCORE_CAPS_JSON`)
and the validation ranges in `leaderboard/src/validation.ts` are derived from
the shipped rules. The **effective maximum round length is 99 seconds**, not
90:

- Each fresh round starts with **60 seconds**.
- Ordinary coins add **+1.5 seconds** each, with the round clock clamped to
  **90 seconds** (`MAX_ROUND_TIME = 90.0` in `src/world.rs`).
- The Time power-up adds **+5 seconds** each, clamped to a hard ceiling of
  **99 seconds** (`TIME_CAP = 99.0` in `src/pickups.rs`). Coins alone cannot
  reach 99s; the Time power-up can push the clock from 90s up to 99s.

So a round can last up to 99 seconds, and `round_duration_ms` / `time_left_ms`
in a submission can legitimately approach `99_000 ms`. The Worker's validation
rejects `round_duration_ms` and `time_left_ms` above `120_000 ms` as a broad
sane-range guard, while the plausibility caps on `terminal_total` are the
primary score bound. Keep this 99s ceiling in mind when tuning
`SCORE_CAPS_JSON`: the maximum plausible pickup count grows with the extra
~9 seconds of play that the Time power-up can add.

## Security note on the client HMAC key

`ROADY_LEADERBOARD_CLIENT_HMAC_KEY` is a **nuisance-only** barrier, not proof
of honest gameplay. The public WASM client is attacker-controlled and the key
is recoverable from it. The leaderboard's real defenses are Turnstile,
one-time D1 sessions, Worker-signed proofs, rate limits, plausibility caps,
hashed-IP attribution, and moderation — see
[LEADERBOARD_ARCHITECTURE.md](LEADERBOARD_ARCHITECTURE.md) §1 for the full
threat model. Never treat a valid client signature as evidence the score was
earned.
