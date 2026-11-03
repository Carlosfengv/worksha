import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { randomUUID, createHash } from "node:crypto";
import { DatabaseSync } from "node:sqlite";
import { setTimeout as pause } from "node:timers/promises";
import http from "node:http";

const bundle = process.env.LOCAL_AGENT_TEST_DESKTOP_APP
  ? path.resolve(process.env.LOCAL_AGENT_TEST_DESKTOP_APP)
  : null;
const binary = bundle
  ? path.join(bundle, "Contents/MacOS/local-agent-service")
  : path.resolve(
      process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
        "target/debug/local-agent-service",
    );
const executable = bundle
  ? path.join(bundle, "Contents/MacOS/local-agent-desktop")
  : binary;
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "local-agent-retention-preview-")),
);
const data = path.join(root, "data"),
  home = path.join(root, "home"),
  observation = path.join(data, "observation");
fs.mkdirSync(observation, { recursive: true });
fs.mkdirSync(home);
const native = path.join(home, "native-sentinel.jsonl");
fs.writeFileSync(native, "private-native-sentinel\n");
const database = path.join(observation, "observations.sqlite3");
const db = new DatabaseSync(database);
db.exec(
  "PRAGMA journal_mode=WAL; CREATE TABLE model_observations(id TEXT PRIMARY KEY,started_at TEXT NOT NULL,revision INTEGER NOT NULL,value TEXT NOT NULL);",
);
const now = Date.now(),
  old = new Date(now - 120 * 86_400_000).toISOString(),
  recent = new Date(now).toISOString();
function row(id, logicalId, at = old, status = "success") {
  return {
    id,
    logicalId,
    agentId: "claude",
    agentProductId: "claude-code-cli",
    provider: "private-provider",
    endpoint: "https://private-endpoint.invalid",
    bindingRevision: 1,
    protocol: "messages",
    model: "fixture-model",
    sessionId: "private-session",
    startedAt: at,
    endedAt: status === "in_progress" ? null : at,
    status,
    revision: 1,
    usage: { inputTokens: 10, outputTokens: 2, totalTokens: 12 },
  };
}
const insert = db.prepare("INSERT INTO model_observations VALUES(?,?,?,?)");
function save(r) {
  insert.run(r.id, r.startedAt, r.revision, JSON.stringify(r));
}
db.exec("BEGIN");
for (let group = 0; group < 900; group++)
  for (let attempt = 0; attempt < 3; attempt++)
    save(row(`old-${group}-${attempt}`, `logical-${group}`));
save(row("retry-old", "mixed"));
save(row("retry-new", "mixed", recent));
save(row("active", "active", old, "in_progress"));
save(row("unknown", "unknown", old, "interrupted"));
const time = row("time", "time");
time.endedAt = null;
save(time);
const future = row("future", "future");
future.usage.future = null;
save(future);
insert.run("broken", old, 1, "{");
db.exec("COMMIT");
const source = () =>
  createHash("sha256")
    .update(
      JSON.stringify(
        db
          .prepare(
            "SELECT id,started_at,revision,value FROM model_observations ORDER BY id",
          )
          .all(),
      ),
    )
    .digest("hex");
const baseline = source(),
  nativeBaseline = fs.readFileSync(native, "utf8");
