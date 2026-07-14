// Roady Car leaderboard unit + replay-ordering tests.
// Covers: canonical byte construction, HMAC sign/verify round-trip, name
// normalization, score validation invariants, plausibility caps & moderation
// flagging, constant-time comparison, and replay-sensitive one-time session
// claim via an in-memory D1 fake.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  canonicalScoreBytes,
  canonicalSessionBytes,
  constantTimeEquals,
  fromBase64Url,
  hmacBase64Url,
  importHmacKey,
  ipHash,
  toBase64Url,
} from "../src/security";
import {
  MAX_ROUND_DURATION_MS,
  NAME_RE,
  moderationReasons,
  normalizeName,
  parseScoreCaps,
  shouldFlagForModeration,
  validateRestorationBody,
  validateScoreBody,
  validateSessionBody,
} from "../src/validation";
import { escapeXml } from "../src/svg";
import { parseExactOrigin, utf8ByteLength } from "../vendor/cloudflare-game-common/src/index";
import {
  FakeD1,
  SCORE_CAPS,
  TEST_CLIENT_KEY,
  TEST_SESSION_KEY,
  sampleScoreBody,
  signScore,
  signSession,
  validateOk,
} from "./helpers";

// ─── Canonical bytes ─────────────────────────────────────────────────────────

describe("canonical score bytes", () => {
  it("produces the exact architecture-specified field order with LF separators and no trailing LF", () => {
    const bytes = canonicalScoreBytes({
      sessionId: "SID",
      proof: "PROOF",
      name: "AAA",
      condition: 0,
      terminalTotal: 42,
      chickens: 30,
      coins: 12,
      objectiveCompleted: true,
      maxCombo: 4,
      roundDurationMs: 65000,
      timeLeftMs: 0,
      gameOverReason: "time_up",
      build: "0.1.0",
      platform: "web",
    });
    const text = new TextDecoder().decode(bytes);
    expect(text).toBe(
      [
        "roady.v1.score",
        "SID",
        "PROOF",
        "AAA",
        "0",
        "42",
        "30",
        "12",
        "1",
        "4",
        "65000",
        "0",
        "time_up",
        "0.1.0",
        "web",
      ].join("\n"),
    );
    // No trailing LF.
    expect(text.endsWith("\n")).toBe(false);
  });

  it("emits objective_completed as 0 or 1, not true/false", () => {
    const on = canonicalScoreBytes({ ...baseInput(), objectiveCompleted: true });
    const off = canonicalScoreBytes({ ...baseInput(), objectiveCompleted: false });
    expect(new TextDecoder().decode(on)).toContain("\n1\n");
    expect(new TextDecoder().decode(off)).toContain("\n0\n");
  });

  it("encodes integers as canonical base-10 with no plus sign or leading zeros", () => {
    const bytes = canonicalScoreBytes({ ...baseInput(), terminalTotal: 7, maxCombo: 2 });
    const text = new TextDecoder().decode(bytes);
    expect(text).toContain("\n7\n");
    expect(text).toContain("\n2\n");
    expect(text).not.toContain("+");
  });

  it("canonical signing preserves the same extended duration exactly", () => {
    const duration = 161_400;
    const first = canonicalScoreBytes({ ...baseInput(), roundDurationMs: duration });
    const second = canonicalScoreBytes({ ...baseInput(), roundDurationMs: duration });
    expect(first).toEqual(second);
    expect(new TextDecoder().decode(first)).toContain(`\n${duration}\n`);
    expect(canonicalScoreBytes({ ...baseInput(), roundDurationMs: duration + 1 })).not.toEqual(first);
  });
});

describe("canonical session bytes", () => {
  it("matches the architecture field order", () => {
    const bytes = canonicalSessionBytes({
      sessionId: "SID",
      challenge: "CHAL",
      condition: 2,
      expiresAt: 1760000000000,
    });
    expect(new TextDecoder().decode(bytes)).toBe(
      ["roady.v1.session", "SID", "CHAL", "2", "1760000000000"].join("\n"),
    );
    expect(new TextDecoder().decode(bytes).endsWith("\n")).toBe(false);
  });
});

