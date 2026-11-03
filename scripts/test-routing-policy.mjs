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

const dir=realpathSync(mkdtempSync(path.join(tmpdir(),"local-agent-policy-")));const home=path.join(dir,"home");
mkdirSync(path.join(home,".config/opencode"),{recursive:true});const config=path.join(home,".config/opencode/opencode.json");writeFileSync(config,"{}\n");
const servers=[],faults=[0,0,0],seen=[];let child,stderr="",holdIndex=-1,arrive,release,retryAfter="1";
const token=randomUUID();const pause=ms=>new Promise(r=>setTimeout(r,ms));
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
      const message={id:randomUUID(),type:"message",role:"assistant",model:b.model,content:[{type:"text",text:"OK"}],stop_reason:"end_turn",usage:{input_tokens:5,output_tokens:2}};
      if(b.stream){res.writeHead(200,{"Content-Type":"text/event-stream"});res.end(startEvent+event("content_block_start",{index:0,content_block:{type:"text",text:""}})+event("content_block_delta",{index:0,delta:{type:"text_delta",text:"OK"}})+event("content_block_stop",{index:0})+event("message_delta",{delta:{stop_reason:"end_turn"},usage:{output_tokens:2}})+event("message_stop",{}));return;}
      res.writeHead(200,{"Content-Type":"application/json"});res.end(JSON.stringify(message));
    });await new Promise(r=>server.listen(0,"127.0.0.1",r));servers.push(server);
  }
  const start=async()=>{
    child=spawn(process.env.LOCAL_AGENT_TEST_SERVICE_BINARY??"target/debug/local-agent-service",["--data-dir",path.join(dir,"app")],{env:{...process.env,LOCAL_AGENT_SERVICE_TOKEN:token,LOCAL_AGENT_SOURCE_HOME:home,LOCAL_AGENT_TEST_MEMORY_VAULT:"1",LOCAL_AGENT_TEST_IDLE_AGENTS:"1"},stdio:["ignore","pipe","pipe"]});
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
  const status=await api("/api/provider-status");const cfg=status.configs.find(c=>c.agentId==="opencode");
  const tx=await api("/api/model-service-connections",{agentId:"opencode",expectedRevision:cfg.revision,serviceId:service.id,expectedServiceRevision:service.revision});await api(`/api/provider-switches/${tx.id}/apply`,{});
  const installed=JSON.parse(readFileSync(config));const group=installed.model.split("/")[0];const local=installed.provider[group].options;const configHash=createHash("sha256").update(readFileSync(config)).digest("hex");
  const user=text=>({role:"user",content:text});
  const send=async(messages,session="ses_"+randomUUID().replaceAll("-", ""),model="local-agent-default",expected=200,extra={})=>{const r=await fetch(local.baseURL+"/messages",{method:"POST",headers:{"x-api-key":local.apiKey,"x-opencode-session":session,"Content-Type":"application/json"},body:JSON.stringify({model,max_tokens:100,messages,...extra}),signal:AbortSignal.timeout(20000)});const raw=await r.text();assert.equal(r.status,expected,raw);return JSON.parse(raw);};
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
  faults[0]=500;holdIndex=0;const changing=new Promise(r=>{arrive=r;});const oldPolicyRequest=send([user("immutable policy")]);await changing;
  await publish({order:[models[0].id,models[2].id,models[1].id]});release();assert.equal((await oldPolicyRequest).model,"m1");faults[0]=0;await publish({order:models.map(m=>m.id)});await pause(1100);
  const streaming=async(session)=>{const r=await fetch(local.baseURL+"/messages",{method:"POST",headers:{"x-api-key":local.apiKey,"x-opencode-session":session,"Content-Type":"application/json"},body:JSON.stringify({model:"local-agent-default",max_tokens:100,stream:true,messages:[user("stream")]})});assert.equal(r.status,200);return r.text();};
  faults[0]="sse-partial";seen.length=0;assert((await streaming("ses_streamPartial")).includes("PARTIAL_A"));assert.deepEqual(seen.map(r=>r.i),[0]);faults[0]=0;await pause(1100);
  for(const [fault,status] of [["sse-auth",401],["sse-quota",429],["sse-context",400]]){faults[0]=fault;seen.length=0;const r=await fetch(local.baseURL+"/messages",{method:"POST",headers:{"x-api-key":local.apiKey,"x-opencode-session":"ses_"+randomUUID().replaceAll("-",""),"Content-Type":"application/json"},body:JSON.stringify({model:"local-agent-default",max_tokens:100,stream:true,messages:[user("permanent stream rejection")]})});assert.equal(r.status,status,await r.text());assert.deepEqual(seen.map(r=>r.i),[0]);}
  faults[0]=500;seen.length=0;await send([user("mark failure cooldown")]);await publish({fallbackOnFailure:false});seen.length=0;await send([user("failure cooldown respects disabled failure fallback")],undefined,"local-agent-default",429);assert.equal(seen.length,0);await publish({fallbackOnFailure:true});faults[0]=0;await pause(1100);
  const database = new DatabaseSync(path.join(dir,"app/local-agent.sqlite3"));
  const uncertainSession = "ses_"+randomUUID().replaceAll("-", "");
  const uncertainTurn = [user("accounting failure must not repeat the upstream call")];
  const previousOwners = new Set(database.prepare("SELECT owner FROM service_route_dispatch").all().map(row=>row.owner));
  try {
    database.exec("CREATE TRIGGER fixture_attempt_result_failure BEFORE UPDATE ON service_route_attempts BEGIN SELECT RAISE(FAIL,'fixture accounting unavailable'); END");
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
  faults[0]="sse-rate";seen.length=0;const streamed=await streaming("ses_streamRate");assert(streamed.includes('"model":"m1"'));assert.deepEqual(seen.map(r=>r.i),[0,1]);faults[0]=0;

  child.kill("SIGTERM");await once(child,"exit");ready=await start();seen.length=0;const restored=await api("/api/model-services");assert(restored.routingHealth.some(h=>h.modelId===models[0].id && h.status.state==="cooling"));
  console.log(JSON.stringify({passed:true,priority:true,http500Fallback:true,http429Fallback:true,cooldownSkip:true,specifiedPreferencePreserved:true,toolContinuationPinned:true,concurrentDuplicateBlocked:true,concurrencySwitch:true,rpmSwitch:true,invalidRequestNotRetried:true,largeByteBodyAccepted:true,semanticContextError:true,configHashUnchanged:true,policyOnlyNoDirectorySync:true,cooldownSurvivesRestart:true,streamBeforeOutputFallback:true,noFallbackAfterPartialOutput:true,quotaNotRetried:true,untrustedSessionPinned:true,inflightPolicySnapshotPreserved:true,permanentSseErrorsNotRetried:true,failureCooldownRespectsToggle:true}));
} finally {
  release?.();if(child&&child.exitCode===null){child.kill("SIGTERM");await once(child,"exit");}
  for(const server of servers)await new Promise(r=>server.close(r));if(process.env.KEEP_POLICY_FIXTURE!=="1")rmSync(dir,{recursive:true,force:true});
}
