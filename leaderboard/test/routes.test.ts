// Fetch-route integration tests for the Roady Car leaderboard Worker.
//
// These tests exercise the default export `fetch` handler end-to-end against
// an in-memory D1 fake (test/helpers.ts FakeD1), fake rate-limit bindings, and
// mocked Turnstile/siteverify. They cover CORS preflight, fail-closed config
// guards, health, public leaderboard (origin-safe caching), session creation,
// score submission (error + success orchestration), personal rank, and
// moderation (error + success). No Miniflare runtime is required.

import { describe, expect, it, beforeAll, afterAll, vi } from "vitest";
import worker from "../src/index";
import type { Env } from "../src/index";
import {
  FakeD1,
  FakeRateLimit,
  TEST_CLIENT_KEY,
  makeEnv,
  sampleScoreBody,
  seedSession,
  signScore,
  validateOk,
} from "./helpers";

// The default origin used by tests (must be in ALLOWED_ORIGINS).
const ORIGIN = "http://localhost:8080";
const DISALLOWED_ORIGIN = "https://evil.example.com";

// ─── Mock Cache API (caches.default) for Node environment ─────────────────────
// The Cloudflare Cache API is not available in the Node test environment.
// We provide a minimal in-memory cache that stores Responses by URL.

class FakeCache {
  private store = new Map<string, Response>();
  async match(request: Request | string): Promise<Response | undefined> {
    const url = typeof request === "string" ? request : request.url;
    const cached = this.store.get(url);
    if (cached) return cached.clone();
    return undefined;
  }
  async put(request: Request | string, response: Response): Promise<void> {
    const url = typeof request === "string" ? request : request.url;
    this.store.set(url, response.clone());
  }
  async delete(request: Request | string): Promise<boolean> {
    const url = typeof request === "string" ? request : request.url;
    return this.store.delete(url);
  }
}

const fakeCaches = { default: new FakeCache() };
// Inject into the global scope so `caches.default` works in tests.
(globalThis as unknown as { caches: unknown }).caches = fakeCaches;

/** A minimal ExecutionContext that collects waitUntil promises. */
class FakeCtx {
  promises: Promise<unknown>[] = [];
  passThroughOnException(): void {}
  exports = {};
  props = {};
  tracing = undefined;
  async waitUntil<T>(p: Promise<T>): Promise<T> {
    this.promises.push(p);
    return p;
  }
}

/** Issue a fetch against the worker and return the Response. */
async function fetchRoute(
  env: Env,
  method: string,
  path: string,
  opts: {
    body?: unknown;
    headers?: Record<string, string>;
    origin?: string;
  } = {},
): Promise<{ response: Response; ctx: FakeCtx }> {
  const url = new URL(`https://roady-leaderboard.test${path}`);
  const init: RequestInit = { method };
  const headers = new Headers(opts.headers);
  if (opts.origin !== undefined) {
    if (opts.origin === null) {
      // no Origin header
    } else {
      headers.set("Origin", opts.origin);
    }
  } else {
    headers.set("Origin", ORIGIN);
  }
  if (opts.body !== undefined) {
    init.body = JSON.stringify(opts.body);
    headers.set("Content-Type", "application/json");
  }
  init.headers = headers;
  const request = new Request(url.toString(), init);
  const ctx = new FakeCtx();
  const response = await worker.fetch(
    request,
    env,
    ctx as unknown as ExecutionContext,
  );
  // Drain any waitUntil promises (cache.put etc.).
  await Promise.allSettled(ctx.promises);
  return { response, ctx };
}

/** Read the JSON body of a Response. */
async function readJson<T = unknown>(r: Response): Promise<T> {
  return (await r.json()) as T;
}

// ─── Mock Turnstile siteverify ───────────────────────────────────────────────

// We mock global fetch so Turnstile siteverify returns a controlled response.
// The always-pass test secret bypasses fetch entirely; these mocks apply only
// when a real (non-test) secret is configured.
const originalFetch = globalThis.fetch;

function mockTurnstile(
  response: { success: boolean; action?: string; hostname?: string },
): void {
  globalThis.fetch = vi.fn(async (input: RequestInfo | URL) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    if (url.includes("challenges.cloudflare.com/turnstile")) {
      return new Response(JSON.stringify(response), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    }
    return originalFetch(input as RequestInfo);
  }) as unknown as typeof fetch;
}

function restoreFetch(): void {
  globalThis.fetch = originalFetch;
}

// ─── CORS tests ──────────────────────────────────────────────────────────────