function baseInput() {
  return {
    sessionId: "SID",
    proof: "PROOF",
    name: "AAA",
    condition: 0,
    terminalTotal: 42,
    chickens: 30,
    coins: 12,
    objectiveCompleted: true,
    maxCombo: 4,
    roundDurationMs: 65000,
    timeLeftMs: 0,
    gameOverReason: "time_up",
    build: "0.1.0",
    platform: "web",
  };
}

// ─── HMAC sign/verify round-trip ─────────────────────────────────────────────

describe("HMAC round-trip", () => {
  it("produces a 32-byte (43-char unpadded base64url) signature", async () => {
    const sig = await signScore(TEST_CLIENT_KEY, validateOk(sampleScoreBody()));
    // 32 bytes → 43 base64url chars without padding.
    expect(sig.length).toBe(43);
    expect(/^[A-Za-z0-9_-]+$/.test(sig)).toBe(true);
    expect(sig.includes("=")).toBe(false);
  });

  it("verifies: Worker-rebuilt bytes match client-signed bytes", async () => {
    const v = validateOk(sampleScoreBody());
    const sig = await signScore(TEST_CLIENT_KEY, v);

    // Worker side: rebuild canonical bytes from the *validated* values and
    // recompute; compare decoded bytes with constant-time equality.
    const key = await importHmacKey(TEST_CLIENT_KEY);
    const expected = await crypto.subtle.sign(
      "HMAC",
      key,
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
    expect(constantTimeEquals(fromBase64Url(sig), new Uint8Array(expected))).toBe(true);
  });

  it("rejects a signature made with a different key", async () => {
    const v = validateOk(sampleScoreBody());
    const wrongSig = await signScore("wrong-key", v);
    const key = await importHmacKey(TEST_CLIENT_KEY);
    const expected = await crypto.subtle.sign("HMAC", key, canonicalScoreBytes({ ...v, objectiveCompleted: v.objectiveCompleted === 1 }));
    expect(constantTimeEquals(fromBase64Url(wrongSig), new Uint8Array(expected))).toBe(false);
  });

  it("produces the same signature for the same extended duration", async () => {
    const duration = 161_400;
    const v = validateOk(sampleScoreBody({ round_duration_ms: duration }));
    const first = await signScore(TEST_CLIENT_KEY, v);
    const second = await signScore(TEST_CLIENT_KEY, v);
    expect(second).toBe(first);

    const changed = validateOk(sampleScoreBody({ round_duration_ms: duration + 1 }));
    expect(await signScore(TEST_CLIENT_KEY, changed)).not.toBe(first);
  });

  it("rejects a signature over a tampered total", async () => {
    const v = validateOk(sampleScoreBody());
    const sig = await signScore(TEST_CLIENT_KEY, v);
    // Tamper: change total but keep the old signature.
    const tampered = { ...v, terminalTotal: v.terminalTotal + 1 };
    const key = await importHmacKey(TEST_CLIENT_KEY);
    const expected = await crypto.subtle.sign(
      "HMAC",
      key,
      canonicalScoreBytes({ ...tampered, objectiveCompleted: tampered.objectiveCompleted === 1 }),
    );
    expect(constantTimeEquals(fromBase64Url(sig), new Uint8Array(expected))).toBe(false);
  });

  it("session proof round-trips and is condition/expiry-bound", async () => {
    const input = { sessionId: "SID", challenge: "CHAL", condition: 1, expiresAt: 1760000000000 };
    const proof = await signSession(TEST_SESSION_KEY, input);
    const key = await importHmacKey(TEST_SESSION_KEY);
    const expected = await hmacBase64Url(key, canonicalSessionBytes(input));
    expect(proof).toBe(expected);

    // Different condition → different proof.
    const other = await signSession(TEST_SESSION_KEY, { ...input, condition: 2 });
    expect(other).not.toBe(proof);
  });
});

// ─── SVG XML escaping ───────────────────────────────────────────────────────

describe("SVG XML escaping", () => {
  it("escapes every XML-significant character defensively", () => {
    expect(escapeXml(`A&B<C>D\"E'F`)).toBe(
      "A&amp;B&lt;C&gt;D&quot;E&apos;F",
    );
  });

  it("stringifies non-string values before escaping", () => {
    expect(escapeXml(42)).toBe("42");
  });
});

// ─── Name normalization ──────────────────────────────────────────────────────

describe("shared Worker adapter", () => {
  it("requires canonical exact origins", () => {
    expect(parseExactOrigin("https://car.segfault.site")).toBe("https://car.segfault.site");
    expect(parseExactOrigin("https://car.segfault.site/")).toBeNull();
    expect(parseExactOrigin("https://*.segfault.site")).toBeNull();
  });

  it("counts UTF-8 bytes rather than UTF-16 code units", () => {
    expect(utf8ByteLength("é🙂")).toBe(6);
  });
});

describe("name normalization", () => {
  it("uppercases and accepts 3–5 chars from [A-Z0-9]", () => {
    expect(normalizeName("abc")).toBe("ABC");
    expect(normalizeName("  ab12  ")).toBe("AB12");
    expect(normalizeName("ROADY")).toBe("ROADY");
    expect(normalizeName("a1b")).toBe("A1B");
  });

  it("rejects invalid names", () => {
    expect(normalizeName("AB")).toBeNull(); // too short
    expect(normalizeName("ROADYX")).toBeNull(); // too long
    expect(normalizeName("AB_")).toBeNull(); // invalid char
    expect(normalizeName("")).toBeNull();
    expect(normalizeName("   ")).toBeNull();
    expect(normalizeName("AB-C")).toBeNull();
  });

  it("NAME_RE matches only 3–5 uppercase alphanumerics", () => {
    expect(NAME_RE.test("AAA")).toBe(true);
    expect(NAME_RE.test("A1B2C")).toBe(true);
    expect(NAME_RE.test("AA")).toBe(false);
    expect(NAME_RE.test("AAAAAA")).toBe(false);
    expect(NAME_RE.test("aaa")).toBe(false);
    expect(NAME_RE.test("A.B")).toBe(false);
  });
});

// ─── Score validation ────────────────────────────────────────────────────────

describe("score validation", () => {
  it("accepts a well-formed body", () => {
    const v = validateOk(sampleScoreBody());
    expect(v.name).toBe("AAA");
    expect(v.objectiveCompleted).toBe(1);
  });

  it("rejects terminal_total != chickens + coins", () => {
    const r = validateScoreBody(sampleScoreBody({ terminal_total: 99, chickens: 30, coins: 12 }), SCORE_CAPS);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.code).toBe("total_mismatch");
  });

  it("rejects condition outside 0–4", () => {
    const r = validateScoreBody(sampleScoreBody({ condition: 5 }), SCORE_CAPS);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.code).toBe("invalid_condition");
    const r2 = validateScoreBody(sampleScoreBody({ condition: -1 }), SCORE_CAPS);
    expect(r2.ok).toBe(false);
  });

  it("rejects max_combo outside 1–5", () => {
    const r = validateScoreBody(sampleScoreBody({ max_combo: 0 }), SCORE_CAPS);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.code).toBe("invalid_combo");
    const r2 = validateScoreBody(sampleScoreBody({ max_combo: 6 }), SCORE_CAPS);
    expect(r2.ok).toBe(false);
  });

  it("rejects invalid game_over_reason / platform", () => {
    const r = validateScoreBody(sampleScoreBody({ game_over_reason: "crashed" }), SCORE_CAPS);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.code).toBe("invalid_reason");
    const r2 = validateScoreBody(sampleScoreBody({ platform: "mobile" }), SCORE_CAPS);
    expect(r2.ok).toBe(false);
    if (!r2.ok) expect(r2.code).toBe("invalid_platform");
  });

  it("rejects negative or non-integer totals", () => {
    const r = validateScoreBody(sampleScoreBody({ terminal_total: -1, chickens: -1, coins: 0 }), SCORE_CAPS);
    expect(r.ok).toBe(false);
  });

  it("accepts reported score 1614 with elapsed duration beyond 120 seconds", () => {
    const r = validateScoreBody(
      sampleScoreBody({
        terminal_total: 1614,
        chickens: 1000,
        coins: 614,
        max_combo: 5,
        round_duration_ms: 161_400,
      }),
      SCORE_CAPS,
    );
    expect(r.ok).toBe(true);
    if (r.ok) expect(r.value.roundDurationMs).toBe(161_400);
  });

  it("accepts zero and the exact 30-minute duration boundary", () => {
    expect(validateScoreBody(sampleScoreBody({ round_duration_ms: 0 }), SCORE_CAPS).ok).toBe(true);
    const r = validateScoreBody(
      sampleScoreBody({ round_duration_ms: MAX_ROUND_DURATION_MS }),
      SCORE_CAPS,
    );
    expect(r.ok).toBe(true);
  });

  it("accepts max duration plus one and flags it for review", () => {
    const r = validateScoreBody(
      sampleScoreBody({ round_duration_ms: MAX_ROUND_DURATION_MS + 1 }),
      SCORE_CAPS,
    );
    expect(r.ok).toBe(true);
    if (r.ok) expect(moderationReasons(r.value, SCORE_CAPS)).toContain("long-duration");
  });

  it("accepts the safe-integer maximum and flags it for review", () => {
    const r = validateScoreBody(
      sampleScoreBody({ round_duration_ms: Number.MAX_SAFE_INTEGER }),
      SCORE_CAPS,
    );
    expect(r.ok).toBe(true);
    if (r.ok) expect(moderationReasons(r.value, SCORE_CAPS)).toContain("long-duration");
  });

  it.each([
    ["fractional", 120_000.5],
    ["negative", -1],
    ["unsafe integer overflow", Number.MAX_SAFE_INTEGER + 1],
    ["positive infinity", Number.POSITIVE_INFINITY],
    ["NaN", Number.NaN],
    ["boolean", true],
    ["object", {}],
    ["numeric string", "161400"],
    ["null", null],
    ["missing", undefined],
  ])("rejects %s round_duration_ms", (_label, roundDurationMs) => {
    const body = sampleScoreBody({ round_duration_ms: roundDurationMs });
    if (roundDurationMs === undefined) delete body.round_duration_ms;
    const r = validateScoreBody(body, SCORE_CAPS);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.code).toBe("invalid_duration");
  });

  it("uses the shipped 99-second remaining-time cap", () => {
    expect(validateScoreBody(sampleScoreBody({ time_left_ms: 99_000 }), SCORE_CAPS).ok).toBe(true);
    const r = validateScoreBody(sampleScoreBody({ time_left_ms: 99_001 }), SCORE_CAPS);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.code).toBe("invalid_time_left");
  });

  it("hard-rejects score components or aggregate outside u32", () => {
    expect(validateScoreBody(sampleScoreBody({ terminal_total: 4_294_967_295, chickens: 4_294_967_295, coins: 0 }), SCORE_CAPS).ok).toBe(true);
    const component = validateScoreBody(sampleScoreBody({ terminal_total: 4_294_967_296, chickens: 4_294_967_296, coins: 0 }), SCORE_CAPS);
    expect(component.ok).toBe(false);
    const overflow = validateScoreBody(sampleScoreBody({ terminal_total: 4_294_967_295, chickens: 4_294_967_295, coins: 1 }), SCORE_CAPS);
    expect(overflow.ok).toBe(false);
    if (!overflow.ok) expect(overflow.code).toBe("score_overflow");
  });

  it("rejects missing sessionId/proof", () => {
    const r = validateScoreBody(sampleScoreBody({ sessionId: "" }), SCORE_CAPS);
    expect(r.ok).toBe(false);
  });

  it("enforces build limits in UTF-8 bytes", () => {
    const r = validateScoreBody(sampleScoreBody({ build: "🙂".repeat(17) }), SCORE_CAPS);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.code).toBe("invalid_build");
  });

  it("rejects non-object body", () => {
    const r = validateScoreBody("nope", SCORE_CAPS);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.code).toBe("invalid_body");
  });

  it("keeps Rust and Worker duration thresholds aligned", () => {
    const rustClient = readFileSync(new URL("../../src/leaderboard.rs", import.meta.url), "utf8");
    const soft = rustClient.match(/const MAX_ROUND_DURATION_MS: u64 = ([\d_]+);/);
    const hard = rustClient.match(/const MAX_SAFE_INTEGER_MS: u64 = ([\d_]+);/);
    expect(Number(soft![1]!.replaceAll("_", ""))).toBe(MAX_ROUND_DURATION_MS);
    expect(Number(hard![1]!.replaceAll("_", ""))).toBe(Number.MAX_SAFE_INTEGER);
  });
});

