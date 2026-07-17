#!/usr/bin/env node
/** Verify the exact remote D1 migration registry/schema after additive apply. */
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import assert from "node:assert/strict";

const root = resolve(import.meta.dirname, "..");
const wrangler = join(root, "node_modules", "wrangler", "bin", "wrangler.js");
const invoke = (args) => execFileSync(process.execPath, [wrangler, ...args], { cwd: root, encoding: "utf8" });
const run = (statement) => invoke(["d1", "execute", "roady_leaderboard", "--remote", "--command", statement, "--json"]);
const results = (statement) => JSON.parse(run(statement))[0].results;
const migration = readFileSync(join(root, "migrations", "0006_ranked_v3.sql"));
const migrationSha256 = createHash("sha256").update(migration).digest("hex");
const expectedMigrationSha256 = process.env.EXPECTED_MIGRATION_SHA256;
if (expectedMigrationSha256) assert.equal(migrationSha256, expectedMigrationSha256, "0006 artifact hash changed");

const applied = results("SELECT name FROM d1_migrations ORDER BY id").map(({ name }) => name);
assert.deepEqual(applied.slice(-2), ["0005_ranked_v2.sql", "0006_ranked_v3.sql"], "remote additive migration order mismatch");
const categories = results("SELECT category_key,protocol_version,protocol_id,rules_version,rules_id,policy_version,policy_id,mode,conduct,display_name,active FROM score_categories_v3 ORDER BY category_key");
assert.deepEqual(categories, [
  { category_key: "rotation.v2.cluck_hunt", protocol_version: 3, protocol_id: "roady-protocol.v3", rules_version: 3, rules_id: "roady-rules.v3", policy_version: 1, policy_id: "roady-ranked-policy.v3.1", mode: "rotation", conduct: "cluck_hunt", display_name: "Cluck Hunt", active: 1 },
  { category_key: "rotation.v2.right_of_way", protocol_version: 3, protocol_id: "roady-protocol.v3", rules_version: 3, rules_id: "roady-rules.v3", policy_version: 1, policy_id: "roady-ranked-policy.v3.1", mode: "rotation", conduct: "right_of_way", display_name: "Right of Way", active: 1 },
]);
const tables = results("SELECT name FROM sqlite_schema WHERE type='table' AND name GLOB '*_v3' ORDER BY name").map(({ name }) => name);
assert.deepEqual(tables, ["admin_restorations_v3", "moderation_log_v3", "score_categories_v3", "score_evidence_v3", "scores_v3", "sessions_v3"]);
const foreignTargets = results("SELECT m.name AS table_name,f.[table] AS target FROM sqlite_schema m JOIN pragma_foreign_key_list(m.name) f WHERE m.type='table' AND m.name GLOB '*_v3' ORDER BY m.name,f.id");
assert.ok(foreignTargets.length > 0 && foreignTargets.every(({ target }) => target.endsWith("_v3")), "remote v3 FK isolation mismatch");
const indexes = results("SELECT name,tbl_name FROM sqlite_schema WHERE type='index' AND name NOT LIKE 'sqlite_%' AND (name LIKE '%v3%' OR tbl_name LIKE '%v3%') ORDER BY name");
assert.ok(indexes.length > 0 && indexes.every(({ name, tbl_name }) => name.includes("v3") && tbl_name.endsWith("_v3")), "remote v3 index isolation mismatch");

// Compare exact SQLite-normalized CREATE statements to a fresh local D1 that
// applied the same checked-in migration chain. This catches a production table,
// constraint, or index that merely has the expected name but the wrong schema.
const schemaQuery = "SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' AND ((type='table' AND name GLOB '*_v3') OR (type='index' AND tbl_name GLOB '*_v3')) ORDER BY type,name";
const remoteSchema = results(schemaQuery);
const state = mkdtempSync(join(tmpdir(), "roady-remote-schema-reference-"));
let localSchema;
try {
  invoke(["d1", "migrations", "apply", "roady_leaderboard", "--local", "--persist-to", state]);
  const localRaw = invoke(["d1", "execute", "roady_leaderboard", "--local", "--persist-to", state, "--command", schemaQuery, "--json"]);
  localSchema = JSON.parse(localRaw)[0].results;
} finally {
  rmSync(state, { recursive: true, force: true });
}
assert.deepEqual(remoteSchema, localSchema, "remote v3 normalized schema differs from checked-in migrations");
const schemaSha256 = createHash("sha256").update(JSON.stringify(remoteSchema)).digest("hex");
const counts = results("SELECT (SELECT COUNT(*) FROM sessions) AS sessions_v1,(SELECT COUNT(*) FROM scores) AS scores_v1,(SELECT COUNT(*) FROM sessions_v2) AS sessions_v2,(SELECT COUNT(*) FROM scores_v2) AS scores_v2")[0];
console.log(JSON.stringify({ ok: true, migrationSha256, schemaSha256, appliedTail: applied.slice(-2), v3Tables: tables, legacyCounts: counts }));
