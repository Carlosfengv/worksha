import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { once } from "node:events";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { DatabaseSync } from "node:sqlite";

const root = fs.realpathSync(
  fs.mkdtempSync(path.join(os.tmpdir(), "local-agent-iam-")),
);
const data = path.join(root, "data");
const home = path.join(root, "home");
const binary = path.resolve(
  process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
    "target/debug/local-agent-service",
);
const plugin = "builtin.iam";
const password = "iam-fixture-password";
fs.mkdirSync(home);
let child, exit, base, token;
const upstream = http.createServer((request, response) => {
  request.resume();
  response.setHeader("Content-Type", "application/json");
  response.end(
    JSON.stringify({
      id: randomUUID(),
      type: "message",
      role: "assistant",
      model: "fixture",
      content: [{ type: "text", text: "OK" }],
      stop_reason: "end_turn",
      usage: { input_tokens: 5, output_tokens: 2 },
    }),
  );
});
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
  let stderr = "";
  child.stderr.on("data", (bytes) => (stderr += bytes));
  exit = once(child, "exit");
  const [line] = await Promise.race([
    once(createInterface({ input: child.stdout }), "line", {
      signal: AbortSignal.timeout(20_000),
    }),
    exit.then(() => {
      throw new Error(stderr);
    }),
  ]);
  base = `http://127.0.0.1:${JSON.parse(line).port}`;
}
async function api(
  route,
  {
    body,
    session,
    expected = 200,
    method = body === undefined ? "GET" : "POST",
    headers,
  } = {},
) {
  const response = await fetch(base + route, {
    method,
    headers: {
      Authorization: "Bearer " + token,
      "Content-Type": "application/json",
      ...(session ? { "X-Local-Agent-Session": session } : {}),
      ...headers,
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(30_000),
  });
  const raw = await response.text();
  const result = response.headers
    .get("content-type")
    ?.includes("application/json")
    ? JSON.parse(raw)
    : { message: raw };
  assert.equal(
    response.status,
    expected,
    `${method} ${route} ${body?.kind ?? ""} ${result.code ?? ""} ${result.message ?? ""}`,
  );
  return result;
}
const action = (kind, input = {}, session, expected) =>
  api("/api/iam", { body: { kind, ...input }, session, expected });
const catalog = (session) => api("/api/builtin-plugins", { session });
async function change(transform, session, expected = 200) {
  const basis = await catalog(session);
  const body = {
    operationId: randomUUID(),
    storeId: basis.storeId,
    expectedGeneration: basis.host.generation,
    expectedRevision: basis.revision,
    expectedImpact: basis.impact,
    composition: transform(basis.composition),
  };
  return api("/api/builtin-plugins/changes", { body, session, expected });
}
async function stop() {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  child.kill("SIGTERM");
  await exit;
}
function sqlite(file, read) {
  const db = new DatabaseSync(path.join(data, file), { readOnly: true });
  try {
    return read(db);
  } finally {
    db.close();
  }
}
function checkVersion(version, file = "local-agent.sqlite3") {
  assert.equal(
    sqlite(file, (db) => db.prepare("PRAGMA user_version").get().user_version),
    version,
  );
}
const poolTables = ["provider_upstream_accounts", "service_account_rotation"];
function removeAccountPoolTables() {
  const db = new DatabaseSync(path.join(data, "local-agent.sqlite3"));
  try {
    for (const table of poolTables) {
      assert.equal(
        db.prepare(`SELECT count(*) AS count FROM ${table}`).get().count,
        0,
      );
      db.exec(`DROP TABLE ${table}`);
    }
  } finally {
    db.close();
  }
}
function checkAccountPoolTables() {
  sqlite("local-agent.sqlite3", (db) => {
    const table = db.prepare(
      "SELECT name FROM sqlite_schema WHERE type = 'table' AND name = ?",
    );
    for (const name of poolTables) assert.equal(table.get(name)?.name, name);
  });
}
const pass = (message) => console.log("通过：" + message);

try {
  upstream.listen(0, "127.0.0.1");
  await once(upstream, "listening");
  await start();
  const runtime = await fetch(base + "/api/runtime", {
    headers: { Authorization: "Bearer " + token },
    signal: AbortSignal.timeout(3000),
  });
  assert.equal(runtime.status, 200);
  assert.equal(runtime.headers.get("x-local-agent-iam-protocol"), "1");
  await runtime.json();
  assert.deepEqual(await api("/api/iam/status"), {
    enabled: false,
    account: null,
    available: true,
  });
  assert.equal(
    (await catalog()).host.plugins.find((item) => item.id === plugin)
      .runtimeActive,
    false,
  );
  await api("/api/snapshot");
  await api("/api/iam/status", {
    headers: { Origin: "https://example.invalid" },
    expected: 401,
  });
  checkVersion(16);
  assert.equal(
    (
      await change(
        (composition) => ({ ...composition, optedInPluginIds: [plugin] }),
        undefined,
        409,
      )
    ).code,
    "iam_setup_required",
  );
  await change((composition) => ({
    ...composition,
    removedPluginIds: [plugin],
    disabledPluginIds: [plugin],
  }));
  const cancelled = await action("prepare", { recovery: false });
  await action("prepare", { recovery: false });
  const ownerInput = {
    username: "owner",
    displayName: "Fixture Owner",
    password,
  };
  await action(
    "initialize",
    { ...ownerInput, operationId: cancelled.operationId },
    undefined,
    409,
  );
  assert.equal((await api("/api/iam/status")).enabled, false);
  pass("默认关闭、普通组合不能开启 IAM、停用态可卸载、取消设置后仍可使用");

  const provider = {
    id: "custom-" + randomUUID(),
    name: "Fixture Provider",
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
  await api("/api/providers", { body: provider, method: "PUT" });
  const verified = await api(`/api/providers/${provider.id}/verify`, {
    body: { expectedRevision: 1, model: provider.model },
  });
  assert.equal(verified.state, "verified");
  const model = {
    id: randomUUID(),
    name: "Fixture model",
    providerId: provider.id,
    model: provider.model,
    capabilities: {
      contextWindow: 48000,
      maxOutputTokens: 4096,
      tools: true,
      thinking: false,
      inputModalities: ["text"],
    },
  };
  const draft = (name) => ({
    id: randomUUID(),
    name,
    revision: 0,
    defaultModelId: model.id,
    models: [model],
  });
  const storeId = (await api("/api/model-services")).writeProtocol.storeId;
  const legacyOperation = randomUUID();
  const legacyReceipt = await api("/api/model-service-operations", {
    body: {
      storeId,
      operationId: legacyOperation,
      action: { kind: "save", draft: draft("Historical") },
    },
  });
  const legacy = legacyReceipt.service;
  const legacyReceiptPath =
    `/api/model-service-operations/${legacyOperation}?storeId=${storeId}`;
  const checkLegacy = async (session) => {
    checkAccountPoolTables();
    const view = await api("/api/model-services", { session });
    assert.equal(view.writeProtocol.storeId, storeId);
    assert.deepEqual(view.services.find((item) => item.id === legacy.id), legacy);
    assert.equal(legacy.routingPolicy.enabled, false);
    assert(!legacy.routingPolicy.accountPools?.length);
    assert.deepEqual(await api(legacyReceiptPath, { session }), legacyReceipt);
  };

  await stop();
  removeAccountPoolTables();
  await start();
  checkVersion(16);
  await checkLegacy();
  pass("IAM 关闭的旧结构数据库重启后保留单连接发布、数据库身份和回执");

  const prepared = await action("prepare", { recovery: false });
  const setup = { ...ownerInput, operationId: prepared.operationId };
  await action("initialize", { ...setup, password: "short" }, undefined, 400);
  assert.equal((await action("initialize", setup)).enabled, true);
  assert.equal(
    (await action("initialize", setup, undefined, 409)).code,
    "iam_locked",
  );
  checkVersion(17);
  checkVersion(16, "local-agent.pre-iam-v16.sqlite3");
  await api("/api/snapshot", { expected: 401 });
  await api("/api/iam/admission", { expected: 401 });
  const owner = await action("login", {
    username: ownerInput.username,
    password,
  });
  let admin = owner.session;
  assert.equal(owner.account.role, "owner");
  assert.equal(
    (await catalog(admin)).host.plugins.find((item) => item.id === plugin)
      .runtimeActive,
    true,
  );
  assert.equal(
    (await api("/api/iam/admission", { session: admin })).allowed,
    true,
  );
  assert.equal(
    (await action("resource", { serviceId: legacy.id }, admin)).creator,
    "未记录",
  );
  assert.equal(
    (
      await change(
        (composition) => ({ ...composition, disabledPluginIds: [plugin] }),
        admin,
        409,
      )
    ).code,
    "iam_locked",
  );
  assert.equal(
    (
      await change(
        (composition) => ({ ...composition, removedPluginIds: [plugin] }),
        admin,
        409,
      )
    ).code,
    "iam_locked",
  );
  assert.equal(
    (
      await change(
        (composition) => ({ ...composition, optedInPluginIds: [] }),
        admin,
        409,
      )
    ).code,
    "iam_locked",
  );
  assert.equal(
    (
      await change(
        (composition) => ({ ...composition, profile: "recovery" }),
        admin,
        409,
      )
    ).code,
    "iam_locked",
  );
  pass(
    "通过正式插件事务重新安装并开启、升级屏障、强制登录和开启后禁止关闭或卸载",
  );

  await stop();
  removeAccountPoolTables();
  await start();
  checkVersion(17);
  await api("/api/snapshot", { session: admin, expected: 401 });
  admin = (await action("login", { username: "owner", password })).session;
  await checkLegacy(admin);
  pass("IAM 开启的旧结构数据库重启后保持登录要求、单连接发布、数据库身份和回执");

  const a = await action(
    "create",
    { username: "alpha", displayName: "Member A", role: "member", password },
    admin,
  );
  assert.equal(
    (await action("login", { username: a.username, password }, admin, 409))
      .code,
    "already_signed_in",
  );
  await api("/api/snapshot", { session: admin });
  const b = await action(
    "create",
    { username: "bravo", displayName: "Member B", role: "member", password },
    admin,
  );
  const aSession = (await action("login", { username: a.username, password }))
    .session;
  const bSession = (await action("login", { username: b.username, password }))
    .session;
  assert.deepEqual(await action("resources", {}, aSession), []);
  assert.deepEqual(await action("resources", {}, bSession), []);
  await api("/api/iam/admission", { session: aSession, expected: 403 });
  await api("/api/model-services", { session: aSession, expected: 403 });
  await api("/api/builtin-plugins", { session: aSession, expected: 403 });
  const serviceCatalog = await api("/api/model-services", { session: admin });
  const poolPath = `/api/provider-upstream-accounts/${provider.id}`;
  const listAccounts = { kind: "list" };
  const { storeId: accountStoreId } = await api(poolPath, {
    body: listAccounts,
    session: admin,
  });
  const secret = "iam-pool-fixture-secret";
  const label = "IAM fixture account";
  const { credentialRef } = await api("/api/provider-credentials", {
    body: { label, secret },
    session: admin,
  });
  const accountId = randomUUID();
  const saveAccount = {
    kind: "save",
    storeId: accountStoreId,
    account: {
      id: accountId,
      expectedRevision: 0,
      providerRevision: 1,
      label,
      credentialRef,
    },
  };
  await api(poolPath, { body: saveAccount, session: aSession, expected: 403 });
  const accounts = await api(poolPath, { body: saveAccount, session: admin });
  const savedAccount = accounts.accounts.find(
    (account) => account.id === accountId,
  );
  assert.equal(savedAccount.label, label);
  assert(!JSON.stringify(accounts).includes(credentialRef));
  assert(!JSON.stringify(accounts).includes(secret));
  const account = { id: accountId, revision: savedAccount.revision };
  const verifyAccount = {
    kind: "verify",
    storeId: accountStoreId,
    account,
    expectedRevision: 1,
    model: provider.model,
  };
  await api(poolPath, { body: verifyAccount, session: admin });
  const saveDraft = {
    ...draft("Document assistant"),
    routingPolicy: {
      enabled: true,
      accountPools: [{ modelId: model.id, accounts: [account] }],
    },
  };
  const publish = {
    operationId: randomUUID(),
    storeId: serviceCatalog.writeProtocol.storeId,
    action: { kind: "save", draft: saveDraft },
  };
  await api("/api/model-service-operations", {
    session: admin,
    body: publish,
  });
  assert.equal(
    (await action("resource", { serviceId: saveDraft.id }, admin)).creator,
    owner.account.displayName,
  );
  await action(
    "grant",
    { accountId: a.id, serviceId: saveDraft.id, allow: true },
    admin,
  );
  await action(
    "grant",
    { accountId: b.id, serviceId: legacy.id, allow: true },
    admin,
  );
  assert.deepEqual(
    (await action("resources", {}, aSession)).map((item) => item.id),
    [saveDraft.id],
  );
  assert.deepEqual(
    (await action("resources", {}, bSession)).map((item) => item.id),
    [legacy.id],
  );
  await action("resource", { serviceId: saveDraft.id }, bSession, 404);
  const detail = await action(
    "resource",
    { serviceId: saveDraft.id },
    aSession,
  );
  assert.deepEqual(detail, {
    id: saveDraft.id,
    name: saveDraft.name,
    creator: owner.account.displayName,
    models: [model.name],
    tenantId: "local",
  });
  await api(poolPath, { body: listAccounts, session: aSession, expected: 403 });
  await api(poolPath, { body: verifyAccount, session: aSession, expected: 403 });
  await api("/api/model-service-operations", {
    body: publish,
    session: aSession,
    expected: 403,
  });
  await api("/api/model-services/preview", {
    body: {
      storeId: publish.storeId,
      draft: saveDraft,
      entry: { kind: "default" },
    },
    session: aSession,
    expected: 403,
  });
  for (const route of [
    "/api/provider-status",
    "/api/requests",
    `/api/request-detail?id=${randomUUID()}`,
    "/api/request-export?snapshot=fixture",
  ]) {
    await api(route, { session: aSession, expected: 403 });
  }
  for (const route of ["/api/request-exports", "/api/domain/request-exports"]) {
    await api(route, { body: {}, session: aSession, expected: 403 });
  }
  pass(
    "只有 Owner 可管理和发布账号池；已授权 Member 仅可查看服务摘要，无上游账号、尝试明细或导出权限",
  );
  await api("/api/model-services", {
    body: { ...saveDraft, revision: 1, name: "Updated assistant" },
    method: "PUT",
    session: admin,
  });
  assert.equal(
    (await action("resource", { serviceId: saveDraft.id }, aSession)).creator,
    owner.account.displayName,
  );
  await action(
    "grant",
    { accountId: b.id, serviceId: saveDraft.id, allow: true },
    admin,
  );
  await action(
    "grant",
    { accountId: a.id, serviceId: saveDraft.id, allow: false },
    admin,
  );
  await action("resource", { serviceId: saveDraft.id }, aSession, 404);
  await action("resource", { serviceId: saveDraft.id }, bSession);
  const edited = await action(
    "edit",
    {
      accountId: b.id,
      input: {
        revision: b.revision,
        displayName: "Member Renamed",
        role: "member",
        enabled: false,
      },
    },
    admin,
  );
  await action("resources", {}, bSession, 401);
  await action("delete", { accountId: b.id, revision: edited.revision }, admin);
  assert(
    (await action("accounts", {}, admin)).every(
      (account) => account.id !== b.id,
    ),
  );
  assert.equal(
    (
      await action(
        "delete",
        { accountId: owner.account.id, revision: owner.account.revision },
        admin,
        409,
      )
    ).code,
    "last_owner",
  );
  pass("账号增删改查、成员隔离、资源共享与撤权、创建者跨发布版本保留");

  const publisher = path.join(root, "publisher.key");
  const archive = path.join(root, "provider-presets.laplug");
  execFileSync(binary, ["--plugin-sdk", "keygen", publisher]);
  execFileSync(binary, [
    "--plugin-sdk",
    "pack",
    "sdk/plugins/examples/provider-presets/package",
    archive,
    publisher,
  ]);
  const upload = { path: archive };
  await api("/api/plugins/upload/inspect", {
    body: upload,
    session: aSession,
    expected: 403,
  });
  await api("/api/plugins/upload/inspect", { body: upload, expected: 401 });
  const inspected = await api("/api/plugins/upload/inspect", {
    body: upload,
    session: admin,
  });
  const checked = inspected.report ?? inspected.review ?? inspected;
  const installed = await api("/api/plugins/install", {
    body: {
      packageRef: checked.packageRef,
      digest: checked.digest,
      approvePublisher: true,
    },
    session: admin,
  });
  const uploadId = {
    pluginId: installed.manifest.id,
    digest: installed.digest,
  };
  await api("/api/plugins/state", {
    body: { ...uploadId, action: "enable" },
    session: admin,
  });
  pass("开启 IAM 后，上传检查和安装启用仍走原插件流程，仅 Owner 可操作");

  await stop();
  await start();
  checkVersion(17);
  await api("/api/snapshot", { session: admin, expected: 401 });
  const again = await action("login", { username: "owner", password });
  assert.equal(
    (await catalog(again.session)).host.plugins.find(
      (item) => item.id === plugin,
    ).runtimeActive,
    true,
  );
  const recovery = await action("prepare", { recovery: true });
  const reset = "iam-fixture-rotated-password";
  await action("recover", {
    operationId: recovery.operationId,
    recoveryCode: prepared.recoveryCode,
    password: reset,
  });
  await api("/api/snapshot", { session: again.session, expected: 401 });
  await action("login", { username: "owner", password }, undefined, 401);
  const recovered = await action("login", {
    username: "owner",
    password: reset,
  });
  await api("/api/snapshot", { session: recovered.session });
  await action("logout", {}, recovered.session);
  await api("/api/snapshot", { session: recovered.session, expected: 401 });
  pass("重启保留启用状态、离线恢复轮换凭据并撤销会话、退出登录");
} finally {
  await stop();
  upstream.close();
  fs.rmSync(root, { recursive: true, force: true });
}
