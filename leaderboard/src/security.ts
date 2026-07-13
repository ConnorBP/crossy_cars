// Security primitives for the Roady Car leaderboard Worker.
// See LEADERBOARD_ARCHITECTURE.md §5 (client HMAC), §6 (session proof), §9
// (IP hashing), and §7 (XOR-accumulate comparison).
//
// All HMAC operations use Web Crypto HMAC-SHA-256. Keys are imported from the
// Worker secret bindings (base64 or raw string) once per request; Workers are
// stateless so there is no benefit to caching an imported key across requests.

import {
  fromBase64Url,
  randomBase64Url,
  sha256,
  sha256Base64Url,
  toBase64Url,
} from "../vendor/cloudflare-game-common/src/index";

/**
 * Import a secret string as a Web Crypto HMAC key.
 *
 * Contract: the secret is used as **raw UTF-8 bytes** — no base64 decoding,
 * no hashing. This is the single, consistent rule for every HMAC key in this
 * Worker (session proof key, client submission key) and for the matching key
 * embedded in the WASM client. Operators may store the secret in any encoding
 * they like externally, but the value delivered to the Worker binding must be
 * the exact string whose UTF-8 bytes are the key material. `importHmacKey` is
 * idempotent: the same string always yields the same key.
 */
export async function importHmacKey(secret: string): Promise<CryptoKey> {
  const raw = new TextEncoder().encode(secret);
  return crypto.subtle.importKey(
    "raw",
    raw,
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign", "verify"],
  );
}

/** Compute HMAC-SHA-256 and return unpadded base64url. */
export async function hmacBase64Url(
  key: CryptoKey,
  data: Uint8Array,
): Promise<string> {
  const sig = await crypto.subtle.sign("HMAC", key, data);
  return toBase64Url(sig);
}

/**
 * Constant-time-ish comparison via fixed-length XOR accumulation across
 * equal-length byte arrays, as specified by the architecture. Returns false
 * immediately if lengths differ (length is not secret here; the signatures
 * are fixed 32-byte HMAC outputs in production, but the caller may pass
 * decoded client signatures of arbitrary length).
 */
export function constantTimeEquals(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) {
    diff |= a[i]! ^ b[i]!;
  }
  return diff === 0;
}

/**
 * Hashed IP attribution: base64url(SHA-256(clientIP + pepper)).
 * Never store raw IP addresses (architecture §9).
 */
export async function ipHash(clientIp: string, pepper: string): Promise<string> {
  return sha256Base64Url(clientIp + pepper);
}

// ─── Canonical byte construction ─────────────────────────────────────────────

/**
 * Build the canonical client submission HMAC bytes (architecture §5).
 * Fixed field order, one ASCII LF separator, no trailing LF:
 *
 *   roady.v1.score
 *   {sessionId}
 *   {proof}
 *   {name}
 *   {condition}
 *   {terminal_total}
 *   {chickens}
 *   {coins}
 *   {objective_completed_0_or_1}
 *   {max_combo}
 *   {round_duration_ms}
 *   {time_left_ms}
 *   {game_over_reason}
 *   {build}
 *   {platform}
 *
 * Integers are canonical base-10 (no leading + or zeroes). The name is already
 * normalized to uppercase [A-Z0-9]{3,5} before this function is called.
 */
export function canonicalScoreBytes(input: {
  sessionId: string;
  proof: string;
  name: string;
  condition: number;
  terminalTotal: number;
  chickens: number;
  coins: number;
  objectiveCompleted: boolean;
  maxCombo: number;
  roundDurationMs: number;
  timeLeftMs: number;
  gameOverReason: string;
  build: string;
  platform: string;
}): Uint8Array {
  const fields: (string | number)[] = [
    "roady.v1.score",
    input.sessionId,
    input.proof,
    input.name,
    input.condition,
    input.terminalTotal,
    input.chickens,
    input.coins,
    input.objectiveCompleted ? 1 : 0,
    input.maxCombo,
    input.roundDurationMs,
    input.timeLeftMs,
    input.gameOverReason,
    input.build,
    input.platform,
  ];
  const joined = fields.join("\n");
  return new TextEncoder().encode(joined);
}

/**
 * Build the canonical Worker session proof HMAC bytes (architecture §6):
 *
 *   roady.v1.session
 *   {sessionId}
 *   {challenge}
 *   {condition}
 *   {expiresAt}
 */
export function canonicalSessionBytes(input: {
  sessionId: string;
  challenge: string;
  condition: number;
  expiresAt: number;
}): Uint8Array {
  const fields: (string | number)[] = [
    "roady.v1.session",
    input.sessionId,
    input.challenge,
    input.condition,
    input.expiresAt,
  ];
  return new TextEncoder().encode(fields.join("\n"));
}

export { fromBase64Url, randomBase64Url, sha256, toBase64Url };
