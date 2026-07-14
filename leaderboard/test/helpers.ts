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

export interface FakeScore {
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
  submission_source?: "verified" | "admin_restore";
  restoration_key?: string | null;
}

interface FakeModLog {
  id: number;
  action: string;
  target_score_id: number;
  admin: string;
  at: number;
  note: string | null;
}

interface FakeRestoration {
  restoration_key: string;
  evidence_hash: string;
  payload_hash: string;
  known_fields_json: string;
  synthetic_fields_json: string;
  reason: string;
  score_id: number;
  restored_at: number;
  admin: string;
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
  restorations = new Map<string, FakeRestoration>();
  nextId = 1;
  nextModId = 1;

  prepare(sql: string) {
    return new FakeStmt(this, sql.trim());
  }

  async batch(statements: FakeStmt[]): Promise<unknown[]> {
    // D1 batches are transactional. Snapshot the small in-memory state so a
    // unique conflict or unsupported operation cannot leave partial rows.
    const sessions = new Map(Array.from(this.sessions, ([key, value]) => [key, { ...value }]));
    const scores = this.scores.map((value) => ({ ...value }));
    const modLog = this.modLog.map((value) => ({ ...value }));
    const restorations = new Map(Array.from(this.restorations, ([key, value]) => [key, { ...value }]));
    const nextId = this.nextId;
    const nextModId = this.nextModId;
    try {
      const results = [];
      for (const statement of statements) results.push(await statement.run());
      return results;
    } catch (error) {
      this.sessions = sessions;
      this.scores = scores;
      this.modLog = modLog;
      this.restorations = restorations;
      this.nextId = nextId;
      this.nextModId = nextModId;
      throw error;
    }
  }
}

