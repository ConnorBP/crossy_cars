import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  ArithmeticOverflowError,
  CanonicalLedger,
  CLUCK_HUNT_CATEGORY,
  FRENZY_RELOCATION_DOMAIN,
  RIGHT_OF_WAY_CATEGORY,
  RightOfWay,
  bytesToHex,
  chainEvent,
  completedWaves,
  creditedPositive,
  eventRecord,
  frenzyOpportunities,
  frenzyRelocationCandidates,
  hexToBytes,
  hmacSha256Base64Url,
  packageClock,
  parseCanonicalDecimal,
  coinClock,
  rightOfWayTerminal,
  rotationSchedule,
  scheduleBytes,
  scheduleHash,
  scheduledEvents,
  scoreHmacInput,
  scoreHmacSignature,
  seedCommitment,
  sha256,
  startedSessionHeader,
  stream,
  streamFnv,
  streamState,
  terminalBytes,
  timePickupClock,
  unstartedSessionHeader,
  workerProofInput,
  type ConductTerminal,
  type EventPayload,
  type ProtocolEvent,
  type ScoreHmacInput,
} from "../src/rules-v3";

interface Golden {
  readonly stream_vectors: readonly StreamVector[];
  readonly schedule_vectors: readonly ScheduleVector[];
  readonly frenzy_seed01: {
    readonly input: { readonly seed_hex: string; readonly through_ms: number };
    readonly output: readonly {
      readonly at_ms: number;
      readonly roll_residue: number;
      readonly spawn: boolean;
      readonly pity: boolean;
    }[];
  };
  readonly relocation_seed01: {
    readonly input: { readonly seed_hex: string };
    readonly output: { readonly lateral_ahead: readonly (readonly [number, number])[] };
  };
  readonly canonical: CanonicalGolden;
  readonly arithmetic_boundaries: ArithmeticGolden;
}

interface StreamVector {
  readonly input: { readonly seed_hex: string; readonly domain: string };
  readonly output: {
    readonly fnv_hex: string;
    readonly initial_state_hex: string;
    readonly first_three_u64_hex: readonly string[];
  };
}

interface ScheduleVector {
  readonly input: { readonly seed_hex: string };
  readonly output: {
    readonly windows: readonly {
      readonly effect_ordinal: number;
      readonly telegraph_start_ms: number;
      readonly active_start_ms: number;
      readonly active_end_ms: number;
      readonly cooldown_end_ms: number;
    }[];
    readonly scheduled_events: readonly { readonly ordinal: number }[];
    readonly seed_commitment_hex: string;
    readonly schedule_commitment_cluck_hunt_hex: string;
    readonly schedule_commitment_right_of_way_hex: string;
  };
}

type JsonRecord = Readonly<Record<string, unknown>>;
interface CanonicalGolden {
  readonly session: { readonly input: JsonRecord; readonly output: JsonRecord };
  readonly schedule: { readonly input: JsonRecord; readonly output: JsonRecord };
  readonly events: readonly {
    readonly name: string;
    readonly input: JsonRecord;
    readonly output: JsonRecord;
  }[];
  readonly ledger: { readonly input: JsonRecord; readonly output: JsonRecord };
  readonly score_hmac_right_of_way_contract_golden: {
    readonly input: JsonRecord;
    readonly output: JsonRecord;
  };
  readonly score_hmac_cluck_hunt: { readonly input: JsonRecord; readonly output: JsonRecord };
}
interface ArithmeticGolden {
  readonly credited_positive: readonly {
    readonly input: { readonly base: number; readonly premium_bps: number; readonly guilt: boolean };
    readonly output: Readonly<{ ok: number } | { error: string }>;
  }[];
  readonly right_of_way_terminal: readonly {
    readonly accumulator: string;
    readonly output: Readonly<{ ok: number } | { error: string }>;
  }[];
  readonly completed_waves: readonly { readonly input_ms: string; readonly output: number }[];
  readonly coin_clock: readonly { readonly input_ms: string; readonly output_ms: string }[];
  readonly package_clock: readonly { readonly input_ms: string; readonly output_ms: string }[];
  readonly time_pickup_clock: readonly { readonly input_ms: string; readonly output_ms: string }[];
}

