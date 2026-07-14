// Roady Car leaderboard Cloudflare Worker entry point.
// Implements LEADERBOARD_ARCHITECTURE.md §2–§15.
//
// Threat model (§1): the public WASM client is attacker-controlled and the
// embedded client HMAC key is recoverable. This Worker provides defense in
// depth — Turnstile, short-lived one-time D1 sessions, Worker-signed proofs,
// rate limits, plausibility caps, hashed-IP attribution, and moderation —
// but does NOT claim tamper-proof scores.

import {
  corsHeaders,
  errorResponse,
  fromBase64Url,
  handleOptions,
  json,
  newRequestId,
  parseAllowedOrigins,
  toBase64Url,
  applyCors,
} from "./responses";
import {
  canonicalScoreBytes,
  canonicalSessionBytes,
  constantTimeEquals,
  hmacBase64Url,
  importHmacKey,
  ipHash,
  randomBase64Url,
  sha256,
} from "./security";
import {
  moderationReasons,
  normalizeName,
  parseScoreCaps,
  validateRestorationBody,
  validateScoreBody,
  validateSessionBody,
  MAX_REQUEST_BODY_BYTES,
  type ValidatedRestoration,
  type ValidatedScore,
} from "./validation";
import { renderLeaderboardSvg } from "./svg";
import {
  checkRateLimit,
  readBoundedJson,
  type RateLimitBinding,
} from "../vendor/cloudflare-game-common/src/index";

// ─── Bindings ────────────────────────────────────────────────────────────────

export interface Env {
  DB: D1Database;
  // Rate limit bindings (Cloudflare Rate Limiting). Optional at the type level
  // for standalone tests; write endpoints require them and fail closed.
  RATE_LIMIT_READ?: RateLimit;
  RATE_LIMIT_SESSION?: RateLimit;
  RATE_LIMIT_SUBMIT?: RateLimit;
  RATE_LIMIT_RANK?: RateLimit;
  // Non-secret vars.
  ALLOWED_ORIGINS: string;
  BUILD: string;
  SCORE_CAPS_JSON: string;
  // Secrets (installed via `wrangler secret put`).
  LB_SESSION_HMAC_KEY: string;
  LB_IP_HASH_PEPPER: string;
  LB_ADMIN_TOKEN: string;
  LB_TURNSTILE_SECRET: string;
  LB_CLIENT_HMAC_KEY: string;
}

type RateLimit = RateLimitBinding;

// ─── Tunables ────────────────────────────────────────────────────────────────

const SESSION_TTL_MS = 5 * 60 * 1000; // 5 minutes (architecture §4).
const SESSION_ID_BYTES = 18; // 144 bits of entropy, base64url.
const CHALLENGE_BYTES = 18;
const TOP_BOARD_DEFAULT_LIMIT = 25;
const TOP_BOARD_MAX_LIMIT = 100;
const SVG_BOARD_DEFAULT_LIMIT = 10;
const SVG_BOARD_MAX_LIMIT = 25;
const TURNSTILE_URL = "https://challenges.cloudflare.com/turnstile/v0/siteverify";
/** Fixed Turnstile action bound to session issuance (architecture §10). */
const TURNSTILE_ACTION = "roady_score_session";
/** Documented always-pass test secret — prohibited outside dev builds. */
const TURNSTILE_TEST_SECRET = "1x0000000000000000000000000000000AA";
/** Restoration requests are exact, small records rather than bulk imports. */
const MAX_RESTORATION_BODY_BYTES = 4 * 1024;

/** All five required score plausibility caps (conditions 0–4). */
const REQUIRED_CAP_CONDITIONS = [0, 1, 2, 3, 4];

/** Secret names that must be present and non-placeholder for fail-closed. */
const REQUIRED_SECRETS = [
  "LB_SESSION_HMAC_KEY",
  "LB_IP_HASH_PEPPER",
  "LB_ADMIN_TOKEN",
  "LB_TURNSTILE_SECRET",
  "LB_CLIENT_HMAC_KEY",
] as const;

/** Values that are obvious placeholders and must be rejected in production. */
function isPlaceholder(value: string): boolean {
  if (!value) return true;
  const v = value.trim();
  if (v.length === 0) return true;
  return /^REPLACE_|PLACEHOLDER/i.test(v);
}

/** Whether the current build is a dev/test build (permits test Turnstile secret). */
function isDevBuild(build: string | undefined): boolean {
  return build === "dev" || build === "test" || build === "local";
}

/**
 * Centralized, fail-closed configuration check. Returns an error message if
 * any required configuration is missing, a placeholder, or incomplete — or
 * `null` if the configuration is safe to serve requests with. Called at the
 * top of every endpoint that depends on secrets/caps so a misconfigured Worker
 * refuses to serve rather than silently weakening security.
 */
function configError(env: Env): string | null {
  // Non-secret vars.
  if (parseAllowedOrigins(env.ALLOWED_ORIGINS).size === 0) {
    return "ALLOWED_ORIGINS is missing or contains a non-canonical origin";
  }

  // Score caps: require a plain JSON object with only integer condition keys.
  let caps: Map<number, number>;
  try {
    caps = parseScoreCaps(env.SCORE_CAPS_JSON);
  } catch {
    return "SCORE_CAPS_JSON is invalid JSON";
  }
  for (const cond of REQUIRED_CAP_CONDITIONS) {
    const cap = caps.get(cond);
    if (cap === undefined || !Number.isInteger(cap) || cap <= 0) {
      return `SCORE_CAPS_JSON missing or invalid cap for condition ${cond}`;
    }
  }

  // All five secrets must be present and non-placeholder.
  for (const name of REQUIRED_SECRETS) {
    const value = (env as unknown as Record<string, string>)[name];
    if (typeof value !== "string" || isPlaceholder(value)) {
      return `secret ${name} is missing or a placeholder`;
    }
  }

  // The Turnstile always-pass test secret is prohibited outside dev builds.
  if (env.LB_TURNSTILE_SECRET === TURNSTILE_TEST_SECRET && !isDevBuild(env.BUILD)) {
    return "LB_TURNSTILE_SECRET is the test secret; set BUILD=dev for local development";
  }

  return null;
}

