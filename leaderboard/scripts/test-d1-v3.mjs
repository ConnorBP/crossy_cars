// Real-D1 proof that 0006 is strictly additive to populated v1/v2 storage.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const cwd = new URL("..", import.meta.url).pathname.replace(/^\/(.:)/, "$1");
const wrangler = join(cwd, "node_modules", "wrangler", "bin", "wrangler.js");
const migrations = (number) => join(cwd, "migrations", `${String(number).padStart(4,"0")}_${number===1?"init":number===2?"indexes":number===3?"deterministic_ranking_indexes":number===4?"admin_restorations":number===5?"ranked_v2":"ranked_v3"}.sql`);
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const run = (state,...args) => execFileSync(process.execPath,[wrangler,...args],{cwd,encoding:"utf8",env:process.env});
const sql = (state,statement) => run(state,"d1","execute","roady_leaderboard","--local","--persist-to",state,"--command",statement,"--json");
const file = (state,path) => run(state,"d1","execute","roady_leaderboard","--local","--persist-to",state,"--file",path);
const normalized = (value) => JSON.stringify(JSON.parse(value).map(({results,success})=>({results,success})));
const snapshot = (state) => {
  const queries = [
    "SELECT * FROM sessions ORDER BY session_id", "SELECT * FROM scores ORDER BY id", "SELECT * FROM moderation_log ORDER BY id", "SELECT * FROM admin_restorations ORDER BY restoration_key",
    "SELECT * FROM score_categories ORDER BY category_key", "SELECT * FROM sessions_v2 ORDER BY session_id", "SELECT * FROM scores_v2 ORDER BY id", "SELECT * FROM score_evidence ORDER BY id", "SELECT * FROM admin_restorations_v2 ORDER BY restoration_key", "SELECT * FROM moderation_log_v2 ORDER BY id",
    "SELECT name,condition,terminal_total,submitted_at FROM scores WHERE status='live' ORDER BY terminal_total DESC,submitted_at,id",
    "SELECT name,category_key,terminal_total,submitted_at FROM scores_v2 WHERE status='live' ORDER BY category_key,terminal_total DESC,submitted_at,id",
  ];
  return queries.map((query)=>{const raw=normalized(sql(state,query));return{query,raw,sha256:hash(Buffer.from(raw))};});
};
const apply = (state,from,to) => { for(let i=from;i<=to;i++) file(state,migrations(i)); };
const states=[];
try {
  // Empty database success means the complete ordered migration chain, not 0006 in isolation.
  const empty=mkdtempSync(join(tmpdir(),"roady-v3-empty-"));states.push(empty);apply(empty,1,6);
  const count=JSON.parse(sql(empty,"SELECT COUNT(*) AS n FROM score_categories_v3"))[0].results[0].n;
  if(count!==2)throw new Error("empty migration chain did not create exact v3 categories");

  const populated=mkdtempSync(join(tmpdir(),"roady-v3-populated-"));states.push(populated);apply(populated,1,5);
  sql(populated,`PRAGMA foreign_keys=ON;
    INSERT INTO sessions VALUES('v1-session','v1-challenge',0,'v1-proof',1,999999,1,1,'v1-ip');
    INSERT INTO scores(name,condition,terminal_total,chickens,coins,objective_completed,max_combo,round_duration_ms,time_left_ms,game_over_reason,build,platform,session_id,submitted_at,ip_hash,status,moderation_note,submission_source,restoration_key) VALUES('V01',0,3,2,1,0,1,60000,0,'time_up','v1','web','v1-session',10,'v1-ip','live',NULL,'verified',NULL);
    INSERT INTO sessions_v2(session_id,category_key,protocol_version,rules_version,policy_version,mode,challenge,proof,seed_enc,seed_commitment,schedule_hash,issued_at,start_by_expiry,started_at,started,used,turnstile_verified,ip_hash) VALUES('v2-session','rotation.v1.cluck_hunt',2,2,1,'rotation','v2-challenge','v2-proof',zeroblob(32),lower(hex(zeroblob(32))),lower(hex(zeroblob(32))),1,NULL,2,1,1,1,'v2-ip');
    INSERT INTO scores_v2(name,category_key,terminal_total,chickens,coins,signed_accumulator,premium_bps,packages_delivered,courtesy_count,animal_hits,objective_completed,max_combo,max_delivery_chain,round_duration_ms,time_left_ms,game_over_reason,build,platform,session_id,submitted_at,ip_hash,status,moderation_note,submission_source,restoration_key,final_root,schedule_hash,event_count,evidence_capability_hash,evidence_expires_at) VALUES('V02','rotation.v1.cluck_hunt',7,5,2,NULL,NULL,NULL,NULL,NULL,1,3,NULL,60000,0,'time_up','v2','web','v2-session',20,'v2-ip','live',NULL,'verified',NULL,lower(hex(zeroblob(32))),lower(hex(zeroblob(32))),1,NULL,NULL);`);
  const before=snapshot(populated),beforeSchema=normalized(sql(populated,"SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name NOT LIKE '_cf_%' AND name NOT LIKE 'sqlite_%' ORDER BY type,name"));
  // Apply only additive 0006 after representative v1/v2 data exist.
  file(populated,migrations(6));
  const after=snapshot(populated);if(JSON.stringify(before)!==JSON.stringify(after))throw new Error("v1/v2 raw query bytes or table hashes changed after 0006");
  const categories=JSON.parse(sql(populated,"SELECT category_key,protocol_version,protocol_id,rules_version,rules_id,policy_version,policy_id,mode,conduct,display_name,active FROM score_categories_v3 ORDER BY category_key"))[0].results;
  const exact=[
    {category_key:"rotation.v2.cluck_hunt",protocol_version:3,protocol_id:"roady-protocol.v3",rules_version:3,rules_id:"roady-rules.v3",policy_version:1,policy_id:"roady-ranked-policy.v3.1",mode:"rotation",conduct:"cluck_hunt",display_name:"Cluck Hunt",active:1},
    {category_key:"rotation.v2.right_of_way",protocol_version:3,protocol_id:"roady-protocol.v3",rules_version:3,rules_id:"roady-rules.v3",policy_version:1,policy_id:"roady-ranked-policy.v3.1",mode:"rotation",conduct:"right_of_way",display_name:"Right of Way",active:1},
  ];
  if(JSON.stringify(categories)!==JSON.stringify(exact))throw new Error("v3 category registry mismatch");
  const fk=JSON.parse(sql(populated,"SELECT m.name AS table_name,f.[table] AS target FROM sqlite_schema m JOIN pragma_foreign_key_list(m.name) f WHERE m.type='table' AND m.name GLOB '*_v3' ORDER BY m.name,f.id"))[0].results;
  if(fk.some(({target})=>!target.endsWith("_v3")))throw new Error("v3 foreign key escapes v3 tables");
  const indexes=JSON.parse(sql(populated,"SELECT name,tbl_name FROM sqlite_schema WHERE type='index' AND name NOT LIKE 'sqlite_%' AND (name LIKE '%v3%' OR tbl_name LIKE '%v3%') ORDER BY name"))[0].results;
  if(indexes.some(({name,tbl_name})=>!name.includes("v3")||!tbl_name.endsWith("_v3")))throw new Error("v3 index isolation failed");
  const afterSchema=normalized(sql(populated,"SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name NOT LIKE '_cf_%' AND name NOT LIKE 'sqlite_%' ORDER BY type,name"));
  if(!afterSchema.includes("scores_v3")||hash(Buffer.from(beforeSchema))===hash(Buffer.from(afterSchema)))throw new Error("schema delta proof failed");
  console.log("v3 D1: ordered empty migration, populated raw-byte isolation, exact registry, FK/index isolation passed");
} finally { for(const state of states)rmSync(state,{recursive:true,force:true}); }
