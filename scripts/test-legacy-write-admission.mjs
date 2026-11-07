import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { randomUUID, createHash } from "node:crypto";
import { request } from "node:http";
import { DatabaseSync } from "node:sqlite";
const binary = path.resolve(process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ?? "target/debug/local-agent-service");
const previous = process.env.LEGACY_WRITE_PREVIOUS_BINARY && path.resolve(process.env.LEGACY_WRITE_PREVIOUS_BINARY);
const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "legacy-write-admission-")));
const home = path.join(root, "home"); fs.mkdirSync(home);
const secret = "disposable-legacy-admission-write-91274";
const hash = file => createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const report = { passed: false, binarySha256: hash(binary), previousBinarySha256: previous ? hash(previous) : null, actualPreviousProcess: false, cases: [], realModelCalls: 0, realKeychainCalls: 0 };
const children = new Set();
async function boot(directory, executable = binary, ready = true) {
  const token = randomUUID(), child = spawn(executable, ["--data-dir", directory], { env: { ...process.env, LOCAL_AGENT_SOURCE_HOME: home, LOCAL_AGENT_SERVICE_TOKEN: token, LOCAL_AGENT_TEST_MEMORY_VAULT: "1", LOCAL_AGENT_TEST_IDLE_AGENTS: "1" }, stdio: ["ignore", "pipe", "pipe"] });
  children.add(child); child.once("exit", () => children.delete(child));
  let stderr = ""; child.stderr.on("data", b => { stderr += b; });
  const lines = createInterface({ input: child.stdout });
  try {
    const result = await Promise.race([once(lines, "line", { signal: AbortSignal.timeout(15000) }).then(([line]) => ({ line })), once(child, "exit").then(([code]) => ({ code }))]);
    if (!ready) { assert.equal(result.line, undefined); assert.notEqual(result.code, 0); return stderr; }
    assert(result.line, stderr);
    return { child, token, data: directory, base: "http://127.0.0.1:" + JSON.parse(result.line).port };
  } finally { lines.close(); }
}
async function stop(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const done = once(child, "exit"), timer = setTimeout(() => child.kill("SIGKILL"), 8000); timer.unref();
  child.kill("SIGTERM"); try { await done; } finally { clearTimeout(timer); }
}
async function api(c, route, body, status = 200, headers = {}) {
  const r = await fetch(c.base + route, { method: body === undefined ? "GET" : "POST", headers: { Authorization: "Bearer " + c.token, "Content-Type": "application/json", ...headers }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(15000) });
  const raw = await r.text(); assert.equal(r.status, status, raw); assert(!raw.includes(secret)); return JSON.parse(raw);
}
function sql(c, fn) { const db = new DatabaseSync(path.join(c.data, "local-agent.sqlite3")); try { return fn(db); } finally { db.close(); } }
const state = c => sql(c, db => ({ version: db.prepare("PRAGMA user_version").get().user_version, scope: db.prepare("SELECT * FROM credential_recovery_scope").all(), values: db.prepare("SELECT domain,name,hex(value) value,value_hash,revision FROM credential_recovery_values ORDER BY domain,name").all(), legacy: db.prepare("SELECT * FROM credential_legacy_scope").all(), ids: db.prepare("SELECT * FROM credential_legacy_operations ORDER BY operation_id").all() }));
const file = (c, id) => path.join(c.data, "provider-configs/credential-operations-v1", id + ".json");
const input = id => ({ operationId: id, label: "Unclaimed fixture", secret });
const create = (c, id, status = 200) => api(c, "/api/provider-credentials", input(id), status);
const get = id => "/api/provider-credential-operations/" + id;
const record = text => report.cases.push(text);
async function unknown(c, id) {
  const before = state(c);
  for (const [route, body] of [["/api/provider-credentials", input(id)], [get(id), undefined], [get(id) + "/cancel", {}]]) {
    const result = await api(c, route, body, 503); assert.equal(result.code, "credential_result_unknown"); assert.equal(result.outcomeUnknown, true);
  }
  assert.deepEqual(state(c), before); assert(!fs.existsSync(file(c, id)));
}
try {
  let c = await boot(path.join(root, "fresh"));
  const initial = await api(c, "/api/snapshot"), storeId = initial.settingsWriteProtocol.storeId;
  const p = await api(c, `/api/provider-credentials/v3/protocol?storeId=${storeId}`);
  assert.deepEqual(await api(c, "/api/provider-credentials/legacy-policy"), { version: 2, storeId, schemaVersion: 16 });
  assert.equal(state(c).legacy[0].allow_new, 1);
  const id = randomUUID(), empty = state(c);
  assert.equal((await api(c, get(id), undefined, 404)).code, "operation_not_found"); assert.deepEqual(state(c), empty);
  record("fresh source advertises policy 2 and schema 16; a read-only unknown query creates no admission row");
  const receipts = await Promise.all(Array.from({ length: 12 }, () => create(c, id)));
  assert.equal(new Set(receipts.map(v => v.credentialRef)).size, 1);
  const original = receipts[0], raw = fs.readFileSync(file(c, id)); assert.equal(state(c).ids.length, 1);
  assert.equal((await api(c, "/api/provider-credentials", { ...input(id), secret: "different-fixture-key" }, 409)).code, "operation_conflict");
  record("twelve same-ID requests still share the original v1 response and reference; changed input conflicts after one durable reservation");
  fs.unlinkSync(file(c, id)); await unknown(c, id); fs.writeFileSync(file(c, id), raw, { mode: 0o600 });
  assert.deepEqual((await api(c, get(id))).receipt, original);
  record("a single unclaimed file loss cannot create, query as absent or falsely cancel; exact restoration reuses the original live Vault reference");
  const folder = path.join(c.data, "provider-configs"), held = folder + ".original";
  fs.renameSync(folder, held); fs.mkdirSync(folder, { mode: 0o700 }); await unknown(c, id);
  const page = await api(c, `/api/provider-credentials/v3/discovery?storeId=${storeId}&namespaceId=${p.namespaceId}`);
  assert.equal(page.items.length, 1); assert.equal(page.items[0].operationId, id); assert.equal(page.items[0].issue, "unavailable"); assert.equal(page.items[0].label, null);
  fs.rmSync(folder, { recursive: true }); fs.renameSync(held, folder); assert.deepEqual((await api(c, get(id))).receipt, original);
  report.unclaimedDirectoryLoss = { allThreeLegacyEntrypointsRejected: true, secondReferenceAllocated: false, originalReferenceRecoveredAfterExactRestore: true, missingKnownIdRemainsDiscoverable: true };
  record("the exact old P1 whole-directory-loss/recreation probe rejects every old entrypoint; discovery retains a disabled unknown row and restored bytes recover the same reference");
  for (const table of ["credential_legacy_operations", "credential_legacy_scope", "credential_recovery_scope"]) {
    const target = randomUUID(), before = state(c), event = table === "credential_legacy_operations" ? "INSERT" : "UPDATE";
    sql(c, db => db.exec(`CREATE TRIGGER reject_legacy_reservation BEFORE ${event} ON ${table} BEGIN SELECT RAISE(IGNORE);END;`));
    await create(c, target, 503); assert.deepEqual(state(c), before); assert(!fs.existsSync(file(c, target)));
    sql(c, db => db.exec("DROP TRIGGER reject_legacy_reservation"));
  }
  record("actual ignored reservation, count and revision writes roll back together before creating a target file or returning a false success");
  const cancelled = randomUUID(); const tombstone = await api(c, get(cancelled) + "/cancel", {}); assert.equal(tombstone.status, "cancelled");
  const tombstoneBytes = fs.readFileSync(file(c, cancelled)); fs.unlinkSync(file(c, cancelled)); await unknown(c, cancelled);
  fs.writeFileSync(file(c, cancelled), tombstoneBytes, { mode: 0o600 }); assert.deepEqual(await api(c, get(cancelled)), tombstone);
  record("a cancellation reserves the same ID and lost tombstone material remains unknown rather than fabricating a second cancellation");
  const beforeInvalid = state(c);
  await api(c, "/api/provider-credentials", { ...input(randomUUID()), unexpected: true }, 400);
  await api(c, "/api/provider-credentials", input(randomUUID()), 401, { Authorization: "Bearer invalid" });
  await api(c, "/api/provider-credentials", input(randomUUID()), 401, { Origin: "https://invalid.example" });
  await new Promise((resolve, reject) => { const req = request(c.base + "/api/provider-credentials/legacy-policy", { headers: { Authorization: "Bearer " + c.token, Host: "invalid.example" } }, response => { try { assert.equal(response.statusCode, 401); response.resume(); response.on("end", resolve); } catch (e) { response.resume(); reject(e); } }); req.on("error", reject); req.setTimeout(15000, () => req.destroy(Error("Host timeout"))); req.end(); });
  assert.deepEqual(state(c), beforeInvalid); assert.deepEqual((await api(c, "/api/snapshot")).providers, initial.providers);
  record("strict fields, authorization, Host and Origin reject before admission; no Provider configuration or model request is added");
  const durable = state(c); await stop(c.child); c = await boot(c.data);
  assert.equal((await api(c, get(id), undefined, 503)).code, "credential_result_unknown"); await create(c, id, 503); assert.deepEqual(state(c), durable);
  sql(c, db => db.exec("UPDATE credential_legacy_scope SET operation_count=0"));
  await api(c, "/api/provider-credentials/legacy-policy", undefined, 503); await api(c, get(id), undefined, 503);
  sql(c, db => db.prepare("UPDATE credential_legacy_scope SET operation_count=?").run(durable.ids.length)); assert.deepEqual(state(c), durable);
  record("actual restart retains admission and refuses the empty fixture Vault; corrupted count cannot advertise policy or fall back to a new operation");
  await stop(c.child); c = await boot(path.join(root, "capacity"));
  const existingId = randomUUID(), existing = await create(c, existingId);
  sql(c, db => { db.exec("BEGIN IMMEDIATE"); for (let n = 1; n < 4096; n++) db.prepare("INSERT INTO credential_legacy_operations VALUES(?)").run(randomUUID()); db.exec("UPDATE credential_legacy_scope SET operation_count=4096;COMMIT"); });
  const cap = state(c); assert.equal((await create(c, randomUUID(), 400)).code, "credential_operation_capacity"); assert.deepEqual(state(c), cap); assert.deepEqual((await api(c, get(existingId))).receipt, existing);
  const capStore = (await api(c, "/api/snapshot")).settingsWriteProtocol.storeId, capP = await api(c, `/api/provider-credentials/v3/protocol?storeId=${capStore}`);
  assert.equal((await api(c, "/api/provider-credentials/v3", { storeId: capStore, namespaceId: capP.namespaceId, operationId: randomUUID(), label: "Modern explicit fixture", secret })).operation.status, "ready");
  record("4096 admissions stop new legacy IDs without evicting existing records; the explicit modern v3 path remains usable");
  if (previous) {
    await stop(c.child); const data = path.join(root, "previous"); const old = await boot(data, previous);
    const oldId = randomUUID(), oldReceipt = await create(old, oldId), oldBytes = fs.readFileSync(file(old, oldId));
    const oldCancelled = randomUUID(); await api(old, get(oldCancelled) + "/cancel", {});
    const oldStore = (await api(old, "/api/snapshot")).settingsWriteProtocol.storeId, oldP = await api(old, `/api/provider-credentials/v3/protocol?storeId=${oldStore}`);
    const oldValues = sql(old, db => db.prepare("SELECT * FROM credential_recovery_values ORDER BY domain,name").all());
    assert.match(await boot(data, binary, false), /already|运行|lock|使用|busy/i); assert(!fs.existsSync(path.join(data, "local-agent.pre-legacy-write-v11.sqlite3"))); assert.deepEqual((await api(old, get(oldId))).receipt, oldReceipt);
    record("the actual live schema-11 service prevents new opening and upgrade before any schema-11 backup is made");
    await stop(old.child); c = await boot(data);
    const upgraded = state(c); assert.equal(upgraded.version, 16); assert.equal(upgraded.legacy[0].allow_new, 0);
    assert.equal((await api(c, "/api/snapshot")).settingsWriteProtocol.storeId, oldStore);
    assert.equal((await api(c, `/api/provider-credentials/v3/protocol?storeId=${oldStore}`)).namespaceId, oldP.namespaceId);
    assert.deepEqual(sql(c, db => db.prepare("SELECT * FROM credential_recovery_values ORDER BY domain,name").all()), oldValues); assert.deepEqual(fs.readFileSync(file(c, oldId)), oldBytes);
    const backup = path.join(data, "local-agent.pre-legacy-write-v11.sqlite3"); assert.equal(fs.statSync(backup).mode & 0o777, 0o600);
    const backupDb = new DatabaseSync(backup, { readOnly: true }); try { assert.equal(backupDb.prepare("PRAGMA user_version").get().user_version, 11); } finally { backupDb.close(); }
    await unknown(c, randomUUID());
    const preview = await api(c, `/api/provider-credentials/v3/legacy/${oldCancelled}?storeId=${oldStore}&namespaceId=${oldP.namespaceId}`);
    assert.equal((await api(c, "/api/provider-credentials/v3/legacy", { storeId: oldStore, namespaceId: oldP.namespaceId, operationId: oldCancelled, sourceDigest: preview.sourceDigest, confirmed: true })).stage, "committed");
    record("actual schema-11 upgrade preserves original identity, namespace, operation bytes and private backup; missing history rejects all old writes while existing cancellation still explicitly migrates");
    const stable = state(c); assert.match(await boot(data, previous, false), /already|运行|lock|使用|busy/i); await stop(c.child);
    assert.match(await boot(data, previous, false), /16|version|版本/i); assert.deepEqual(state(c), stable);
    report.actualPreviousProcess = true; report.actualPreviousReaderRejected = true;
    record("new ownership blocks the actual old process while live, then the old schema-11 reader rejects schema 16 after shutdown without resetting admission");
  }
  assert.equal(hash(binary), report.binarySha256); if (previous) assert.equal(hash(previous), report.previousBinarySha256);
  report.passed = true;
} finally {
  await Promise.all([...children].map(stop)); report.ownedProcessesExited = children.size === 0;
  fs.rmSync(root, { recursive: true, force: true }); report.temporaryRootRemoved = !fs.existsSync(root);
  if (process.env.LEGACY_WRITE_ADMISSION_REPORT) { fs.mkdirSync(path.dirname(process.env.LEGACY_WRITE_ADMISSION_REPORT), { recursive: true }); fs.writeFileSync(process.env.LEGACY_WRITE_ADMISSION_REPORT, JSON.stringify(report, null, 2) + "\n"); }
}
console.log(JSON.stringify(report, null, 2));