describe("fetch-route: CORS", () => {
  it("preflight from an allowed origin returns 204 with per-origin CORS headers", async () => {
    const env = makeEnv() as unknown as Env;
    const { response } = await fetchRoute(env, "OPTIONS", "/v1/leaderboard", {
      origin: ORIGIN,
      headers: {
        "Access-Control-Request-Method": "GET",
        "Access-Control-Request-Headers": "X-Roady-Client-Signature",
      },
    });
    expect(response.status).toBe(204);
    expect(response.headers.get("Access-Control-Allow-Origin")).toBe(ORIGIN);
    expect(response.headers.get("Vary")).toBe("Origin");
    // X-Roady-Client-Signature must be in the allowed headers (review blocker).
    const allowHeaders = response.headers.get("Access-Control-Allow-Headers") ?? "";
    expect(allowHeaders).toContain("X-Roady-Client-Signature");
  });

  it("preflight from a disallowed origin returns 204 without CORS allow-origin", async () => {
    const env = makeEnv() as unknown as Env;
    const { response } = await fetchRoute(env, "OPTIONS", "/v1/leaderboard", {
      origin: DISALLOWED_ORIGIN,
    });
    expect(response.status).toBe(204);
    expect(response.headers.get("Access-Control-Allow-Origin")).toBeNull();
  });

  it("non-preflight from a disallowed origin gets no CORS headers on the body", async () => {
    const env = makeEnv() as unknown as Env;
    const { response } = await fetchRoute(env, "GET", "/healthz", {
      origin: DISALLOWED_ORIGIN,
    });
    expect(response.headers.get("Access-Control-Allow-Origin")).toBeNull();
  });
});

// ─── Config fail-closed tests ────────────────────────────────────────────────

describe("fetch-route: fail-closed config", () => {
  it("returns 503 config_error when a secret is a placeholder", async () => {
    const env = makeEnv({ LB_ADMIN_TOKEN: "REPLACE_WITH_RANDOM" }) as unknown as Env;
    const { response } = await fetchRoute(env, "GET", "/v1/leaderboard");
    expect(response.status).toBe(503);
    const body = await readJson<{ error: { code: string } }>(response);
    expect(body.error.code).toBe("config_error");
  });

  it("returns 503 config_error when a cap is missing", async () => {
    const env = makeEnv({
      SCORE_CAPS_JSON: JSON.stringify({ "0": 3000, "1": 3000, "2": 4000, "3": 3000 }),
    }) as unknown as Env;
    const { response } = await fetchRoute(env, "GET", "/v1/leaderboard");
    expect(response.status).toBe(503);
    const body = await readJson<{ error: { code: string } }>(response);
    expect(body.error.code).toBe("config_error");
  });

  it("returns 503 when Turnstile test secret is used with a non-dev BUILD", async () => {
    const env = makeEnv({ BUILD: "1.0.0" }) as unknown as Env;
    const { response } = await fetchRoute(env, "POST", "/v1/session", {
      body: { condition: 0, turnstileToken: "tok" },
    });
    expect(response.status).toBe(503);
    const body = await readJson<{ error: { code: string } }>(response);
    expect(body.error.code).toBe("config_error");
  });

  it("healthz still works even when config is broken", async () => {
    const env = makeEnv({ LB_ADMIN_TOKEN: "REPLACE_WITH_RANDOM" }) as unknown as Env;
    const { response } = await fetchRoute(env, "GET", "/healthz");
    expect(response.status).toBe(200);
    const body = await readJson<{ ok: boolean }>(response);
    expect(body.ok).toBe(true);
  });
});

// ─── Health ──────────────────────────────────────────────────────────────────

describe("fetch-route: health", () => {
  it("GET /healthz returns ok with build and time", async () => {
    const env = makeEnv() as unknown as Env;
    const { response } = await fetchRoute(env, "GET", "/healthz");
    expect(response.status).toBe(200);
    const body = await readJson<{ ok: boolean; build: string; time: number }>(response);
    expect(body.ok).toBe(true);
    expect(body.build).toBe("dev");
    expect(typeof body.time).toBe("number");
  });
});

// ─── Leaderboard ─────────────────────────────────────────────────────────────

