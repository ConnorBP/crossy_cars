// Test helpers for the Roady Car leaderboard. These build canonical inputs,
// sign them with known keys, and provide a minimal in-memory D1 fake for
// replay-sensitive logic that does not require a live Miniflare runtime.
//
// The pure-logic tests (canonical bytes, HMAC, names, validation, caps) run
// anywhere vitest runs. The D1 fake is used for the replay ordering test so it
// can execute without the workers pool bindings.

import {
  canonicalScoreBytes,
  canonicalSessionBytes,
  constantTimeEquals,
  hmacBase64Url,
  importHmacKey,
  toBase64Url,
} from "../src/security";
import {
  parseScoreCaps,
  validateScoreBody,
} from "../src/validation";
import type { ValidatedScore } from "../src/validation";

/** A fixed, obviously-fake test key. NEVER used outside tests. */
export const TEST_SESSION_KEY = "test-session-key-not-a-real-secret";
export const TEST_CLIENT_KEY = "test-client-key-not-a-real-secret";

export const SCORE_CAPS = parseScoreCaps(
  JSON.stringify({ "0": 3000, "1": 3000, "2": 4000, "3": 3000, "4": 6000 }),
);

/** A well-formed submission body used across tests. */
export function sampleScoreBody(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    sessionId: "sess-aaaaaaaa",
    proof: "proof-base64url",
    name: "AAA",
    condition: 0,
    terminal_total: 42,
    chickens: 30,
    coins: 12,
    objective_completed: true,
    max_combo: 4,
    round_duration_ms: 65000,
    time_left_ms: 0,
    game_over_reason: "time_up",
    build: "0.1.0",
    platform: "web",
    ...overrides,
  };
}

/** Validate a sample body and assert it is ok, returning the typed value. */
export function validateOk(
  body: Record<string, unknown>,
  caps = SCORE_CAPS,
): ValidatedScore {
  const r = validateScoreBody(body, caps);
  if (!r.ok) throw new Error(`expected ok, got ${r.code}: ${r.message}`);
  return r.value;
}

/** Sign the canonical client score bytes with the test client key. */
export async function signScore(
  keySecret: string,
  v: ValidatedScore,
): Promise<string> {
  const key = await importHmacKey(keySecret);
  const bytes = canonicalScoreBytes({
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
  });
  return hmacBase64Url(key, bytes);
}

/** Sign the canonical session proof bytes with the test session key. */
export async function signSession(
  keySecret: string,
  input: { sessionId: string; challenge: string; condition: number; expiresAt: number },
): Promise<string> {
  const key = await importHmacKey(keySecret);
  return hmacBase64Url(key, canonicalSessionBytes(input));
}

/**
 * Create a session directly in the FakeD1 (bypassing the HTTP path) with a
 * valid Worker-issued proof. Returns the session fields needed to submit a
 * score. Used by fetch-route tests to set up a valid session quickly.
 */
export async function seedSession(
  db: FakeD1,
  opts: {
    sessionId?: string;
    challenge?: string;
    condition?: number;
    ttlMs?: number;
    sessionKey?: string;
  } = {},
): Promise<{
  sessionId: string;
  challenge: string;
  condition: number;
  expiresAt: number;
  proof: string;
}> {
  const sessionId = opts.sessionId ?? `sess-${Math.random().toString(36).slice(2, 12)}`;
  const challenge = opts.challenge ?? `chal-${Math.random().toString(36).slice(2, 12)}`;
  const condition = opts.condition ?? 0;
  const issuedAt = Date.now();
  const expiresAt = issuedAt + (opts.ttlMs ?? 5 * 60 * 1000);
  const keySecret = opts.sessionKey ?? TEST_SESSION_KEY;
  const proof = await signSession(keySecret, { sessionId, challenge, condition, expiresAt });
  db.sessions.set(sessionId, {
    session_id: sessionId,
    challenge,
    condition,
    proof,
    expires_at: expiresAt,
    used: 0,
    turnstile_verified: 1,
    ip_hash: "test-hash",
  });
  return { sessionId, challenge, condition, expiresAt, proof };
}

// ─── In-memory D1 fake (replay ordering + fetch-route tests) ───────────────

