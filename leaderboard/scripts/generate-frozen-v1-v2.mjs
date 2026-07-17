// Generates the reviewed raw-byte baseline inventory. Run from a clean worktree.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { writeFileSync } from "node:fs";
import { resolve } from "node:path";
const root=resolve(new URL("../..",import.meta.url).pathname.replace(/^\/(.:)/,"$1"));
const baseline="ddf6745c76f6eb450378e8acc74c489ef1b56b04";
const git=(...args)=>execFileSync("git",args,{cwd:root});
const baselinePaths=new Set(git("ls-tree","-r","--name-only",baseline).toString("utf8").split(/\r?\n/));
const selected=[
  "rules/roady-rules.v1.json","rules/roady-rules.v1.schema.json","rules/roady-rules.v2.json","rules/roady-rules.v2.schema.json","rules/roady-rules.v2.golden.json",
  "crates/roady-score-rules/tests/golden.rs","crates/roady-score-rules/tests/golden_v2.rs",
  "leaderboard/migrations/0001_init.sql","leaderboard/migrations/0002_indexes.sql","leaderboard/migrations/0003_deterministic_ranking_indexes.sql","leaderboard/migrations/0004_admin_restorations.sql","leaderboard/migrations/0005_ranked_v2.sql",
  "leaderboard/test/helpers.ts","leaderboard/test/index.test.ts","leaderboard/test/routes.test.ts","leaderboard/test/rules-v2.test.ts","leaderboard/test/svg.test.ts",
  ...[...baselinePaths].filter(path=>path.startsWith("docs/data/leaderboard-v1/")),
].sort((a,b)=>Buffer.from(a).compare(Buffer.from(b)));
const files=selected.map(path=>{const ref=baselinePaths.has(path)?baseline:"HEAD",bytes=git("show",`${ref}:${path}`);return{path,size:bytes.length,blob:git("rev-parse",`${ref}:${path}`).toString("utf8").trim(),sha256:createHash("sha256").update(bytes).digest("hex")};});
const output={baselineCommit:baseline,generator:"leaderboard/scripts/generate-frozen-v1-v2.mjs@1 raw git-show bytes; 0005 frozen at implementation HEAD 692e9c7ddebed17b22db14cf68832be63db9cb7e",files};
writeFileSync(resolve(root,"docs/frozen-v1-v2-baseline.sha256.json"),JSON.stringify(output,null,2)+"\n",{encoding:"utf8"});
console.log(`generated frozen v1/v2 inventory (${files.length} files)`);
