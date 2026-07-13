# Vendored cloudflare-game-common adapter

This directory is the Roady Car standalone-CI adapter for unpublished package
`@segfault-site/cloudflare-game-common`, initially copied from the isolated
sibling worktree `../cloudflare-game-common` on 2026-07-13.

`src/index.ts` preserves the shared package contracts used by the leaderboard:
strict exact-origin parsing/checking, UTF-8 byte bounds, bounded JSON parsing,
Web Crypto SHA-256/base64url/random helpers, and fail-closed rate limiting.
The leaderboard imports this local source directly, so CI does not depend on a
workspace layout, registry publication, package installation, or the sibling
repository. The vendored code is MIT-licensed; see `LICENSE`.

## Sync procedure

Until the package is published, compare this `src/index.ts` against exports in
`cloudflare-game-common/src/` whenever the upstream package changes, copy only
compatible API changes, update the provenance date/revision here, and run the
leaderboard test/typecheck gates. Do not replace this with a relative import
outside the Roady repository: that would break standalone checkout CI.