// Deliberately load the published artifact from the filesystem rather than
// importing JSON, so parity tests exercise the exact cross-language fixture.
const goldenPath = fileURLToPath(new URL("../../rules/roady-rules.v3.golden.json", import.meta.url));
const golden = JSON.parse(readFileSync(goldenPath, "utf8")) as Golden;

function stringField(record: JsonRecord, key: string): string {
  const value = record[key];
  if (typeof value !== "string") throw new TypeError(`${key} is not a string`);
  return value;
}
function numberField(record: JsonRecord, key: string): number {
  const value = record[key];
  if (typeof value !== "number") throw new TypeError(`${key} is not a number`);
  return value;
}
function objectField(record: JsonRecord, key: string): JsonRecord {
  const value = record[key];
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new TypeError(`${key} is not an object`);
  }
  return value as JsonRecord;
}

function terminalFromFixture(input: JsonRecord): ConductTerminal {
  const common = {
    reason: numberField(input, "reason"),
    total: BigInt(numberField(input, "total")),
    objectiveCompleted: input["objective_completed"] === true,
    durationMs: BigInt(numberField(input, "duration_ms")),
    remainingMs: BigInt(numberField(input, "remaining_ms")),
    build: stringField(input, "build"),
    platform: numberField(input, "platform"),
  };
  if (input["conduct"] === "cluck_hunt") {
    return {
      conduct: "cluck_hunt",
      ...common,
      chickens: BigInt(numberField(input, "chickens")),
      coins: BigInt(numberField(input, "coins")),
      maxCombo: numberField(input, "max_combo"),
    };
  }
  return {
    conduct: "right_of_way",
    ...common,
    accumulator: BigInt(stringField(input, "accumulator")),
    premiumBps: BigInt(numberField(input, "premium_bps")),
    packagesDelivered: BigInt(numberField(input, "packages_delivered")),
    courtesyCount: BigInt(numberField(input, "courtesy_count")),
    animalHits: BigInt(numberField(input, "animal_hits")),
    maxDeliveryChain: BigInt(numberField(input, "max_delivery_chain")),
  };
}

function camelCase(value: string): string {
  return value.replace(/_([a-z])/g, (_match, letter: string) => letter.toUpperCase());
}

function payloadFromFixture(name: string, input: JsonRecord): EventPayload {
  if (name === "terminal_cluck" || name === "terminal_right_of_way") {
    return { type: "terminal", terminal: terminalFromFixture(input) };
  }
  const payload: Record<string, unknown> = { type: name };
  for (const [key, value] of Object.entries(input)) {
    if (key === "objective") continue;
    if (key === "objective_ordinal") {
      payload["objective"] = value;
    } else {
      payload[camelCase(key)] = value;
    }
  }
  return payload as EventPayload;
}

function scoreInputFromFixture(input: JsonRecord): ScoreHmacInput {
  return {
    category: stringField(input, "category"),
    sessionId: stringField(input, "session_id"),
    finalRoot: hexToBytes(stringField(input, "final_root_hex")),
    scheduleHash: hexToBytes(stringField(input, "schedule_hash_hex")),
    seedCommitment: hexToBytes(stringField(input, "seed_commitment_hex")),
    terminal: terminalFromFixture(objectField(input, "terminal")),
  };
}

function expectResult(
  expected: Readonly<{ ok: number } | { error: string }>,
  operation: () => bigint,
): void {
  if ("ok" in expected) {
    expect(operation()).toBe(BigInt(expected.ok));
  } else {
    expect(operation).toThrow(ArithmeticOverflowError);
  }
}

