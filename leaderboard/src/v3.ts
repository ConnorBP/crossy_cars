import { applyCors, errorResponse, fromBase64Url, json, parseAllowedOrigins, toBase64Url } from "./responses";
import { constantTimeEquals, ipHash, randomBase64Url, sha256 } from "./security";
import {
  CLUCK_HUNT_CATEGORY, RIGHT_OF_WAY_CATEGORY, MODE, POLICY_ID,
  PROTOCOL_ID, RULES_ID, MAX_LEDGER_BYTES,
  evidenceBytes, hexToBytes, bytesToHex, scheduleHash, seedCommitment,
  scoreHmacInput, startedSessionHeader, unstartedSessionHeader,
  workerProofInput, hmacSha256Base64Url, type ConductTerminal, type SessionHeader,
} from "./rules-v3";
import { checkRateLimit, isPlainObject, readBoundedJson, type RateLimitBinding } from "../vendor/cloudflare-game-common/src/index";
import { replayEvidence } from "./replay-v3";

export interface V3Env {
  DB: D1Database; BUILD: string; ALLOWED_ORIGINS: string; LB_IP_HASH_PEPPER: string;
  LB_ADMIN_TOKEN: string; LB_TURNSTILE_SECRET: string; ROADY_V3_RANKED_ENABLED?: string;
  LB_V3_PROOF_HMAC_KEY?: string; LB_V3_SEED_ENCRYPTION_KEY?: string; LB_V3_SEED_KEY_ID?: string;
  LB_V3_SEED_ENCRYPTION_KEYS_JSON?: string;
  LB_V3_EVIDENCE_CAPABILITY_KEY?: string; LB_V3_CLIENT_HMAC_KEYS_JSON?: string;
  RATE_LIMIT_READ?: RateLimitBinding; RATE_LIMIT_SESSION?: RateLimitBinding;
  RATE_LIMIT_SUBMIT?: RateLimitBinding; RATE_LIMIT_RANK?: RateLimitBinding;
}

type Cors = Record<string, string> | null;
const CATEGORIES = [CLUCK_HUNT_CATEGORY, RIGHT_OF_WAY_CATEGORY] as const;
const CACHE_CAP = "public, max-age=60, s-maxage=300, stale-while-revalidate=600";
const CACHE_BOARD = "public, max-age=30, s-maxage=60, stale-while-revalidate=120";
const NO_STORE = { "Cache-Control": "no-store" };
const TEST_TURNSTILE_SECRET = "1x0000000000000000000000000000000AA";
const PRODUCTION_PARITY_PROVEN = true; // Deliberately false until section 9 release evidence is approved.
const te = new TextEncoder();

function dev(build: string): boolean { return build === "dev" || build === "test" || build === "local"; }
function placeholder(v: unknown): boolean { return typeof v !== "string" || !v.trim() || /^(?:REPLACE_|PLACEHOLDER)/i.test(v.trim()); }
function enabled(env: V3Env): boolean {
  if (env.ROADY_V3_RANKED_ENABLED !== "true") return false;
  // The environment flag is only an upper bound. Keep production false until
  // separately reviewed deployment-parity evidence lands. Test/local builds
  // may exercise the complete protocol without claiming production readiness.
  return PRODUCTION_PARITY_PROVEN || dev(env.BUILD);
}
function exactObject(value: unknown, fields: readonly string[]): value is Record<string, unknown> {
  if (!isPlainObject(value)) return false;
  const keys = Object.keys(value);
  return keys.length === fields.length && keys.every((key) => fields.includes(key));
}
function category(value: unknown): value is typeof CATEGORIES[number] { return typeof value === "string" && CATEGORIES.includes(value as never); }
function conductFor(value: string): "cluck_hunt" | "right_of_way" { return value === CLUCK_HUNT_CATEGORY ? "cluck_hunt" : "right_of_way"; }
function safeUInt(value: unknown, max = Number.MAX_SAFE_INTEGER): value is number { return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 && value <= max; }
function exactHex(value: unknown): value is string { return typeof value === "string" && /^[0-9a-f]{64}$/.test(value); }
function exactB64(value: unknown): value is string { if (typeof value !== "string" || !/^[A-Za-z0-9_-]+$/.test(value)) return false; try { return toBase64Url(fromBase64Url(value)) === value; } catch { return false; } }
function exactHmac(value:unknown):value is string{return exactB64(value)&&fromBase64Url(value).length===32;}
function utf8(value: unknown, min: number, max: number): value is string { return typeof value === "string" && te.encode(value).length >= min && te.encode(value).length <= max; }
function reasonOrdinal(value: string): number { return value === "time_up" ? 1 : value === "wrecked" ? 2 : value === "drowned" ? 3 : 0; }
function platformOrdinal(value: string): number { return value === "web" ? 1 : value === "native" ? 2 : 0; }
function tuple(body: Record<string, unknown>): boolean {
  return body.protocolVersion === 3 && body.protocolId === PROTOCOL_ID && body.rulesVersion === 3 &&
    body.rulesId === RULES_ID && body.policyVersion === 1 && body.policyId === POLICY_ID &&
    body.mode === MODE && category(body.categoryKey);
}
function err(code: string, message: string, status: number, id: string, cors: Cors): Response { return errorResponse(code, message, status, id, cors); }
async function bounded(request: Request, max: number): Promise<unknown | undefined> { try { return await readBoundedJson(request, max); } catch { return undefined; } }
async function limit(binding: RateLimitBinding | undefined, key: string, bucket: string, required: boolean): Promise<boolean> {
  if (!binding) return !required;
  return checkRateLimit(binding, key, bucket);
}
function clientIp(request: Request): string { return request.headers.get("CF-Connecting-IP") ?? request.headers.get("X-Forwarded-For")?.split(",")[0]?.trim() ?? "127.0.0.1"; }
async function digestHex(bytes: Uint8Array): Promise<string> { return bytesToHex(await sha256(bytes)); }
async function proof(key: string, header: Uint8Array): Promise<string> { return hmacSha256Base64Url(key, workerProofInput(header)); }
function sessionHeader(row: SessionRow): SessionHeader { return { category: row.category_key, sessionId: row.session_id, challenge: row.challenge, seedCommitment: hexToBytes(row.seed_commitment), scheduleHash: hexToBytes(row.schedule_hash), issuedAtMs: BigInt(row.issued_at) }; }

