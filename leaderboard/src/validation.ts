// Request validation, name normalization, and plausibility checks for the
// Roady Car leaderboard. See LEADERBOARD_ARCHITECTURE.md §3 (names), §4
// (request shapes), and §8 (score invariants & caps).

/** Parse SCORE_CAPS_JSON into a condition→cap map. Invalid JSON is fatal. */
export function parseScoreCaps(raw: string | undefined): Map<number, number> {
  const caps = new Map<number, number>();
  if (!raw) return caps;
  const parsed = JSON.parse(raw) as Record<string, number>;
  for (const [k, v] of Object.entries(parsed)) {
    const cond = Number(k);
    if (Number.isInteger(cond) && Number.isFinite(v)) {
      caps.set(cond, Math.floor(v));
    }
  }
  return caps;
}

export const NAME_RE = /^[A-Z0-9]{3,5}$/;

/** Normalize a submitted name to uppercase [A-Z0-9]{3,5}. Returns null if invalid. */
export function normalizeName(raw: string): string | null {
  if (typeof raw !== "string") return null;
  const trimmed = raw.trim().toUpperCase();
  if (!NAME_RE.test(trimmed)) return null;
  return trimmed;
}

export const CONDITIONS = new Set([0, 1, 2, 3, 4]);
export const PLATFORMS = new Set(["web", "native"]);

/**
 * Plausibility invariant: a higher max combo implies the player banked enough
 * pickups to reach it. Each combo tier requires a minimum terminal total.
 * max_combo 1 → ≥ 0, 2 → ≥ 5, 3 → ≥ 10, 4 → ≥ 15, 5 → ≥ 20. A score with
 * max_combo 5 but a total of 1 is implausible and rejected.
 */
export const MAX_COMBO_MIN_TOTAL: ReadonlyMap<number, number> = new Map([
  [1, 0],
  [2, 5],
  [3, 10],
  [4, 15],
  [5, 20],
]);

/** Maximum request body size accepted by the Worker (bytes). */
export const MAX_REQUEST_BODY_BYTES = 16 * 1024; // 16 KiB
/** Maximum string field length for free-text score fields. */
export const MAX_BUILD_LENGTH = 64;
export const GAME_OVER_REASONS = new Set(["time_up", "wrecked"]);

/** A validated score submission payload. */
export interface ValidatedScore {
  sessionId: string;
  proof: string;
  name: string;
  condition: number;
  terminalTotal: number;
  chickens: number;
  coins: number;
  objectiveCompleted: 0 | 1;
  maxCombo: number;
  roundDurationMs: number;
  timeLeftMs: number;
  gameOverReason: string;
  build: string;
  platform: string;
}

export type ValidationResult =
  | { ok: true; value: ValidatedScore }
  | { ok: false; code: string; message: string };

/** Is `x` a non-negative integer that fits safely? */
function isNonNegInt(x: unknown): x is number {
  return typeof x === "number" && Number.isInteger(x) && x >= 0 && x <= Number.MAX_SAFE_INTEGER;
}

/**
 * Validate a raw parsed score-submission body against the hard invariants.
 * Returns a normalized value or an error code/message. Does NOT check the
 * session proof or client signature (handled in index.ts); only the JSON
 * fields, ranges, and `terminal_total == chickens + coins` invariant.
 *
 * Plausibility caps: only *above-cap* totals are hard-rejected (returns
 * `score_over_cap`). Near-cap scores return `ok` with `flagForModeration`
 * set on the caller side via {@link shouldFlagForModeration}.
 */