describe("fetch-route: leaderboard", () => {
  it("GET /v1/leaderboard returns entries with CORS headers", async () => {
    const db = new FakeD1();
    db.scores.push(
      { id: 1, name: "AAA", condition: 0, terminal_total: 100, chickens: 60, coins: 40, objective_completed: 1, max_combo: 3, round_duration_ms: 60000, time_left_ms: 0, game_over_reason: "time_up", build: "0.1.0", platform: "web", session_id: "s1", submitted_at: 1000, ip_hash: "h", status: "live", moderation_note: null },
      { id: 2, name: "BBB", condition: 0, terminal_total: 200, chickens: 100, coins: 100, objective_completed: 1, max_combo: 4, round_duration_ms: 60000, time_left_ms: 0, game_over_reason: "time_up", build: "0.1.0", platform: "web", session_id: "s2", submitted_at: 2000, ip_hash: "h", status: "live", moderation_note: null },
    );
    const env = makeEnv({ DB: db }) as unknown as Env;
    const { response } = await fetchRoute(env, "GET", "/v1/leaderboard?condition=0&limit=10");
    expect(response.status).toBe(200);
    expect(response.headers.get("Access-Control-Allow-Origin")).toBe(ORIGIN);
    const body = await readJson<{ entries: { rank: number; name: string; score: number }[] }>(response);
    expect(body.entries).toHaveLength(2);
    // Sorted DESC by total: BBB (200) first, AAA (100) second.
    expect(body.entries[0]!.name).toBe("BBB");
    expect(body.entries[0]!.rank).toBe(1);
    expect(body.entries[1]!.name).toBe("AAA");
    expect(body.entries[1]!.rank).toBe(2);
  });

  it("rejects invalid condition with 422", async () => {
    const env = makeEnv() as unknown as Env;
    const { response } = await fetchRoute(env, "GET", "/v1/leaderboard?condition=9");
    expect(response.status).toBe(422);
    const body = await readJson<{ error: { code: string } }>(response);
    expect(body.error.code).toBe("invalid_condition");
  });

  it("rejects invalid limit with 422", async () => {
    const env = makeEnv() as unknown as Env;
    const { response } = await fetchRoute(env, "GET", "/v1/leaderboard?limit=0");
    expect(response.status).toBe(422);
    const body = await readJson<{ error: { code: string } }>(response);
    expect(body.error.code).toBe("invalid_limit");
  });

  it("returns 404 for unknown routes", async () => {
    const env = makeEnv() as unknown as Env;
    const { response } = await fetchRoute(env, "GET", "/nope");
    expect(response.status).toBe(404);
  });
});

// ─── Session creation ────────────────────────────────────────────────────────

describe("fetch-route: session", () => {
  it("POST /v1/session creates a session with the always-pass test secret (dev)", async () => {
    const db = new FakeD1();
    const env = makeEnv({ DB: db }) as unknown as Env;
    const { response } = await fetchRoute(env, "POST", "/v1/session", {
      body: { condition: 0, turnstileToken: "test-token" },
    });
    expect(response.status).toBe(200);
    const body = await readJson<{
      sessionId: string; challenge: string; condition: number; expiresAt: number; proof: string;
    }>(response);
    expect(body.sessionId).toBeTruthy();
    expect(body.challenge).toBeTruthy();
    expect(body.condition).toBe(0);
    expect(typeof body.proof).toBe("string");
    expect(db.sessions.has(body.sessionId)).toBe(true);
    expect(response.headers.get("Cache-Control")).toBe("no-store");
  });

  it("rejects missing turnstile token with 422", async () => {
    const env = makeEnv() as unknown as Env;
    const { response } = await fetchRoute(env, "POST", "/v1/session", {
      body: { condition: 0 },
    });
    expect(response.status).toBe(422);
    const body = await readJson<{ error: { code: string } }>(response);
    expect(body.error.code).toBe("invalid_turnstile");
  });

  it("rejects malformed JSON with 422", async () => {
    const env = makeEnv() as unknown as Env;
    const url = new URL("https://roady-leaderboard.test/v1/session");
    const request = new Request(url.toString(), {
      method: "POST",
      headers: { Origin: ORIGIN, "Content-Type": "application/json" },
      body: "{not json",
    });
    const ctx = new FakeCtx();
    const response = await worker.fetch(request, env, ctx as unknown as ExecutionContext);
    expect(response.status).toBe(422);
    const body = await readJson<{ error: { code: string } }>(response);
    expect(body.error.code).toBe("invalid_body");
  });

  it("rejects oversized body with 422", async () => {
    const env = makeEnv() as unknown as Env;
    const big = { condition: 0, turnstileToken: "x".repeat(20 * 1024) };
    const { response } = await fetchRoute(env, "POST", "/v1/session", { body: big });
    expect(response.status).toBe(422);
    const body = await readJson<{ error: { code: string } }>(response);
    expect(body.error.code).toBe("invalid_body");
  });
});

