import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync, realpathSync, copyFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { randomUUID, createHash } from "node:crypto";
import { createInterface } from "node:readline";
import { once } from "node:events";
import http from "node:http";
import { DatabaseSync } from "node:sqlite";
import { cleanFixtureKeychain, fixtureClaudeCredential } from "./fixtures/service-keychain.mjs";

const systemKeychain = process.argv.includes("--system-keychain");
if (systemKeychain) {
  assert.equal(process.platform, "darwin");
  assert(process.env.LOCAL_AGENT_TEST_SERVICE_BINARY, "Pass the signed macOS app's service binary");
}

const dir=realpathSync(mkdtempSync(path.join(tmpdir(),"local-agent-policy-")));const home=path.join(dir,"home");
const data = path.join(dir, "app");
// Keep the Keychain run on the Claude Code profile so an open OpenCode
// desktop does not block the real configuration write guard.
const agent = systemKeychain ? "claude" : "opencode";
const config = path.join(home, systemKeychain ? ".claude/settings.json" : ".config/opencode/opencode.json");
mkdirSync(path.dirname(config), { recursive: true });
writeFileSync(config, "{}\n", { mode: 0o600 });
const sessions = new Map();
function sessionId(name) {
  if (!systemKeychain) return name;
  if (!sessions.has(name)) sessions.set(name, randomUUID());
  return sessions.get(name);
}
const servers=[],faults=[0,0,0],seen=[];let child,stderr="",holdIndex=-1,arrive,release,retryAfter="1";
const token=randomUUID();const pause=ms=>new Promise(r=>setTimeout(r,ms));
let gatewaySendAfterRestartVerified = false;

