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
const previous = process.env.LOCAL_TOKEN_PREVIOUS_BINARY && path.resolve(process.env.LOCAL_TOKEN_PREVIOUS_BINARY);
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
async function fixture(name) {
  const c = await boot(context(name));
  const p = {id: "custom-" + randomUUID(), name: "Owned local token", vendor: "custom", endpoint: "http://127.0.0.1:" + upstream.address().port, protocol: "messages", model: "fixture-model", models: ["fixture-model"], disabledModels: [], configured: true, revision: 0, credentialRef: null, credentialLabel: null, category: "local"};
  await api(c, "/api/providers", p, 200, "PUT"); c.provider = {...p, revision: 1};
  assert.equal((await api(c, `/api/providers/${p.id}/verify`, {expectedRevision: 1, model: p.model})).state, "verified");
  const modelId = randomUUID(); c.service = await api(c, "/api/model-services", {id: randomUUID(), revision: 0, name: "Owned service", defaultModelId: modelId, models: [{id: modelId, name: "Default", providerId: p.id, model: p.model, capabilities: {contextWindow: 32768, maxOutputTokens: 8192, tools: true, thinking: false, inputModalities: ["text"]}}]}, 200, "PUT");
  c.config = (await api(c, "/api/provider-status")).configs.find(v => v.agentId === "claude");
  const pcol = await protocol(c); c.identity = {storeId: pcol.storeId, namespaceId: pcol.namespaceId, operationId: randomUUID()};
  assert.equal(pcol.schemaVersion, 16); return c;
}
function input(c, purpose) {return purpose === "model_service" ? {agentId: "claude", expectedRevision: c.config.revision, serviceId: c.service.id, expectedServiceRevision: c.service.revision, operation: c.identity} : {agentId: "claude", expectedRevision: c.config.revision, providerId: c.provider.id, expectedProviderRevision: 1, model: c.provider.model, operation: c.identity};}
const routes = {fixed_route: "/api/provider-switches", dynamic_binding: "/api/dynamic-connections", model_service: "/api/model-service-connections"};
async function refused(executable, c) {
  const child = spawn(executable, ["--data-dir", c.data], {env: {...process.env, LOCAL_AGENT_SOURCE_HOME: c.home, LOCAL_AGENT_SERVICE_TOKEN: randomUUID(), LOCAL_AGENT_TEST_MEMORY_VAULT: "1", LOCAL_AGENT_TEST_IDLE_AGENTS: "1"}, stdio: ["ignore", "pipe", "pipe"]});
  children.add(child); child.once("exit", () => children.delete(child)); let output = ""; child.stdout.on("data", bytes => {output += bytes;}); child.stderr.resume();
  await Promise.race([once(child, "exit"), new Promise((_, reject) => {const timer = setTimeout(() => reject(Error("reader barrier timeout")), 15000); timer.unref(); child.once("exit", () => clearTimeout(timer));})]);
  assert.equal(output.trim(), ""); assert.notEqual(child.exitCode, 0);
}
try {
  for (const purpose of Object.keys(routes)) {
    const c = await fixture(purpose), request = input(c, purpose), nativeBefore = hash(c.file), old = originalMaterial(c), calls = report.simulatedUpstreamCalls;
    const plan = path.join(c.data, "provider-configs", c.identity.operationId + ".sealed");
    if (purpose === "fixed_route") fs.writeFileSync(plan, "retained unknown material", {mode: 0o600});
    else sql(c, db => db.exec(`CREATE TRIGGER fault BEFORE INSERT ON ${purpose === "model_service" ? "model_service_bindings" : "gateway_bindings"} BEGIN SELECT RAISE(IGNORE); END;`));
    assert.equal((await api(c, routes[purpose], request, 503)).code, "local_token_result_unknown");
    const pending = await query(c, c.identity); assert.equal(pending.operation.status, "pending"); assert.equal(pending.operation.request.purpose, purpose); assert.equal(pending.credential.status, "ready"); assert.equal(pending.preview, null);
    const originalReference = pending.operation.credentialRef, savedTokenValues = tokenValues(c);
    assert.equal((await protocol(c)).pending[0].ownerId, c.identity.operationId); assert.equal(pending.credential.receipt.credentialRef, originalReference);
    if (purpose !== "fixed_route") assert.deepEqual(bindings(c), {gateway_bindings: 0, gateway_route_versions: 0, model_service_bindings: 0});
    if (purpose === "fixed_route") fs.unlinkSync(plan); else sql(c, db => db.exec("DROP TRIGGER fault"));
    assert.equal((await api(c, routes[purpose], request, 503)).code, "local_token_result_unknown");
    assert.deepEqual(tokenValues(c), savedTokenValues);
    fs.writeFileSync(c.file, '{"env":{"KEEP_ME":"changed"}}\n');
    assert.equal((await query(c, c.identity)).canContinue, false); await resume(c, c.identity, 409); assert.deepEqual(tokenValues(c), savedTokenValues);
    fs.writeFileSync(c.file, '{"env":{"KEEP_ME":"yes"},"permissions":{"allow":["Read"]}}\n'); assert.equal(hash(c.file), nativeBefore);
    assert.equal((await query(c, c.identity)).canContinue, true);
    const continued = await resume(c, c.identity); assert.equal(continued.operation.status, "committed"); assert.equal(continued.operation.credentialRef, originalReference); assert.equal(continued.preview.id, c.identity.operationId); assert.equal(continued.preview.state, "pending");
    assert.deepEqual(tokenValues(c), savedTokenValues); assert.deepEqual(originalMaterial(c), old); assert.equal(report.simulatedUpstreamCalls, calls); assert.equal(hash(c.file), nativeBefore);
    const replay = await api(c, routes[purpose], request); assert.equal(replay.id, c.identity.operationId);
    assert.equal((await query(c, c.identity)).preview.id, c.identity.operationId);
    assert.equal((await cancel(c, c.identity)).preview.state, "cancelled"); assert.deepEqual(tokenValues(c), savedTokenValues);
    await query(c, {...c.identity, storeId: randomUUID()}, 409); await query(c, {...c.identity, namespaceId: randomUUID()}, 409);
    const unseen = {...c.identity, operationId: randomUUID()};
    await query(c, unseen, 503);
    const stopped = await cancel(c, unseen); assert.deepEqual(stopped.cancellation, {version: 1, identity: unseen});
    assert.deepEqual((await query(c, unseen)).cancellation, stopped.cancellation);
    assert.equal((await resume(c, unseen, 409)).code, "local_token_cancelled");
    assert.equal(hash(c.file), nativeBefore); record(`${purpose}: staged ready credential survives owner/preview failure; ordinary replay does not write, changed source blocks, explicit same-ID continuation preserves token and finds native preview, cancellation retains token and owner`);
    await stop(c.child);
  }
  const c = await fixture("before-external-write"), request = input(c, "dynamic_binding");
  const providerBytes = sql(c, db => db.prepare("SELECT value FROM providers WHERE id=?").get(c.provider.id).value);
  sql(c, db => db.exec("CREATE TRIGGER fault AFTER INSERT ON local_token_owners BEGIN UPDATE providers SET value=json_set(value,'$.name','source changed after intent') WHERE id=json_extract(NEW.value,'$.operation.request.provider.id');END;"));
  await api(c, routes.dynamic_binding, request, 409); assert.equal(tokenValues(c).length, 0);
  assert.equal((await query(c, c.identity)).credential, null); const nativeHash = hash(c.file);
  await stop(c.child); await boot(c); assert.equal((await query(c, c.identity)).operation.ownerId, c.identity.operationId);
  sql(c, db => {db.exec("DROP TRIGGER fault"); db.prepare("UPDATE providers SET value=? WHERE id=?").run(providerBytes, c.provider.id);}); assert.equal((await resume(c, c.identity)).preview.id, c.identity.operationId); assert.equal(hash(c.file), nativeHash);
  record("actual process restart before target credential creation preserves intent and continues only on explicit original-ID request");
  const beforeAuth = tokenValues(c); await api(c, "/api/local-tokens/protocol", undefined, 401, "GET", {Origin: "https://foreign.invalid"}); await api(c, "/api/local-tokens/query", c.identity, 401, "POST", {Authorization: "Bearer invalid"}); assert.deepEqual(tokenValues(c), beforeAuth);
  record("actual management Origin and token checks reject before local-token changes"); await stop(c.child);

  if (previous) {
    const old = await boot(context("actual-schema13"), previous);
    assert.equal(sql(old, db => db.prepare("PRAGMA user_version").get().user_version), 13);
    const snapshot = await api(old, "/api/snapshot"), storeId = snapshot.settingsWriteProtocol.storeId;
    const scope = await api(old, `/api/provider-credentials/v3/protocol?storeId=${storeId}`);
    await api(old, "/api/provider-credentials/v3", {storeId, namespaceId: scope.namespaceId, operationId: randomUUID(), label: "Old owned fixture", secret: "offline-schema13-fixture"});
    const ip = await api(old, "/api/provider-imports/protocol"), candidate = (await api(old, "/api/provider-imports")).find(v => v.agentId === "codex" && v.canImport); assert(candidate);
    await api(old, "/api/provider-imports", {candidateId: candidate.id, expectedRevision: candidate.revision, model: "fixture", operation: {storeId, namespaceId: scope.namespaceId, operationId: randomUUID(), expectedSequence: ip.sequence}});
    const original = originalMaterial(old); await refused(binary, old); assert(!fs.existsSync(path.join(old.data, "local-agent.pre-local-token-v13.sqlite3"))); await stop(old.child);
    await boot(old); const upgraded = await protocol(old); assert.equal(upgraded.storeId, storeId); assert.equal(upgraded.namespaceId, scope.namespaceId); assert.deepEqual(originalMaterial(old), original);
    const backupPath = path.join(old.data, "local-agent.pre-local-token-v13.sqlite3"), backup = new DatabaseSync(backupPath, {readOnly: true});
    try {assert.equal(backup.prepare("PRAGMA user_version").get().user_version, 13); assert.equal(backup.prepare("PRAGMA quick_check").get().quick_check, "ok"); for (const [table, rows] of Object.entries(original)) assert.deepEqual(backup.prepare(`SELECT * FROM ${table} ORDER BY rowid`).all(), rows);} finally {backup.close();}
    assert.equal(fs.statSync(backupPath).mode & 0o777, 0o600); const saved = hash(backupPath); await refused(previous, old); await stop(old.child); await refused(previous, old); assert.equal(hash(backupPath), saved);
    report.actualSchema13Upgrade = {previousBinarySha256: hash(previous), targetSchema: 16, originalMaterialUnchanged: true, privateBackupUnchanged: true, oldWriterRefused: true};
    record("actual signed schema-13 upgrade preserves API-key/native-import/provider bytes and source IDs with a private consistent backup; live ownership and old schema reader barriers are enforced");
  }
  assert.equal(hash(binary), report.binarySha256); report.passed = true;
} finally {
  for (const child of [...children]) await stop(child); await new Promise(resolve => upstream.close(resolve));
  assert.equal(children.size, 0); report.allOwnedProcessesExited = true; fs.rmSync(root, {recursive: true, force: true}); report.temporaryRootRemoved = !fs.existsSync(root);
  if (process.env.LOCAL_TOKEN_REPORT) fs.writeFileSync(path.resolve(process.env.LOCAL_TOKEN_REPORT), JSON.stringify(report, null, 2) + "\n");
  console.log(JSON.stringify(report, null, 2));
}