// ─── Score submission ────────────────────────────────────────────────────────

describe("fetch-route: score submission", () => {
  it("rejects submission without X-Roady-Client-Signature header with 401", async () => {
    const db = new FakeD1();
    const seeded = await seedSession(db);
    const env = makeEnv({ DB: db }) as unknown as Env;
    const body = sampleScoreBody({
      sessionId: seeded.sessionId,
      proof: seeded.proof,
      condition: seeded.condition,
    });
    const { response } = await fetchRoute(env, "POST", "/v1/scores", { body });
    expect(response.status).toBe(401);
    const rbody = await readJson<{ error: { code: string } }>(response);
    expect(rbody.error.code).toBe("missing_signature");
  });

  it("rejects submission with a malformed signature with 401", async () => {
    const db = new FakeD1();
    const seeded = await seedSession(db);
    const env = makeEnv({ DB: db }) as unknown as Env;
    const body = sampleScoreBody({
      sessionId: seeded.sessionId,
      proof: seeded.proof,
      condition: seeded.condition,
    });
    const { response } = await fetchRoute(env, "POST", "/v1/scores", {
      body,
      headers: { "X-Roady-Client-Signature": "not!valid" },
    });
    expect(response.status).toBe(401);
    const rbody = await readJson<{ error: { code: string } }>(response);
    expect(rbody.error.code).toBe("invalid_signature");
  });

  it("rejects submission with a valid-format but wrong-key signature with 401", async () => {
    const db = new FakeD1();
    const seeded = await seedSession(db);
    const env = makeEnv({ DB: db }) as unknown as Env;
    const v = validateOk(
      sampleScoreBody({
        sessionId: seeded.sessionId,
        proof: seeded.proof,
        condition: seeded.condition,
      }),
    );
    const wrongSig = await signScore("wrong-client-key", v);
    const { response } = await fetchRoute(env, "POST", "/v1/scores", {
      body: sampleScoreBody({
        sessionId: seeded.sessionId,
        proof: seeded.proof,
        condition: seeded.condition,
      }),
      headers: { "X-Roady-Client-Signature": wrongSig },
    });
    expect(response.status).toBe(401);
    const rbody = await readJson<{ error: { code: string } }>(response);
    expect(rbody.error.code).toBe("invalid_signature");
  });

  it("rejects submission for an unknown session with 404", async () => {
    const env = makeEnv() as unknown as Env;
    const v = validateOk(sampleScoreBody({ sessionId: "nonexistent" }));
    const sig = await signScore(TEST_CLIENT_KEY, v);
    const { response } = await fetchRoute(env, "POST", "/v1/scores", {
      body: sampleScoreBody({ sessionId: "nonexistent" }),
      headers: { "X-Roady-Client-Signature": sig },
    });
    expect(response.status).toBe(404);
    const rbody = await readJson<{ error: { code: string } }>(response);
    expect(rbody.error.code).toBe("invalid_session");
  });

  it("rejects a replay (already-used session) with 409", async () => {
    const db = new FakeD1();
    const seeded = await seedSession(db);
    // Mark as used.
    db.sessions.get(seeded.sessionId)!.used = 1;
    const env = makeEnv({ DB: db }) as unknown as Env;
    const v = validateOk(
      sampleScoreBody({
        sessionId: seeded.sessionId,
        proof: seeded.proof,
        condition: seeded.condition,
      }),
    );
    const sig = await signScore(TEST_CLIENT_KEY, v);
    const { response } = await fetchRoute(env, "POST", "/v1/scores", {
      body: sampleScoreBody({
        sessionId: seeded.sessionId,
        proof: seeded.proof,
        condition: seeded.condition,
      }),
      headers: { "X-Roady-Client-Signature": sig },
    });
    expect(response.status).toBe(409);
    const rbody = await readJson<{ error: { code: string } }>(response);
    expect(rbody.error.code).toBe("replay");
  });

  it("rejects a condition mismatch with 409", async () => {
    const db = new FakeD1();
    const seeded = await seedSession(db, { condition: 0 });
    const env = makeEnv({ DB: db }) as unknown as Env;
    // Submit with condition 1 but session is condition 0.
    const v = validateOk(
      sampleScoreBody({
        sessionId: seeded.sessionId,
        proof: seeded.proof,
        condition: 1,
        terminal_total: 42, chickens: 30, coins: 12,
      }),
    );
    const sig = await signScore(TEST_CLIENT_KEY, v);
    const { response } = await fetchRoute(env, "POST", "/v1/scores", {
      body: sampleScoreBody({
        sessionId: seeded.sessionId,
        proof: seeded.proof,
        condition: 1,
      }),
      headers: { "X-Roady-Client-Signature": sig },
    });
    expect(response.status).toBe(409);
    const rbody = await readJson<{ error: { code: string } }>(response);
    expect(rbody.error.code).toBe("condition_mismatch");
  });

  it("successfully submits a valid score and returns rank 201", async () => {
    const db = new FakeD1();
    const seeded = await seedSession(db, { condition: 0 });
    const env = makeEnv({ DB: db }) as unknown as Env;
    const bodyFields = {
      sessionId: seeded.sessionId,
      proof: seeded.proof,
      condition: seeded.condition,
    };
    const v = validateOk(sampleScoreBody(bodyFields));
    const sig = await signScore(TEST_CLIENT_KEY, v);
    const { response } = await fetchRoute(env, "POST", "/v1/scores", {
      body: sampleScoreBody(bodyFields),
      headers: { "X-Roady-Client-Signature": sig },
    });
    expect(response.status).toBe(201);
    const rbody = await readJson<{
      inserted: boolean; rank: number; globalRank: number; total: number;
    }>(response);
    expect(rbody.inserted).toBe(true);
    expect(rbody.rank).toBe(1); // first score in condition 0
    expect(rbody.globalRank).toBe(1); // first score globally
    expect(rbody.total).toBe(42);
    expect(response.headers.get("Cache-Control")).toBe("no-store");
    // Session is now used.
    expect(db.sessions.get(seeded.sessionId)!.used).toBe(1);
  });

  it("computes correct rank when other live scores exist", async () => {
    const db = new FakeD1();
    // Seed two existing higher scores.
    db.scores.push(
      { id: 1, name: "AAA", condition: 0, terminal_total: 100, chickens: 60, coins: 40, objective_completed: 1, max_combo: 3, round_duration_ms: 60000, time_left_ms: 0, game_over_reason: "time_up", build: "0.1.0", platform: "web", session_id: "s1", submitted_at: 1000, ip_hash: "h", status: "live", moderation_note: null },
      { id: 2, name: "BBB", condition: 0, terminal_total: 50, chickens: 30, coins: 20, objective_completed: 1, max_combo: 2, round_duration_ms: 60000, time_left_ms: 0, game_over_reason: "time_up", build: "0.1.0", platform: "web", session_id: "s2", submitted_at: 2000, ip_hash: "h", status: "live", moderation_note: null },
    );
    const seeded = await seedSession(db, { condition: 0 });
    const env = makeEnv({ DB: db }) as unknown as Env;
    // Submit a score of 30 (lower than both existing).
    const v = validateOk(
      sampleScoreBody({
        sessionId: seeded.sessionId,
        proof: seeded.proof,
        condition: 0,
        terminal_total: 30, chickens: 20, coins: 10, max_combo: 2,
      }),
    );
    const sig = await signScore(TEST_CLIENT_KEY, v);
    const { response } = await fetchRoute(env, "POST", "/v1/scores", {
      body: sampleScoreBody({
        sessionId: seeded.sessionId,
        proof: seeded.proof,
        condition: 0,
        terminal_total: 30, chickens: 20, coins: 10, max_combo: 2,
      }),
      headers: { "X-Roady-Client-Signature": sig },
    });
    expect(response.status).toBe(201);
    const rbody = await readJson<{ rank: number; globalRank: number }>(response);
    // 30 was submitted after two higher scores in the same condition.
    expect(rbody.rank).toBe(3);
    expect(rbody.globalRank).toBe(3);
  });

  it("returns condition rank 1 but a lower global rank across conditions", async () => {
    const db = new FakeD1();
    db.scores.push(
      { id: 1, name: "HIGH", condition: 1, terminal_total: 100, chickens: 60, coins: 40, objective_completed: 1, max_combo: 3, round_duration_ms: 60000, time_left_ms: 0, game_over_reason: "time_up", build: "0.1.0", platform: "web", session_id: "s1", submitted_at: 1000, ip_hash: "h", status: "live", moderation_note: null },
    );
    const seeded = await seedSession(db, { condition: 0 });
    const env = makeEnv({ DB: db }) as unknown as Env;
    const fields = {
      sessionId: seeded.sessionId,
      proof: seeded.proof,
      condition: 0,
      terminal_total: 30,
      chickens: 20,
      coins: 10,
      max_combo: 2,
    };
    const sig = await signScore(TEST_CLIENT_KEY, validateOk(sampleScoreBody(fields)));
    const { response } = await fetchRoute(env, "POST", "/v1/scores", {
      body: sampleScoreBody(fields),
      headers: { "X-Roady-Client-Signature": sig },
    });

    expect(response.status).toBe(201);
    const rbody = await readJson<{ rank: number; globalRank: number }>(response);
    expect(rbody.rank).toBe(1);
    expect(rbody.globalRank).toBe(2);
  });

  it("rejects an implausible combo (max_combo 5 with total 1) with 422", async () => {
    const db = new FakeD1();
    const seeded = await seedSession(db);
    const env = makeEnv({ DB: db }) as unknown as Env;
    const { response } = await fetchRoute(env, "POST", "/v1/scores", {
      body: sampleScoreBody({
        sessionId: seeded.sessionId,
        proof: seeded.proof,
        terminal_total: 1, chickens: 1, coins: 0, max_combo: 5,
      }),
    });
    expect(response.status).toBe(422);
    const rbody = await readJson<{ error: { code: string } }>(response);
    expect(rbody.error.code).toBe("implausible_combo");
  });
});