function v3Config(env: V3Env): string | null {
  if (parseAllowedOrigins(env.ALLOWED_ORIGINS).size === 0) return "allowed origins invalid";
  for (const key of ["LB_IP_HASH_PEPPER","LB_ADMIN_TOKEN","LB_TURNSTILE_SECRET","LB_V3_PROOF_HMAC_KEY","LB_V3_SEED_KEY_ID","LB_V3_EVIDENCE_CAPABILITY_KEY","LB_V3_CLIENT_HMAC_KEYS_JSON"] as const) {
    if (placeholder(env[key])) return `${key} missing`;
  }
  if (placeholder(env.LB_V3_SEED_ENCRYPTION_KEY) && placeholder(env.LB_V3_SEED_ENCRYPTION_KEYS_JSON)) return "seed encryption key registry missing";
  try {
    const keys = JSON.parse(env.LB_V3_CLIENT_HMAC_KEYS_JSON!) as unknown;
    if (!isPlainObject(keys) || Object.keys(keys).length < 1 || Object.entries(keys).some(([id, value]) => !/^v3\.client\.[A-Za-z0-9._-]+$/.test(id) || placeholder(value))) return "client key registry invalid";
  } catch { return "client key registry invalid"; }
  try {
    if (env.LB_V3_SEED_ENCRYPTION_KEYS_JSON) {
      const registry=JSON.parse(env.LB_V3_SEED_ENCRYPTION_KEYS_JSON) as unknown;
      if(!isPlainObject(registry)||Object.keys(registry).length<1||Object.entries(registry).some(([id,value])=>!/^v3\.seed\.[A-Za-z0-9._-]+$/.test(id)||typeof value!=="string"||fromBase64Url(value).length!==32)||typeof registry[env.LB_V3_SEED_KEY_ID!]!=="string") return "seed encryption key registry invalid";
    }
    if (seedKeyBytes(env).length !== 32) return "seed encryption key must decode to 32 bytes";
  } catch { return "seed encryption key is invalid"; }
  if (env.LB_TURNSTILE_SECRET === TEST_TURNSTILE_SECRET && !dev(env.BUILD)) return "test Turnstile secret in production";
  return null;
}
function seedKeyBytes(env: V3Env, keyId = env.LB_V3_SEED_KEY_ID): Uint8Array {
  if (env.LB_V3_SEED_ENCRYPTION_KEYS_JSON) {
    const registry = JSON.parse(env.LB_V3_SEED_ENCRYPTION_KEYS_JSON) as Record<string, unknown>;
    const encoded = keyId === undefined ? undefined : registry[keyId];
    if (typeof encoded !== "string") throw new Error("unknown seed key id");
    return fromBase64Url(encoded);
  }
  if (keyId !== env.LB_V3_SEED_KEY_ID) throw new Error("unknown seed key id");
  return fromBase64Url(env.LB_V3_SEED_ENCRYPTION_KEY!);
}
async function encryptSeed(seed: Uint8Array, env: V3Env, aad: string): Promise<{ iv: Uint8Array; ciphertext: Uint8Array }> {
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const key = await crypto.subtle.importKey("raw", seedKeyBytes(env), "AES-GCM", false, ["encrypt"]);
  return { iv, ciphertext: new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: te.encode(aad), tagLength: 128 }, key, seed)) };
}
async function decryptAndVerifySeed(row: SessionRow, env: V3Env): Promise<Uint8Array | null> {
  if (!row.seed_iv || !row.seed_ciphertext || !row.seed_key_id) return null;
  try {
    const keyBytes = seedKeyBytes(env, row.seed_key_id);
    if (keyBytes.length !== 32) return null;
    const key = await crypto.subtle.importKey("raw", keyBytes, "AES-GCM", false, ["decrypt"]);
    const aad = `${row.session_id}|${row.category_key}|${row.seed_commitment}`;
    const seed = new Uint8Array(await crypto.subtle.decrypt({ name:"AES-GCM", iv:new Uint8Array(row.seed_iv), additionalData:te.encode(aad), tagLength:128 }, key, row.seed_ciphertext));
    if (seed.length !== 32) return null;
    return bytesToHex(await seedCommitment(seed)) === row.seed_commitment && bytesToHex(await scheduleHash(seed,row.category_key)) === row.schedule_hash ? seed : null;
  } catch { return null; }
}
async function verifySeedMaterial(row: SessionRow, env: V3Env): Promise<boolean> { return (await decryptAndVerifySeed(row,env)) !== null; }
async function verifyTurnstile(token: string, ip: string, env: V3Env): Promise<boolean> {
  if (env.LB_TURNSTILE_SECRET === TEST_TURNSTILE_SECRET) return dev(env.BUILD);
  const form = new FormData(); form.set("secret", env.LB_TURNSTILE_SECRET); form.set("response", token); form.set("remoteip", ip);
  try {
    const result = await fetch("https://challenges.cloudflare.com/turnstile/v0/siteverify", { method: "POST", body: form });
    const data = await result.json() as { success?: boolean; action?: string; hostname?: string };
    const hosts=Array.from(parseAllowedOrigins(env.ALLOWED_ORIGINS),(origin)=>new URL(origin).hostname);
    return data.success === true && data.action === "roady_score_session" && typeof data.hostname === "string" && hosts.includes(data.hostname);
  } catch { return false; }
}

export async function handleV3(request: Request, env: V3Env, ctx: ExecutionContext, cors: Cors, requestId: string): Promise<Response | null> {
  const url = new URL(request.url); const path = url.pathname; const method = request.method;
  if (!(path === "/v3" || path.startsWith("/v3/"))) return null;
  if (path === "/v3/capabilities" && method === "GET") return capabilities(request, env, ctx, cors, requestId);
  if (path === "/v3/capabilities") return err("not_found","No route for that method",404,requestId,cors);
  if (path === "/v3") return err("not_found", "No route for that path", 404, requestId, cors);
  const knownPath=path==="/v3/session"||path==="/v3/scores"||path==="/v3/evidence"||path==="/v3/leaderboard"||path==="/v3/me/rank"||path==="/v3/admin/scores/restore"||/^\/v3\/session\/[^/]+\/start$/.test(path)||/^\/v3\/admin\/scores\/\d+(?:\/hide)?$/.test(path);
  if(!knownPath)return err("not_found","No route for that path",404,requestId,cors);
  const cfg = v3Config(env);
  if (cfg) return err("config_error", "Service misconfigured", 503, requestId, cors);
  if (path === "/v3/session" && method === "POST") return createSession(request, env, cors, requestId);
  const start = path.match(/^\/v3\/session\/([^/]+)\/start$/);
  if (start && method === "POST") return startSession(request, decodeURIComponent(start[1]!), env, cors, requestId);
  if (path === "/v3/scores" && method === "POST") return submitScore(request, env, cors, requestId);
  if (path === "/v3/evidence" && method === "POST") return uploadEvidence(request, env, cors, requestId);
  if (path === "/v3/leaderboard" && method === "GET") return leaderboard(request, env, ctx, cors, requestId);
  if (path === "/v3/me/rank" && method === "GET") return myRank(request, env, cors, requestId);
  if (path === "/v3/admin/scores/restore" && method === "POST") return restore(request, env, cors, requestId);
  const moderation = path.match(/^\/v3\/admin\/scores\/(\d+)(?:\/(hide))?$/);
  if (moderation && ((method === "POST" && moderation[2] === "hide") || (method === "DELETE" && !moderation[2]))) return moderate(request, Number(moderation[1]), method === "POST" ? "hidden" : "deleted", env, cors, requestId);
  return err("not_found", "No route for that path", 404, requestId, cors);
}

