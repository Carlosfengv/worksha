import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { request as httpRequest } from "node:http";
import { createHash, randomUUID } from "node:crypto";
import { DatabaseSync } from "node:sqlite";

const binary = path.resolve(process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ?? "target/debug/local-agent-service");
const previous = process.env.CREDENTIAL_RECOVERY_PREVIOUS_BINARY && path.resolve(process.env.CREDENTIAL_RECOVERY_PREVIOUS_BINARY);
const root = fs.realpathSync(fs.mkdtempSync(path.join(tmpdir(), "local-agent-credential-recovery-")));
const home = path.join(root, "home"); fs.mkdirSync(home);
const hash = file => createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const report = { passed: false, binarySha256: hash(binary), previousBinarySha256: previous ? hash(previous) : null, actualPreviousProcess: false, cases: [], realModelCalls: 0, realKeychainCalls: 0 };
const children = new Set();
const secret = "disposable-journal-fixture-319613";
const record = text => report.cases.push(text);
async function boot(name, executable = binary) {
  const data = path.join(root, name), token = randomUUID();
  const env = { ...process.env, LOCAL_AGENT_SOURCE_HOME: home, LOCAL_AGENT_SERVICE_TOKEN: token, LOCAL_AGENT_TEST_MEMORY_VAULT: "1", LOCAL_AGENT_TEST_IDLE_AGENTS: "1" };
  delete env.LOCAL_AGENT_VAULT_WORKER;
  const child = spawn(executable, ["--data-dir", data], { env, stdio: ["ignore", "pipe", "pipe"] });
  children.add(child); child.once("exit", () => children.delete(child));
  let stderr = ""; child.stderr.on("data", bytes => { stderr += bytes; });
  const lines = createInterface({ input: child.stdout });
  const [line] = await Promise.race([once(lines, "line", { signal: AbortSignal.timeout(15000) }), once(child, "exit").then(() => { throw Error("fixture service exited: " + stderr); })]);
  return { child, data, token, base: `http://127.0.0.1:${JSON.parse(line).port}` };
}
async function stop(instance) {
  const child = instance.child;
  if (child.exitCode !== null || child.signalCode !== null) return;
  const done = once(child, "exit"); const timer = setTimeout(() => child.kill("SIGKILL"), 8000); timer.unref();
  child.kill("SIGTERM"); try { await done; } finally { clearTimeout(timer); }
}
async function api(instance, route, body, expected = 200, headers = {}) {
  const response = await fetch(instance.base + route, { method: body === undefined ? "GET" : "POST", headers: { Authorization: `Bearer ${instance.token}`, "Content-Type": "application/json", ...headers }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(15000) });
  const raw = await response.text(); assert.equal(response.status, expected, raw); assert(!raw.includes(secret), "secret must never be returned");
  return JSON.parse(raw);
}
const dbAt = instance => new DatabaseSync(path.join(instance.data, "local-agent.sqlite3"));
const sql = (instance, fn) => { const db = dbAt(instance); try { return fn(db); } finally { db.close(); } };
const snapshot = instance => sql(instance, db => ({ scope: db.prepare("SELECT * FROM credential_recovery_scope").all(), values: db.prepare("SELECT domain,name,hex(value) AS value,value_hash,revision FROM credential_recovery_values ORDER BY domain,name").all() }));
const protocol = (instance, storeId) => api(instance, `/api/provider-credentials/v3/protocol?storeId=${storeId}`);
const identity = (p, operationId = randomUUID()) => ({ storeId: p.storeId, namespaceId: p.namespaceId, operationId });
const input = id => ({ ...id, label: "Journal fixture", secret });
const query = id => `/api/provider-credential-operations/v3/${id.operationId}?storeId=${id.storeId}&namespaceId=${id.namespaceId}`;
const create = (instance, id, status = 200) => api(instance, "/api/provider-credentials/v3", input(id), status);
const initialize = (instance, p, choice, status = 200) => api(instance, "/api/provider-credentials/v3/initialize", { storeId: p.storeId, namespaceId: p.namespaceId, choice }, status);
const fileHashes = folder => {
  const result = {};
  const visit = dir => { for (const entry of fs.readdirSync(dir, { withFileTypes: true })) { const file = path.join(dir, entry.name); if (entry.isDirectory()) visit(file); else { assert(entry.isFile()); result[path.relative(folder, file)] = { sha256: hash(file), mode: fs.statSync(file).mode & 0o777 }; } } };
  visit(folder); return result;
};
try {
  let a = await boot("a");
  const storeId = (await api(a, "/api/snapshot")).settingsWriteProtocol.storeId;
  const p = await protocol(a, storeId); assert.equal(p.version, 3); assert.equal(p.namespaceState, "active"); assert.equal(p.legacyState, "fresh");
  assert.equal(sql(a, db => db.prepare("PRAGMA user_version").get().user_version),15);
  const id = identity(p), results = await Promise.all(Array.from({ length: 12 }, () => create(a, id)));
  const ready = results[0], reference = ready.operation.receipt.credentialRef;
  assert(results.every(value => value.operation.receipt.credentialRef === reference)); assert.deepEqual(await api(a, query(id)), ready);
  assert.equal((await api(a, "/api/provider-credentials/v3", { ...input(id), secret: "other-disposable-key" }, 409)).code, "operation_conflict");
  record("fresh schema 15 advertises the original namespace; 12 concurrent requests and retry share one reference, changed input conflicts");
  const before = snapshot(a), foreignStore = randomUUID(), foreignNamespace = randomUUID();
  for (const foreign of [{ ...id, storeId: foreignStore }, { ...id, namespaceId: foreignNamespace }]) {
    const code = foreign.storeId !== storeId ? "credential_store_changed" : "credential_namespace_changed";
    for (const [route, body] of [[query(foreign), undefined], ["/api/provider-credentials/v3", input(foreign)], ["/api/provider-credential-operations/v3/cancel", foreign], ["/api/provider-credentials/v3/initialize", { storeId: foreign.storeId, namespaceId: foreign.namespaceId, choice: "new" }]]) assert.equal((await api(a, route, body, 409)).code, code);
  }
  assert.equal((await api(a, `/api/provider-credentials/v3/protocol?storeId=${foreignStore}`, undefined, 409)).code, "credential_store_changed"); assert.deepEqual(snapshot(a), before);
  record("all v3 action boundaries reject a foreign database or original namespace without mutating recovery rows");
  // Move the entire Provider directory, not just a marker nested in it.
  const config = path.join(a.data, "provider-configs"), moved = config + ".moved-fixture";
  fs.renameSync(config, moved);
  assert.deepEqual(await api(a, query(id)), ready); assert.deepEqual(await create(a, id), ready); assert.deepEqual(snapshot(a), before);
  fs.renameSync(moved, config);
  record("actual whole Provider-directory loss preserves the original SQL intent and credential reference on query and retry");
  for (const name of [`operations/${id.operationId}.json`, "protection-key.json"]) {
    const saved = sql(a, db => { const row = db.prepare("SELECT * FROM credential_recovery_values WHERE domain=3 AND name=?").get(name); db.prepare("DELETE FROM credential_recovery_values WHERE domain=3 AND name=?").run(name); return row; });
    assert.equal((await api(a, query(id), undefined, 503)).code, "credential_recovery_corrupt"); assert.equal((await create(a, identity(p), 503)).code, "credential_recovery_corrupt");
    sql(a, db => db.prepare("INSERT INTO credential_recovery_values VALUES(?,?,?,?,?)").run(saved.domain, saved.name, saved.value, saved.value_hash, saved.revision)); assert.deepEqual(await api(a, query(id)), ready);
  }
  record("missing operation or protection-key SQL rows reject before recovery can reinitialize; exact fixture restoration recovers");
  const ignored = identity(p), stateBefore = snapshot(a);
  sql(a, db => db.exec("CREATE TRIGGER ignore_fixture_intent BEFORE INSERT ON credential_recovery_values WHEN NEW.name GLOB 'operations/*' BEGIN SELECT RAISE(IGNORE); END;"));
  await create(a, ignored, 503); assert.deepEqual(snapshot(a), stateBefore);
  sql(a, db => db.exec("DROP TRIGGER ignore_fixture_intent"));
  assert.equal((await api(a, query(ignored), undefined, 404)).code, "operation_not_found");
  const finish = identity(p);
  sql(a, db => db.exec("CREATE TRIGGER ignore_fixture_ready BEFORE UPDATE ON credential_recovery_values WHEN NEW.name GLOB 'operations/*' AND CAST(NEW.value AS TEXT) LIKE '%\"state\":\"ready\"%' BEGIN SELECT RAISE(IGNORE); END;"));
  await create(a, finish, 503); sql(a, db => db.exec("DROP TRIGGER ignore_fixture_ready"));
  const recoveredFinish = await api(a, query(finish)); assert.equal(recoveredFinish.operation.status, "ready"); assert.deepEqual(await create(a, finish), recoveredFinish);
  record("SQL ignored intent writes roll back atomically; ignored finalization remains recoverable under the same reference");
  const cancelled = identity(p); assert.equal((await api(a, "/api/provider-credential-operations/v3/cancel", cancelled)).operation.status, "cancelled"); assert.equal((await create(a, cancelled, 409)).code, "credential_operation_cancelled");
  const lost = identity(p);
  await new Promise((resolve, reject) => { const request = httpRequest(a.base + "/api/provider-credentials/v3", { method: "POST", headers: { Authorization: `Bearer ${a.token}`, "Content-Type": "application/json" } }, response => { try { assert.equal(response.statusCode, 200); response.destroy(); resolve(); } catch (error) { reject(error); } }); request.on("error", reject); request.end(JSON.stringify(input(lost))); });
  const recoveredLost = await api(a, query(lost)); assert.equal(recoveredLost.operation.status, "ready"); assert.deepEqual(await create(a, lost), recoveredLost);
  record("cancellation before delivery fences late creation; an actually discarded HTTP reply can be queried and retried by its original identity");
  await api(a, query(id), undefined, 401, { Authorization: "Bearer invalid" }); await api(a, "/api/provider-credentials/v3", input(id), 401, { Origin: "https://invalid.example" });
  await new Promise((resolve, reject) => { const req = httpRequest(a.base + query(id), { headers: { Authorization: `Bearer ${a.token}`, Host: "invalid.example" } }, res => { try { assert.equal(res.statusCode, 401); res.resume(); res.on("end", resolve); } catch (error) { reject(error); } }); req.on("error", reject); req.end(); });
  await api(a, "/api/provider-credentials/v3", { ...input(id), unexpected: secret }, 400); await api(a, `/api/provider-credentials/v3/protocol?storeId=${storeId}&extra=true`, undefined, 400);
  record("actual routes preserve authorization, Host/Origin and strict request fields, returning no secret bytes");
  const durable = snapshot(a); assert(!JSON.stringify(durable).includes(Buffer.from(secret).toString("hex").toUpperCase()));
  await stop(a); a = await boot("a"); assert.deepEqual(await protocol(a, storeId), p);
  assert.equal((await api(a, query(id), undefined, 503)).code, "credential_result_unknown"); assert.equal((await create(a, id, 503)).code, "credential_result_unknown");
  assert.equal((await api(a, query(cancelled))).operation.status, "cancelled"); assert.deepEqual(snapshot(a), durable); await stop(a);
  record("real process restart with an empty fixture Vault retains SQL bytes and tombstones and cannot falsely report saved credentials");
  if (previous) {
    for (const variant of ["import", "explicit-restore", "explicit-new"]) {
      let old = await boot(variant, previous); const oldStore = (await api(old, "/api/snapshot")).settingsWriteProtocol.storeId;
      assert.equal(sql(old, db => db.prepare("PRAGMA user_version").get().user_version), 9);
      let oldResult, archive, originalArchive;
      const oldId = randomUUID();
      if (variant !== "explicit-new") {
        oldResult = await api(old, "/api/provider-credentials/scoped", { storeId: oldStore, operationId: oldId, label: "Journal fixture", secret });
        archive = path.join(old.data, "provider-configs/credential-operations-v2"); originalArchive = fileHashes(archive);
      }
      const metadataBefore = sql(old, db => ({ providers: db.prepare("SELECT * FROM providers ORDER BY id").all(), settings: db.prepare("SELECT * FROM app_settings").all() }));
      await stop(old);
      const misplaced = archive && archive + ".held-fixture";
      if (variant === "explicit-restore") fs.renameSync(archive, misplaced);
      const current = await boot(variant), backup = path.join(current.data, "local-agent.pre-credential-recovery-v9.sqlite3"), backupHash = hash(backup);
      assert.equal(fs.statSync(backup).mode & 0o777, 0o600); const backupDb = new DatabaseSync(backup, { readOnly: true }); assert.equal(backupDb.prepare("PRAGMA user_version").get().user_version, 9); backupDb.close();
      let currentP = await protocol(current, oldStore); const originalNs = currentP.namespaceId;
      assert.deepEqual(sql(current, db => ({ providers: db.prepare("SELECT * FROM providers ORDER BY id").all(), settings: db.prepare("SELECT * FROM app_settings").all() })), metadataBefore);
      if (variant !== "import") {
        assert.equal(currentP.namespaceState, "unverified"); assert.equal(currentP.legacyState, "unverified"); assert.equal(snapshot(current).values.length, 0);
        assert.equal((await create(current, identity(currentP), 503)).code, "credential_namespace_unverified");
        if (variant === "explicit-restore") {
          assert.equal((await initialize(current, currentP, "restore", 503)).code, "credential_namespace_unverified"); assert.equal(snapshot(current).values.length, 0);
          fs.renameSync(misplaced, archive); currentP = await initialize(current, currentP, "restore");
        } else {
          currentP = await initialize(current, currentP, "new"); assert.equal(currentP.legacyState, "unverified"); assert.equal(snapshot(current).values.length, 0);
          assert.equal((await api(current, "/api/provider-credentials/scoped", { storeId: oldStore, operationId: oldId, label: "Journal fixture", secret }, 503)).code, "credential_namespace_unverified");
        }
      }
      assert.equal(currentP.namespaceId, originalNs); assert.equal(currentP.namespaceState, "active");
      if (oldResult) {
        assert.equal(currentP.legacyState, "imported"); assert.deepEqual(fileHashes(archive), originalArchive);
        const imported = snapshot(current); const oldRow = imported.values.find(row => row.domain === 2 && row.name === `operations/${oldId}.json`); assert(oldRow);
        assert.equal(JSON.parse(Buffer.from(oldRow.value, "hex").toString()).operation.reference, oldResult.operation.receipt.credentialRef);
        assert.equal((await api(current, `/api/provider-credential-operations/scoped/${oldId}?storeId=${oldStore}`, undefined, 503)).code, "credential_result_unknown");
        assert.equal((await api(current, "/api/provider-credentials/scoped", { storeId: oldStore, operationId: oldId, label: "Journal fixture", secret }, 503)).code, "credential_result_unknown");
        // Unknown old IDs must not become new writes after an import.
        assert.equal((await api(current, "/api/provider-credentials/scoped", { storeId: oldStore, operationId: randomUUID(), label: "Journal fixture", secret }, 503)).code, "credential_protocol_unavailable"); assert.deepEqual(snapshot(current), imported);
      }
      assert.equal((await create(current, identity(currentP))).operation.status, "ready");
      assert.equal(hash(backup), backupHash); const beforeOldReader = snapshot(current); await stop(current);
      const refused = spawn(previous, ["--data-dir", current.data], { env: { ...process.env, LOCAL_AGENT_SOURCE_HOME: home, LOCAL_AGENT_SERVICE_TOKEN: randomUUID(), LOCAL_AGENT_TEST_MEMORY_VAULT: "1", LOCAL_AGENT_TEST_IDLE_AGENTS: "1" }, stdio: ["ignore", "pipe", "pipe"] });
      children.add(refused); refused.once("exit", () => children.delete(refused)); let oldError = ""; refused.stdout.resume(); refused.stderr.on("data", b => { oldError += b; });
      const [code] = await once(refused, "exit", { signal: AbortSignal.timeout(15000) }); assert.notEqual(code, 0, oldError); assert.deepEqual(snapshot(current), beforeOldReader); assert.equal(hash(backup), backupHash);
      record(`actual previous schema 9 process: ${variant} preserves database identity/settings/Provider bytes, private backup and original references; schema 15 rejects the old reader`);
    }
    report.actualPreviousProcess = true;
  }
  assert.equal(hash(binary), report.binarySha256); if (previous) assert.equal(hash(previous), report.previousBinarySha256);
  report.passed = true;
} finally {
  await Promise.all([...children].map(child => stop({ child })));
  fs.rmSync(root, { recursive: true, force: true });
  if (process.env.CREDENTIAL_RECOVERY_REPORT) { fs.mkdirSync(path.dirname(process.env.CREDENTIAL_RECOVERY_REPORT), { recursive: true }); fs.writeFileSync(process.env.CREDENTIAL_RECOVERY_REPORT, JSON.stringify(report, null, 2) + "\n"); }
}
console.log(JSON.stringify(report, null, 2));
