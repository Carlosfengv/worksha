import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { createHash, randomUUID } from "node:crypto";
import { request as httpRequest } from "node:http";
import { DatabaseSync } from "node:sqlite";

const binary=path.resolve(process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ?? "target/debug/local-agent-service");
const previous=process.env.NATIVE_IMPORT_PREVIOUS_BINARY && path.resolve(process.env.NATIVE_IMPORT_PREVIOUS_BINARY);
const root=fs.realpathSync(fs.mkdtempSync(path.join(tmpdir(),"native-import-operations-")));
const home=path.join(root,"home");fs.mkdirSync(path.join(home,".claude"),{recursive:true});fs.mkdirSync(path.join(home,".codex"));
const secret="offline-native-import-owned-key-72819", source=JSON.stringify({env:{ANTHROPIC_API_KEY:secret,ANTHROPIC_MODEL:"native-model"}});
fs.writeFileSync(path.join(home,".claude/settings.json"),source);
const hash=p=>createHash("sha256").update(fs.readFileSync(p)).digest("hex");
const children=new Set();
const report={passed:false,binarySha256:hash(binary),previousBinarySha256:previous?hash(previous):null,cases:[],realModelCalls:0,realKeychainCalls:0};
function sql(c,fn) {const db=new DatabaseSync(path.join(c.data,"local-agent.sqlite3"));try{return fn(db)}finally{db.close()}}
async function boot(data,executable=binary,sourceHome=home) {
  const token=randomUUID(),child=spawn(executable,["--data-dir",data],{env:{...process.env,LOCAL_AGENT_SOURCE_HOME:sourceHome,LOCAL_AGENT_SERVICE_TOKEN:token,LOCAL_AGENT_TEST_MEMORY_VAULT:"1",LOCAL_AGENT_TEST_IDLE_AGENTS:"1"},stdio:["ignore","pipe","pipe"]});
  children.add(child);child.once("exit",()=>children.delete(child));let stderr="";child.stderr.on("data",b=>{stderr+=b});
  const lines=createInterface({input:child.stdout});
  try {const [line]=await Promise.race([once(lines,"line",{signal:AbortSignal.timeout(15000)}),once(child,"exit").then(()=>{throw Error(stderr)})]);return {child,data,token,base:"http://127.0.0.1:"+JSON.parse(line).port}}
  finally {lines.close()}
}
async function stop(child) {if(!child||child.exitCode!==null||child.signalCode!==null)return;const done=once(child,"exit"),timer=setTimeout(()=>child.kill("SIGKILL"),8000);timer.unref();child.kill("SIGTERM");try{await done}finally{clearTimeout(timer)}}
async function api(c,route,body,status=200,headers={}) {
  const response=await fetch(c.base+route,{method:body===undefined?"GET":"POST",headers:{Authorization:"Bearer "+c.token,"Content-Type":"application/json",...headers},body:body===undefined?undefined:JSON.stringify(body),signal:AbortSignal.timeout(15000)});
  const raw=await response.text();assert.equal(response.status,status,raw);assert(!raw.includes(secret));return JSON.parse(raw);
}
const protocol=c=>api(c,"/api/provider-imports/protocol");
const result=(c,input,cancel=false,status=200)=>api(c,"/api/provider-imports/operations/"+(cancel?"cancel":"query"),input,status);
const submit=(c,input,status=200)=>api(c,"/api/provider-imports",input,status);
const record=text=>report.cases.push(text);
async function input(c,model,agent="claude") {
  const p=await protocol(c),candidate=(await api(c,"/api/provider-imports")).find(item=>item.agentId===agent&&item.canImport);assert(candidate);
  return {candidateId:candidate.id,expectedRevision:candidate.revision,model,operation:{storeId:p.storeId,namespaceId:p.namespaceId,operationId:randomUUID(),expectedSequence:p.sequence}};
}
const credential=(c,input)=>api(c,`/api/provider-credential-operations/v3/${input.operation.operationId}?storeId=${input.operation.storeId}&namespaceId=${input.operation.namespaceId}`);
const counts=c=>sql(c,db=>({providers:db.prepare("SELECT count(*) n FROM providers").get().n,scope:db.prepare("SELECT * FROM native_import_scope").get(),values:db.prepare("SELECT domain,name,hex(value) value,value_hash,revision FROM credential_recovery_values ORDER BY domain,name").all()}));
async function refused(executable,data) {
  const child=spawn(executable,["--data-dir",data],{env:{...process.env,LOCAL_AGENT_SOURCE_HOME:home,LOCAL_AGENT_SERVICE_TOKEN:randomUUID(),LOCAL_AGENT_TEST_MEMORY_VAULT:"1",LOCAL_AGENT_TEST_IDLE_AGENTS:"1"},stdio:["ignore","pipe","pipe"]});
  children.add(child);child.once("exit",()=>children.delete(child));let stdout="";child.stdout.on("data",b=>{stdout+=b});child.stderr.resume();
  await Promise.race([once(child,"exit"),new Promise((_,reject)=>{const timer=setTimeout(()=>reject(Error("old/new process did not refuse protected data")),15000);timer.unref();child.once("exit",()=>clearTimeout(timer))})]);
  assert.equal(stdout.trim(),"");assert.notEqual(child.exitCode,0);
}
try {
  let c=await boot(path.join(root,"fresh"));
  let p=await protocol(c);assert.equal(p.version,1);assert.equal(p.operationLimit,4096);assert.equal(p.pendingLimit,4);assert.equal(p.sequence,0);
  assert.equal(sql(c,db=>db.prepare("PRAGMA user_version").get().user_version),16);
  const first=await input(c,"concurrent-model");
  const responses=await Promise.all(Array.from({length:8},()=>submit(c,first)));
  assert.equal(new Set(responses.map(v=>v.provider.credentialRef)).size,1);assert.equal(responses[0].provider.id,"custom-"+first.operation.operationId);
  assert.deepEqual(Object.keys(responses[0]).sort(),["provider","reused"]);assert.equal((await result(c,first)).status,"committed");assert.equal((await protocol(c)).sequence,1);
  const changed=await submit(c,{...first,model:"different-model"},409);assert.equal(changed.code,"operation_conflict");
  assert.equal((await submit(c,{...first,operation:{...first.operation,storeId:randomUUID()}},409)).code,"credential_store_changed");
  assert.equal((await submit(c,{...first,operation:{...first.operation,namespaceId:randomUUID()}},409)).code,"credential_namespace_changed");
  record("eight concurrent same-ID imports share one intent, Provider and credential; changed input and foreign source are rejected");
  const expired={...first,operation:{...first.operation,operationId:randomUUID()}},beforeExpired=counts(c);
  assert.equal((await result(c,expired)).status,"expired");assert.equal((await result(c,expired,true)).status,"expired");
  assert.equal((await submit(c,expired,409)).code,"native_import_expired");assert.deepEqual(counts(c),beforeExpired);
  record("a different request that lost the sequence race is proved expired by the retained occupying intent; query and cancel never invent execution or permanently strand its local recovery hint");

  const lost=await input(c,"lost-http-reply");
  await new Promise((resolve,reject)=>{const req=httpRequest(c.base+"/api/provider-imports",{method:"POST",headers:{Authorization:"Bearer "+c.token,"Content-Type":"application/json"}},res=>{try{assert.equal(res.statusCode,200);res.destroy();resolve()}catch(e){res.resume();reject(e)}});req.on("error",reject);req.setTimeout(15000,()=>req.destroy(Error("HTTP reply timeout")));req.end(JSON.stringify(lost))});
  const recovered=await result(c,lost);assert.equal(recovered.status,"committed");assert.equal(recovered.provider.credentialRef,(await credential(c,lost)).operation.receipt.credentialRef);
  fs.unlinkSync(path.join(home,".claude/settings.json"));assert.equal((await submit(c,lost)).provider.id,recovered.provider.id);fs.writeFileSync(path.join(home,".claude/settings.json"),source);
  record("a disconnected HTTP success is independently queried and replays the recorded result after source removal without a second import");

  const before=await input(c,"cancel-before-late");assert.equal((await result(c,before,true)).status,"cancelled");assert.equal((await submit(c,before,409)).code,"native_import_cancelled");
  record("a durable cancellation before a late submission prevents any Provider or credential creation");

  for(const [name,trigger] of [["intent","BEFORE INSERT ON native_import_operations"],["clock","BEFORE UPDATE ON native_import_scope"]]) {
    const target=await input(c,"ignored-"+name),saved=counts(c);sql(c,db=>db.exec(`CREATE TRIGGER ignored_import ${trigger} BEGIN SELECT RAISE(IGNORE);END;`));
    await submit(c,target,503);assert.deepEqual(counts(c),saved);sql(c,db=>db.exec("DROP TRIGGER ignored_import"));
  }
  record("ignored intent and scope writes roll back before any credential recovery bytes are added");

  sql(c,db=>db.exec("CREATE TRIGGER ignored_provider BEFORE INSERT ON providers BEGIN SELECT RAISE(IGNORE);END;"));
  const staged=await input(c,"pending-source-change"),unknown=await submit(c,staged,503);assert.equal(unknown.code,"native_import_result_unknown");assert.equal(unknown.outcomeUnknown,true);
  const pending=await result(c,staged);assert.equal(pending.status,"pending");const reference=pending.provider.credentialRef;assert.equal(reference,(await credential(c,staged)).operation.receipt.credentialRef);
  assert((await protocol(c)).pendingRequests.some(v=>v.operation.operationId===staged.operation.operationId));
  fs.writeFileSync(path.join(home,".claude/settings.json"),JSON.stringify({env:{ANTHROPIC_API_KEY:"changed-offline-native-key",ANTHROPIC_MODEL:"native-model"}}));
  assert.equal((await submit(c,staged,409)).code,"conflict");assert.equal((await result(c,staged,true)).status,"cancelled");assert.equal((await credential(c,staged)).operation.receipt.credentialRef,reference);
  fs.writeFileSync(path.join(home,".claude/settings.json"),source);sql(c,db=>db.exec("DROP TRIGGER ignored_provider"));
  record("a pending owned credential stays independently recoverable; changed source cannot resume, and cancellation preserves the original credential");

  sql(c,db=>db.exec("CREATE TRIGGER rewritten_provider AFTER INSERT ON providers BEGIN UPDATE providers SET value=json_set(value,'$.name','changed') WHERE id=NEW.id;END;"));
  const rewritten=await input(c,"provider-rewrite"),saved=counts(c).providers;await submit(c,rewritten,503);assert.equal(counts(c).providers,saved);const original=(await result(c,rewritten)).provider.credentialRef;
  sql(c,db=>db.exec("DROP TRIGGER rewritten_provider"));assert.equal((await submit(c,rewritten)).provider.credentialRef,original);
  record("a trigger rewriting Provider metadata rolls back the final transaction; explicit continuation reuses the original staged reference");

  sql(c,db=>db.exec("CREATE TRIGGER ignored_provider BEFORE INSERT ON providers BEGIN SELECT RAISE(IGNORE);END;"));
  const noId=await input(c,"compatibility-no-id");delete noId.operation;await submit(c,noId,503);const discovered=(await protocol(c)).pendingRequests.find(v=>v.model==="compatibility-no-id");assert(discovered?.operation);await result(c,discovered,true);
  const full=[];for(let i=0;i<4;i++){const target=await input(c,"capacity-"+i);await submit(c,target,503);full.push(target)}
  const overflow=await input(c,"overflow"),capacity=counts(c);assert.equal((await submit(c,overflow,400)).code,"capacity");assert.deepEqual(counts(c),capacity);
  await result(c,full[0],true);sql(c,db=>db.exec("DROP TRIGGER ignored_provider"));await submit(c,overflow);
  for(const target of full.slice(1))await result(c,target,true);
  record("unnumbered compatibility failures expose their generated original request; four pending intents block a fifth external write until an explicit cancellation releases a slot");

  const hostile=await input(c,"hostile-origin"),protectedState=counts(c);await submit(c,hostile,200); // A normal admitted request remains available.
  const foreign=await input(c,"foreign-origin");await api(c,"/api/provider-imports",foreign,401,{Origin:"https://foreign.invalid"});assert.equal((await protocol(c)).sequence,protectedState.scope.sequence+1);
  assert.equal(fs.readFileSync(path.join(home,".claude/settings.json"),"utf8"),source);
  record("management Origin protection rejects a foreign import and every native source remains byte-identical");

  fs.writeFileSync(path.join(home,".codex/config.toml"),"model='local-model'\nmodel_provider='local'\n[model_providers.local]\nbase_url='http://localhost:11434/v1'\nwire_api='chat'\n");
  sql(c,db=>db.exec("CREATE TRIGGER ignored_provider BEFORE INSERT ON providers BEGIN SELECT RAISE(IGNORE);END;"));
  const local=await input(c,"restart-local","codex");await submit(c,local,503);await stop(c.child);c=await boot(c.data);assert.equal((await result(c,local)).status,"pending");sql(c,db=>db.exec("DROP TRIGGER ignored_provider"));
  const localSaved=await submit(c,local);assert.equal(localSaved.provider.credentialRef,null);assert.equal(localSaved.provider.id,"custom-"+local.operation.operationId);
  record("an actual service restart preserves the original no-auth import intent and completes it only after explicit continuation");
  report.mainSchema=16;await stop(c.child);

  if(previous) {
    const data=path.join(root,"old-12");const old=await boot(data,previous);const oldSnapshot=await api(old,"/api/snapshot");const storeId=oldSnapshot.settingsWriteProtocol.storeId;
    const oldScope=await api(old,`/api/provider-credentials/v3/protocol?storeId=${storeId}`);
    await api(old,"/api/provider-credentials/v3",{storeId,namespaceId:oldScope.namespaceId,operationId:randomUUID(),label:"Previous schema-12 owned key",secret});
    const historicalCandidate=(await api(old,"/api/provider-imports")).find(v=>v.agentId==="claude"&&v.canImport);
    await api(old,"/api/provider-imports",{candidateId:historicalCandidate.id,expectedRevision:historicalCandidate.revision,model:"historical-native-import"});
    const originalState=sql(old,db=>({scope:db.prepare("SELECT * FROM credential_recovery_scope").all(),values:db.prepare("SELECT domain,name,hex(value) value,value_hash,revision FROM credential_recovery_values ORDER BY domain,name").all(),legacy:db.prepare("SELECT * FROM credential_legacy_scope").all(),providers:db.prepare("SELECT id,value FROM providers ORDER BY id").all()}));assert(originalState.values.length>=2);assert(originalState.providers.length>0);
    assert.equal(sql(old,db=>db.prepare("PRAGMA user_version").get().user_version),12);await refused(binary,data);assert(!fs.existsSync(path.join(data,"local-agent.pre-native-import-v12.sqlite3")));await stop(old.child);
    const upgraded=await boot(data);const upgradedProtocol=await protocol(upgraded);assert.equal(upgradedProtocol.storeId,storeId);assert.equal(upgradedProtocol.namespaceId,oldScope.namespaceId);
    const after=sql(upgraded,db=>({scope:db.prepare("SELECT * FROM credential_recovery_scope").all(),values:db.prepare("SELECT domain,name,hex(value) value,value_hash,revision FROM credential_recovery_values ORDER BY domain,name").all(),legacy:db.prepare("SELECT * FROM credential_legacy_scope").all(),providers:db.prepare("SELECT id,value FROM providers ORDER BY id").all()}));assert.deepEqual(after,originalState);assert.equal(sql(upgraded,db=>db.prepare("SELECT count(*) n FROM native_import_operations").get().n),0);
    const backupPath=path.join(data,"local-agent.pre-native-import-v12.sqlite3"),backup=new DatabaseSync(backupPath,{readOnly:true});try{assert.equal(backup.prepare("PRAGMA user_version").get().user_version,12);assert.equal(backup.prepare("PRAGMA quick_check").get().quick_check,"ok")}finally{backup.close()}
    assert.equal(fs.statSync(backupPath).mode&0o777,0o600);await refused(previous,data);await stop(upgraded.child);await refused(previous,data);report.actualPreviousSchema12UpgradeAndReaderBarrier=true;
    record("the actual signed schema-12 service owns its running lock; upgrade preserves recovery bytes and identity with a private verified backup, and the old writer refuses schema 16 both live and after shutdown");
  }
  report.passed=true;
} finally {
  await Promise.allSettled([...children].map(stop));assert.equal(children.size,0);report.allOwnedProcessesExited=true;
  fs.rmSync(root,{recursive:true,force:true});report.temporaryRootRemoved=!fs.existsSync(root);
  const destination=process.env.NATIVE_IMPORT_REPORT;if(destination)fs.writeFileSync(path.resolve(destination),JSON.stringify(report,null,2)+"\n");
}
console.log(JSON.stringify(report,null,2));
