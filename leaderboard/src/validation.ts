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
      !Number.isSafeInteger(value) ||
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
 * max_combo 5 but a total of 1 is implausible and flagged for moderation
 * (not hard-rejected) via {@link moderationReasons}.
 */
export const MAX_COMBO_MIN_TOTAL: ReadonlyMap<number, number> = new Map([
  [1, 0],
  [2, 5],
  [3, 10],
  [4, 15],
  [5, 20],
]);

/**
 * Soft review threshold for elapsed round duration: 30 minutes.
 *
 * Pickups can repeatedly extend the clock, so elapsed wall-clock gameplay is
 * not bounded by the clock's 99-second *remaining-time* cap. Durations up to
 * Number.MAX_SAFE_INTEGER — the hard max — are accepted for exact canonical
 * integer signing; anything beyond this 30-minute threshold is accepted but
 * flagged for moderation via {@link moderationReasons}. The constant is
 * shared with the Rust client contract, which likewise keeps it as a soft
 * review bound, and is asserted equal by the cross-impl alignment test.
 */
export const MAX_ROUND_DURATION_MS = 1_800_000;
/** Maximum request body size accepted by the Worker (bytes). */
export const MAX_REQUEST_BODY_BYTES = 16 * 1024; // 16 KiB
/** Maximum UTF-8 byte length for free-text score fields. */
export const MAX_BUILD_LENGTH = 64;
export const GAME_OVER_REASONS = new Set(["time_up", "wrecked"]);
export const MAX_U32 = 4_294_967_295;
export const MAX_REMAINING_TIME_MS = 99_000;

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

/**
 * Exact, evidence-backed payload accepted by the admin restoration route. The
 * score is nested under exactly one envelope kind — `known` (a historical
 * record) or `synthetic` (an admin-fabricated entry) — and the canonical JSON
 * of that envelope is preserved for audit alongside the administrator's
 * free-text reason.
 */
export interface ValidatedRestoration {
  restorationKey: string;
  evidenceHash: string;
  submittedAt: number;
  score: ValidatedScore;
  /** Canonical screenshot-proven fields persisted for audit. */
  knownFieldsJson: string;
  /** Canonical inferred/placeholder fields persisted for audit. */
  syntheticFieldsJson: string;
  /** The administrator's free-text restoration reason. */
  reason: string;
}

export type RestorationValidationResult =
  | { ok: true; value: ValidatedRestoration }
  | { ok: false; code: string; message: string };

/**
 * Allowed top-level fields for a restoration payload. Screenshot-proven and
 * inferred fields are separate exact objects. Unknown or missing fields are
 * rejected.
 */
const RESTORATION_FIELDS = new Set([
  "restoration_key",
  "evidence_hash",
  "known",
  "synthetic",
  "reason",
]);

const RESTORATION_KNOWN_FIELDS = new Set([
  "name", "condition", "terminal_total", "chickens", "coins",
  "objective_completed", "game_over_reason",
]);
const RESTORATION_SYNTHETIC_FIELDS = new Set([
  "max_combo", "round_duration_ms", "time_left_ms", "build", "platform",
  "submitted_at",
]);
/** Maximum UTF-8 byte length for a restoration reason. */
export const MAX_RESTORATION_REASON_BYTES = 256;

/**
 * Validate an administrator's historical restoration payload. The shape is
 * exact: the only allowed top-level fields are `restoration_key`,
 * `evidence_hash`, `known`, `synthetic`, and `reason`; unknown or missing
 * fields are rejected. Combined known/synthetic values pass through the same
 * hard score invariants and condition cap as public submissions. Session/proof
 * fields are generated by the Worker and cannot be supplied by the caller.
 * Both canonical field groups are returned separately for permanent audit.
 */
