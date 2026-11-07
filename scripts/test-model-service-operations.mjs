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
const migrationBinary = process.env.MODEL_SERVICE_MIGRATION_SOURCE_BINARY
  ? path.resolve(process.env.MODEL_SERVICE_MIGRATION_SOURCE_BINARY)
  : null;
const root = fs.realpathSync(
    fs.mkdtempSync(path.join(tmpdir(), "local-agent-model-operations-")),
  ),
  data = path.join(root, "data"),
  home = path.join(root, "home");
fs.mkdirSync(home);
const env = {
  ...process.env,
  LOCAL_AGENT_SOURCE_HOME: home,
  LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
  LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
};
delete env.LOCAL_AGENT_VAULT_WORKER;
const report = {
  complete: false,
  binarySha256: createHash("sha256")
    .update(fs.readFileSync(binary))
    .digest("hex"),
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
};
let child,
  base,
  token,
  db,
  fakeCalls = 0,
  dropNext = false,
  dropped = 0;
const upstream = createServer(async (req, res) => {
  for await (const _ of req) {
  }
  fakeCalls++;
  res.writeHead(200, { "content-type": "application/json" });
  res.end(
    JSON.stringify({
      id: "fixture",
      type: "message",
      role: "assistant",
      model: "fixture",
      content: [{ type: "text", text: "fixture" }],
      stop_reason: "end_turn",
      usage: { input_tokens: 1, output_tokens: 1 },
    }),
  );
});
const proxy = createServer(async (req, res) => {
  try {
    let body = "";
    for await (const chunk of req) body += chunk;
    const response = await fetch(base + req.url, {
      method: req.method,
      headers: {
        authorization: `Bearer ${token}`,
        "content-type": "application/json",
      },
      body: body || undefined,
      signal: AbortSignal.timeout(10000),
    });
    const text = await response.text();
    if (dropNext && req.url === "/api/model-service-operations") {
      dropNext = false;
      dropped++;
      res.destroy();
      return;
    }
    res.writeHead(response.status, { "content-type": "application/json" });
    res.end(text);
  } catch {
    res.destroy();
  }
});
async function boot(selectedBinary = binary) {
  token = randomUUID();
  child = spawn(selectedBinary, ["--data-dir", data], {
    env: { ...env, LOCAL_AGENT_SERVICE_TOKEN: token },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stderr.resume();
  const lines = createInterface({ input: child.stdout });
  const [line] = await once(lines, "line", {
    signal: AbortSignal.timeout(15000),
  });
  base = `http://127.0.0.1:${JSON.parse(line).port}`;
}
async function api(
  route,
  value,
  status = 200,
  method = value === undefined ? "GET" : "POST",
  endpoint = base,
) {
  const r = await fetch(endpoint + route, {
    method,
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
    body: value === undefined ? undefined : JSON.stringify(value),
    signal: AbortSignal.timeout(12000),
  });
  const v = await r.json();
  assert.equal(r.status, status, `${route}: ${JSON.stringify(v)}`);
  return v;
}
async function stop() {
  if (child && child.exitCode === null && child.signalCode === null) {
    const exit = once(child, "exit", { signal: AbortSignal.timeout(12000) });
    await api("/api/runtime/shutdown", {});
    await exit;
  }
}
const passed = (name) => {
  report.cases.push(name);
  console.log(`通过：${name}`);
};
try {
  await new Promise((r) => upstream.listen(0, "127.0.0.1", r));
  await new Promise((r) => proxy.listen(0, "127.0.0.1", r));
  let oldSettings, migrationProvider;
  if (migrationBinary) {
    report.migrationSourceSha256 = createHash("sha256")
      .update(fs.readFileSync(migrationBinary))
      .digest("hex");
    await boot(migrationBinary);
    const previous = new DatabaseSync(path.join(data, "local-agent.sqlite3"), {
      readOnly: true,
    });
    assert.equal(previous.prepare("PRAGMA user_version").get().user_version, 7);
    previous.close();
    oldSettings = await api(
      "/api/settings",
      {
        value: { captureContent: true, retentionDays: 90 },
        expectedRevision: 0,
        operationId: randomUUID(),
      },
      200,
      "PUT",
    );
    migrationProvider = {
      id: `custom-${randomUUID()}`,
      name: "Migration fixture",
      vendor: "custom",
      endpoint: "http://127.0.0.1:9",
      protocol: "messages",
      model: "fixture",
      models: ["fixture"],
      configured: true,
      credentialRef: null,
      credentialLabel: null,
      revision: 0,
      category: "local",
    };
    await api("/api/providers", migrationProvider, 200, "PUT");
    await stop();
  }
  await boot();
  if (oldSettings) {
    assert.deepEqual(
      await api(`/api/settings/operations/${oldSettings.operationId}`),
      oldSettings,
    );
    const snapshot = await api("/api/snapshot");
    assert.deepEqual(snapshot.settings, oldSettings.value);
    assert.equal(snapshot.settingsRevision, 1);
    assert.equal(
      snapshot.providers.find((p) => p.id === migrationProvider.id).revision,
      1,
    );
    const backup = new DatabaseSync(
      path.join(data, "local-agent.pre-model-service-operations-v7.sqlite3"),
      { readOnly: true },
    );
    assert.equal(backup.prepare("PRAGMA user_version").get().user_version, 7);
    assert.equal(backup.prepare("PRAGMA quick_check").get().quick_check, "ok");
    assert.equal(
      backup
        .prepare(
          "SELECT count(*) AS count FROM sqlite_master WHERE name='model_service_operations'",
        )
        .get().count,
      0,
    );
    assert.equal(
      backup.prepare("SELECT revision FROM settings_version WHERE id=1").get()
        .revision,
      1,
    );
    assert.equal(
      JSON.parse(
        backup
          .prepare("SELECT value FROM providers WHERE id=?")
          .get(migrationProvider.id).value,
      ).name,
      migrationProvider.name,
    );
    backup.close();
    passed(
      "真实 schema 7 服务保存后升级至当前服务；设置、旧回执与 Provider 保留，schema 7 一致性备份未改写",
    );
  }
  const protocol = (await api("/api/model-services")).writeProtocol;
  assert.equal(protocol.version, 1);
  assert.equal(protocol.receiptLimit, 256);
  assert.match(protocol.storeId, /^[0-9a-f-]{36}$/);
  const storeId = protocol.storeId;
  const provider = {
    id: `custom-${randomUUID()}`,
    name: "Operations fixture",
    vendor: "custom",
    protocol: "messages",
    endpoint: `http://127.0.0.1:${upstream.address().port}`,
    model: "fixture",
    models: ["fixture"],
    configured: true,
    credentialRef: null,
    credentialLabel: null,
    revision: 0,
    category: "local",
  };
  await api("/api/providers", provider, 200, "PUT");
  await api(
    "/api/model-service-members/verify",
    {
      storeId: randomUUID(),
      providerId: provider.id,
      expectedRevision: 1,
      model: "fixture",
    },
    409,
  );
  assert.equal(fakeCalls, 0);
  assert.equal(
    (
      await api("/api/model-service-members/verify", {
        storeId,
        providerId: provider.id,
        expectedRevision: 1,
        model: "fixture",
      })
    ).state,
    "verified",
  );
  assert.equal(fakeCalls, 1);
  const model = {
    id: randomUUID(),
    name: "Model",
    providerId: provider.id,
    model: "fixture",
    capabilities: {
      contextWindow: 32000,
      maxOutputTokens: 8192,
      tools: true,
      thinking: false,
      inputModalities: ["text"],
    },
  };
  const draft = {
    id: randomUUID(),
    revision: 0,
    name: "Receipt fixture",
    defaultModelId: model.id,
    models: [model],
  };
  const request = {
    storeId,
    operationId: randomUUID(),
    action: { kind: "save", draft },
  };
  await api(
    "/api/model-service-operations",
    { ...request, storeId: randomUUID() },
    409,
  );
  assert.equal(fakeCalls, 1);
  dropNext = true;
  await assert.rejects(
    api(
      "/api/model-service-operations",
      request,
      200,
      "POST",
      `http://127.0.0.1:${proxy.address().port}`,
    ),
  );
  assert.equal(dropped, 1);
  const query = (r) =>
    `/api/model-service-operations/${r.operationId}?storeId=${r.storeId}`;
  const first = await api(query(request));
  assert.equal(first.storeId, storeId);
  assert.equal(first.operationId, request.operationId);
  assert.equal(first.committedRevision, 1);
  assert.equal(first.service.id, draft.id);
  assert.equal(
    (await api("/api/model-service-operations", request)).service.versionId,
    first.service.versionId,
  );
  assert.equal(fakeCalls, 1);
  await api(
    "/api/model-service-operations",
    {
      ...request,
      action: { kind: "save", draft: { ...draft, name: "changed payload" } },
    },
    409,
  );
  await api(
    `/api/model-service-operations/${request.operationId}?storeId=${randomUUID()}`,
    undefined,
    409,
  );
  passed(
    "数据库身份在验证和提交前检查；提交后断开回复可核查、同编号重试不重发模型或发布版本，换载荷拒绝",
  );
  const cached = path.join(data, "providers", "verifications.json");
  // Locate only the fixture's persisted verification file; no user home is read.
  function findVerification(folder) {
    for (const entry of fs.readdirSync(folder, { withFileTypes: true })) {
      const p = path.join(folder, entry.name);
      if (entry.isFile() && entry.name === "verifications.json") return p;
      if (entry.isDirectory()) {
        const found = findVerification(p);
        if (found) return found;
      }
    }
  }
  const verificationFile = fs.existsSync(cached)
    ? cached
    : findVerification(data);
  assert(verificationFile);
  const originalVerification = fs.readFileSync(verificationFile);
  await stop();
  fs.writeFileSync(verificationFile, "[]");
  await boot();
  assert.equal(
    (await api("/api/model-services")).writeProtocol.storeId,
    storeId,
  );
  assert.equal(
    (await api("/api/model-service-operations", request)).service.versionId,
    first.service.versionId,
  );
  assert.equal(fakeCalls, 1);
  const later = {
    storeId,
    operationId: randomUUID(),
    action: {
      kind: "save",
      draft: { ...draft, revision: 1, name: "Later publication" },
    },
  };
  await api("/api/model-service-operations", later);
  assert.equal((await api(query(request))).committedRevision, 1);
  assert.equal((await api("/api/model-services")).services[0].revision, 2);
  assert.equal(fakeCalls, 1);
  passed(
    "重启保留数据库身份与回执；验证缓存为空时旧操作仍可重放，原回执不覆盖后续版本",
  );
  fs.writeFileSync(verificationFile, originalVerification);
  db = new DatabaseSync(path.join(data, "local-agent.sqlite3"));
  assert.equal(db.prepare("PRAGMA user_version").get().user_version, 16);
  db.exec(
    "CREATE TRIGGER reject_operation_receipt BEFORE INSERT ON model_service_operations BEGIN SELECT RAISE(ABORT,'fixture'); END;",
  );
  const failure = {
    storeId,
    operationId: randomUUID(),
    action: {
      kind: "save",
      draft: { ...draft, id: randomUUID(), name: "Rollback fixture" },
    },
  };
  await api("/api/model-service-operations", failure, 503);
  assert.equal(
    db
      .prepare("SELECT count(*) n FROM model_services WHERE id=?")
      .get(failure.action.draft.id).n,
    0,
  );
  await api(query(failure), undefined, 404);
  db.exec("DROP TRIGGER reject_operation_receipt");
  const [one, two] = await Promise.all([
    api("/api/model-service-operations", failure),
    api("/api/model-service-operations", failure),
  ]);
  assert.equal(one.service.versionId, two.service.versionId);
  assert.equal(
    db
      .prepare("SELECT count(*) n FROM model_service_versions WHERE service=?")
      .get(failure.action.draft.id).n,
    1,
  );
  assert.equal(fakeCalls, 1);
  const raw = db
    .prepare(
      "SELECT result,service_version FROM model_service_operations WHERE operation_id=?",
    )
    .get(failure.operationId);
  assert.equal(raw.service_version, one.service.versionId);
  assert(!raw.result.includes("http://"));
  const prior = raw.result;
  db.prepare(
    "UPDATE model_service_operations SET result='{}' WHERE operation_id=?",
  ).run(failure.operationId);
  assert.equal(
    (await api(query(failure), undefined, 503)).code,
    "invalid_record",
  );
  db.prepare(
    "UPDATE model_service_operations SET result=? WHERE operation_id=?",
  ).run(prior, failure.operationId);
  passed(
    "回执写失败与发布一起回滚；并发相同编号只有一个版本，引用不可变快照且损坏记录不伪装为空",
  );
  const deletion = {
    storeId,
    operationId: randomUUID(),
    action: { kind: "delete", serviceId: draft.id, expectedRevision: 2 },
  };
  dropNext = true;
  await assert.rejects(
    api(
      "/api/model-service-operations",
      deletion,
      200,
      "POST",
      `http://127.0.0.1:${proxy.address().port}`,
    ),
  );
  assert.equal(dropped, 2);
  const receipt = await api(query(deletion));
  assert.equal(receipt.action, "delete");
  assert.equal(receipt.service, null);
  assert.equal(receipt.committedRevision, 3);
  assert.equal(
    (await api("/api/model-service-operations", deletion)).committedRevision,
    3,
  );
  assert.equal(fakeCalls, 1);
  await stop();
  await boot();
  assert.equal((await api(query(deletion))).committedRevision, 3);
  assert.equal(
    (await api(query(request))).service.versionId,
    first.service.versionId,
  );
  assert.equal(fakeCalls, 1);
  passed(
    "删除回复丢失后可核查并跨重启重放；已删除服务不影响原发布回执，核查和重试不新增模型调用",
  );
  await stop();
  const originalReceipts = db
    .prepare(
      "SELECT operation_id,request,result,service_version FROM model_service_operations ORDER BY sequence",
    )
    .all();
  db.exec("DELETE FROM model_service_operation_scope");
  child = spawn(binary, ["--data-dir", data], {
    env: { ...env, LOCAL_AGENT_SERVICE_TOKEN: randomUUID() },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stdout.resume();
  child.stderr.resume();
  const [exitCode, signal] = await once(child, "exit", {
    signal: AbortSignal.timeout(15000),
  });
  assert.equal(signal, null);
  assert.notEqual(exitCode, 0);
  assert.equal(
    db
      .prepare("SELECT count(*) AS count FROM model_service_operation_scope")
      .get().count,
    0,
  );
  assert.deepEqual(
    db
      .prepare(
        "SELECT operation_id,request,result,service_version FROM model_service_operations ORDER BY sequence",
      )
      .all(),
    originalReceipts,
  );
  db.prepare("INSERT INTO model_service_operation_scope VALUES(1,?)").run(
    storeId,
  );
  await boot();
  assert.equal(
    (await api("/api/model-services")).writeProtocol.storeId,
    storeId,
  );
  assert.deepEqual(await api(query(deletion)), receipt);
  assert.equal(fakeCalls, 1);
  passed(
    "schema 16 身份丢失时真实服务启动失败且不替换身份或回执；只修复临时夹具后原结果仍可核查",
  );
  report.fakeModelCalls = fakeCalls;
  report.droppedCommittedReplies = dropped;
  report.complete = true;
} finally {
  db?.close();
  try {
    await stop();
  } catch {
    if (child && child.exitCode === null && child.signalCode === null) {
      child.kill("SIGKILL");
      await once(child, "exit");
    }
  }
  upstream.closeAllConnections();
  proxy.closeAllConnections();
  await Promise.all([
    new Promise((r) => upstream.close(r)),
    new Promise((r) => proxy.close(r)),
  ]);
  report.processExited =
    !!child && (child.exitCode !== null || child.signalCode !== null);
  fs.rmSync(root, { recursive: true, force: true });
  report.temporaryRootRemoved = !fs.existsSync(root);
  if (process.env.MODEL_SERVICE_OPERATIONS_REPORT)
    fs.writeFileSync(
      process.env.MODEL_SERVICE_OPERATIONS_REPORT,
      JSON.stringify(report, null, 2) + "\n",
    );
}
console.log(JSON.stringify(report, null, 2));