describe("rules v3 SplitMix64 and schedule golden parity", () => {
  it("matches every stream derivation and frozen draw", () => {
    for (const vector of golden.stream_vectors) {
      const seed = hexToBytes(vector.input.seed_hex);
      expect(streamFnv(seed, vector.input.domain).toString(16).padStart(16, "0"))
        .toBe(vector.output.fnv_hex);
      expect(streamState(seed, vector.input.domain).toString(16).padStart(16, "0"))
        .toBe(vector.output.initial_state_hex);
      const rng = stream(seed, vector.input.domain);
      expect(Array.from({ length: 3 }, () => rng.nextU64().toString(16).padStart(16, "0")))
        .toEqual(vector.output.first_three_u64_hex);
    }
  });

  it("matches all 20 rotation, event, byte, and commitment vectors", async () => {
    for (const vector of golden.schedule_vectors) {
      const seed = hexToBytes(vector.input.seed_hex);
      const schedule = rotationSchedule(seed);
      expect(schedule.map((window) => ({
        effect_ordinal: window.effect,
        telegraph_start_ms: Number(window.telegraphStartMs),
        active_start_ms: Number(window.activeStartMs),
        active_end_ms: Number(window.activeEndMs),
        cooldown_end_ms: Number(window.cooldownEndMs),
      }))).toEqual(vector.output.windows.map((window) => ({
        effect_ordinal: window.effect_ordinal,
        telegraph_start_ms: window.telegraph_start_ms,
        active_start_ms: window.active_start_ms,
        active_end_ms: window.active_end_ms,
        cooldown_end_ms: window.cooldown_end_ms,
      })));
      expect(scheduledEvents(seed, schedule)).toEqual(
        vector.output.scheduled_events.map((event) => event.ordinal),
      );
      expect(bytesToHex(await seedCommitment(seed))).toBe(vector.output.seed_commitment_hex);
      expect(bytesToHex(await scheduleHash(seed, CLUCK_HUNT_CATEGORY)))
        .toBe(vector.output.schedule_commitment_cluck_hunt_hex);
      expect(bytesToHex(await scheduleHash(seed, RIGHT_OF_WAY_CATEGORY)))
        .toBe(vector.output.schedule_commitment_right_of_way_hex);
    }
  });

  it("matches frenzy opportunities and f32 relocation coordinates", () => {
    const frenzySeed = hexToBytes(golden.frenzy_seed01.input.seed_hex);
    expect(frenzyOpportunities(frenzySeed, BigInt(golden.frenzy_seed01.input.through_ms)).map(
      (value) => ({
        at_ms: Number(value.atMs),
        roll_residue: Number(value.rollResidue),
        spawn: value.spawn,
        pity: value.pity,
      }),
    )).toEqual(golden.frenzy_seed01.output);

    const relocationSeed = hexToBytes(golden.relocation_seed01.input.seed_hex);
    expect(streamState(relocationSeed, FRENZY_RELOCATION_DOMAIN)).toBeDefined();
    expect(frenzyRelocationCandidates(relocationSeed))
      .toEqual(golden.relocation_seed01.output.lateral_ahead);
  });
});

