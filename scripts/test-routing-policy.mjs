import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { randomUUID, createHash } from "node:crypto";
import { createInterface } from "node:readline";
import { once } from "node:events";
import http from "node:http";
import { DatabaseSync } from "node:sqlite";
import { cleanFixtureKeychain, fixtureClaudeCredential } from "./fixtures/service-keychain.mjs";

const dir=realpathSync(mkdtempSync(path.join(tmpdir(),"local-agent-policy-")));const home=path.join(dir,"home");
const systemKeychain = process.argv.includes("--system-keychain");
if (systemKeychain) {
  assert.equal(process.platform, "darwin");
  assert(process.env.LOCAL_AGENT_TEST_SERVICE_BINARY, "Pass the signed macOS app's service binary");
}
const agent = systemKeychain ? "claude" : "opencode";
const data = path.join(dir, "app");
const config = path.join(home, systemKeychain ? ".claude/settings.json" : ".config/opencode/opencode.json");
mkdirSync(path.dirname(config),{recursive:true});writeFileSync(config,"{}\n",{mode:0o600});
const sessions = new Map();
const wireSession = session => {
  if (!systemKeychain || !session.startsWith("ses_")) return session;
  if (!sessions.has(session)) sessions.set(session, randomUUID());
  return sessions.get(session);
};
const sessionKey = session => agent + ":" + wireSession(session);
const servers=[],faults=[0,0,0],seen=[];let child,stderr="",holdIndex=-1,arrive,release,retryAfter="1";
let holdRecoveryStream=false, releaseRecoveryStream;
const token=randomUUID();const pause=ms=>new Promise(r=>setTimeout(r,ms));
try {
  for(let i=0;i<3;i++) {
    const server=http.createServer(async(req,res)=>{
      let text="";for await(const chunk of req)text+=chunk;const b=JSON.parse(text);seen.push({i,model:b.model,assistantNotices:JSON.stringify(b.messages.filter(m=>m.role==="assistant")).includes("【路由提示】"),classifier:JSON.stringify(b.system??"").includes("Classify the current request")});
      if(holdIndex===i){holdIndex=-1;arrive();await new Promise(r=>{release=r;});}
      const event=(type,value)=>`event: ${type}\ndata: ${JSON.stringify({type,...value})}\n\n`;
      const startEvent=event("message_start",{message:{id:randomUUID(),type:"message",role:"assistant",model:b.model,content:[],usage:{input_tokens:5,output_tokens:0}}});
      if(i===0 && b.stream && holdRecoveryStream) {
        holdRecoveryStream=false;
        res.writeHead(200,{"Content-Type":"text/event-stream"});
        res.write(startEvent+event("content_block_start",{index:0,content_block:{type:"text",text:""}})+event("content_block_delta",{index:0,delta:{type:"text_delta",text:"PROBE_PREFIX"}}));
        const failure=await new Promise(resolve=>{releaseRecoveryStream=resolve;});
        if(failure) res.end(event("error",{error:{type:"api_error",message:"fixture probe failed after first output"}}));
        else res.end(event("content_block_stop",{index:0})+event("message_delta",{delta:{stop_reason:"end_turn"},usage:{output_tokens:2}})+event("message_stop",{}));
        return;
      }
      if(faults[i]==="truncated-success") {
        res.writeHead(200,{"Content-Type":b.stream?"text/event-stream":"application/json","Content-Length":4096});
        res.write(b.stream?startEvent:'{"type":"message","content":[');
        setTimeout(()=>res.destroy(),25);return;
      }
      if(faults[i]==="quota") {res.writeHead(429,{"Content-Type":"application/json"});res.end(JSON.stringify({error:{type:"insufficient_quota",code:"insufficient_quota",message:"fixture quota exhausted"}}));return;}
      if(["sse-auth","sse-quota","sse-context"].includes(faults[i])) {res.writeHead(200,{"Content-Type":"text/event-stream"});const error=faults[i]==="sse-auth"?{type:"authentication_error",message:"fixture auth"}:faults[i]==="sse-quota"?{type:"rate_limit_error",code:"insufficient_quota",message:"fixture quota"}:{type:"invalid_request_error",code:"context_length_exceeded",message:"Input exceeds the context window"};res.write(startEvent);const frame=event("error",{error});res.write(frame.slice(0,17));setTimeout(()=>res.end(frame.slice(17)),10);return;}
      if(faults[i]==="sse-rate") {res.writeHead(200,{"Content-Type":"text/event-stream"});res.end(startEvent+event("error",{error:{type:"rate_limit_error",message:"fixture throttle"}}));return;}
      if(["sse-partial","sse-partial-same-chunk","sse-early-eof"].includes(faults[i])) {
        res.writeHead(200,{"Content-Type":"text/event-stream"});
        const prefix=startEvent+event("content_block_start",{index:0,content_block:{type:"text",text:""}})+event("content_block_delta",{index:0,delta:{type:"text_delta",text:"PARTIAL_A"}});
        const failure=event("error",{error:{type:"api_error",message:"fixture stream stop"}});
        if(faults[i]==="sse-early-eof") {
          res.end(prefix+event("content_block_stop",{index:0})+event("message_delta",{delta:{stop_reason:"end_turn"},usage:{output_tokens:2}}));
        } else if(faults[i]==="sse-partial-same-chunk") res.end(prefix+failure);
        else { res.write(prefix); setTimeout(()=>res.end(failure),30); }
        return;
      }
      if(faults[i] && faults[i]!=="stream-as-json"){res.writeHead(faults[i],{"Content-Type":"application/json","Retry-After":retryAfter});res.end(JSON.stringify({error:{type:faults[i]===429?"rate_limit_error":"api_error",message:"fixture upstream rejection"}}));return;}
      const message={id:randomUUID(),type:"message",role:"assistant",model:b.model,content:[{type:"text",text:"OK"}],stop_reason:"end_turn",usage:{input_tokens:5,output_tokens:2}};
      if(b.stream && faults[i]!=="stream-as-json"){res.writeHead(200,{"Content-Type":"text/event-stream"});res.end(startEvent+event("content_block_start",{index:0,content_block:{type:"text",text:""}})+event("content_block_delta",{index:0,delta:{type:"text_delta",text:"OK"}})+event("content_block_stop",{index:0})+event("message_delta",{delta:{stop_reason:"end_turn"},usage:{output_tokens:2}})+event("message_stop",{}));return;}
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
  const installed=JSON.parse(readFileSync(config));
  const local=systemKeychain
    ? {baseURL:installed.env.ANTHROPIC_BASE_URL+"/v1",apiKey:fixtureClaudeCredential(installed,data)}
    : installed.provider[installed.model.split("/")[0]].options;
  const configHash=createHash("sha256").update(readFileSync(config)).digest("hex");
  const headersFor=session=>({"x-api-key":local.apiKey,[systemKeychain?"session_id":"x-opencode-session"]:wireSession(session),"Content-Type":"application/json"});
  const user=text=>({role:"user",content:text});
  const send=async(messages,session="ses_"+randomUUID().replaceAll("-", ""),model="local-agent-default",expected=200,extra={})=>{const r=await fetch(local.baseURL+"/messages",{method:"POST",headers:headersFor(session),body:JSON.stringify({model,max_tokens:100,messages,...extra}),signal:AbortSignal.timeout(20000)});const raw=await r.text();assert.equal(r.status,expected,raw);return JSON.parse(raw);};
  const publish=async(policy)=>{draft={...draft,revision:service.revision,routingPolicy:{...draft.routingPolicy,...policy}};service=await api("/api/model-services",draft,"PUT");};
  const noticeText=response=>response.content.filter(p=>p.type==="text").map(p=>p.text).join("");
  seen.length=0;faults[0]=500;const first=[user("first")];const sid="ses_"+randomUUID().replaceAll("-", "");const fallback=await send(first,sid);assert.equal(fallback.model,"m1");assert.equal(noticeText(fallback).split("【路由提示】").length-1,1);assert(noticeText(fallback).includes("Policy 1 / m1"));assert.equal(fallback.usage.output_tokens,2);assert.deepEqual(seen.map(r=>r.i),[0,1]);
  faults[0]=0;
  const continuation=[...first,{role:"assistant",content:[...fallback.content,{type:"tool_use",id:"call1",name:"read",input:{}}]},{role:"user",content:[{type:"tool_result",tool_use_id:"call1",content:"fixture"}]}];
  const pinned=await send(continuation,sid);assert.equal(pinned.model,"m1");assert(!noticeText(pinned).includes("路由提示"));assert.equal(seen.at(-1).assistantNotices,false);
  await pause(1100);const recovered=await send([...first,{role:"assistant",content:"OK"},user("next")],sid);assert.equal(recovered.model,"m0");assert(noticeText(recovered).includes("已恢复使用 Policy 0 / m0"));
  faults[0]=429;seen.length=0;assert.equal((await send([user("429")])).model,"m1");assert.deepEqual(seen.map(r=>r.i),[0,1]);
  seen.length=0;assert.equal((await send([user("skip cooling")])).model,"m1");assert.deepEqual(seen.map(r=>r.i),[1]);
  faults[0]=0;await pause(1100);
  const repairSession="ses_repairBeforeDispatch";
  const repairTurn=[user("Repair invalid request before dispatch")];
  seen.length=0;
  await send(repairTurn,repairSession,"local-agent-default",400,{max_tokens:0});
  assert.deepEqual(seen,[]);
  await publish({fallbackOnFailure:false});
  faults[0]=500;
  assert.equal((await send(repairTurn,repairSession)).model,"m1");
  assert.deepEqual(seen.map(r=>r.i),[0,1],"A corrected request must enter its original published fallback plan");
  await publish({fallbackOnFailure:true});
  faults[0]=0;await pause(1100);
  faults[1]=500;const specified=`local-agent-model-${models[1].id}`;const ssid="ses_"+randomUUID().replaceAll("-", "");const ss=[user("prefer B")];assert.equal((await send(ss,ssid,specified)).model,"m0");
  faults[1]=0;await pause(1100);assert.equal((await send([...ss,{role:"assistant",content:"OK"},user("prefer B remains")],ssid,specified)).model,"m1");
  await publish({limits:[{modelId:models[0].id,maxConcurrent:1,requestsPerMinute:0}]});
  holdIndex=0;const arriving=new Promise(r=>{arrive=r;});const held=send([user("held")],"ses_heldSession");await arriving;
  assert.equal((await send([user("another session")])).model,"m1");
  await send([user("held")],"ses_heldSession","local-agent-default",409);release();assert.equal((await held).model,"m0");
  await publish({limits:[{modelId:models[0].id,maxConcurrent:0,requestsPerMinute:1}]});seen.length=0;assert.equal((await send([user("RPM full")])).model,"m1");assert.deepEqual(seen.map(r=>r.i),[1]);
  await publish({limits:[]});
  faults[0]=500;
  for(const extra of [{response_format:{type:"json_object"}},{output_config:{format:{type:"json_schema",schema:{type:"object"}}}},{tools:[{name:"StructuredOutput",input_schema:{type:"object"}}]}]){
    const response=await send([user("contract output")],undefined,"local-agent-default",200,extra);assert.equal(response.model,"m1");assert.equal(noticeText(response),"OK");
  }
  assert.equal(noticeText(await send([user("只返回 JSON")])) ,"OK");faults[0]=0;await pause(1100);
  faults[0]=400;seen.length=0;await send([user("invalid")],"ses_"+randomUUID().replaceAll("-", ""),"local-agent-default",400);assert.deepEqual(seen.map(r=>r.i),[0]);faults[0]=0;
  seen.length=0;assert.equal((await send([user("large text ".repeat(12000))])).model,"m0");assert.equal(seen.length,1);
  seen.length=0;const overflow=await send([user("large text ".repeat(50000))],"ses_"+randomUUID().replaceAll("-", ""),"local-agent-default",400);assert.equal(overflow.error.code,"context_length_exceeded");assert.equal(seen.length,0);
  const observations=await api("/api/requests");const rows=observations.items??observations;
  assert(rows.some(r=>r.routing?.service?.preferredModelId===models[1].id && r.routing.service.modelId===models[0].id && r.routing.service.attempt===2));
  assert.equal(createHash("sha256").update(readFileSync(config)).digest("hex"),configHash);
  assert((await api("/api/model-services")).attachments.filter(a=>a.attached).every(a=>!a.directorySyncRequired));
  faults[0]="quota";seen.length=0;await send([user("quota is not throttling")],undefined,"local-agent-default",429);assert.deepEqual(seen.map(r=>r.i),[0]);faults[0]=0;
  faults[0]=500;seen.length=0;await send([user("untrusted session")],"untrusted","local-agent-default",500);assert.deepEqual(seen.map(r=>r.i),[0]);faults[0]=0;
  seen.length=0;assert.equal((await send([user("pinned failure shares cooldown")])).model,"m1");assert.deepEqual(seen.map(r=>r.i),[1]);await pause(1100);
  faults[0]=500;holdIndex=0;const changing=new Promise(r=>{arrive=r;});const oldPolicyRequest=send([user("immutable policy")]);await Promise.race([changing,oldPolicyRequest.then(()=>assert.fail("The cooled target must reach its recovery probe"))]);
  await publish({order:[models[0].id,models[2].id,models[1].id]});release();assert.equal((await oldPolicyRequest).model,"m1");faults[0]=0;await publish({order:models.map(m=>m.id)});await pause(1100);
  const streaming=async(session,messages=[user("stream")])=>{const r=await fetch(local.baseURL+"/messages",{method:"POST",headers:headersFor(session),body:JSON.stringify({model:"local-agent-default",max_tokens:100,stream:true,messages})});assert.equal(r.status,200);return r.text();};
  const pinnedDuringProbe="ses_pinnedDuringProbe";
  const pinnedProbeInput=user("Existing session before recovery");
  assert.equal((await send([pinnedProbeInput],pinnedDuringProbe)).model,"m0");
  const pinnedProbeTurn=[
    pinnedProbeInput,
    {role:"assistant",content:[{type:"tool_use",id:"call-during-probe",name:"read",input:{}}]},
    {role:"user",content:[{type:"tool_result",tool_use_id:"call-during-probe",content:"fixture"}]},
  ];
  const assertRecoveryPhase=async(state,reason)=>{
    const catalog=await api("/api/model-services");
    const status=catalog.routingHealth.find(row=>row.serviceId===service.id && row.modelId===models[0].id).status;
    assert.equal(status.state,state,"Model service cooldown phase");
    assert.equal(status.reason,reason);
    const preview=await api("/api/model-services/preview",{
      storeId:catalog.writeProtocol.storeId,entry:{kind:"default"},draft:{...draft,revision:service.revision},
    });
    const account=preview.models.find(model=>model.id===models[0].id).accounts[0];
    assert.equal(account.cooldownKnown,true);
    assert.deepEqual(account.cooldown,state==="cooling"?{untilMs:status.untilMs,reason}:null);
    assert.deepEqual(account.recovery??null,["waitingForProbe","probing"].includes(state)?{state,untilMs:status.untilMs,reason}:null);
    return status.untilMs;
  };
  const waitForProbeCooldown=async(reason)=>{
    const deadline=Date.now()+2000;
    for (;;) {
      const catalog=await api("/api/model-services");
      const status=catalog.routingHealth.find(row=>row.serviceId===service.id && row.modelId===models[0].id).status;
      if (status.state==="cooling" || Date.now()>=deadline) break;
      await pause(25);
    }
    return assertRecoveryPhase("cooling",reason);
  };
  for(const [initialStatus, lateFailure] of [[429,false],[503,true]]) {
    faults[0]=initialStatus;
    assert.equal((await send([user("Trigger probe cooldown")])).model,"m1");
    const reason=initialStatus===429?"upstream_rate_limit":"upstream_failure";
    const until=await assertRecoveryPhase("cooling",reason);
    faults[0]=0;await pause(Math.max(0,until-Date.now())+50);
    await assertRecoveryPhase("waitingForProbe",reason);
    holdRecoveryStream=true;seen.length=0;
    const recovery=await fetch(local.baseURL+"/messages",{
      method:"POST",headers:headersFor(`ses_heldRecovery${initialStatus}`),
      body:JSON.stringify({model:"local-agent-default",max_tokens:100,stream:true,messages:[user("Hold recovery after first output")]}),
      signal:AbortSignal.timeout(20000),
    });
    assert.equal(recovery.status,200);
    const reader=recovery.body.getReader();
    const decoder=new TextDecoder();
    let wire="";
    try {
      while(!wire.includes("PROBE_PREFIX")) {
        const {done,value}=await reader.read();
        assert(!done,"The probe finished before its first output");
        wire+=decoder.decode(value,{stream:true});
      }
      assert.deepEqual(seen.map(row=>row.i),[0]);
      await assertRecoveryPhase("probing",reason);
      assert.equal((await send([user("New turn while recovery is still streaming")])).model,"m1");
      await send(pinnedProbeTurn,pinnedDuringProbe,"local-agent-default",429);
      assert.deepEqual(seen.map(row=>row.i),[0,1]);
    } finally {
      releaseRecoveryStream?.(lateFailure);
      releaseRecoveryStream=undefined;
    }
    for(;;) {
      const {done,value}=await reader.read();
      if(done) break;
      wire+=decoder.decode(value,{stream:true});
    }
    assert(wire.includes(lateFailure?'"type":"error"':'"type":"message_stop"'));
    await assertRecoveryPhase(lateFailure?"cooling":"ready",lateFailure?"stream_interrupted_after_commit":undefined);
    seen.length=0;
    assert.equal((await send([user("New turn after the probe result")])).model,lateFailure?"m1":"m0");
    assert.deepEqual(seen.map(row=>row.i),[lateFailure?1:0]);
    if(lateFailure) await pause(1100);
  }
  console.log(JSON.stringify({singleRecoveryProbeUntilStreamTerminal:true,lateProbeFailureRenewsCooldown:true}));
  const restartCooldown = draft.routingPolicy.cooldownSeconds;
  const originalDeadline = draft.routingPolicy.deadlineSeconds;
  await publish({ cooldownSeconds: 3, deadlineSeconds: 5 });
  const interruptedProbe = async (initialStatus, interrupt, expectedSends=[0]) => {
    faults[0]=initialStatus;
    assert.equal((await send([user("Establish cooldown before interrupted recovery")])).model,"m1");
    const reason=initialStatus===429?"upstream_rate_limit":"upstream_failure";
    const originalUntil=await assertRecoveryPhase("cooling",reason);
    faults[0]=0;
    await pause(Math.max(0,originalUntil-Date.now())+50);
    seen.length=0;
    await interrupt();
    assert.deepEqual(seen.map(row=>row.i),expectedSends,"Unexpected sends during the interrupted probe");
    seen.length=0;
    assert.equal((await send([user("Immediate new turn after interrupted recovery")])).model,"m1");
    assert.deepEqual(seen.map(row=>row.i),[1]);
    const renewed=await waitForProbeCooldown(reason);
    assert(renewed>originalUntil);
    const db=new DatabaseSync(path.join(dir,"app/local-agent.sqlite3"),{readOnly:true});
    try {
      assert.equal(db.prepare("SELECT count(*) AS count FROM service_cooldown_probe_sends").get().count,0);
    } finally { db.close(); }
    await pause(Math.max(0,renewed-Date.now())+50);
    await assertRecoveryPhase("waitingForProbe",reason);
    seen.length=0;
    assert.equal((await send([user("Recover after the renewed cooldown")])).model,"m0");
    assert.deepEqual(seen.map(row=>row.i),[0]);
    await assertRecoveryPhase("ready",undefined);
  };
  await interruptedProbe(429,async()=>{
    const session="ses_probeTruncatedBeforeOutput";
    const turn=[user("Truncate the first recovery response")];
    faults[0]="truncated-success";
    try { await send(turn,session,"local-agent-default",502); }
    finally { faults[0]=0; }
    await send(turn,session,"local-agent-default",409);
  });
  await interruptedProbe(503,async()=>{
    holdIndex=0;
    const entered=new Promise(resolve=>{arrive=resolve;});
    const waiting=send(pinnedProbeTurn,pinnedDuringProbe,"local-agent-default",504);
    try {
      await Promise.race([entered,waiting.then(()=>assert.fail("The pinned probe must reach the upstream"))]);
      await assertRecoveryPhase("probing","upstream_failure");
      await waiting;
    } finally { release?.(); release=undefined; }
  });
  try {
    await interruptedProbe(429,async()=>{
      holdRecoveryStream=true;
      const response=await fetch(local.baseURL+"/messages",{
        method:"POST",headers:headersFor("ses_cancelledProbeAfterOutput"),
        body:JSON.stringify({model:"local-agent-default",max_tokens:100,stream:true,messages:[user("Cancel after the probe starts streaming")]}),
        signal:AbortSignal.timeout(20000),
      });
      assert.equal(response.status,200);
      const reader=response.body.getReader();
      const decoder=new TextDecoder();
      let wire="";
      while (!wire.includes("PROBE_PREFIX")) {
        const {done,value}=await reader.read();
        assert(!done,"The probe must be streaming before cancellation");
        wire+=decoder.decode(value,{stream:true});
      }
      await assertRecoveryPhase("probing","upstream_rate_limit");
      await reader.cancel();
    });
  } finally { releaseRecoveryStream?.(false); releaseRecoveryStream=undefined; }
  await interruptedProbe(429,async()=>{
    await publish({
      intentClassifier:{modelId:models[0].id},
      rules:[{id:randomUUID(),name:"Classifier timeout",agent,intent:"排查错误",modelId:models[1].id}],
    });
    holdIndex=0;
    const entered=new Promise(resolve=>{arrive=resolve;});
    const waiting=send([user("Hold the classifier until its probe times out")]);
    try {
      await Promise.race([entered,waiting.then(()=>assert.fail("The classifier probe must reach the upstream"))]);
      await assertRecoveryPhase("probing","upstream_rate_limit");
      assert.equal((await waiting).model,"m1");
      assert.deepEqual(seen.map(({i,classifier})=>[i,classifier]),[[0,true],[1,false]]);
    } finally { release?.(); release=undefined; }
    await publish({intentClassifier:null,rules:[]});
  },[0,1]);
  console.log(JSON.stringify({unconfirmedRecoveryRenewsCooldown:true,pinnedProbeDeadlineRenewsCooldown:true,cancelledRecoveryRenewsCooldown:true,classifierTimeoutRenewsCooldown:true}));
  await publish({ deadlineSeconds: originalDeadline });
  if (systemKeychain) {
    for (const [status, session, messages] of [
      [429, "ses_restartRecovery", [user("Recovery stream interrupted by restart")]],
      [503, pinnedDuringProbe, pinnedProbeTurn],
    ]) {
      faults[0]=status;
      assert.equal((await send([user("Trigger a cooldown before restarting a probe")])).model,"m1");
      const reason=status===429?"upstream_rate_limit":"upstream_failure";
      const until=await assertRecoveryPhase("cooling",reason);
      faults[0]=0;await pause(Math.max(0,until-Date.now())+50);
      holdRecoveryStream=true;seen.length=0;
      const recovering=await fetch(local.baseURL+"/messages",{
        method:"POST",headers:headersFor(session),
        body:JSON.stringify({model:"local-agent-default",max_tokens:100,stream:true,messages}),
        signal:AbortSignal.timeout(20000),
      });
      assert.equal(recovering.status,200);
      const reader=recovering.body.getReader();
      const decoder=new TextDecoder();
      try {
        let wire="";
        while(!wire.includes("PROBE_PREFIX")) {
          const {done,value}=await reader.read();
          assert(!done,"Recovery must remain in flight at restart");
          wire+=decoder.decode(value,{stream:true});
        }
        await assertRecoveryPhase("probing",reason);
        assert.deepEqual(seen.map(row=>row.i),[0]);
        const exited=once(child,"exit");
        child.kill("SIGKILL");
        await exited;
      } finally {
        releaseRecoveryStream?.(false);
        releaseRecoveryStream=undefined;
        await reader.cancel().catch(()=>{});
      }
      ready=await start();seen.length=0;
      const restartedUntil=await assertRecoveryPhase("cooling",reason);
      assert(restartedUntil>until,"The restart must preserve an interrupted probe as cooling");
      await send(pinnedProbeTurn,pinnedDuringProbe,"local-agent-default",429);
      assert.deepEqual(seen,[],"Pinned tool continuations must wait for restart cooldown");
      assert.equal((await send([user("New turn while the interrupted probe is cooling")])).model,"m1");
      assert.deepEqual(seen.map(row=>row.i),[1]);
      await pause(Math.max(0,restartedUntil-Date.now())+50);
      await assertRecoveryPhase("waitingForProbe",reason);
      assert.equal((await send([user("New probe after the restart cooldown")])).model,"m0");
      await assertRecoveryPhase("ready",undefined);
    }
    faults[0]=429;
    assert.equal((await send([user("Trigger cooldown before a classifier probe")])).model,"m1");
    const beforeClassifier=await assertRecoveryPhase("cooling","upstream_rate_limit");
    faults[0]=0;await pause(Math.max(0,beforeClassifier-Date.now())+50);
    await publish({
      intentClassifier:{modelId:models[0].id},
      rules:[{id:randomUUID(),name:"Classifier recovery",agent,intent:"排查错误",modelId:models[1].id}],
    });
    holdIndex=0;seen.length=0;release=undefined;
    const classifierArrived=new Promise(resolve=>{arrive=resolve;});
    const classifierRequest=send([user("Hold the classifier during recovery")]).catch(error=>error);
    try {
      await Promise.race([classifierArrived,classifierRequest.then(()=>assert.fail("Classifier recovery did not reach the upstream"))]);
      assert.deepEqual(seen.map(({i,classifier})=>[i,classifier]),[[0,true]]);
      await assertRecoveryPhase("probing","upstream_rate_limit");
      const exited=once(child,"exit");
      child.kill("SIGKILL");
      await exited;
    } finally {
      release?.();
      await classifierRequest;
    }
    ready=await start();seen.length=0;
    const restartedClassifier=await assertRecoveryPhase("cooling","upstream_rate_limit");
    assert.equal((await send([user("New turn while classifier recovery is cooling")])).model,"m1");
    assert.deepEqual(seen.map(({i,classifier})=>[i,classifier]),[[1,false]]);
    await publish({intentClassifier:null,rules:[]});
    await pause(Math.max(0,restartedClassifier-Date.now())+50);
    assert.equal((await send([user("New probe after interrupted classification")])).model,"m0");
    await assertRecoveryPhase("ready",undefined);
    console.log(JSON.stringify({interruptedRecoveryCoolsAcrossRestart:true,pinnedRecoveryAlsoCoolsAcrossRestart:true,classifierRecoveryCoolsAcrossRestart:true}));
  }

  faults[0]=429;
  assert.equal((await send([user("Trigger cooldown before a failed send checkpoint")])).model,"m1");
  const unsentProbe=await assertRecoveryPhase("cooling","upstream_rate_limit");
  faults[0]=0;await pause(Math.max(0,unsentProbe-Date.now())+50);seen.length=0;
  const probeDb=new DatabaseSync(path.join(dir,"app/local-agent.sqlite3"));
  try {
    probeDb.exec("CREATE TRIGGER fixture_probe_checkpoint_failure BEFORE INSERT ON service_cooldown_probe_sends BEGIN SELECT RAISE(FAIL,'fixture checkpoint unavailable'); END");
    const session="ses_probeCheckpointUnavailable";
    await send([user("Fail the probe send checkpoint")],session,"local-agent-default",503);
    assert.deepEqual(seen,[],"An unrecorded probe must not reach the upstream");
    assert.deepEqual(probeDb.prepare("SELECT outcome FROM service_route_attempts WHERE session=?").all(sessionKey(session)).map(row=>row.outcome),["not_sent"]);
    assert.equal(probeDb.prepare("SELECT count(*) AS count FROM service_cooldown_probe_sends").get().count,0);
  } finally {
    probeDb.exec("DROP TRIGGER IF EXISTS fixture_probe_checkpoint_failure");
    probeDb.close();
  }
  assert.equal((await send([user("Probe after send checkpoint is writable")])).model,"m0");
  if (systemKeychain) {
    const finishedExit=once(child,"exit");
    child.kill("SIGTERM");await finishedExit;ready=await start();
    await assertRecoveryPhase("ready",undefined);
  }
  await publish({ cooldownSeconds: restartCooldown });
  console.log(JSON.stringify({unrecordedProbeDoesNotSend:true}));
  faults[0]="sse-partial";seen.length=0;assert((await streaming("ses_streamPartial")).includes("PARTIAL_A"));assert.deepEqual(seen.map(r=>r.i),[0]);faults[0]=0;await pause(1100);
  const originalCooldown = draft.routingPolicy.cooldownSeconds;
  await publish({ cooldownSeconds: 3 });
  faults[0]="sse-partial-same-chunk";seen.length=0;
  const coalesced=await streaming("ses_streamPartialCoalesced");
  assert(coalesced.includes("PARTIAL_A") && coalesced.includes('"type":"error"'),coalesced);
  assert.deepEqual(seen.map(r=>r.i),[0]);
  faults[0]=0;seen.length=0;
  assert.equal((await send([user("New turn after the interrupted stream")])).model,"m1");
  assert.deepEqual(seen.map(r=>r.i),[1]);
  await pause(3100);
  faults[0]="sse-early-eof";seen.length=0;
  const truncated=await streaming("ses_streamPartialEarlyEof");
  assert(truncated.includes("PARTIAL_A") && !truncated.includes('"type":"message_stop"'),truncated);
  assert.deepEqual(seen.map(r=>r.i),[0]);
  faults[0]=0;seen.length=0;
  assert.equal((await send([user("New turn after the missing terminal event")])).model,"m1");
  assert.deepEqual(seen.map(r=>r.i),[1]);
  await pause(3100);
  const pinnedSession="ses_streamPinnedEarlyEof";
  const pinnedInput=user("Start the pinned tool fixture");
  assert.equal((await send([pinnedInput],pinnedSession)).model,"m0");
  const pinnedMessages=[
    pinnedInput,
    {role:"assistant",content:[{type:"tool_use",id:"call-pinned",name:"read",input:{}}]},
    {role:"user",content:[{type:"tool_result",tool_use_id:"call-pinned",content:"fixture"}]},
  ];
  faults[0]="sse-early-eof";seen.length=0;
  const pinnedTruncated=await streaming(pinnedSession,pinnedMessages);
  assert(pinnedTruncated.includes("PARTIAL_A") && !pinnedTruncated.includes('"type":"message_stop"'),pinnedTruncated);
  assert.deepEqual(seen.map(r=>r.i),[0]);
  faults[0]=0;seen.length=0;
  assert.equal((await send([user("New turn after the interrupted tool continuation")])).model,"m1");
  assert.deepEqual(seen.map(r=>r.i),[1]);
  await pause(3100);
  await publish({ cooldownSeconds: originalCooldown });
  console.log(JSON.stringify({coalescedOutputKeepsOriginalModel:true,coalescedErrorCoolsPrimary:true,missingTerminalCoolsPrimary:true,pinnedStreamMissingTerminalCoolsPrimary:true}));
  for(const [fault,status] of [["sse-auth",401],["sse-quota",429],["sse-context",400]]){faults[0]=fault;seen.length=0;const r=await fetch(local.baseURL+"/messages",{method:"POST",headers:headersFor("ses_"+randomUUID().replaceAll("-","")),body:JSON.stringify({model:"local-agent-default",max_tokens:100,stream:true,messages:[user("permanent stream rejection")]})});assert.equal(r.status,status,await r.text());assert.deepEqual(seen.map(r=>r.i),[0]);}
  faults[0]=500;seen.length=0;await send([user("mark failure cooldown")]);await publish({fallbackOnFailure:false});seen.length=0;await send([user("failure cooldown respects disabled failure fallback")],undefined,"local-agent-default",429);assert.equal(seen.length,0);await publish({fallbackOnFailure:true});faults[0]=0;await pause(1100);
  seen.length=0;
  assert.equal((await send([user("Complete recovery before testing ordinary interrupted replies")])).model,"m0");
  assert.deepEqual(seen.map(row=>row.i),[0]);
  const database = new DatabaseSync(path.join(dir,"app/local-agent.sqlite3"));
  const interruptedTurns = [
    { stream: false, fault: "truncated-success", prompt: "Interrupted JSON response" },
    { stream: true, fault: "truncated-success", prompt: "Interrupted event stream" },
    { stream: true, fault: "stream-as-json", prompt: "Event stream answered with JSON" },
  ].map(({ stream, fault, prompt }) => ({
    session: "ses_"+randomUUID().replaceAll("-", ""),
    messages: [user(prompt)],
    stream,
    fault,
  }));
  const replyOutcomes = (db, session) => db.prepare(
    "SELECT a.outcome,d.state FROM service_route_attempts a JOIN service_route_dispatch d USING(binding,session,turn) WHERE d.session=? ORDER BY a.id",
  ).all(sessionKey(session)).map(a=>[a.outcome,a.state]);
  try {
    for (const { session, messages, stream, fault } of interruptedTurns) {
      faults[0]=fault;seen.length=0;
      const result=await send(messages,session,"local-agent-default",502,{stream});
      assert.match(result.error.message,/上游结果未确认/);
      assert.deepEqual(seen.map(r=>r.i),[0]);
      assert.deepEqual(replyOutcomes(database,session),[["outcome_unknown","pending"]]);
      await send(messages,session,"local-agent-default",409,{stream});
      assert.deepEqual(seen.map(r=>r.i),[0]);
      faults[0]=0;
      const compacted = [user("Compacted history of the interrupted turn")];
      const changedToolHistory = [
        ...compacted,
        {role:"assistant",content:[{type:"tool_use",id:"call-compacted",name:"read",input:{}}]},
        {role:"user",content:[{type:"tool_result",tool_use_id:"call-compacted",content:"fixture"}]},
      ];
      for (const changed of [compacted, changedToolHistory]) {
        const blocked=await send(changed,session,"local-agent-default",409,{stream});
        assert.match(blocked.error.message,/原请求.*尚未确认/);
      }
      assert.deepEqual(seen.map(r=>r.i),[0]);
      assert.equal(database.prepare("SELECT count(*) AS count FROM gateway_route_leases WHERE session=?").get(sessionKey(session)).count,1);
      const query = new URLSearchParams({agent,session:sessionKey(session)});
      let blockedDetails = [];
      for (let tries=0;tries<20 && blockedDetails.length<3;tries++) {
        const dashboard=await api(`/api/request-dashboard?${query}`);
        const details=await Promise.all(dashboard.items.filter(row=>row.attempts.length===0).map(row=>api(`/api/request-detail?id=${encodeURIComponent(row.id)}`)));
        blockedDetails=details.filter(detail=>detail.routeTrace?.httpStatus===409);
        if (blockedDetails.length<3) await pause(50);
      }
      assert.equal(blockedDetails.length,3);
      for (const {request,routeTrace} of blockedDetails) {
        assert.equal(routeTrace.sentUpstream,false);
        assert.equal(request.attempts.length,0);
        assert(routeTrace.decisions.some(decision=>decision.reason==="outcome-unknown"));
      }
    }
  } finally {
    faults[0]=0;
  }
  const unsentSession = "ses_"+randomUUID().replaceAll("-", "");
  const unsentTurn = [user("prepared attempt that cannot record the send boundary")];
  const localRetrySession = "ses_"+randomUUID().replaceAll("-", "");
  try {
    database.exec(`CREATE TRIGGER fixture_send_boundary_failure BEFORE UPDATE ON service_route_attempts
      WHEN NEW.session IN ('${sessionKey(unsentSession)}','${sessionKey(localRetrySession)}')
      BEGIN SELECT RAISE(FAIL,'fixture dispatch write interrupted'); END`);
    seen.length=0;
    for (const session of [unsentSession,localRetrySession]) {
      await send(unsentTurn,session,"local-agent-default",503);
      assert.deepEqual(replyOutcomes(database,session),[["preparing","pending"]]);
    }
    assert.equal(seen.length,0);
  } finally {
    database.exec("DROP TRIGGER IF EXISTS fixture_send_boundary_failure");
  }
  assert.equal((await send(unsentTurn,localRetrySession)).model,"m0");
  assert.deepEqual(seen.map(r=>r.i),[0]);
  assert.deepEqual(replyOutcomes(database,localRetrySession),[["not_sent","committed"],["committed","committed"]]);
  const interruptedFallback = "ses_"+randomUUID().replaceAll("-", "");
  const fallbackTurn = [user("recover the unstarted backup after a confirmed throttle")];
  await publish({maxAttempts:2});
  try {
    database.exec(`CREATE TRIGGER fixture_backup_before_send BEFORE UPDATE ON service_route_attempts
      WHEN NEW.session='${sessionKey(interruptedFallback)}' AND NEW.model='${models[1].id}'
      BEGIN SELECT RAISE(FAIL,'fixture backup preparation interrupted'); END`);
    faults[0]=429;seen.length=0;
    await send(fallbackTurn,interruptedFallback,"local-agent-default",503);
    assert.deepEqual(seen.map(r=>r.i),[0]);
    assert.deepEqual(replyOutcomes(database,interruptedFallback),[["rate_limited","pending"],["preparing","pending"]]);
  } finally {
    database.exec("DROP TRIGGER IF EXISTS fixture_backup_before_send");
    faults[0]=0;
  }
  await publish({fallbackOnRateLimit:false,maxAttempts:3});
  await pause(1100);
  faults[1]=500;seen.length=0;
  await send(fallbackTurn,interruptedFallback,"local-agent-default",500);
  assert.deepEqual(seen.map(r=>r.i),[1],"Recovery must consume the original attempt and skip the old primary");
  faults[1]=0;
  await publish({fallbackOnRateLimit:true,maxAttempts:3});
  await pause(1100);
  const unknownBackup = "ses_"+randomUUID().replaceAll("-", "");
  faults[0]=429; faults[1]="truncated-success"; seen.length=0;
  await send(fallbackTurn,unknownBackup,"local-agent-default",502);
  assert.deepEqual(seen.map(r=>r.i),[0,1]);
  assert.deepEqual(replyOutcomes(database,unknownBackup),[["rate_limited","pending"],["outcome_unknown","pending"]]);
  await send(fallbackTurn,unknownBackup,"local-agent-default",409);
  assert.deepEqual(seen.map(r=>r.i),[0,1]);
  faults[0]=0; faults[1]=0;
  await pause(1100);
  const uncertainSession = "ses_"+randomUUID().replaceAll("-", "");
  const uncertainTurn = [user("accounting failure must not repeat the upstream call")];
  const previousOwners = new Set(database.prepare("SELECT owner FROM service_route_dispatch").all().map(row=>row.owner));
  try {
    database.exec("CREATE TRIGGER fixture_attempt_result_failure BEFORE UPDATE ON service_route_attempts WHEN NEW.status IS NOT NULL BEGIN SELECT RAISE(FAIL,'fixture accounting unavailable'); END");
    faults[0]=500;seen.length=0;
    await send(uncertainTurn,uncertainSession,"local-agent-default",503);
    assert.deepEqual(seen.map(r=>r.i),[0]);
    const created = database.prepare("SELECT owner,state FROM service_route_dispatch").all().filter(row=>!previousOwners.has(row.owner));
    assert.equal(created.length,1);
    assert.equal(created[0].state,"pending");
    await send(uncertainTurn,uncertainSession,"local-agent-default",409);
    assert.deepEqual(seen.map(r=>r.i),[0]);
  } finally {
    database.exec("DROP TRIGGER IF EXISTS fixture_attempt_result_failure");
    database.close();faults[0]=0;
  }
  const accountingCooldown=await waitForProbeCooldown("upstream_rate_limit");
  await pause(Math.max(0,accountingCooldown-Date.now())+50);
  // Use a window long enough to measure persistence independently of startup time.
  await publish({cooldownSeconds:10});
  faults[0]="sse-rate";seen.length=0;const streamed=await streaming("ses_streamRate");assert(streamed.includes('"model":"m1"'));assert.deepEqual(seen.map(r=>r.i),[0,1]);faults[0]=0;
  const restartUntil=await waitForProbeCooldown("upstream_rate_limit");

  child.kill("SIGTERM");await once(child,"exit");ready=await start();seen.length=0;
  const restored=await api("/api/model-services");
  const restoredStatus=restored.routingHealth.find(h=>h.modelId===models[0].id)?.status;
  assert.equal(restoredStatus?.state,"cooling");
  assert.equal(restoredStatus.untilMs,restartUntil,"Restart preserves the original cooldown deadline");
  const reopened = new DatabaseSync(path.join(dir,"app/local-agent.sqlite3"), { readOnly: true });
  try {
    for (const {session} of interruptedTurns) {
      assert.deepEqual(replyOutcomes(reopened,session),[["outcome_unknown","pending"]]);
    }
    assert.deepEqual(replyOutcomes(reopened,uncertainSession),[["pending","pending"]]);
    assert.deepEqual(replyOutcomes(reopened,unsentSession),[["preparing","pending"]]);
  } finally {
    reopened.close();
  }
  console.log(JSON.stringify({passed:true,priority:true,http500Fallback:true,http429Fallback:true,cooldownSkip:true,correctedInputUsesOriginalPublication:true,specifiedPreferencePreserved:true,toolContinuationPinned:true,concurrentDuplicateBlocked:true,concurrencySwitch:true,rpmSwitch:true,invalidRequestNotRetried:true,largeByteBodyAccepted:true,semanticContextError:true,configHashUnchanged:true,policyOnlyNoDirectorySync:true,cooldownSurvivesRestart:true,streamBeforeOutputFallback:true,noFallbackAfterPartialOutput:true,quotaNotRetried:true,untrustedSessionPinned:true,inflightPolicySnapshotPreserved:true,permanentSseErrorsNotRetried:true,failureCooldownRespectsToggle:true,interruptedRepliesPersistAfterRestart:true,compactedPendingHistoryBlocked:true,unsentPlanCanResume:true,sendBoundaryPersistsAcrossRestart:true,confirmedFallbackRetainsOriginalBudget:true,unknownBackupDoesNotResume:true}));
} finally {
  release?.();releaseRecoveryStream?.(true);if(child&&child.exitCode===null&&child.signalCode===null){child.kill("SIGTERM");await once(child,"exit");}
  for(const server of servers)await new Promise(r=>server.close(r));
  if (systemKeychain) console.log(JSON.stringify(cleanFixtureKeychain(data)));
  if(process.env.KEEP_POLICY_FIXTURE!=="1")rmSync(dir,{recursive:true,force:true});
}