try {
  for(let i=0;i<3;i++) {
    const server=http.createServer(async(req,res)=>{
      let text="";for await(const chunk of req)text+=chunk;const b=JSON.parse(text);seen.push({i,model:b.model,assistantNotices:JSON.stringify(b.messages.filter(m=>m.role==="assistant")).includes("【路由提示】")});
      if(holdIndex===i){holdIndex=-1;arrive();await new Promise(r=>{release=r;});}
      const event=(type,value)=>`event: ${type}\ndata: ${JSON.stringify({type,...value})}\n\n`;
      const startEvent=event("message_start",{message:{id:randomUUID(),type:"message",role:"assistant",model:b.model,content:[],usage:{input_tokens:5,output_tokens:0}}});
      if(faults[i]==="quota") {res.writeHead(429,{"Content-Type":"application/json"});res.end(JSON.stringify({error:{type:"insufficient_quota",code:"insufficient_quota",message:"fixture quota exhausted"}}));return;}
      if(["sse-auth","sse-quota","sse-context"].includes(faults[i])) {res.writeHead(200,{"Content-Type":"text/event-stream"});const error=faults[i]==="sse-auth"?{type:"authentication_error",message:"fixture auth"}:faults[i]==="sse-quota"?{type:"rate_limit_error",code:"insufficient_quota",message:"fixture quota"}:{type:"invalid_request_error",code:"context_length_exceeded",message:"Input exceeds the context window"};res.write(startEvent);const frame=event("error",{error});res.write(frame.slice(0,17));setTimeout(()=>res.end(frame.slice(17)),10);return;}
      if(faults[i]==="sse-rate") {res.writeHead(200,{"Content-Type":"text/event-stream"});res.end(startEvent+event("error",{error:{type:"rate_limit_error",message:"fixture throttle"}}));return;}
      if(faults[i]==="sse-partial") {res.writeHead(200,{"Content-Type":"text/event-stream"});res.write(startEvent+event("content_block_start",{index:0,content_block:{type:"text",text:""}})+event("content_block_delta",{index:0,delta:{type:"text_delta",text:"PARTIAL_A"}}));setTimeout(()=>res.end(event("error",{error:{type:"api_error",message:"fixture stream stop"}})),30);return;}
      if(faults[i]){res.writeHead(faults[i],{"Content-Type":"application/json","Retry-After":retryAfter});res.end(JSON.stringify({error:{type:faults[i]===429?"rate_limit_error":"api_error",message:"fixture upstream rejection"}}));return;}
      const classification=JSON.stringify(b.system)?.includes("Classify the current request by its primary purpose");
      const message={id:randomUUID(),type:"message",role:"assistant",model:b.model,content:[{type:"text",text:classification?"1":"OK"}],stop_reason:"end_turn",usage:{input_tokens:5,output_tokens:2}};
      if(b.stream){res.writeHead(200,{"Content-Type":"text/event-stream"});res.end(startEvent+event("content_block_start",{index:0,content_block:{type:"text",text:""}})+event("content_block_delta",{index:0,delta:{type:"text_delta",text:"OK"}})+event("content_block_stop",{index:0})+event("message_delta",{delta:{stop_reason:"end_turn"},usage:{output_tokens:2}})+event("message_stop",{}));return;}
      res.writeHead(200,{"Content-Type":"application/json"});res.end(JSON.stringify(message));
    });await new Promise(r=>server.listen(0,"127.0.0.1",r));servers.push(server);
  }
  const start=async()=>{
    const env = { ...process.env, LOCAL_AGENT_SERVICE_TOKEN: token, LOCAL_AGENT_SOURCE_HOME: home, LOCAL_AGENT_TEST_IDLE_AGENTS: "1" };
    if (systemKeychain) delete env.LOCAL_AGENT_TEST_MEMORY_VAULT;
    else env.LOCAL_AGENT_TEST_MEMORY_VAULT = "1";
    child=spawn(process.env.LOCAL_AGENT_TEST_SERVICE_BINARY??"target/debug/local-agent-service",["--data-dir",data],{env,stdio:["ignore","pipe","pipe"]});
    child.stderr.on("data",b=>stderr+=b);return Promise.race([once(createInterface({input:child.stdout}),"line").then(([l])=>JSON.parse(l)),once(child,"exit").then(()=>{throw Error(stderr);})]);
  };
  let ready=await start();console.log(JSON.stringify({fixtureRoot:dir}));
  const api=async(route,body,method=body===undefined?"GET":"POST")=>{const r=await fetch(`http://127.0.0.1:${ready.port}`+route,{method,headers:{Authorization:`Bearer ${token}`,"Content-Type":"application/json"},body:body===undefined?undefined:JSON.stringify(body)});const v=await r.json();assert.equal(r.status,200,JSON.stringify(v));return v;};
  const routeDetail = async (session) => {
    const query = new URLSearchParams({ agent, session: `${agent}:${sessionId(session)}` });
    for (let tries = 0; tries < 20; tries++) {
      const { items } = await api(`/api/request-dashboard?${query}`);
      if (items[0]) {
        const detail = await api(`/api/request-detail?id=${encodeURIComponent(items[0].id)}`);
        if (detail.routeTrace) return detail;
      }
      await pause(100);
    }
    assert.fail(`Missing route decisions for ${session}`);
  };
  const sources=[];
  for(const [i,server]of servers.entries()){
    const p={id:`custom-${randomUUID()}`,name:`Policy ${i}`,vendor:"custom",protocol:"messages",endpoint:`http://127.0.0.1:${server.address().port}`,model:`m${i}`,models:[`m${i}`],configured:true,credentialRef:null,credentialLabel:null,revision:0,category:"local"};
    await api("/api/providers",p,"PUT");assert.equal((await api(`/api/providers/${p.id}/verify`,{expectedRevision:1,model:p.model})).state,"verified");sources.push(p);
  }
  const models=sources.map(p=>({id:randomUUID(),name:p.name,providerId:p.id,model:p.model,capabilities:{contextWindow:96000,maxOutputTokens:4096,tools:true,thinking:false,inputModalities:["text"]}}));
  let draft={id:randomUUID(),revision:0,name:"Policy fixture",defaultModelId:models[0].id,models,routingPolicy:{enabled:true,order:models.map(m=>m.id),disabledModels:[],fallbackOnFailure:true,fallbackOnRateLimit:true,maxAttempts:3,deadlineSeconds:10,cooldownSeconds:1,limits:[]}};
  let service=await api("/api/model-services",draft,"PUT");
  const status=await api("/api/provider-status");const cfg=status.configs.find(c=>c.agentId===agent);
  const tx=await api("/api/model-service-connections",{agentId:agent,expectedRevision:cfg.revision,serviceId:service.id,expectedServiceRevision:service.revision});await api(`/api/provider-switches/${tx.id}/apply`,{});
  const installed = JSON.parse(readFileSync(config));
  const local = systemKeychain
    ? { baseURL: installed.env.ANTHROPIC_BASE_URL + "/v1", apiKey: fixtureClaudeCredential(installed, data) }
    : installed.provider[installed.model.split("/")[0]].options;
  const configHash = createHash("sha256").update(readFileSync(config)).digest("hex");
  const headersFor = session => ({
    "x-api-key": local.apiKey,
    [systemKeychain ? "session_id" : "x-opencode-session"]: sessionId(session),
    "Content-Type": "application/json",
  });
  const user=text=>({role:"user",content:text});
  const send=async(messages,session="ses_"+randomUUID().replaceAll("-", ""),model="local-agent-default",expected=200,extra={})=>{const r=await fetch(local.baseURL+"/messages",{method:"POST",headers:headersFor(session),body:JSON.stringify({model,max_tokens:100,messages,...extra}),signal:AbortSignal.timeout(20000)});const raw=await r.text();assert.equal(r.status,expected,raw);try{return JSON.parse(raw);}catch(error){if(expected>=400)return {error:{message:raw}};throw error;}};
  const publish=async(policy)=>{draft={...draft,revision:service.revision,routingPolicy:{...draft.routingPolicy,...policy}};service=await api("/api/model-services",draft,"PUT");};
  const noticeText=response=>response.content.filter(p=>p.type==="text").map(p=>p.text).join("");

  const binary=path.resolve(process.env.LOCAL_AGENT_TEST_SERVICE_BINARY??"target/debug/local-agent-service");
  const sdk=(...args)=>execFileSync(binary,["--plugin-sdk",...args],{encoding:"utf8",stdio:["ignore","pipe","pipe"],timeout:30000});
  const cases=[];
  const add=name=>{cases.push(name);console.log(JSON.stringify({case:name,passed:true}));};
  seen.length=0;assert.equal((await send([user("unchanged default")])).model,"m0");assert.deepEqual(seen.map(r=>r.i),[0]);assert.equal(existsSync(path.join(dir,"app/routing-policy.sqlite3")),false);add("default route requires no policy database or worker");
  const storeId=(await api("/api/model-services")).writeProtocol.storeId;
  const policy=body=>api("/api/routing-policy",body.kind==="publish"?body:{storeId,...body});
  const publishRaw=async body=>{const r=await fetch("http://127.0.0.1:"+ready.port+"/api/routing-policy",{method:"POST",headers:{Authorization:"Bearer "+token,"Content-Type":"application/json"},body:JSON.stringify(body)});const v=await r.json();return {status:r.status,body:v};};
  const rejected=async(body,code)=>{const r=await fetch(`http://127.0.0.1:${ready.port}/api/routing-policy`,{method:"POST",headers:{Authorization:`Bearer ${token}`,"Content-Type":"application/json"},body:JSON.stringify({storeId,...body})});const v=await r.json();assert.notEqual(r.status,200,JSON.stringify(v));if(code)assert.equal(v.code,code,JSON.stringify(v));return v;};
  const key=path.join(dir,"signing-key");sdk("keygen",key);
  const baseManifest=JSON.parse(readFileSync("sdk/plugins/examples/pure-policy/package/manifest.json"));
  const packages={};
  const install=async(name,wasm="sdk/plugins/examples/pure-policy/package/plugin.wasm",version="1.0.0",pluginId=`org.example.policy-${name}`)=>{
    const folder=path.join(dir,`${name}-${version}`);mkdirSync(folder);writeFileSync(path.join(folder,"manifest.json"),JSON.stringify({...baseManifest,id:pluginId,version}));copyFileSync(wasm,path.join(folder,"plugin.wasm"));const archive=path.join(dir,`${name}-${version}.laplug`);sdk("pack",folder,archive,key);const checked=JSON.parse(sdk("check",archive));assert.equal(checked.apiVersion,"1.0.0");assert.equal(checked.manifest.manifestVersion,1);
    const {review}=await policy({kind:"inspect",path:archive});assert.equal(review.digest,checked.digest);const q={pluginId:checked.manifest.id,digest:checked.digest};
    if(name==="good"){await rejected({kind:"install",packageRef:review.packageRef,digest:review.digest,expectedSigningKey:review.signingKey,approvePublisher:false},"publisher_untrusted");}
    await policy({kind:"install",packageRef:review.packageRef,digest:review.digest,expectedSigningKey:review.signingKey,approvePublisher:true});const enabled=await policy({kind:"state",package:q,action:"enable"});assert.equal(enabled.status.active,true);packages[name]=q;return q;
  };
  let binding;let bindingRevision=null;
  const bind=async(q,failureMode="reject")=>{const r=await policy({kind:"bind",serviceId:service.id,serviceVersionId:service.versionId,expectedBindingRevision:bindingRevision,package:q,failureMode});binding=r.binding;bindingRevision=binding.revision;return binding;};
  const good=await install("good");await bind(good);
  seen.length=0;assert.equal((await send([user("reverse order")])).model,"m2");assert.deepEqual(seen.map(r=>r.i),[2]);add("signed pure policy package changes only allowed ordering");

  const preferredAlias=`local-agent-model-${models[2].id}`;
  await publish({order:[models[0].id,models[2].id,models[1].id]});await bind(good);seen.length=0;assert.equal((await send([user("candidate order remains published order")],undefined,preferredAlias)).model,"m2");assert.deepEqual(seen.map(r=>r.i),[2]);
  const configured=await install("configured",".work/domain-d5-policy-fixtures/configured.wasm");await bind(configured);seen.length=0;assert.equal((await send([user("configured order is not pre-ranked by native preference")],undefined,preferredAlias)).model,"m0");assert.deepEqual(seen.map(r=>r.i),[0]);
  await publish({order:models.map(m=>m.id)});await bind(good);add("builtin and external contracts share published candidate order and unmodified configured order");
  const empty=await install("empty",".work/domain-d5-policy-fixtures/empty.wasm");await bind(empty,"builtin");seen.length=0;
  const emptySession = "ses_policytraceempty";
  await send([user("valid empty subset cannot add omitted candidates")],emptySession,"local-agent-default",429);
  assert.equal(seen.length,0);
  const emptyTrace = await routeDetail(emptySession);
  assert.deepEqual(emptyTrace.request.attempts, []);
  assert.equal(emptyTrace.routeTrace.sentUpstream, false);
  assert.deepEqual(
    emptyTrace.routeTrace.decisions.map(d => [d.reason, d.path[0].modelId, d.path[0].versionId]),
    models.map(model => ["policy-excluded", model.id, service.versionId]),
  );
  await bind(good);add("successful empty subset records exclusions without creating upstream attempts");
  const beforeList=seen.length;const inventory=await policy({kind:"list"});assert.equal(inventory.bindings[0].authorized,true);assert.equal(seen.length,beforeList);await rejected({kind:"bind",serviceId:service.id,serviceVersionId:service.versionId,expectedBindingRevision:null,package:good,failureMode:"reject"},"stale_snapshot");add("metadata reads do not run a model; binding uses revision CAS");
  await policy({kind:"state",package:good,action:"disable"});seen.length=0;await send([user("reject disabled policy")],undefined,"local-agent-default",503);assert.equal(seen.length,0);
  await policy({kind:"state",package:good,action:"enable"});await bind(good,"builtin");await policy({kind:"state",package:good,action:"disable"});seen.length=0;assert.equal((await send([user("explicit builtin failure choice")])).model,"m0");assert.deepEqual(seen.map(r=>r.i),[0]);await policy({kind:"state",package:good,action:"enable"});await bind(good);add("disabled or stale authorization rejects unless builtin fallback was explicitly chosen");
  await publish({
    intentClassifier:{modelId:models[0].id},
    rules:[{id:randomUUID(),name:"Intent route",modelId:models[1].id,intent:"Debug"}],
  });
  await bind(good);seen.length=0;
  assert.equal((await send([user("Debug a test")])).model,"m1");
  assert.deepEqual(seen.map(r=>r.i),[0,1]);
  await policy({kind:"state",package:good,action:"disable"});seen.length=0;
  await send([user("Revoked policy cannot send classifier traffic")],undefined,"local-agent-default",503);
  assert.equal(seen.length,0);
  await policy({kind:"state",package:good,action:"enable"});
  await publish({intentClassifier:undefined,rules:[]});await bind(good);
  add("intent classification follows the live fail-closed policy grant and its answer stays in the ranked candidates");
  for(const mode of ["duplicate","foreign","stale","host-call","loop","memory"]){
    const q=await install(mode,`.work/domain-d5-policy-fixtures/${mode}.wasm`);await bind(q);seen.length=0;await send([user(`${mode} rejected before send`)],undefined,"local-agent-default",503);assert.equal(seen.length,0);await bind(q,"builtin");assert.equal((await send([user(`${mode} explicit builtin`)] )).model,"m0");assert.deepEqual(seen.map(r=>r.i),[0]);
  }add("duplicate, foreign, stale, broker access, fuel exhaustion and excessive allocation cannot send or expand authority");
  await bind(good);await publish({disabledModels:[models[2].id]});seen.length=0;await send([user("new version is not implicitly rebound")],undefined,"local-agent-default",503);assert.equal(seen.length,0);await bind(good);assert.equal((await send([user("disabled candidate stays excluded")])).model,"m1");assert.deepEqual(seen.map(r=>r.i),[1]);add("service version changes need explicit binding and disabled candidates are excluded");
  await publish({disabledModels:[],maxAttempts:1});await bind(good);faults[2]=500;seen.length=0;
  const attemptBudgetSession = "ses_policytracebudget";
  await send([user("host attempt budget")],attemptBudgetSession,"local-agent-default",500);
  assert.deepEqual(seen.map(r=>r.i),[2]);
  const budgetTrace = await routeDetail(attemptBudgetSession);
  assert.equal(budgetTrace.request.attempts.length, 1);
  assert.deepEqual(budgetTrace.routeTrace.decisions.map(d => d.reason), ["attempt-limit", "attempt-limit"]);
  faults[2]=0;await pause(1100);await publish({maxAttempts:3});await bind(good);add("external order cannot expand host attempt budget and remaining candidates are recorded");

  await publish({enabled:false});seen.length=0;await send([user("disabled routing cannot bypass external binding")],undefined,"local-agent-default",503);assert.equal(seen.length,0);await publish({enabled:true});await bind(good);add("disabling host strategy cannot silently bypass a required external binding");
  const upgrade=await install("upgrade","sdk/plugins/examples/pure-policy/package/plugin.wasm","1.1.0",good.pluginId);seen.length=0;await send([user("upgrade does not change pinned version")],undefined,"local-agent-default",503);assert.equal(seen.length,0);assert.equal((await policy({kind:"list"})).bindings[0].package.digest,good.digest);await bind(upgrade);assert.equal((await send([user("explicit upgrade binding")])).model,"m2");await policy({kind:"state",package:upgrade,action:"rollback"});seen.length=0;await send([user("rollback needs binding")],undefined,"local-agent-default",503);assert.equal(seen.length,0);await bind(good);assert.equal((await send([user("explicit old binding restored")])).model,"m2");add("upgrade and rollback keep published bindings pinned until explicit rebind");

  await publish({maxAttempts:1});await bind(good);const pinnedSession="ses_"+randomUUID().replaceAll("-","");const pinnedTurn=[user("failed turn preserves its captured policy version")];faults[2]=500;seen.length=0;await send(pinnedTurn,pinnedSession,"local-agent-default",500);assert.deepEqual(seen.map(r=>r.i),[2]);faults[2]=0;
  await publish({maxAttempts:3});await bind(empty,"builtin");await policy({kind:"state",package:good,action:"disable"});await pause(1100);seen.length=0;await send(pinnedTurn,pinnedSession,"local-agent-default",503);assert.equal(seen.length,0);await send([user("current empty strategy stays separate")],undefined,"local-agent-default",429);assert.equal(seen.length,0);await policy({kind:"state",package:good,action:"enable"});await bind(good);add("failed turns keep their captured policy version and cannot borrow a newer version failure choice");
  // Inflight uses the captured version. A revoke before a fallback cancels that fallback.
  holdIndex=2;faults[2]=500;const entered=new Promise(r=>{arrive=r;});seen.length=0;const flight=send([user("revoke after first send")],undefined,"local-agent-default",500).then(value=>({value}),error=>({error}));
  let arrivalTimer;try{await Promise.race([entered,flight.then(result=>{if(result.error)throw result.error;throw Error("request completed without reaching held upstream");}),new Promise((_,reject)=>{arrivalTimer=setTimeout(()=>reject(Error("held upstream was not reached")),15000);})]);}finally{clearTimeout(arrivalTimer);}
  await policy({kind:"state",package:good,action:"revoke-grants"});release();const flightResult=await flight;if(flightResult.error)throw flightResult.error;assert.deepEqual(seen.map(r=>r.i),[2]);faults[2]=0;await pause(1100);await policy({kind:"state",package:good,action:"enable"});await bind(good);add("revocation during a failed first attempt cannot initiate a second send");

  const interruptedFallback = async session => {
    const input = [user("resume only the originally ranked external backup")];
    const db = new DatabaseSync(path.join(data, "local-agent.sqlite3"));
    try {
      db.exec(`CREATE TRIGGER fixture_external_backup BEFORE UPDATE ON service_route_attempts
        WHEN NEW.session='${agent}:${sessionId(session)}' AND NEW.model<>'${models[2].id}'
        BEGIN SELECT RAISE(FAIL,'fixture backup interrupted'); END`);
      faults[2] = 429; seen.length = 0;
      await send(input, session, "local-agent-default", 503);
      assert.deepEqual(seen.map(call => call.i), [2]);
    } finally {
      db.exec("DROP TRIGGER IF EXISTS fixture_external_backup");
      db.close();
      faults[2] = 0;
    }
    return input;
  };
  const liveSession = "ses_externalFallbackLive";
  const liveInput = await interruptedFallback(liveSession);
  await pause(1100); seen.length = 0;
  assert.equal((await send(liveInput, liveSession)).model, "m1");
  assert.deepEqual(seen.map(call => call.i), [1]);
  const revokedSession = "ses_externalFallbackRevoked";
  const revokedInput = await interruptedFallback(revokedSession);
  await policy({kind:"state",package:good,action:"revoke-grants"});
  await policy({kind:"state",package:good,action:"enable"});
  await bind(good);
  seen.length = 0;
  await send(revokedInput, revokedSession, "local-agent-default", 503);
  assert.equal(seen.length, 0);
  await pause(1100);
  add("interrupted fallback keeps the original external order and cannot use a replacement grant");

  await publish({deadlineSeconds:5});await bind(good);seen.length=0;holdIndex=2;const budgetSession="ses_"+randomUUID().replaceAll("-","");const budgetTurn=[user("external ranking shares whole service deadline")];const budgetStart=performance.now();
  const deadlineResponse=await fetch(local.baseURL+"/messages",{method:"POST",headers:headersFor(budgetSession),body:JSON.stringify({model:"local-agent-default",max_tokens:100,messages:budgetTurn}),signal:AbortSignal.timeout(10000)});
  const deadlineBody=await deadlineResponse.text();const deadlineElapsed=performance.now()-budgetStart;assert(deadlineElapsed<5500,"whole service deadline expanded: "+deadlineElapsed);assert([503,504].includes(deadlineResponse.status),deadlineBody);
  if(seen.length){assert.equal(deadlineResponse.status,504,deadlineBody);assert.deepEqual(seen.map(r=>r.i),[2]);release();await send(budgetTurn,budgetSession,"local-agent-default",409);assert.equal(seen.length,1);}else{assert.equal(JSON.parse(deadlineBody).error.code,"deadline_exceeded");holdIndex=-1;}
  const externalUnknownSendAttemptVerified=seen.length===1;
  await publish({deadlineSeconds:10});await bind(good);add("external ranking consumes the whole service deadline and never extends the send budget");
  // The preceding deadline may put this candidate in its configured one-second cooldown.
  await pause(1100);
  faults[2]="sse-partial";seen.length=0;
  const partial=await fetch(local.baseURL+"/messages",{method:"POST",headers:headersFor("ses_"+randomUUID().replaceAll("-","")),body:JSON.stringify({model:"local-agent-default",max_tokens:100,stream:true,messages:[user("external policy partial stream")]})});
  const partialBody=await partial.text();assert.equal(partial.status,200,partialBody);assert(partialBody.includes("PARTIAL_A"));assert.deepEqual(seen.map(r=>r.i),[2]);faults[2]=0;await pause(1100);add("external policy cannot retry another candidate after partial streamed content");
  // Durable publication delegates the original operation ID; queries never replay it.
  const preparationDb=new DatabaseSync(path.join(dir,"app/routing-policy.sqlite3"));
  preparationDb.exec("CREATE TRIGGER deny_begin_publication BEFORE INSERT ON policy_intents WHEN json_extract(NEW.record,'$.status.state')='committing' BEGIN SELECT RAISE(FAIL,'owned preparation pause'); END");
  const cancelledRequest={storeId,operationId:randomUUID(),action:{kind:"save",draft:{...draft,revision:service.revision,name:"Cancelled preparation"}}};
  seen.length=0;const preparation=await publishRaw({kind:"publish",request:cancelledRequest,expectedBindingRevision:binding.revision,package:good,failureMode:"reject"});assert.notEqual(preparation.status,200);
  assert.equal((await policy({kind:"query-publish",operationId:cancelledRequest.operationId})).status.state,"prepared");
  const pausedResume=await publishRaw({kind:"resume-publish",storeId,operationId:cancelledRequest.operationId});assert.notEqual(pausedResume.status,200);assert.equal((await api("/api/model-services")).services[0].revision,service.revision);
  assert.equal((await policy({kind:"cancel-publish",operationId:cancelledRequest.operationId})).status.state,"cancelled");
  assert.equal((await policy({kind:"resume-publish",operationId:cancelledRequest.operationId})).status.state,"cancelled");assert.equal((await api("/api/model-services")).services[0].revision,service.revision);assert.equal(seen.length,0);
  preparationDb.exec("DROP TRIGGER deny_begin_publication");preparationDb.close();bindingRevision=(await policy({kind:"list"})).scopes[0].revision;await bind(good);
  add("prepared cancellation is atomic and resuming a cancelled identity never publishes");
  const request={storeId,operationId:randomUUID(),action:{kind:"save",draft:{...draft,revision:service.revision,name:"Published with policy"}}};
  const published=await policy({kind:"publish",request,expectedBindingRevision:binding.revision,package:good,failureMode:"reject"});assert.equal(published.status.state,"committed");binding=published.status.binding;bindingRevision=binding.revision;service=(await api("/api/model-services")).services.find(s=>s.id===service.id);draft={...draft,revision:service.revision,name:service.name};
  const original=await api(`/api/model-service-operations/${request.operationId}?storeId=${storeId}`);assert.equal(original.service.versionId,service.versionId);const query=await policy({kind:"query-publish",operationId:request.operationId});assert.equal(query.status.binding.revision,binding.revision);assert.equal((await api("/api/model-services")).services[0].revision,service.revision);add("publish and query preserve original core receipt without duplicate publication");
  // Inspect the independently persisted publication record.
  const policyDb=new DatabaseSync(path.join(dir,"app/routing-policy.sqlite3"));const coreDb=new DatabaseSync(path.join(dir,"app/local-agent.sqlite3"));
  const savedIntent=JSON.parse(policyDb.prepare("SELECT record FROM policy_intents WHERE id=?").get(request.operationId).record);assert.equal(savedIntent.status.state,"committed");
  policyDb.close();coreDb.close();

  const failureDb=new DatabaseSync(path.join(dir,"app/routing-policy.sqlite3"));
  const bindingScope=storeId+":"+service.id;
  const originalBindingRecord=failureDb.prepare("SELECT record FROM policy_bindings WHERE scope=? AND version=?").get(bindingScope,service.versionId).record;
  const redirectedBinding=JSON.parse(originalBindingRecord);redirectedBinding.status.serviceId=randomUUID();failureDb.prepare("UPDATE policy_bindings SET record=? WHERE scope=? AND version=?").run(JSON.stringify(redirectedBinding),bindingScope,service.versionId);
  const damagedInventory=await policy({kind:"list"});assert.equal(damagedInventory.bindings[0].authorized,false);assert.equal(damagedInventory.bindings[0].errorCode,"invalid_record");const damagedPublication=damagedInventory.publishes.find(p=>p.operationId===request.operationId);assert(damagedPublication);assert.equal(damagedPublication.state,"committed");assert.equal(damagedPublication.binding.authorized,false);
  assert.equal((await policy({kind:"resume-publish",operationId:request.operationId})).status.state,"committed");seen.length=0;await send([user("stored binding cannot redirect its scope")],undefined,"local-agent-default",503);assert.equal(seen.length,0);
  failureDb.prepare("UPDATE policy_bindings SET record=? WHERE scope=? AND version=?").run(originalBindingRecord,bindingScope,service.versionId);assert.equal((await policy({kind:"list"})).bindings[0].authorized,true);assert.equal((await api("/api/model-services")).services[0].revision,service.revision);
  add("mis-scoped stored bindings remain unauthorized without hiding publication history or replaying it");
  failureDb.exec("CREATE TRIGGER deny_policy_finalize BEFORE INSERT ON policy_intents WHEN json_extract(NEW.record,'$.status.state')='committed' BEGIN SELECT RAISE(FAIL,'owned finalize failure'); END");
  const finalizeRequest={storeId,operationId:randomUUID(),action:{kind:"save",draft:{...draft,revision:service.revision,name:"Finalize recovery"}}};
  const finalFailed=await publishRaw({kind:"publish",request:finalizeRequest,expectedBindingRevision:binding.revision,package:good,failureMode:"reject"});assert.notEqual(finalFailed.status,200);
  const mainCommitted=await api("/api/model-service-operations/"+finalizeRequest.operationId+"?storeId="+storeId);assert.equal(mainCommitted.committedRevision,service.revision+1);
  const pendingInventory=await policy({kind:"list"});assert.equal(pendingInventory.scopes[0].pendingOperationId,finalizeRequest.operationId);
  seen.length=0;await send([user("finalization failure retains pending barrier")],undefined,"local-agent-default",503);assert.equal(seen.length,0);
  await rejected({kind:"unbind",serviceId:service.id,expectedBindingRevision:pendingInventory.scopes[0].revision},"result_pending");
  const blockedPublish=await publishRaw({kind:"publish",request:{...finalizeRequest,operationId:randomUUID(),action:{kind:"save",draft:{...finalizeRequest.action.draft,revision:mainCommitted.committedRevision}}},expectedBindingRevision:pendingInventory.scopes[0].revision,package:good,failureMode:"reject"});assert.notEqual(blockedPublish.status,200);
  failureDb.exec("DROP TRIGGER deny_policy_finalize");failureDb.close();
  const finalized=await policy({kind:"query-publish",operationId:finalizeRequest.operationId});assert.equal(finalized.status.state,"committed");binding=finalized.status.binding;bindingRevision=binding.revision;service=mainCommitted.service;draft={...draft,revision:service.revision,name:service.name};assert.equal((await api("/api/model-services")).services[0].revision,service.revision);add("lost policy finalization is recovered from original receipt without replaying publication");

  const uncertainDb=new DatabaseSync(path.join(dir,"app/local-agent.sqlite3"));
  uncertainDb.exec("CREATE TRIGGER deny_core_receipt BEFORE INSERT ON model_service_operations BEGIN SELECT RAISE(FAIL,'owned core commit failure'); END");
  const uncertainRequest={storeId,operationId:randomUUID(),action:{kind:"save",draft:{...draft,revision:service.revision,name:"Explicit original operation recovery"}}};
  const uncertain=await policy({kind:"publish",request:uncertainRequest,expectedBindingRevision:binding.revision,package:good,failureMode:"reject"});assert.equal(uncertain.status.state,"unknown");assert.equal((await policy({kind:"query-publish",operationId:uncertainRequest.operationId})).status.state,"unknown");
  assert.equal((await policy({kind:"publish",request:uncertainRequest,expectedBindingRevision:binding.revision,package:good,failureMode:"reject"})).status.state,"unknown");
  seen.length=0;await send([user("pending publication blocks only new sends")],undefined,"local-agent-default",503);assert.equal(seen.length,0);await rejected({kind:"cancel-publish",operationId:uncertainRequest.operationId},"result_pending");
  assert.equal((await api("/api/model-services")).services[0].revision,service.revision);
  uncertainDb.exec("DROP TRIGGER deny_core_receipt");uncertainDb.close();
  const changedOriginal={...uncertainRequest,action:{kind:"save",draft:{...uncertainRequest.action.draft,name:"Altered original operation"}}};
  const changedResponse=await fetch("http://127.0.0.1:"+ready.port+"/api/model-service-operations",{method:"POST",headers:{Authorization:"Bearer "+token,"Content-Type":"application/json"},body:JSON.stringify(changedOriginal)});assert.notEqual(changedResponse.status,200);assert.equal((await changedResponse.json()).code,"result_pending");
  const explicitlyRecovered=await api("/api/model-service-operations",uncertainRequest);assert.equal(explicitlyRecovered.committedRevision,service.revision+1);const recoveredIntent=await policy({kind:"query-publish",operationId:uncertainRequest.operationId});assert.equal(recoveredIntent.status.state,"committed");binding=recoveredIntent.status.binding;bindingRevision=binding.revision;service=explicitlyRecovered.service;draft={...draft,revision:service.revision,name:service.name};add("unknown cross-store outcome blocks new sends and is never automatically replayed or cancelled");
  const resumeDb=new DatabaseSync(path.join(dir,"app/local-agent.sqlite3"));
  resumeDb.exec("CREATE TRIGGER deny_resume_core_receipt BEFORE INSERT ON model_service_operations BEGIN SELECT RAISE(FAIL,'owned resume failure'); END");
  const resumeRequest={storeId,operationId:randomUUID(),action:{kind:"save",draft:{...draft,revision:service.revision,name:"Resume original request"}}};
  seen.length=0;const resumeUnknown=await policy({kind:"publish",request:resumeRequest,expectedBindingRevision:binding.revision,package:good,failureMode:"reject"});assert.equal(resumeUnknown.status.state,"unknown");
  await policy({kind:"state",package:good,action:"disable"});
  const extraResume=await fetch("http://127.0.0.1:"+ready.port+"/api/routing-policy",{method:"POST",headers:{Authorization:"Bearer "+token,"Content-Type":"application/json"},body:JSON.stringify({kind:"resume-publish",storeId,operationId:resumeRequest.operationId,request:{...resumeRequest,operationId:randomUUID()}})});assert.notEqual(extraResume.status,200);await extraResume.text();
  assert.equal((await policy({kind:"query-publish",operationId:resumeRequest.operationId})).status.state,"unknown");assert.equal((await api("/api/model-services")).services[0].revision,service.revision);
  resumeDb.exec("DROP TRIGGER deny_resume_core_receipt");resumeDb.close();
  const resumed=await Promise.all([policy({kind:"resume-publish",operationId:resumeRequest.operationId}),policy({kind:"resume-publish",operationId:resumeRequest.operationId})]);
  assert(resumed.every(r=>r.status.state==="committed"&&r.status.binding.authorized===false));assert.equal(resumed[0].status.binding.revision,resumed[1].status.binding.revision);
  const resumedReceipt=await api("/api/model-service-operations/"+resumeRequest.operationId+"?storeId="+storeId);assert.equal(resumedReceipt.committedRevision,service.revision+1);assert.equal(resumedReceipt.service.name,"Resume original request");
  service=resumedReceipt.service;draft={...draft,revision:service.revision,name:service.name};binding=resumed[0].status.binding;bindingRevision=binding.revision;
  assert.equal((await policy({kind:"resume-publish",operationId:resumeRequest.operationId})).status.binding.revision,binding.revision);assert.equal((await api("/api/model-services")).services[0].revision,service.revision);assert.equal(seen.length,0);
  await policy({kind:"state",package:good,action:"enable"});assert.equal((await policy({kind:"query-publish",operationId:resumeRequest.operationId})).status.binding.authorized,false);await bind(good);
  add("explicit original resume is idempotent under concurrent requests and never renews policy authorization");
  const configBefore=readFileSync(config);const unbound=await policy({kind:"unbind",serviceId:service.id,expectedBindingRevision:binding.revision});bindingRevision=unbound.revision;binding=undefined;seen.length=0;assert.equal((await send([user("explicit unbind restores builtin")])).model,"m0");assert.deepEqual(seen.map(r=>r.i),[0]);assert.deepEqual(readFileSync(config),configBefore);add("explicit unbind restores native ordering without changing Agent configuration");
  await bind(good);
  const restartDb=new DatabaseSync(path.join(dir,"app/routing-policy.sqlite3"));restartDb.exec("CREATE TRIGGER deny_restart_finalize BEFORE INSERT ON policy_intents WHEN json_extract(NEW.record,'$.status.state')='committed' BEGIN SELECT RAISE(FAIL,'owned restart finalize failure'); END");
  const restartRequest={storeId,operationId:randomUUID(),action:{kind:"save",draft:{...draft,revision:service.revision,name:"Restart finalization"}}};
  const restartFailed=await publishRaw({kind:"publish",request:restartRequest,expectedBindingRevision:binding.revision,package:good,failureMode:"reject"});assert.notEqual(restartFailed.status,200);
  const restartReceipt=await api("/api/model-service-operations/"+restartRequest.operationId+"?storeId="+storeId);restartDb.exec("DROP TRIGGER deny_restart_finalize");restartDb.close();
  child.kill("SIGTERM");await once(child,"exit");ready=await start();seen.length=0;
  await send([user("restart does not restore policy grants")],undefined,"local-agent-default",systemKeychain ? 503 : 401);
  assert.equal(seen.length,0);
  const recoveredAfterRestart=await policy({kind:"query-publish",operationId:restartRequest.operationId});assert.equal(recoveredAfterRestart.status.state,"committed");assert.equal(recoveredAfterRestart.status.binding.authorized,false);assert.equal((await api("/api/model-services")).services[0].revision,restartReceipt.committedRevision);
  const afterRestart=await policy({kind:"list"});assert.equal(afterRestart.bindings[0].authorized,false);assert.equal(afterRestart.scopes[0].pendingOperationId,null);assert(afterRestart.publishes.every(p=>!p.binding||p.binding.authorized===false));
  const replayDisabled=await policy({kind:"publish",request:restartRequest,expectedBindingRevision:binding.revision,package:good,failureMode:"reject"});assert.equal(replayDisabled.status.state,"committed");assert.equal(replayDisabled.status.binding.authorized,false);
  await policy({kind:"state",package:good,action:"enable"});assert.equal((await policy({kind:"query-publish",operationId:restartRequest.operationId})).status.binding.authorized,false);
  if (systemKeychain) {
    await send([user("plugin enable alone cannot restore the old grant")], undefined, "local-agent-default", 503);
    assert.equal(seen.length, 0);
  }
  service=restartReceipt.service;draft={...draft,revision:service.revision,name:service.name};binding=recoveredAfterRestart.status.binding;bindingRevision=binding.revision;await bind(good);assert.equal((await policy({kind:"list"})).bindings[0].authorized,true);assert.equal(seen.length,0);add("restart finalizes the original receipt without model replay or implicit policy authorization");
  if (systemKeychain) {
    assert.equal(createHash("sha256").update(readFileSync(config)).digest("hex"), configHash);
    assert(fixtureClaudeCredential(JSON.parse(readFileSync(config)), data) === local.apiKey,
      "The original Claude Code credential helper returned a different token after restart");
    assert.equal((await send([user("restart with the original local entry and explicit rebind")])).model, "m2");
    assert.deepEqual(seen.map(request => request.i), [2]);
    gatewaySendAfterRestartVerified = true;
    add("the original Keychain-backed gateway entry works after restart and explicit rebind");
  }
  const schemaDb=new DatabaseSync(path.join(data,"local-agent.sqlite3"));const mainSchemaVersion=schemaDb.prepare("PRAGMA user_version").get().user_version;schemaDb.close();assert.equal(mainSchemaVersion,16);console.log(JSON.stringify({complete:true,cases,realModelCalls:0,systemKeychain,mainSchemaVersion,externalUnknownSendAttemptVerified,gatewaySendAfterRestartVerified,...(!systemKeychain && {realKeychainCalls:0,restartLimit:"MemoryVault intentionally does not persist local entry credentials; policy metadata reauthorization verified, real Keychain restart unverified"})}));
} finally {
  release?.();if(child&&child.exitCode===null){child.kill("SIGTERM");await once(child,"exit");}
  for(const server of servers)await new Promise(r=>server.close(r));
  if (systemKeychain) {
    const cleanup = cleanFixtureKeychain(data);
    assert(!gatewaySendAfterRestartVerified || cleanup.removed >= 3, "The fixture did not exercise system Keychain");
    console.log(JSON.stringify(cleanup));
  }
  if(process.env.KEEP_POLICY_FIXTURE!=="1")rmSync(dir,{recursive:true,force:true});
}