describe("rules v3 canonical byte and hash parity", () => {
  it("matches session headers, proof inputs, hashes, and HMAC signatures", async () => {
    const fixture = golden.canonical.session;
    const input = fixture.input;
    const output = fixture.output;
    const headerInput = {
      category: stringField(input, "category"),
      sessionId: stringField(input, "session_id"),
      challenge: stringField(input, "challenge"),
      seedCommitment: hexToBytes(stringField(input, "seed_commitment_hex")),
      scheduleHash: hexToBytes(stringField(input, "schedule_hash_hex")),
      issuedAtMs: BigInt(numberField(input, "issued_at_ms")),
    };
    const unstarted = unstartedSessionHeader(
      headerInput,
      BigInt(numberField(input, "start_by_expiry_ms")),
    );
    const started = startedSessionHeader(headerInput, BigInt(numberField(input, "started_at_ms")));
    expect(bytesToHex(unstarted)).toBe(stringField(output, "unstarted_header_hex"));
    expect(bytesToHex(started)).toBe(stringField(output, "started_header_hex"));
    expect(bytesToHex(workerProofInput(unstarted))).toBe(stringField(output, "unstarted_proof_input_hex"));
    expect(bytesToHex(workerProofInput(started))).toBe(stringField(output, "started_proof_input_hex"));
    const key = stringField(input, "proof_key_utf8");
    expect(await hmacSha256Base64Url(key, workerProofInput(unstarted)))
      .toBe(stringField(output, "unstarted_proof_base64url"));
    expect(await hmacSha256Base64Url(key, workerProofInput(started)))
      .toBe(stringField(output, "started_proof_base64url"));
    expect(bytesToHex(await sha256(started))).toBe(stringField(output, "h0_hex"));
  });

  it("matches exact schedule bytes and hash", async () => {
    const fixture = golden.canonical.schedule;
    const seed = hexToBytes(stringField(fixture.input, "seed_hex"));
    const category = stringField(fixture.input, "category");
    const bytes = scheduleBytes(seed, category);
    expect(bytesToHex(bytes)).toBe(stringField(fixture.output, "bytes_hex"));
    expect(bytesToHex(await sha256(bytes))).toBe(stringField(fixture.output, "sha256_hex"));
  });

  it("matches every event record, chained hash, and stored event", async () => {
    for (const fixture of golden.canonical.events) {
      const event: ProtocolEvent = {
        seq: BigInt(numberField(fixture.input, "seq")),
        activeMs: BigInt(stringField(fixture.input, "active_ms")),
        payload: payloadFromFixture(fixture.name, objectField(fixture.input, "payload")),
      };
      const previous = hexToBytes(stringField(fixture.input, "previous_hash_hex"));
      const record = eventRecord(event);
      const stored = await chainEvent(previous, event);
      expect(bytesToHex(record), fixture.name).toBe(stringField(fixture.output, "event_record_hex"));
      expect(record.length, fixture.name).toBe(numberField(fixture.output, "event_record_length"));
      expect(bytesToHex(stored.eventHash), fixture.name).toBe(stringField(fixture.output, "event_hash_hex"));
      expect(bytesToHex(new Uint8Array([...stored.record, ...stored.eventHash])), fixture.name)
        .toBe(stringField(fixture.output, "stored_event_hex"));
    }
  });

  it("reconstructs the exact ledger, evidence, aggregates, and final root", async () => {
    const fixture = golden.canonical.ledger;
    const started = hexToBytes(stringField(fixture.input, "started_header_hex"));
    const ledger = await CanonicalLedger.create(started);
    const events = fixture.input["events"];
    if (!Array.isArray(events) || events.length !== 2) throw new TypeError("invalid ledger fixture");
    const delivery = events[0] as JsonRecord;
    const terminalEvent = events[1] as JsonRecord;
    const terminal = terminalFromFixture(objectField(terminalEvent, "payload"));
    await ledger.append({
      seq: BigInt(numberField(delivery, "seq")),
      activeMs: BigInt(numberField(delivery, "active_ms")),
      payload: payloadFromFixture("package_delivery", objectField(delivery, "payload")),
    });
    await ledger.append({
      seq: BigInt(numberField(terminalEvent, "seq")),
      activeMs: BigInt(numberField(terminalEvent, "active_ms")),
      payload: { type: "terminal", terminal },
    });
    const output = fixture.output;
    const evidence = ledger.evidence(stringField(fixture.input, "session_id"));
    expect(bytesToHex(ledger.h0)).toBe(stringField(output, "h0_hex"));
    expect(bytesToHex(ledger.lastHash)).toBe(stringField(output, "hN_hex"));
    expect(bytesToHex(ledger.storedBytes())).toBe(stringField(output, "stored_ledger_hex"));
    expect(bytesToHex(evidence)).toBe(stringField(output, "evidence_bytes_hex"));
    expect(bytesToHex(await sha256(evidence))).toBe(stringField(output, "evidence_hash_hex"));
    expect(bytesToHex(terminalBytes(terminal))).toBe(stringField(output, "conduct_aggregates_hex"));
    expect(bytesToHex(await ledger.root())).toBe(stringField(output, "final_root_hex"));
  });

  it.each([
    ["right_of_way", () => golden.canonical.score_hmac_right_of_way_contract_golden],
    ["cluck_hunt", () => golden.canonical.score_hmac_cluck_hunt],
  ])("matches exact %s score input bytes and HMAC signature", async (_name, getFixture) => {
    const fixture = getFixture();
    const input = scoreInputFromFixture(fixture.input);
    const bytes = scoreHmacInput(input);
    expect(bytesToHex(bytes)).toBe(stringField(fixture.output, "score_input_hex"));
    expect(bytes.length).toBe(numberField(fixture.output, "score_input_length"));
    expect(await scoreHmacSignature(stringField(fixture.input, "key_utf8"), input))
      .toBe(stringField(fixture.output, "hmac_sha256_base64url"));
  });
});