async function v3StorageReady(env:V3Env):Promise<boolean>{
  try{
    const rows=await env.DB.prepare(`SELECT category_key,protocol_version,protocol_id,rules_version,rules_id,policy_version,policy_id,mode,conduct,display_name,active FROM score_categories_v3 ORDER BY category_key`).all<Record<string,unknown>>();
    const expected=[
      {category_key:CLUCK_HUNT_CATEGORY,protocol_version:3,protocol_id:PROTOCOL_ID,rules_version:3,rules_id:RULES_ID,policy_version:1,policy_id:POLICY_ID,mode:MODE,conduct:"cluck_hunt",display_name:"Cluck Hunt",active:1},
      {category_key:RIGHT_OF_WAY_CATEGORY,protocol_version:3,protocol_id:PROTOCOL_ID,rules_version:3,rules_id:RULES_ID,policy_version:1,policy_id:POLICY_ID,mode:MODE,conduct:"right_of_way",display_name:"Right of Way",active:1},
    ];
    return JSON.stringify(rows.results)===JSON.stringify(expected);
  }catch{return false;}
}
async function capabilities(request: Request, env: V3Env, ctx: ExecutionContext, cors: Cors, id: string): Promise<Response> {
  const url = new URL(request.url);
  if (url.search !== "" || (request.headers.get("Content-Length") !== null && request.headers.get("Content-Length") !== "0") || request.headers.has("Transfer-Encoding")) {
    return err("invalid_body", "Capabilities takes no query or body", 422, id, cors);
  }
  if (!(await limit(env.RATE_LIMIT_READ, `v3:cap:${clientIp(request)}`, "read", false))) return err("rate_limited", "Too many requests", 429, id, cors);
  // A missing migration/config/artifact must force false rather than make the
  // capability route unavailable. The checked-in parity latch already keeps
  // this false; retain the independent computation for future review.
  const advertised = enabled(env) && v3Config(env) === null && env.DB !== undefined && await v3StorageReady(env);
  const cacheUrl = `https://roady-leaderboard.cache/v3|capabilities|${encodeURIComponent(env.BUILD || "unknown")}|${advertised}`;
  // Release verification sends standard no-cache directives so two probes can
  // independently execute the storage/config latch without changing the exact
  // route or response contract. Normal public requests retain edge caching.
  const bypassCache = /(?:^|,)\s*(?:no-cache|no-store|max-age=0)\s*(?:,|$)/i.test(request.headers.get("Cache-Control") ?? "");
  if (!bypassCache) {
    const cached = await caches.default.match(cacheUrl); if (cached) return applyCors(cached, cors);
  }
  const body = { ranked: { enabled: advertised, categories: [...CATEGORIES] }, protocolVersion: 3, protocolId: PROTOCOL_ID, rulesVersion: 3, rulesId: RULES_ID, policyVersion: 1, policyId: POLICY_ID, mode: MODE };
  const response = json(body, 200, null, { "Cache-Control": CACHE_CAP });
  if (!bypassCache) ctx.waitUntil(caches.default.put(cacheUrl, response.clone()));
  return applyCors(response, cors);
}

async function createSession(request: Request, env: V3Env, cors: Cors, id: string): Promise<Response> {
  if (!enabled(env)) return err("ranked_disabled", "Ranked v3 session issuance is disabled", 503, id, cors);
  const ip = clientIp(request); if (!(await limit(env.RATE_LIMIT_SESSION, `v3:session:${ip}`, "session", true))) return err("rate_limited", "Too many requests", 429, id, cors);
  const body = await bounded(request, 16_384);
  if (!exactObject(body, ["mode","categoryKey","turnstileToken"])) return err("invalid_body", "Malformed, oversized, or unknown fields", 422, id, cors);
  if (body.mode !== MODE || !category(body.categoryKey)) return err("unknown_version_tuple", "Unknown v3 tuple", 422, id, cors);
  if (!utf8(body.turnstileToken, 1, 4096)) return err("invalid_body", "Invalid Turnstile token", 422, id, cors);
  if (!(await verifyTurnstile(body.turnstileToken, ip, env))) return err("turnstile_failed", "Turnstile verification failed", 422, id, cors);
  const sessionId = randomBase64Url(18), challenge = randomBase64Url(18), seed = crypto.getRandomValues(new Uint8Array(32));
  const commitment = await seedCommitment(seed), schedule = await scheduleHash(seed, body.categoryKey), issuedAt = Date.now(), expiry = issuedAt + 300_000;
  const header: SessionHeader = { category: body.categoryKey, sessionId, challenge, seedCommitment: commitment, scheduleHash: schedule, issuedAtMs: BigInt(issuedAt) };
  const signed = await proof(env.LB_V3_PROOF_HMAC_KEY!, unstartedSessionHeader(header, BigInt(expiry)));
  const encrypted = await encryptSeed(seed, env, `${sessionId}|${body.categoryKey}|${bytesToHex(commitment)}`);
  await env.DB.prepare(`INSERT INTO sessions_v3 (session_id,category_key,protocol_version,protocol_id,rules_version,rules_id,policy_version,policy_id,mode,conduct,challenge,proof,seed_iv,seed_ciphertext,seed_key_id,seed_commitment,schedule_hash,issued_at,start_by_expiry,started_at,started,used,turnstile_verified,ip_hash) VALUES (?,?,3,?,3,?,1,?,\'rotation\',?,?,?,?,?,?,?,?,?, ?,NULL,0,0,1,?)`)
    .bind(sessionId, body.categoryKey, PROTOCOL_ID, RULES_ID, POLICY_ID, conductFor(body.categoryKey), challenge, signed, encrypted.iv, encrypted.ciphertext, env.LB_V3_SEED_KEY_ID, bytesToHex(commitment), bytesToHex(schedule), issuedAt, expiry, await ipHash(ip, env.LB_IP_HASH_PEPPER)).run();
  return json({ sessionId, challenge, mode: MODE, categoryKey: body.categoryKey, seedHex: bytesToHex(seed), seedCommitment: bytesToHex(commitment), scheduleHash: bytesToHex(schedule), issuedAt, startByExpiry: expiry, proof: signed, protocolVersion: 3, protocolId: PROTOCOL_ID, rulesVersion: 3, rulesId: RULES_ID, policyVersion: 1, policyId: POLICY_ID }, 200, cors, NO_STORE);
}