export class FakeStmt {
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
    // Restoration lookup by idempotency key or evidence hash.
    if (sql.includes("FROM admin_restorations ar JOIN scores")) {
      const restorationKey = this.params[0] as string;
      const evidenceHash = this.params[1] as string | undefined;
      const restoration = this.db.restorations.get(restorationKey) ??
        Array.from(this.db.restorations.values()).find((row) => row.evidence_hash === evidenceHash);
      if (!restoration) return null;
      const score = this.db.scores.find((row) => row.id === restoration.score_id);
      if (!score) return null;
      return { ...restoration, name: score.name, condition: score.condition,
        terminal_total: score.terminal_total, submitted_at: score.submitted_at } as unknown as T;
    }
    // Condition + global rank counts returned after score submission.
    if (sql.includes("AS condition_ahead") && sql.includes("AS global_ahead")) {
      const condition = this.params[0] as number;
      const total = this.params[1] as number;
      const total2 = this.params[2] as number;
      const submittedAt = this.params[3] as number;
      const tiedTotal = this.params[4] as number;
      const tiedAt = this.params[5] as number;
      const insertedId = this.params[6] as number;
      const ahead = this.db.scores.filter(
        (s) =>
          s.status === "live" &&
          (s.terminal_total > total ||
            (s.terminal_total === total2 && s.submitted_at < submittedAt) ||
            (s.terminal_total === tiedTotal && s.submitted_at === tiedAt && s.id < insertedId)),
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
      const tiedTotal = this.params[4] as number;
      const tiedAt = this.params[5] as number;
      const scoreId = this.params[6] as number;
      const ahead = this.db.scores.filter(
        (s) =>
          s.condition === condition &&
          s.status === "live" &&
          (s.terminal_total > total ||
            (s.terminal_total === total2 && s.submitted_at < submittedAt) ||
            (s.terminal_total === tiedTotal && s.submitted_at === tiedAt && s.id < scoreId)),
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
    // getMyRank nearby.
    if (sql.includes("FROM scores") && sql.includes("LIMIT 21")) {
      const condition = this.params[0] as number;
      const offset = this.params[1] as number;
      const rows = this.db.scores
        .filter((s) => s.condition === condition && s.status === "live")
        .sort((a, b) =>
          b.terminal_total - a.terminal_total ||
          a.submitted_at - b.submitted_at ||
          a.id - b.id,
        );
      const sliced = rows.slice(offset, offset + 21);
      return {
        results: sliced.map((r) => ({
          id: r.id,
          name: r.name,
          total: r.terminal_total,
          submitted_at: r.submitted_at,
        })) as unknown as T[],
      } as unknown as { results: T[] };
    }
    // Global or condition leaderboard query.
    if (sql.includes("FROM scores") && sql.includes("ORDER BY terminal_total")) {
      const hasCondition = sql.includes("AND condition = ?");
      const condition = hasCondition ? (this.params[0] as number) : null;
      const limit = this.params[hasCondition ? 1 : 0] as number;
      const offset = this.params[hasCondition ? 2 : 1] as number;
      let rows = this.db.scores.filter((s) => s.status === "live");
      if (condition !== null) rows = rows.filter((s) => s.condition === condition);
      rows = [...rows].sort((a, b) =>
        b.terminal_total - a.terminal_total ||
        a.submitted_at - b.submitted_at ||
        a.id - b.id,
      );
      const sliced = rows.slice(offset, offset + limit);
      return {
        results: sliced.map((r) => ({
          id: r.id,
          name: r.name,
          condition: r.condition,
          total: r.terminal_total,
          submitted_at: r.submitted_at,
        })) as unknown as T[],
      } as unknown as { results: T[] };
    }
    return { results: [] };
  }

  async run(): Promise<{ meta: { changes: number; last_row_id?: number } }> {
    const sql = this.sql;
    // Session insert (createSession or synthetic admin restoration session).
    if (sql.startsWith("INSERT INTO sessions")) {
      const p = this.params;
      const adminRestore = sql.includes("'admin_restore'");
      const row: FakeSession = adminRestore ? {
        session_id: p[0] as string,
        challenge: "admin_restore",
        condition: p[1] as number,
        proof: "admin_restore",
        expires_at: p[3] as number,
        used: 1,
        turnstile_verified: 0,
        ip_hash: "admin_restore",
      } : {
        session_id: p[0] as string,
        challenge: p[1] as string,
        condition: p[2] as number,
        proof: p[3] as string,
        expires_at: p[5] as number,
        used: 0,
        turnstile_verified: 1,
        ip_hash: p[6] as string,
      };
      if (this.db.sessions.has(row.session_id)) throw new Error("UNIQUE sessions.session_id");
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
    // Score insert (public submission or admin restoration).
    if (sql.startsWith("INSERT INTO scores")) {
      const p = this.params;
      const id = Math.max(
        this.db.nextId,
        ...this.db.scores.map((score) => score.id + 1),
      );
      this.db.nextId = id + 1;
      const adminRestore = sql.includes("'admin_restore'");
      const restorationKey = adminRestore ? p[14] as string : null;
      if (restorationKey && this.db.scores.some((score) => score.restoration_key === restorationKey)) {
        throw new Error("UNIQUE scores.restoration_key");
      }
      const row: FakeScore = {
        id,
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
        moderation_note: adminRestore
          ? "review:v1:admin_restore,synthetic_combo,synthetic_duration,synthetic_time_left,synthetic_build,synthetic_platform,synthetic_submitted_at"
          : (p[16] as string | null) ?? null,
        submission_source: adminRestore ? "admin_restore" : "verified",
        restoration_key: restorationKey,
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
          b.terminal_total - a.terminal_total ||
          a.submitted_at - b.submitted_at ||
          a.id - b.id,
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
    // Admin restoration audit insert selects the just-created score.
    if (sql.startsWith("INSERT INTO admin_restorations")) {
      const p = this.params;
      const restorationKey = p[0] as string;
      const evidenceHash = p[1] as string;
      if (this.db.restorations.has(restorationKey) ||
          Array.from(this.db.restorations.values()).some((row) => row.evidence_hash === evidenceHash)) {
        throw new Error("UNIQUE admin_restorations");
      }
      const score = this.db.scores.find((row) => row.restoration_key === p[7]);
      if (!score) throw new Error("missing restored score");
      this.db.restorations.set(restorationKey, {
        restoration_key: restorationKey,
        evidence_hash: evidenceHash,
        payload_hash: p[2] as string,
        known_fields_json: p[3] as string,
        synthetic_fields_json: p[4] as string,
        reason: p[5] as string,
        score_id: score.id,
        restored_at: p[6] as number,
        admin: "admin",
      });
      return { meta: { changes: 1 } };
    }
    // Moderation log insert (bound public moderation or SELECT restoration).
    if (sql.startsWith("INSERT INTO moderation_log")) {
      const p = this.params;
      const restore = sql.includes("SELECT 'restore'");
      const score = restore ? this.db.scores.find((row) => row.restoration_key === p[2]) : undefined;
      if (restore && !score) throw new Error("missing restored score for log");
      this.db.modLog.push({
        id: this.db.nextModId++,
        action: restore ? "restore" : p[0] as string,
        target_score_id: restore ? score!.id : p[1] as number,
        admin: restore ? "admin" : p[2] as string,
        at: restore ? p[0] as number : p[3] as number,
        note: (restore ? p[1] : p[4]) as string | null,
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