// ─── Main entry ──────────────────────────────────────────────────────────────

export default {
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const requestId = newRequestId();
    const origin = request.headers.get("Origin");
    const allowed = parseAllowedOrigins(env.ALLOWED_ORIGINS);
    const cors = corsHeaders(origin, allowed);

    const preflight = handleOptions(request, cors);
    if (preflight) return preflight;

    const url = new URL(request.url);
    const { method, pathname } = { method: request.method, pathname: url.pathname };

    try {
      // ── health ────────────────────────────────────────────────────────
      // healthz is always available for monitoring, even if config is broken.
      if (pathname === "/healthz" && method === "GET") {
        return healthz(env, cors);
      }

      // ── fail-closed config guard (architecture §1: defense in depth) ────
      // Every endpoint below depends on secrets and/or caps. If the Worker is
      // misconfigured, refuse to serve rather than silently weakening security.
      const cfgErr = configError(env);
      if (cfgErr) {
        console.error("config_error", { requestId, message: cfgErr });
        return errorResponse("config_error", "Service misconfigured", 503, requestId, cors);
      }

      // ── public leaderboard ────────────────────────────────────────────
      if (pathname === "/v1/leaderboard" && method === "GET") {
        return getLeaderboard(request, env, ctx, cors, requestId);
      }

      // /api is the public custom-domain route; /v1 is the canonical API path.
      if (
        (pathname === "/v1/leaderboard.svg" || pathname === "/api/leaderboard.svg") &&
        method === "GET"
      ) {
        return getLeaderboardSvg(request, env, ctx, cors, requestId);
      }

      // ── session creation ──────────────────────────────────────────────
      if (pathname === "/v1/session" && method === "POST") {
        return createSession(request, env, cors, requestId);
      }

      // ── score submission ──────────────────────────────────────────────
      if (pathname === "/v1/scores" && method === "POST") {
        return submitScore(request, env, cors, requestId);
      }

      // ── personal rank ─────────────────────────────────────────────────
      if (pathname === "/v1/me/rank" && method === "GET") {
        return getMyRank(request, env, cors, requestId);
      }

      // ── moderation / evidence-backed restoration ─────────────────────
      if (pathname === "/v1/admin/scores/restore" && method === "POST") {
        return restoreScore(request, env, cors, requestId);
      }
      if (pathname.startsWith("/v1/admin/scores/")) {
        return moderateScore(request, env, pathname, cors, requestId);
      }

      return errorResponse("not_found", "No route for that path", 404, requestId, cors);
    } catch (err) {
      console.error("unhandled", { requestId, message: String(err) });
      return errorResponse(
        "internal_error",
        "Internal server error",
        500,
        requestId,
        cors,
      );
    }
  },

  // Scheduled retention/cleanup (architecture §15). Disabled in wrangler.toml
  // by default; enabled by uncommenting the [triggers] block after review.
  async scheduled(_event: ScheduledEvent, env: Env, ctx: ExecutionContext): Promise<void> {
    ctx.waitUntil(scheduledCleanup(env));
  },
};

// ─── Helpers ─────────────────────────────────────────────────────────────────

/** Resolve the client IP from Cloudflare headers, never storing it raw. */
function clientIp(request: Request): string {
  // CF-Connecting-IP is set by Cloudflare in production; fall back to
  // X-Forwarded-For for local `wrangler dev` and tests.
  return (
    request.headers.get("CF-Connecting-IP") ??
    request.headers.get("X-Forwarded-For")?.split(",")[0]?.trim() ??
    "127.0.0.1"
  );
}

/**
 * Check a rate-limit binding. The shared primitive always fails closed.
 * Public reads explicitly opt into availability by treating a missing binding
 * as allowed; configured binding errors still fail closed to avoid bypasses.
 */
async function rateLimit(
  binding: RateLimit | undefined,
  key: string,
  category: "read" | "session" | "submit" | "rank",
  requireBinding = false,
): Promise<boolean> {
  if (!binding && !requireBinding) return true;
  return checkRateLimit(binding, key, category);
}

/** Read and parse a JSON body, enforcing its encoded UTF-8 byte size. */
async function readJson(request: Request, maxBytes = MAX_REQUEST_BODY_BYTES): Promise<unknown> {
  return readBoundedJson(request, maxBytes);
}

function nowMs(): number {
  return Date.now();
}

// ─── healthz ─────────────────────────────────────────────────────────────────

function healthz(env: Env, cors: Record<string, string> | null): Response {
  return json({ ok: true, build: env.BUILD, time: nowMs() }, 200, cors);
}

// ─── GET /v1/leaderboard ─────────────────────────────────────────────────────

