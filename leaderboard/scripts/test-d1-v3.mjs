// Applies every migration to a real local D1, proves the exact v3 registry and
// FK/check isolation, and verifies representative frozen v1 bytes survive.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const cwd = new URL("..", import.meta.url).pathname.replace(/^\/(.:)/, "$1");
const state = mkdtempSync(join(tmpdir(), "roady-v3-d1-"));
const wrangler = join(cwd, "node_modules", "wrangler", "bin", "wrangler.js");
const run = (...args) => execFileSync(process.execPath, [wrangler, ...args], { cwd, encoding: "utf8", env: process.env });
const sql = (statement) => run("d1", "execute", "roady_leaderboard", "--local", "--persist-to", state, "--command", statement, "--json");
try {
  run("d1", "migrations", "apply", "roady_leaderboard", "--local", "--persist-to", state);
  sql("PRAGMA foreign_keys=ON; INSERT INTO sessions(session_id,challenge,condition,proof,issued_at,expires_at,used,turnstile_verified,ip_hash) VALUES('frozen','c',0,'p',1,2,1,1,'h'); INSERT INTO scores(name,condition,terminal_total,chickens,coins,objective_completed,max_combo,round_duration_ms,time_left_ms,game_over_reason,build,platform,session_id,submitted_at,ip_hash,status) VALUES('AAA',0,3,2,1,0,1,1,0,'time_up','x','web','frozen',1,'h','live');");
  const before = sql("SELECT name,condition,terminal_total,submitted_at FROM scores WHERE status='live' ORDER BY terminal_total DESC,submitted_at,id;");
  const categories = sql("SELECT * FROM score_categories_v3 ORDER BY category_key;");
  const after = sql("SELECT name,condition,terminal_total,submitted_at FROM scores WHERE status='live' ORDER BY terminal_total DESC,submitted_at,id;");
  const resultBytes = (value) => JSON.stringify(JSON.parse(value).map(({ results, success }) => ({ results, success })));
  const hash = (value) => createHash("sha256").update(resultBytes(value)).digest("hex");
  if (hash(before) !== hash(after)) throw new Error("v1 board query bytes changed");
  if (!categories.includes("rotation.v2.cluck_hunt") || !categories.includes("rotation.v2.right_of_way")) {
    console.error(categories);
    throw new Error("v3 category registry mismatch");
  }
  let rejected = false;
  try { sql("INSERT INTO score_categories_v3 VALUES('bad',3,'roady-protocol.v3',3,'roady-rules.v3',1,'roady-ranked-policy.v3.1','rotation','cluck_hunt','Cluck Hunt',1);"); } catch { rejected = true; }
  if (!rejected) throw new Error("tuple CHECK failed open");
  console.log("v3 local D1 migrations, checks, and v1 byte isolation passed");
} finally { rmSync(state, { recursive: true, force: true }); }