describe("rules v3 bigint RightOfWay arithmetic parity", () => {
  it("matches all checked premium/guilt and terminal boundaries", () => {
    for (const fixture of golden.arithmetic_boundaries.credited_positive) {
      expectResult(fixture.output, () => creditedPositive(
        BigInt(fixture.input.base),
        BigInt(fixture.input.premium_bps),
        fixture.input.guilt,
      ));
    }
    for (const fixture of golden.arithmetic_boundaries.right_of_way_terminal) {
      expectResult(fixture.output, () => rightOfWayTerminal(BigInt(fixture.accumulator)));
    }
  });

  it("matches all bigint wave and clock boundaries, including u64 max", () => {
    for (const fixture of golden.arithmetic_boundaries.completed_waves) {
      expect(completedWaves(BigInt(fixture.input_ms))).toBe(BigInt(fixture.output));
    }
    for (const fixture of golden.arithmetic_boundaries.coin_clock) {
      expect(coinClock(BigInt(fixture.input_ms))).toBe(BigInt(fixture.output_ms));
    }
    for (const fixture of golden.arithmetic_boundaries.package_clock) {
      expect(packageClock(BigInt(fixture.input_ms))).toBe(BigInt(fixture.output_ms));
    }
    for (const fixture of golden.arithmetic_boundaries.time_pickup_clock) {
      expect(timePickupClock(BigInt(fixture.input_ms))).toBe(BigInt(fixture.output_ms));
    }
  });

  it("preserves exact package, premium, guilt, accumulator, and time transitions", () => {
    const row = new RightOfWay(1_000n);
    expect([row.pickupPackage(), row.pickupPackage(), row.pickupPackage(), row.pickupPackage()])
      .toEqual([true, true, true, false]);
    expect(row.deliverPackage()?.credited).toBe(5n);
    expect(row.deliverPackage()?.credited).toBe(6n);
    expect(row.deliverPackage()?.credited).toBe(7n);
    expect(row.remainingMs).toBe(10_000n);
    expect(row.animalHit()).toEqual([18n, 8n]);
    expect(row.premiumBps).toBe(9_000n);
    expect(row.pickupPackage()).toBe(true);
    expect(row.deliverPackage()?.credited).toBe(2n);
    expect(row.accumulator).toBe(10n);
    expect(row.terminalTotal()).toBe(10n);
  });
});