// ─── Plausibility caps & moderation flagging ─────────────────────────────────

describe("plausibility caps", () => {
  it("parseScoreCaps parses the condition→cap map", () => {
    const caps = parseScoreCaps(JSON.stringify({ "0": 3000, "2": 4000, "4": 6000 }));
    expect(caps.get(0)).toBe(3000);
    expect(caps.get(2)).toBe(4000);
    expect(caps.get(4)).toBe(6000);
  });

  it("parseScoreCaps rejects unsafe root shapes and entries", () => {
    expect(() => parseScoreCaps("null")).toThrow();
    expect(() => parseScoreCaps("[]")).toThrow();
    expect(() => parseScoreCaps('{"0":1.5}')).toThrow();
    expect(() => parseScoreCaps('{"unexpected":100}')).toThrow();
  });

  it("accepts above-cap totals and flags over-cap moderation", () => {
    const r = validateScoreBody(
      sampleScoreBody({ condition: 4, terminal_total: 6001, chickens: 3000, coins: 3001 }),
      SCORE_CAPS,
    );
    expect(r.ok).toBe(true);
    if (r.ok) expect(moderationReasons(r.value, SCORE_CAPS)).toEqual(["near-cap", "over-cap"]);
  });

  it("accepts at-cap totals (boundary)", () => {
    const r = validateScoreBody(
      sampleScoreBody({ condition: 4, terminal_total: 6000, chickens: 3000, coins: 3000 }),
      SCORE_CAPS,
    );
    expect(r.ok).toBe(true);
  });

  it("flags near-cap (>=80%) scores for moderation, not rejection", () => {
    // Condition 4 cap 6000; 80% = 4800.
    expect(shouldFlagForModeration(4800, 4, SCORE_CAPS)).toBe(true);
    expect(shouldFlagForModeration(4799, 4, SCORE_CAPS)).toBe(false);
    // Condition 0 cap 3000; 80% = 2400.
    expect(shouldFlagForModeration(2400, 0, SCORE_CAPS)).toBe(true);
    expect(shouldFlagForModeration(42, 0, SCORE_CAPS)).toBe(false);
  });

  it("retains all deterministic moderation reasons", () => {
    const r = validateScoreBody(sampleScoreBody({
      condition: 0,
      terminal_total: 2400,
      chickens: 2399,
      coins: 1,
      max_combo: 5,
      round_duration_ms: MAX_ROUND_DURATION_MS + 1,
    }), SCORE_CAPS);
    expect(r.ok).toBe(true);
    if (r.ok) {
      expect(moderationReasons(r.value, SCORE_CAPS)).toEqual(["near-cap", "long-duration"]);
    }
    const unusual = validateScoreBody(sampleScoreBody({
      terminal_total: 1,
      chickens: 1,
      coins: 0,
      max_combo: 5,
    }), SCORE_CAPS);
    expect(unusual.ok).toBe(true);
    if (unusual.ok) expect(moderationReasons(unusual.value, SCORE_CAPS)).toContain("implausible-combo");
  });
});

