import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { DatabaseSync } from "node:sqlite";

// Disposable package, source file, SQL rows and MemoryVault only. No model calls.
const binary = path.resolve(
  process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
    "target/debug/local-agent-service",
);
const hash = (value) => createHash("sha256").update(value).digest("hex");
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "plugin-config-lifecycle-")),
);
const data = path.join(root, "data"),
  home = path.join(root, "home"),
  target = path.join(root, "selected.json");
fs.mkdirSync(home);
const original = '{"env":{"KEEP":"preserve"},"unrelated":[1,2,3]}';
fs.writeFileSync(target, original, { mode: 0o600 });
const report = {
  complete: false,
  binarySha256: hash(fs.readFileSync(binary)),
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
  capacityFixtureSeededRows: 63,
};
let child, base, token, db;
async function start() {
  token = randomUUID();
  child = spawn(binary, ["--data-dir", data], {
    env: {
      ...process.env,
      LOCAL_AGENT_SOURCE_HOME: home,
      LOCAL_AGENT_SERVICE_TOKEN: token,
      LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
      LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stderr.resume();
  const lines = createInterface({ input: child.stdout });
  const [line] = await Promise.race([
    once(lines, "line", { signal: AbortSignal.timeout(15000) }),
    once(child, "exit").then(() => {
      throw Error("owned service exited before readiness");
    }),
  ]);
  base = "http://127.0.0.1:" + JSON.parse(line).port;
}
async function stop() {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const done = once(child, "exit");
  child.kill("SIGINT");
  const timer = setTimeout(() => child.kill("SIGKILL"), 5000);
  await done.finally(() => clearTimeout(timer));
}
async function api(
  route,
  body,
  expected = 200,
  method = body === undefined ? "GET" : "POST",
) {
  const response = await fetch(base + route, {
    method,
    headers: {
      authorization: "Bearer " + token,
      "content-type": "application/json",
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    signal: AbortSignal.timeout(15000),
  });
  const value = await response.json();
  assert.equal(response.status, expected, route + " " + JSON.stringify(value));
  return value;
}
const passed = (name) => {
  report.cases.push(name);
  console.log("通过：" + name);
};
const history = () =>
  api("/api/plugin-extensions/status", { pluginId: "", digest: "" });
const rows = () => db.prepare("SELECT count(*) AS n FROM plans").get().n;
try {
  const folder = path.join(root, "package");
  fs.cpSync("sdk/plugins/examples/json-config/package", folder, {
    recursive: true,
  });
  const key = path.join(root, "publisher.key"),
    archive = path.join(root, "config.laplug");
  const sdk = (...args) =>
    execFileSync(binary, ["--plugin-sdk", ...args], { stdio: "pipe" });
  sdk("keygen", key);
  sdk("pack", folder, archive, key);
  await start();
  assert.equal(
    (
      await fetch(base + "/api/plugin-extensions/action", {
        method: "POST",
        body: "{}",
      })
    ).status,
    401,
  );
  const inspected = await api("/api/plugins/inspect", { path: archive });
  const installed = await api("/api/plugins/install", {
    packageRef: inspected.packageRef,
    digest: inspected.digest,
    approvePublisher: true,
  });
  const query = { pluginId: installed.manifest.id, digest: installed.digest };
  await api("/api/plugins/state", { ...query, action: "enable" });
  await api("/api/plugin-extensions/resource", { ...query, path: target });
  const credential = await api("/api/provider-credentials", {
    label: "Disposable config fixture",
    secret: "fixture-config-not-a-real-credential",
  });
  let provider = (await api("/api/snapshot")).providers.find(
    (p) => p.id === "openai",
  );
  await api(
    "/api/providers",
    {
      ...provider,
      ...credential,
      endpoint: "http://127.0.0.1:9/v1",
      model: "fixture",
      models: ["fixture"],
      configured: true,
    },
    200,
    "PUT",
  );
  provider = (await api("/api/snapshot")).providers.find(
    (p) => p.id === "openai",
  );
  const action = (action, expected = 200) =>
    api("/api/plugin-extensions/action", { ...query, action }, expected);
  const preview = () =>
    action({ kind: "prepare-config", providerId: provider.id });
  const plan = await preview();
  assert.equal(plan.canDiscard, true);
  assert.equal((await preview()).planId, plan.planId);
  assert.equal(fs.readFileSync(target, "utf8"), original);
  db = new DatabaseSync(path.join(data, "plugin-extensions.sqlite3"));
  const record = JSON.parse(
    db.prepare("SELECT record FROM plans WHERE id=?").get(plan.planId).record,
  );
  const seeded = [];
  for (let i = 0; i < 63; i++) {
    const copy = structuredClone(record);
    copy.id = randomUUID();
    copy.provider.revision += i + 100;
    db.prepare("INSERT INTO plans VALUES(?,?,?)").run(
      copy.id,
      query.pluginId,
      JSON.stringify(copy),
    );
    seeded.push(copy.id);
  }
  assert.equal(rows(), 64);
  assert.equal((await preview()).planId, plan.planId);
  assert.deepEqual((await history()).planCapacity, {
    used: 64,
    limit: 64,
    remaining: 0,
  });
  await api(
    "/api/providers",
    { ...provider, model: "fixture-changed", models: ["fixture-changed"] },
    200,
    "PUT",
  );
  const full = await action(
    { kind: "prepare-config", providerId: provider.id },
    400,
  );
  assert.equal(full.code, "plan_capacity_full");
  assert.match(full.message, /放弃.*未执行预览/);
  assert.equal(rows(), 64);
  passed(
    "真实 Wasm 预览重复与满额时均复用原编号；变更连接后满额明确拒绝，未改文件",
  );

  const protectedRecord = JSON.parse(
    db.prepare("SELECT record FROM plans WHERE id=?").get(seeded[0]).record,
  );
  protectedRecord.state = "applying";
  db.prepare("UPDATE plans SET record=? WHERE id=?").run(
    JSON.stringify(protectedRecord),
    seeded[0],
  );
  assert.equal(
    (await action({ kind: "discard-config", planId: seeded[0] }, 409)).code,
    "result_pending",
  );
  assert.equal(
    db.prepare("SELECT record FROM plans WHERE id=?").get(seeded[0]).record,
    JSON.stringify(protectedRecord),
  );
  assert.equal(rows(), 64);
  await action({ kind: "discard-config", planId: plan.planId });
  assert.equal(rows(), 63);
  const next = await preview();
  assert.notEqual(next.planId, plan.planId);
  assert.equal(rows(), 64);
  assert.equal(fs.readFileSync(target, "utf8"), original);
  const applied = await action({
    kind: "apply-config",
    planId: next.planId,
    revision: next.revision,
  });
  assert.equal(applied.state, "applied");
  assert.equal(applied.canDiscard, false);
  const changedHash = hash(fs.readFileSync(target));
  assert.notEqual(changedHash, hash(original));
  assert.equal(
    (await action({ kind: "discard-config", planId: next.planId }, 409)).code,
    "result_pending",
  );
  assert.equal(hash(fs.readFileSync(target)), changedHash);
  assert.equal(
    JSON.stringify(await history()).includes(
      "fixture-config-not-a-real-credential",
    ),
    false,
  );
  passed(
    "只删除原未执行预览并恢复一个名额；真实应用与待核查记录均拒绝放弃，凭据不返回界面",
  );

  await api("/api/plugins/state", { ...query, action: "remove" });
  assert.equal(
    (await action({ kind: "restore-config", operationId: next.planId })).state,
    "restored",
  );
  assert.equal(fs.readFileSync(target, "utf8"), original);
  assert.equal(
    (await action({ kind: "discard-config", planId: next.planId }, 409)).code,
    "result_pending",
  );
  passed("插件卸载后宿主仍可恢复原文件；恢复记录与加密备份继续保留");

  await stop();
  await start();
  assert.equal(rows(), 64);
  const wrong = await api(
    "/api/plugin-extensions/action",
    {
      ...query,
      digest: "f".repeat(64),
      action: { kind: "discard-config", planId: seeded[1] },
    },
    403,
  );
  assert.equal(wrong.code, "permission_denied");
  assert.equal(rows(), 64);
  await action({ kind: "discard-config", planId: seeded[1] });
  assert.equal(rows(), 63);
  assert.deepEqual((await history()).planCapacity, {
    used: 63,
    limit: 64,
    remaining: 1,
  });
  assert.equal(fs.readFileSync(target, "utf8"), original);
  passed(
    "卸载后重启保留全部记录；旧身份可释放未执行预览，错误摘要被拒绝且不访问凭据库",
  );

  db.exec("ALTER TABLE plans RENAME TO preserved_plans");
  assert.equal(
    (await action({ kind: "discard-config", planId: seeded[2] }, 400)).code,
    "unavailable",
  );
  db.exec("ALTER TABLE preserved_plans RENAME TO plans");
  assert.equal(rows(), 63);
  assert.equal(fs.readFileSync(target, "utf8"), original);
  passed("SQL 失败不伪造名额释放，也不丢弃文件与恢复记录");
  report.complete = true;
} finally {
  db?.close();
  await stop();
  fs.rmSync(root, { recursive: true, force: true });
  report.ownedServiceStopped = true;
  report.temporaryDataRemoved = true;
  if (process.env.PLUGIN_CONFIG_LIFECYCLE_REPORT) {
    const destination = path.resolve(
      process.env.PLUGIN_CONFIG_LIFECYCLE_REPORT,
    );
    fs.mkdirSync(path.dirname(destination), { recursive: true });
    fs.writeFileSync(destination, JSON.stringify(report, null, 2) + "\n");
  }
}