interface FakeSession {
  session_id: string;
  challenge: string;
  condition: number;
  proof: string;
  expires_at: number;
  used: 0 | 1;
  turnstile_verified: 0 | 1;
  ip_hash: string;
}

interface FakeScore {
  id: number;
  name: string;
  condition: number;
  terminal_total: number;
  chickens: number;
  coins: number;
  objective_completed: 0 | 1;
  max_combo: number;
  round_duration_ms: number;
  time_left_ms: number;
  game_over_reason: string;
  build: string;
  platform: string;
  session_id: string;
  submitted_at: number;
  ip_hash: string;
  status: "live" | "hidden" | "deleted";
  moderation_note: string | null;
}

interface FakeModLog {
  id: number;
  action: string;
  target_score_id: number;
  admin: string;
  at: number;
  note: string | null;
}

/**
 * An in-memory D1 subset that supports all the statements used by the Worker's
 * endpoints: session lookup/insert/claim, score insert/select/count, and
 * moderation status updates + log inserts. It reports `meta.changes` for
 * UPDATE/INSERT, mirroring D1. This lets fetch-route tests run in a plain
 * Node vitest environment without Miniflare.
 */
export class FakeD1 {
  sessions = new Map<string, FakeSession>();
  scores: FakeScore[] = [];
  modLog: FakeModLog[] = [];
  nextId = 1;
  nextModId = 1;

  prepare(sql: string) {
    return new FakeStmt(this, sql.trim());
  }
}

class FakeStmt {
  constructor(private db: FakeD1, private sql: string) {}
  private params: unknown[] = [];

  bind(...params: unknown[]): FakeStmt {
    this.params = params;
    return this;
  }

  async first<T>(): Promise<T | null> {
    const sql = this.sql;
    // Session lookup by id (submitScore).
    if (sql.includes("FROM sessions WHERE session_id") && sql.startsWith("SELECT")) {
      const id = this.params[0] as string;
      const s = this.db.sessions.get(id);
      return (s as unknown as T) ?? null;
    }
    // Condition + global rank counts returned after score submission.
    if (sql.includes("AS condition_ahead") && sql.includes("AS global_ahead")) {
      const condition = this.params[0] as number;
      const total = this.params[1] as number;
      const total2 = this.params[2] as number;
      const submittedAt = this.params[3] as number;
      const ahead = this.db.scores.filter(
        (s) =>
          s.status === "live" &&
          (s.terminal_total > total ||
            (s.terminal_total === total2 && s.submitted_at < submittedAt)),
      );
      return {
        condition_ahead: ahead.filter((s) => s.condition === condition).length,
        global_ahead: ahead.length,
      } as unknown as T;
    }
    // Condition rank count (getMyRank).
    if (sql.includes("SELECT COUNT(*) AS ahead")) {
      const condition = this.params[0] as number;
      const total = this.params[1] as number;
      const total2 = this.params[2] as number;
      const submittedAt = this.params[3] as number;
      const ahead = this.db.scores.filter(
        (s) =>
          s.condition === condition &&
          s.status === "live" &&
          (s.terminal_total > total ||
            (s.terminal_total === total2 && s.submitted_at < submittedAt)),
      ).length;
      return { ahead } as unknown as T;
    }
    // getMyRank session+score join.
    if (sql.includes("JOIN scores")) {
      const id = this.params[0] as string;
      const s = this.db.sessions.get(id);
      if (!s) return null;
      const score = this.db.scores.find((sc) => sc.session_id === id);
      if (!score) return null;
      return {
        used: s.used,
        condition: s.condition,
        id: score.id,
        name: score.name,
        terminal_total: score.terminal_total,
        submitted_at: score.submitted_at,
      } as unknown as T;
    }
    return null;
  }

