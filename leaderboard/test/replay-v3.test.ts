import { describe,expect,it } from "vitest";
import { replayEvidence } from "../src/replay-v3";
import {
  CanonicalLedger, CLUCK_HUNT_CATEGORY, RIGHT_OF_WAY_CATEGORY, bytesToHex,
  hexToBytes, scheduleHash, seedCommitment, startedSessionHeader,
  type ConductTerminal, type SessionHeader,
} from "../src/rules-v3";

const seed=hexToBytes("0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20");
async function fixture(category:string,terminal:ConductTerminal){
  const header:SessionHeader={category,sessionId:"replay",challenge:"challenge",seedCommitment:await seedCommitment(seed),scheduleHash:await scheduleHash(seed,category),issuedAtMs:1n};
  const ledger=await CanonicalLedger.create(startedSessionHeader(header,2n));
  await ledger.append({seq:0n,activeMs:terminal.durationMs,payload:{type:"terminal",terminal}});
  return{header,ledger};
}
const common=(reason:number)=>({reason,total:0n,objectiveCompleted:false,durationMs:0n,remainingMs:0n,build:"dev",platform:1});

describe("v3 semantic evidence replay",()=>{
  for(const reason of [1,2,3])for(const conduct of ["cluck_hunt","right_of_way"] as const)it(`accepts a complete zero-transition ${conduct} reason ${reason}`,async()=>{
    const terminal:ConductTerminal=conduct==="cluck_hunt"?{conduct,...common(reason),chickens:0n,coins:0n,maxCombo:1}:{conduct,...common(reason),accumulator:0n,premiumBps:10_000n,packagesDelivered:0n,courtesyCount:0n,animalHits:0n,maxDeliveryChain:0n};
    const category=conduct==="cluck_hunt"?CLUCK_HUNT_CATEGORY:RIGHT_OF_WAY_CATEGORY,{header,ledger}=await fixture(category,terminal),root=bytesToHex(await ledger.root());
    const result=await replayEvidence({category_key:category,final_root:root,event_count:1},{started_at:2},header,ledger.storedBytes(),seed,()=>terminal);
    expect(result).toMatchObject({match:true,reason:"match",root});
  });
  it("rejects a self-consistent fabricated nonzero terminal with a valid chain and root",async()=>{
    const terminal:ConductTerminal={conduct:"cluck_hunt",...common(3),total:1n,chickens:1n,coins:0n,maxCombo:1};
    const {header,ledger}=await fixture(CLUCK_HUNT_CATEGORY,terminal),root=bytesToHex(await ledger.root());
    const result=await replayEvidence({category_key:CLUCK_HUNT_CATEGORY,final_root:root,event_count:1},{started_at:2},header,ledger.storedBytes(),seed,()=>terminal);
    expect(result).toMatchObject({match:false,reason:"fabricated_terminal"});
  });
});