const report = {
  complete: false,
  mode: bundle ? "packaged-desktop" : "standalone-service",
  windowCheck: "not_exercised",
  desktopSha256: bundle
    ? createHash("sha256").update(fs.readFileSync(executable)).digest("hex")
    : null,
  binarySha256: createHash("sha256")
    .update(fs.readFileSync(binary))
    .digest("hex"),
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
};
let child, base, token, toolDb;
async function boot() {
  token = randomUUID();
  const env = {
    ...process.env,
    LOCAL_AGENT_SOURCE_HOME: home,
    LOCAL_AGENT_DATA_DIR: data,
    LOCAL_AGENT_SERVICE_TOKEN: token,
    LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
    LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
    TOKIO_WORKER_THREADS: "1",
  };
  delete env.LOCAL_AGENT_VAULT_WORKER;
  child = spawn(executable, bundle ? [] : ["--data-dir", data], {
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stderr.resume();
  if (bundle) {
    child.stdout.resume();
    const deadline = performance.now() + 15000;
    while (performance.now() < deadline) {
      assert.equal(
        child.exitCode,
        null,
        "isolated desktop exited before readiness",
      );
      const record = path.join(data, "runtime/service.json");
      if (fs.existsSync(record)) {
        const ready = JSON.parse(fs.readFileSync(record, "utf8"));
        assert(
          Number.isSafeInteger(ready.identity.pid) && ready.identity.pid > 0,
        );
        const processInfo = spawnSync(
          "/bin/ps",
          ["-p", String(ready.identity.pid), "-o", "ppid=,comm="],
          { encoding: "utf8" },
        );
        assert.equal(processInfo.status, 0);
        const owned = processInfo.stdout.trim().match(/^(\d+)\s+([\s\S]+)$/);
        assert(owned);
        assert.equal(Number(owned[1]), child.pid);
        assert.equal(fs.realpathSync(owned[2]), fs.realpathSync(binary));
        token = ready.token;
        base = `http://127.0.0.1:${ready.port}`;
        return;
      }
      await pause(50);
    }
    throw Error("isolated desktop readiness timed out");
  }
  const lines = createInterface({ input: child.stdout });
  const [line] = await once(lines, "line", {
    signal: AbortSignal.timeout(15000),
  });
  base = `http://127.0.0.1:${JSON.parse(line).port}`;
}
async function api(
  route,
  { value, method = "GET", status = 200, headers = {} } = {},
) {
  // Send the actual Host header. Fetch can replace a supplied Host with its URL.
  return new Promise((resolve, reject) => {
    const request = http.request(
      new URL(base + route),
      {
        method,
        headers: {
          authorization: `Bearer ${token}`,
          "content-type": "application/json",
          ...headers,
        },
      },
      (response) => {
        let text = "";
        response.setEncoding("utf8");
        response.on("error", reject);
        response.on("data", (chunk) => {
          text += chunk;
          if (text.length > 2 * 1024 * 1024)
            request.destroy(Error("fixture response exceeds bound"));
        });
        response.on("end", () => {
          try {
            assert.equal(response.statusCode, status, `${route}: ${text}`);
            let body;
            try {
              body = JSON.parse(text);
            } catch {
              body = { text };
            }
            resolve(body);
          } catch (error) {
            reject(error);
          }
        });
      },
    );
    request.setTimeout(15000, () =>
      request.destroy(Error("fixture request timed out")),
    );
    request.on("error", reject);
    request.end(value === undefined ? undefined : JSON.stringify(value));
  });
}
async function stop() {
  if (child && child.exitCode === null && child.signalCode === null) {
    const exit = once(child, "exit", { signal: AbortSignal.timeout(15000) });
    await api("/api/runtime/shutdown", { method: "POST", value: {} });
    if (bundle) {
      const deadline = performance.now() + 15000;
      while (
        fs.existsSync(path.join(data, "runtime/service.json")) &&
        performance.now() < deadline
      )
        await pause(50);
      assert(
        !fs.existsSync(path.join(data, "runtime/service.json")),
        "owned service must stop before its desktop",
      );
      child.kill("SIGTERM");
    }
    await exit;
  }
}
const passed = (name) => {
  report.cases.push(name);
  console.log(`通过：${name}`);
};
try {
  await boot();
  const snapshot = await api("/api/snapshot"),
    storeId = snapshot.settingsWriteProtocol.storeId;
  const route = (
    days = 30,
    revision = snapshot.settingsRevision,
    owner = storeId,
  ) =>
    `/api/settings/retention-preview?storeId=${owner}&expectedSettingsRevision=${revision}&days=${days}`;
  for (const days of [7, 30, 90]) {
    const result = await api(route(days));
    assert.equal(result.version, 1);
    assert.equal(result.storeId, storeId);
    assert.equal(result.days, days);
    assert.equal(result.scope, "committed_gateway_only");
    assert.equal(result.otherSources, "not_checked");
    assert.equal(result.aggregateRetention, "continuous");
    assert.equal(result.cleanupEnabled, false);
    assert.equal(result.extraArchiveEnabled, false);
    assert.equal(result.confirmationAvailable, false);
    assert.equal(result.gateway.attempts, 2707);
    assert.equal(result.gateway.logicalRequests, 905);
    assert.equal(result.gateway.expired, 900);
    assert.equal(result.gateway.withinWindow, 1);
    assert.deepEqual(result.gateway.protected, {
      active: 1,
      unknownOutcome: 1,
      unknownTime: 1,
      unsupportedSource: 1,
    });
    assert.equal(result.gateway.unreadableAttempts, 1);
    assert.equal(result.gateway.classificationComplete, false);
    assert.equal(
      Date.parse(result.capturedAt) - Date.parse(result.cutoff),
      days * 86_400_000,
    );
    assert.equal(
      Date.parse(result.validUntil) - Date.parse(result.capturedAt),
      300_000,
    );
    assert(!JSON.stringify(result).includes("private-"));
    assert.equal(source(), baseline);
    passed(
      `${days} 天：完整 2707 次尝试、905 个逻辑组，900 个超过期限，未知与保护单列，原字节不变`,
    );
  }
  const inventoryRoute = (
    days = 30,
    revision = snapshot.settingsRevision,
    owner = storeId,
  ) =>
    route(days, revision, owner).replace(
      "/retention-preview?",
      "/retention-inventory?",
    );
  const toolFile = path.join(data, "plugin-extensions.sqlite3");
  assert.equal(fs.existsSync(toolFile), false);
  const absent = await api(inventoryRoute());
  assert.equal(absent.version, 1);
  assert.equal(Object.hasOwn(absent, "activityCaches"), false);
  const contentRoute = (days = 30, revision = snapshot.settingsRevision) =>
    inventoryRoute(days, revision) + "&inventoryVersion=2";
  const absentContent = await api(contentRoute());
  assert.equal(absentContent.version, 2);
  for (const kind of ["gateway", "native"]) {
    assert.deepEqual(absentContent.activityCaches[kind], {
      status: "unverified",
      reason: "source_absent",
    });
    assert.equal(
      fs.existsSync(path.join(observation, `${kind}-activity-inputs.json`)),
      false,
    );
  }
  await api(inventoryRoute() + "&inventoryVersion=3", { status: 400 });
  passed(
    "新版摘要范围须显式请求 v2；旧 v1 形状保持，缺失摘要来源不初始化也不冒充空",
  );
  assert.equal(absent.scope, "product_owned_saved_metadata");
  assert.equal(absent.gateway.status, "checked");
  assert.deepEqual(absent.tools, {
    status: "unverified",
    reason: "source_absent",
  });
  assert.equal(fs.existsSync(toolFile), false);
  passed("综合预览保留已核查网关，工具来源缺失不显示为零，也不初始化数据库");
  toolDb = new DatabaseSync(toolFile);
  toolDb.exec(
    "PRAGMA journal_mode=WAL;CREATE TABLE extension_version(version INTEGER NOT NULL);INSERT INTO extension_version VALUES(1);CREATE TABLE connector_calls(id INTEGER PRIMARY KEY,plugin_id TEXT NOT NULL,identity TEXT NOT NULL,record TEXT NOT NULL,UNIQUE(plugin_id,identity));CREATE TRIGGER call_retention AFTER INSERT ON connector_calls BEGIN DELETE FROM connector_calls WHERE id<=NEW.id-10000;END;BEGIN;",
  );
  const toolInsert = toolDb.prepare(
    "INSERT INTO connector_calls VALUES(?,?,?,?)",
  );
  const toolRow = (id, status = "success", at = old) => ({
    identity: `private-tool-${id}`,
    rpcId: "private-rpc",
    mcpConnection: "private-connection",
    tool: "search",
    status,
    durationMs: null,
    observedAt: at,
    agentSession: null,
    content: "not-persisted",
    modelTokens: null,
  });
  for (let id = 1; id <= 10007; id++) {
    const value = toolRow(
      id,
      id === 10002
        ? "in_progress"
        : id === 10003
          ? "timeout"
          : id === 10004
            ? "tool_error"
            : "success",
      id === 10001 ? recent : id === 10005 ? "unknown" : old,
    );
    if (id === 10006) value.future = null;
    toolInsert.run(
      id,
      "fixture.plugin",
      value.identity,
      id === 10007 ? "{" : JSON.stringify(value),
    );
  }
  toolDb.exec("COMMIT");
  const toolSource = () =>
    createHash("sha256")
      .update(
        JSON.stringify(
          toolDb
            .prepare(
              "SELECT id,plugin_id,identity,record FROM connector_calls ORDER BY id",
            )
            .all(),
        ),
      )
      .digest("hex");
  const toolBaseline = toolSource(),
    triggerBaseline = toolDb
      .prepare("SELECT sql FROM sqlite_master WHERE name='call_retention'")
      .get().sql;
  let lastInventory;
  for (const days of [7, 30, 90]) {
    const inventory = await api(inventoryRoute(days));
    lastInventory = inventory;
    assert.equal(inventory.gateway.status, "checked");
    assert.equal(inventory.tools.status, "checked");
    assert.equal(inventory.gateway.value.attempts, 2707);
    assert.equal(inventory.tools.value.records, 10000);
    assert.equal(inventory.tools.value.expired, 9994);
    assert.equal(inventory.tools.value.withinWindow, 1);
    assert.deepEqual(inventory.tools.value.protected, {
      active: 1,
      unknownOutcome: 1,
      unknownTime: 1,
      unsupportedSource: 2,
    });
    assert.equal(inventory.tools.value.unreadableRecords, 0);
    assert.equal(inventory.tools.value.classificationComplete, false);
    assert.equal(inventory.tools.value.capacityLimit, 10000);
    assert.equal(inventory.toolHistory, "saved_only_prior_history_unknown");
    assert.equal(inventory.remainingSources, "not_checked");
    assert.equal(inventory.cleanupEnabled, false);
    assert.equal(inventory.extraArchiveEnabled, false);
    assert.equal(inventory.confirmationAvailable, false);
    for (const source of [inventory.gateway, inventory.tools]) {
      assert(
        Date.parse(source.readStartedAt) >= Date.parse(inventory.capturedAt),
      );
      assert(
        Date.parse(source.readFinishedAt) >= Date.parse(source.readStartedAt),
      );
      assert(
        Date.parse(source.readFinishedAt) <= Date.parse(inventory.validUntil),
      );
    }
    assert.equal(source(), baseline);
    assert.equal(toolSource(), toolBaseline);
    assert.equal(
      toolDb
        .prepare("SELECT sql FROM sqlite_master WHERE name='call_retention'")
        .get().sql,
      triggerBaseline,
    );
    assert(!JSON.stringify(inventory).includes("private-"));
    passed(
      `${days} 天综合预览：10007 条经既有容量规则剩 10000 条，只统计保存集合，保护 / 未知与独立读取时段可核查`,
    );
  }
  assert.equal(
    (await api(inventoryRoute(1), { status: 400 })).code,
    "invalid_input",
  );
  assert.equal(
    (
      await api(inventoryRoute(30, snapshot.settingsRevision, randomUUID()), {
        status: 409,
      })
    ).code,
    "retention_store_changed",
  );
  assert.equal(
    (
      await api(inventoryRoute(30, snapshot.settingsRevision + 1), {
        status: 409,
      })
    ).code,
    "retention_settings_changed",
  );
  await api(inventoryRoute() + "&future=1", { status: 400 });
  for (const headers of [
    { authorization: "Bearer wrong" },
    { origin: "https://untrusted.invalid" },
    { host: "untrusted.invalid" },
  ])
    await api(inventoryRoute(), { status: 401, headers });
  passed(
    "综合入口同样验证参数、未来字段、库 / 设置版本和 token / Host / Origin",
  );
  toolDb.exec("UPDATE extension_version SET version=2");
  const futureTool = await api(inventoryRoute());
  assert.equal(futureTool.gateway.status, "checked");
  assert.deepEqual(futureTool.tools, {
    status: "unverified",
    reason: "unsupported_schema",
  });
  toolDb.exec("UPDATE extension_version SET version=1");
  db.exec("PRAGMA user_version=2");
  const futureGateway = await api(inventoryRoute());
  assert.deepEqual(futureGateway.gateway, {
    status: "unverified",
    reason: "unsupported_schema",
  });
  assert.equal(futureGateway.tools.status, "checked");
  db.exec("PRAGMA user_version=1");
  const epochBeforeFailure = db
    .prepare("SELECT value FROM observation_content_epoch WHERE id=1")
    .get().value;
  db.exec("UPDATE observation_content_epoch SET format=2");
  assert.deepEqual((await api(inventoryRoute())).gateway, {
    status: "unverified",
    reason: "unsupported_schema",
  });
  db.exec("UPDATE observation_content_epoch SET format=1");
  assert.equal(
    db.prepare("SELECT value FROM observation_content_epoch WHERE id=1").get()
      .value,
    epochBeforeFailure,
  );
  passed("未来内容代次格式不返回已核查数量；原代次与原始来源保持");
  db.exec("DELETE FROM observation_content_epoch");
  assert.deepEqual((await api(contentRoute())).gateway, {
    status: "unverified",
    reason: "unsupported_schema",
  });
  assert.equal(
    db.prepare("SELECT count(*) AS n FROM observation_content_epoch").get().n,
    0,
  );
  db.prepare("INSERT INTO observation_content_epoch VALUES(1,1,?)").run(
    epochBeforeFailure,
  );
  passed("缺失内容代次拒绝只读预览且不自动重建；仅测试夹具显式恢复原值");
  const excerpt = (key, at) => ({
    key,
    agent: "claude",
    sessionId: "private-session",
    startedAt: at,
    sourceStamp: null,
    input: { at, text: "private-excerpt-never-return" },
  });
  const gatewayCache = path.join(observation, "gateway-activity-inputs.json"),
    nativeCache = path.join(observation, "native-activity-inputs.json");
  const gatewayContent = JSON.stringify({
    version: 1,
    rows: [
      excerpt("old", old),
      excerpt("recent", recent),
      excerpt("future", new Date(Date.now() + 86_400_000).toISOString()),
      excerpt("bad", "invalid"),
    ],
  });
  const nativeContent = JSON.stringify({
    version: 1,
    rows: [excerpt("native", old)],
  });
  fs.writeFileSync(gatewayCache, gatewayContent, { mode: 0o600 });
  fs.writeFileSync(nativeCache, nativeContent, { mode: 0o600 });
  for (const days of [7, 30, 90]) {
    const result = await api(contentRoute(days));
    assert.equal(result.scope, "product_owned_saved_sources");
    assert.equal(result.gateway.status, "checked");
    assert.equal(result.tools.status, "checked");
    assert.deepEqual(result.activityCaches.gateway.value, {
      records: 4,
      olderThanCutoff: 1,
      withinWindow: 1,
      unknownTime: 2,
      fileBytes: Buffer.byteLength(gatewayContent),
    });
    assert.deepEqual(result.activityCaches.native.value, {
      records: 1,
      olderThanCutoff: 1,
      withinWindow: 0,
      unknownTime: 0,
      fileBytes: Buffer.byteLength(nativeContent),
    });
    assert.equal(result.cleanupEnabled, false);
    assert.equal(result.confirmationAvailable, false);
    assert.equal(result.remainingSources, "not_checked");
    for (const source of Object.values(result.activityCaches)) {
      assert.equal(source.status, "checked");
      assert(Date.parse(source.readStartedAt) >= Date.parse(result.capturedAt));
      assert(
        Date.parse(source.readFinishedAt) >= Date.parse(source.readStartedAt),
      );
      assert(
        Date.parse(source.readFinishedAt) <= Date.parse(result.validUntil),
      );
    }
    assert(!JSON.stringify(result).includes("private-"));
    assert.equal(fs.readFileSync(gatewayCache, "utf8"), gatewayContent);
    assert.equal(fs.readFileSync(nativeCache, "utf8"), nativeContent);
    passed(
      `${days} 天：两份产品摘要副本全集合年龄核对、未知时间保护、内容不外传且原字节保持`,
    );
  }
  fs.writeFileSync(gatewayCache, JSON.stringify({ version: 2, rows: [] }));
  const futureCache = await api(contentRoute());
  assert.deepEqual(futureCache.activityCaches.gateway, {
    status: "unverified",
    reason: "unsupported_schema",
  });
  assert.equal(futureCache.activityCaches.native.status, "checked");
  assert.equal(futureCache.tools.status, "checked");
  assert.equal((await api(inventoryRoute())).version, 1);
  fs.writeFileSync(gatewayCache, gatewayContent);
  fs.writeFileSync(nativeCache, JSON.stringify({ version: 1, rows: [] }));
  assert.equal(
    (await api(contentRoute())).activityCaches.native.value.records,
    0,
  );
  fs.writeFileSync(nativeCache, nativeContent);
  passed(
    "单份摘要未来格式保持未核查、其他来源仍可读；恢复重试与确认空区别于来源缺失，旧版不读取新范围",
  );
  assert.deepEqual(
    toolDb
      .prepare("SELECT name FROM sqlite_master WHERE name LIKE 'retention_%'")
      .all(),
    [],
  );
  passed("每个来源可独立失败或恢复，不伪造零值；未生成工具暂存或身份表");
  assert.equal((await api(route(1), { status: 400 })).code, "invalid_input");
  assert.equal(
    (
      await api(route(30, snapshot.settingsRevision, randomUUID()), {
        status: 409,
      })
    ).code,
    "retention_store_changed",
  );
  assert.equal(
    (await api(route(30, snapshot.settingsRevision + 1), { status: 409 })).code,
    "retention_settings_changed",
  );
  await api(route() + "&future=true", { status: 400 });
  await api(route(), {
    status: 401,
    headers: { authorization: "Bearer wrong" },
  });
  await api(route(), {
    status: 401,
    headers: { origin: "https://untrusted.invalid" },
  });
  await api(route(), { status: 401, headers: { host: "untrusted.invalid" } });
  passed("参数、未来查询字段、数据库归属、设置版本、认证、Host 和 Origin 保护");
  const saved = await api("/api/settings", {
    method: "PUT",
    value: {
      storeId,
      value: { captureContent: true, retentionDays: 7 },
      expectedRevision: snapshot.settingsRevision,
      operationId: randomUUID(),
    },
  });
  assert.equal(
    (await api(route(), { status: 409 })).code,
    "retention_settings_changed",
  );
  const preference = await api(route(7, saved.revision));
  assert.equal(preference.extraArchiveEnabled, false);
  assert.equal(preference.cleanupEnabled, false);
  assert.equal(source(), baseline);
  assert.equal(
    (await api(inventoryRoute(7, saved.revision))).cleanupEnabled,
    false,
  );
  passed("保存内容与天数偏好不转换为归档或清理授权；旧版本预览拒绝");
  const tables = db
    .prepare("SELECT name FROM sqlite_master WHERE name LIKE 'retention_%'")
    .all();
  assert.deepEqual(tables, []);
  await stop();
  await boot();
  const restarted = await api("/api/snapshot");
  assert.equal(restarted.settingsWriteProtocol.storeId, storeId);
  assert.equal(restarted.settingsRevision, saved.revision);
  const after = await api(route(7, saved.revision));
  assert.deepEqual(after.gateway, preference.gateway);
  assert.equal(source(), baseline);
  assert.equal(fs.readFileSync(native, "utf8"), nativeBaseline);
  const restartedInventory = await api(inventoryRoute(7, saved.revision));
  assert.deepEqual(restartedInventory.tools.value, lastInventory.tools.value);
  assert.equal(toolSource(), toolBaseline);
  const restartedContent = await api(contentRoute(7, saved.revision));
  assert.equal(restartedContent.activityCaches.gateway.value.records, 4);
  // NativeWatch drops excerpts for disabled/missing native sources on startup.
  // This fixture has no authorized transcript: do not mistake that existing
  // writer action for preview mutation or require stale text to resurrect.
  assert.equal(restartedContent.activityCaches.native.value.records, 0);
  assert.equal(fs.readFileSync(gatewayCache, "utf8"), gatewayContent);
  const nativeAfterRestart = fs.readFileSync(nativeCache, "utf8");
  assert.deepEqual(JSON.parse(nativeAfterRestart), { version: 1, rows: [] });
  assert.equal(
    (await api(contentRoute(7, saved.revision))).activityCaches.native.value
      .records,
    0,
  );
  assert.equal(fs.readFileSync(nativeCache, "utf8"), nativeAfterRestart);
  passed(
    "重启保留设置身份与版本；统计、网关摘要与原始哨兵不变；既有未授权原生摘要已移除，预览不会复活，无留存命名空间创建",
  );
  // Only this synthetic source is changed, and restored byte-for-byte afterwards.
  db.exec("PRAGMA user_version=2");
  assert.equal(
    (await api(route(7, saved.revision), { status: 503 })).code,
    "retention_source_unsupported",
  );
  db.exec("PRAGMA user_version=1");
  passed("未来来源结构明确拒绝，未以空历史继续");
  await stop();
  report.complete = true;
} finally {
  if (child && child.exitCode === null && child.signalCode === null) {
    child.kill("SIGTERM");
    await once(child, "exit", { signal: AbortSignal.timeout(15000) }).catch(
      () => {},
    );
  }
  db.close();
  toolDb?.close();
  if (process.env.RETENTION_PREVIEW_REPORT)
    fs.writeFileSync(
      path.resolve(process.env.RETENTION_PREVIEW_REPORT),
      JSON.stringify(report, null, 2) + "\n",
    );
  fs.rmSync(root, { recursive: true, force: true });
}
console.log(JSON.stringify(report, null, 2));
