import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import http from "node:http";
import {tmpdir} from "node:os";
import {spawn} from "node:child_process";
import {once} from "node:events";
import {createInterface} from "node:readline";
import {createHash, randomUUID} from "node:crypto";
import {DatabaseSync} from "node:sqlite";

const binary = path.resolve(process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ?? "target/debug/local-agent-service");
const previous = process.env.LOCAL_TOKEN_CANCELLATION_PREVIOUS_BINARY && path.resolve(process.env.LOCAL_TOKEN_CANCELLATION_PREVIOUS_BINARY);
const root = fs.realpathSync(fs.mkdtempSync(path.join(tmpdir(), "local-token-operations-")));
const hash = file => createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const report = {passed: false, binarySha256: hash(binary), cases: [], realKeychainCalls: 0, realModelCalls: 0, simulatedUpstreamCalls: 0};
const children = new Set();
const upstream = http.createServer(async (request, response) => {
  let raw = ""; for await (const bytes of request) raw += bytes;
  const body = JSON.parse(raw); report.simulatedUpstreamCalls++;
  response.writeHead(200, {"content-type": "application/json"});
  response.end(JSON.stringify({id: randomUUID(), type: "message", role: "assistant", model: body.model, content: [{type: "text", text: "OK"}], stop_reason: "end_turn", usage: {input_tokens: 5, output_tokens: 2}}));
});
await new Promise(resolve => upstream.listen(0, "127.0.0.1", resolve));
const record = text => report.cases.push(text);
function sql(context, fn) {const db = new DatabaseSync(path.join(context.data, "local-agent.sqlite3")); try {return fn(db);} finally {db.close();}}
async function stop(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const done = once(child, "exit"), timer = setTimeout(() => child.kill("SIGKILL"), 8000); timer.unref(); child.kill("SIGTERM");
  try {await done;} finally {clearTimeout(timer);}
}
async function boot(context, executable = binary) {
  const child = spawn(executable, ["--data-dir", context.data], {env: {...process.env, LOCAL_AGENT_SOURCE_HOME: context.home, LOCAL_AGENT_SERVICE_TOKEN: context.token, LOCAL_AGENT_TEST_MEMORY_VAULT: "1", LOCAL_AGENT_TEST_IDLE_AGENTS: "1"}, stdio: ["ignore", "pipe", "pipe"]});
  children.add(child); child.once("exit", () => children.delete(child)); context.child = child;
  let stderr = ""; child.stderr.on("data", bytes => {stderr += bytes;});
  const lines = createInterface({input: child.stdout});
  try {const [line] = await Promise.race([once(lines, "line", {signal: AbortSignal.timeout(15000)}), once(child, "exit").then(() => {throw Error(stderr);})]); context.base = "http://127.0.0.1:" + JSON.parse(line).port;}
  finally {lines.close();}
  return context;
}
async function api(context, route, body, status = 200, method = body === undefined ? "GET" : "POST", headers = {}) {
  const response = await fetch(context.base + route, {method, headers: {Authorization: "Bearer " + context.token, "content-type": "application/json", ...headers}, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(15000)});
  const raw = await response.text(); assert.equal(response.status, status, raw); return JSON.parse(raw);
}
function context(name) {
  const folder = path.join(root, name), home = path.join(folder, "home"), data = path.join(folder, "data");
  fs.mkdirSync(path.join(home, ".claude"), {recursive: true}); fs.mkdirSync(path.join(home, ".codex"));
  const file = path.join(home, ".claude/settings.json"); fs.writeFileSync(file, '{"env":{"KEEP_ME":"yes"},"permissions":{"allow":["Read"]}}\n', {mode: 0o600});
  fs.writeFileSync(path.join(home, ".codex/config.toml"), "model='fixture'\nmodel_provider='local'\n[model_providers.local]\nbase_url='http://localhost:11434/v1'\nwire_api='chat'\n", {mode: 0o600});
  return {home, data, file, token: randomUUID(), sourceHash: hash(file)};
}
const protocol = context => api(context, "/api/local-tokens/protocol");
const query = (context, identity, status = 200) => api(context, "/api/local-tokens/query", identity, status);
const resume = (context, identity, status = 200) => api(context, "/api/local-tokens/continue", identity, status);
const cancel = (context, identity, status = 200) => api(context, "/api/local-tokens/cancel", identity, status);
const bindings = context => sql(context, db => Object.fromEntries(["gateway_bindings", "gateway_route_versions", "model_service_bindings"].map(table => [table, db.prepare(`SELECT count(*) n FROM ${table}`).get().n])));
const tokenValues = context => sql(context, db => db.prepare("SELECT domain,name,hex(value) value,value_hash,revision FROM local_token_values ORDER BY domain,name").all());
const originalMaterial = context => sql(context, db => Object.fromEntries(["credential_recovery_scope", "credential_recovery_values", "credential_legacy_scope", "credential_legacy_operations", "native_import_scope", "native_import_operations", "providers", "provider_operation_scope", "provider_operations"].map(table => [table, db.prepare(`SELECT * FROM ${table} ORDER BY rowid`).all()])));
async function fixture(name, executable = binary, expectedSchema = 15) {
  const c = await boot(context(name), executable);
  const p = {id: "custom-" + randomUUID(), name: "Owned local token", vendor: "custom", endpoint: "http://127.0.0.1:" + upstream.address().port, protocol: "messages", model: "fixture-model", models: ["fixture-model"], disabledModels: [], configured: true, revision: 0, credentialRef: null, credentialLabel: null, category: "local"};
  await api(c, "/api/providers", p, 200, "PUT"); c.provider = {...p, revision: 1};
  assert.equal((await api(c, `/api/providers/${p.id}/verify`, {expectedRevision: 1, model: p.model})).state, "verified");
  const modelId = randomUUID(); c.service = await api(c, "/api/model-services", {id: randomUUID(), revision: 0, name: "Owned service", defaultModelId: modelId, models: [{id: modelId, name: "Default", providerId: p.id, model: p.model, capabilities: {contextWindow: 32768, maxOutputTokens: 8192, tools: true, thinking: false, inputModalities: ["text"]}}]}, 200, "PUT");
  c.config = (await api(c, "/api/provider-status")).configs.find(v => v.agentId === "claude");
  const pcol = await protocol(c); c.identity = {storeId: pcol.storeId, namespaceId: pcol.namespaceId, operationId: randomUUID()};
  assert.equal(pcol.schemaVersion, expectedSchema); return c;
}
function input(c, purpose) {return purpose === "model_service" ? {agentId: "claude", expectedRevision: c.config.revision, serviceId: c.service.id, expectedServiceRevision: c.service.revision, operation: c.identity} : {agentId: "claude", expectedRevision: c.config.revision, providerId: c.provider.id, expectedProviderRevision: 1, model: c.provider.model, operation: c.identity};}
const routes = {fixed_route: "/api/provider-switches", dynamic_binding: "/api/dynamic-connections", model_service: "/api/model-service-connections"};
async function refused(executable, c) {
  const child = spawn(executable, ["--data-dir", c.data], {env: {...process.env, LOCAL_AGENT_SOURCE_HOME: c.home, LOCAL_AGENT_SERVICE_TOKEN: randomUUID(), LOCAL_AGENT_TEST_MEMORY_VAULT: "1", LOCAL_AGENT_TEST_IDLE_AGENTS: "1"}, stdio: ["ignore", "pipe", "pipe"]});
  children.add(child); child.once("exit", () => children.delete(child)); let output = ""; child.stdout.on("data", bytes => {output += bytes;}); child.stderr.resume();
  await Promise.race([once(child, "exit"), new Promise((_, reject) => {const timer = setTimeout(() => reject(Error("reader barrier timeout")), 15000); timer.unref(); child.once("exit", () => clearTimeout(timer));})]);
  assert.equal(output.trim(), ""); assert.notEqual(child.exitCode, 0);
}