describe("admin restoration validation", () => {
  function restoration(overrides: Record<string, unknown> = {}) {
    const score = sampleScoreBody();
    return {
      restoration_key: "incident-2026-07-14-001",
      evidence_hash: "ab".repeat(32),
      known: {
        name: score.name,
        condition: score.condition,
        terminal_total: score.terminal_total,
        chickens: score.chickens,
        coins: score.coins,
        objective_completed: score.objective_completed,
        game_over_reason: score.game_over_reason,
      },
      synthetic: {
        max_combo: score.max_combo,
        round_duration_ms: score.round_duration_ms,
        time_left_ms: score.time_left_ms,
        build: score.build,
        platform: score.platform,
        submitted_at: 1_750_000_000_000,
      },
      reason: "Test restoration.",
      ...overrides,
    };
  }

  it("accepts and normalizes an exact evidence-backed payload", () => {
    const base = restoration();
    const known = { ...(base.known as Record<string, unknown>), name: "abc" };
    const result = validateRestorationBody({ ...base, evidence_hash: "AB".repeat(32), known }, SCORE_CAPS);
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.value.evidenceHash).toBe("ab".repeat(32));
      expect(result.value.score.name).toBe("ABC");
    }
  });

  it("rejects unknown/missing fields, malformed evidence, and unsafe timestamps", () => {
    expect(validateRestorationBody(restoration({ extra: true }), SCORE_CAPS).ok).toBe(false);
    const missing = restoration();
    delete (missing.synthetic as Record<string, unknown>).build;
    expect(validateRestorationBody(missing, SCORE_CAPS).ok).toBe(false);
    for (const evidence_hash of ["a".repeat(63), "g".repeat(64), 42]) {
      const result = validateRestorationBody(restoration({ evidence_hash }), SCORE_CAPS);
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe("invalid_evidence_hash");
    }
    const unsafeBase = restoration();
    const unsafe = validateRestorationBody({
      ...unsafeBase,
      synthetic: { ...(unsafeBase.synthetic as Record<string, unknown>), submitted_at: Number.MAX_SAFE_INTEGER + 1 },
    }, SCORE_CAPS);
    expect(unsafe.ok).toBe(false);
    if (!unsafe.ok) expect(unsafe.code).toBe("invalid_submitted_at");
  });

  it("retains all public hard score invariants and caps", () => {
    const mismatchBase = restoration();
    const mismatch = validateRestorationBody({
      ...mismatchBase,
      known: { ...(mismatchBase.known as Record<string, unknown>), terminal_total: 99 },
    }, SCORE_CAPS);
    expect(mismatch.ok).toBe(false);
    if (!mismatch.ok) expect(mismatch.code).toBe("total_mismatch");
    const overBase = restoration();
    const overCap = validateRestorationBody({
      ...overBase,
      known: {
        ...(overBase.known as Record<string, unknown>),
        terminal_total: 3001, chickens: 3000, coins: 1,
      },
    }, SCORE_CAPS);
    expect(overCap.ok).toBe(true);
  });
});

