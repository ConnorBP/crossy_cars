// Request validation, name normalization, and plausibility checks for the
// Roady Car leaderboard. See LEADERBOARD_ARCHITECTURE.md §3 (names), §4
// (request shapes), and §8 (score invariants & caps).

import { boundedString, isPlainObject } from "../vendor/cloudflare-game-common/src/index";

/** Parse SCORE_CAPS_JSON into a condition→cap map. Invalid JSON is fatal. */
export function parseScoreCaps(raw: string | undefined): Map<number, number> {
  const caps = new Map<number, number>();
  if (!raw) return caps;
  const parsed: unknown = JSON.parse(raw);
  if (!isPlainObject(parsed)) throw new TypeError("score caps must be a plain object");
  for (const [key, value] of Object.entries(parsed)) {
    if (
      !/^(0|1|2|3|4)$/.test(key) ||
      typeof value !== "number" ||
      !Number.isInteger(value) ||
      value <= 0
    ) {
      throw new TypeError("score caps contain an invalid entry");
    }
    caps.set(Number(key), value);
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

/**
 * Maximum legitimate elapsed round duration: 30 minutes.
 *
 * Pickups can repeatedly extend the clock, so elapsed wall-clock gameplay is
 * not bounded by the clock's 99-second *remaining-time* cap. This generous
 * anti-abuse ceiling is shared with the Rust client contract and remains far
 * below Number.MAX_SAFE_INTEGER so canonical integer signing is exact.
 */
export const MAX_ROUND_DURATION_MS = 1_800_000;
/** Maximum request body size accepted by the Worker (bytes). */
export const MAX_REQUEST_BODY_BYTES = 16 * 1024; // 16 KiB
/** Maximum UTF-8 byte length for free-text score fields. */
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
  return typeof x === "number" && Number.isSafeInteger(x) && x >= 0;
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

  const sessionId = boundedString(body.sessionId, { minBytes: 1, maxBytes: 256 });
  const proof = boundedString(body.proof, { minBytes: 1, maxBytes: 256 });
  const nameRaw = body.name;
  const condition = body.condition;
  const terminalTotal = body.terminal_total;
  const chickens = body.chickens;
  const coins = body.coins;
  const objectiveCompleted = body.objective_completed;
  const maxCombo = body.max_combo;
  const roundDurationMs = body.round_duration_ms;
  const timeLeftMs = body.time_left_ms;
  const gameOverReason = body.game_over_reason;
  const build = boundedString(body.build, { minBytes: 1, maxBytes: MAX_BUILD_LENGTH });
  const platform = body.platform;

  if (sessionId === null)
    return fail("invalid_session", "Missing or oversized sessionId");
  if (proof === null)
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

  // Elapsed duration can exceed 120 seconds because time pickups extend play.
  // Keep it a non-negative safe integer for exact canonical signing, with the
  // shared 30-minute ceiling as a generous anti-abuse bound.
  if (!isNonNegInt(roundDurationMs) || roundDurationMs > MAX_ROUND_DURATION_MS)
    return fail(
      "invalid_duration",
      `round_duration_ms must be a non-negative safe integer <= ${MAX_ROUND_DURATION_MS}`,
    );
  if (!isNonNegInt(timeLeftMs) || timeLeftMs > 120_000)
    return fail("invalid_time_left", "time_left_ms out of range");

  if (typeof gameOverReason !== "string" || !GAME_OVER_REASONS.has(gameOverReason))
    return fail("invalid_reason", "game_over_reason must be 'time_up' or 'wrecked'");
  if (build === null)
    return fail("invalid_build", "build must be a non-empty UTF-8 string (<= 64 bytes)");
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

const isObject = isPlainObject;

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
  const token = boundedString(body.turnstileToken, { minBytes: 1, maxBytes: 4096 });
  if (typeof condition !== "number" || !CONDITIONS.has(condition))
    return fail("invalid_condition", "condition must be 0–4");
  if (token === null)
    return fail("invalid_turnstile", "turnstileToken required (<= 4096 UTF-8 bytes)");
  return { ok: true, condition, turnstileToken: token };
}
