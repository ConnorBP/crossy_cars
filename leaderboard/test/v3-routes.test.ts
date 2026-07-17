import { beforeEach, describe, expect, it } from "vitest";
import worker, { type Env } from "../src/index";
import { FakeD1, FakeRateLimit, makeEnv } from "./helpers";

class Cache {
  data = new Map<string,Response>();
  async match(r:Request|string){const x=this.data.get(typeof r==="string"?r:r.url);return x?.clone();}
  async put(r:Request|string,v:Response){this.data.set(typeof r==="string"?r:r.url,v.clone());}
  async delete(){return false;}
}
const cache=new Cache();
(globalThis as unknown as {caches:{default:Cache}}).caches={default:cache};
const ctx={waitUntil<T>(p:Promise<T>){return p;},passThroughOnException(){},props:{}} as unknown as ExecutionContext;
function env(extra:Record<string,unknown>={}):Env{return {...makeEnv(),ROADY_V3_RANKED_ENABLED:"false",...extra} as unknown as Env;}
async function call(e:Env,path:string,init:RequestInit={}){return worker.fetch(new Request(`https://worker.test${path}`,{...init,headers:{Origin:"http://localhost:8080",...init.headers}}),e,ctx);}

beforeEach(()=>cache.data.clear());
describe("additive /v3 capability and strict gate",()=>{
  it("returns the exact disabled capability body and cache policy without v3 secrets",async()=>{
    const response=await call(env(),"/v3/capabilities");
    expect(response.status).toBe(200);
    expect(await response.text()).toBe('{"ranked":{"enabled":false,"categories":["rotation.v2.cluck_hunt","rotation.v2.right_of_way"]},"protocolVersion":3,"protocolId":"roady-protocol.v3","rulesVersion":3,"rulesId":"roady-rules.v3","policyVersion":1,"policyId":"roady-ranked-policy.v3.1","mode":"rotation"}');
    expect(response.headers.get("Cache-Control")).toBe("public, max-age=60, s-maxage=300, stale-while-revalidate=600");
  });
  it("bypasses capability cache on release no-cache probes without changing exact bytes",async()=>{
    const response=await call(env(),"/v3/capabilities",{headers:{"Cache-Control":"no-cache, no-store, max-age=0"}});
    expect(response.status).toBe(200);
    expect(response.headers.get("Cache-Control")).toBe("public, max-age=60, s-maxage=300, stale-while-revalidate=600");
    expect(cache.data.size).toBe(0);
    expect((await response.json() as {ranked:{enabled:boolean}}).ranked.enabled).toBe(false);
  });
  it("keeps the reviewed production latch off even with exact true and complete-looking secrets",async()=>{
    const response=await call(env({ROADY_V3_RANKED_ENABLED:"true",BUILD:"production",LB_V3_PROOF_HMAC_KEY:"proof",LB_V3_SEED_ENCRYPTION_KEY:"AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8",LB_V3_SEED_KEY_ID:"v3.seed.prod.1",LB_V3_EVIDENCE_CAPABILITY_KEY:"cap",LB_V3_CLIENT_HMAC_KEYS_JSON:'{"v3.client.1":"client"}',LB_TURNSTILE_SECRET:"real"}),"/v3/capabilities");
    expect((await response.json() as {ranked:{enabled:boolean}}).ranked.enabled).toBe(false);
  });
  it("rejects a capability query/body and does not alias v3 routes",async()=>{
    expect((await call(env(),"/v3/capabilities?x=1")).status).toBe(422);
    expect((await call(env(),"/v3",{method:"GET"})).status).toBe(404);
    expect((await call(env(),"/api/v3/capabilities")).status).toBe(404);
  });
  it("reapplies per-origin CORS on cached origin-agnostic bytes",async()=>{
    const e=env({ALLOWED_ORIGINS:"http://localhost:8080,https://other.example"});
    const a=await call(e,"/v3/capabilities");
    const b=await worker.fetch(new Request("https://worker.test/v3/capabilities",{headers:{Origin:"https://other.example"}}),e,ctx);
    expect(a.headers.get("Access-Control-Allow-Origin")).toBe("http://localhost:8080");
    expect(b.headers.get("Access-Control-Allow-Origin")).toBe("https://other.example");
  });
  it("requires v3 config for protected handlers and fails session issuance closed",async()=>{
    const e=env({RATE_LIMIT_SESSION:new FakeRateLimit(3),DB:new FakeD1()});
    const response=await call(e,"/v3/session",{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({mode:"rotation",categoryKey:"rotation.v2.cluck_hunt",turnstileToken:"x"})});
    expect(response.status).toBe(503);
    expect(await response.json()).toMatchObject({error:{code:"config_error"}});
  });
  it("returns not_found for unknown v3 routes even when protected config is absent",async()=>{
    const response=await call(env(),"/v3/unknown",{method:"POST",body:"{}"});
    expect(response.status).toBe(404);expect(await response.json()).toMatchObject({error:{code:"not_found"}});
  });
  it("keeps malformed flag values disabled in local builds",async()=>{
    for(const value of ["TRUE"," true ","1",""]){const response=await call(env({BUILD:"dev",ROADY_V3_RANKED_ENABLED:value}),"/v3/capabilities");expect((await response.json() as {ranked:{enabled:boolean}}).ranked.enabled).toBe(false);}
  });
  it("supports OPTIONS for every v3 route through the unchanged CORS policy",async()=>{
    for(const path of ["/v3/capabilities","/v3/session","/v3/session/S/start","/v3/scores","/v3/evidence","/v3/leaderboard","/v3/me/rank","/v3/admin/scores/restore","/v3/admin/scores/1/hide","/v3/admin/scores/1"]){
      const response=await call(env(),path,{method:"OPTIONS"});expect(response.status).toBe(204);expect(response.headers.get("Access-Control-Allow-Origin")).toBe("http://localhost:8080");
    }
  });
});