// ─── Session body validation ─────────────────────────────────────────────────

describe("session body validation", () => {
  it("accepts a valid body", () => {
    const r = validateSessionBody({ condition: 0, turnstileToken: "tok" });
    expect(r.ok).toBe(true);
  });
  it("rejects missing token", () => {
    const r = validateSessionBody({ condition: 0 });
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.code).toBe("invalid_turnstile");
  });
  it("rejects bad condition", () => {
    const r = validateSessionBody({ condition: 9, turnstileToken: "tok" });
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.code).toBe("invalid_condition");
  });
});

// ─── Constant-time comparison ────────────────────────────────────────────────

describe("constantTimeEquals", () => {
  it("returns true for equal bytes", () => {
    const a = new Uint8Array([1, 2, 3]);
    expect(constantTimeEquals(a, new Uint8Array([1, 2, 3]))).toBe(true);
  });
  it("returns false for differing bytes of equal length", () => {
    expect(constantTimeEquals(new Uint8Array([1, 2, 3]), new Uint8Array([1, 2, 4]))).toBe(false);
  });
  it("returns false for differing lengths", () => {
    expect(constantTimeEquals(new Uint8Array([1]), new Uint8Array([1, 2]))).toBe(false);
  });
  it("returns true for two empty arrays", () => {
    expect(constantTimeEquals(new Uint8Array(), new Uint8Array())).toBe(true);
  });
});