export function validateRestorationBody(
  body: unknown,
  caps: Map<number, number>,
): RestorationValidationResult {
  if (!isPlainObject(body)) return fail("invalid_body", "Malformed JSON body");
  const keys = Object.keys(body);
  if (keys.length !== RESTORATION_FIELDS.size || keys.some((key) => !RESTORATION_FIELDS.has(key))) {
    return fail("invalid_body", "Restoration payload must contain only the documented fields");
  }

  const restorationKey = boundedString(body.restoration_key, { minBytes: 1, maxBytes: 128 });
  if (restorationKey === null || !/^[A-Za-z0-9._:-]+$/.test(restorationKey)) {
    return fail("invalid_restoration_key", "restoration_key must be 1–128 ASCII key characters");
  }
  const evidenceHash = boundedString(body.evidence_hash, { minBytes: 64, maxBytes: 64 });
  if (evidenceHash === null || !/^[0-9a-fA-F]{64}$/.test(evidenceHash)) {
    return fail("invalid_evidence_hash", "evidence_hash must be exactly 64 hexadecimal characters");
  }
  const known = body.known;
  const synthetic = body.synthetic;
  if (!isPlainObject(known) || !isPlainObject(synthetic)) {
    return fail("invalid_score", "known and synthetic must be plain objects");
  }
  const knownKeys = Object.keys(known);
  const syntheticKeys = Object.keys(synthetic);
  if (knownKeys.length !== RESTORATION_KNOWN_FIELDS.size || knownKeys.some((k) => !RESTORATION_KNOWN_FIELDS.has(k))) {
    return fail("invalid_score", "known fields must exactly match the documented screenshot fields");
  }
  if (syntheticKeys.length !== RESTORATION_SYNTHETIC_FIELDS.size || syntheticKeys.some((k) => !RESTORATION_SYNTHETIC_FIELDS.has(k))) {
    return fail("invalid_score", "synthetic fields must exactly match the documented placeholders");
  }

  const reason = boundedString(body.reason, { minBytes: 1, maxBytes: MAX_RESTORATION_REASON_BYTES });
  if (reason === null) {
    return fail("invalid_reason", "reason must be a non-empty UTF-8 string (<= 256 bytes)");
  }

  const scoreResult = validateScoreBody({
    sessionId: "admin_restore",
    proof: "admin_restore",
    name: known.name,
    condition: known.condition,
    terminal_total: known.terminal_total,
    chickens: known.chickens,
    coins: known.coins,
    objective_completed: known.objective_completed,
    max_combo: synthetic.max_combo,
    round_duration_ms: synthetic.round_duration_ms,
    time_left_ms: synthetic.time_left_ms,
    game_over_reason: known.game_over_reason,
    build: synthetic.build,
    platform: synthetic.platform,
  }, caps);
  if (!scoreResult.ok) return scoreResult;

  if (!isNonNegInt(synthetic.submitted_at)) {
    return fail("invalid_submitted_at", "synthetic.submitted_at must be a non-negative safe integer");
  }
  const knownFieldsJson = JSON.stringify({
    name: scoreResult.value.name,
    condition: scoreResult.value.condition,
    terminal_total: scoreResult.value.terminalTotal,
    chickens: scoreResult.value.chickens,
    coins: scoreResult.value.coins,
    objective_completed: scoreResult.value.objectiveCompleted,
    game_over_reason: scoreResult.value.gameOverReason,
  });
  const syntheticFieldsJson = JSON.stringify({
    max_combo: scoreResult.value.maxCombo,
    round_duration_ms: scoreResult.value.roundDurationMs,
    time_left_ms: scoreResult.value.timeLeftMs,
    build: scoreResult.value.build,
    platform: scoreResult.value.platform,
    submitted_at: synthetic.submitted_at,
  });

  return {
    ok: true,
    value: {
      restorationKey,
      evidenceHash: evidenceHash.toLowerCase(),
      submittedAt: synthetic.submitted_at,
      score: scoreResult.value,
      knownFieldsJson,
      syntheticFieldsJson,
      reason,
    },
  };
}

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
 * Hard rejections are limited to structurally invalid values (non-integer,
 * out-of-range, unsafe, mismatched). Soft plausibility concerns — near-cap
 * totals, durations beyond {@link MAX_ROUND_DURATION_MS}, and combo/total
 * mismatches and provisional condition caps — are NOT rejected here; they are
 * surfaced as deterministic moderation reasons via {@link moderationReasons}.
 */