interface SessionRow { session_id:string; category_key:typeof CATEGORIES[number]; challenge:string; proof:string; seed_iv?:ArrayBuffer; seed_ciphertext?:ArrayBuffer; seed_key_id?:string; seed_commitment:string; schedule_hash:string; issued_at:number; start_by_expiry:number|null; started_at:number|null; started:number; used:number; turnstile_verified:number; }
async function startSession(request: Request, pathId: string, env: V3Env, cors: Cors, id: string): Promise<Response> {
  const ip = clientIp(request); if (!(await limit(env.RATE_LIMIT_SUBMIT, `v3:start:${ip}`, "submit", true))) return err("rate_limited", "Too many requests", 429, id, cors);
  const body = await bounded(request, 16_384); if (!exactObject(body, ["sessionId","proof"]) || !utf8(body.sessionId,1,255) || !exactHmac(body.proof)) return err("invalid_body", "Malformed, oversized, or unknown fields", 422, id, cors);
  if (body.sessionId !== pathId) return err("session_mismatch", "URL and body session differ", 409, id, cors);
  const row = await env.DB.prepare(`SELECT session_id,category_key,challenge,proof,seed_iv,seed_ciphertext,seed_key_id,seed_commitment,schedule_hash,issued_at,start_by_expiry,started_at,started,used,turnstile_verified FROM sessions_v3 WHERE session_id=?`).bind(pathId).first<SessionRow>();
  if (!row) return err("invalid_session", "Unknown session", 404, id, cors);
  if (row.started || row.used) return err("replay", "Session already started", 409, id, cors);
  if ((row.start_by_expiry ?? 0) <= Date.now()) return err("expired_session", "Session start window expired", 409, id, cors);
  const expected = await proof(env.LB_V3_PROOF_HMAC_KEY!, unstartedSessionHeader(sessionHeader(row), BigInt(row.start_by_expiry!)));
  if (!constantTimeEquals(te.encode(body.proof), te.encode(expected))) return err("invalid_proof", "Session proof mismatch", 401, id, cors);
  const startedAt = Date.now(); const startedProof = await proof(env.LB_V3_PROOF_HMAC_KEY!, startedSessionHeader(sessionHeader(row), BigInt(startedAt)));
  const claim = await env.DB.prepare(`UPDATE sessions_v3 SET started=1,started_at=?,start_by_expiry=NULL,proof=? WHERE session_id=? AND started=0 AND used=0 AND start_by_expiry>?`).bind(startedAt, startedProof, pathId, startedAt).run();
  if (claim.meta?.changes !== 1) return err("replay", "Session already started or expired", 409, id, cors);
  return json({ started: true, startedAt, proof: startedProof }, 200, cors, NO_STORE);
}

const COMMON_SCORE = ["sessionId","proof","name","categoryKey","terminalTotal","objectiveCompleted","roundDurationMs","timeLeftMs","gameOverReason","build","platform","finalRoot","scheduleHash","eventCount","signatureKeyId","protocolVersion","protocolId","rulesVersion","rulesId","policyVersion","policyId","mode"] as const;
const CLUCK_SCORE = [...COMMON_SCORE,"chickens","coins","maxCombo"] as const;
const ROW_SCORE = [...COMMON_SCORE,"signedAccumulator","premiumBps","packagesDelivered","courtesyCount","animalHits","maxDeliveryChain"] as const;
interface ParsedScore { body:Record<string,unknown>; terminal:ConductTerminal; accumulator:number|null; }
function parseScore(body: unknown): { ok:true; value:ParsedScore } | { ok:false; code:string; message:string } {
  if (!isPlainObject(body)) return {ok:false,code:"invalid_body",message:"Malformed body"};
  // Tuple/category is intentionally checked before selecting a conduct parser.
  if (!tuple(body)) return {ok:false,code:"unknown_version_tuple",message:"Unknown v3 tuple"};
  const isCluck = body.categoryKey === CLUCK_HUNT_CATEGORY;
  if (!exactObject(body, isCluck ? CLUCK_SCORE : ROW_SCORE)) return {ok:false,code:"invalid_body",message:"Unknown or missing score fields"};
  if (!utf8(body.sessionId,1,255) || !exactHmac(body.proof) || typeof body.name !== "string" || !/^[A-Z0-9]{3,5}$/.test(body.name) ||
      !safeUInt(body.terminalTotal,0xffff_ffff) || typeof body.objectiveCompleted !== "boolean" || !safeUInt(body.roundDurationMs) ||
      !safeUInt(body.timeLeftMs,99_000) || !["time_up","wrecked","drowned"].includes(body.gameOverReason as string) ||
      !utf8(body.build,1,64) || !["web","native"].includes(body.platform as string) || !exactHex(body.finalRoot) || !exactHex(body.scheduleHash) ||
      !safeUInt(body.eventCount,4096) || body.eventCount < 1 || !utf8(body.signatureKeyId,1,64)) return {ok:false,code:"invalid_body",message:"Invalid score field"};
  if (isCluck) {
    if (!safeUInt(body.chickens,0xffff_ffff) || !safeUInt(body.coins,0xffff_ffff) || !safeUInt(body.maxCombo,5) || body.maxCombo < 1) return {ok:false,code:"invalid_body",message:"Invalid Cluck Hunt aggregate"};
    if (body.chickens + body.coins !== body.terminalTotal || body.chickens + body.coins > 0xffff_ffff) return {ok:false,code:"total_mismatch",message:"Terminal aggregate mismatch"};
    return {ok:true,value:{body,accumulator:null,terminal:{conduct:"cluck_hunt",reason:reasonOrdinal(body.gameOverReason as string),total:BigInt(body.terminalTotal),chickens:BigInt(body.chickens),coins:BigInt(body.coins),objectiveCompleted:body.objectiveCompleted,maxCombo:body.maxCombo,durationMs:BigInt(body.roundDurationMs),remainingMs:BigInt(body.timeLeftMs),build:body.build as string,platform:platformOrdinal(body.platform as string)}}};
  }
  if (typeof body.signedAccumulator !== "string" || !/^(?:0|-[1-9][0-9]{0,18}|[1-9][0-9]{0,18})$/.test(body.signedAccumulator)) return {ok:false,code:"invalid_signed_accumulator",message:"Invalid canonical i64"};
  let accumulator: bigint; try { accumulator=BigInt(body.signedAccumulator); } catch { return {ok:false,code:"invalid_signed_accumulator",message:"Invalid signed i64"}; }
  if (accumulator < -(1n<<63n) || accumulator > (1n<<63n)-1n || accumulator < BigInt(Number.MIN_SAFE_INTEGER) || accumulator > BigInt(Number.MAX_SAFE_INTEGER)) return {ok:false,code:"invalid_signed_accumulator",message:"Accumulator cannot be represented by D1 exactly"};
  for (const key of ["premiumBps","packagesDelivered","courtesyCount","animalHits","maxDeliveryChain"] as const) if (!safeUInt(body[key], key === "premiumBps" ? 10_000 : 0xffff_ffff)) return {ok:false,code:"invalid_body",message:"Invalid Right of Way aggregate"};
  const total = accumulator < 0n ? 0n : accumulator; if (total > 0xffff_ffffn || BigInt(body.terminalTotal as number)!==total) return {ok:false,code:"total_mismatch",message:"Terminal aggregate mismatch"};
  return {ok:true,value:{body,accumulator:Number(accumulator),terminal:{conduct:"right_of_way",reason:reasonOrdinal(body.gameOverReason as string),total:BigInt(body.terminalTotal as number),accumulator,premiumBps:BigInt(body.premiumBps as number),packagesDelivered:BigInt(body.packagesDelivered as number),courtesyCount:BigInt(body.courtesyCount as number),animalHits:BigInt(body.animalHits as number),maxDeliveryChain:BigInt(body.maxDeliveryChain as number),objectiveCompleted:body.objectiveCompleted as boolean,durationMs:BigInt(body.roundDurationMs as number),remainingMs:BigInt(body.timeLeftMs as number),build:body.build as string,platform:platformOrdinal(body.platform as string)}}};
}
function clientKeys(env: V3Env): Record<string,string> { return JSON.parse(env.LB_V3_CLIENT_HMAC_KEYS_JSON!); }
async function capability(env: V3Env, scoreId:number, sessionId:string, root:string, expiry:number): Promise<string> { const nonce=randomBase64Url(16); return hmacSha256Base64Url(env.LB_V3_EVIDENCE_CAPABILITY_KEY!, te.encode(`roady.v3.evidence-capability\n${scoreId}\n${sessionId}\n${root}\n${expiry}\n${nonce}`)); }