// ─── IP hashing ──────────────────────────────────────────────────────────────

describe("ip hashing", () => {
  it("produces a stable base64url hash that depends on the pepper", async () => {
    const h1 = await ipHash("203.0.113.7", "pepper-a");
    const h2 = await ipHash("203.0.113.7", "pepper-a");
    const h3 = await ipHash("203.0.113.7", "pepper-b");
    expect(h1).toBe(h2);
    expect(h1).not.toBe(h3);
    expect(/^[A-Za-z0-9_-]+$/.test(h1)).toBe(true);
  });
  it("different IPs hash differently under the same pepper", async () => {
    const a = await ipHash("203.0.113.7", "p");
    const b = await ipHash("198.51.100.1", "p");
    expect(a).not.toBe(b);
  });
});

// ─── base64url helpers ───────────────────────────────────────────────────────

describe("base64url", () => {
  it("round-trips bytes without padding", () => {
    const bytes = new Uint8Array([0, 1, 2, 250, 255, 128, 64, 32]);
    const enc = toBase64Url(bytes);
    expect(enc.includes("=")).toBe(false);
    const dec = fromBase64Url(enc);
    expect(Array.from(dec)).toEqual(Array.from(bytes));
  });
  it("rejects invalid base64url", () => {
    expect(() => fromBase64Url("not!valid")).toThrow();
  });
});

