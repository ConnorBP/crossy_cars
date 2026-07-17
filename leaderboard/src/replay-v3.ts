import { constantTimeEquals, sha256 } from "./security";
import {
  CLUCK_HUNT_CATEGORY, MAX_LEDGER_BYTES, concatBytes, creditedPositive,
  completedWaves, coinClock, finalRoot, packageClock, rotationSchedule,
  scheduledEvents, startedSessionHeader, terminalBytes, timePickupClock,
  bytesToHex, type ConductTerminal, type SessionHeader,
} from "./rules-v3";

export interface ReplayScore {
  category_key:string; final_root:string; event_count:number;
}
export interface ReplaySession { started_at:number|null }
export type TerminalFactory = () => ConductTerminal;

class Reader {
  constructor(readonly bytes:Uint8Array,public at=0){}
  need(n:number){if(!Number.isSafeInteger(n)||n<0||this.at+n>this.bytes.length)throw new Error("truncated");}
  u8(){this.need(1);return this.bytes[this.at++]!;}
  u32(){this.need(4);let n=0;for(let i=0;i<4;i++)n=n*256+this.bytes[this.at++]!;return n;}
  i32(){const n=this.u32();return n>=0x8000_0000?n-0x1_0000_0000:n;}
  u64(){this.need(8);let n=0n;for(let i=0;i<8;i++)n=n*256n+BigInt(this.bytes[this.at++]!);return n;}
  i64(){const n=this.u64();return n>=(1n<<63n)?n-(1n<<64n):n;}
  take(n:number){this.need(n);const value=this.bytes.slice(this.at,this.at+n);this.at+=n;return value;}
  lp1(expected?:string){const value=this.take(this.u8());if(value.length===0)throw new Error("empty_lp1");const text=new TextDecoder("utf-8",{fatal:true,ignoreBOM:true}).decode(value);if(expected!==undefined&&text!==expected)throw new Error("domain");return text;}
}
interface Event { seq:number; ms:bigint; kind:number; p:Record<string,unknown>; record:Uint8Array }
const n=(v:unknown)=>v as number;
const b=(v:unknown)=>v as bigint;
const flag=(v:unknown)=>v as boolean;
function boolean(r:Reader):boolean{const value=r.u8();if(value!==0&&value!==1)throw new Error("invalid_boolean");return value===1;}
function parseEvent(r:Reader,expectedSeq:number,rightOfWay:boolean):Event{
  const begin=r.at;r.lp1("roady.v3.event");const seq=r.u32();if(seq!==expectedSeq)throw new Error("sequence");const ms=r.u64(),kind=r.u8();let p:Record<string,unknown>={};
  switch(kind){
    case 1:p={base:r.u32(),eventBonus:r.u32(),frenzyBonus:r.u32(),comboBefore:r.u8(),comboAfter:r.u8(),bucketBefore:r.u32(),bucketAfter:r.u32()};break;
    case 2:p={mega:boolean(r),base:r.u32(),comboBefore:r.u8(),comboAfter:r.u8(),bucketBefore:r.u32(),bucketAfter:r.u32(),remainingBefore:r.u64(),remainingAfter:r.u64()};break;
    case 3:p={remainingBefore:r.u64(),remainingAfter:r.u64()};break;
    case 4:{const objective=r.u8(),target=r.u32(),base=r.u32();if(!rightOfWay)p={conduct:0,objective,target,base,bucketBefore:r.u32(),bucketAfter:r.u32()};else p={conduct:1,objective,target,base,premium:r.u32(),guilt:boolean(r),credited:r.u32(),before:r.i64(),after:r.i64()};break;}
    case 5:p={penalty:r.u32(),bucketBefore:r.u32(),bucketAfter:r.u32(),cooldown:r.u64()};break;
    case 6:p={segmentKind:r.u8(),effect:r.u8(),active:boolean(r),start:r.u64(),end:r.u64()};break;
    case 7:{const payloadStart=r.at,conduct=r.u8(),reason=r.u8();if(conduct>1)throw new Error("terminal_conduct");if(reason<1||reason>3)throw new Error("terminal_reason");p={conduct,reason,total:r.u32()};if(conduct===0)Object.assign(p,{chickens:r.u32(),coins:r.u32(),objective:boolean(r),maxCombo:r.u8()});else Object.assign(p,{accumulator:r.i64(),premium:r.u32(),packages:r.u32(),courtesy:r.u32(),hits:r.u32(),maxChain:r.u32(),objective:boolean(r)});Object.assign(p,{duration:r.u64(),remaining:r.u64(),build:r.lp1(),platform:r.u8(),terminalBytes:r.bytes.slice(payloadStart,r.at)});break;}
    case 8:p={carriedBefore:r.u8(),carriedAfter:r.u8()};break;
    case 9:p={ordinal:r.u8(),chain:r.u32(),base:r.u32(),premium:r.u32(),guilt:boolean(r),credited:r.u32(),before:r.i64(),after:r.i64(),remainingBefore:r.u64(),remainingAfter:r.u64()};break;
    case 10:p={chickenId:r.u32(),premium:r.u32(),guilt:boolean(r),credited:r.u32(),before:r.i64(),after:r.i64(),cooldown:r.u32()};break;
    case 11:p={animal:r.u8(),delta:r.i32(),premiumBefore:r.u32(),premiumAfter:r.u32(),guiltAfter:r.u64(),before:r.i64(),after:r.i64()};break;
    case 12:p={base:r.u32(),premium:r.u32(),guilt:boolean(r),credited:r.u32(),before:r.i64(),after:r.i64()};break;
    case 13:p={base:r.u32(),premium:r.u32(),guilt:boolean(r),credited:r.u32(),before:r.i64(),after:r.i64(),remainingBefore:r.u64(),remainingAfter:r.u64()};break;
    case 14:{const phase=r.u8();if(phase<1||phase>4)throw new Error("frenzy_phase");p={phase,start:r.u64(),end:r.u64()};break;}
    default:throw new Error("unknown_event");
  }
  const record=r.bytes.slice(begin,r.at);if(record.length>192)throw new Error("record_size");return{seq,ms,kind,p,record};
}
function order(e:Event):number{if(e.kind===7)return 255;if(e.kind===14&&n(e.p.phase)===4)return 0;if(e.kind===6&&!flag(e.p.active))return 1;if(e.kind===6&&flag(e.p.active))return 2;if(e.kind===14&&n(e.p.phase)===1)return 3;if(e.kind===14&&n(e.p.phase)===2)return 5;return 6+e.kind;}
function eq(actual:unknown,expected:unknown,reason:string):void{if(actual!==expected)throw new Error(reason);}
function saturatedAdd(value:bigint,add:bigint):bigint{return value+add>0xffff_ffffn?0xffff_ffffn:value+add;}
function effectAt(seed:Uint8Array,ms:bigint):number|undefined{return rotationSchedule(seed).find(w=>ms>=w.activeStartMs&&ms<w.activeEndMs)?.effect;}
function scheduledAt(seed:Uint8Array,ms:bigint):number|undefined{const values=scheduledEvents(seed,rotationSchedule(seed));if(ms>=15_000n&&ms<23_000n)return values[0];if(ms>=40_000n&&ms<48_000n)return values[1];return undefined;}
function validateSchedule(events:Event[],seed:Uint8Array):void{
  const windows=rotationSchedule(seed),scheduled=scheduledEvents(seed,windows);let previousSegmentStart=-1n;
  for(const event of events){if(event.kind===14){const start=b(event.p.start),end=b(event.p.end);if(end<start)throw new Error("frenzy_schedule");const phase=n(event.p.phase);if((phase===1||phase===2||phase===3)&&event.ms!==start)throw new Error("frenzy_edge");if(phase===4&&event.ms!==end)throw new Error("frenzy_edge");continue;}if(event.kind!==6)continue;const p=event.p,start=b(p.start),end=b(p.end),active=flag(p.active);if(start<previousSegmentStart)throw new Error("schedule_order");previousSegmentStart=start;if(n(p.segmentKind)===0){if(!windows.some(w=>w.effect===n(p.effect)&&w.activeStartMs===start&&w.activeEndMs===end))throw new Error("forced_schedule");}else if(n(p.segmentKind)===1){if(![[15_000n,23_000n],[40_000n,48_000n]].some((x,i)=>x[0]===start&&x[1]===end&&scheduled[i]===n(p.effect)))throw new Error("forced_schedule");}else throw new Error("segment_kind");eq(event.ms,active?start:end,"segment_edge");}
}
function replayCluck(events:Event[],terminal:ConductTerminal,seed:Uint8Array):void{
  if(terminal.conduct!=="cluck_hunt")throw new Error("category_conduct");let chickens=0n,coins=0n,maxCombo=1n,hitCount=0n,coinCount=0n,objective=false;
  if(events.length===0){if(terminal.total!==0n||terminal.chickens!==0n||terminal.coins!==0n||terminal.objectiveCompleted||terminal.maxCombo!==1)throw new Error("fabricated_terminal");return;}
  for(const event of events){const p=event.p;if([8,9,10,11,12,13].includes(event.kind))throw new Error("cross_conduct_event");
    if(event.kind===1){eq(BigInt(n(p.bucketBefore)),chickens,"cluck_before");eq(n(p.base),1,"chicken_base");const combo=BigInt(n(p.comboAfter));if(combo<1n||combo>5n)throw new Error("combo");eq(n(p.eventBonus),scheduledAt(seed,event.ms)===1?1:0,"event_bonus");if(n(p.frenzyBonus)!==0&&n(p.frenzyBonus)!==1)throw new Error("frenzy_bonus");const multiplier=effectAt(seed,event.ms)===4||scheduledAt(seed,event.ms)===2?2n:1n;chickens=saturatedAdd(chickens,1n+BigInt(n(p.eventBonus))+BigInt(n(p.frenzyBonus))+(combo-1n)*multiplier);eq(BigInt(n(p.bucketAfter)),chickens,"cluck_after");hitCount++;if(combo>maxCombo)maxCombo=combo;
    }else if(event.kind===2){eq(BigInt(n(p.bucketBefore)),coins,"coin_before");const award=flag(p.mega)?5n:1n;eq(BigInt(n(p.base)),award,"coin_base");coins=saturatedAdd(coins,award);coinCount++;eq(BigInt(n(p.bucketAfter)),coins,"coin_after");eq(b(p.remainingAfter),coinClock(b(p.remainingBefore)),"coin_clock");
    }else if(event.kind===3)eq(b(p.remainingAfter),timePickupClock(b(p.remainingBefore)),"pickup_clock");
    else if(event.kind===4){if(n(p.conduct)!==0||objective)throw new Error("objective");if(![[1,10],[2,6],[3,3]].some(x=>x[0]===n(p.objective)&&x[1]===n(p.target))||n(p.base)!==10)throw new Error("objective");if((n(p.objective)===1&&hitCount<10n)||(n(p.objective)===2&&coinCount<6n)||(n(p.objective)===3&&maxCombo<3n))throw new Error("objective_not_earned");eq(BigInt(n(p.bucketBefore)),chickens,"objective_before");chickens=saturatedAdd(chickens,10n);eq(BigInt(n(p.bucketAfter)),chickens,"objective_after");objective=true;
    }else if(event.kind===5){eq(n(p.penalty),2,"penalty");eq(BigInt(n(p.bucketBefore)),chickens,"penalty_before");chickens=chickens>=2n?chickens-2n:0n;eq(BigInt(n(p.bucketAfter)),chickens,"penalty_after");}
  }
  chickens=saturatedAdd(chickens,completedWaves(terminal.durationMs)*2n);eq(terminal.chickens,chickens,"terminal_chickens");eq(terminal.coins,coins,"terminal_coins");eq(terminal.total,chickens+coins,"terminal_total");eq(BigInt(terminal.maxCombo),maxCombo,"terminal_combo");eq(terminal.objectiveCompleted,objective,"terminal_objective");
}
function replayRightOfWay(events:Event[],terminal:ConductTerminal):void{
  if(terminal.conduct!=="right_of_way")throw new Error("category_conduct");let accumulator=0n,premiumBps=10_000n,chain=0n,maxChain=0n,carried=0n,packages=0n,courtesy=0n,hits=0n,coins=0n,objective=false,guiltUntil=0n,waves=0n,lastDrop=-1n,lastOrdinal=-1;
  if(events.length===0){if(terminal.total!==0n||terminal.accumulator!==0n||terminal.premiumBps!==10_000n||terminal.packagesDelivered!==0n||terminal.courtesyCount!==0n||terminal.animalHits!==0n||terminal.maxDeliveryChain!==0n||terminal.objectiveCompleted)throw new Error("fabricated_terminal");return;}
  const positive=(event:Event,base:bigint)=>{const p=event.p;eq(BigInt(n(p.premium)),premiumBps,"premium");eq(flag(p.guilt),event.ms<guiltUntil,"guilt");eq(b(p.before),accumulator,"accumulator_before");eq(BigInt(n(p.credited)),creditedPositive(base,premiumBps,flag(p.guilt)),"credited");eq(b(p.after),accumulator+BigInt(n(p.credited)),"accumulator_after");accumulator=b(p.after);};
  for(const event of events){const p=event.p;if([1,2,3,5].includes(event.kind))throw new Error("cross_conduct_event");
    if(event.kind===8){eq(BigInt(n(p.carriedBefore)),carried,"carried_before");if(carried>=3n||n(p.carriedAfter)!==n(p.carriedBefore)+1)throw new Error("package_pickup");carried++;}
    else if(event.kind===9){if(event.ms!==lastDrop){lastDrop=event.ms;lastOrdinal=-1;}if(n(p.ordinal)!==lastOrdinal+1||n(p.ordinal)>2)throw new Error("package_order");lastOrdinal=n(p.ordinal);eq(BigInt(n(p.chain)),chain,"delivery_chain");eq(BigInt(n(p.base)),5n+chain,"delivery_base");positive(event,BigInt(n(p.base)));if(carried<1n)throw new Error("package_without_pickup");carried--;chain++;packages++;if(chain>maxChain)maxChain=chain;eq(b(p.remainingAfter),packageClock(b(p.remainingBefore)),"package_clock");}
    else if(event.kind===10){positive(event,2n);eq(n(p.cooldown),500,"courtesy_cooldown");if(n(p.credited)>0)courtesy++;}
    else if(event.kind===11){if(n(p.animal)>1||n(p.delta)!==-10)throw new Error("animal_hit");eq(BigInt(n(p.premiumBefore)),premiumBps,"premium_before");eq(b(p.before),accumulator,"accumulator_before");eq(b(p.after),accumulator-10n,"animal_delta");premiumBps=premiumBps*9_000n/10_000n;eq(BigInt(n(p.premiumAfter)),premiumBps,"premium_after");eq(b(p.guiltAfter),5_000n,"guilt_after");accumulator-=10n;chain=0n;hits++;guiltUntil=event.ms+5_000n;}
    else if(event.kind===12){positive(event,2n);waves++;if(event.ms<36_000n+(waves-1n)*28_000n)throw new Error("wave_schedule");}
    else if(event.kind===13){positive(event,1n);coins++;eq(b(p.remainingAfter),coinClock(b(p.remainingBefore)),"coin_clock");}
    else if(event.kind===4){if(n(p.conduct)!==1||objective)throw new Error("objective");if(![[4,3],[5,3],[2,6]].some(x=>x[0]===n(p.objective)&&x[1]===n(p.target))||n(p.base)!==10)throw new Error("objective");if((n(p.objective)===4&&packages<3n)||(n(p.objective)===5&&courtesy<3n)||(n(p.objective)===2&&coins<6n))throw new Error("objective_not_earned");positive(event,10n);objective=true;}
  }
  eq(waves,completedWaves(terminal.durationMs),"wave_count");eq(terminal.accumulator,accumulator,"terminal_accumulator");eq(terminal.total,accumulator<0n?0n:accumulator,"terminal_total");eq(terminal.premiumBps,premiumBps,"terminal_premium");eq(terminal.packagesDelivered,packages,"terminal_packages");eq(terminal.courtesyCount,courtesy,"terminal_courtesy");eq(terminal.animalHits,hits,"terminal_hits");eq(terminal.maxDeliveryChain,maxChain,"terminal_chain");eq(terminal.objectiveCompleted,objective,"terminal_objective");
}
export async function replayEvidence(row:ReplayScore,session:ReplaySession,header:SessionHeader,ledger:Uint8Array,seed:Uint8Array,makeTerminal:TerminalFactory):Promise<{match:boolean;reason:string;terminal:Uint8Array;root:string}>{
  if(ledger.length<1||ledger.length>MAX_LEDGER_BYTES)return{match:false,reason:"ledger_size",terminal:new Uint8Array(),root:""};
  const started=startedSessionHeader(header,BigInt(session.started_at!)),h0=await sha256(started);let previous=h0;const reader=new Reader(ledger),events:Event[]=[];let found:Uint8Array<ArrayBufferLike>=new Uint8Array(),priorMs=-1n,priorOrder=-1;
  try{for(let seq=0;seq<row.event_count;seq++){const event=parseEvent(reader,seq,row.category_key!==CLUCK_HUNT_CATEGORY),eventOrder=order(event);if(event.ms<priorMs||(event.ms===priorMs&&eventOrder<priorOrder))throw new Error("event_order");priorMs=event.ms;priorOrder=eventOrder;const stored=reader.take(32),calculated=await sha256(concatBytes(previous,event.record));if(!constantTimeEquals(stored,calculated))throw new Error("chain");previous=stored;if(event.kind===7){if(seq!==row.event_count-1)throw new Error("terminal_not_last");found=event.p.terminalBytes as Uint8Array;}events.push(event);}
    if(reader.at!==ledger.length)throw new Error("trailing_bytes");if(events.length===0||events.at(-1)!.kind!==7)throw new Error("missing_terminal");const terminal=makeTerminal();if(!constantTimeEquals(found,terminalBytes(terminal)))throw new Error("terminal_aggregate");if(events.at(-1)!.ms!==terminal.durationMs)throw new Error("terminal_duration");if(terminal.reason===1&&terminal.remainingMs!==0n)throw new Error("time_up_remaining");for(const event of events)if(event.ms>terminal.durationMs)throw new Error("event_after_terminal_time");validateSchedule(events,seed);const gameplay=events.slice(0,-1);if(row.category_key===CLUCK_HUNT_CATEGORY)replayCluck(gameplay,terminal,seed);else replayRightOfWay(gameplay,terminal);const root=bytesToHex(await finalRoot(h0,previous,terminal));if(root!==row.final_root)throw new Error("root");return{match:true,reason:"match",terminal:found,root};
  }catch(error){return{match:false,reason:String(error instanceof Error?error.message:error),terminal:found,root:""};}
}