async function submitScore(request: Request, env: V3Env, cors:Cors, id:string): Promise<Response> {
  const ip=clientIp(request); if (!(await limit(env.RATE_LIMIT_SUBMIT,`v3:score:${ip}`,"submit",true))) return err("rate_limited","Too many requests",429,id,cors);
  const raw=await bounded(request,16_384); const parsed=parseScore(raw); if(!parsed.ok) return err(parsed.code,parsed.message,422,id,cors); const b=parsed.value.body;
  const signature=request.headers.get("X-Roady-Client-Signature"); if(!signature) return err("missing_signature","Client signature required",401,id,cors);
  if(!exactHmac(signature)) return err("invalid_signature","Malformed client signature",401,id,cors);
  const key=clientKeys(env)[b.signatureKeyId as string]; if(!key) return err("unknown_signature_key","Signature key is not accepted",401,id,cors);
  const row=await env.DB.prepare(`SELECT session_id,category_key,challenge,proof,seed_iv,seed_ciphertext,seed_key_id,seed_commitment,schedule_hash,issued_at,start_by_expiry,started_at,started,used,turnstile_verified FROM sessions_v3 WHERE session_id=?`).bind(b.sessionId).first<SessionRow>();
  if(!row) return err("invalid_session","Unknown session",404,id,cors);
  if(row.category_key!==b.categoryKey) return err("condition_mismatch","Session category mismatch",409,id,cors);
  if(row.started!==1 || row.used!==0 || row.started_at===null || row.turnstile_verified!==1) return err("replay","Session is not started and unused",409,id,cors);
  if(row.schedule_hash!==b.scheduleHash) return err("condition_mismatch","Schedule commitment mismatch",409,id,cors);
  const expectedProof=await proof(env.LB_V3_PROOF_HMAC_KEY!,startedSessionHeader(sessionHeader(row),BigInt(row.started_at)));
  if(!constantTimeEquals(te.encode(b.proof as string),te.encode(expectedProof))) return err("invalid_proof","Started proof mismatch",401,id,cors);
  if(!(await verifySeedMaterial(row,env))) return err("invalid_session","Stored seed material does not match commitments",403,id,cors);
  const expectedSig=await hmacSha256Base64Url(key,scoreHmacInput({category:b.categoryKey as string,sessionId:b.sessionId as string,finalRoot:hexToBytes(b.finalRoot as string),scheduleHash:hexToBytes(row.schedule_hash),seedCommitment:hexToBytes(row.seed_commitment),terminal:parsed.value.terminal}));
  if(!constantTimeEquals(te.encode(signature),te.encode(expectedSig))) return err("invalid_signature","Client signature mismatch",401,id,cors);
  const submittedAt=Date.now(), expires=submittedAt+86_400_000;
  // A random placeholder satisfies the pending-row CHECK until the returned
  // score ID can be bound into the real capability. It is never returned.
  const provisionalCapabilityHash=await digestHex(crypto.getRandomValues(new Uint8Array(32)));
  let scoreId:number;
  try {
    const insertStatement=env.DB.prepare(`INSERT INTO scores_v3 (name,category_key,protocol_version,protocol_id,rules_version,rules_id,policy_version,policy_id,mode,conduct,terminal_total,chickens,coins,signed_accumulator,premium_bps,packages_delivered,courtesy_count,animal_hits,objective_completed,max_combo,max_delivery_chain,round_duration_ms,time_left_ms,game_over_reason,build,platform,session_id,submitted_at,ip_hash,status,moderation_note,submission_source,restoration_key,final_root,schedule_hash,seed_commitment,event_count,signature_key_id,evidence_capability_hash,evidence_expires_at) VALUES (?,?,3,?,3,?,1,?,\'rotation\',?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,'pending',NULL,'verified',NULL,?,?,?,?,?,?,?)`)
      .bind(b.name,b.categoryKey,PROTOCOL_ID,RULES_ID,POLICY_ID,conductFor(b.categoryKey as string),b.terminalTotal,b.chickens??null,b.coins??null,parsed.value.accumulator,b.premiumBps??null,b.packagesDelivered??null,b.courtesyCount??null,b.animalHits??null,b.objectiveCompleted?1:0,b.maxCombo??null,b.maxDeliveryChain??null,b.roundDurationMs,b.timeLeftMs,b.gameOverReason,b.build,b.platform,b.sessionId,submittedAt,await ipHash(ip,env.LB_IP_HASH_PEPPER),b.finalRoot,b.scheduleHash,row.seed_commitment,b.eventCount,b.signatureKeyId,provisionalCapabilityHash,expires);
    const claim=await env.DB.prepare(`UPDATE sessions_v3 SET used=1 WHERE session_id=? AND started=1 AND used=0`).bind(b.sessionId).run();
    if(claim.meta?.changes!==1) return err("replay","Session already used",409,id,cors);
    const insert=await insertStatement.run();
    scoreId=Number(insert.meta?.last_row_id); if(!Number.isSafeInteger(scoreId)||scoreId<1) throw new Error("missing id");
    const cap=await capability(env,scoreId,b.sessionId as string,b.finalRoot as string,expires), capHash=await digestHex(te.encode(cap));
    const upd=await env.DB.prepare(`UPDATE scores_v3 SET evidence_capability_hash=? WHERE id=? AND status='pending' AND evidence_capability_hash=?`).bind(capHash,scoreId,provisionalCapabilityHash).run(); if(upd.meta?.changes!==1) throw new Error("capability update failed");
    return json({inserted:true,rank:null,globalRank:null,categoryKey:b.categoryKey,total:b.terminalTotal,submittedAt,status:"pending",evidenceCapability:cap,evidenceExpiresAt:expires},201,cors,NO_STORE);
  } catch(e) { console.error("v3_score_insert",{requestId:id,message:String(e)}); return err("insert_failed","Score insert failed; session remains consumed",500,id,cors); }
}

