// Pure TypeScript mirror of the immutable roady-rules.v3 protocol.
//
// Protocol arithmetic is performed with bigint throughout. Numbers are used
// only for byte-sized ordinals and the explicitly f32 relocation output.

const U8_MAX = 0xffn;
const U32_MAX = 0xffff_ffffn;
const U64_MAX = 0xffff_ffff_ffff_ffffn;
const I64_MIN = -0x8000_0000_0000_0000n;
const I64_MAX = 0x7fff_ffff_ffff_ffffn;
const MASK64 = U64_MAX;

export const PROTOCOL_VERSION = 3;
export const PROTOCOL_ID = "roady-protocol.v3";
export const RULES_VERSION = 3;
export const RULES_ID = "roady-rules.v3";
export const POLICY_VERSION = 1;
export const POLICY_ID = "roady-ranked-policy.v3.1";
export const MODE = "rotation";
export const CLUCK_HUNT_CATEGORY = "rotation.v2.cluck_hunt";
export const RIGHT_OF_WAY_CATEGORY = "rotation.v2.right_of_way";

export const SCHEDULE_SEGMENTS = 16;
export const INITIAL_GRACE_MS = 8_000n;
export const TELEGRAPH_MS = 3_000n;
export const ACTIVE_MS = 18_000n;
export const COOLDOWN_MS = 7_000n;
export const CADENCE_MS = 28_000n;
export const EVENT_WINDOWS: readonly (readonly [bigint, bigint])[] = [
  [15_000n, 23_000n],
  [40_000n, 48_000n],
];

export const ROTATION_DOMAIN = "roady.rotation.v3.rotation";
export const SCHEDULED_EVENTS_DOMAIN = "roady.rotation.v3.scheduled_events";
export const FRENZY_INTERVAL_DOMAIN = "roady.rotation.v3.frenzy.interval";
export const FRENZY_ROLL_DOMAIN = "roady.rotation.v3.frenzy.roll";
export const FRENZY_KIND_DOMAIN = "roady.rotation.v3.frenzy.kind";
export const FRENZY_POSITION_DOMAIN = "roady.rotation.v3.frenzy.position";
export const FRENZY_RELOCATION_DOMAIN = "roady.rotation.v3.frenzy.relocation";

export const Effect = {
  Standard: 0,
  RushHour: 1,
  ChickenFrenzy: 2,
  Stampede: 3,
  GlassCannon: 4,
} as const;
export type EffectOrdinal = (typeof Effect)[keyof typeof Effect];

export const ScheduledEvent = {
  TrafficSurge: 0,
  ChickenBurst: 1,
  ComboFrenzy: 2,
  CritterBurst: 3,
} as const;
export type ScheduledEventOrdinal = (typeof ScheduledEvent)[keyof typeof ScheduledEvent];

export const EventKind = {
  ChickenHit: 1,
  CoinCollected: 2,
  TimePickup: 3,
  ObjectiveCompleted: 4,
  CritterPenalty: 5,
  SegmentChanged: 6,
  Terminal: 7,
  PackagePickup: 8,
  PackageDelivery: 9,
  CourtesyAward: 10,
  AnimalHit: 11,
  WaveAward: 12,
  CoinAward: 13,
  FrenzyChanged: 14,
} as const;

export const MAX_EVENTS = 4_096n;
export const MAX_EVENT_RECORD_BYTES = 192;
export const MAX_LEDGER_BYTES = 262_144;
export const MAX_EVIDENCE_BYTES = 524_288;
export const MAX_LP4_BYTES = 524_288;
export const MAX_BUILD_BYTES = 64;

const ENCODER = new TextEncoder();

function checked(value: bigint, minimum: bigint, maximum: bigint, name: string): bigint {
  if (value < minimum || value > maximum) throw new RangeError(`${name} is out of range`);
  return value;
}

export function parseCanonicalDecimal(value: string, name = "integer"): bigint {
  if (!/^(?:0|-[1-9][0-9]*|[1-9][0-9]*)$/.test(value)) {
    throw new TypeError(`${name} must be canonical signed decimal`);
  }
  return BigInt(value);
}

function asBigInt(value: bigint | number | string, name: string): bigint {
  if (typeof value === "string") return parseCanonicalDecimal(value, name);
  if (typeof value === "number" && !Number.isSafeInteger(value)) {
    throw new RangeError(`${name} must be a safe integer or bigint`);
  }
  try { return BigInt(value); } catch { throw new TypeError(`${name} must be an integer`); }
}