export function validateScoreBody(
  body: unknown,
  _caps: Map<number, number>,
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

  if (!isNonNegInt(terminalTotal) || terminalTotal > MAX_U32)
    return fail("invalid_total", "terminal_total must fit u32");
  if (!isNonNegInt(chickens) || chickens > MAX_U32)
    return fail("invalid_chickens", "chickens must fit u32");
  if (!isNonNegInt(coins) || coins > MAX_U32)
    return fail("invalid_coins", "coins must fit u32");

  // Hard invariant: checked u32 aggregate and exact equality.
  const aggregate = chickens + coins;
  if (!Number.isSafeInteger(aggregate) || aggregate > MAX_U32)
    return fail("score_overflow", "chickens + coins must fit u32");
  if (terminalTotal !== aggregate)
    return fail("total_mismatch", "terminal_total must equal chickens + coins");

  if (typeof objectiveCompleted !== "boolean")
    return fail("invalid_objective", "objective_completed must be boolean");

  if (!isNonNegInt(maxCombo) || maxCombo < 1 || maxCombo > 5)
    return fail("invalid_combo", "max_combo must be an integer 1–5");

  // Elapsed duration can exceed 120 seconds because time pickups extend play.
  // Hard-reject only non-integral / negative / unsafe values so canonical
  // integer signing stays exact; durations beyond the 30-minute soft review
  // threshold (MAX_ROUND_DURATION_MS) are accepted but flagged for
  // moderation via moderationReasons().
  if (!isNonNegInt(roundDurationMs))
    return fail(
      "invalid_duration",
      "round_duration_ms must be a non-negative safe integer",
    );
  if (!isNonNegInt(timeLeftMs) || timeLeftMs > MAX_REMAINING_TIME_MS)
    return fail("invalid_time_left", `time_left_ms must be <= ${MAX_REMAINING_TIME_MS}`);

  if (typeof gameOverReason !== "string" || !GAME_OVER_REASONS.has(gameOverReason))
    return fail("invalid_reason", "game_over_reason must be 'time_up' or 'wrecked'");
  if (build === null)
    return fail("invalid_build", "build must be a non-empty UTF-8 string (<= 64 bytes)");
  if (typeof platform !== "string" || !PLATFORMS.has(platform))
    return fail("invalid_platform", "platform must be 'web' or 'native'");

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
 * Scores at or above 80% of a provisional cap are review candidates. Above-
 * cap scores are accepted with an additional `over-cap` reason.
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

/**
 * Deterministic moderation reason codes, in stable evaluation order. The
 * order matches {@link moderationReasons} output so a caller can join the
 * returned list into a single `moderation_note` without ambiguity.
 */
export const MODERATION_REASONS = [
  "near-cap",
  "over-cap",
  "long-duration",
  "implausible-combo",
] as const;

/** A single moderation reason code from {@link MODERATION_REASONS}. */
export type ModerationReason = (typeof MODERATION_REASONS)[number];

/**
 * Compute the deterministic, ordered list of moderation reasons for a
 * validated score. Returns a stable subset of {@link MODERATION_REASONS};
 * an empty list means the score passes unflagged. This is the canonical
 * reason-list API for the submission flow: the caller joins the reasons
 * (e.g. with `"; "`) into the `moderation_note` column.
 *
 * Reasons (in order):
 *  - `"near-cap"`: terminal_total >= 80% of the condition's plausibility cap.
 *  - `"long-duration"`: round_duration_ms exceeds the 30-minute soft review
 *    threshold ({@link MAX_ROUND_DURATION_MS}) but is still a safe integer.
 *  - `"implausible-combo"`: max_combo implies a higher terminal_total than
 *    reported (see {@link MAX_COMBO_MIN_TOTAL}).
 */
export function moderationReasons(
  value: ValidatedScore,
  caps: Map<number, number>,
): readonly ModerationReason[] {
  const reasons: ModerationReason[] = [];
  if (shouldFlagForModeration(value.terminalTotal, value.condition, caps)) {
    reasons.push("near-cap");
  }
  const cap = caps.get(value.condition);
  if (cap !== undefined && value.terminalTotal > cap) {
    reasons.push("over-cap");
  }
  if (value.roundDurationMs > MAX_ROUND_DURATION_MS) {
    reasons.push("long-duration");
  }
  const minTotalForCombo = MAX_COMBO_MIN_TOTAL.get(value.maxCombo);
  if (minTotalForCombo !== undefined && value.terminalTotal < minTotalForCombo) {
    reasons.push("implausible-combo");
  }
  return reasons;
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