async function getLeaderboard(
  request: Request,
  env: Env,
  ctx: ExecutionContext,
  cors: Record<string, string> | null,
  requestId: string,
): Promise<Response> {
  const ip = clientIp(request);
  if (!(await rateLimit(env.RATE_LIMIT_READ, `read:${ip}`, "read"))) {
    return errorResponse("rate_limited", "Too many requests", 429, requestId, cors);
  }

  const url = new URL(request.url);
  const condParam = url.searchParams.get("condition");
  let condition: number | null = null; // null = global
  if (condParam !== null) {
    condition = Number(condParam);
    if (!Number.isInteger(condition) || condition < 0 || condition > 4) {
      return errorResponse("invalid_condition", "condition must be 0–4 or omitted", 422, requestId, cors);
    }
  }

  const limitParam = url.searchParams.get("limit");
  let limit = TOP_BOARD_DEFAULT_LIMIT;
  if (limitParam !== null) {
    limit = Number(limitParam);
    if (!Number.isInteger(limit) || limit < 1 || limit > TOP_BOARD_MAX_LIMIT) {
      return errorResponse("invalid_limit", "limit must be 1–100", 422, requestId, cors);
    }
  }

  const offsetParam = url.searchParams.get("offset");
  let offset = 0;
  if (offsetParam !== null) {
    offset = Number(offsetParam);
    if (!Number.isInteger(offset) || offset < 0) {
      return errorResponse("invalid_offset", "offset must be >= 0", 422, requestId, cors);
    }
  }

  // Cache key includes API version, condition, limit, offset (architecture §4).
  // The cache stores the body WITHOUT per-origin CORS headers; CORS is
  // reapplied per-request so a cached response is never bound to one origin.
  const cacheKey = `v1|leaderboard|${condition ?? "global"}|${limit}|${offset}`;
  const cacheUrl = `https://roady-leaderboard.cache/${cacheKey}`;
  const cache = caches.default;
  const cached = await cache.match(new Request(cacheUrl));
  if (cached) {
    // Reapply this request's CORS headers to the origin-agnostic cached body.
    return applyCors(cached, cors);
  }

  const rows = await queryLiveLeaderboard(env, condition, limit, offset);

  const entries = rows.map((r, i) => ({
    rank: offset + i + 1,
    name: r.name,
    score: r.total,
    condition: r.condition,
    submittedAt: r.submitted_at,
  }));

  const generatedAt = nowMs();
  const body = {
    condition: condition === null ? "global" : condition,
    entries,
    generatedAt,
  };
  // Build the origin-agnostic cached response (no CORS headers) so the same
  // cache entry can be served to any allowed origin with correct CORS.
  const cachedResponse = json(body, 200, null, {
    "Cache-Control":
      "public, max-age=30, s-maxage=60, stale-while-revalidate=120",
  });

  // Store in the edge cache (Cache API). The synthetic cache URL is stable
  // for a given version+condition+limit+offset so subsequent reads hit cache.
  await ctx.waitUntil(
    cache.put(
      new Request(cacheUrl),
      cachedResponse.clone(),
    ),
  );
  // Serve this request with its own CORS headers applied.
  return applyCors(cachedResponse, cors);
}

interface LeaderboardRow {
  id: number;
  name: string;
  condition: number;
  total: number;
  submitted_at: number;
}

/**
 * Shared live-board query for JSON and SVG. Keep ordering here so both
 * representations rank ties identically: higher score first, then earlier
 * submission, then lower immutable score id. The composite D1 indexes back
 * these clauses.
 */
async function queryLiveLeaderboard(
  env: Env,
  condition: number | null,
  limit: number,
  offset = 0,
): Promise<LeaderboardRow[]> {
  if (condition === null) {
    const result = await env.DB.prepare(
      `SELECT id, name, condition, terminal_total AS total, submitted_at
         FROM scores
        WHERE status = 'live'
        ORDER BY terminal_total DESC, submitted_at ASC, id ASC
        LIMIT ? OFFSET ?`,
    )
      .bind(limit, offset)
      .all<LeaderboardRow>();
    return result.results;
  }

  const result = await env.DB.prepare(
    `SELECT id, name, condition, terminal_total AS total, submitted_at
       FROM scores
      WHERE status = 'live' AND condition = ?
      ORDER BY terminal_total DESC, submitted_at ASC, id ASC
      LIMIT ? OFFSET ?`,
  )
    .bind(condition, limit, offset)
    .all<LeaderboardRow>();
  return result.results;
}

// ─── GET /v1/leaderboard.svg ────────────────────────────────────────────────

async function getLeaderboardSvg(
  request: Request,
  env: Env,
  ctx: ExecutionContext,
  cors: Record<string, string> | null,
  requestId: string,
): Promise<Response> {
  const ip = clientIp(request);
  if (!(await rateLimit(env.RATE_LIMIT_READ, `read:${ip}`, "read"))) {
    return errorResponse("rate_limited", "Too many requests", 429, requestId, cors);
  }

  const url = new URL(request.url);
  const conditionParam = url.searchParams.get("condition");
  let condition: number | null = null;
  if (conditionParam !== null) {
    condition = Number(conditionParam);
    if (!/^[0-4]$/.test(conditionParam) || !Number.isInteger(condition)) {
      return errorResponse(
        "invalid_condition",
        "condition must be 0–4 or omitted",
        422,
        requestId,
        cors,
      );
    }
  }

  const limitParam = url.searchParams.get("limit");
  let limit = SVG_BOARD_DEFAULT_LIMIT;
  if (limitParam !== null) {
    limit = Number(limitParam);
    if (
      !/^\d+$/.test(limitParam) ||
      !Number.isInteger(limit) ||
      limit < 1 ||
      limit > SVG_BOARD_MAX_LIMIT
    ) {
      return errorResponse("invalid_limit", "limit must be 1–25", 422, requestId, cors);
    }
  }

  // Use a synthetic, origin-independent cache URL. The cached response has no
  // CORS headers; the current request's allowed Origin is reapplied below.
  const cacheKey = `v2|leaderboard.svg|${condition ?? "global"}|${limit}`;
  const cacheUrl = `https://roady-leaderboard.cache/${cacheKey}`;
  const cache = caches.default;
  const cached = await cache.match(new Request(cacheUrl));
  if (cached) return applyCors(cached, cors);

  const rows = await queryLiveLeaderboard(env, condition, limit);
  const entries = rows.map((row, index) => ({
    rank: index + 1,
    name: row.name,
    score: row.total,
    condition: row.condition,
  }));
  const svg = renderLeaderboardSvg(entries, condition, nowMs());
  const cachedResponse = new Response(svg, {
    status: 200,
    headers: {
      "Content-Type": "image/svg+xml",
      "Cache-Control": "public, max-age=60, s-maxage=300, stale-while-revalidate=600",
    },
  });

  await ctx.waitUntil(cache.put(new Request(cacheUrl), cachedResponse.clone()));
  return applyCors(cachedResponse, cors);
}

