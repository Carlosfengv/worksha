import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { randomUUID, createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { once } from "node:events";
import { createInterface } from "node:readline";
import http from "node:http";
import { DatabaseSync } from "node:sqlite";

// Owned service, synthetic source home, no model or credential calls.
const binary = path.resolve(
  process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
    "target/debug/local-agent-service",
);
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "builtin-composition-")),
);
const home = path.join(root, "home"),
  data = path.join(root, "data");
fs.mkdirSync(path.join(home, ".claude", "projects", "fixture"), {
  recursive: true,
});
const source = path.join(
  home,
  ".claude",
  "projects",
  "fixture",
  "fixture.jsonl",
);
const original =
  JSON.stringify({
    type: "user",
    sessionId: "fixture",
    uuid: "one",
    cwd: home,
    timestamp: new Date().toISOString(),
    message: { role: "user", content: "offline composition fixture" },
  }) + "\n";
fs.writeFileSync(source, original);
const report = {
  complete: false,
  binarySha256: createHash("sha256")
    .update(fs.readFileSync(binary))
    .digest("hex"),
  realModelCalls: 0,
  realCredentialCalls: 0,
  cases: [],
};
let child, base, token;
const passed = (name) => {
  report.cases.push(name);
  console.log("通过：" + name);
};
async function start(persistent = false, directory = data) {
  token = randomUUID();
  child = spawn(
    binary,
    persistent
      ? ["gateway", "serve", "--data-dir", directory]
      : ["--data-dir", directory],
    {
      env: {
        ...process.env,
        LOCAL_AGENT_SOURCE_HOME: home,
        LOCAL_AGENT_SERVICE_TOKEN: token,
        LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
        LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  child.stderr.resume();
  const lines = createInterface({ input: child.stdout });
  const [line] = await Promise.race([
    once(lines, "line", { signal: AbortSignal.timeout(15000) }),
    once(child, "exit").then(() => {
      throw Error("service exited before readiness");
    }),
  ]);
  base = "http://127.0.0.1:" + JSON.parse(line).port;
  if (persistent)
    token = fs.readFileSync(
      path.join(directory, "runtime", "admin-token"),
      "utf8",
    );
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
  extra = {},
) {
  const response = await fetch(base + route, {
    method,
    headers: {
      authorization: "Bearer " + token,
      "content-type": "application/json",
      ...extra,
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    signal: AbortSignal.timeout(15000),
  });
  const value = await response.json();
  assert.equal(response.status, expected, route + " " + JSON.stringify(value));
  return value;
}
const catalog = () => api("/api/builtin-plugins");
async function request(composition) {
  const current = await catalog();
  return {
    operationId: randomUUID(),
    storeId: current.storeId,
    expectedRevision: current.revision,
    expectedGeneration: current.host.generation,
    composition,
  };
}
const change = (input) => api("/api/builtin-plugins/changes", input);
const op = (input) =>
  api("/api/builtin-plugins/operations", {
    storeId: input.storeId,
    operationId: input.operationId,
  });
const snapshot = () => api("/api/snapshot");
const active = (value) =>
  value.host.plugins.filter((plugin) => plugin.runtimeActive);
try {
  await start();
  await api("/api/builtin-plugins", undefined, 401, "GET", {
    authorization: "invalid",
  });
  await api("/api/builtin-plugins", undefined, 401, "GET", {
    origin: "http://127.0.0.1",
  });
  assert.equal(
    await new Promise((resolve, reject) => {
      const call = http.get(
        base + "/api/builtin-plugins",
        { headers: { host: "localhost", authorization: "Bearer " + token } },
        (response) => {
          response.resume();
          response.on("end", () => resolve(response.statusCode));
        },
      );
      call.on("error", reject);
      call.setTimeout(5000, () =>
        call.destroy(Error("raw Host probe timed out")),
      );
    }),
    401,
  );
  passed("新目录与操作沿用令牌、Host、Origin 保护");
  let current = await catalog();
  assert.equal(current.schemaVersion, 1);
  assert.equal(active(current).length, 18);
  const projects = current.host.plugins.find((plugin) => plugin.id === "builtin.projects");
  assert(projects.optIn && !projects.runtimeActive);
  assert(
    current.host.plugins.every(
      (plugin) =>
        plugin.executionKind === "builtin" &&
        plugin.displayName &&
        plugin.capabilities.length,
    ),
  );
  assert(
    (await snapshot()).sessionSources.every(
      (source) => !source.enabled && source.runtimeActive,
    ),
  );
  passed("生产共享目录登记默认关闭的项目插件，注册不代表启用或来源授权");
  const before = await catalog();
  for (const input of [
    await request({
      profile: "desktop",
      disabledPluginIds: ["builtin.providers"],
    }),
    await request({ profile: "desktop", disabledPluginIds: ["unknown"] }),
  ]) {
    const rejected = await api("/api/builtin-plugins/changes", input, 409);
    assert.equal(rejected.outcomeUnknown, false);
    assert.equal((await op(input)).receipt, null);
  }
  assert.deepEqual(await catalog(), before);
  passed("缺失必需依赖和未知插件拒绝提交，目录与回执保持原状");
  await api(
    "/api/session-sources",
    { agentId: "claude", enabled: true },
    200,
    "PUT",
  );
  assert(
    (await snapshot()).sessionSources.find(
      (source) => source.agentId === "claude",
    ).enabled,
  );
  const sourcePlugin = current.host.plugins.find((plugin) =>
    plugin.capabilities.some(
      (cap) =>
        cap.address.name === "session.source" &&
        cap.address.instance === "claude",
    ),
  ).id;
  const disable = await request({
    profile: "desktop",
    disabledPluginIds: [sourcePlugin],
  });
  const first = await change(disable);
  let state = (await snapshot()).sessionSources.find(
    (source) => source.agentId === "claude",
  );
  assert.equal(state.enabled, false);
  assert.equal(state.runtimeActive, false);
  const denied = await api(
    "/api/session-sources",
    { agentId: "claude", enabled: true },
    400,
    "PUT",
  );
  assert.equal(denied.code, "plugin_disabled");
  await api(
    "/api/session-sources",
    { agentId: "claude", enabled: false },
    200,
    "PUT",
  );
  assert.deepEqual((await op(disable)).receipt, first.receipt);
  assert.deepEqual((await change(disable)).receipt, first.receipt);
  assert.equal((await catalog()).revision, first.receipt.revision);
  passed("停用同时撤销读取授权，拒绝重新授权；重复保存仅返回原回执");
  await stop();
  await start();
  state = (await snapshot()).sessionSources.find(
    (source) => source.agentId === "claude",
  );
  assert.equal(state.enabled, false);
  assert.equal(state.runtimeActive, false);
  assert.deepEqual((await op(disable)).receipt, first.receipt);
  assert.deepEqual((await change(disable)).receipt, first.receipt);
  passed("独立进程重启后保存配置、来源撤销与原回执一致");
  const restore = await request({ profile: "desktop", disabledPluginIds: [] });
  const restored = await change(restore);
  assert.equal(active(restored.catalog).length, 18);
  assert((await snapshot()).sessionSources.every((source) => !source.enabled));
  assert.equal(
    (await change(disable)).catalog.revision,
    restored.catalog.revision,
  );
  assert((await snapshot()).sessionSources.every((source) => !source.enabled));
  passed("重新启用获得新激活代；旧操作重放不会撤销新组合或恢复授权");
  const stale = { ...restore, operationId: randomUUID() };
  assert.equal(
    (await api("/api/builtin-plugins/changes", stale, 409)).code,
    "stale_snapshot",
  );
  const foreign = {
    ...(await request({ profile: "desktop", disabledPluginIds: [] })),
    storeId: randomUUID(),
  };
  assert.equal(
    (await api("/api/builtin-plugins/changes", foreign, 409)).code,
    "operation_store_changed",
  );
  const conflict = {
    ...disable,
    composition: { profile: "recovery", disabledPluginIds: [] },
  };
  assert.equal(
    (await api("/api/builtin-plugins/changes", conflict, 409)).code,
    "operation_conflict",
  );
  passed("旧版本、其他数据库和复用编号换参数均无写入");
  const key = path.join(root, "provider-preset.key");
  const archive = path.join(root, "provider-preset.laplug");
  execFileSync(binary, ["--plugin-sdk", "keygen", key]);
  execFileSync(binary, [
    "--plugin-sdk", "pack", "sdk/plugins/examples/provider-presets/package",
    archive, key,
  ]);
  const presetReview = await api("/api/plugins/inspect", { path: archive });
  const preset = await api("/api/plugins/install", {
    packageRef: presetReview.packageRef,
    digest: presetReview.digest,
    approvePublisher: true,
  });
  const plugin = { pluginId: preset.manifest.id, digest: preset.digest };
  await api("/api/plugins/state", { ...plugin, action: "enable" });
  const createPreset = { ...plugin, action: { kind: "create-provider", preset: "responses" } };
  const writeProtocol = await api("/api/provider-operations/protocol");
  const provider = {
    id: `custom-${randomUUID()}`,
    name: "Disabled Provider fixture",
    vendor: "custom",
    endpoint: "https://provider.example.invalid/v1",
    protocol: "responses",
    model: "fixture",
    models: ["fixture"],
    configured: false,
    revision: 0,
    category: "cloud",
  };
  const operation = {
    storeId: writeProtocol.storeId,
    expectedSequence: writeProtocol.sequence,
    operationId: randomUUID(),
    kind: "save",
    provider,
  };
  const records = (file, sql) => {
    const db = new DatabaseSync(path.join(data, file), { readOnly: true });
    try {
      return db.prepare(sql).all();
    } finally {
      db.close();
    }
  };
  const providerRows = () => records("local-agent.sqlite3", "SELECT * FROM providers ORDER BY id");
  const beforeProviders = providerRows();
  await change(await request({
    profile: "desktop",
    disabledPluginIds: ["builtin.agents", "builtin.model-services", "builtin.providers"],
  }));
  for (const [route, body, method] of [
    ["/api/providers", provider, "PUT"],
    ["/api/provider-operations", operation, "POST"],
    ["/api/provider-connect", provider, "POST"],
    ["/api/provider-imports", {}, "POST"],
    ["/api/provider-source-operations", {}, "POST"],
    ["/api/catalog-provider-operations", {}, "POST"],
  ]) {
    assert.equal((await api(route, body, 409, method)).code, "plugin_disabled");
  }
  assert.equal((await api("/api/plugins/action", createPreset, 400)).code, "plugin_disabled");
  assert.deepEqual(providerRows(), beforeProviders);
  assert.deepEqual(records("plugin-host.sqlite3", "SELECT * FROM provider_presets"), []);
  await change(await request({ profile: "desktop", disabledPluginIds: [] }));
  const created = await api("/api/plugins/action", createPreset);
  assert(providerRows().some((row) => row.id === created.providerId));
  const providerReceipt = await api("/api/provider-operations", operation);
  assert.equal(providerReceipt.status, "committed");
  const sourceProtocol = await api("/api/provider-operations/protocol");
  const sourceOperation = {
    operation: {
      ...operation,
      expectedSequence: sourceProtocol.sequence,
      operationId: randomUUID(),
      provider: { ...provider, id: `custom-${randomUUID()}` },
    },
    source: { kind: "builtin" },
    expectedSourceRevision: null,
  };
  const sourceReceipt = await api("/api/provider-source-operations", {
    kind: "commit",
    request: sourceOperation,
  });
  assert.equal(sourceReceipt.status, "committed");
  passed("Providers 停用后所有创建入口拒绝写入，恢复后预设与普通创建可用");
  const background = await change(
    await request({ profile: "background", disabledPluginIds: [] }),
  );
  assert.equal(active(background.catalog).length, 16);
  assert.equal(
    (await snapshot()).builtinComposition.nativeActivityActive,
    false,
  );
  assert(
    background.catalog.host.plugins
      .filter((plugin) =>
        plugin.capabilities.some((cap) => cap.address.name === "collector"),
      )
      .every(
        (plugin) => !plugin.runtimeActive && plugin.resources.length === 0,
      ),
  );
  await api("/api/domain/directory");
  passed("后台组合保留只读目录，两个采集器停止并释放资源");
  const recovery = await change(
    await request({ profile: "recovery", disabledPluginIds: [] }),
  );
  assert.equal(active(recovery.catalog).length, 0);
  assert(
    recovery.catalog.host.plugins.every(
      (plugin) => plugin.resources.length === 0,
    ),
  );
  assert.equal((await snapshot()).gatewayStatus, "paused");
  assert.equal(
    (await api("/api/domain/directory", undefined, 409)).code,
    "plugin_disabled",
  );
  const receiptRoute = (request) =>
    `/api/provider-operations/${request.operationId}?storeId=${request.storeId}`;
  assert.deepEqual(await api(receiptRoute(operation)), providerReceipt);
  await api(receiptRoute(operation), undefined, 401, "GET", {
    authorization: "invalid",
  });
  const sourceQueryRoute = "/api/provider-source-operations/query";
  assert.deepEqual(await api(sourceQueryRoute, sourceOperation), sourceReceipt);
  await api(sourceQueryRoute, sourceOperation, 401, "POST", {
    authorization: "invalid",
  });
  assert.equal(
    (await api("/api/provider-source-operations", {
      kind: "commit",
      request: sourceOperation,
    }, 409)).code,
    "plugin_disabled",
  );
  const recoveryProtocol = await api("/api/provider-operations/protocol");
  const cancelRequest = {
    ...operation,
    expectedSequence: recoveryProtocol.sequence,
    operationId: randomUUID(),
    provider: { ...provider, id: `custom-${randomUUID()}` },
  };
  const cancelRoute = "/api/provider-operations/cancel";
  await api(cancelRoute, cancelRequest, 401, "POST", {
    origin: "https://fixture.example.invalid",
  });
  const cancellation = await api(cancelRoute, cancelRequest);
  assert.equal(cancellation.status, "cancelled");
  assert.deepEqual(await api(receiptRoute(cancelRequest)), cancellation);
  assert.equal((await api("/api/provider-operations", cancelRequest, 409)).code, "plugin_disabled");
  assert(!providerRows().some((row) => row.id === cancelRequest.provider.id));
  await api("/api/builtin-plugins", undefined, 401, "GET", {
    authorization: "invalid",
  });
  assert.equal(
    (
      await api(
        "/api/session-sources",
        { agentId: "codex", enabled: true },
        400,
        "PUT",
      )
    ).code,
    "plugin_disabled",
  );
  await api(
    "/api/session-sources",
    { agentId: "codex", enabled: false },
    200,
    "PUT",
  );
  passed("恢复组合拒绝新业务，保留受保护诊断、撤销授权与恢复入口");
  await stop();
  await start();
  assert.equal((await catalog()).effectiveProfile, "recovery");
  assert.equal(
    (await snapshot()).builtinComposition.nativeActivityActive,
    false,
  );
  await change(await request({ profile: "desktop", disabledPluginIds: [] }));
  assert.equal(
    (await snapshot()).builtinComposition.nativeActivityActive,
    true,
  );
  await api(
    "/api/overview?range=24&metric=calls&dimension=agent&source=native",
  );
  assert.equal(fs.readFileSync(source, "utf8"), original);
  passed("恢复模式重启后可恢复桌面采集，合成来源文件保持原样");
  assert.deepEqual(await api("/api/provider-operations", cancelRequest), cancellation);

  // The trigger runs in the real composition transaction. Its invalid source
  // row makes authorization reconciliation fail after the commit has succeeded.
  const fault = new DatabaseSync(path.join(data, "local-agent.sqlite3"));
  try {
    fault.exec(`CREATE TRIGGER fixture_reconcile_failure
      AFTER UPDATE ON builtin_plugin_composition BEGIN
        INSERT INTO native_sources(agent_id, enabled) VALUES(x'80', 1);
      END;`);
    const interrupted = await request({
      profile: "desktop",
      disabledPluginIds: ["builtin.overview"],
    });
    const rejected = await api("/api/builtin-plugins/changes", interrupted, 503);
    assert.equal(rejected.outcomeUnknown, true);
    const observed = await op(interrupted);
    assert.deepEqual(observed.receipt.composition, interrupted.composition);
    assert.equal(observed.catalog.effectiveProfile, "recovery");
    assert.equal(observed.catalog.problem, "source_reconcile_failed");
    assert.equal(active(observed.catalog).length, 0);
    assert.equal((await api("/api/domain/directory", undefined, 409)).code, "plugin_disabled");
    assert.deepEqual(await api(receiptRoute(operation)), providerReceipt);
    fault.exec("DROP TRIGGER fixture_reconcile_failure; DELETE FROM native_sources WHERE agent_id=x'80';");
    const replayed = await change(interrupted);
    assert.deepEqual(replayed, observed);
    assert.equal((await api("/api/domain/directory", undefined, 409)).code, "plugin_disabled");
    const repaired = await change(await request({ profile: "desktop", disabledPluginIds: [] }));
    assert.equal(repaired.catalog.problem, null);
    assert.equal(repaired.catalog.effectiveProfile, "desktop");
    await api("/api/domain/directory");
    passed("提交后来源同步失败会暂停业务并保留回执，显式恢复后才能继续");
  } finally {
    fault.close();
  }
  await stop();
  await start(true, path.join(root, "headless-data"));
  assert.equal((await catalog()).effectiveProfile, "background");
  assert.equal(active(await catalog()).length, 16);
  assert.equal(
    (await snapshot()).builtinComposition.nativeActivityActive,
    false,
  );
  passed("全新常驻服务默认后台组合，直接运行 Rust 二进制且无需界面进程");
  report.complete = true;
} finally {
  await stop();
  fs.rmSync(root, { recursive: true, force: true });
  if (process.env.BUILTIN_PLUGIN_TEST_REPORT)
    fs.writeFileSync(
      process.env.BUILTIN_PLUGIN_TEST_REPORT,
      JSON.stringify(report, null, 2) + "\n",
    );
  console.log(JSON.stringify(report, null, 2));
}