// ─── Personal rank ───────────────────────────────────────────────────────────

describe("fetch-route: personal rank", () => {
  it("GET /v1/me/rank requires sessionId", async () => {
    const env = makeEnv() as unknown as Env;
    const { response } = await fetchRoute(env, "GET", "/v1/me/rank");
    expect(response.status).toBe(422);
    const body = await readJson<{ error: { code: string } }>(response);
    expect(body.error.code).toBe("invalid_session");
  });

  it("GET /v1/me/rank returns 404 for unknown session", async () => {
    const env = makeEnv() as unknown as Env;
    const { response } = await fetchRoute(env, "GET", "/v1/me/rank?sessionId=nonexistent");
    expect(response.status).toBe(404);
  });

  it("GET /v1/me/rank returns rank for a used session", async () => {
    const db = new FakeD1();
    db.scores.push(
      { id: 1, name: "AAA", condition: 0, terminal_total: 100, chickens: 60, coins: 40, objective_completed: 1, max_combo: 3, round_duration_ms: 60000, time_left_ms: 0, game_over_reason: "time_up", build: "0.1.0", platform: "web", session_id: "sess-rank", submitted_at: 1000, ip_hash: "h", status: "live", moderation_note: null },
    );
    db.sessions.set("sess-rank", {
      session_id: "sess-rank", challenge: "c", condition: 0, proof: "p",
      expires_at: Date.now() + 60000, used: 1, turnstile_verified: 1, ip_hash: "h",
    });
    const env = makeEnv({ DB: db }) as unknown as Env;
    const { response } = await fetchRoute(env, "GET", "/v1/me/rank?sessionId=sess-rank");
    expect(response.status).toBe(200);
    const body = await readJson<{ rank: number; entry: { name: string } }>(response);
    expect(body.rank).toBe(1);
    expect(body.entry.name).toBe("AAA");
    expect(response.headers.get("Cache-Control")).toBe("private, no-store");
  });
});