export function validateScoreBody(
  body: unknown,
  caps: Map<number, number>,
): ValidationResult {
  if (!isObject(body)) return fail("invalid_body", "Malformed JSON body");

  const sessionId = (body as Record<string, unknown>).sessionId;
  const proof = (body as Record<string, unknown>).proof;
  const nameRaw = (body as Record<string, unknown>).name;
  const condition = (body as Record<string, unknown>).condition;
  const terminalTotal = (body as Record<string, unknown>).terminal_total;
  const chickens = (body as Record<string, unknown>).chickens;
  const coins = (body as Record<string, unknown>).coins;
  const objectiveCompleted = (body as Record<string, unknown>).objective_completed;
  const maxCombo = (body as Record<string, unknown>).max_combo;
  const roundDurationMs = (body as Record<string, unknown>).round_duration_ms;
  const timeLeftMs = (body as Record<string, unknown>).time_left_ms;
  const gameOverReason = (body as Record<string, unknown>).game_over_reason;
  const build = (body as Record<string, unknown>).build;
  const platform = (body as Record<string, unknown>).platform;

  if (typeof sessionId !== "string" || sessionId.length === 0 || sessionId.length > 256)
    return fail("invalid_session", "Missing or oversized sessionId");
  if (typeof proof !== "string" || proof.length === 0 || proof.length > 256)
    return fail("invalid_proof", "Missing or oversized proof");

  const name = normalizeName(nameRaw as string);
  if (name === null) return fail("invalid_name", "Name must be 3–5 chars from A–Z0–9");

  if (typeof condition !== "number" || !CONDITIONS.has(condition))
    return fail("invalid_condition", "condition must be 0–4");

  if (!isNonNegInt(terminalTotal)) return fail("invalid_total", "terminal_total must be a non-negative integer");
  if (!isNonNegInt(chickens)) return fail("invalid_chickens", "chickens must be a non-negative integer");
  if (!isNonNegInt(coins)) return fail("invalid_coins", "coins must be a non-negative integer");

  // Hard invariant: terminal_total == chickens + coins.
  if (terminalTotal !== chickens + coins)
    return fail("total_mismatch", "terminal_total must equal chickens + coins");

  if (typeof objectiveCompleted !== "boolean")
    return fail("invalid_objective", "objective_completed must be boolean");

  if (!isNonNegInt(maxCombo) || maxCombo < 1 || maxCombo > 5)
    return fail("invalid_combo", "max_combo must be an integer 1–5");

  // Plausibility: a higher combo tier implies a minimum terminal total.
  const minTotalForCombo = MAX_COMBO_MIN_TOTAL.get(maxCombo);
  if (minTotalForCombo !== undefined && terminalTotal < minTotalForCombo) {
    return fail("implausible_combo", `max_combo ${maxCombo} requires terminal_total >= ${minTotalForCombo}`);
  }

  // Broad sane ranges (advisory telemetry). 90s round → 90_000ms; allow generous
  // headroom for clock skew and pickups that add time (cap 99s → 99_000ms).
  if (!isNonNegInt(roundDurationMs) || roundDurationMs > 120_000)
    return fail("invalid_duration", "round_duration_ms out of range");
  if (!isNonNegInt(timeLeftMs) || timeLeftMs > 120_000)
    return fail("invalid_time_left", "time_left_ms out of range");

  if (typeof gameOverReason !== "string" || !GAME_OVER_REASONS.has(gameOverReason))
    return fail("invalid_reason", "game_over_reason must be 'time_up' or 'wrecked'");
  if (typeof build !== "string" || build.length === 0 || build.length > MAX_BUILD_LENGTH)
    return fail("invalid_build", "build must be a non-empty string (<= 64 chars)");
  if (typeof platform !== "string" || !PLATFORMS.has(platform))
    return fail("invalid_platform", "platform must be 'web' or 'native'");

  // Plausibility cap: hard-reject only above-cap totals.
  const cap = caps.get(condition);
  if (cap !== undefined && terminalTotal > cap)
    return fail("score_over_cap", "terminal_total exceeds plausibility cap");

  return {
    ok: true,
    value: {
      sessionId,
      proof,
      name,
      condition,
      terminalTotal,
      chickens,
      coins,
      objectiveCompleted: objectiveCompleted ? 1 : 0,
      maxCombo,
      roundDurationMs,
      timeLeftMs,
      gameOverReason,
      build,
      platform,
    },
  };
}

/**
 * Whether a validated score should be flagged for moderation (not rejected).
 * The architecture: scores *near* the cap are flagged, only *above*-cap are
 * rejected (handled in validateScoreBody). "Near" = at least 80% of the cap.
 */
export function shouldFlagForModeration(
  total: number,
  condition: number,
  caps: Map<number, number>,
): boolean {
  const cap = caps.get(condition);
  if (cap === undefined) return false;
  return total >= Math.floor(cap * 0.8);
}

function isObject(x: unknown): x is Record<string, unknown> {
  return typeof x === "object" && x !== null && !Array.isArray(x);
}

function fail(code: string, message: string): { ok: false; code: string; message: string } {
  return { ok: false, code, message };
}

/** Validate a session-creation body: { condition, turnstileToken }. */
export function validateSessionBody(body: unknown): {
  ok: true;
  condition: number;
  turnstileToken: string;
} | { ok: false; code: string; message: string } {
  if (!isObject(body)) return fail("invalid_body", "Malformed JSON body");
  const condition = body.condition;
  const token = body.turnstileToken;
  if (typeof condition !== "number" || !CONDITIONS.has(condition))
    return fail("invalid_condition", "condition must be 0–4");
  if (typeof token !== "string" || token.length === 0 || token.length > 4096)
    return fail("invalid_turnstile", "turnstileToken required");
  return { ok: true, condition, turnstileToken: token };
}