  async all<T>(): Promise<{ results: T[] }> {
    const sql = this.sql;
    // Leaderboard global query.
    if (sql.includes("FROM scores") && sql.includes("ORDER BY terminal_total")) {
      const condition = this.params[0];
      const limit = this.params.length === 3 ? (this.params[1] as number) : (this.params[0] as number);
      const offset = this.params.length === 3 ? (this.params[2] as number) : (this.params[1] as number);
      let rows = this.db.scores.filter((s) => s.status === "live");
      if (typeof condition === "number" && sql.includes("AND condition = ?")) {
        rows = rows.filter((s) => s.condition === condition);
      }
      rows = [...rows].sort((a, b) =>
        b.terminal_total - a.terminal_total || a.submitted_at - b.submitted_at,
      );
      const sliced = rows.slice(offset, offset + limit);
      return {
        results: sliced.map((r) => ({
          name: r.name,
          condition: r.condition,
          total: r.terminal_total,
          submitted_at: r.submitted_at,
        })) as unknown as T[],
      } as unknown as { results: T[] };
    }
    // getMyRank nearby.
    if (sql.includes("FROM scores") && sql.includes("LIMIT 21")) {
      const condition = this.params[0] as number;
      const offset = this.params[1] as number;
      const rows = this.db.scores
        .filter((s) => s.condition === condition && s.status === "live")
        .sort((a, b) =>
          b.terminal_total - a.terminal_total || a.submitted_at - b.submitted_at,
        );
      const sliced = rows.slice(offset, offset + 21);
      return {
        results: sliced.map((r) => ({
          name: r.name,
          total: r.terminal_total,
          submitted_at: r.submitted_at,
        })) as unknown as T[],
      } as unknown as { results: T[] };
    }
    return { results: [] };
  }

  async run(): Promise<{ meta: { changes: number; last_row_id?: number } }> {
    const sql = this.sql;
    // Session insert (createSession).
    if (sql.startsWith("INSERT INTO sessions")) {
      const p = this.params;
      const row: FakeSession = {
        session_id: p[0] as string,
        challenge: p[1] as string,
        condition: p[2] as number,
        proof: p[3] as string,
        expires_at: p[5] as number,
        used: 0,
        turnstile_verified: 1,
        ip_hash: p[6] as string,
      };
      this.db.sessions.set(row.session_id, row);
      return { meta: { changes: 1 } };
    }
    // One-time session claim (submitScore).
    if (sql.startsWith("UPDATE sessions SET used = 1")) {
      const id = this.params[0] as string;
      const now = this.params[1] as number;
      const s = this.db.sessions.get(id);
      if (s && s.used === 0 && s.expires_at > now) {
        s.used = 1;
        return { meta: { changes: 1 } };
      }
      return { meta: { changes: 0 } };
    }
    // Score insert (submitScore).
    if (sql.startsWith("INSERT INTO scores")) {
      const p = this.params;
      const row: FakeScore = {
        id: this.db.nextId++,
        name: p[0] as string,
        condition: p[1] as number,
        terminal_total: p[2] as number,
        chickens: p[3] as number,
        coins: p[4] as number,
        objective_completed: p[5] as 0 | 1,
        max_combo: p[6] as number,
        round_duration_ms: p[7] as number,
        time_left_ms: p[8] as number,
        game_over_reason: p[9] as string,
        build: p[10] as string,
        platform: p[11] as string,
        session_id: p[12] as string,
        submitted_at: p[13] as number,
        ip_hash: p[14] as string,
        status: (p[15] as "live" | "hidden" | "deleted") ?? "live",
        moderation_note: (p[16] as string | null) ?? null,
      };
      this.db.scores.push(row);
      return { meta: { changes: 1, last_row_id: row.id } };
    }
    // Scheduled cleanup: hide old non-top scores. Must be checked BEFORE the
    // moderation status update since both start with "UPDATE scores SET status".
    if (sql.startsWith("UPDATE scores SET status = 'hidden'")) {
      const condition = this.params[0] as number;
      const cutoff = this.params[1] as number;
      const live = this.db.scores
        .filter((s) => s.condition === condition && s.status === "live")
        .sort((a, b) =>
          b.terminal_total - a.terminal_total || a.submitted_at - b.submitted_at,
        );
      const topIds = new Set(live.slice(0, 1000).map((s) => s.id));
      let changes = 0;
      for (const s of live) {
        if (!topIds.has(s.id) && s.submitted_at < cutoff) {
          s.status = "hidden";
          s.moderation_note = "retention: older than 90d and outside top 1000";
          changes++;
        }
      }
      return { meta: { changes } };
    }
    // Moderation status update.
    if (sql.startsWith("UPDATE scores SET status")) {
      const status = this.params[0] as "hidden" | "deleted";
      const note = this.params[1] as string;
      const id = this.params[2] as number;
      const score = this.db.scores.find((s) => s.id === id && s.status === "live");
      if (score) {
        score.status = status;
        score.moderation_note = note;
        return { meta: { changes: 1 } };
      }
      return { meta: { changes: 0 } };
    }
    // Moderation log insert.
    if (sql.startsWith("INSERT INTO moderation_log")) {
      const p = this.params;
      this.db.modLog.push({
        id: this.db.nextModId++,
        action: p[0] as string,
        target_score_id: p[1] as number,
        admin: p[2] as string,
        at: p[3] as number,
        note: (p[4] as string | null) ?? null,
      });
      return { meta: { changes: 1 } };
    }
    // Scheduled cleanup: delete expired sessions.
    if (sql.startsWith("DELETE FROM sessions")) {
      const now = this.params[0] as number;
      let changes = 0;
      for (const [id, s] of this.db.sessions) {
        if (s.expires_at < now) {
          this.db.sessions.delete(id);
          changes++;
        }
      }
      return { meta: { changes } };
    }
    return { meta: { changes: 0 } };
  }
}