// ─── Moderation ──────────────────────────────────────────────────────────────

describe("fetch-route: moderation", () => {
  it("rejects missing admin token with 401", async () => {
    const env = makeEnv() as unknown as Env;
    const { response } = await fetchRoute(env, "POST", "/v1/admin/scores/1/hide");
    expect(response.status).toBe(401);
    const body = await readJson<{ error: { code: string } }>(response);
    expect(body.error.code).toBe("unauthorized");
  });

  it("rejects wrong admin token with 401", async () => {
    const env = makeEnv() as unknown as Env;
    const { response } = await fetchRoute(env, "POST", "/v1/admin/scores/1/hide", {
      headers: { Authorization: "Bearer wrong-token" },
    });
    expect(response.status).toBe(401);
  });

  it("returns 503 when admin token is a placeholder (defense in depth)", async () => {
    const env = makeEnv({ LB_ADMIN_TOKEN: "REPLACE_WITH_RANDOM" }) as unknown as Env;
    // configError would already catch this, but test the admin guard directly:
    const { response } = await fetchRoute(env, "POST", "/v1/admin/scores/1/hide", {
      headers: { Authorization: "Bearer REPLACE_WITH_RANDOM" },
    });
    // configError fires first at the entry → 503 config_error.
    expect(response.status).toBe(503);
    const body = await readJson<{ error: { code: string } }>(response);
    expect(body.error.code).toBe("config_error");
  });

  it("hides a live score with a valid admin token", async () => {
    const db = new FakeD1();
    db.scores.push(
      { id: 5, name: "AAA", condition: 0, terminal_total: 100, chickens: 60, coins: 40, objective_completed: 1, max_combo: 3, round_duration_ms: 60000, time_left_ms: 0, game_over_reason: "time_up", build: "0.1.0", platform: "web", session_id: "s5", submitted_at: 1000, ip_hash: "h", status: "live", moderation_note: null },
    );
    const env = makeEnv({ DB: db }) as unknown as Env;
    const adminToken = (env as unknown as Record<string, string>).LB_ADMIN_TOKEN;
    const { response } = await fetchRoute(env, "POST", "/v1/admin/scores/5/hide", {
      headers: { Authorization: `Bearer ${adminToken}` },
    });
    expect(response.status).toBe(200);
    const body = await readJson<{ ok: boolean; id: number; status: string }>(response);
    expect(body.ok).toBe(true);
    expect(body.id).toBe(5);
    expect(body.status).toBe("hidden");
    expect(db.scores[0]!.status).toBe("hidden");
    expect(db.modLog).toHaveLength(1);
  });

  it("deletes a live score with DELETE", async () => {
    const db = new FakeD1();
    db.scores.push(
      { id: 6, name: "AAA", condition: 0, terminal_total: 100, chickens: 60, coins: 40, objective_completed: 1, max_combo: 3, round_duration_ms: 60000, time_left_ms: 0, game_over_reason: "time_up", build: "0.1.0", platform: "web", session_id: "s6", submitted_at: 1000, ip_hash: "h", status: "live", moderation_note: null },
    );
    const env = makeEnv({ DB: db }) as unknown as Env;
    const adminToken = (env as unknown as Record<string, string>).LB_ADMIN_TOKEN;
    const { response } = await fetchRoute(env, "DELETE", "/v1/admin/scores/6", {
      headers: { Authorization: `Bearer ${adminToken}` },
    });
    expect(response.status).toBe(200);
    const body = await readJson<{ status: string }>(response);
    expect(body.status).toBe("deleted");
    expect(db.scores[0]!.status).toBe("deleted");
  });

  it("returns 404 when hiding a non-existent score", async () => {
    const env = makeEnv() as unknown as Env;
    const adminToken = (env as unknown as Record<string, string>).LB_ADMIN_TOKEN;
    const { response } = await fetchRoute(env, "POST", "/v1/admin/scores/999/hide", {
      headers: { Authorization: `Bearer ${adminToken}` },
    });
    expect(response.status).toBe(404);
  });

  it("rejects invalid id with 422", async () => {
    const env = makeEnv() as unknown as Env;
    const adminToken = (env as unknown as Record<string, string>).LB_ADMIN_TOKEN;
    const { response } = await fetchRoute(env, "POST", "/v1/admin/scores/abc/hide", {
      headers: { Authorization: `Bearer ${adminToken}` },
    });
    expect(response.status).toBe(422);
  });
});

