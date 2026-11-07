import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import http from "node:http";
import { randomUUID, createHash } from "node:crypto";
import { DatabaseSync } from "node:sqlite";

const binary=path.resolve(process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ?? "target/debug/local-agent-service");
const oldBinary=process.env.PROVIDER_MIGRATION_SOURCE_BINARY ? path.resolve(process.env.PROVIDER_MIGRATION_SOURCE_BINARY) : null;
const root=fs.realpathSync(fs.mkdtempSync(path.join(tmpdir(),"local-agent-provider-operations-")));
const data=path.join(root,"data"), home=path.join(root,"home");fs.mkdirSync(home);
const secret="disposable-provider-operation-fixture-key";
const sha=(file)=>createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const report={complete:false,binarySha256:sha(binary),cases:[],realModelCalls:0,realKeychainCalls:0};
let child,base,token,currentData=data,arrive,release;
let probes=0;
const upstream=http.createServer(async(req,res)=>{
  let raw="";for await(const chunk of req)raw+=chunk;
  const route=req.url.split("/").filter(Boolean)[0];
  const json=(status,body)=>{res.writeHead(status,{"content-type":"application/json"});res.end(JSON.stringify(body));};
  if(req.method==="GET"){json(200,{data:[{id:"fixture-model"}]});return;}
  probes++;
  if(route==="held"){arrive?.();await new Promise((resolve)=>{release=resolve;});}
  if(res.destroyed)return;
  if(route==="keyed")assert.equal(req.headers.authorization,`Bearer ${secret}`);
  if(route==="reject"){json(401,{});return;}
  if(!req.url.endsWith("/responses")){json(404,{});return;}
  const body=JSON.parse(raw);
  json(200,{id:randomUUID(),object:"response",status:"completed",model:body.model,
    output:[{type:"message",id:randomUUID(),role:"assistant",status:"completed",content:[{type:"output_text",text:"OK",annotations:[]}]}],
    usage:{input_tokens:5,output_tokens:2,total_tokens:7}});
});
await new Promise((resolve)=>upstream.listen(0,"127.0.0.1",resolve));
const endpoint=(kind="ok")=>`http://127.0.0.1:${upstream.address().port}/${kind}/v1`;
async function start(selected=binary,folder=data){
  token=randomUUID();currentData=folder;let stderr="";
  const owned=spawn(selected,["--data-dir",folder],{env:{...process.env,LOCAL_AGENT_SOURCE_HOME:home,LOCAL_AGENT_SERVICE_TOKEN:token,LOCAL_AGENT_TEST_MEMORY_VAULT:"1",LOCAL_AGENT_TEST_IDLE_AGENTS:"1"},stdio:["ignore","pipe","pipe"]});
  child=owned;owned.stderr.on("data",(bytes)=>{stderr+=bytes;});
  const lines=createInterface({input:owned.stdout});
  const ready=await Promise.race([once(lines,"line").then(([line])=>JSON.parse(line)),once(owned,"exit").then(()=>{throw Error(`isolated service exited: ${stderr}`);})]);
  lines.close();base=`http://127.0.0.1:${ready.port}`;
}
async function stop(signal="SIGTERM"){
  const owned=child;child=undefined;if(!owned || owned.exitCode!==null)return;
  const exited=once(owned,"exit");const fallback=setTimeout(()=>owned.kill("SIGKILL"),8000);fallback.unref();
  owned.kill(signal);try{await exited;}finally{clearTimeout(fallback);}
}
async function call(route,body,method=body===undefined?"GET":"POST",expected=200,headers={}){
  const response=await fetch(base+route,{method,headers:{Authorization:`Bearer ${token}`,"Content-Type":"application/json",...headers},body:body===undefined?undefined:JSON.stringify(body),signal:AbortSignal.timeout(12000)});
  const raw=await response.text();assert.equal(response.status,expected,raw);assert(!raw.includes(secret));return JSON.parse(raw);
}
const protocol=()=>call("/api/provider-operations/protocol");
const commit=(request)=>call("/api/provider-operations",request);
const query=(request)=>call(`/api/provider-operations/${request.operationId}?storeId=${request.storeId}`);
const cancel=(request)=>call("/api/provider-operations/cancel",request);
const provider=(kind="ok")=>({id:`custom-${randomUUID()}`,name:"Provider operation fixture",vendor:"custom",endpoint:endpoint(kind),protocol:"responses",model:"fixture-model",models:["fixture-model"],configured:false,credentialRef:null,credentialLabel:null,revision:0,category:"local"});
async function request(kind,p=provider()){
  const scope=await protocol();if(kind==="connect")p={...p,endpoint:p.endpoint+"/responses"};return {storeId:scope.storeId,operationId:randomUUID(),expectedSequence:scope.sequence,kind,provider:p};
}
const dbRead=(action)=>{const db=new DatabaseSync(path.join(currentData,"local-agent.sqlite3"),{readOnly:true});try{return action(db);}finally{db.close();}};
const value=(id)=>dbRead((db)=>db.prepare("SELECT value FROM providers WHERE id=?").get(id)?.value);
const record=(label)=>report.cases.push(label);
async function loseReply(input){
  await new Promise((resolve,reject)=>{
    const req=http.request(base+"/api/provider-operations",{method:"POST",headers:{Authorization:`Bearer ${token}`,"Content-Type":"application/json"}},(res)=>{assert.equal(res.statusCode,200);res.destroy();resolve();});
    req.on("error",reject);req.end(JSON.stringify(input));
  });
}
try{
  await start();
  const initial=await protocol();assert.equal(initial.version,1);assert.equal(initial.sequence,0);assert.deepEqual(initial.pendingOperationIds,[]);
  const manual=await request("save");const receipts=await Promise.all(Array.from({length:12},()=>commit(manual)));
  assert(receipts.every((r)=>r.status==="committed"));assert(receipts.every((r)=>JSON.stringify(r)===JSON.stringify(receipts[0])));
  assert.equal(JSON.parse(value(manual.provider.id)).revision,1);
  record("12 concurrent manual requests share one receipt and one committed metadata revision");
  const changed={...manual,provider:{...manual.provider,name:"Changed payload"}};
  assert.equal((await call("/api/provider-operations",changed,"POST",409)).code,"operation_conflict");
  assert.equal((await call("/api/provider-operations",{...manual,storeId:randomUUID()},"POST",409)).code,"operation_store_changed");
  const legacy=receipts[0].provider;await call("/api/providers",{...legacy,name:"Later legacy writer"},"PUT");
  assert.deepEqual(await commit(manual),receipts[0]);assert.equal(JSON.parse(value(manual.provider.id)).revision,2);
  record("same-ID conflicts and cross-store requests reject; replay never overwrites a later legacy edit");
  const large=provider();large.models=Array.from({length:2000},(_,n)=>`${String(n).padStart(4,"0")}-${"m".repeat(195)}`);large.model=large.models[0];large.disabledModels=large.models.slice(1);
  const largeRequest=await request("save",large);assert(Buffer.byteLength(JSON.stringify(largeRequest))>512*1024);
  const largeReceipt=await commit(largeRequest);assert.equal(largeReceipt.status,"committed");assert.deepEqual(largeReceipt.provider.models,large.models);assert.deepEqual(largeReceipt.provider.disabledModels,large.disabledModels);
  assert.deepEqual(await query(largeRequest),largeReceipt);record("the complete supported 2000-model catalog and 1999 disabled entries survive a real HTTP write and query without truncation");
  const lost=await request("save");await loseReply(lost);const recovered=await query(lost);assert.equal(recovered.status,"committed");assert.deepEqual(await commit(lost),recovered);
  record("a deliberately discarded HTTP response is recovered with its original durable result");
  const credential=await call("/api/provider-credentials",{operationId:randomUUID(),label:"Fixture provider credential",secret});
  const keyed={...provider("keyed"),credentialRef:credential.credentialRef,credentialLabel:credential.credentialLabel};
  const connection=await request("connect",keyed);const before=probes;const connected=await commit(connection);
  assert.equal(connected.status,"committed");assert.equal(connected.provider.revision,1);assert.equal(connected.verification.state,"verified");
  assert(!dbRead((db)=>db.prepare("SELECT value FROM provider_operations WHERE operation_id=?").get(connection.operationId).value).includes(secret));
  assert.equal(probes-before,1);const providerBytes=value(connection.provider.id);
  for(let n=0;n<6;n++)assert.deepEqual(await commit(connection),connected);
  assert.equal(probes-before,1);assert.equal(value(connection.provider.id),providerBytes);
  record("connected metadata and receipt commit together; six replays reuse the result without another upstream request");
  const rejected=await request("connect",provider("reject"));const failed=await commit(rejected);assert.equal(failed.status,"failed");
  const callsAfterFailure=probes;assert.deepEqual(await commit(rejected),failed);assert.equal(probes,callsAfterFailure);assert.equal(value(rejected.provider.id),undefined);
  record("a failed connection is durable and retries of its ID never re-run probes");
  const cancelled=await request("connect");assert.equal((await cancel(cancelled)).status,"cancelled");const beforeCancelled=probes;assert.equal((await commit(cancelled)).status,"cancelled");assert.equal(probes,beforeCancelled);
  record("cancellation before delivery creates a durable fence and does not probe");
  const held=await request("connect",provider("held"));let entered;
  const enteredPromise=new Promise((resolve)=>{entered=resolve;});arrive=entered;
  const heldResponse=commit(held);await enteredPromise;
  assert.equal((await query(held)).status,"pending");assert((await protocol()).pendingOperationIds.includes(held.operationId));
  const heldCount=probes;assert.equal((await commit(held)).status,"pending");assert.equal(probes,heldCount);
  assert.equal((await cancel(held)).status,"cancelled");release();release=undefined;
  assert.equal((await heldResponse).status,"cancelled");assert.equal(value(held.provider.id),undefined);
  record("while a real fake-upstream response is held, reads/replay do not probe; cancellation wins over late completion");
  const interrupted=await request("connect",provider("held"));let interruptedEntered;
  const interruptedPromise=new Promise((resolve)=>{interruptedEntered=resolve;});arrive=interruptedEntered;
  const interruptedResponse=commit(interrupted).catch(()=>null);await interruptedPromise;
  const beforeRestart=probes;await stop("SIGKILL");release?.();release=undefined;await interruptedResponse;
  await start();assert.equal((await query(interrupted)).status,"pending");assert.equal((await commit(interrupted)).status,"pending");assert.equal(probes,beforeRestart);
  assert((await protocol()).pendingOperationIds.includes(interrupted.operationId));assert.equal((await cancel(interrupted)).status,"cancelled");
  assert.deepEqual(await query(connection),connected);assert.deepEqual(await commit(connection),connected);assert.equal(probes,beforeRestart);
  record("actual service process kill/restart preserves pending work and historical results without resuming paid probes");
  const expired=await request("connect");await cancel(expired);
  const seq=expired.expectedSequence;
  for(let n=0;n<256;n++)await cancel({...expired,operationId:randomUUID(),expectedSequence:seq+1+n});
  assert.equal((await call(`/api/provider-operations/${expired.operationId}?storeId=${expired.storeId}`,undefined,"GET",404)).code,"operation_not_found");
  assert.equal((await call("/api/provider-operations",expired,"POST",409)).code,"operation_expired");assert.equal(probes,beforeRestart);
  const counts=dbRead((db)=>db.prepare("SELECT count(*) AS count,sum(length(CAST(value AS BLOB))) AS bytes FROM provider_operations WHERE state!='pending'").get());
  assert(counts.count<=256 && counts.bytes<=16*1024*1024);
  record("receipt reclamation remains bounded and an evicted operation cannot restart inference");
  await call("/api/provider-operations/protocol",undefined,"GET",401,{Authorization:"Bearer invalid"});
  await call("/api/provider-operations/protocol",undefined,"GET",401,{Origin:"https://invalid.example"});
  await new Promise((resolve,reject)=>{const req=http.get(base+"/api/provider-operations/protocol",{headers:{Authorization:`Bearer ${token}`,Host:"invalid.example"}},(res)=>{assert.equal(res.statusCode,401);res.resume();res.on("end",resolve);});req.on("error",reject);});
  const invalid=await request("save");const invalidBody=await call("/api/provider-operations",{...invalid,future:true},"POST",422);assert.equal(invalidBody.code,"invalid_input");assert.equal(invalidBody.outcomeUnknown,false);
  const invalidQuery=await call(`/api/provider-operations/${invalid.operationId}?storeId=${invalid.storeId}&future=true`,undefined,"GET",400);assert.equal(invalidQuery.code,"invalid_input");
  assert.equal(value(invalid.provider.id),undefined);
  assert.equal(dbRead((db)=>db.prepare("SELECT count(*) AS count FROM provider_operations WHERE instr(value,?)>0").get(secret).count),0);
  record("management authorization, Host/Origin, unknown fields and receipt plaintext boundaries are checked on actual HTTP routes");
  await stop();
  if(oldBinary){
    const migration=path.join(root,"migration");await start(oldBinary,migration);
    const seed=provider();await call("/api/providers",seed,"PUT");const oldSnapshot=await call("/api/snapshot");const bytes=value(seed.id);
    assert.equal(dbRead((db)=>db.prepare("PRAGMA user_version").get().user_version),8);await stop();
    await start(binary,migration);assert.equal(value(seed.id),bytes);const upgraded=await call("/api/snapshot");assert.deepEqual(upgraded.providers,oldSnapshot.providers);
    assert.equal((await protocol()).storeId,oldSnapshot.settingsWriteProtocol.storeId);
    const backupPath=path.join(migration,"local-agent.pre-provider-operations-v8.sqlite3");const backup=new DatabaseSync(backupPath,{readOnly:true});
    assert.equal(backup.prepare("PRAGMA user_version").get().user_version,8);assert.equal(backup.prepare("SELECT value FROM providers WHERE id=?").get(seed.id).value,bytes);backup.close();
    assert.equal(fs.statSync(backupPath).mode&0o777,0o600);const backupSha=sha(backupPath);await stop();
    await assert.rejects(()=>start(oldBinary,migration),/isolated service exited/);await stop();
    assert.equal(dbRead((db)=>db.prepare("PRAGMA user_version").get().user_version),16);assert.equal(value(seed.id),bytes);assert.equal(sha(backupPath),backupSha);
    report.migration={actualPreviousProcess:true,sourceBinarySha256:sha(oldBinary),sourceSchema:8,targetSchema:16,privateBackupUnchanged:true,downgradeRejected:true};
    record("a fixed previous binary creates schema 8; schema 16 preserves metadata/identity/private backup and rejects the old reader on downgrade");
  }else report.migration={actualPreviousProcess:false,note:"Core tests cover generated v8 migration; set PROVIDER_MIGRATION_SOURCE_BINARY for previous-process upgrade and downgrade evidence."};
  assert.equal(sha(binary),report.binarySha256);report.fakeUpstreamRequests=probes;report.complete=true;
}finally{
  release?.();upstream.closeAllConnections();await stop();await new Promise((resolve)=>upstream.close(resolve));
  if(process.env.PROVIDER_OPERATIONS_REPORT)fs.writeFileSync(process.env.PROVIDER_OPERATIONS_REPORT,JSON.stringify(report,null,2)+"\n");
  fs.rmSync(root,{recursive:true,force:true});
}
console.log(JSON.stringify(report,null,2));