/** A fake RateLimit binding that allows N requests per test invocation. */
export class FakeRateLimit {
  private counts = new Map<string, number>();
  constructor(private maxLimit: number, private shouldError = false) {}
  async limit(input: { key: string }): Promise<{ success: boolean }> {
    if (this.shouldError) throw new Error("rate limit binding error");
    const c = (this.counts.get(input.key) ?? 0) + 1;
    this.counts.set(input.key, c);
    return { success: c <= this.maxLimit };
  }
}

/**
 * Build a complete mock Env for fetch-route tests. All secrets are non-placeholder
 * by default; the Turnstile secret is the always-pass test secret, requiring
 * BUILD=dev (which the default sets).
 */
export function makeEnv(overrides: Partial<{
  DB: FakeD1;
  ALLOWED_ORIGINS: string;
  BUILD: string;
  SCORE_CAPS_JSON: string;
  LB_SESSION_HMAC_KEY: string;
  LB_IP_HASH_PEPPER: string;
  LB_ADMIN_TOKEN: string;
  LB_TURNSTILE_SECRET: string;
  LB_CLIENT_HMAC_KEY: string;
  RATE_LIMIT_READ: FakeRateLimit;
  RATE_LIMIT_SESSION: FakeRateLimit;
  RATE_LIMIT_SUBMIT: FakeRateLimit;
  RATE_LIMIT_RANK: FakeRateLimit;
}> = {}): Record<string, unknown> {
  return {
    DB: overrides.DB ?? new FakeD1(),
    ALLOWED_ORIGINS: overrides.ALLOWED_ORIGINS ?? "http://localhost:8080",
    BUILD: overrides.BUILD ?? "dev",
    SCORE_CAPS_JSON:
      overrides.SCORE_CAPS_JSON ??
      JSON.stringify({ "0": 3000, "1": 3000, "2": 4000, "3": 3000, "4": 6000 }),
    LB_SESSION_HMAC_KEY: overrides.LB_SESSION_HMAC_KEY ?? TEST_SESSION_KEY,
    LB_IP_HASH_PEPPER: overrides.LB_IP_HASH_PEPPER ?? "test-pepper-not-real",
    LB_ADMIN_TOKEN: overrides.LB_ADMIN_TOKEN ?? "test-admin-token-not-real",
    LB_TURNSTILE_SECRET: overrides.LB_TURNSTILE_SECRET ?? "1x0000000000000000000000000000000AA",
    LB_CLIENT_HMAC_KEY: overrides.LB_CLIENT_HMAC_KEY ?? TEST_CLIENT_KEY,
    RATE_LIMIT_READ: overrides.RATE_LIMIT_READ ?? new FakeRateLimit(30),
    RATE_LIMIT_SESSION: overrides.RATE_LIMIT_SESSION ?? new FakeRateLimit(3),
    RATE_LIMIT_SUBMIT: overrides.RATE_LIMIT_SUBMIT ?? new FakeRateLimit(5),
    RATE_LIMIT_RANK: overrides.RATE_LIMIT_RANK ?? new FakeRateLimit(60),
  };
}

export { constantTimeEquals, toBase64Url };
