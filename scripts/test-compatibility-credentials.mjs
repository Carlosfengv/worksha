import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { randomUUID, createHash } from "node:crypto";
import { request } from "node:http";
import { DatabaseSync } from "node:sqlite";

const binary = path.resolve(process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ?? "target/debug/local-agent-service");
const previous = process.env.COMPATIBILITY_CREDENTIAL_PREVIOUS_BINARY && path.resolve(process.env.COMPATIBILITY_CREDENTIAL_PREVIOUS_BINARY);
const root = fs.realpathSync(fs.mkdtempSync(path.join(tmpdir(), "compatibility-credentials-")));
const home = path.join(root, "home"), data = path.join(root, "data");
fs.mkdirSync(home);
const secret = "disposable-compatibility-private-71829";
const hash = p => createHash("sha256").update(fs.readFileSync(p)).digest("hex");
const report = { passed: false, binarySha256: hash(binary), previousBinarySha256: previous ? hash(previous) : null, actualPreviousProcess: false, cases: [], realModelCalls: 0, realKeychainCalls: 0 };
const children = new Set();
let current;
async function boot(executable = binary, directory = data) {
  const token = randomUUID();
  const child = spawn(executable, ["--data-dir", directory], { env: { ...process.env, LOCAL_AGENT_SOURCE_HOME: home, LOCAL_AGENT_SERVICE_TOKEN: token, LOCAL_AGENT_TEST_MEMORY_VAULT: "1", LOCAL_AGENT_TEST_IDLE_AGENTS: "1" }, stdio: ["ignore", "pipe", "pipe"] });
  children.add(child); child.once("exit", () => children.delete(child));
  let stderr = ""; child.stderr.on("data", b => { stderr += b; });
  const lines = createInterface({ input: child.stdout });
  try {
    const [line] = await Promise.race([once(lines, "line", { signal: AbortSignal.timeout(15000) }), once(child, "exit").then(() => { throw Error(stderr); })]);
    return { child, token, base: "http://127.0.0.1:" + JSON.parse(line).port };
  } finally { lines.close(); }
}
async function stop(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const done = once(child, "exit"), timer = setTimeout(() => child.kill("SIGKILL"), 8000); timer.unref();
  child.kill("SIGTERM"); try { await done; } finally { clearTimeout(timer); }
}
async function call(route, body, status = 200, headers = {}) {
  const response = await fetch(current.base + route, { method: body === undefined ? "GET" : "POST", headers: { Authorization: "Bearer " + current.token, "Content-Type": "application/json", ...headers }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(15000) });
  const raw = await response.text(); assert.equal(response.status, status, raw); assert(!raw.includes(secret));
  return raw ? JSON.parse(raw) : null;
}
function sql(fn) {
  const db = new DatabaseSync(path.join(data, "local-agent.sqlite3"));
  try { return fn(db); } finally { db.close(); }
}
const savedState = () => sql(db => ({ scope: db.prepare("SELECT * FROM credential_recovery_scope").all(), values: db.prepare("SELECT domain,name,hex(value) value,value_hash,revision FROM credential_recovery_values ORDER BY domain,name").all() }));
const list = p => call(`/api/provider-credentials/v3/discovery?storeId=${p.storeId}&namespaceId=${p.namespaceId}`);
const query = (p, id, status = 200) => call(`/api/provider-credential-operations/v3/${id}?storeId=${p.storeId}&namespaceId=${p.namespaceId}`, undefined, status);
const record = label => report.cases.push(label);
try {
  current = await boot();
  const initial = await call("/api/snapshot");
  const storeId = initial.settingsWriteProtocol.storeId;
  const p = await call(`/api/provider-credentials/v3/protocol?storeId=${storeId}`);
  const first = await call("/api/provider-credentials", { label: "Compatibility first", secret });
  assert.deepEqual(Object.keys(first).sort(), ["credentialLabel", "credentialRef"]);
  assert.equal(first.credentialLabel, "Compatibility first");
  const discovered = await list(p);
  if (process.env.COMPATIBILITY_EXPECT_UNTRACKED === "1") {
    assert.equal(discovered.items.length, 0);
    report.defectReproduced = true;
    report.credentialReturnedButNoDiscoverableRecord = true;
    report.passed = true;
  } else {
    assert.equal(discovered.items.length, 1);
    const row = discovered.items[0]; assert.equal(row.kind, "scoped_v3"); assert.equal(row.phase, "ready");
    assert.equal((await query(p, row.operationId)).operation.receipt.credentialRef, first.credentialRef);
    record("the old two-field no-ID response is preserved and its new v3 intent is discoverable with the original reference");

    await new Promise((resolve, reject) => {
      const req = request(current.base + "/api/provider-credentials", { method: "POST", headers: { Authorization: "Bearer " + current.token, "Content-Type": "application/json" } }, response => { try { assert.equal(response.statusCode, 200); response.destroy(); resolve(); } catch (e) { response.resume(); reject(e); } });
      req.on("error", reject); req.setTimeout(15000, () => req.destroy(Error("response timeout")));
      req.end(JSON.stringify({ label: "Lost compatibility reply", secret }));
    });
    const lost = (await list(p)).items.find(item => item.label === "Lost compatibility reply"); assert(lost);
    const lostResult = await query(p, lost.operationId); assert.equal(lostResult.operation.status, "ready");
    assert.equal((await query(p, lost.operationId)).operation.receipt.credentialRef, lostResult.operation.receipt.credentialRef);
    record("an actual disconnected no-ID response is recovered by discovery and a separate query without resending the key");

    const duplicate = await call("/api/provider-credentials", { label: "Compatibility first", secret });
    assert.notEqual(duplicate.credentialRef, first.credentialRef);
    assert.equal((await list(p)).items.filter(item => item.label === "Compatibility first").length, 2);
    record("independent repeated no-ID submissions remain distinct but both are tracked; idempotence is not falsely claimed");

    const beforeInvalid = savedState();
    await call("/api/provider-credentials", { label: secret, secret }, 400);
    await call("/api/provider-credentials", { label: "Bad extra", secret, extra: true }, 400);
    await call("/api/provider-credentials", { label: "Bad key", secret: "contains whitespace" }, 400);
    await call("/api/provider-credentials", { label: "Unauthorized", secret }, 401, { Authorization: "Bearer invalid" });
    await call("/api/provider-credentials", { label: "Bad origin", secret }, 401, { Origin: "https://invalid.example" });
    assert.deepEqual(savedState(), beforeInvalid);
    record("invalid labels, keys and unknown fields plus authorization and Origin rejection create no recovery rows or secret responses");

    sql(db => db.exec("CREATE TRIGGER reject_compat_intent BEFORE INSERT ON credential_recovery_values WHEN NEW.domain=3 AND NEW.name GLOB 'operations/*' BEGIN SELECT RAISE(IGNORE); END;"));
    await call("/api/provider-credentials", { label: "Rejected intent", secret }, 503);
    assert.deepEqual(savedState(), beforeInvalid);
    sql(db => db.exec("DROP TRIGGER reject_compat_intent"));
    record("an actual SQLite ignored intent cannot return a credential or a false success and preserves prior recovery bytes");

    assert.deepEqual((await call("/api/snapshot")).providers, initial.providers);
    const durable = savedState();
    await stop(current.child); current = await boot();
    assert.equal((await list(p)).items.length, 3);
    assert.equal((await query(p, row.operationId, 503)).code, "credential_result_unknown");
    assert.equal((await call("/api/provider-credentials", { label: "After empty Vault restart", secret }, 503)).code, "credential_result_unknown");
    assert.deepEqual(savedState(), durable);
    record("restart preserves discoverable metadata but never claims a credential exists or replaces its missing auxiliary key");

    if (previous) {
      await stop(current.child);
      let rejected=false;
      try { await boot(previous); } catch (error) { rejected=true;assert.match(String(error),/13|version|版本/); }
      assert(rejected,"actual previous schema-11 service must reject schema 13");
      assert.deepEqual(savedState(), durable);
      report.actualPreviousProcess = true;
      report.downgradeRejected = true;
      record("the actual previous schema-11 executable rejects schema 13 before readiness and cannot rewrite the new v3 intents");
      await stop(current.child);
      const priorData = path.join(root, "previous-data");
      current = await boot(previous, priorData);
      await call("/api/provider-credentials", { label: "Historical untracked", secret });
      await stop(current.child); current = await boot(binary, priorData);
      const priorId = (await call("/api/snapshot")).settingsWriteProtocol.storeId;
      const priorProtocol = await call(`/api/provider-credentials/v3/protocol?storeId=${priorId}`);
      assert.equal((await list(priorProtocol)).items.length, 0);
      await call("/api/provider-credentials", { label: "New tracked", secret });
      assert.deepEqual((await list(priorProtocol)).items.map(item => item.label), ["New tracked"]);
      record("historical untracked keys from the actual previous process are not automatically claimed; only the new explicit request gains a record");
    }
    report.passed = true;
  }
  assert.equal(hash(binary), report.binarySha256);
  if (previous) assert.equal(hash(previous), report.previousBinarySha256);
} finally {
  await Promise.all([...children].map(stop));
  report.ownedProcessesExited = children.size === 0;
  fs.rmSync(root, { recursive: true, force: true });
  report.temporaryRootRemoved = !fs.existsSync(root);
  if (process.env.COMPATIBILITY_CREDENTIAL_REPORT) {
    fs.mkdirSync(path.dirname(process.env.COMPATIBILITY_CREDENTIAL_REPORT), { recursive: true });
    fs.writeFileSync(process.env.COMPATIBILITY_CREDENTIAL_REPORT, JSON.stringify(report, null, 2) + "\n");
  }
}
console.log(JSON.stringify(report, null, 2));