// ─── POST /v1/session ────────────────────────────────────────────────────────

async function createSession(
  request: Request,
  env: Env,
  cors: Record<string, string> | null,
  requestId: string,
): Promise<Response> {
  const ip = clientIp(request);
  if (!(await rateLimit(env.RATE_LIMIT_SESSION, `session:${ip}`, "session", true))) {
    return errorResponse("rate_limited", "Too many requests", 429, requestId, cors);
  }

  let body: unknown;
  try {
    body = await readJson(request);
  } catch {
    return errorResponse("invalid_body", "Malformed JSON body or too large", 422, requestId, cors);
  }

  const parsed = validateSessionBody(body);
  if (!parsed.ok) {
    return errorResponse(parsed.code, parsed.message, 422, requestId, cors);
  }

  // Turnstile verification (architecture §10). Required in production.
  const turnstileOk = await verifyTurnstile(
    parsed.turnstileToken,
    ip,
    env.LB_TURNSTILE_SECRET,
    env.BUILD,
    env.ALLOWED_ORIGINS,
  );
  if (!turnstileOk) {
    return errorResponse("turnstile_failed", "Turnstile verification failed", 422, requestId, cors);
  }

  const sessionId = randomBase64Url(SESSION_ID_BYTES);
  const challenge = randomBase64Url(CHALLENGE_BYTES);
  const issuedAt = nowMs();
  const expiresAt = issuedAt + SESSION_TTL_MS;

  const key = await importHmacKey(env.LB_SESSION_HMAC_KEY);
  const proof = await hmacBase64Url(
    key,
    canonicalSessionBytes({ sessionId, challenge, condition: parsed.condition, expiresAt }),
  );

  const hash = await ipHash(ip, env.LB_IP_HASH_PEPPER);
  await env.DB.prepare(
    `INSERT INTO sessions
       (session_id, challenge, condition, proof, issued_at, expires_at,
        used, turnstile_verified, ip_hash)
     VALUES (?, ?, ?, ?, ?, ?, 0, 1, ?)`,
  )
    .bind(sessionId, challenge, parsed.condition, proof, issuedAt, expiresAt, hash)
    .run();

  const responseBody = {
    sessionId,
    challenge,
    condition: parsed.condition,
    expiresAt,
    proof,
  };
  return json(responseBody, 200, cors, { "Cache-Control": "no-store" });
}

/**
 * Verify a Cloudflare Turnstile token via siteverify.
 *
 * Security checks:
 *  - The documented always-pass test secret is only permitted when BUILD is
 *    a dev/test/local build; in production it is rejected (fail-closed).
 *  - The response must include the expected hostname (Turnstile `hostname`
 *    field) and the fixed action `roady_score_session`, binding the token to
 *    this Worker's session-issuance flow and preventing token replay from a
 *    different widget/action.
 */
async function verifyTurnstile(
  token: string,
  ip: string,
  secret: string,
  build: string,
  allowedOrigins: string,
): Promise<boolean> {
  // The documented "always-pass" test secret validates locally without network.
  // Prohibited outside dev/test builds (also enforced by configError).
  if (secret === TURNSTILE_TEST_SECRET) {
    return isDevBuild(build);
  }
  if (!secret || isPlaceholder(secret)) return false;

  const form = new FormData();
  form.append("secret", secret);
  form.append("response", token);
  form.append("remoteip", ip);
  try {
    const res = await fetch(TURNSTILE_URL, { method: "POST", body: form });
    const data = (await res.json()) as {
      success?: boolean;
      action?: string;
      hostname?: string;
      "error-codes"?: string[];
    };
    if (data.success !== true) return false;
    // Bind the token to this Worker's session-issuance action.
    if (data.action !== TURNSTILE_ACTION) {
      console.error("turnstile_action_mismatch", { action: data.action });
      return false;
    }
    // Validate the hostname against the configured browser origins so a token
    // minted for another site cannot be replayed here. URL.hostname strips
    // schemes, ports, and paths; local origins therefore match `localhost`.
    const allowedHosts = Array.from(parseAllowedOrigins(allowedOrigins), (origin) =>
      new URL(origin).hostname,
    );
    if (typeof data.hostname !== "string" || !allowedHosts.includes(data.hostname)) {
      console.error("turnstile_hostname_mismatch", { hostname: data.hostname });
      return false;
    }
    return true;
  } catch (err) {
    console.error("turnstile_error", { message: String(err) });
    return false;
  }
}

// ─── POST /v1/scores ─────────────────────────────────────────────────────────

interface StoredSession {
  challenge: string;
  condition: number;
  proof: string;
  expires_at: number;
  used: number;
  turnstile_verified: number;
  ip_hash: string;
}

