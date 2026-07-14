#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";

const root = resolve(import.meta.dirname, "..");
const temp = mkdtempSync(join(tmpdir(), "roady-d1-restore-"));
const persist = join(temp, "state");
const before = join(temp, "before");
const after = join(temp, "after");
mkdirSync(before); mkdirSync(after);
for (const name of ["0001_init.sql", "0002_indexes.sql", "0003_deterministic_ranking_indexes.sql"])
  cpSync(join(root, "migrations", name), join(before, name));
cpSync(join(root, "migrations", "0004_admin_restorations.sql"), join(after, "0004_admin_restorations.sql"));
const baseConfig = readFileSync(join(root, "wrangler.toml"), "utf8");
const config = (dir, file) => writeFileSync(file, baseConfig.replace('migrations_dir = "migrations"', `migrations_dir = "${dir.replaceAll("\\", "/")}"`));
const beforeConfig = join(temp, "before.toml");
const afterConfig = join(temp, "after.toml");
config(before, beforeConfig); config(after, afterConfig);
const wranglerBin = join(root, "node_modules", "wrangler", "bin", "wrangler.js");
const run = (args) => execFileSync(process.execPath, [wranglerBin, ...args], {
  cwd: root,
  encoding: "utf8",
  stdio: ["ignore", "pipe", "pipe"],
});
const migrations = (cfg) => run(["d1", "migrations", "apply", "roady_leaderboard", "--local", "--persist-to", persist, "--config", cfg]);
const sql = (cfg, command) => run(["d1", "execute", "roady_leaderboard", "--local", "--persist-to", persist, "--config", cfg, "--command", command]);
try {
  migrations(beforeConfig);
  sql(beforeConfig, "INSERT INTO sessions(session_id,challenge,condition,proof,issued_at,expires_at,used,turnstile_verified,ip_hash) VALUES('legacy','c',0,'p',1,2,1,1,'h'); INSERT INTO scores(name,condition,terminal_total,chickens,coins,objective_completed,max_combo,round_duration_ms,time_left_ms,game_over_reason,build,platform,session_id,submitted_at,ip_hash,status,moderation_note) VALUES('OLD',0,1,1,0,0,1,1000,0,'time_up','0.1.0','web','legacy',1,'h','live',NULL);");
  migrations(afterConfig);
  const defaults = sql(afterConfig, "SELECT submission_source, restoration_key FROM scores WHERE session_id='legacy';");
  if (!defaults.includes('"submission_source": "verified"') || !defaults.includes('"restoration_key": null')) throw new Error("migration defaults failed");
  sql(afterConfig, "INSERT INTO sessions(session_id,challenge,condition,proof,issued_at,expires_at,used,turnstile_verified,ip_hash) VALUES('normal','c',0,'p',2,3,1,1,'h'); INSERT INTO scores(name,condition,terminal_total,chickens,coins,objective_completed,max_combo,round_duration_ms,time_left_ms,game_over_reason,build,platform,session_id,submitted_at,ip_hash,status,moderation_note) VALUES('NEW',0,2,1,1,0,1,1000,0,'time_up','0.1.1','web','normal',2,'h','live',NULL);");
  const normalDefaults = sql(afterConfig, "SELECT submission_source,restoration_key FROM scores WHERE session_id='normal';");
  if (!normalDefaults.includes('"submission_source": "verified"') || !normalDefaults.includes('"restoration_key": null')) throw new Error("normal post-migration defaults failed");
  const batchSql = "INSERT INTO sessions(session_id,challenge,condition,proof,issued_at,expires_at,used,turnstile_verified,ip_hash) VALUES('admin_restore:test','admin_restore',2,'admin_restore',10,10,1,0,'admin_restore'); INSERT INTO scores(name,condition,terminal_total,chickens,coins,objective_completed,max_combo,round_duration_ms,time_left_ms,game_over_reason,build,platform,session_id,submitted_at,ip_hash,status,moderation_note,submission_source,restoration_key) VALUES('MODZ',2,1614,1179,435,1,3,1800001,0,'wrecked','0.1.1','web','admin_restore:test',10,'admin_restore','live','review:v1:admin_restore','admin_restore','test'); INSERT INTO admin_restorations(restoration_key,evidence_hash,payload_hash,known_fields_json,synthetic_fields_json,reason,score_id,restored_at,admin) SELECT 'test', lower(hex(randomblob(32))), lower(hex(randomblob(32))), '{\"name\":\"MODZ\"}', '{\"round_duration_ms\":1800001}', 'test', id, 10, 'admin' FROM scores WHERE restoration_key='test';";
  sql(afterConfig, batchSql);
  const visible = sql(afterConfig, "SELECT s.name, ar.restoration_key FROM scores s JOIN admin_restorations ar ON ar.score_id=s.id WHERE s.restoration_key='test';");
  if (!visible.includes('"name": "MODZ"') || !visible.includes('"restoration_key": "test"')) throw new Error("dependent batch visibility failed");
  let failed = false;
  try { sql(afterConfig, "INSERT INTO sessions(session_id,challenge,condition,proof,issued_at,expires_at,used,turnstile_verified,ip_hash) VALUES('rollback','x',0,'x',1,1,1,0,'x'); INSERT INTO scores(name,condition,terminal_total,chickens,coins,objective_completed,max_combo,round_duration_ms,time_left_ms,game_over_reason,build,platform,session_id,submitted_at,ip_hash,status,moderation_note,submission_source,restoration_key) VALUES('BAD',0,1,1,0,0,1,1,0,'time_up','x','web','rollback',1,'x','live',NULL,'admin_restore','rollback'); INSERT INTO admin_restorations(restoration_key,evidence_hash,payload_hash,known_fields_json,synthetic_fields_json,reason,score_id,restored_at,admin) VALUES('broken','short','short','{}','{}','x',999,1,'admin');"); } catch { failed = true; }
  if (!failed) throw new Error("forced transaction failure unexpectedly succeeded");
  const rollback = sql(afterConfig, "SELECT COUNT(*) AS n FROM sessions WHERE session_id='rollback';");
  if (!rollback.includes('"n": 0')) throw new Error("D1 batch did not roll back");
  console.log("local D1 restoration migration/batch tests passed");
} finally { rmSync(temp, { recursive: true, force: true }); }