interface EvidenceScore { id:number; session_id:string; category_key:string; final_root:string; schedule_hash:string; seed_commitment:string; event_count:number; evidence_capability_hash:string; evidence_expires_at:number; status:string; terminal_total:number; chickens:number|null; coins:number|null; signed_accumulator:number|null; premium_bps:number|null; packages_delivered:number|null; courtesy_count:number|null; animal_hits:number|null; objective_completed:number; max_combo:number|null; max_delivery_chain:number|null; round_duration_ms:number; time_left_ms:number; game_over_reason:string; build:string; platform:string; submitted_at:number; }
interface ExistingEvidence { final_root:string; evidence_hash:string; ledger_bytes:ArrayBuffer; replay_result:string; quarantine_reason:string|null; }
function terminalForRow(r:EvidenceScore):ConductTerminal { const base={reason:reasonOrdinal(r.game_over_reason),total:BigInt(r.terminal_total),objectiveCompleted:r.objective_completed===1,durationMs:BigInt(r.round_duration_ms),remainingMs:BigInt(r.time_left_ms),build:r.build,platform:platformOrdinal(r.platform)}; return r.category_key===CLUCK_HUNT_CATEGORY?{...base,conduct:"cluck_hunt",chickens:BigInt(r.chickens!),coins:BigInt(r.coins!),maxCombo:r.max_combo!}:{...base,conduct:"right_of_way",accumulator:BigInt(r.signed_accumulator!),premiumBps:BigInt(r.premium_bps!),packagesDelivered:BigInt(r.packages_delivered!),courtesyCount:BigInt(r.courtesy_count!),animalHits:BigInt(r.animal_hits!),maxDeliveryChain:BigInt(r.max_delivery_chain!)}; }
async function rankFor(env:V3Env,row:{id:number;category_key:string;terminal_total:number;submitted_at:number}):Promise<number>{const a=await env.DB.prepare(`SELECT COUNT(*) ahead FROM scores_v3 WHERE category_key=? AND status='live' AND (terminal_total>? OR (terminal_total=? AND submitted_at<?) OR (terminal_total=? AND submitted_at=? AND id<?))`).bind(row.category_key,row.terminal_total,row.terminal_total,row.submitted_at,row.terminal_total,row.submitted_at,row.id).first<{ahead:number}>();return Number(a?.ahead??0)+1;}

async function uploadEvidence(request:Request,env:V3Env,cors:Cors,id:string):Promise<Response>{
 const ip=clientIp(request);if(!(await limit(env.RATE_LIMIT_SUBMIT,`v3:evidence:${ip}`,"submit",true)))return err("rate_limited","Too many requests",429,id,cors);
 const body=await bounded(request,524_288);if(!exactObject(body,["evidenceCapability","finalRoot","ledgerBytes","evidenceHash"])||!exactB64(body.evidenceCapability)||!exactHex(body.finalRoot)||!exactB64(body.ledgerBytes)||!exactHex(body.evidenceHash))return err("invalid_body","Malformed, oversized, or unknown fields",422,id,cors);
 const capHash=await digestHex(te.encode(body.evidenceCapability));const row=await env.DB.prepare(`SELECT id,session_id,category_key,final_root,schedule_hash,seed_commitment,event_count,evidence_capability_hash,evidence_expires_at,status,terminal_total,chickens,coins,signed_accumulator,premium_bps,packages_delivered,courtesy_count,animal_hits,objective_completed,max_combo,max_delivery_chain,round_duration_ms,time_left_ms,game_over_reason,build,platform,submitted_at FROM scores_v3 WHERE evidence_capability_hash=?`).bind(capHash).first<EvidenceScore>();
 if(!row)return err("invalid_capability","Unknown evidence capability",404,id,cors);
 let ledger:Uint8Array;try{ledger=fromBase64Url(body.ledgerBytes);}catch{return err("invalid_body","Malformed ledger bytes",422,id,cors);}if(ledger.length>MAX_LEDGER_BYTES)return err("invalid_body","Ledger too large",422,id,cors);
 const envelope=evidenceBytes(row.session_id,BigInt(row.event_count),ledger),hash=await digestHex(envelope);
 const existing=await env.DB.prepare(`SELECT final_root,evidence_hash,ledger_bytes,replay_result,quarantine_reason FROM score_evidence_v3 WHERE score_id=?`).bind(row.id).first<ExistingEvidence>();
 if(existing){const same=existing.final_root===body.finalRoot&&existing.evidence_hash===body.evidenceHash&&constantTimeEquals(new Uint8Array(existing.ledger_bytes),ledger);if(!same)return err("evidence_conflict","Evidence capability was already consumed by different bytes",409,id,cors);const live=existing.replay_result==="match";return json({accepted:live,idempotent:true,status:live?"live":"quarantined",rank:live?await rankFor(env,row):null},200,cors,NO_STORE);}
 if(Date.now()>row.evidence_expires_at)return err("expired_capability","Evidence capability expired",409,id,cors);
 let result:{match:boolean;reason:string};if(body.finalRoot!==row.final_root)result={match:false,reason:"submitted_root"};else if(body.evidenceHash!==hash)result={match:false,reason:"evidence_hash"};else {const session=await env.DB.prepare(`SELECT session_id,category_key,challenge,proof,seed_iv,seed_ciphertext,seed_key_id,seed_commitment,schedule_hash,issued_at,start_by_expiry,started_at,started,used,turnstile_verified FROM sessions_v3 WHERE session_id=?`).bind(row.session_id).first<SessionRow>();if(!session||session.started!==1)result={match:false,reason:"session_binding"};else {const seed=await decryptAndVerifySeed(session,env);result=!seed?{match:false,reason:"seed_binding"}:await replayEvidence(row,session,sessionHeader(session),ledger,seed,()=>terminalForRow(row));}}
 try{await env.DB.batch([env.DB.prepare(`INSERT INTO score_evidence_v3(score_id,session_id,final_root,evidence_hash,ledger_bytes,replay_result,quarantine_reason,uploaded_at) VALUES(?,?,?,?,?,?,?,?)`).bind(row.id,row.session_id,body.finalRoot,body.evidenceHash,ledger,result.match?"match":"mismatch",result.match?null:result.reason,Date.now()),env.DB.prepare(`UPDATE scores_v3 SET status=?,moderation_note=? WHERE id=? AND status='pending'`).bind(result.match?"live":"quarantined",result.match?null:`evidence:${result.reason}`,row.id)]);}catch{return err("evidence_conflict","Evidence was concurrently consumed",409,id,cors);}
 if(!result.match)return err("evidence_conflict","Evidence did not match the immutable score",409,id,cors);return json({accepted:true,idempotent:false,status:"live",rank:await rankFor(env,row)},201,cors,NO_STORE);
}