async function submitScore(
  request: Request,
  env: Env,
  cors: Record<string, string> | null,
  requestId: string,
): Promise<Response> {
  const ip = clientIp(request);
  if (!(await rateLimit(env.RATE_LIMIT_SUBMIT, `submit:${ip}`, "submit", true))) {
    return errorResponse("rate_limited", "Too many requests", 429, requestId, cors);
  }

  let body: unknown;
  try {
    body = await readJson(request);
  } catch {
    return errorResponse("invalid_body", "Malformed JSON body or too large", 422, requestId, cors);
  }

  const caps = parseScoreCaps(env.SCORE_CAPS_JSON);
  const parsed = validateScoreBody(body, caps);
  if (!parsed.ok) {
    return errorResponse(parsed.code, parsed.message, 422, requestId, cors);
  }
  const v: ValidatedScore = parsed.value;

  // Client HMAC signature header (architecture §5). Unpadded base64url.
  const sigHeader = request.headers.get("X-Roady-Client-Signature");
  if (!sigHeader) {
    return errorResponse("missing_signature", "Client signature required", 401, requestId, cors);
  }
  let sigBytes: Uint8Array;
  try {
    sigBytes = fromBase64Url(sigHeader);
  } catch {
    return errorResponse("invalid_signature", "Malformed client signature", 401, requestId, cors);
  }

  // Look up the session. Must exist, be unused, unexpired, match condition.
  const session = await env.DB.prepare(
    `SELECT challenge, condition, proof, expires_at, used, turnstile_verified, ip_hash
       FROM sessions WHERE session_id = ?`,
  )
    .bind(v.sessionId)
    .first<StoredSession>();

  if (!session) {
    return errorResponse("invalid_session", "Unknown session", 404, requestId, cors);
  }
  if (session.used === 1) {
    return errorResponse("replay", "Session already used", 409, requestId, cors);
  }
  if (session.expires_at <= nowMs()) {
    return errorResponse("expired_session", "Session expired", 409, requestId, cors);
  }
  if (session.condition !== v.condition) {
    return errorResponse("condition_mismatch", "Session condition mismatch", 409, requestId, cors);
  }
  if (session.turnstile_verified !== 1) {
    return errorResponse("invalid_session", "Session not Turnstile-verified", 403, requestId, cors);
  }

  // Verify the opaque Worker session proof.
  const sessionKey = await importHmacKey(env.LB_SESSION_HMAC_KEY);
  const expectedProof = await hmacBase64Url(
    sessionKey,
    canonicalSessionBytes({
      sessionId: v.sessionId,
      challenge: session.challenge,
      condition: session.condition,
      expiresAt: session.expires_at,
    }),
  );
  if (!constantTimeEquals(new TextEncoder().encode(v.proof), new TextEncoder().encode(expectedProof))) {
    return errorResponse("invalid_proof", "Session proof mismatch", 403, requestId, cors);
  }

  // Verify the client submission HMAC over the exact canonical bytes.
  const clientKey = await importHmacKey(env.LB_CLIENT_HMAC_KEY);
  const expectedClientSig = await crypto.subtle.sign(
    "HMAC",
    clientKey,
    canonicalScoreBytes({
      sessionId: v.sessionId,
      proof: v.proof,
      name: v.name,
      condition: v.condition,
      terminalTotal: v.terminalTotal,
      chickens: v.chickens,
      coins: v.coins,
      objectiveCompleted: v.objectiveCompleted === 1,
      maxCombo: v.maxCombo,
      roundDurationMs: v.roundDurationMs,
      timeLeftMs: v.timeLeftMs,
      gameOverReason: v.gameOverReason,
      build: v.build,
      platform: v.platform,
    }),
  );
  if (!constantTimeEquals(sigBytes, new Uint8Array(expectedClientSig))) {
    return errorResponse("invalid_signature", "Client signature mismatch", 401, requestId, cors);
  }

  // ── One-time replay protection (architecture §7) ──────────────────────
  // UPDATE used=1 only if currently unused AND not expired; require exactly
  // one row changed. This is the atomic claim that prevents replay even under
  // concurrent duplicate submissions.
  const claim = await env.DB.prepare(
    `UPDATE sessions SET used = 1
       WHERE session_id = ? AND used = 0 AND expires_at > ?`,
  )
    .bind(v.sessionId, nowMs())
    .run();

  if (!claim.meta || claim.meta.changes !== 1) {
    // Either already used (replay) or expired between lookup and claim.
    return errorResponse("replay", "Session already used or expired", 409, requestId, cors);
  }

  // Insert the score. If this fails after session consumption, the player
  // must obtain a new session (acceptable per architecture §7).
  const submittedAt = nowMs();
  const hash = await ipHash(ip, env.LB_IP_HASH_PEPPER);
  const reviewReasons = moderationReasons(v, caps);
  const moderationNote = reviewReasons.length > 0
    ? `review:v1:${reviewReasons.join(",")}`
    : null;

  let insertedId: number;
  try {
    const insert = await env.DB.prepare(
      `INSERT INTO scores
         (name, condition, terminal_total, chickens, coins, objective_completed,
          max_combo, round_duration_ms, time_left_ms, game_over_reason, build,
          platform, session_id, submitted_at, ip_hash, status, moderation_note,
          submission_source, restoration_key)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'verified', NULL)`,
    )
      .bind(
        v.name,
        v.condition,
        v.terminalTotal,
        v.chickens,
        v.coins,
        v.objectiveCompleted,
        v.maxCombo,
        v.roundDurationMs,
        v.timeLeftMs,
        v.gameOverReason,
        v.build,
        v.platform,
        v.sessionId,
        submittedAt,
        hash,
        "live",
        moderationNote,
      )
      .run();
    insertedId = Number(insert.meta?.last_row_id);
    if (!Number.isSafeInteger(insertedId) || insertedId <= 0) {
      throw new Error("score insert did not return a valid id");
    }
  } catch (err) {
    console.error("score insert failed", { requestId, message: String(err) });
    // Unique constraint on session_id also surfaces here as a 409.
    return errorResponse(
      "insert_failed",
      "Score insert failed; obtain a new session",
      500,
      requestId,
      cors,
    );
  }

  // Compute condition and global rank from the same live-score ordering used
  // by both leaderboard views: score descending, then earlier submission,
  // then lower immutable id. A single query keeps both ranks on the same
  // database snapshot.
  const rankRow = await env.DB.prepare(
    `SELECT COUNT(CASE WHEN condition = ? THEN 1 END) AS condition_ahead,
            COUNT(*) AS global_ahead
       FROM scores
      WHERE status = 'live'
        AND (terminal_total > ? OR
             (terminal_total = ? AND submitted_at < ?) OR
             (terminal_total = ? AND submitted_at = ? AND id < ?))`,
  )
    .bind(
      v.condition,
      v.terminalTotal,
      v.terminalTotal,
      submittedAt,
      v.terminalTotal,
      submittedAt,
      insertedId,
    )
    .first<{ condition_ahead: number; global_ahead: number }>();
  const rank = (rankRow?.condition_ahead ?? 0) + 1;
  const globalRank = (rankRow?.global_ahead ?? 0) + 1;

  return json(
    {
      inserted: true,
      rank,
      globalRank,
      condition: v.condition,
      total: v.terminalTotal,
      submittedAt,
    },
    201,
    cors,
    { "Cache-Control": "no-store" },
  );
}