// ─── Rate limiting ───────────────────────────────────────────────────────────

describe("fetch-route: rate limiting", () => {
  it("returns 429 when the read rate limit is exceeded", async () => {
    const env = makeEnv({ RATE_LIMIT_READ: new FakeRateLimit(1) }) as unknown as Env;
    // First request OK.
    const r1 = await fetchRoute(env, "GET", "/v1/leaderboard");
    expect(r1.response.status).toBe(200);
    // Second request limited.
    const r2 = await fetchRoute(env, "GET", "/v1/leaderboard");
    expect(r2.response.status).toBe(429);
    const body = await readJson<{ error: { code: string } }>(r2.response);
    expect(body.error.code).toBe("rate_limited");
  });

  it("write rate limiter fails closed when binding errors", async () => {
    const env = makeEnv({ RATE_LIMIT_SESSION: new FakeRateLimit(3, true) }) as unknown as Env;
    const { response } = await fetchRoute(env, "POST", "/v1/session", {
      body: { condition: 0, turnstileToken: "tok" },
    });
    expect(response.status).toBe(429);
    const body = await readJson<{ error: { code: string } }>(response);
    expect(body.error.code).toBe("rate_limited");
  });

  it("write rate limiter fails closed when binding is absent", async () => {
    const env = makeEnv() as unknown as Env;
    // Remove the submit binding entirely.
    delete (env as unknown as Record<string, unknown>).RATE_LIMIT_SUBMIT;
    const db = new FakeD1();
    (env as unknown as Record<string, unknown>).DB = db;
    const { response } = await fetchRoute(env, "POST", "/v1/scores", {
      body: sampleScoreBody(),
    });
    // Should be 429 (fail closed) not 401 (missing signature) — rate limit fires first.
    expect(response.status).toBe(429);
  });
});