async function leaderboard(request:Request,env:V3Env,ctx:ExecutionContext,cors:Cors,id:string):Promise<Response>{
 if(!(await limit(env.RATE_LIMIT_READ,`v3:board:${clientIp(request)}`,"read",false)))return err("rate_limited","Too many requests",429,id,cors);const u=new URL(request.url);if([...u.searchParams.keys()].some(k=>!["categoryKey","limit","offset"].includes(k)))return err("invalid_query","Unknown query field",422,id,cors);const cat=u.searchParams.get("categoryKey");if(!category(cat))return err("unknown_version_tuple","Unknown v3 category",422,id,cors);const limitN=u.searchParams.get("limit")??"25",offsetN=u.searchParams.get("offset")??"0";if(!/^\d+$/.test(limitN)||Number(limitN)<1||Number(limitN)>100||!/^\d+$/.test(offsetN))return err("invalid_query","Invalid pagination",422,id,cors);const key=`https://roady-leaderboard.cache/v3|board|${encodeURIComponent(env.BUILD)}|${cat}|${limitN}|${offsetN}`;const hit=await caches.default.match(key);if(hit)return applyCors(hit,cors);const rows=await env.DB.prepare(`SELECT id,name,terminal_total,submitted_at FROM scores_v3 WHERE category_key=? AND status='live' ORDER BY terminal_total DESC,submitted_at ASC,id ASC LIMIT ? OFFSET ?`).bind(cat,Number(limitN),Number(offsetN)).all<{name:string;terminal_total:number;submitted_at:number}>();const body={categoryKey:cat,entries:rows.results.map((r,i)=>({rank:Number(offsetN)+i+1,name:r.name,score:r.terminal_total,submittedAt:r.submitted_at})),generatedAt:Date.now()};const response=json(body,200,null,{"Cache-Control":CACHE_BOARD});ctx.waitUntil(caches.default.put(key,response.clone()));return applyCors(response,cors);
}
async function myRank(request:Request,env:V3Env,cors:Cors,id:string):Promise<Response>{
 if(!(await limit(env.RATE_LIMIT_RANK,`v3:rank:${clientIp(request)}`,"rank",true)))return err("rate_limited","Too many requests",429,id,cors);const u=new URL(request.url);if([...u.searchParams.keys()].some(k=>k!=="sessionId")||!utf8(u.searchParams.get("sessionId"),1,255))return err("invalid_session","sessionId required",422,id,cors);const row=await env.DB.prepare(`SELECT sc.id,sc.session_id,sc.category_key,sc.name,sc.terminal_total,sc.submitted_at,sc.status,sc.submission_source,s.used FROM sessions_v3 s LEFT JOIN scores_v3 sc ON sc.session_id=s.session_id WHERE s.session_id=?`).bind(u.searchParams.get("sessionId")).first<{id:number|null;session_id:string;category_key:string;name:string;terminal_total:number;submitted_at:number;status:string;submission_source:string;used:number}>();if(!row)return err("invalid_session","Unknown session",404,id,cors);if(row.used!==1||row.id===null)return err("invalid_session","Session has no score",403,id,cors);if(row.status!=="live")return json({sessionId:row.session_id,status:row.status,rank:null,categoryKey:row.category_key,entry:{name:row.name,total:row.terminal_total,submittedAt:row.submitted_at,submissionSource:row.submission_source},nearby:[]},200,cors,{"Cache-Control":"private, no-store"});const rank=await rankFor(env,row as never);const nearby=await env.DB.prepare(`SELECT name,terminal_total,submitted_at FROM scores_v3 WHERE category_key=? AND status='live' ORDER BY terminal_total DESC,submitted_at ASC,id ASC LIMIT 21 OFFSET ?`).bind(row.category_key,Math.max(0,rank-11)).all<{name:string;terminal_total:number;submitted_at:number}>();return json({sessionId:row.session_id,status:"live",rank,categoryKey:row.category_key,entry:{name:row.name,total:row.terminal_total,submittedAt:row.submitted_at,submissionSource:row.submission_source},nearby:nearby.results.map((x,i)=>({rank:Math.max(0,rank-11)+i+1,name:x.name,score:x.terminal_total,submittedAt:x.submitted_at}))},200,cors,{"Cache-Control":"private, no-store"});
}

function admin(request:Request,env:V3Env):boolean{const got=request.headers.get("Authorization")??"",expected=`Bearer ${env.LB_ADMIN_TOKEN}`;return !placeholder(env.LB_ADMIN_TOKEN)&&got.length===expected.length&&constantTimeEquals(te.encode(got),te.encode(expected));}
async function moderate(request:Request,scoreId:number,status:"hidden"|"deleted",env:V3Env,cors:Cors,id:string):Promise<Response>{if(!admin(request,env))return err("unauthorized","Invalid admin token",401,id,cors);const action=status==="hidden"?"hide":"delete",at=Date.now();try{const result=await env.DB.batch([env.DB.prepare(`UPDATE scores_v3 SET status=?,moderation_note=? WHERE id=? AND status='live'`).bind(status,`${status} by admin`,scoreId),env.DB.prepare(`INSERT INTO moderation_log_v3(action,target_score_id,admin,at,note) SELECT ?,id,'admin',?,? FROM scores_v3 WHERE id=? AND status=?`).bind(action,at,`${status} by admin`,scoreId,status)]);if((result[0] as D1Result).meta?.changes!==1)return err("not_found","No live v3 score with that id",404,id,cors);return json({ok:true,id:scoreId,status},200,cors,NO_STORE);}catch{return err("moderation_failed","Moderation failed safely",500,id,cors);}}