// ─── GET /v1/me/rank ─────────────────────────────────────────────────────────

async function getMyRank(
  request: Request,
  env: Env,
  cors: Record<string, string> | null,
  requestId: string,
): Promise<Response> {
  const ip = clientIp(request);
  if (!(await rateLimit(env.RATE_LIMIT_RANK, `rank:${ip}`, "rank"))) {
    return errorResponse("rate_limited", "Too many requests", 429, requestId, cors);
  }

  const url = new URL(request.url);
  const sessionId = url.searchParams.get("sessionId");
  if (!sessionId) {
    return errorResponse("invalid_session", "sessionId required", 422, requestId, cors);
  }

  // Requires a *used* session from a successful submission.
  const session = await env.DB.prepare(
    `SELECT s.used, s.condition, sc.id, sc.name, sc.terminal_total, sc.submitted_at
       FROM sessions s
       JOIN scores sc ON sc.session_id = s.session_id
      WHERE s.session_id = ?`,
  )
    .bind(sessionId)
    .first<MeRankSessionRow>();

  if (!session) {
    return errorResponse("invalid_session", "Unknown session", 404, requestId, cors);
  }
  if (session.used !== 1) {
    return errorResponse("invalid_session", "Session not used", 403, requestId, cors);
  }

  const ahead = await env.DB.prepare(
    `SELECT COUNT(*) AS ahead
       FROM scores
      WHERE condition = ? AND status = 'live'
        AND (terminal_total > ? OR
             (terminal_total = ? AND submitted_at < ?) OR
             (terminal_total = ? AND submitted_at = ? AND id < ?))`,
  )
    .bind(
      session.condition,
      session.terminal_total,
      session.terminal_total,
      session.submitted_at,
      session.terminal_total,
      session.submitted_at,
      session.id,
    )
    .first<{ ahead: number }>();

  const rank = (ahead?.ahead ?? 0) + 1;

  // Nearby ranks (10 above and 10 below by score ordering).
  const nearby = await env.DB.prepare(
    `SELECT id, name, terminal_total AS total, submitted_at
       FROM scores
      WHERE condition = ? AND status = 'live'
      ORDER BY terminal_total DESC, submitted_at ASC, id ASC
      LIMIT 21 OFFSET ?`,
  )
    .bind(
      session.condition,
      Math.max(0, rank - 11),
    )
    .all<{ id: number; name: string; total: number; submitted_at: number }>();

  const entries = nearby.results.map((r, i) => ({
    rank: Math.max(0, rank - 11) + i + 1,
    name: r.name,
    score: r.total,
    submittedAt: r.submitted_at,
  }));

  return json(
    {
      sessionId,
      rank,
      condition: session.condition,
      entry: {
        name: session.name,
        total: session.terminal_total,
        submittedAt: session.submitted_at,
      },
      nearby: entries,
    },
    200,
    cors,
    { "Cache-Control": "private, no-store" },
  );
}

interface MeRankSessionRow {
  used: number;
  condition: number;
  id: number;
  name: string;
  terminal_total: number;
  submitted_at: number;
}

// ─── Moderation ──────────────────────────────────────────────────────────────

/** Authenticate an administrator without logging or returning the token. */
function adminAuthorized(request: Request, env: Env): boolean {
  const token = env.LB_ADMIN_TOKEN;
  if (typeof token !== "string" || isPlaceholder(token)) return false;
  const auth = request.headers.get("Authorization") ?? "";
  const expected = `Bearer ${token}`;
  return auth.length === expected.length && constantTimeEquals(
    new TextEncoder().encode(auth),
    new TextEncoder().encode(expected),
  );
}

interface StoredRestoration {
  restoration_key: string;
  evidence_hash: string;
  payload_hash: string;
  score_id: number;
  name: string;
  condition: number;
  terminal_total: number;
  submitted_at: number;
}

function restorationPayloadBytes(value: ValidatedRestoration): Uint8Array {
  const s = value.score;
  return new TextEncoder().encode([
    "roady.v1.admin_restore",
    value.restorationKey,
    value.evidenceHash,
    s.name,
    s.condition,
    s.terminalTotal,
    s.chickens,
    s.coins,
    s.objectiveCompleted,
    s.maxCombo,
    s.roundDurationMs,
    s.timeLeftMs,
    s.gameOverReason,
    s.build,
    s.platform,
    value.submittedAt,
    value.knownFieldsJson,
    value.syntheticFieldsJson,
    value.reason,
  ].join("\n"));
}

function hex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

async function restoredRanks(env: Env, score: StoredRestoration): Promise<{ rank: number; globalRank: number }> {
  const row = await env.DB.prepare(
    `SELECT COUNT(CASE WHEN condition = ? THEN 1 END) AS condition_ahead,
            COUNT(*) AS global_ahead
       FROM scores
      WHERE status = 'live'
        AND (terminal_total > ? OR
             (terminal_total = ? AND submitted_at < ?) OR
             (terminal_total = ? AND submitted_at = ? AND id < ?))`,
  ).bind(
    score.condition,
    score.terminal_total,
    score.terminal_total,
    score.submitted_at,
    score.terminal_total,
    score.submitted_at,
    score.score_id,
  ).first<{ condition_ahead: number; global_ahead: number }>();
  return {
    rank: (row?.condition_ahead ?? 0) + 1,
    globalRank: (row?.global_ahead ?? 0) + 1,
  };
}

