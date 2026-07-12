// Shared HTTP response + CORS helpers for the Roady Car leaderboard Worker.
// See LEADERBOARD_ARCHITECTURE.md §4 (error shape) and §11 (CORS).

export interface ErrorBody {
  error: {
    code: string;
    message: string;
    requestId: string;
  };
}

const COMMON_HEADERS: Record<string, string> = {
  "Content-Type": "application/json; charset=utf-8",
};

/** Generate a short, opaque request id for error correlation. */
export function newRequestId(): string {
  const bytes = new Uint8Array(8);
  crypto.getRandomValues(bytes);
  return toBase64Url(bytes);
}

/** Encode bytes as unpadded base64url (RFC 4648 §5, no `=`). */
export function toBase64Url(bytes: ArrayBuffer | Uint8Array): string {
  const view = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  let s = "";
  for (let i = 0; i < view.length; i++) s += String.fromCharCode(view[i]!);
  const b64 = btoa(s);
  return b64.replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** Decode an unpadded base64url string into bytes. Throws on invalid input. */
export function fromBase64Url(s: string): Uint8Array {
  if (!/^[A-Za-z0-9_-]*$/.test(s)) {
    throw new Error("invalid base64url");
  }
  const padded = s.replace(/-/g, "+").replace(/_/g, "/").padEnd(
    Math.ceil(s.length / 4) * 4,
    "=",
  );
  const bin = atob(padded);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** Parse a comma-separated ALLOWED_ORIGINS var into a trimmed set. */
export function parseAllowedOrigins(raw: string | undefined): Set<string> {
  if (!raw) return new Set();
  return new Set(
    raw
      .split(",")
      .map((o) => o.trim())
      .filter((o) => o.length > 0),
  );
}

/**
 * Build CORS headers for a request. Only configured origins are allowed; the
 * wildcard is never used for submission endpoints. Returns `null` if the
 * origin is not allowed (caller should not emit any CORS headers).
 */
export function corsHeaders(
  origin: string | null | undefined,
  allowed: Set<string>,
): Record<string, string> | null {
  if (!origin || !allowed.has(origin)) return null;
  return {
    "Access-Control-Allow-Origin": origin,
    "Access-Control-Allow-Methods": "GET, POST, DELETE, OPTIONS",
    // X-Roady-Client-Signature is required on POST /v1/scores (architecture §5).
    "Access-Control-Allow-Headers": "Content-Type, Authorization, X-Roady-Client-Signature",
    "Access-Control-Max-Age": "600",
    Vary: "Origin",
  };
}

function withCors(
  base: Record<string, string>,
  cors: Record<string, string> | null,
): Record<string, string> {
  return cors ? { ...base, ...cors } : base;
}

/** The CORS header keys managed by {@link corsHeaders}. */
const CORS_HEADER_KEYS = [
  "Access-Control-Allow-Origin",
  "Access-Control-Allow-Methods",
  "Access-Control-Allow-Headers",
  "Access-Control-Max-Age",
  "Vary",
];

/**
 * Reapply per-origin CORS headers to a response that was built or cached
 * without them (or with a different origin's headers). This is used when serving
 * the public leaderboard from the edge Cache API: the cached bytes must not be
 * origin-bound, so CORS is stripped before caching and re-added per request.
 */
export function applyCors(
  response: Response,
  cors: Record<string, string> | null,
): Response {
  if (!cors) return response;
  const headers = new Headers(response.headers);
  for (const key of CORS_HEADER_KEYS) headers.delete(key);
  for (const [k, v] of Object.entries(cors)) headers.set(k, v);
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  });
}

/** JSON success response (default 200). */
export function json(
  body: unknown,
  status = 200,
  cors: Record<string, string> | null = null,
  extra: Record<string, string> = {},
): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: withCors({ ...COMMON_HEADERS, ...extra }, cors),
  });
}

/** Error response in the architecture's canonical shape. */
export function errorResponse(
  code: string,
  message: string,
  status: number,
  requestId: string,
  cors: Record<string, string> | null = null,
): Response {
  const body: ErrorBody = {
    error: { code, message, requestId },
  };
  return new Response(JSON.stringify(body), {
    status,
    headers: withCors(COMMON_HEADERS, cors),
  });
}

/** Handle OPTIONS preflight; returns null if not a preflight. */
export function handleOptions(
  request: Request,
  cors: Record<string, string> | null,
): Response | null {
  if (request.method !== "OPTIONS") return null;
  return new Response(null, {
    status: 204,
    headers: cors ?? { Vary: "Origin" },
  });
}