describe("rules v3 Drowned and strict decimal parity", () => {
  it("publishes all terminal reasons for both conducts", () => {
    const vectors = (golden as unknown as { terminal_reason_vectors: readonly { conduct: string; reason: string; reason_ordinal: number; terminal: JsonRecord; conduct_aggregates_hex: string; event_record_hex: string }[] }).terminal_reason_vectors;
    expect(vectors.map((v) => [v.conduct, v.reason, v.reason_ordinal])).toEqual([
      ["cluck_hunt", "time_up", 1], ["right_of_way", "time_up", 1],
      ["cluck_hunt", "wrecked", 2], ["right_of_way", "wrecked", 2],
      ["cluck_hunt", "drowned", 3], ["right_of_way", "drowned", 3],
    ]);
    for (const vector of vectors) {
      const terminal = terminalFromFixture(vector.terminal);
      expect(bytesToHex(terminalBytes(terminal))).toBe(vector.conduct_aggregates_hex);
      expect(bytesToHex(eventRecord({ seq: 0n, activeMs: 60_000n, payload: { type: "terminal", terminal } })))
        .toBe(vector.event_record_hex);
    }
  });

  it("matches every immutable minimal Drowned proof, ledger, root, hash, and HMAC", async () => {
    type Drowned = { conduct: string; input: JsonRecord; output: JsonRecord };
    const vectors = (golden as unknown as { drowned_vectors: readonly Drowned[] }).drowned_vectors;
    expect(vectors).toHaveLength(2);
    for (const vector of vectors) {
      const input = vector.input; const output = vector.output;
      const seed = hexToBytes(stringField(input, "seed_hex"));
      const category = stringField(input, "category");
      const seedHash = await seedCommitment(seed); const schedule = await scheduleHash(seed, category);
      expect(bytesToHex(seedHash)).toBe(stringField(output, "seed_commitment_hex"));
      expect(bytesToHex(schedule)).toBe(stringField(output, "schedule_hash_hex"));
      const headerInput = { category, sessionId: stringField(input, "session_id"), challenge: stringField(input, "challenge"), seedCommitment: seedHash, scheduleHash: schedule, issuedAtMs: BigInt(stringField(input, "issued_at_ms")) };
      const unstarted = unstartedSessionHeader(headerInput, BigInt(stringField(input, "start_by_expiry_ms")));
      const started = startedSessionHeader(headerInput, BigInt(stringField(input, "started_at_ms")));
      expect(bytesToHex(unstarted)).toBe(stringField(output, "unstarted_header_hex"));
      expect(bytesToHex(started)).toBe(stringField(output, "started_header_hex"));
      expect(await hmacSha256Base64Url(stringField(input, "proof_key_utf8"), workerProofInput(unstarted))).toBe(stringField(output, "unstarted_proof_base64url"));
      expect(await hmacSha256Base64Url(stringField(input, "proof_key_utf8"), workerProofInput(started))).toBe(stringField(output, "started_proof_base64url"));
      const terminal = terminalFromFixture(objectField(input, "terminal"));
      const ledger = await CanonicalLedger.create(started);
      await ledger.append({ seq: 0n, activeMs: 60_000n, payload: { type: "terminal", terminal } });
      expect(bytesToHex(ledger.h0)).toBe(stringField(output, "h0_hex"));
      expect(bytesToHex(ledger.lastHash)).toBe(stringField(output, "terminal_event_hash_hex"));
      expect(bytesToHex(ledger.storedBytes())).toBe(stringField(output, "stored_ledger_hex"));
      const evidence = ledger.evidence(stringField(input, "session_id"));
      expect(bytesToHex(evidence)).toBe(stringField(output, "evidence_bytes_hex"));
      expect(bytesToHex(await sha256(evidence))).toBe(stringField(output, "evidence_hash_hex"));
      const root = await ledger.root(); expect(bytesToHex(root)).toBe(stringField(output, "final_root_hex"));
      const score = { category, sessionId: stringField(input, "session_id"), finalRoot: root, scheduleHash: schedule, seedCommitment: seedHash, terminal };
      expect(bytesToHex(scoreHmacInput(score))).toBe(stringField(output, "score_input_hex"));
      expect(await scoreHmacSignature(stringField(input, "client_key_utf8"), score)).toBe(stringField(output, "score_hmac_base64url"));
    }
  });

  it("rejects noncanonical signed decimal and unknown terminal reasons", () => {
    for (const invalid of ["", "+1", "01", "-0", "-01", " 1", "1 ", "1.0", "1e3"]) {
      expect(() => parseCanonicalDecimal(invalid)).toThrow(TypeError);
    }
    expect(parseCanonicalDecimal("-9223372036854775808")).toBe(-9223372036854775808n);
    const valid = terminalFromFixture((golden as unknown as { terminal_reason_vectors: readonly { terminal: JsonRecord }[] }).terminal_reason_vectors[0]!.terminal);
    for (const reason of [0, 4, 255]) expect(() => terminalBytes({ ...valid, reason })).toThrow(RangeError);
  });
});