// ─── Turnstile hostname/action validation ────────────────────────────────────

describe("fetch-route: Turnstile validation", () => {
  beforeAll(() => {
    // We use a non-test secret so the fetch path is exercised.
  });
  afterAll(() => {
    restoreFetch();
  });

  it("rejects Turnstile response with wrong action", async () => {
    mockTurnstile({ success: true, action: "wrong-action", hostname: "localhost" });
    const db = new FakeD1();
    const env = makeEnv({
      DB: db,
      BUILD: "dev",
      LB_TURNSTILE_SECRET: "1x0000000000000000000000000000000BB", // non-test secret
    }) as unknown as Env;
    const { response } = await fetchRoute(env, "POST", "/v1/session", {
      body: { condition: 0, turnstileToken: "tok" },
    });
    expect(response.status).toBe(422);
    const body = await readJson<{ error: { code: string } }>(response);
    expect(body.error.code).toBe("turnstile_failed");
  });

  it("rejects Turnstile response with missing hostname", async () => {
    mockTurnstile({ success: true, action: "roady_score_session" });
    const env = makeEnv({
      BUILD: "dev",
      LB_TURNSTILE_SECRET: "1x0000000000000000000000000000000BB",
    }) as unknown as Env;
    const { response } = await fetchRoute(env, "POST", "/v1/session", {
      body: { condition: 0, turnstileToken: "tok" },
    });
    expect(response.status).toBe(422);
    const body = await readJson<{ error: { code: string } }>(response);
    expect(body.error.code).toBe("turnstile_failed");
  });

  it("accepts Turnstile response with correct action and hostname", async () => {
    mockTurnstile({ success: true, action: "roady_score_session", hostname: "localhost" });
    const db = new FakeD1();
    const env = makeEnv({
      DB: db,
      BUILD: "dev",
      LB_TURNSTILE_SECRET: "1x0000000000000000000000000000000BB",
    }) as unknown as Env;
    const { response } = await fetchRoute(env, "POST", "/v1/session", {
      body: { condition: 0, turnstileToken: "tok" },
    });
    expect(response.status).toBe(200);
    restoreFetch();
  });
});

// ─── Scheduled cleanup ───────────────────────────────────────────────────────

describe("fetch-route: scheduled cleanup preserves rank", () => {
  it("hides old non-top scores but preserves ranks of live scores", async () => {
    const db = new FakeD1();
    const now = Date.now();
    const old = now - 100 * 24 * 60 * 60 * 1000; // 100 days ago
    // Add enough old scores that some fall outside the top 1000.
    // Top score (id 1) stays in top 1000; the rest (ids 2..1002) are old
    // and low-scoring, so they fall outside top 1000 and get hidden.
    db.scores.push(
      { id: 1, name: "TOP", condition: 0, terminal_total: 5000, chickens: 2500, coins: 2500, objective_completed: 1, max_combo: 5, round_duration_ms: 60000, time_left_ms: 0, game_over_reason: "time_up", build: "0.1.0", platform: "web", session_id: "s1", submitted_at: old, ip_hash: "h", status: "live", moderation_note: null },
    );
    for (let i = 2; i <= 1002; i++) {
      db.scores.push({
        id: i, name: `O${i}`, condition: 0, terminal_total: 10, chickens: 5, coins: 5,
        objective_completed: 0, max_combo: 1, round_duration_ms: 60000, time_left_ms: 0,
        game_over_reason: "time_up", build: "0.1.0", platform: "web", session_id: `s${i}`,
        submitted_at: old, ip_hash: "h", status: "live", moderation_note: null,
      });
    }
    const env = makeEnv({ DB: db }) as unknown as Env;
    const ctx = new FakeCtx();
    await worker.scheduled({} as ScheduledEvent, env, ctx as unknown as ExecutionContext);
    // Await any waitUntil promises (the cleanup runs inside waitUntil).
    await Promise.allSettled(ctx.promises);
    // Top score stays live (it's in the top 1000).
    expect(db.scores[0]!.status).toBe("live");
    // The lowest old score (id 1002) is outside top 1000 and hidden.
    expect(db.scores[1001]!.status).toBe("hidden");
  });
});
