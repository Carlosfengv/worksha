import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { request as httpRequest } from "node:http";
import { randomUUID, createHash } from "node:crypto";
import { DatabaseSync } from "node:sqlite";

const binary = path.resolve(process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ?? "target/debug/local-agent-service");
const previous = process.env.LEGACY_ADMISSION_PREVIOUS_BINARY && path.resolve(process.env.LEGACY_ADMISSION_PREVIOUS_BINARY);
const root = fs.realpathSync(fs.mkdtempSync(path.join(tmpdir(), "local-agent-legacy-admission-")));
const home = path.join(root, "home"); let data = path.join(root, "data");
fs.mkdirSync(home);
const hash = file => createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const report = { passed: false, binarySha256: hash(binary), previousBinarySha256: previous ? hash(previous) : null, actualPreviousProcess: false, cases: [], realModelCalls: 0, realKeychainCalls: 0 };
const children = new Set(), secret = "disposable-admission-fixture-71944";
const record = text => report.cases.push(text);
async function boot(executable = binary, ready = true) {
  const token = randomUUID();
  const child = spawn(executable, ["--data-dir", data], { env: { ...process.env, LOCAL_AGENT_SOURCE_HOME: home, LOCAL_AGENT_SERVICE_TOKEN: token, LOCAL_AGENT_TEST_MEMORY_VAULT: "1", LOCAL_AGENT_TEST_IDLE_AGENTS: "1" }, stdio: ["ignore", "pipe", "pipe"] });
  children.add(child); child.once("exit", () => children.delete(child));
  let stderr = ""; child.stderr.on("data", bytes => { stderr += bytes; });
  const lines = createInterface({ input: child.stdout });
  try {
    const event = await Promise.race([
      once(lines, "line", { signal: AbortSignal.timeout(15000) }).then(([line]) => ({ line })),
      once(child, "exit").then(([code]) => ({ code })),
    ]);
    if (!ready) { assert.equal(event.line, undefined, "rejected service announced readiness"); assert.notEqual(event.code, 0, stderr); return stderr; }
    assert(event.line, stderr);
    return { child, token, base: "http://127.0.0.1:" + JSON.parse(event.line).port };
  } finally { lines.close(); }
}
async function stop(c) {
  if (!c || c.exitCode !== null || c.signalCode !== null) return;
  const done = once(c, "exit"), timer = setTimeout(() => c.kill("SIGKILL"), 8000); timer.unref();
  c.kill("SIGTERM"); try { await done; } finally { clearTimeout(timer); }
}
async function api(c, route, body, status = 200, headers = {}) {
  const response = await fetch(c.base + route, { method: body === undefined ? "GET" : "POST", headers: { Authorization: "Bearer " + c.token, "Content-Type": "application/json", ...headers }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(15000) });
  const text = await response.text(); assert.equal(response.status, status, text); assert(!text.includes(secret)); return JSON.parse(text);
}
const sql = fn => { const db = new DatabaseSync(path.join(data, "local-agent.sqlite3")); try { return fn(db); } finally { db.close(); } };
const state = () => sql(db => ({ version: db.prepare("PRAGMA user_version").get().user_version, scope: db.prepare("SELECT * FROM credential_recovery_scope").all(), values: db.prepare("SELECT domain,name,hex(value) value,value_hash,revision FROM credential_recovery_values ORDER BY domain,name").all() }));
const source = id => path.join(data, "provider-configs/credential-operations-v1", id + ".json");
const preview = (p, id) => `/api/provider-credentials/v3/legacy/${id}?storeId=${p.storeId}&namespaceId=${p.namespaceId}`;
const query = (p, id) => `/api/provider-credential-operations/v3/${id}?storeId=${p.storeId}&namespaceId=${p.namespaceId}`;
const confirmation = p => ({ storeId: p.storeId, namespaceId: p.namespaceId, operationId: p.operationId, sourceDigest: p.sourceDigest, confirmed: true });
async function fenced(c, id) {
  for (const [route, body] of [
    ["/api/provider-credentials", { operationId: id, label: "Fixture", secret }],
    [`/api/provider-credential-operations/${id}`, undefined],
    [`/api/provider-credential-operations/${id}/cancel`, {}],
  ]) assert.equal((await api(c, route, body, 409)).code, "credential_legacy_migrated");
  assert(!fs.existsSync(source(id)));
}
try {
  let oldState, oldId, oldBytes;
  if (previous) {
    const old = await boot(previous);
    const storeId = (await api(old, "/api/snapshot")).settingsWriteProtocol.storeId;
    const p = await api(old, `/api/provider-credentials/v3/protocol?storeId=${storeId}`);
    oldId = randomUUID();
    await api(old, `/api/provider-credential-operations/${oldId}/cancel`, {});
    oldBytes = fs.readFileSync(source(oldId));
    await api(old, "/api/provider-credential-operations/v3/cancel", { storeId, namespaceId: p.namespaceId, operationId: randomUUID() });
    oldState = state(); assert.equal(oldState.version, 10);
    const rejected = await boot(binary, false);
    assert.match(rejected, /already|运行|locked|lock|使用|busy/i);
    assert.deepEqual(state(), oldState);
    assert(!fs.existsSync(path.join(data, "local-agent.pre-legacy-admission-v10.sqlite3")));
    await api(old, "/api/snapshot");
    record("actual schema-10 service retains ownership: new process cannot open or upgrade its store while it runs");
    await stop(old.child); report.actualPreviousProcess = true;
  }
  let current = await boot();
  let storeId = (await api(current, "/api/snapshot")).settingsWriteProtocol.storeId;
  let p = await api(current, `/api/provider-credentials/v3/protocol?storeId=${storeId}`);
  assert.deepEqual(await api(current, "/api/provider-credentials/legacy-policy"), { version: 2, storeId, schemaVersion: 16 });
  assert.equal(state().version, 16);
  record("read-only admission policy identifies the actual main store and schema 16 without a credential operation");
  if (previous) {
    assert.deepEqual(state(), { ...oldState, version: 16 });
    assert.equal(p.namespaceId, oldState.scope[0].namespace_id);
    assert.deepEqual(fs.readFileSync(source(oldId)), oldBytes);
    const backup = path.join(data, "local-agent.pre-legacy-admission-v10.sqlite3");
    assert.equal(fs.statSync(backup).mode & 0o777, 0o600);
    const db = new DatabaseSync(backup, { readOnly: true });
    try { assert.equal(db.prepare("PRAGMA user_version").get().user_version, 10); } finally { db.close(); }
    const beforePreview = state();
    const v = await api(current, preview(p, oldId)); assert.equal(v.version, 2); assert.deepEqual(state(), beforePreview);
    await api(current, "/api/provider-credentials/v3/legacy", confirmation(v));
    assert.equal((await api(current, query(p, oldId))).operation.status, "cancelled");
    record("actual schema-10 upgrade preserves SQL claim bytes, original namespace, v1 files and private backup; explicit preview 2 restores a cancellation");
  }
  if (previous) {
    const missing = randomUUID();
    assert.equal((await api(current,"/api/provider-credentials",{operationId:missing,label:"Unknown historical ID",secret},503)).code,"credential_result_unknown");
    await stop(current.child); data=path.join(root,"fresh-data"); current=await boot();
    storeId=(await api(current,"/api/snapshot")).settingsWriteProtocol.storeId;
    p=await api(current,`/api/provider-credentials/v3/protocol?storeId=${storeId}`);
    report.upgradedUnknownLegacyCreationRejected=true;
  }
  const id = randomUUID(), input = { operationId: id, label: "Fixture", secret };
  const receipt = await api(current, "/api/provider-credentials", input);
  const v = await api(current, preview(p, id)); assert.equal(v.version, 2);
  await api(current, "/api/provider-credentials/v3/legacy", confirmation(v));
  const stable = state();
  fs.renameSync(path.join(data, "provider-configs"), path.join(data, "provider-configs.held-fixture"));
  fs.mkdirSync(path.join(data, "provider-configs"), { mode: 0o700 });
  await fenced(current, id);
  assert.equal((await api(current, query(p, id))).operation.receipt.credentialRef, receipt.credentialRef);
  assert.deepEqual(state(), stable);
  report.fenceLossReproduction = { previousDefect: "legacy-fence-loss-probe.json (batch 49)", sameProcessSameOperationId: true, wholeProviderDirectoryMovedAndRecreated: true, allThreeLegacyEntrypointsRejected: true, v3JournalStillReturnsOriginalReference: true, secondReferenceAllocated: false };
  record("exact prior P1 reproduction: whole source folder recreation cannot enter v1 create/query/cancel or allocate another target; v3 retains original reference");
  await api(current, "/api/provider-credentials/legacy-policy", undefined, 401, { Authorization: "Bearer invalid" });
  await api(current, "/api/provider-credentials/legacy-policy", undefined, 401, { Origin: "https://invalid.example" });
  await new Promise((resolve, reject) => {
    const request = httpRequest(current.base + "/api/provider-credentials/legacy-policy", { headers: { Authorization: "Bearer " + current.token, Host: "invalid.example" } }, response => {
      try { assert.equal(response.statusCode, 401); response.resume(); response.on("end", resolve); } catch (error) { response.resume(); reject(error); }
    });
    request.setTimeout(15000, () => request.destroy(Error("Host probe timed out")));
    request.on("error", reject); request.end();
  });
  record("new policy preserves authorization and Host/Origin admission");
  if (previous) {
    assert.match(await boot(previous, false), /already|运行|locked|lock|使用|busy/i);
    assert.deepEqual(state(), stable);
    record("new service ownership also prevents the actual old service from running concurrently");
  }
  await stop(current.child); current = await boot();
  // No policy, protocol or scoped request may bind admission on our behalf.
  // The managed service must already hold the original source at readiness.
  const main = path.join(data, "local-agent.sqlite3"), heldMain = main + ".held-fixture";
  fs.renameSync(main, heldMain);
  try {
    for (const [route, body] of [
      ["/api/provider-credentials", input],
      [`/api/provider-credential-operations/${id}`, undefined],
      [`/api/provider-credential-operations/${id}/cancel`, {}],
    ]) {
      const response = await api(current, route, body, 503);
      assert.equal(response.code, "credential_recovery_unavailable");
    }
    assert(!fs.existsSync(source(id)));
  } finally { fs.renameSync(heldMain, main); }
  record("startup binds the original main file before readiness: first v1 requests after restart reject main-file loss without protocol warm-up or Vault fallback");
  await fenced(current, id);
  assert.equal((await api(current, query(p, id), undefined, 503)).code, "credential_result_unknown");
  assert.deepEqual(state(), stable);
  record("actual restart with empty MemoryVault preserves SQL admission and cannot claim a ready target or resurrect v1");
  await stop(current.child);
  if (previous) {
    const rejected = await boot(previous, false); assert.match(rejected, /16|version|版本/i);
    assert.deepEqual(state(), stable); assert(!fs.existsSync(source(id)));
    record("after all current processes exit, schema 16 rejects the actual schema-10 executable before it can recreate old state");
  }
  assert.equal(hash(binary), report.binarySha256); if (previous) assert.equal(hash(previous), report.previousBinarySha256);
  report.passed = true;
} finally {
  await Promise.all([...children].map(stop));
  report.ownedProcessesExited = children.size === 0;
  fs.rmSync(root, { recursive: true, force: true }); report.temporaryRootRemoved = !fs.existsSync(root);
  if (process.env.LEGACY_CREDENTIAL_ADMISSION_REPORT) { fs.mkdirSync(path.dirname(process.env.LEGACY_CREDENTIAL_ADMISSION_REPORT), { recursive: true }); fs.writeFileSync(process.env.LEGACY_CREDENTIAL_ADMISSION_REPORT, JSON.stringify(report, null, 2) + "\n"); }
}
console.log(JSON.stringify(report, null, 2));