const fence = (c,id=c.identity) => sql(c,db=>db.prepare("SELECT operation_id,store_id,namespace_id,hex(value) value,value_hash FROM local_token_cancellations WHERE operation_id=?").get(id.operationId));
const watermark = c => sql(c,db=>db.prepare("SELECT cancellation_count,cancellation_bytes,owner_count,pending_count FROM local_token_scope").get());
function assertProof(value,id) {assert.equal(value.version,2);assert.deepEqual(value.cancellation,{version:1,identity:id});assert.equal(value.operation,null);assert.equal(value.preview,null);}
try {
 for (const purpose of Object.keys(routes)) {
  const c=await fixture('cancel-'+purpose),original=originalMaterial(c),calls=report.simulatedUpstreamCalls,before=hash(c.file);
  const proof=await cancel(c,c.identity);assertProof(proof,c.identity);assertProof(await query(c,c.identity),c.identity);const saved=fence(c),clock=watermark(c);assert.equal(clock.cancellation_count,1);assert.equal(clock.pending_count,0);assert.equal(clock.owner_count,0);assert.deepEqual(tokenValues(c),[]);
  for(const latePurpose of Object.keys(routes)) {const late=input(c,latePurpose);if(latePurpose==='model_service')late.serviceId=randomUUID();else late.providerId='custom-'+randomUUID();assert.equal((await api(c,routes[latePurpose],late,409)).code,'local_token_cancelled');}
  assert.equal((await resume(c,c.identity,409)).code,'local_token_cancelled');assertProof(await cancel(c,c.identity),c.identity);assert.deepEqual(fence(c),saved);assert.deepEqual(watermark(c),clock);assert.deepEqual(originalMaterial(c),original);assert.equal(report.simulatedUpstreamCalls,calls);assert.equal(hash(c.file),before);
  const cancelledIdentity=c.identity;await stop(c.child);await boot(c);assertProof(await query(c,c.identity),c.identity);assert.equal((await api(c,routes[purpose],input(c,purpose),409)).code,'local_token_cancelled');assert.deepEqual(fence(c),saved);
  c.identity={...c.identity,operationId:randomUUID()};const fresh=await api(c,routes[purpose],input(c,purpose));assert.equal(fresh.id,c.identity.operationId);assertProof(await query(c,cancelledIdentity),cancelledIdentity);assert.equal(watermark(c).cancellation_count,1);
  record(purpose+': durable same-ID cancellation survives actual restart, blocks all three late purposes before target lookup or Vault writes, and does not consume pending capacity or block an explicit fresh ID');await stop(c.child);
 }
 const reused=await fixture('fixed-reuse');const originalPreview=await api(reused,routes.fixed_route,input(reused,'fixed_route'));await cancel(reused,reused.identity);const before=tokenValues(reused),bindingsBefore=bindings(reused);reused.identity={...reused.identity,operationId:randomUUID()};assertProof(await cancel(reused,reused.identity),reused.identity);assert.equal((await api(reused,routes.fixed_route,input(reused,'fixed_route'),409)).code,'local_token_cancelled');assert.deepEqual(tokenValues(reused),before);assert.deepEqual(bindings(reused),bindingsBefore);const native=(await api(reused,'/api/provider-status')).transactions;assert(native.some(t=>t.id===originalPreview.id&&t.state==='cancelled'));assert(!native.some(t=>t.id===reused.identity.operationId));record('fixed-route reuse checks the same durable fence even though it creates no new SQL owner or credential; original route and cancelled preview are retained');await stop(reused.child);
 for(const [name,trigger] of [['row','BEFORE INSERT ON local_token_cancellations BEGIN SELECT RAISE(IGNORE);END;'],['watermark','BEFORE UPDATE ON local_token_scope BEGIN SELECT RAISE(IGNORE);END;'],['rewrite',"AFTER INSERT ON local_token_cancellations BEGIN UPDATE local_token_cancellations SET value_hash=printf('%064d',0) WHERE operation_id=NEW.operation_id;END;"]]) {
  const c=await fixture('fault-'+name);sql(c,db=>db.exec('CREATE TRIGGER fault '+trigger));await cancel(c,c.identity,503);assert.equal(watermark(c).cancellation_count,0);assert.equal(fence(c),undefined);assert.deepEqual(tokenValues(c),[]);sql(c,db=>db.exec('DROP TRIGGER fault'));assertProof(await cancel(c,c.identity),c.identity);assert.equal((await api(c,routes.dynamic_binding,input(c,'dynamic_binding'),409)).code,'local_token_cancelled');await stop(c.child);
 }
 record('ignored row/watermark and rewritten proof writes roll back; no acknowledgement, proof or external credential is invented, and explicit retry commits a checked proof');
 const damaged=await fixture('damaged-proof');await cancel(damaged,damaged.identity);sql(damaged,db=>db.exec('DELETE FROM local_token_cancellations'));await query(damaged,damaged.identity,503);await api(damaged,routes.dynamic_binding,input(damaged,'dynamic_binding'),503);assert.deepEqual(tokenValues(damaged),[]);assert.equal(watermark(damaged).cancellation_count,1);record('a deleted proof cannot silently reset admission because its retained watermark rejects query and late creation');await stop(damaged.child);
 const indexed=await fixture('corrupt-index');await cancel(indexed,indexed.identity);sql(indexed,db=>{assert.throws(()=>db.exec('UPDATE local_token_cancellations SET operation_id=NULL'),/NOT NULL/);db.prepare('UPDATE local_token_cancellations SET operation_id=?').run(randomUUID());});await query(indexed,indexed.identity,503);await api(indexed,routes.dynamic_binding,input(indexed,'dynamic_binding'),503);assert.deepEqual(tokenValues(indexed),[]);assert.equal(watermark(indexed).owner_count,0);record('NULL cancellation indexes are constrained and a retargeted index with original proof bytes makes the complete small ledger unverified before absence can permit a target write');await stop(indexed.child);
 const full=await fixture('capacity');let first;sql(full,db=>{db.exec('BEGIN IMMEDIATE');const insert=db.prepare('INSERT INTO local_token_cancellations VALUES(?,?,?,?,?)');let bytes=0;for(let n=0;n<4096;n++){const id={...full.identity,operationId:randomUUID()};first??=id;const raw=Buffer.from(JSON.stringify({version:1,identity:id}));bytes+=raw.length;insert.run(id.operationId,id.storeId,id.namespaceId,raw,createHash('sha256').update(raw).digest('hex'));}db.prepare('UPDATE local_token_scope SET cancellation_count=4096,cancellation_bytes=?').run(bytes);db.exec('COMMIT');});assert.equal((await cancel(full,full.identity,400)).code,'capacity');assertProof(await cancel(full,first),first);assert.equal(watermark(full).cancellation_count,4096);assert.deepEqual(tokenValues(full),[]);record('4096 proofs remain valid and replayable; a new cancellation at capacity is explicitly refused without reclaiming original evidence or touching Vault');await stop(full.child);
 for(let n=0;n<6;n++) {
  const c=await fixture('race-'+n),draft=input(c,'dynamic_binding');const creation=fetch(c.base+routes.dynamic_binding,{method:'POST',headers:{Authorization:'Bearer '+c.token,'content-type':'application/json'},body:JSON.stringify(draft),signal:AbortSignal.timeout(15000)}).then(async r=>({status:r.status,body:await r.json()}));
  const [created,cancelled]=await Promise.all([creation,cancel(c,c.identity)]);
  if(cancelled.cancellation){assertProof(cancelled,c.identity);assert.equal(created.status,409);assert.equal(created.body.code,'local_token_cancelled');assert.equal(watermark(c).owner_count,0);assert.deepEqual(tokenValues(c),[]);}else{assert.equal(created.status,200);assert.equal(cancelled.preview.id,c.identity.operationId);assert.equal(cancelled.preview.state,'cancelled');assert.equal((await query(c,c.identity)).credential.status,'ready');assert.equal(watermark(c).cancellation_count,0);}
  await stop(c.child);
 }
 record('six actual HTTP admission/cancellation races resolve to either a blocking proof with no owner or an admitted cancelled native preview retaining its ready credential');
 const auth=await fixture('auth');const beforeAuth=watermark(auth);await api(auth,'/api/local-tokens/cancel',auth.identity,401,'POST',{Origin:'https://foreign.invalid'});await api(auth,'/api/local-tokens/cancel',auth.identity,401,'POST',{Authorization:'Bearer invalid'});await cancel(auth,{...auth.identity,storeId:randomUUID()},409);await cancel(auth,{...auth.identity,namespaceId:randomUUID()},409);assert.deepEqual(watermark(auth),beforeAuth);assert.deepEqual(tokenValues(auth),[]);record('management token, Origin and original store/namespace checks refuse foreign cancellation before any evidence or credential write');await stop(auth.child);
 if(previous) {
  const old=await fixture('actual-schema14',previous,14);const original=originalMaterial(old),sourceBytes=sql(old,db=>db.prepare('SELECT value FROM providers WHERE id=?').get(old.provider.id).value);
  sql(old,db=>db.exec("CREATE TRIGGER before_key AFTER INSERT ON local_token_owners BEGIN UPDATE providers SET value=json_set(value,'$.name','changed after intent') WHERE id=json_extract(NEW.value,'$.operation.request.provider.id');END;"));await api(old,routes.dynamic_binding,input(old,'dynamic_binding'),409);sql(old,db=>{db.exec('DROP TRIGGER before_key');db.prepare('UPDATE providers SET value=? WHERE id=?').run(sourceBytes,old.provider.id);});
  assert.deepEqual(tokenValues(old),[]);const owners=sql(old,db=>db.prepare('SELECT * FROM local_token_owners ORDER BY rowid').all());assert.equal(owners.length,1);assert.equal((await protocol(old)).version,1);assert.equal(sql(old,db=>db.prepare('SELECT format FROM local_token_scope').get().format),1);
  await refused(binary,old);assert(!fs.existsSync(path.join(old.data,'local-agent.pre-local-token-admission-v14.sqlite3')));await stop(old.child);await boot(old);const current=await protocol(old);assert.equal(current.version,2);assert.equal(current.schemaVersion,15);assert.equal(current.cancellationLimit,4096);assert.equal(current.storeId,old.identity.storeId);assert.equal(current.namespaceId,old.identity.namespaceId);assert.deepEqual(originalMaterial(old),original);assert.deepEqual(sql(old,db=>db.prepare('SELECT * FROM local_token_owners ORDER BY rowid').all()),owners);assert.deepEqual(tokenValues(old),[]);
  const backupPath=path.join(old.data,'local-agent.pre-local-token-admission-v14.sqlite3'),backup=new DatabaseSync(backupPath,{readOnly:true});try{assert.equal(backup.prepare('PRAGMA user_version').get().user_version,14);assert.equal(backup.prepare('PRAGMA quick_check').get().quick_check,'ok');assert.equal(backup.prepare('SELECT format FROM local_token_scope').get().format,1);assert.deepEqual(backup.prepare('SELECT * FROM local_token_owners ORDER BY rowid').all(),owners);}finally{backup.close();}assert.equal(fs.statSync(backupPath).mode&0o777,0o600);assert.equal(sql(old,db=>db.prepare('SELECT format FROM local_token_scope').get().format),2);
  const resumed=await resume(old,old.identity);assert.equal(resumed.preview.id,old.identity.operationId);const id={...old.identity,operationId:randomUUID()};assertProof(await cancel(old,id),id);const saved=fence(old,id);await refused(previous,old);await stop(old.child);await refused(previous,old);await boot(old);assertProof(await query(old,id),id);assert.deepEqual(fence(old,id),saved);await stop(old.child);
  report.actualSchema14Upgrade={previousBinarySha256:hash(previous),targetSchema:15,originalOwnerAndApiMaterialUnchanged:true,oldWriterRefused:true,privateBackupVerified:true};record('actual signed schema-14 intent survives private-backup upgrade to 15 without rewriting owner/API material, continues only explicitly, and old writers refuse the new cancellation evidence both live and after shutdown');
 }
 assert.equal(hash(binary),report.binarySha256);report.passed=true;
}finally {
 for(const child of [...children])await stop(child);await new Promise(resolve=>upstream.close(resolve));assert.equal(children.size,0);report.allOwnedProcessesExited=true;fs.rmSync(root,{recursive:true,force:true});report.temporaryRootRemoved=!fs.existsSync(root);
 if(process.env.LOCAL_TOKEN_CANCELLATION_REPORT)fs.writeFileSync(path.resolve(process.env.LOCAL_TOKEN_CANCELLATION_REPORT),JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report,null,2));
}
