// Raw-byte inventory enforcement for the addendum's immutable v1/v2 set.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root=resolve(new URL("../..",import.meta.url).pathname.replace(/^\/(.:)/,"$1"));
const inventory=JSON.parse(readFileSync(resolve(root,"docs/frozen-v1-v2-baseline.sha256.json"),"utf8"));
const git=(...args)=>execFileSync("git",args,{cwd:root});
const sha=(bytes)=>createHash("sha256").update(bytes).digest("hex");
const listed=inventory.files.map(({path})=>path);
if(JSON.stringify(listed)!==JSON.stringify([...listed].sort((a,b)=>Buffer.from(a).compare(Buffer.from(b)))))throw new Error("frozen inventory is not UTF-8 byte-path sorted");
for(const entry of inventory.files){
  const bytes=git("show",`HEAD:${entry.path}`),blob=git("rev-parse",`HEAD:${entry.path}`).toString("utf8").trim();
  if(bytes.length!==entry.size||blob!==entry.blob||sha(bytes)!==entry.sha256)throw new Error(`frozen raw-byte mismatch: ${entry.path}`);
}
console.log(`frozen v1/v2 raw-byte inventory passed (${inventory.files.length} files)`);
