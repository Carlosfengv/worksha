import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { randomUUID, createHash } from "node:crypto";
import { createServer } from "node:http";
import { createServer as tcpServer } from "node:net";
import { DatabaseSync } from "node:sqlite";

const binary = path.resolve(
  process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
    "target/debug/local-agent-service",
);
const oldBinary = process.env.LAN_OPERATIONS_MIGRATION_BINARY
  ? path.resolve(process.env.LAN_OPERATIONS_MIGRATION_BINARY)
  : null;
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "local-agent-lan-operations-")),
);
const data = path.join(root, "data"),
  home = path.join(root, "home");
fs.mkdirSync(home, { mode: 0o700 });
const env = {
  ...process.env,
  LOCAL_AGENT_SOURCE_HOME: home,
  LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
  LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
  NO_PROXY: "*",
};
delete env.LOCAL_AGENT_VAULT_WORKER;
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const report = {
  complete: false,
  binarySha256: hash(fs.readFileSync(binary)),
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
};
const passed = (name) => {
  report.cases.push(name);
  console.log(`通过：${name}`);
};
let child,
  base,
  token,
  dropNext = false,
  dropped = 0;
const observedKeys = new Set();
const proxy = createServer(async (req, res) => {
  try {
    let body = "";
    for await (const part of req) body += part;
    const reply = await fetch(base + req.url, {
      method: req.method,
      headers: {
        authorization: `Bearer ${token}`,
        "content-type": "application/json",
      },
      body: body || undefined,
      signal: AbortSignal.timeout(10000),
    });
    const text = await reply.text();
    if (dropNext && req.url === "/api/gateway-lan/operations") {
      assert.equal(reply.status, 200);
      const result = JSON.parse(text);
      if (result.key) observedKeys.add(result.key);
      dropNext = false;
      dropped++;
      res.destroy();
      return;
    }
    res.writeHead(reply.status, { "content-type": "application/json" });
    res.end(text);
  } catch {
    res.destroy();
  }
});
const reservation = tcpServer();
await new Promise((resolve) => reservation.listen(0, "127.0.0.1", resolve));
const listen = `127.0.0.1:${reservation.address().port}`;
await new Promise((resolve) => reservation.close(resolve));
const run = promisify(execFile);
async function boot(selected = binary, expectedFailure = false) {
  let stderr = "";
  child = spawn(selected, ["gateway", "serve", "--data-dir", data], {
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const process = child;
  process.stderr.on("data", (b) => {
    stderr += b;
  });
  const lines = createInterface({ input: process.stdout });
  let timer;
  try {
    const state = await Promise.race([
      once(lines, "line").then(([line]) => ({ ready: JSON.parse(line) })),
      once(process, "exit").then(([code]) => ({ exited: code })),
      new Promise((_, reject) => {
        timer = setTimeout(
          () => reject(Error("isolated service readiness timed out")),
          15000,
        );
      }),
    ]);
    if (expectedFailure) {
      assert.ok(
        "exited" in state,
        "corrupt / foreign / old-format service must refuse startup",
      );
      assert.notEqual(state.exited, 0);
      assert.equal(
        fs.existsSync(path.join(data, "runtime/service.json")),
        false,
      );
      return;
    }
    assert.ok("ready" in state, stderr);
    const runtime = JSON.parse(
      fs.readFileSync(path.join(data, "runtime/service.json"), "utf8"),
    );
    base = `http://127.0.0.1:${runtime.port}`;
    token = runtime.token;
  } finally {
    clearTimeout(timer);
    lines.close();
  }
}
async function api(
  route,
  value,
  expected = 200,
  method = value === undefined ? "GET" : "POST",
  endpoint = base,
) {
  const response = await fetch(endpoint + route, {
    method,
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
    body: value === undefined ? undefined : JSON.stringify(value),
    signal: AbortSignal.timeout(12000),
  });
  const body = await response.json();
  assert.equal(
    response.status,
    expected,
    `${route}: ${body.code ?? body.message ?? response.status}`,
  );
  return body;
}
async function stop() {
  if (child && child.exitCode === null && child.signalCode === null) {
    const exit = once(child, "exit", { signal: AbortSignal.timeout(16000) });
    await api("/api/runtime/shutdown", {});
    await exit;
  }
}
const registryPath = path.join(data, "gateway-lan/clients.json");
const bytes = () => fs.readFileSync(registryPath);
const commit = (request, expected = 200, endpoint) =>
  api("/api/gateway-lan/operations", request, expected, "POST", endpoint);
const query = (request, expected = 200) =>
  api(
    `/api/gateway-lan/operations/${request.operationId}?storeId=${request.storeId}`,
    undefined,
    expected,
  );
const modelCatalog = (key) =>
  fetch(`http://${listen}/v1/models`, {
    headers: { authorization: `Bearer ${key}` },
    signal: AbortSignal.timeout(5000),
  });
try {
  await new Promise((resolve) => proxy.listen(0, "127.0.0.1", resolve));
  const proxyBase = `http://127.0.0.1:${proxy.address().port}`;
  await run(
    binary,
    [
      "gateway",
      "configure",
      "--data-dir",
      data,
      "--listen",
      listen,
      "--concurrency",
      "2",
    ],
    { env, timeout: 15000 },
  );
  if (oldBinary)
    report.migrationBinarySha256 = hash(fs.readFileSync(oldBinary));
  await boot(oldBinary ?? binary);
  await api(
    "/api/providers",
    {
      id: "anthropic",
      name: "LAN receipt fixture",
      endpoint: "http://127.0.0.1:9/v1",
      protocol: "messages",
      model: "fixture",
      models: ["fixture"],
      configured: false,
    },
    200,
    "PUT",
  );
  const draft = {
    name: "测试客户端",
    agent: "claude",
    providerId: "anthropic",
    models: ["fixture"],
    concurrency: 1,
  };
  const legacy = await api("/api/gateway-lan/clients", draft);
  const legacyBytes = bytes();
  assert.ok(Array.isArray(JSON.parse(legacyBytes)));
  await stop();
  await boot();
  let status = await api("/api/gateway-lan");
  const storeId = status.writeProtocol.storeId;
  assert.equal(status.writeProtocol.version, 1);
  assert.equal(status.writeProtocol.secretReplay, false);
  assert.equal((await modelCatalog(legacy.key)).status, 200);
  const request = {
    storeId,
    operationId: randomUUID(),
    action: { kind: "create", draft },
  };
  assert.equal((await query(request, 404)).code, "operation_not_found");
  assert.equal(
    (await commit({ ...request, storeId: randomUUID() }, 409)).code,
    "operation_store_changed",
  );
  assert.deepEqual(bytes(), legacyBytes);
  passed(
    "旧版本哈希客户端与来源保持；旧无回执条目不补贴操作身份，异库写入拒绝",
  );

  dropNext = true;
  await assert.rejects(commit(request, 200, proxyBase));
  assert.equal(dropped, 1);
  const created = await query(request);
  assert.deepEqual(created.request, request);
  assert.equal(created.receipt.clientId, request.operationId);
  assert.equal(created.receipt.listen, listen);
  const afterCreate = bytes();
  assert.equal((await commit(request)).key, null);
  assert.deepEqual(bytes(), afterCreate);
  const backupName = `clients.pre-operations-v1-${hash(legacyBytes)}.json`;
  const backup = path.join(data, "gateway-lan", backupName);
  assert.deepEqual(fs.readFileSync(backup), legacyBytes);
  assert.equal(fs.statSync(backup).mode & 0o777, 0o600);
  assert.equal(fs.statSync(registryPath).mode & 0o777, 0o600);
  assert.equal(
    (
      await commit(
        {
          ...request,
          action: { kind: "create", draft: { ...draft, name: "changed" } },
        },
        409,
      )
    ).code,
    "operation_conflict",
  );
  assert.deepEqual(bytes(), afterCreate);
  passed(
    "真实提交后丢失回复：同编号可核查，重试不重发密钥、不重复签发；迁移备份逐字节保持",
  );

  await stop();
  await boot();
  assert.deepEqual(await query(request), created);
  assert.equal((await commit(request)).key, null);
  const parallel = { ...request, operationId: randomUUID() };
  const replies = await Promise.all(
    Array.from({ length: 12 }, () => commit(parallel)),
  );
  assert.equal(replies.filter((r) => r.key !== null).length, 1);
  const key = replies.find((r) => r.key !== null).key;
  observedKeys.add(key);
  assert.equal(new Set(replies.map((r) => JSON.stringify(r.outcome))).size, 1);
  assert.equal((await modelCatalog(key)).status, 200);
  status = await api("/api/gateway-lan");
  assert.equal(status.clients.length, 3);
  passed("重启后原回执一致；12 个并发同编号只生成一个客户端与一次密钥回复");

  const revoke = {
    storeId,
    operationId: randomUUID(),
    action: { kind: "revoke", clientId: parallel.operationId },
  };
  dropNext = true;
  await assert.rejects(commit(revoke, 200, proxyBase));
  assert.equal(dropped, 2);
  const revoked = await query(revoke);
  assert.deepEqual(revoked.request, revoke);
  assert.equal(revoked.receipt.action, "revoke");
  const revokedBytes = bytes();
  assert.equal((await modelCatalog(key)).status, 401);
  assert.equal((await modelCatalog(legacy.key)).status, 200);
  const revocations = await Promise.all(
    Array.from({ length: 8 }, () => commit(revoke)),
  );
  assert.ok(
    revocations.every(
      (r) =>
        r.key === null && r.outcome.receipt.operationId === revoke.operationId,
    ),
  );
  await commit({ ...revoke, operationId: randomUUID() }, 409);
  assert.deepEqual(bytes(), revokedBytes);
  await stop();
  await boot();
  assert.deepEqual(await query(revoke), revoked);
  assert.equal((await modelCatalog(key)).status, 401);
  assert.equal((await modelCatalog(legacy.key)).status, 200);
  passed(
    "撤销回复丢失与并发重放可核查；重启仍撤销，其他客户端和既有额度不受影响",
  );

  const cli = async (verb, args) =>
    JSON.parse(
      (
        await run(binary, ["gateway", verb, "--data-dir", data, ...args], {
          env,
          timeout: 15000,
        })
      ).stdout,
    );
  assert.deepEqual(
    await cli("client-operation", [
      "--operation-id",
      revoke.operationId,
      "--store-id",
      storeId,
    ]),
    revoked,
  );
  assert.equal(
    (
      await cli("client-add", [
        "--name",
        draft.name,
        "--agent",
        draft.agent,
        "--provider",
        draft.providerId,
        "--models",
        "fixture",
        "--concurrency",
        "1",
        "--operation-id",
        request.operationId,
        "--store-id",
        storeId,
      ])
    ).key,
    null,
  );
  await assert.rejects(
    cli("client-revoke", [
      "--id",
      parallel.operationId,
      "--operation-id",
      revoke.operationId,
    ]),
  );
  assert.deepEqual(bytes(), revokedBytes);
  passed(
    "CLI 支持原编号 / 原来源查询与重放，缺少来源拒绝；不自动选择当前数据库或新编号",
  );

  const failing = { ...request, operationId: randomUUID() };
  const beforeFailure = bytes();
  fs.chmodSync(path.dirname(registryPath), 0o500);
  try {
    assert.equal((await commit(failing, 503)).code, "gateway_result_unknown");
  } finally {
    fs.chmodSync(path.dirname(registryPath), 0o700);
  }
  assert.deepEqual(bytes(), beforeFailure);
  assert.equal((await query(failing, 404)).code, "operation_not_found");
  const recovered = await commit(failing);
  assert.ok(recovered.key);
  observedKeys.add(recovered.key);
  assert.equal((await commit(failing)).key, null);
  assert.equal((await api("/api/gateway-lan")).clients.length, 4);
  passed(
    "真实目录写权限失败保留原记录；原编号未查到后可显式重试，不新增重复客户端",
  );

  // Receipt replay must not depend on a provider still allowing the old model.
  const snapshot = await api("/api/snapshot");
  const provider = snapshot.providers.find((p) => p.id === "anthropic");
  await api(
    "/api/providers",
    { ...provider, disabledModels: ["fixture"] },
    200,
    "PUT",
  );
  assert.equal((await commit(failing)).key, null);
  assert.equal(
    (await commit({ ...request, operationId: randomUUID() }, 400)).code,
    "invalid_input",
  );
  await commit({ ...request, operationId: randomUUID(), extra: true }, 400);
  await commit({ ...request, operationId: "invalid" }, 400);
  await api(
    `/api/gateway-lan/operations/${request.operationId}`,
    undefined,
    400,
  );
  await api(
    `/api/gateway-lan/operations/${request.operationId}?storeId=${storeId}&extra=true`,
    undefined,
    400,
  );
  passed(
    "回执优先于当前 Provider 校验；原参数严格匹配，非法编号及额外字段拒绝",
  );

  await api(`/api/gateway-lan/clients/${request.operationId}/revoke`, {});
  assert.deepEqual(await query(request), created);
  assert.equal((await commit(request)).key, null);
  assert.equal(
    (await api("/api/gateway-lan")).clients.find(
      (c) => c.id === request.operationId,
    ).enabled,
    false,
  );
  passed(
    "旧撤销入口兼容新注册表；旧签发回执只表达历史提交，重放不会重新启用已撤销客户端",
  );

  const record = JSON.parse(bytes());
  for (const secret of [legacy.key, ...observedKeys]) {
    assert.ok(!bytes().includes(secret));
    assert.ok(!JSON.stringify(await api("/api/gateway-lan")).includes(secret));
    assert.ok(!JSON.stringify(await query(request)).includes(secret));
  }
  assert.equal(record.operations.length, 4);
  await stop();
  if (oldBinary) {
    await boot(oldBinary, true);
    assert.deepEqual(JSON.parse(bytes()), record);
    passed("升级后旧服务明确拒绝新格式，不能使用旧快照重新启用撤销密钥");
  }
  const committed = bytes();
  for (const mutate of [
    (v) => {
      v.operations[0].receipt.operationId = randomUUID();
    },
    (v) => {
      v.operations[0].request.action.draft.name = "corrupt";
    },
    (v) => {
      v.operations.push(v.operations[0]);
    },
  ]) {
    const bad = structuredClone(record);
    mutate(bad);
    fs.writeFileSync(registryPath, JSON.stringify(bad), { mode: 0o600 });
    const expected = bytes();
    await boot(binary, true);
    assert.deepEqual(bytes(), expected);
  }
  fs.writeFileSync(registryPath, committed);
  fs.renameSync(registryPath, registryPath + ".held");
  await boot(binary, true);
  assert.equal(fs.existsSync(registryPath), false);
  fs.renameSync(registryPath + ".held", registryPath);
  const marker = path.join(data, "gateway-lan/operations-v1.json");
  fs.renameSync(marker, marker + ".held");
  await boot(binary, true);
  assert.equal(fs.existsSync(marker), false);
  fs.renameSync(marker + ".held", marker);
  passed(
    "回执身份、原请求、重复记录、缺失注册表 / 迁移标记均拒绝启动并保留原字节",
  );

  const db = new DatabaseSync(path.join(data, "local-agent.sqlite3"));
  db.prepare(
    "UPDATE model_service_operation_scope SET store_id=? WHERE id=1",
  ).run(randomUUID());
  db.close();
  await boot(binary, true);
  assert.deepEqual(bytes(), committed);
  const restore = new DatabaseSync(path.join(data, "local-agent.sqlite3"));
  restore
    .prepare("UPDATE model_service_operation_scope SET store_id=? WHERE id=1")
    .run(storeId);
  restore.close();
  await boot();
  assert.deepEqual(await query(request), created);
  await stop();
  passed(
    "客户端回执绑定原数据库；更换数据库身份不会重分配来源或覆盖客户端记录",
  );
  report.complete = true;
  report.droppedReplies = dropped;
  report.secretReplay = false;
  report.schemaVersion = 8;
} finally {
  try {
    await stop();
  } catch {
    if (child && child.exitCode === null && child.signalCode === null) {
      const exit = once(child, "exit", { signal: AbortSignal.timeout(5000) });
      child.kill("SIGKILL");
      await exit;
    }
  }
  await new Promise((resolve) => proxy.close(resolve));
  report.ownedProcessesStopped =
    !child || child.exitCode !== null || child.signalCode !== null;
  fs.rmSync(root, { recursive: true, force: true });
  report.temporaryDataRemoved = !fs.existsSync(root);
  const destination = process.env.LAN_OPERATIONS_REPORT;
  if (destination) {
    fs.mkdirSync(path.dirname(destination), { recursive: true });
    fs.writeFileSync(destination, JSON.stringify(report, null, 2) + "\n");
  }
}
