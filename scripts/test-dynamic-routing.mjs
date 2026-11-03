import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { randomUUID, createHash } from "node:crypto";
import { createInterface } from "node:readline";
import { once } from "node:events";
import http from "node:http";

const dir = realpathSync(mkdtempSync(path.join(tmpdir(), "local-agent-dynamic-")));
const home = path.join(dir,"home");
mkdirSync(path.join(home,".config/opencode"),{recursive:true});
const file = path.join(home,".config/opencode/opencode.json");
const original=JSON.stringify({provider:{untouched:{npm:"@ai-sdk/anthropic"}},permission:{"*":"deny"},keep:true},null,2)+"\n";
writeFileSync(file,original,{mode:0o600});
const seen=[];let release,arrive,hold=false;
const servers=[];let child;let stderr="";
const hash=()=>createHash("sha256").update(readFileSync(file)).digest("hex");
const token=randomUUID();
try {
  for(const protocol of ["messages","chat-completions"]){
    const server=http.createServer(async(req,res)=>{
      let text="";for await(const chunk of req)text+=chunk;
      const body=JSON.parse(text);seen.push({protocol,model:body.model});
      if(hold){hold=false;arrive();await new Promise(resolve=>{release=resolve;});}
      const result=protocol==="messages"?{id:randomUUID(),type:"message",role:"assistant",model:body.model,content:[{type:"text",text:"OK"}],stop_reason:"end_turn",usage:{input_tokens:5,output_tokens:2}}:{id:randomUUID(),object:"chat.completion",model:body.model,choices:[{index:0,message:{role:"assistant",content:"OK"},finish_reason:"stop"}],usage:{prompt_tokens:5,completion_tokens:2,total_tokens:7}};
      res.writeHead(200,{"Content-Type":"application/json"});res.end(JSON.stringify(result));
    });
    await new Promise(resolve=>server.listen(0,"127.0.0.1",resolve));servers.push(server);
  }
  child=spawn(process.env.LOCAL_AGENT_TEST_SERVICE_BINARY??".work/provider-tests-target/debug/local-agent-service",["--data-dir",path.join(dir,"app")],{env:{...process.env,LOCAL_AGENT_SERVICE_TOKEN:token,LOCAL_AGENT_SOURCE_HOME:home,LOCAL_AGENT_TEST_MEMORY_VAULT:"1",LOCAL_AGENT_TEST_IDLE_AGENTS:"1"},stdio:["ignore","pipe","pipe"]});
  child.stderr.on("data",b=>stderr+=b);
  const ready=await Promise.race([once(createInterface({input:child.stdout}),"line").then(([line])=>JSON.parse(line)),once(child,"exit").then(()=>{throw Error("service exited: "+stderr);})]);
  const base=`http://127.0.0.1:${ready.port}`;
  const api=async(route,body,expected=200,method=body===undefined?"GET":"POST")=>{
    const r=await fetch(base+route,{method,headers:{Authorization:`Bearer ${token}`,"Content-Type":"application/json"},body:body===undefined?undefined:JSON.stringify(body)});
    const doc=await r.json();assert.equal(r.status,expected,JSON.stringify(doc));return doc;
  };
  const providers=[];
  for(const [index,server]of servers.entries()){
    const p={id:`custom-${randomUUID()}`,vendor:"custom",name:index?"Fixture B":"Fixture A",endpoint:`http://127.0.0.1:${server.address().port}${index?"/v1":""}`,protocol:index?"chat-completions":"messages",model:index?"b1":"a1",models:index?["b1"]:["a1","a2"],disabledModels:[],configured:true,credentialRef:null,credentialLabel:null,revision:0,category:"local"};
    await api("/api/providers",p,200,"PUT");
    providers.push({...p,revision:1});
    for(const model of p.models)assert.equal((await api(`/api/providers/${p.id}/verify`,{expectedRevision:1,model})).state,"verified");
  }
  let status=await api("/api/provider-status");
  const config=status.configs.find(c=>c.agentId==="opencode");
  const t=await api("/api/dynamic-connections",{agentId:"opencode",expectedRevision:config.revision,providerId:providers[0].id,expectedProviderRevision:1,model:"a1"});
  assert.equal(readFileSync(file,"utf8"),original);
  assert(t.backupPath.endsWith(".sealed"));
  await api(`/api/provider-switches/${t.id}/apply`,{});
  const installedHash=hash();
  const installed=JSON.parse(readFileSync(file,"utf8"));
  assert(installed.provider.untouched);assert(installed.keep);
  const group=installed.model.split("/")[0];
  const local=installed.provider[group].options;
  const route=local.baseURL+"/messages";
  const request=async(messages,session="ses_first",extra={})=>{
    const r=await fetch(route,{method:"POST",headers:{Authorization:`Bearer ${local.apiKey}`,"Content-Type":"application/json","x-opencode-session":session,...extra},body:JSON.stringify({model:"local-agent-default",max_tokens:30,messages})});
    const text=await r.text();assert.equal(r.status,200,text);return JSON.parse(text);
  };
  const first=[{role:"user",content:"one"}];
  hold=true;
  const arrived=new Promise(resolve=>arrive=resolve);
  const running=request(first);
  await arrived;
  const switchTo=async(provider,model)=>{
    status=await api("/api/provider-status");const binding=status.dynamicBindings.find(b=>b.id===t.routing.id);
    const c=await api("/api/dynamic-routes",{operationId:randomUUID(),bindingId:binding.id,expectedGeneration:binding.generation,providerId:provider.id,expectedProviderRevision:1,model});
    assert.equal(hash(),installedHash);
    await api(`/api/dynamic-routes/${c.id}`,{action:"commit"});
    const repeated=await api(`/api/dynamic-routes/${c.id}`,{action:"commit"});assert.equal(repeated.state,"applied");
    assert.equal(hash(),installedHash);return c;
  };
  const change=await switchTo(providers[1],"b1");release();assert.equal((await running).model,"a1");
  const toolHistory=[...first,{role:"assistant",content:[{type:"tool_use",id:"call-one",name:"read",input:{}}]},{role:"user",content:[{type:"tool_result",tool_use_id:"call-one",content:"fixture result"}]}];
  assert.equal((await request(toolHistory)).model,"a1");
  const second=[...first,{role:"assistant",content:"OK"},{role:"user",content:"two"}];
  assert.equal((await request(second)).model,"b1");
  assert.equal((await request(first,"ses_parallel")).model,"b1");
  assert.equal((await request(first)).model,"a1");
  await switchTo(providers[0],"a2");
  const third=[...second,{role:"assistant",content:"OK"},{role:"user",content:"three"}];
  assert.equal((await request(third)).model,"a2");
  assert.equal((await request([...first,{role:"user",content:"two"}],"ses_parallel")).model,"a2");
  const bad=await fetch(route,{method:"POST",headers:{Authorization:`Bearer ${local.apiKey}`,Origin:"https://untrusted.invalid","Content-Type":"application/json"},body:JSON.stringify({model:"local-agent-default",messages:first})});assert.equal(bad.status,401);
  const before=seen.length;
  const invalid=await fetch(route,{method:"POST",headers:{Authorization:`Bearer ${local.apiKey}`,"Content-Type":"application/json"},body:JSON.stringify({model:"not-published",messages:first})});assert.equal(invalid.status,400);assert.equal(seen.length,before);
  status=await api("/api/provider-status");
  assert.equal(status.dynamicBindings.find(b=>b.id===t.routing.id).generation,"3");
  assert(status.dynamicLeases.some(l=>l.routeVersionId!==status.dynamicBindings.find(b=>b.id===t.routing.id).target.id));
  assert.equal(hash(),installedHash);
  const restored=await api(`/api/provider-switches/${t.id}/restore`,{});
  await api(`/api/provider-switches/${restored.id}/apply`,{});
  assert.equal(readFileSync(file,"utf8"),original);
  await api(`/api/dynamic-routes/${change.id}`,{action:"restore"},409);
  console.log(JSON.stringify({passed:true,concurrentSwitch:true,toolContinuationPinned:true,sameSessionNextTurn:true,providerAndModelSwitch:true,oldRequestReplay:true,independentSessions:true,configDigestUnchanged:true,encryptedBackupAndByteRestore:true,requests:seen}));
}finally{
  release?.();
  if(child && child.exitCode===null){child.kill("SIGTERM");await once(child,"exit");}
  for(const s of servers)await new Promise(resolve=>s.close(resolve));
  rmSync(dir,{recursive:true,force:true});
}