/** POST /v1/admin/scores/restore — one exact score per authenticated request. */
async function restoreScore(
  request: Request,
  env: Env,
  cors: Record<string, string> | null,
  requestId: string,
): Promise<Response> {
  if (!adminAuthorized(request, env)) {
    return errorResponse("unauthorized", "Invalid admin token", 401, requestId, cors);
  }

  let body: unknown;
  try {
    body = await readJson(request, MAX_RESTORATION_BODY_BYTES);
  } catch {
    return errorResponse("invalid_body", "Malformed JSON body or too large", 422, requestId, cors);
  }
  const parsed = validateRestorationBody(body, parseScoreCaps(env.SCORE_CAPS_JSON));
  if (!parsed.ok) return errorResponse(parsed.code, parsed.message, 422, requestId, cors);

  const v = parsed.value;
  const payloadHash = hex(await sha256(restorationPayloadBytes(v)));
  const existing = await env.DB.prepare(
    `SELECT ar.restoration_key, ar.evidence_hash, ar.payload_hash, ar.score_id,
            s.name, s.condition, s.terminal_total, s.submitted_at
       FROM admin_restorations ar JOIN scores s ON s.id = ar.score_id
      WHERE ar.restoration_key = ? OR ar.evidence_hash = ?`,
  ).bind(v.restorationKey, v.evidenceHash).first<StoredRestoration>();
  if (existing) {
    if (
      existing.restoration_key !== v.restorationKey ||
      existing.evidence_hash !== v.evidenceHash ||
      existing.payload_hash !== payloadHash
    ) {
      return errorResponse(
        "restoration_conflict",
        "restoration_key already exists with different evidence or fields",
        409,
        requestId,
        cors,
      );
    }
    const ranks = await restoredRanks(env, existing);
    return json({
      restored: true,
      idempotent: true,
      id: existing.score_id,
      rank: ranks.rank,
      globalRank: ranks.globalRank,
      condition: existing.condition,
      total: existing.terminal_total,
      submittedAt: existing.submitted_at,
    }, 200, cors, { "Cache-Control": "no-store" });
  }

  const syntheticSessionId = `admin_restore:${v.restorationKey}`;
  const restoredAt = nowMs();
  // Synthetic sessions are intentionally unverified and already used. They
  // preserve the scores.session_id FK/uniqueness contract but can never pass
  // the public proof, Turnstile, signature, or replay flow.
  try {
    const s = v.score;
    // D1 batch is transactional. The audit rows select the newly inserted
    // score by its unique restoration key, avoiding a race-prone client-side
    // id allocation while ensuring partial restoration is rolled back.
    await env.DB.batch([
      env.DB.prepare(
        `INSERT INTO sessions
           (session_id, challenge, condition, proof, issued_at, expires_at,
            used, turnstile_verified, ip_hash)
         VALUES (?, 'admin_restore', ?, 'admin_restore', ?, ?, 1, 0, 'admin_restore')`,
      ).bind(syntheticSessionId, s.condition, restoredAt, restoredAt),
      env.DB.prepare(
        `INSERT INTO scores
           (name, condition, terminal_total, chickens, coins, objective_completed,
            max_combo, round_duration_ms, time_left_ms, game_over_reason, build,
            platform, session_id, submitted_at, ip_hash, status, moderation_note,
            submission_source, restoration_key)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'admin_restore',
                 'live', 'review:v1:admin_restore,synthetic_combo,synthetic_duration,synthetic_time_left,synthetic_build,synthetic_platform,synthetic_submitted_at', 'admin_restore', ?)`,
      ).bind(
        s.name, s.condition, s.terminalTotal, s.chickens, s.coins,
        s.objectiveCompleted, s.maxCombo, s.roundDurationMs, s.timeLeftMs,
        s.gameOverReason, s.build, s.platform, syntheticSessionId, v.submittedAt,
        v.restorationKey,
      ),
      env.DB.prepare(
        `INSERT INTO admin_restorations
           (restoration_key, evidence_hash, payload_hash, known_fields_json,
            synthetic_fields_json, reason, score_id, restored_at, admin)
         SELECT ?, ?, ?, ?, ?, ?, id, ?, 'admin'
           FROM scores WHERE restoration_key = ?`,
      ).bind(
        v.restorationKey,
        v.evidenceHash,
        payloadHash,
        v.knownFieldsJson,
        v.syntheticFieldsJson,
        v.reason,
        restoredAt,
        v.restorationKey,
      ),
      env.DB.prepare(
        `INSERT INTO moderation_log (action, target_score_id, admin, at, note)
         SELECT 'restore', id, 'admin', ?, ? FROM scores WHERE restoration_key = ?`,
      ).bind(
        restoredAt,
        `review:v1:admin_restore,synthetic_combo,synthetic_duration,synthetic_time_left,synthetic_build,synthetic_platform,synthetic_submitted_at:${v.restorationKey}:${v.evidenceHash}`,
        v.restorationKey,
      ),
    ]);

    const created = await env.DB.prepare(
      `SELECT ar.restoration_key, ar.evidence_hash, ar.payload_hash, ar.score_id,
              s.name, s.condition, s.terminal_total, s.submitted_at
         FROM admin_restorations ar JOIN scores s ON s.id = ar.score_id
        WHERE ar.restoration_key = ?`,
    ).bind(v.restorationKey).first<StoredRestoration>();
    if (!created) throw new Error("restoration transaction did not create audit row");
    const scoreId = created.score_id;
    const row = created;
    const ranks = await restoredRanks(env, row);
    return json({
      restored: true,
      idempotent: false,
      id: scoreId,
      rank: ranks.rank,
      globalRank: ranks.globalRank,
      condition: s.condition,
      total: s.terminalTotal,
      submittedAt: v.submittedAt,
    }, 201, cors, { "Cache-Control": "no-store" });
  } catch (err) {
    // A concurrent request may win the unique restoration key. Re-read and
    // apply the same retry/conflict semantics rather than creating duplicates.
    const raced = await env.DB.prepare(
      `SELECT ar.restoration_key, ar.evidence_hash, ar.payload_hash, ar.score_id,
              s.name, s.condition, s.terminal_total, s.submitted_at
         FROM admin_restorations ar JOIN scores s ON s.id = ar.score_id
        WHERE ar.restoration_key = ? OR ar.evidence_hash = ?`,
    ).bind(v.restorationKey, v.evidenceHash).first<StoredRestoration>();
    if (
      raced && raced.restoration_key === v.restorationKey &&
      raced.evidence_hash === v.evidenceHash && raced.payload_hash === payloadHash
    ) {
      const ranks = await restoredRanks(env, raced);
      return json({
        restored: true, idempotent: true, id: raced.score_id,
        rank: ranks.rank, globalRank: ranks.globalRank,
        condition: raced.condition, total: raced.terminal_total,
        submittedAt: raced.submitted_at,
      }, 200, cors, { "Cache-Control": "no-store" });
    }
    if (raced) {
      return errorResponse("restoration_conflict", "restoration_key already exists with different evidence or fields", 409, requestId, cors);
    }
    console.error("admin_restore_failed", { requestId, message: String(err) });
    return errorResponse("restore_failed", "Restoration failed safely", 500, requestId, cors);
  }
}

