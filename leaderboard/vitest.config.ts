import { defineConfig } from "vitest/config";

// The leaderboard tests are pure unit tests (canonical bytes, HMAC sign/verify
// round-trips, name normalization, score validation, plausibility caps,
// constant-time comparison, IP hashing) plus replay-sensitive one-time session
// claim logic exercised against an in-memory D1 fake (test/helpers.ts FakeD1).
//
// They run in the default Node environment: Node 22 provides the global Web
// Crypto API (`crypto.subtle`, `crypto.getRandomValues`) and `btoa`/`atob`,
// which the security module relies on. No Cloudflare bindings or Miniflare
// runtime are required for these tests. An optional integration tier using
// `@cloudflare/vitest-pool-workers` with a real Miniflare D1 can be layered on
// later; see README.md.
export default defineConfig({
  test: {
    environment: "node",
    include: ["test/**/*.test.ts"],
    globals: false,
  },
});
