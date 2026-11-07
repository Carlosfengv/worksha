import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { randomUUID, createHash } from "node:crypto";
import { createServer } from "node:http";
import { DatabaseSync } from "node:sqlite";

const binary = path.resolve(
  process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
    "target/debug/local-agent-service",
);
const previousBinary = process.env.SETTINGS_SCOPE_SOURCE_BINARY
  ? path.resolve(process.env.SETTINGS_SCOPE_SOURCE_BINARY)
  : null;
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "local-agent-settings-scope-")),
);
const instances = ["a", "b"].map((name) => {
  const folder = path.join(root, name),
    home = path.join(folder, "home");
  fs.mkdirSync(home, { recursive: true });
  return {
    data: path.join(folder, "data"),
    home,
    token: "",
    base: "",
    child: null,
  };
});
const [a, b] = instances;
const digest = (file) =>
  createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const report = {
  complete: false,
  binarySha256: digest(binary),
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
};
let db,
  dropNext = false,
  dropped = 0;
const proxy = createServer(async (req, res) => {
  try {
    let body = "";
    for await (const chunk of req) body += chunk;
    const result = await fetch(a.base + req.url, {
      method: req.method,
      headers: {
        authorization: `Bearer ${a.token}`,
        "content-type": "application/json",
      },
      body: body || undefined,
      signal: AbortSignal.timeout(10000),
    });
    const text = await result.text();
    if (dropNext && req.url === "/api/settings") {
      assert.equal(result.status, 200);
      dropNext = false;
      dropped++;
      res.destroy();
      return;
    }
    res.writeHead(result.status, { "content-type": "application/json" });
    res.end(text);
  } catch {
    res.destroy();
  }
});
async function boot(instance, executable = binary) {
  instance.token = randomUUID();
  const env = {
    ...process.env,
    LOCAL_AGENT_SOURCE_HOME: instance.home,
    LOCAL_AGENT_SERVICE_TOKEN: instance.token,
    LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
    LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
  };
  delete env.LOCAL_AGENT_VAULT_WORKER;
  instance.child = spawn(executable, ["--data-dir", instance.data], {
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  instance.child.stderr.resume();
  const lines = createInterface({ input: instance.child.stdout });
  const [line] = await once(lines, "line", {
    signal: AbortSignal.timeout(15000),
  });
  instance.base = `http://127.0.0.1:${JSON.parse(line).port}`;
}
async function api(
  instance,
  route,
  value,
  expected = 200,
  method = value === undefined ? "GET" : "PUT",
  endpoint = instance.base,
) {
  const response = await fetch(endpoint + route, {
    method,
    headers: {
      authorization: `Bearer ${instance.token}`,
      "content-type": "application/json",
    },
    body: value === undefined ? undefined : JSON.stringify(value),
    signal: AbortSignal.timeout(12000),
  });
  const result = await response.json();
  assert.equal(
    response.status,
    expected,
    `${route}: ${JSON.stringify(result)}`,
  );
  return result;
}
async function stop(instance) {
  const child = instance.child;
  if (child && child.exitCode === null && child.signalCode === null) {
    const exit = once(child, "exit", { signal: AbortSignal.timeout(12000) });
    await api(instance, "/api/runtime/shutdown", {}, 200, "POST");
    await exit;
  }
}
const savedQuery = (request) =>
  `/api/settings/scoped/operations/${request.operationId}?storeId=${request.storeId}`;
const passed = (description) => {
  report.cases.push(description);
  console.log(`通过：${description}`);
};
try {
  await new Promise((resolve) => proxy.listen(0, "127.0.0.1", resolve));
  let historical, historicalBytes;
  if (previousBinary) {
    report.sourceBinarySha256 = digest(previousBinary);
    await boot(a, previousBinary);
    const snapshot = await api(a, "/api/snapshot");
    assert.equal(snapshot.settingsWriteProtocol, undefined);
    historical = await api(a, "/api/settings", {
      value: { captureContent: false, retentionDays: 7 },
      expectedRevision: 0,
      operationId: randomUUID(),
    });
    await stop(a);
    const prior = new DatabaseSync(path.join(a.data, "local-agent.sqlite3"), {
      readOnly: true,
    });
    historicalBytes = prior
      .prepare(
        "SELECT request,result FROM settings_operations WHERE operation_id=?",
      )
      .get(historical.operationId);
    prior.close();
  }
  await boot(a);
  await boot(b);
  const initial = await api(a, "/api/snapshot"),
    protocol = initial.settingsWriteProtocol;
  assert.equal(protocol.version, 1);
  assert.equal(protocol.receiptLimit, 256);
  assert.equal(
    protocol.storeId,
    (await api(a, "/api/model-services")).writeProtocol.storeId,
  );
  const other = (await api(b, "/api/snapshot")).settingsWriteProtocol;
  assert.notEqual(other.storeId, protocol.storeId);
  db = new DatabaseSync(path.join(a.data, "local-agent.sqlite3"));
  assert.equal(db.prepare("PRAGMA user_version").get().user_version, 16);
  if (historical) {
    assert.deepEqual(
      await api(a, `/api/settings/operations/${historical.operationId}`),
      historical,
    );
    assert.equal(
      (
        await api(
          a,
          `/api/settings/scoped/operations/${historical.operationId}?storeId=${protocol.storeId}`,
          undefined,
          409,
        )
      ).code,
      "operation_not_scoped",
    );
    assert.deepEqual(
      db
        .prepare(
          "SELECT request,result FROM settings_operations WHERE operation_id=?",
        )
        .get(historical.operationId),
      historicalBytes,
    );
    passed(
      "真实旧版 schema 8 升级不改数据库身份或历史回执；旧只读接口保留，新查询不补贴来源",
    );
  }
  const baseRevision = initial.settingsRevision;
  const seed = await api(b, "/api/settings", {
    storeId: other.storeId,
    operationId: randomUUID(),
    expectedRevision: 0,
    value: { captureContent: false, retentionDays: 90 },
  });
  if (baseRevision === 0) {
    await api(a, "/api/settings", {
      storeId: protocol.storeId,
      operationId: randomUUID(),
      expectedRevision: 0,
      value: { captureContent: false, retentionDays: 7 },
    });
  }
  const request = {
    storeId: protocol.storeId,
    operationId: randomUUID(),
    expectedRevision: 1,
    value: { captureContent: true, retentionDays: 30 },
  };
  assert.equal(seed.revision, 1);
  assert.equal(
    (await api(b, "/api/settings", request, 409)).code,
    "operation_store_changed",
  );
  assert.equal(
    (await api(b, savedQuery(request), undefined, 409)).code,
    "operation_store_changed",
  );
  assert.deepEqual((await api(b, "/api/snapshot")).settings, seed.value);
  assert.equal((await api(b, "/api/snapshot")).settingsRevision, 1);
  const unscoped = { ...request };
  delete unscoped.storeId;
  assert.equal(
    (await api(a, "/api/settings", unscoped, 400)).code,
    "write_protocol_unavailable",
  );
  const unauthorized = await fetch(a.base + savedQuery(request));
  assert.equal(unauthorized.status, 401);
  await unauthorized.arrayBuffer();
  assert.equal(
    (await api(a, savedQuery(request), undefined, 404)).code,
    "operation_not_found",
  );
  passed(
    "两个真实后台同版本互不混用；跨库写入/查询及无身份写入拒绝，新查询仍受管理鉴权",
  );
  dropNext = true;
  await assert.rejects(
    api(
      a,
      "/api/settings",
      request,
      200,
      "PUT",
      `http://127.0.0.1:${proxy.address().port}`,
    ),
  );
  assert.equal(dropped, 1);
  const receipt = await api(a, savedQuery(request));
  assert.equal(receipt.storeId, request.storeId);
  assert.equal(receipt.revision, 2);
  assert.deepEqual(receipt.value, request.value);
  assert.deepEqual(await api(a, "/api/settings", request), receipt);
  assert.equal(
    (
      await api(
        a,
        "/api/settings",
        { ...request, value: { captureContent: false, retentionDays: 90 } },
        409,
      )
    ).code,
    "operation_conflict",
  );
  const parallelId = {
    ...request,
    storeId: other.storeId,
    value: { captureContent: false, retentionDays: 7 },
  };
  const bReceipt = await api(b, "/api/settings", parallelId);
  assert.equal(bReceipt.operationId, receipt.operationId);
  assert.notEqual(bReceipt.storeId, receipt.storeId);
  assert.equal(
    (await api(b, savedQuery(request), undefined, 409)).code,
    "operation_store_changed",
  );
  passed(
    "实际提交后断开回复，按原库查询与同编号重试不重复写；同编号不同参数拒绝，同编号跨库仍按来源隔离",
  );
  const next = await api(a, "/api/settings", {
    ...request,
    operationId: randomUUID(),
    expectedRevision: 2,
    value: { captureContent: false, retentionDays: 90 },
  });
  await stop(a);
  await boot(a);
  assert.equal(
    (await api(a, "/api/snapshot")).settingsWriteProtocol.storeId,
    protocol.storeId,
  );
  assert.deepEqual(await api(a, savedQuery(request)), receipt);
  assert.deepEqual(await api(a, "/api/settings", request), receipt);
  assert.equal((await api(a, "/api/snapshot")).settingsRevision, next.revision);
  passed("同库重启保留来源与原结果；旧回执不覆盖后续保存");
  const failure = {
    ...request,
    operationId: randomUUID(),
    expectedRevision: next.revision,
    value: { captureContent: true, retentionDays: 7 },
  };
  db.exec(
    "CREATE TRIGGER fail_settings_scope BEFORE INSERT ON settings_operations BEGIN SELECT RAISE(ABORT,'fixture'); END",
  );
  assert.equal(
    (await api(a, "/api/settings", failure, 503)).code,
    "unavailable",
  );
  assert.equal((await api(a, "/api/snapshot")).settingsRevision, next.revision);
  assert.equal(
    (await api(a, savedQuery(failure), undefined, 404)).code,
    "operation_not_found",
  );
  db.exec("DROP TRIGGER fail_settings_scope");
  const receipts = await Promise.all(
    Array.from({ length: 12 }, () => api(a, "/api/settings", failure)),
  );
  assert(
    receipts.every((r) => JSON.stringify(r) === JSON.stringify(receipts[0])),
  );
  assert.equal(
    db
      .prepare(
        "SELECT count(*) AS count FROM settings_operations WHERE operation_id=?",
      )
      .get(failure.operationId).count,
    1,
  );
  assert.equal(
    (await api(a, "/api/snapshot")).settingsRevision,
    next.revision + 1,
  );
  passed("回执 SQL 失败与设置版本原子回滚；同编号 12 个并发只提交一次");
  const original = db
    .prepare("SELECT result FROM settings_operations WHERE operation_id=?")
    .get(failure.operationId).result;
  const corrupt = JSON.stringify({
    ...JSON.parse(original),
    storeId: other.storeId,
  });
  db.prepare(
    "UPDATE settings_operations SET result=? WHERE operation_id=?",
  ).run(corrupt, failure.operationId);
  assert.equal(
    (await api(a, savedQuery(failure), undefined, 503)).code,
    "invalid_record",
  );
  assert.equal(
    (await api(a, "/api/settings", failure, 503)).code,
    "invalid_record",
  );
  assert.equal(
    db
      .prepare("SELECT result FROM settings_operations WHERE operation_id=?")
      .get(failure.operationId).result,
    corrupt,
  );
  db.prepare(
    "UPDATE settings_operations SET result=? WHERE operation_id=?",
  ).run(original, failure.operationId);
  assert.deepEqual(await api(a, savedQuery(failure)), receipts[0]);
  passed("损坏回执不冒充成功或缺失，不被重试覆盖；仅修复临时夹具后原结果可读");
  report.droppedCommittedReplies = dropped;
  report.complete = true;
} finally {
  db?.close();
  for (const instance of instances) {
    try {
      await stop(instance);
    } catch {
      const child = instance.child;
      if (child && child.exitCode === null && child.signalCode === null) {
        child.kill("SIGKILL");
        await once(child, "exit");
      }
    }
  }
  proxy.closeAllConnections();
  await new Promise((resolve) => proxy.close(resolve));
  report.processesExited = instances.every(
    (v) => !v.child || v.child.exitCode !== null || v.child.signalCode !== null,
  );
  fs.rmSync(root, { recursive: true, force: true });
  report.temporaryRootRemoved = !fs.existsSync(root);
  if (process.env.SETTINGS_SCOPE_REPORT) {
    const destination = path.resolve(process.env.SETTINGS_SCOPE_REPORT);
    fs.mkdirSync(path.dirname(destination), { recursive: true });
    fs.writeFileSync(destination, JSON.stringify(report, null, 2) + "\n");
  }
}
console.log(JSON.stringify(report, null, 2));
