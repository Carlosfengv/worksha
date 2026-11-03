import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn, execFileSync } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { randomUUID, createHash } from "node:crypto";
import net from "node:net";
import http from "node:http";
import { readCoreSchema } from "./lib/core-schema.mjs";

const binary=path.resolve(process.env.PLUGIN_TEST_BINARY??"target/debug/local-agent-service");
const root=fs.realpathSync(fs.mkdtempSync(path.join(tmpdir(),"local-agent-plugin-m5-")));const home=path.join(root,"home");fs.mkdirSync(home);const data=path.join(root,"data");
const report={complete:false,binary,root,cases:[],realModelCalls:0};const passed=name=>{report.cases.push(name);console.log("通过："+name);};
const hash=p=>createHash("sha256").update(fs.readFileSync(p)).digest("hex");
const production=process.env.PLUGIN_M5_PRODUCTION==="1";
const env={...process.env,LOCAL_AGENT_SOURCE_HOME:home,...(production?{LOCAL_AGENT_VAULT_WORKER:"1"}:{LOCAL_AGENT_TEST_MEMORY_VAULT:"1",LOCAL_AGENT_TEST_IDLE_AGENTS:"1"})};
if(production){delete env.LOCAL_AGENT_TEST_MEMORY_VAULT;delete env.LOCAL_AGENT_TEST_IDLE_AGENTS;}
const sdk=(...args)=>execFileSync(binary,["--plugin-sdk",...args],{encoding:"utf8"});const key=path.join(root,"publisher.key");sdk("keygen",key);
function pack(name,version="1.0.0"){const folder=path.join(root,name+"-"+version);fs.cpSync("sdk/plugins/examples/"+name+"/package",folder,{recursive:true});const m=JSON.parse(fs.readFileSync(folder+"/manifest.json"));m.version=version;fs.writeFileSync(folder+"/manifest.json",JSON.stringify(m));const file=folder+".laplug";sdk("pack",folder,file,key);return file;}
const reservation=net.createServer();await new Promise(r=>reservation.listen(0,"127.0.0.1",r));const lanPort=reservation.address().port;await new Promise(r=>reservation.close(r));
execFileSync(binary,["gateway","configure","--data-dir",data,"--listen",`127.0.0.1:${lanPort}`,"--concurrency","4"],{env,stdio:"pipe"});
let base,token,child,stderr="",behavior="ok",httpCalls=0;const upstreamRequests=[];
const response={id:"m5-offline-response",object:"response",status:"completed",model:"fixture",output:[{type:"message",id:"msg-1",role:"assistant",content:[{type:"output_text",text:"M5_OFFLINE_你好",annotations:[]}]}],usage:{input_tokens:10,output_tokens:2,total_tokens:12}};
const upstream=http.createServer(async(req,res)=>{let raw="";for await(const b of req)raw+=b;const body=raw?JSON.parse(raw):{};upstreamRequests.push({path:req.url,authorization:req.headers.authorization,body});
 if(req.url==="/mcp"){
  assert.equal(req.headers.authorization,"Bearer m5-fake-upstream-key");httpCalls++;
  if(req.method==="GET"){res.writeHead(200,{"content-type":"text/event-stream"});res.write('data: {"jsonrpc":"2.0","method":"notifications/tools/list_changed"}\r\n\r\n');req.on("close",()=>res.end());return;}
  if(!body.id){res.writeHead(202);res.end();return;}
  if(body.method==="tools/call"&&body.params?.name==="wait"){res.writeHead(200,{"content-type":"text/event-stream"});res.write('data: {"jsonrpc":"2.0","method":"notifications/progress","params":{"progress":0}}\r\n\r\n');return;}
  const result=body.method==="initialize"?{protocolVersion:"2025-11-25",capabilities:{tools:{}},serverInfo:{name:"offline-http",version:"1"}}:body.method==="tools/list"?{tools:[{name:"echo"}]}:{content:[{type:"text",text:"HTTP_OFFLINE"}],isError:body.params?.name==="error"};
  const value={jsonrpc:"2.0",id:body.id,result};res.writeHead(200,{"content-type":"text/event-stream","mcp-session-id":"m5-http-session"});res.end(`data: ${JSON.stringify(value)}\r\n\r\n`);return;
 }
 const request=body.payload??body;
 if(req.url!=="/v1/envelope"){res.writeHead(200,{"content-type":"application/json"});res.end(JSON.stringify(response));return;}
 if(behavior==="redirect"){res.writeHead(307,{location:"/credential-trap"});res.end();return;}
 if(behavior==="echo"){res.writeHead(200,{"content-type":"application/x-ndjson"});const echoed=JSON.stringify({payload:'data: '+JSON.stringify({echo:"m5-fake-upstream-key"})+'\n\n'}).replace("m5-fake",'m5-fa\\u006be');for(const c of Buffer.from(echoed))res.write(Buffer.from([c]));res.end();return;}
 if(!request.stream){res.writeHead(200,{"content-type":"application/json"});res.end(JSON.stringify({payload:response}));return;}
 res.writeHead(200,{"content-type":"application/x-ndjson"});
 const events=[{type:"response.created",response:{id:response.id,model:"fixture",status:"in_progress"}},{type:"response.output_text.delta",delta:"M5_OFFLINE_你好"},{type:"response.output_item.added",output_index:1,item:{type:"function_call",id:"fc-1",call_id:"call-1",name:"namespace__tool",arguments:""}},{type:"response.function_call_arguments.delta",item_id:"fc-1",output_index:1,delta:'{"text":"完整工具参数"}'},{type:"response.output_item.done",output_index:1,item:{type:"function_call",id:"fc-1",call_id:"call-1",name:"namespace__tool",arguments:'{"text":"完整工具参数"}'}},{type:"response.completed",response}];
 const packet=v=>JSON.stringify({payload:`data: ${JSON.stringify(v)}\n\n`})+"\n";
 const bytes=Buffer.from(events.map(packet).join(""));
 if(behavior==="hold"){res.write(packet(events[0]));return;}
 if(behavior==="truncated"){res.end(packet(events[0])+JSON.stringify({payload:"unfinished"}).slice(0,12));return;}
 for(let i=0;i<bytes.length;i+=13)res.write(bytes.subarray(i,i+13));res.end();
});await new Promise(r=>upstream.listen(0,"127.0.0.1",r));
async function boot(){token=randomUUID();child=spawn(binary,["gateway","serve","--data-dir",data],{env:{...env,LOCAL_AGENT_SERVICE_TOKEN:token},stdio:["ignore","pipe","pipe"]});child.stderr.on("data",b=>stderr+=b);const lines=createInterface({input:child.stdout});const [line]=await Promise.race([once(lines,"line",{signal:AbortSignal.timeout(20000)}),once(child,"exit").then(()=>{throw Error(stderr)})]);base="http://127.0.0.1:"+JSON.parse(line).port;token=JSON.parse(fs.readFileSync(path.join(data,"runtime/service.json"))).token;}
async function api(route,body,expected=200,method=body===undefined?"GET":"POST"){const r=await fetch(base+route,{method,headers:{authorization:"Bearer "+token,"content-type":"application/json"},body:body===undefined?undefined:JSON.stringify(body),signal:AbortSignal.timeout(40000)});const v=await r.json();assert.equal(r.status,expected,route+JSON.stringify(v));return v;}
const ref=p=>({pluginId:p.manifest.id,digest:p.digest});const state=(p,action)=>api("/api/plugins/state",{...ref(p),action});const action=(p,action,expected=200)=>api("/api/plugin-extensions/action",{...ref(p),action},expected);const status=p=>api("/api/plugin-extensions/status",ref(p));
async function install(file){const r=await api("/api/plugins/inspect",{path:file});return api("/api/plugins/install",{packageRef:r.packageRef,digest:r.digest,approvePublisher:true});}
const authorize=(p,file,expected=200)=>api("/api/plugin-extensions/resource",{...ref(p),path:file},expected);
async function stop(){if(!child||child.exitCode!==null)return;const done=once(child,"exit");await api("/api/runtime/shutdown",{});await done;}
const wait=ms=>new Promise(r=>setTimeout(r,ms));
async function poll(fn){for(let i=0;i<80;i++){if(await fn())return;await wait(30);}throw Error("poll timeout");}
let provider;
try {
 await boot();
 const coreSchema=readCoreSchema(path.join(data,"local-agent.sqlite3"));assert(coreSchema.version>0);
 assert.equal((await fetch(base+"/api/plugin-extensions/status",{method:"POST",body:"{}"})).status,401);
 const cred=await api("/api/provider-credentials",{label:"M5 fixture",secret:"m5-fake-upstream-key"});const p0=(await api("/api/snapshot")).providers.find(p=>p.id==="openai");provider={...p0,...cred,endpoint:`http://127.0.0.1:${upstream.address().port}/v1`,model:"fixture",models:["fixture"],configured:true};await api("/api/providers",provider,200,"PUT");provider=(await api("/api/snapshot")).providers.find(p=>p.id==="openai");
 let protocol=await state(await install(pack("envelope-protocol")),"enable");assert.equal(upstreamRequests.length,0);
 await action(protocol,{kind:"bind-protocol",providerId:provider.id,allowPrivateNetwork:true});await action(protocol,{kind:"bind-protocol",providerId:provider.id,allowPrivateNetwork:true},400);
 const client=await api("/api/gateway-lan/clients",{name:"offline protocol test",agent:"codex",providerId:provider.id,models:["fixture"],concurrency:2});
 const request={model:"fixture",stream:true,input:[{role:"user",content:"只用合成上游"}],tools:[{type:"custom",name:"plain",format:{type:"text"}},{type:"namespace",name:"space",tools:[{type:"function",name:"nested",parameters:{type:"object"}}]}],metadata:{keep:"unchanged"}};
 const modelCall=body=>fetch(`http://127.0.0.1:${lanPort}/v1/responses`,{method:"POST",headers:{authorization:"Bearer "+client.key,"content-type":"application/json"},body:JSON.stringify(body),signal:AbortSignal.timeout(40000)});
 await state(protocol,"remove").then(()=>assert.fail("bound protocol must not be removed"),e=>assert.match(String(e),/protocol_binding_active/));
 await action(protocol,{kind:"unbind-protocol"});await action(protocol,{kind:"bind-protocol",providerId:provider.id,allowPrivateNetwork:false});const deniedBefore=upstreamRequests.length;assert.equal((await modelCall(request)).status,502);assert.equal(upstreamRequests.length,deniedBefore);await action(protocol,{kind:"unbind-protocol"});await action(protocol,{kind:"bind-protocol",providerId:provider.id,allowPrivateNetwork:true});
 const t=performance.now();let r=await modelCall(request);assert.equal(r.status,200,await r.clone().text());const sse=await r.text();assert.match(sse,/M5_OFFLINE_你好/);assert.match(sse,/namespace__tool/);assert.match(sse,/response.completed/);assert.deepEqual(upstreamRequests.at(-1).body.payload.tools,request.tools);report.firstStreamMs=performance.now()-t;
 r=await modelCall({...request,stream:false});assert.equal(r.status,200);assert.deepEqual(await r.json(),response);
 passed("外部 Wasm 请求 / 字节流 / 完整结束；UTF-8 切片、命名空间和自由格式工具保留；JSON 非流式回放");
 protocol=await state(await install(pack("envelope-protocol","1.1.0")),"enable");assert.equal((await modelCall(request)).status,502);await action(protocol,{kind:"unbind-protocol"});await action(protocol,{kind:"bind-protocol",providerId:provider.id,allowPrivateNetwork:true});
 behavior="echo";r=await modelCall(request);await assert.rejects(r.text());behavior="ok";
 behavior="truncated";r=await modelCall(request);await assert.rejects(r.text());behavior="hold";
 r=await modelCall(request);const reader=r.body.getReader();await reader.read();await reader.cancel();await poll(async()=>{const rows=execFileSync("ps",["-axo","ppid=,command="],{encoding:"utf8"});return !rows.split("\n").some(s=>s.trim().startsWith(String(child.pid)+" ")&&s.includes(" --plugin-worker "));});
 r=await modelCall(request);const reading=r.text();await state(protocol,"revoke-grants");await assert.rejects(reading);behavior="ok";r=await modelCall(request);assert.equal(r.status,502);await action(protocol,{kind:"unbind-protocol"});
 passed("截断失败、客户端取消、在途撤权和旧绑定拒绝；无自动重发，worker 回收");
 let config=await state(await install(pack("json-config")),"enable");const configFile=path.join(root,"agent-config.json");const original=JSON.stringify({env:{KEEP:"preserve"},unrelated:[1,2,3]});fs.writeFileSync(configFile,original);await authorize(config,configFile);
 let plan=await action(config,{kind:"prepare-config",providerId:provider.id});assert.equal(fs.readFileSync(configFile,"utf8"),original);assert(!JSON.stringify(plan).includes("m5-fake-upstream-key"));
 fs.writeFileSync(configFile,original+" ");await action(config,{kind:"apply-config",planId:plan.planId,revision:plan.revision},409);assert.equal(fs.readFileSync(configFile,"utf8"),original+" ");
 fs.writeFileSync(configFile,original);await authorize(config,configFile);plan=await action(config,{kind:"prepare-config",providerId:provider.id});let applied=await action(config,{kind:"apply-config",planId:plan.planId,revision:plan.revision});assert.equal(applied.state,"applied");assert.equal(JSON.parse(fs.readFileSync(configFile)).env.API_KEY,"m5-fake-upstream-key");assert.equal(JSON.parse(fs.readFileSync(configFile)).env.KEEP,"preserve");const afterHash=hash(configFile);await action(config,{kind:"apply-config",planId:plan.planId,revision:plan.revision});assert.equal(hash(configFile),afterHash);
 assert.equal(fs.statSync(configFile).mode&0o777,0o600);
 fs.writeFileSync(configFile,"{\"new\":true}");await action(config,{kind:"restore-config",operationId:plan.planId},409);assert.equal(fs.readFileSync(configFile,"utf8"),"{\"new\":true}");
 const secondConfig=path.join(root,"second-config.json");fs.writeFileSync(secondConfig,original);const secondMode=fs.statSync(secondConfig).mode&0o777;await authorize(config,secondConfig);const secondPlan=await action(config,{kind:"prepare-config",providerId:provider.id});await action(config,{kind:"apply-config",planId:secondPlan.planId,revision:secondPlan.revision});
 await state(config,"remove");await action(config,{kind:"restore-config",operationId:secondPlan.planId});assert.equal(fs.readFileSync(secondConfig,"utf8"),original);assert.equal(fs.readFileSync(configFile,"utf8"),"{\"new\":true}");
 assert.equal(fs.statSync(secondConfig).mode&0o777,secondMode);
 passed("有限配置候选、凭据占位、预览无副作用、文件变化拒绝、幂等执行、冲突保护和卸载后宿主恢复");
 const stdio=await state(await install(pack("mcp-stdio")),"enable");const stdioFile=path.join(root,"stdio.json");fs.writeFileSync(stdioFile,JSON.stringify({transport:"stdio",command:"/usr/bin/python3",args:[path.resolve("sdk/plugins/fixtures/mcp-server.py")]}));await authorize(stdio,stdioFile);await action(stdio,{kind:"start-connector"});
 let st=await status(stdio);assert.equal(st.calls.length,0);assert.equal(st.connector.transportConnected,false);const connection=await api("/api/plugin-extensions/export",ref(stdio));const entry=Object.values(connection.mcpServers)[0];
 const rpc=(value,extra={})=>fetch(entry.url,{method:"POST",headers:{...entry.headers,"content-type":"application/json",...extra.headers},body:JSON.stringify(value),signal:AbortSignal.timeout(40000),...extra});
 assert.equal((await fetch(entry.url,{method:"POST",body:"{}"})).status,401);assert.equal((await rpc({jsonrpc:"2.0",id:0,method:"initialize"},{headers:{...entry.headers,origin:"https://untrusted.invalid"}})).status,401);
 const init={jsonrpc:"2.0",id:1,method:"initialize",params:{protocolVersion:"2025-11-25",capabilities:{},clientInfo:{name:"offline-test",version:"1"}}};assert.equal((await (await rpc(init)).json()).result.protocolVersion,"2025-11-25");await rpc({jsonrpc:"2.0",method:"notifications/initialized"});await rpc({jsonrpc:"2.0",id:2,method:"tools/list"});assert.equal((await status(stdio)).calls.length,0);
 for(const [id,name] of [[3,"echo"],[4,"error"],[3,"echo"]])assert.equal((await rpc({jsonrpc:"2.0",id,method:"tools/call",params:{name,arguments:{}}})).status,200);
 const pending=rpc({jsonrpc:"2.0",id:5,method:"tools/call",params:{name:"wait",arguments:{}}});await wait(50);await rpc({jsonrpc:"2.0",method:"notifications/cancelled",params:{requestId:5}});await pending;
 await poll(async()=>{st=await status(stdio);return st.calls.length===4&&st.connector.observationIncomplete===false;});assert.equal(st.calls.length,4);assert.equal(st.calls.filter(c=>c.status==="success").length,2);assert(st.calls.some(c=>c.status==="tool_error"));assert(st.calls.some(c=>c.status==="cancelled"));assert.equal(st.connector.tools,3);assert(!JSON.stringify(st).includes(entry.headers.Authorization));
 await action(stdio,{kind:"stop-connector"});try{const denied=await fetch(entry.url);assert(!denied.ok,"Stopped MCP connector cannot continue serving");}catch(e){assert(e instanceof TypeError);}passed("MCP stdio：真实独立进程、握手 / 工具目录不计调用、RPC ID 再用、通知 / 取消、错误分类、认证与停止");
 const httpPlugin=await state(await install(pack("mcp-http")),"enable");const httpFile=path.join(root,"http.json");fs.writeFileSync(httpFile,JSON.stringify({transport:"http",url:`http://127.0.0.1:${upstream.address().port}/mcp`,allowPrivateNetwork:true,providerId:provider.id}));await authorize(httpPlugin,httpFile);await action(httpPlugin,{kind:"start-connector"});const exported=await api("/api/plugin-extensions/export",ref(httpPlugin));const httpEntry=Object.values(exported.mcpServers)[0];
 const httpRpc=v=>fetch(httpEntry.url,{method:"POST",headers:{...httpEntry.headers,"content-type":"application/json","mcp-protocol-version":"2025-11-25","mcp-session-id":"m5-http-session"},body:JSON.stringify(v)});
 for(const v of [init,{jsonrpc:"2.0",id:2,method:"tools/list"},{jsonrpc:"2.0",id:3,method:"tools/call",params:{name:"echo"}}]){const r=await httpRpc(v);assert.equal(r.status,200);assert.equal(r.headers.get("mcp-session-id"),"m5-http-session");assert.match(await r.text(),/data:/);}
 await poll(async()=>{const current=await status(httpPlugin);return current.calls.length===1&&current.connector.observationIncomplete===false;});assert.equal((await status(httpPlugin)).calls.length,1);
 const held=await httpRpc({jsonrpc:"2.0",id:5,method:"tools/call",params:{name:"wait"}});const heldReading=held.text().then(()=>false,()=>true);await httpRpc({jsonrpc:"2.0",method:"notifications/cancelled",params:{requestId:5}});assert.equal(await heldReading,true);await poll(async()=> (await status(httpPlugin)).calls.some(c=>c.status==="cancelled"));
 const countBeforeRevision=httpCalls;await api("/api/providers",{...provider,name:"Changed fixture provider"},200,"PUT");provider=(await api("/api/snapshot")).providers.find(p=>p.id===provider.id);assert.equal((await httpRpc(init)).status,403);assert.equal(httpCalls,countBeforeRevision);
 await state(httpPlugin,"disable");await wait(100);try{const denied=await fetch(httpEntry.url);assert(!denied.ok,"Disabled MCP connector cannot serve");}catch(e){assert(e instanceof TypeError);}
 let httpReenabled=await state(httpPlugin,"enable");fs.writeFileSync(httpFile,JSON.stringify({transport:"http",url:"http://127.0.0.1:1/mcp",allowPrivateNetwork:true,providerId:provider.id}));await authorize(httpReenabled,httpFile);await action(httpReenabled,{kind:"start-connector"},403);await state(httpReenabled,"disable");
 passed("MCP Streamable HTTP：POST/SSE、CRLF、Session / Version、取消、凭据同源和版本复核、停用与独立来源统计");
 const core=await api("/api/snapshot");assert.equal(core.providers.find(p=>p.id===provider.id).revision,provider.revision);assert.deepEqual(readCoreSchema(path.join(data,"local-agent.sqlite3")),coreSchema);
 report.complete=true;report.syntheticModelRequests=upstreamRequests.filter(r=>r.path!=="/mcp").length;report.httpMcpRequests=httpCalls;report.stderrHadCredential=stderr.includes("m5-fake-upstream-key");assert(!report.stderrHadCredential);
}finally{try{await stop();}finally{upstream.closeAllConnections();await new Promise(r=>upstream.close(r));fs.rmSync(key,{force:true});report.temporarySigningKeyRemoved=true;report.processesExited=child?.exitCode!==null;report.binarySha256=hash(binary);
 if(production){const {DatabaseSync}=await import("node:sqlite");const refs=new Set();const core=new DatabaseSync(path.join(data,"local-agent.sqlite3"),{readOnly:true});for(const {value} of core.prepare("SELECT value FROM providers").all()){const p=JSON.parse(value);if(p.credentialRef)refs.add(p.credentialRef);}core.close();const metadata=new DatabaseSync(path.join(data,"plugin-extensions.sqlite3"),{readOnly:true});for(const {record} of metadata.prepare("SELECT record FROM plans").all()){const p=JSON.parse(record);if(p.backup_ref)refs.add(p.backup_ref);}metadata.close();report.temporaryCredentialsRemoved=Array.from(refs).map(account=>{const reply=JSON.parse(execFileSync(binary,["--vault-worker"],{env:{...env,LOCAL_AGENT_VAULT_WORKER:"1"},input:JSON.stringify({operation:"delete",account,bytes:[]}),encoding:"utf8"}));return reply.code===null;});}
 const out=process.env.PLUGIN_M5_REPORT??".work/plugin-extensions.json";fs.mkdirSync(path.dirname(out),{recursive:true});fs.writeFileSync(out,JSON.stringify(report,null,2)+"\n");}}