export function hexToBytes(value: string): Uint8Array {
  if (!/^(?:[0-9a-fA-F]{2})*$/.test(value)) throw new TypeError("invalid hex");
  const output = new Uint8Array(value.length / 2);
  for (let index = 0; index < output.length; index++) {
    output[index] = Number.parseInt(value.slice(index * 2, index * 2 + 2), 16);
  }
  return output;
}

export function bytesToHex(value: Uint8Array): string {
  return Array.from(value, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export function concatBytes(...values: readonly Uint8Array[]): Uint8Array {
  const length = values.reduce((sum, value) => sum + value.length, 0);
  const output = new Uint8Array(length);
  let offset = 0;
  for (const value of values) {
    output.set(value, offset);
    offset += value.length;
  }
  return output;
}

export async function sha256(value: Uint8Array): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", value));
}

function toBase64Url(value: Uint8Array): string {
  let binary = "";
  for (const byte of value) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export async function hmacSha256(
  key: string | Uint8Array,
  input: Uint8Array,
): Promise<Uint8Array> {
  const raw = typeof key === "string" ? ENCODER.encode(key) : key;
  const cryptoKey = await crypto.subtle.importKey(
    "raw",
    raw,
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  return new Uint8Array(await crypto.subtle.sign("HMAC", cryptoKey, input));
}

export async function hmacSha256Base64Url(
  key: string | Uint8Array,
  input: Uint8Array,
): Promise<string> {
  return toBase64Url(await hmacSha256(key, input));
}

const SPLITMIX_INCREMENT = 0x9e37_79b9_7f4a_7c15n;
const FNV_OFFSET = 0xcbf2_9ce4_8422_2325n;
const FNV_PRIME = 0x0000_0100_0000_01b3n;

function wrap64(value: bigint): bigint {
  return value & MASK64;
}

export function splitmixFinalizer(value: bigint): bigint {
  let z = wrap64(value);
  z = wrap64((z ^ (z >> 30n)) * 0xbf58_476d_1ce4_e5b9n);
  z = wrap64((z ^ (z >> 27n)) * 0x94d0_49bb_1331_11ebn);
  return wrap64(z ^ (z >> 31n));
}

export function streamFnv(seed: Uint8Array, domain: string): bigint {
  if (seed.length !== 32) throw new RangeError("seed must be 32 bytes");
  const domainBytes = ENCODER.encode(domain);
  if (domainBytes.length > Number(U32_MAX)) throw new RangeError("domain is too long");
  const length = domainBytes.length;
  const lengthLe = new Uint8Array([
    length & 0xff,
    (length >>> 8) & 0xff,
    (length >>> 16) & 0xff,
    (length >>> 24) & 0xff,
  ]);
  let hash = FNV_OFFSET;
  for (const byte of concatBytes(seed, lengthLe, domainBytes)) {
    hash = wrap64((hash ^ BigInt(byte)) * FNV_PRIME);
  }
  return hash;
}

export function streamState(seed: Uint8Array, domain: string): bigint {
  return splitmixFinalizer(streamFnv(seed, domain));
}

export class SplitMix64 {
  public constructor(public state: bigint) {
    this.state = wrap64(state);
  }

  public nextU64(): bigint {
    this.state = wrap64(this.state + SPLITMIX_INCREMENT);
    return splitmixFinalizer(this.state);
  }

  public range(n: bigint): bigint {
    if (n <= 0n || n > U64_MAX) throw new RangeError("SplitMix range must be nonzero u64");
    return this.nextU64() % n;
  }
}

export function stream(seed: Uint8Array, domain: string): SplitMix64 {
  return new SplitMix64(streamState(seed, domain));
}

export interface RotationWindow {
  readonly effect: EffectOrdinal;
  readonly telegraphStartMs: bigint;
  readonly activeStartMs: bigint;
  readonly activeEndMs: bigint;
  readonly cooldownEndMs: bigint;
}

function rotationPool(index: bigint): EffectOrdinal {
  if (index === 0n) return Effect.RushHour;
  if (index === 1n) return Effect.Stampede;
  return Effect.GlassCannon;
}

function cyclicSuccessor(effect: EffectOrdinal): EffectOrdinal {
  if (effect === Effect.RushHour) return Effect.Stampede;
  if (effect === Effect.Stampede) return Effect.GlassCannon;
  return Effect.RushHour;
}

export function windowTimes(index: number, effect: EffectOrdinal): RotationWindow {
  const telegraphStartMs = INITIAL_GRACE_MS + BigInt(index) * CADENCE_MS;
  const activeStartMs = telegraphStartMs + TELEGRAPH_MS;
  const activeEndMs = activeStartMs + ACTIVE_MS;
  return {
    effect,
    telegraphStartMs,
    activeStartMs,
    activeEndMs,
    cooldownEndMs: activeEndMs + COOLDOWN_MS,
  };
}

export function rotationSchedule(seed: Uint8Array): readonly RotationWindow[] {
  const rng = stream(seed, ROTATION_DOMAIN);
  const result: RotationWindow[] = [];
  let previous: EffectOrdinal | undefined;
  for (let index = 0; index < SCHEDULE_SEGMENTS; index++) {
    let effect = rotationPool(rng.range(3n));
    if (effect === previous) {
      effect = rotationPool(rng.range(3n));
      if (effect === previous) effect = cyclicSuccessor(effect);
    }
    previous = effect;
    result.push(windowTimes(index, effect));
  }
  return result;
}

export function activeEffectAt(
  schedule: readonly RotationWindow[],
  activeMs: bigint,
): EffectOrdinal | undefined {
  return schedule.find(
    (window) => activeMs >= window.activeStartMs && activeMs < window.activeEndMs,
  )?.effect;
}

export function scheduledEvents(
  seed: Uint8Array,
  schedule: readonly RotationWindow[],
): readonly ScheduledEventOrdinal[] {
  const rng = stream(seed, SCHEDULED_EVENTS_DOMAIN);
  return EVENT_WINDOWS.map(([start]) => {
    const active = activeEffectAt(schedule, start);
    let eligible: readonly ScheduledEventOrdinal[];
    if (active === Effect.RushHour) {
      eligible = [ScheduledEvent.ChickenBurst, ScheduledEvent.ComboFrenzy, ScheduledEvent.CritterBurst];
    } else if (active === Effect.GlassCannon) {
      eligible = [ScheduledEvent.TrafficSurge, ScheduledEvent.ChickenBurst, ScheduledEvent.CritterBurst];
    } else if (active === Effect.Stampede) {
      eligible = [ScheduledEvent.TrafficSurge, ScheduledEvent.ChickenBurst, ScheduledEvent.ComboFrenzy];
    } else {
      eligible = [ScheduledEvent.TrafficSurge, ScheduledEvent.ChickenBurst, ScheduledEvent.ComboFrenzy];
    }
    return eligible[Number(rng.range(3n))]!;
  });
}

export interface FrenzyOpportunity {
  readonly atMs: bigint;
  readonly rollResidue: bigint;
  readonly spawn: boolean;
  readonly pity: boolean;
}

export function frenzyOpportunities(seed: Uint8Array, throughMs: bigint): readonly FrenzyOpportunity[] {
  const intervals = stream(seed, FRENZY_INTERVAL_DOMAIN);
  const rolls = stream(seed, FRENZY_ROLL_DOMAIN);
  let at = 8_000n + intervals.range(4_001n);
  const result: FrenzyOpportunity[] = [];
  while (at <= throughMs) {
    const rollResidue = rolls.range(10_000n);
    const pity = at >= 55_000n;
    const spawn = rollResidue < 400n || pity;
    result.push({ atMs: at, rollResidue, spawn, pity });
    if (spawn) break;
    const interval = 8_000n + intervals.range(4_001n);
    at = at > U64_MAX - interval ? U64_MAX : at + interval;
  }
  return result;
}

export function frenzyRelocationCandidates(seed: Uint8Array): readonly (readonly [number, number])[] {
  const rng = stream(seed, FRENZY_RELOCATION_DOMAIN);
  return Array.from({ length: 8 }, () => {
    const lateralUnits = Number(rng.nextU64() % 2_001n) - 1_000;
    const aheadUnits = Number(rng.nextU64() % 1_001n);
    const lateral = Math.fround(Math.fround(Math.fround(lateralUnits) * 22) / 1_000);
    const aheadOffset = Math.fround(Math.fround(Math.fround(aheadUnits) * 11.25) / 1_000);
    const ahead = Math.fround(13.75 + aheadOffset);
    return [lateral, ahead] as const;
  });
}

export function comboMultiplier(count: bigint): bigint {
  checked(count, 0n, U32_MAX, "combo count");
  if (count <= 4n) return 1n;
  if (count <= 9n) return 2n;
  if (count <= 14n) return 3n;
  if (count <= 19n) return 4n;
  return 5n;
}

export function completedWaves(activeMs: bigint): bigint {
  if (activeMs < 36_000n) return 0n;
  const waves = 1n + (activeMs - 36_000n) / CADENCE_MS;
  return waves > U32_MAX ? U32_MAX : waves;
}

export class ArithmeticOverflowError extends RangeError {
  public constructor() {
    super("arithmetic_overflow");
    this.name = "ArithmeticOverflowError";
  }
}

function checkedU32(value: bigint): bigint {
  if (value < 0n || value > U32_MAX) throw new ArithmeticOverflowError();
  return value;
}

function checkedI64(value: bigint): bigint {
  if (value < I64_MIN || value > I64_MAX) throw new ArithmeticOverflowError();
  return value;
}

export function creditedPositive(base: bigint, premiumBps: bigint, guilt: boolean): bigint {
  checked(base, 0n, U32_MAX, "base");
  checked(premiumBps, 0n, U32_MAX, "premiumBps");
  const premiumValue = (base * premiumBps) / 10_000n;
  const credited = guilt ? (premiumValue * 5_000n) / 10_000n : premiumValue;
  return checkedU32(credited);
}

export function rightOfWayTerminal(accumulator: bigint): bigint {
  checkedI64(accumulator);
  return checkedU32(accumulator < 0n ? 0n : accumulator);
}

export interface PositiveTransition {
  readonly base: bigint;
  readonly credited: bigint;
  readonly before: bigint;
  readonly after: bigint;
}

export class RightOfWay {
  public accumulator = 0n;
  public premiumBps = 10_000n;
  public deliveryChain = 0n;
  public maxDeliveryChain = 0n;
  public carriedPackages = 0n;
  public packagesDelivered = 0n;
  public courtesyCount = 0n;
  public coinsCollected = 0n;
  public animalHits = 0n;
  public objectiveCompleted = false;
  public guiltRemainingMs = 0n;
  public remainingMs = 0n;

  public constructor(remainingMs = 0n) {
    this.remainingMs = remainingMs;
  }

  public tickGuilt(elapsedMs: bigint): void {
    this.guiltRemainingMs = this.guiltRemainingMs > elapsedMs
      ? this.guiltRemainingMs - elapsedMs
      : 0n;
  }

  public pickupPackage(): boolean {
    if (this.carriedPackages >= 3n) return false;
    this.carriedPackages += 1n;
    return true;
  }

  public positiveAward(base: bigint): PositiveTransition {
    const credited = creditedPositive(base, this.premiumBps, this.guiltRemainingMs !== 0n);
    const before = this.accumulator;
    const after = checkedI64(before + credited);
    this.accumulator = after;
    return { base, credited, before, after };
  }

  public deliverPackage(): PositiveTransition | undefined {
    if (this.carriedPackages === 0n) return undefined;
    const base = checkedU32(5n + this.deliveryChain);
    const credited = creditedPositive(base, this.premiumBps, this.guiltRemainingMs !== 0n);
    const after = checkedI64(this.accumulator + credited);
    const deliveryChain = checkedU32(this.deliveryChain + 1n);
    const packagesDelivered = checkedU32(this.packagesDelivered + 1n);
    const result = { base, credited, before: this.accumulator, after };
    this.accumulator = after;
    this.deliveryChain = deliveryChain;
    if (deliveryChain > this.maxDeliveryChain) this.maxDeliveryChain = deliveryChain;
    this.packagesDelivered = packagesDelivered;
    this.carriedPackages -= 1n;
    this.remainingMs = packageClock(this.remainingMs);
    return result;
  }

  public coin(): PositiveTransition {
    const credited = creditedPositive(1n, this.premiumBps, this.guiltRemainingMs !== 0n);
    const after = checkedI64(this.accumulator + credited);
    const coins = checkedU32(this.coinsCollected + 1n);
    const result = { base: 1n, credited, before: this.accumulator, after };
    this.accumulator = after;
    this.coinsCollected = coins;
    this.remainingMs = coinClock(this.remainingMs);
    return result;
  }

  public courtesy(): PositiveTransition {
    const credited = creditedPositive(2n, this.premiumBps, this.guiltRemainingMs !== 0n);
    const after = checkedI64(this.accumulator + credited);
    const courtesyCount = credited > 0n
      ? checkedU32(this.courtesyCount + 1n)
      : this.courtesyCount;
    const transition = { base: 2n, credited, before: this.accumulator, after };
    this.accumulator = after;
    this.courtesyCount = courtesyCount;
    return transition;
  }

  public objective(): PositiveTransition | undefined {
    if (this.objectiveCompleted) return undefined;
    const transition = this.positiveAward(10n);
    this.objectiveCompleted = true;
    return transition;
  }

  public wave(ranked: boolean): PositiveTransition | undefined {
    return ranked ? this.positiveAward(2n) : undefined;
  }

  public animalHit(): readonly [bigint, bigint] {
    const before = this.accumulator;
    const after = checkedI64(before - 10n);
    const animalHits = checkedU32(this.animalHits + 1n);
    this.accumulator = after;
    this.premiumBps = (this.premiumBps * 9_000n) / 10_000n;
    this.deliveryChain = 0n;
    this.guiltRemainingMs = 5_000n;
    this.animalHits = animalHits;
    return [before, after];
  }

  public terminalTotal(): bigint {
    return rightOfWayTerminal(this.accumulator);
  }
}

export function coinClock(currentMs: bigint): bigint {
  const current = currentMs > 90_000n ? 90_000n : currentMs;
  return current + 1_500n > 90_000n ? 90_000n : current + 1_500n;
}

export function packageClock(currentMs: bigint): bigint {
  return currentMs > 87_000n ? 90_000n : currentMs + 3_000n;
}

export function timePickupClock(currentMs: bigint): bigint {
  return currentMs > 94_000n ? 99_000n : currentMs + 5_000n;
}

export class CanonicalWriter {
  readonly #bytes: number[] = [];

  public u8(value: bigint | number): void {
    const integer = checked(asBigInt(value, "u8"), 0n, U8_MAX, "u8");
    this.#bytes.push(Number(integer));
  }

  #fixed(value: bigint | number | string, bytes: number, signed: boolean, name: string): void {
    let integer = asBigInt(value, name);
    const bits = BigInt(bytes * 8);
    const minimum = signed ? -(1n << (bits - 1n)) : 0n;
    const maximum = signed ? (1n << (bits - 1n)) - 1n : (1n << bits) - 1n;
    checked(integer, minimum, maximum, name);
    if (integer < 0n) integer += 1n << bits;
    for (let shift = bytes - 1; shift >= 0; shift--) {
      this.#bytes.push(Number((integer >> BigInt(shift * 8)) & 0xffn));
    }
  }

  public u16(value: bigint | number): void { this.#fixed(value, 2, false, "u16"); }
  public u32(value: bigint | number): void { this.#fixed(value, 4, false, "u32"); }
  public i32(value: bigint | number): void { this.#fixed(value, 4, true, "i32"); }
  public u64(value: bigint | number | string): void { this.#fixed(value, 8, false, "u64"); }
  public i64(value: bigint | number | string): void { this.#fixed(value, 8, true, "i64"); }

  public raw(value: Uint8Array): void { this.#bytes.push(...value); }

  public raw32(value: Uint8Array): void {
    if (value.length !== 32) throw new RangeError("expected 32 bytes");
    this.raw(value);
  }

  public lp1(value: string): void {
    const bytes = ENCODER.encode(value);
    if (bytes.length === 0) throw new RangeError("lp1 value must not be empty");
    if (bytes.length > 255) throw new RangeError("lp1 value is too long");
    this.u8(bytes.length);
    this.raw(bytes);
  }

  public lp4(value: Uint8Array): void {
    if (value.length > MAX_LP4_BYTES) throw new RangeError("lp4 value is too long");
    this.u32(value.length);
    this.raw(value);
  }

  public bytes(): Uint8Array { return Uint8Array.from(this.#bytes); }
}

export interface SessionHeader {
  readonly category: string;
  readonly sessionId: string;
  readonly challenge: string;
  readonly seedCommitment: Uint8Array;
  readonly scheduleHash: Uint8Array;
  readonly issuedAtMs: bigint;
}

function writeV3Prefix(writer: CanonicalWriter, category: string): void {
  if (category !== CLUCK_HUNT_CATEGORY && category !== RIGHT_OF_WAY_CATEGORY) {
    throw new RangeError("category is not in the v3 tuple");
  }
  writer.u8(PROTOCOL_VERSION); writer.u8(RULES_VERSION); writer.u8(POLICY_VERSION);
  writer.lp1(PROTOCOL_ID); writer.lp1(RULES_ID); writer.lp1(POLICY_ID);
  writer.lp1(MODE); writer.lp1(category);
}

function sessionHeaderPrefix(input: SessionHeader): CanonicalWriter {
  const writer = new CanonicalWriter();
  writer.lp1("roady.v3.session");
  writeV3Prefix(writer, input.category);
  writer.lp1(input.sessionId);
  writer.lp1(input.challenge);
  writer.raw32(input.seedCommitment);
  writer.raw32(input.scheduleHash);
  writer.u64(input.issuedAtMs);
  return writer;
}

export function unstartedSessionHeader(input: SessionHeader, startByExpiryMs: bigint): Uint8Array {
  const writer = sessionHeaderPrefix(input);
  writer.u64(startByExpiryMs);
  writer.u8(0);
  writer.u64(0n);
  return writer.bytes();
}

export function startedSessionHeader(input: SessionHeader, startedAtMs: bigint): Uint8Array {
  const writer = sessionHeaderPrefix(input);
  writer.u64(0n);
  writer.u8(1);
  writer.u64(startedAtMs);
  return writer.bytes();
}

export function workerProofInput(header: Uint8Array): Uint8Array {
  const writer = new CanonicalWriter();
  writer.lp1("roady.v3.proof");
  writer.raw(header);
  return writer.bytes();
}

export function scheduleBytes(seed: Uint8Array, category: string): Uint8Array {
  const writer = new CanonicalWriter();
  writer.lp1("roady.v3.schedule");
  writeV3Prefix(writer, category);
  writer.raw32(seed);
  writer.u16(SCHEDULE_SEGMENTS);
  for (const window of rotationSchedule(seed)) {
    writer.u8(window.effect);
    writer.u64(window.telegraphStartMs);
    writer.u64(window.activeStartMs);
    writer.u64(window.activeEndMs);
    writer.u64(window.cooldownEndMs);
  }
  return writer.bytes();
}

export async function seedCommitment(seed: Uint8Array): Promise<Uint8Array> {
  const writer = new CanonicalWriter();
  writer.lp1("roady.v3.seed");
  writer.raw32(seed);
  return sha256(writer.bytes());
}

export async function scheduleHash(seed: Uint8Array, category: string): Promise<Uint8Array> {
  return sha256(scheduleBytes(seed, category));
}

export interface CluckTerminal {
  readonly conduct: "cluck_hunt";
  readonly reason: number;
  readonly total: bigint;
  readonly chickens: bigint;
  readonly coins: bigint;
  readonly objectiveCompleted: boolean;
  readonly maxCombo: number;
  readonly durationMs: bigint;
  readonly remainingMs: bigint;
  readonly build: string;
  readonly platform: number;
}

export interface RightOfWayTerminal {
  readonly conduct: "right_of_way";
  readonly reason: number;
  readonly total: bigint;
  readonly accumulator: bigint;
  readonly premiumBps: bigint;
  readonly packagesDelivered: bigint;
  readonly courtesyCount: bigint;
  readonly animalHits: bigint;
  readonly maxDeliveryChain: bigint;
  readonly objectiveCompleted: boolean;
  readonly durationMs: bigint;
  readonly remainingMs: bigint;
  readonly build: string;
  readonly platform: number;
}

export type ConductTerminal = CluckTerminal | RightOfWayTerminal;

export function terminalBytes(value: ConductTerminal): Uint8Array {
  const buildLength = ENCODER.encode(value.build).length;
  if (buildLength === 0 || buildLength > MAX_BUILD_BYTES) throw new RangeError("build length is out of range");
  if (value.reason !== 1 && value.reason !== 2 && value.reason !== 3) throw new RangeError("invalid terminal reason");
  if (value.remainingMs < 0n || value.remainingMs > 99_000n) throw new RangeError("remainingMs is out of range");
  const writer = new CanonicalWriter();
  if (value.conduct === "cluck_hunt") {
    writer.u8(0); writer.u8(value.reason); writer.u32(value.total);
    writer.u32(value.chickens); writer.u32(value.coins);
    writer.u8(value.objectiveCompleted ? 1 : 0); writer.u8(value.maxCombo);
    writer.u64(value.durationMs); writer.u64(value.remainingMs);
    writer.lp1(value.build); writer.u8(value.platform);
  } else {
    writer.u8(1); writer.u8(value.reason); writer.u32(value.total);
    writer.i64(value.accumulator); writer.u32(value.premiumBps);
    writer.u32(value.packagesDelivered); writer.u32(value.courtesyCount);
    writer.u32(value.animalHits); writer.u32(value.maxDeliveryChain);
    writer.u8(value.objectiveCompleted ? 1 : 0);
    writer.u64(value.durationMs); writer.u64(value.remainingMs);
    writer.lp1(value.build); writer.u8(value.platform);
  }
  return writer.bytes();
}

interface PayloadBase { readonly type: string }
export type EventPayload = PayloadBase & Readonly<Record<string, unknown>>;
export interface ProtocolEvent {
  readonly seq: bigint;
  readonly activeMs: bigint;
  readonly payload: EventPayload;
}

function integer(payload: EventPayload, key: string): bigint {
  const value = payload[key];
  if (typeof value !== "bigint" && typeof value !== "number" && typeof value !== "string") {
    throw new TypeError(`${key} must be an integer`);
  }
  return asBigInt(value, key);
}
function ordinal(payload: EventPayload, key: string): number {
  return Number(integer(payload, key));
}
function boolean(payload: EventPayload, key: string): boolean {
  const value = payload[key];
  if (typeof value !== "boolean") throw new TypeError(`${key} must be boolean`);
  return value;
}

export function eventKind(payload: EventPayload): number {
  switch (payload.type) {
    case "chicken_hit": return EventKind.ChickenHit;
    case "coin_collected": return EventKind.CoinCollected;
    case "time_pickup": return EventKind.TimePickup;
    case "objective_completed_cluck":
    case "objective_completed_right_of_way": return EventKind.ObjectiveCompleted;
    case "critter_penalty": return EventKind.CritterPenalty;
    case "segment_changed": return EventKind.SegmentChanged;
    case "terminal": return EventKind.Terminal;
    case "package_pickup": return EventKind.PackagePickup;
    case "package_delivery": return EventKind.PackageDelivery;
    case "courtesy_award": return EventKind.CourtesyAward;
    case "animal_hit": return EventKind.AnimalHit;
    case "wave_award": return EventKind.WaveAward;
    case "coin_award": return EventKind.CoinAward;
    case "frenzy_changed": return EventKind.FrenzyChanged;
    default: throw new TypeError(`unknown event payload: ${payload.type}`);
  }
}

function writePayload(writer: CanonicalWriter, payload: EventPayload): void {
  const u8 = (key: string): void => writer.u8(ordinal(payload, key));
  const u32 = (key: string): void => writer.u32(integer(payload, key));
  const i32 = (key: string): void => writer.i32(integer(payload, key));
  const u64 = (key: string): void => writer.u64(integer(payload, key));
  const i64 = (key: string): void => writer.i64(integer(payload, key));
  switch (payload.type) {
    case "chicken_hit":
      u32("base"); u32("eventBonus"); u32("frenzyBonus"); u8("comboBefore");
      u8("comboAfter"); u32("bucketBefore"); u32("bucketAfter"); break;
    case "coin_collected":
      writer.u8(boolean(payload, "mega") ? 1 : 0); u32("base"); u8("comboBefore");
      u8("comboAfter"); u32("bucketBefore"); u32("bucketAfter");
      u64("remainingBeforeMs"); u64("remainingAfterMs"); break;
    case "time_pickup": u64("remainingBeforeMs"); u64("remainingAfterMs"); break;
    case "objective_completed_cluck":
      u8("objective"); u32("target"); u32("baseReward"); u32("bucketBefore"); u32("bucketAfter"); break;
    case "critter_penalty":
      u32("penalty"); u32("bucketBefore"); u32("bucketAfter"); u64("cooldownAfterMs"); break;
    case "segment_changed":
      u8("segmentKind"); u8("effectOrEvent"); writer.u8(boolean(payload, "active") ? 1 : 0);
      u64("startMs"); u64("endMs"); break;
    case "terminal": {
      const terminal = payload["terminal"];
      if (typeof terminal !== "object" || terminal === null) throw new TypeError("terminal is required");
      writer.raw(terminalBytes(terminal as ConductTerminal)); break;
    }
    case "package_pickup": u8("carriedBefore"); u8("carriedAfter"); break;
    case "package_delivery":
      u8("deliveredOrdinalWithinDropoff"); u32("chainIndex"); u32("base"); u32("premiumBps");
      writer.u8(boolean(payload, "guilt") ? 1 : 0); u32("credited"); i64("accumulatorBefore");
      i64("accumulatorAfter"); u64("remainingBeforeMs"); u64("remainingAfterMs"); break;
    case "courtesy_award":
      u32("chickenStableId"); u32("premiumBps"); writer.u8(boolean(payload, "guilt") ? 1 : 0);
      u32("credited"); i64("accumulatorBefore"); i64("accumulatorAfter"); u32("cooldownAfterMs"); break;
    case "animal_hit":
      u8("animalKind"); i32("delta"); u32("premiumBeforeBps"); u32("premiumAfterBps");
      u64("guiltAfterMs"); i64("accumulatorBefore"); i64("accumulatorAfter"); break;
    case "wave_award":
      u32("base"); u32("premiumBps"); writer.u8(boolean(payload, "guilt") ? 1 : 0);
      u32("credited"); i64("accumulatorBefore"); i64("accumulatorAfter"); break;
    case "coin_award":
      u32("base"); u32("premiumBps"); writer.u8(boolean(payload, "guilt") ? 1 : 0);
      u32("credited"); i64("accumulatorBefore"); i64("accumulatorAfter");
      u64("remainingBeforeMs"); u64("remainingAfterMs"); break;
    case "frenzy_changed": u8("phase"); u64("startMs"); u64("endMs"); break;
    case "objective_completed_right_of_way":
      u8("objective"); u32("target"); u32("base"); u32("premiumBps");
      writer.u8(boolean(payload, "guilt") ? 1 : 0); u32("credited");
      i64("accumulatorBefore"); i64("accumulatorAfter"); break;
    default: throw new TypeError(`unknown event payload: ${payload.type}`);
  }
}

export function eventRecord(event: ProtocolEvent): Uint8Array {
  const writer = new CanonicalWriter();
  writer.lp1("roady.v3.event"); writer.u32(event.seq); writer.u64(event.activeMs);
  writer.u8(eventKind(event.payload)); writePayload(writer, event.payload);
  const bytes = writer.bytes();
  if (bytes.length > MAX_EVENT_RECORD_BYTES) throw new RangeError("event record is too long");
  return bytes;
}

export interface StoredEvent { readonly record: Uint8Array; readonly eventHash: Uint8Array }

export async function chainEvent(previousHash: Uint8Array, event: ProtocolEvent): Promise<StoredEvent> {
  if (previousHash.length !== 32) throw new RangeError("previous hash must be 32 bytes");
  const record = eventRecord(event);
  return { record, eventHash: await sha256(concatBytes(previousHash, record)) };
}

export function evidenceBytes(sessionId: string, eventCount: bigint, storedLedger: Uint8Array): Uint8Array {
  if (eventCount > MAX_EVENTS) throw new RangeError("too many events");
  if (storedLedger.length > MAX_LEDGER_BYTES) throw new RangeError("ledger is too long");
  const writer = new CanonicalWriter();
  writer.lp1("roady.v3.evidence"); writer.lp1(sessionId); writer.u32(eventCount); writer.lp4(storedLedger);
  const bytes = writer.bytes();
  if (bytes.length > MAX_EVIDENCE_BYTES) throw new RangeError("evidence is too long");
  return bytes;
}

export async function finalRoot(
  h0: Uint8Array,
  hN: Uint8Array,
  terminal: ConductTerminal,
): Promise<Uint8Array> {
  const writer = new CanonicalWriter();
  writer.lp1("roady.v3.root"); writer.raw32(h0); writer.raw32(hN); writer.raw(terminalBytes(terminal));
  return sha256(writer.bytes());
}

export class CanonicalLedger {
  public readonly h0: Uint8Array;
  public lastHash: Uint8Array;
  public eventCount = 0n;
  #stored: Uint8Array<ArrayBufferLike> = new Uint8Array();
  #terminal: ConductTerminal | undefined;

  private constructor(h0: Uint8Array) {
    this.h0 = h0;
    this.lastHash = h0;
  }

  public static async create(startedHeader: Uint8Array): Promise<CanonicalLedger> {
    return new CanonicalLedger(await sha256(startedHeader));
  }

  public storedBytes(): Uint8Array { return this.#stored.slice(); }

  public async append(event: ProtocolEvent): Promise<Uint8Array> {
    if (this.#terminal !== undefined) throw new Error("no event may follow Terminal");
    if (this.eventCount >= MAX_EVENTS) throw new RangeError("too many events");
    if (event.seq !== this.eventCount) throw new RangeError("invalid event sequence");
    const stored = await chainEvent(this.lastHash, event);
    const next = concatBytes(this.#stored, stored.record, stored.eventHash);
    if (next.length > MAX_LEDGER_BYTES) throw new RangeError("ledger is too long");
    this.#stored = next;
    this.lastHash = stored.eventHash;
    this.eventCount += 1n;
    if (event.payload.type === "terminal") {
      this.#terminal = event.payload["terminal"] as ConductTerminal;
    }
    return this.lastHash;
  }

  #requireTerminal(): ConductTerminal {
    if (this.#terminal === undefined) throw new Error("ledger has no Terminal event");
    return this.#terminal;
  }

  public evidence(sessionId: string): Uint8Array {
    this.#requireTerminal();
    return evidenceBytes(sessionId, this.eventCount, this.#stored);
  }

  public async root(): Promise<Uint8Array> {
    return finalRoot(this.h0, this.lastHash, this.#requireTerminal());
  }
}

export interface ScoreHmacInput {
  readonly category: string;
  readonly sessionId: string;
  readonly finalRoot: Uint8Array;
  readonly scheduleHash: Uint8Array;
  readonly seedCommitment: Uint8Array;
  readonly terminal: ConductTerminal;
}

export function scoreHmacInput(input: ScoreHmacInput): Uint8Array {
  const writer = new CanonicalWriter();
  writer.lp1("roady.v3.score"); writeV3Prefix(writer, input.category);
  writer.lp1(input.sessionId); writer.raw32(input.finalRoot); writer.raw32(input.scheduleHash);
  writer.raw32(input.seedCommitment); writer.raw(terminalBytes(input.terminal));
  return writer.bytes();
}

export async function scoreHmacSignature(
  key: string | Uint8Array,
  input: ScoreHmacInput,
): Promise<string> {
  return hmacSha256Base64Url(key, scoreHmacInput(input));
}