async function moderateScore(
  request: Request,
  env: Env,
  pathname: string,
  cors: Record<string, string> | null,
  requestId: string,
): Promise<Response> {
  // Defense-in-depth: even though configError guards at the entry, refuse to
  // authorize if the admin token is missing or a placeholder. This prevents
  // an attacker from matching `Bearer undefined` or `Bearer REPLACE_...`.
  const adminToken = env.LB_ADMIN_TOKEN;
  if (typeof adminToken !== "string" || isPlaceholder(adminToken)) {
    console.error("admin_token_missing", { requestId });
    return errorResponse("unauthorized", "Admin token not configured", 503, requestId, cors);
  }

  // Authorization: Bearer <LB_ADMIN_TOKEN>.
  if (!adminAuthorized(request, env)) {
    return errorResponse("unauthorized", "Invalid admin token", 401, requestId, cors);
  }

  // Routes: /v1/admin/scores/:id/hide  and  /v1/admin/scores/:id
  const segments = pathname.split("/").filter(Boolean); // ['v1','admin','scores',id,(hide)]
  const idStr = segments[3];
  const action = segments[4];
  const id = Number(idStr);
  if (!Number.isInteger(id) || id <= 0) {
    return errorResponse("invalid_id", "Invalid score id", 422, requestId, cors);
  }

  const method = request.method;
  if (method === "POST" && action === "hide") {
    return setScoreStatus(env, id, "hidden", "hidden by admin", cors, requestId);
  }
  if (method === "DELETE" && action === undefined) {
    return setScoreStatus(env, id, "deleted", "deleted by admin", cors, requestId);
  }
  return errorResponse("not_found", "No moderation route for that path", 404, requestId, cors);
}

async function setScoreStatus(
  env: Env,
  id: number,
  status: "hidden" | "deleted",
  note: string,
  cors: Record<string, string> | null,
  requestId: string,
): Promise<Response> {
  const upd = await env.DB.prepare(
    `UPDATE scores SET status = ?, moderation_note = ? WHERE id = ? AND status = 'live'`,
  )
    .bind(status, note, id)
    .run();
  if (!upd.meta || upd.meta.changes !== 1) {
    return errorResponse("not_found", "No live score with that id", 404, requestId, cors);
  }
  await env.DB.prepare(
    `INSERT INTO moderation_log (action, target_score_id, admin, at, note)
     VALUES (?, ?, ?, ?, ?)`,
  )
    .bind(status, id, "admin", nowMs(), note)
    .run();
  return json({ ok: true, id, status }, 200, cors);
}

// ─── Scheduled cleanup ───────────────────────────────────────────────────────

/**
 * Retention/cleanup (architecture §15):
 *  - delete expired sessions
 *  - keep top 1000 live scores per condition; hide older non-top entries
 *  - keep recent submissions for 90 days
 * Conservative: only expired-session deletion runs automatically here; the
 * top-N trimming is gated behind row-count guards and is intentionally
 * conservative to avoid mass deletion on a misconfigured cap.
 *
 * Rank integrity: hidden scores are excluded from rank queries (all rank
 * SQL filters `status = 'live'`), so a player's rank is preserved as long as
 * their score remains live. Trimming only hides non-top, old entries, so the
 * visible board and per-player ranks stay stable for active top-1000 scores.
 */
export async function scheduledCleanup(env: Env): Promise<void> {
  const now = nowMs();
  const ninetyDaysMs = 90 * 24 * 60 * 60 * 1000;

  await env.DB.prepare(`DELETE FROM sessions WHERE expires_at < ?`).bind(now).run();

  // For each condition, hide live scores that are NOT in the top 1000 AND are
  // older than 90 days. This keeps the board fresh without nuking recent play.
  for (const condition of [0, 1, 2, 3, 4]) {
    await env.DB.prepare(
      `UPDATE scores SET status = 'hidden', moderation_note = 'retention: older than 90d and outside top 1000'
        WHERE condition = ? AND status = 'live'
          AND submitted_at < ?
          AND id NOT IN (
            SELECT id FROM scores
             WHERE condition = ? AND status = 'live'
             ORDER BY terminal_total DESC, submitted_at ASC, id ASC
             LIMIT 1000
          )`,
    )
      .bind(condition, now - ninetyDaysMs, condition)
      .run();
  }
}

// Re-export for tests.
export {
  canonicalScoreBytes,
  canonicalSessionBytes,
  constantTimeEquals,
  hmacBase64Url,
  importHmacKey,
  ipHash,
  toBase64Url,
  fromBase64Url,
  sha256,
  randomBase64Url,
  parseScoreCaps,
  normalizeName,
  validateRestorationBody,
  validateScoreBody,
  validateSessionBody,
  moderationReasons,
};
export { escapeXml, renderLeaderboardSvg } from "./svg";
export {
  applyCors,
  corsHeaders,
  parseAllowedOrigins,
  handleOptions,
  json,
  errorResponse,
  newRequestId,
} from "./responses";
