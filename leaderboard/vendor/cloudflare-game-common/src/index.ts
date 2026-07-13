// Vendored adapter from @segfault-site/cloudflare-game-common (2026-07-13).
// See ../README.md for provenance and synchronization policy.

export type ExactOrigin = string & { readonly __exactOrigin: unique symbol };

export function parseExactOrigin(value: unknown): ExactOrigin | null {
  if (typeof value !== "string" || value.length === 0 || value !== value.trim()) return null;
  if (value === "null" || value.includes("*")) return null;
  try {
    const url = new URL(value);
    if (url.protocol !== "https:" && url.protocol !== "http:") return null;
    if (url.username || url.password || url.pathname !== "/" || url.search || url.hash) return null;
    if (value.endsWith("/") || url.origin !== value) return null;
    return value as ExactOrigin;
  } catch {
    return null;
  }
}

export function parseExactOrigins(raw: unknown): ReadonlySet<ExactOrigin> | null {
  if (typeof raw !== "string" || raw.length === 0) return null;
  const result = new Set<ExactOrigin>();
  for (const item of raw.split(",")) {
    const origin = parseExactOrigin(item.trim());
    if (origin === null) return null;
    result.add(origin);
  }
  return result.size > 0 ? result : null;
}

export function isExactOriginAllowed(origin: unknown, allowed: ReadonlySet<string>): origin is ExactOrigin {
  const parsed = parseExactOrigin(origin);
  return parsed !== null && allowed.has(parsed);
}

const ENCODER = new TextEncoder();
export function utf8ByteLength(value: string): number {
  return ENCODER.encode(value).byteLength;
}

export function boundedString(
  value: unknown,
  options: { minBytes?: number; maxBytes: number; trim?: boolean; pattern?: RegExp },
): string | null {
  if (typeof value !== "string") return null;
  const { minBytes = 0, maxBytes, trim = false, pattern } = options;
  if (!Number.isSafeInteger(minBytes) || !Number.isSafeInteger(maxBytes) || minBytes < 0 || maxBytes < minBytes) {
    throw new RangeError("invalid string byte bounds");
  }
  if (pattern?.global || pattern?.sticky) throw new TypeError("pattern must not be global or sticky");
  const result = trim ? value.trim() : value;
  const length = utf8ByteLength(result);
  return length >= minBytes && length <= maxBytes && (!pattern || pattern.test(result)) ? result : null;
}

export function isPlainObject(value: unknown): value is Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  try {
    const prototype = Object.getPrototypeOf(value) as unknown;
    return prototype === Object.prototype || prototype === null;
  } catch {
    return false;
  }
}

export async function readBoundedJson(request: Request, maxBytes: number): Promise<unknown> {
  if (!Number.isSafeInteger(maxBytes) || maxBytes < 0) throw new RangeError("invalid JSON byte limit");
  const declared = request.headers.get("Content-Length");
  if (declared !== null && /^\d+$/.test(declared) && Number(declared) > maxBytes) throw new Error("JSON body too large");
  if (request.body === null) throw new Error("empty JSON body");
  const reader = request.body.getReader();
  const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: false });
  let total = 0;
  let text = "";
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      total += value.byteLength;
      if (total > maxBytes) {
        await reader.cancel();
        throw new Error("JSON body too large");
      }
      text += decoder.decode(value, { stream: true });
    }
    text += decoder.decode();
  } finally {
    reader.releaseLock();
  }
  if (total === 0) throw new Error("empty JSON body");
  return JSON.parse(text) as unknown;
}

function asBytes(value: string | ArrayBuffer | Uint8Array): Uint8Array {
  if (typeof value === "string") return ENCODER.encode(value);
  return value instanceof Uint8Array ? value : new Uint8Array(value);
}

export function toBase64Url(value: ArrayBuffer | Uint8Array): string {
  let binary = "";
  for (const byte of asBytes(value)) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function fromBase64Url(value: string): Uint8Array {
  if (!/^[A-Za-z0-9_-]*$/.test(value) || value.length % 4 === 1) throw new TypeError("invalid base64url");
  const padded = value.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(value.length / 4) * 4, "=");
  const binary = atob(padded);
  const output = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) output[i] = binary.charCodeAt(i);
  if (toBase64Url(output) !== value) throw new TypeError("non-canonical base64url");
  return output;
}

export async function sha256(value: string | ArrayBuffer | Uint8Array): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", asBytes(value)));
}
export async function sha256Hex(value: string | ArrayBuffer | Uint8Array): Promise<string> {
  return Array.from(await sha256(value), (byte) => byte.toString(16).padStart(2, "0")).join("");
}
export async function sha256Base64Url(value: string | ArrayBuffer | Uint8Array): Promise<string> {
  return toBase64Url(await sha256(value));
}
export function randomBytes(length: number): Uint8Array {
  if (!Number.isSafeInteger(length) || length < 0 || length > 65_536) throw new RangeError("invalid random byte length");
  return crypto.getRandomValues(new Uint8Array(length));
}
export function randomBase64Url(length: number): string {
  return toBase64Url(randomBytes(length));
}
export function randomHex(length: number): string {
  return Array.from(randomBytes(length), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export interface RateLimitBinding {
  limit(input: { key: string }): Promise<{ success: boolean }>;
}
export async function checkRateLimit(
  binding: RateLimitBinding | null | undefined,
  key: string,
  category: string,
  logger: { error(event: string, details: { category: string }): void } = console,
): Promise<boolean> {
  if (!binding) {
    logger.error("rate_limit_missing", { category });
    return false;
  }
  try {
    return (await binding.limit({ key })).success === true;
  } catch {
    logger.error("rate_limit_error", { category });
    return false;
  }
}