async function restore(request:Request,env:V3Env,cors:Cors,id:string):Promise<Response>{
 if(!admin(request,env))return err("unauthorized","Invalid admin token",401,id,cors);const body=await bounded(request,4096);if(!exactObject(body,["restorationKey","evidenceHash","known","synthetic","reason"])||!utf8(body.restorationKey,1,128)||!/^[A-Za-z0-9._:-]+$/.test(body.restorationKey)||!exactHex(body.evidenceHash)||!utf8(body.reason,1,256)||!isPlainObject(body.known)||!isPlainObject(body.synthetic))return err("invalid_body","Invalid restoration body",422,id,cors);
 const known=body.known,synthetic=body.synthetic;if(!category(known.categoryKey))return err("unknown_version_tuple","Unknown v3 category",422,id,cors);const cluck=known.categoryKey===CLUCK_HUNT_CATEGORY;const knownFields=cluck?["name","categoryKey","terminalTotal","chickens","coins","objectiveCompleted","gameOverReason"]:["name","categoryKey","terminalTotal","signedAccumulator","premiumBps","packagesDelivered","courtesyCount","animalHits","maxDeliveryChain","objectiveCompleted","gameOverReason"];const syntheticFields=cluck?["maxCombo","roundDurationMs","timeLeftMs","build","platform","submittedAt"]:["roundDurationMs","timeLeftMs","build","platform","submittedAt"];if(!exactObject(known,knownFields)||!exactObject(synthetic,syntheticFields))return err("invalid_body","Unknown restoration fields",422,id,cors);
 const scoreBody={sessionId:"admin_restore",proof:"admin_restore",...known,...synthetic,finalRoot:body.evidenceHash,scheduleHash:"0".repeat(64),eventCount:1,signatureKeyId:"v3.client.1",protocolVersion:3,protocolId:PROTOCOL_ID,rulesVersion:3,rulesId:RULES_ID,policyVersion:1,policyId:POLICY_ID,mode:MODE};delete (scoreBody as Record<string,unknown>).submittedAt;const parsed=parseScore(scoreBody);if(!parsed.ok||!safeUInt(synthetic.submittedAt))return err(parsed.ok?"invalid_body":parsed.code,parsed.ok?"Invalid submittedAt":parsed.message,422,id,cors);
 const canonical=JSON.stringify({restorationKey:body.restorationKey,evidenceHash:body.evidenceHash,known,synthetic,reason:body.reason}),payloadHash=await digestHex(te.encode(canonical));const previous=await env.DB.prepare(`SELECT restoration_key,evidence_hash,payload_hash,score_id FROM admin_restorations_v3 WHERE restoration_key=? OR evidence_hash=?`).bind(body.restorationKey,body.evidenceHash).first<{restoration_key:string;evidence_hash:string;payload_hash:string;score_id:number}>();if(previous){if(previous.restoration_key!==body.restorationKey||previous.evidence_hash!==body.evidenceHash||previous.payload_hash!==payloadHash)return err("restoration_conflict","Restoration key was reused",409,id,cors);return json({restored:true,idempotent:true,scoreId:previous.score_id},200,cors,NO_STORE);}
 const sid=`admin_restore:${body.restorationKey}`,zero="0".repeat(64),at=synthetic.submittedAt as number,p=parsed.value.body,encrypted=await encryptSeed(new Uint8Array(32),env,`${sid}|${p.categoryKey}|${zero}`);
 try{await env.DB.batch([env.DB.prepare(`INSERT INTO sessions_v3(session_id,category_key,protocol_version,protocol_id,rules_version,rules_id,policy_version,policy_id,mode,conduct,challenge,proof,seed_iv,seed_ciphertext,seed_key_id,seed_commitment,schedule_hash,issued_at,start_by_expiry,started_at,started,used,turnstile_verified,ip_hash) VALUES(?,?,3,?,3,?,1,?,'rotation',?,'admin_restore','admin_restore',?,?,?,?,?,?,NULL,?,1,1,0,'admin_restore')`).bind(sid,p.categoryKey,PROTOCOL_ID,RULES_ID,POLICY_ID,conductFor(p.categoryKey as string),encrypted.iv,encrypted.ciphertext,env.LB_V3_SEED_KEY_ID,zero,zero,at,at),env.DB.prepare(`INSERT INTO scores_v3(name,category_key,protocol_version,protocol_id,rules_version,rules_id,policy_version,policy_id,mode,conduct,terminal_total,chickens,coins,signed_accumulator,premium_bps,packages_delivered,courtesy_count,animal_hits,objective_completed,max_combo,max_delivery_chain,round_duration_ms,time_left_ms,game_over_reason,build,platform,session_id,submitted_at,ip_hash,status,moderation_note,submission_source,restoration_key,final_root,schedule_hash,seed_commitment,event_count,signature_key_id,evidence_capability_hash,evidence_expires_at) VALUES(?,?,3,?,3,?,1,?,'rotation',?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,'admin_restore','live','review:v3:admin_restore','admin_restore',?,?,?,?,1,'admin_restore',NULL,NULL)`).bind(p.name,p.categoryKey,PROTOCOL_ID,RULES_ID,POLICY_ID,conductFor(p.categoryKey as string),p.terminalTotal,p.chickens??null,p.coins??null,parsed.value.accumulator,p.premiumBps??null,p.packagesDelivered??null,p.courtesyCount??null,p.animalHits??null,p.objectiveCompleted?1:0,p.maxCombo??null,p.maxDeliveryChain??null,p.roundDurationMs,p.timeLeftMs,p.gameOverReason,p.build,p.platform,sid,at,body.restorationKey,body.evidenceHash,zero,zero),env.DB.prepare(`INSERT INTO admin_restorations_v3(restoration_key,evidence_hash,payload_hash,category_key,known_json,synthetic_json,reason,score_id,restored_at,admin) SELECT ?,?,?,?,?,?,?,id,?,'admin' FROM scores_v3 WHERE restoration_key=?`).bind(body.restorationKey,body.evidenceHash,payloadHash,p.categoryKey,JSON.stringify(known),JSON.stringify(synthetic),body.reason,Date.now(),body.restorationKey),env.DB.prepare(`INSERT INTO moderation_log_v3(action,target_score_id,admin,at,note) SELECT 'restore',id,'admin',?,'review:v3:admin_restore' FROM scores_v3 WHERE restoration_key=?`).bind(Date.now(),body.restorationKey)]);const created=await env.DB.prepare(`SELECT score_id FROM admin_restorations_v3 WHERE restoration_key=?`).bind(body.restorationKey).first<{score_id:number}>();if(!created)throw new Error("not created");return json({restored:true,idempotent:false,scoreId:created.score_id},201,cors,NO_STORE);}catch(e){console.error("v3_restore",{requestId:id,message:String(e)});return err("restore_failed","Restoration failed safely",500,id,cors);}
}

export async function cleanupV3(env:V3Env):Promise<void>{const now=Date.now();await env.DB.prepare(`DELETE FROM sessions_v3 WHERE started=0 AND used=0 AND start_by_expiry<?`).bind(now).run();await env.DB.prepare(`UPDATE scores_v3 SET status='unranked_missing_evidence',moderation_note='evidence:missing' WHERE status='pending' AND evidence_expires_at<?`).bind(now).run();for(const cat of CATEGORIES)await env.DB.prepare(`UPDATE scores_v3 SET status='hidden',moderation_note='retention: older than 90d and outside top 1000' WHERE category_key=? AND status='live' AND submitted_at<? AND id NOT IN(SELECT id FROM scores_v3 WHERE category_key=? AND status='live' ORDER BY terminal_total DESC,submitted_at ASC,id ASC LIMIT 1000)`).bind(cat,now-90*86_400_000,cat).run();}