// ─── Replay-sensitive one-time session claim (DB logic) ──────────────────────

describe("replay-sensitive one-time session claim", () => {
  function freshDb(now: number): FakeD1 {
    const db = new FakeD1();
    db.sessions.set("sess-1", {
      session_id: "sess-1",
      challenge: "chal",
      condition: 0,
      proof: "proof",
      expires_at: now + 60_000,
      used: 0,
      turnstile_verified: 1,
      ip_hash: "hash",
    });
    return db;
  }

  /** Mirrors the claim step from src/index.ts submitScore. */
  async function claim(db: FakeD1, sessionId: string, now: number) {
    return db
      .prepare(`UPDATE sessions SET used = 1 WHERE session_id = ? AND used = 0 AND expires_at > ?`)
      .bind(sessionId, now)
      .run();
  }

  it("claims an unused, unexpired session exactly once (changes == 1)", async () => {
    const now = 1_000_000;
    const db = freshDb(now);
    const r1 = await claim(db, "sess-1", now);
    expect(r1.meta.changes).toBe(1);
    expect(db.sessions.get("sess-1")!.used).toBe(1);
  });

  it("rejects a replayed (already-used) session (changes == 0)", async () => {
    const now = 1_000_000;
    const db = freshDb(now);
    await claim(db, "sess-1", now); // first use
    const r2 = await claim(db, "sess-1", now); // replay
    expect(r2.meta.changes).toBe(0);
    expect(db.sessions.get("sess-1")!.used).toBe(1);
  });

  it("rejects an expired session even if unused (changes == 0)", async () => {
    const now = 1_000_000;
    const db = freshDb(now); // expires_at = now + 60_000
    const r = await claim(db, "sess-1", now + 61_000);
    expect(r.meta.changes).toBe(0);
    expect(db.sessions.get("sess-1")!.used).toBe(0);
  });

  it("rejects an unknown session id (changes == 0)", async () => {
    const now = 1_000_000;
    const db = freshDb(now);
    const r = await claim(db, "does-not-exist", now);
    expect(r.meta.changes).toBe(0);
  });

  it("two concurrent claimers cannot both succeed: second gets changes == 0", async () => {
    const now = 1_000_000;
    const db = freshDb(now);
    const [a, b] = await Promise.all([claim(db, "sess-1", now), claim(db, "sess-1", now)]);
    const successes = [a.meta.changes, b.meta.changes].filter((c) => c === 1).length;
    expect(successes).toBe(1);
  });
});
