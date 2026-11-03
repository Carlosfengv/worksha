import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import http from "node:http";
import { spawn, execFileSync } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { createHash, randomUUID } from "node:crypto";
import { DatabaseSync } from "node:sqlite";
import { readCoreSchema } from "./lib/core-schema.mjs";
const binary = path.resolve(
  process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
    "target/debug/local-agent-service",
);
const output = process.env.PROVIDER_PRESETS_REPORT;
assert(output, "Explicit report required");
const sha = (p) =>
  createHash("sha256").update(fs.readFileSync(p)).digest("hex");
const root = fs.realpathSync(
    fs.mkdtempSync(path.join(os.tmpdir(), "local-agent-presets-")),
  ),
  data = path.join(root, "data"),
  home = path.join(root, "home");
fs.mkdirSync(home);
const report = {
  complete: false,
  binary,
  binarySha256: sha(binary),
  harnessSha256: sha(new URL(import.meta.url)),
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
};
const secret = "owned-preset-key-0398",
  network = [];
let child, base, token;
const upstream = http.createServer(async (req, res) => {
  network.push({ method: req.method, path: req.url });
  assert.equal(req.headers.authorization, "Bearer " + secret);
  if (req.method === "GET") {
    assert.equal(req.url, "/v1/models");
    res.end(JSON.stringify({ data: [{ id: "model-a" }, { id: "model-b" }] }));
    return;
  }
  assert.equal(req.method, "POST");
  assert.equal(req.url, "/v1/chat/completions");
  let raw = "";
  for await (const part of req) raw += part;
  const input = JSON.parse(raw);
  res.setHeader("content-type", "application/json");
  res.end(
    JSON.stringify({
      id: "owned-preset-response",
      object: "chat.completion",
      model: input.model,
      choices: [
        {
          index: 0,
          message: { role: "assistant", content: "OK" },
          finish_reason: "stop",
        },
      ],
      usage: { prompt_tokens: 4, completion_tokens: 1, total_tokens: 5 },
    }),
  );
});
await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));
const endpoint = `http://127.0.0.1:${upstream.address().port}/v1`;
const sdk = (...args) =>
  execFileSync(binary, ["--plugin-sdk", ...args], { stdio: "pipe" }).toString();
async function boot() {
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
  const [line] = await Promise.race([
    once(createInterface({ input: child.stdout }), "line", {
      signal: AbortSignal.timeout(15000),
    }),
    once(child, "exit").then(() => {
      throw Error("owned service exited");
    }),
  ]);
  base = "http://127.0.0.1:" + JSON.parse(line).port;
}
async function stop() {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const exited = once(child, "exit"),
    timer = setTimeout(() => child.kill("SIGKILL"), 5000);
  child.kill("SIGINT");
  await exited.finally(() => clearTimeout(timer));
}
async function api(route, body, expected = 200, headers = {}) {
  const r = await fetch(base + route, {
    method: body === undefined ? "GET" : "POST",
    headers: {
      authorization: "Bearer " + token,
      "content-type": "application/json",
      ...headers,
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(20000),
  });
  const raw = await r.text();
  assert(!raw.includes(secret));
  if (expected !== null) assert.equal(r.status, expected, route + " " + raw);
  return {
    status: r.status,
    value: (() => {
      try {
        return JSON.parse(raw);
      } catch {
        return { text: raw };
      }
    })(),
  };
}
const value = async (...args) => (await api(...args)).value;
const passed = (name) => {
  report.cases.push(name);
  console.log("通过：" + name);
};
const key = path.join(root, "publisher.key");
function pack(version = "1.0.0") {
  const folder = path.join(root, "package-" + version);
  fs.cpSync("sdk/plugins/examples/provider-presets/package", folder, {
    recursive: true,
  });
  const m = JSON.parse(fs.readFileSync(path.join(folder, "manifest.json")));
  m.version = version;
  m.providers[0].endpoint = endpoint;
  m.providers[1].endpoint = endpoint;
  m.providers[0].model = "model-a";
  fs.writeFileSync(path.join(folder, "manifest.json"), JSON.stringify(m));
  const file = folder + ".laplug";
  sdk("pack", folder, file, key);
  assert(JSON.parse(sdk("check", file)).valid);
  return file;
}
const install = async (file) => {
  const review = await value("/api/plugins/inspect", { path: file });
  return value("/api/plugins/install", {
    packageRef: review.packageRef,
    digest: review.digest,
    approvePublisher: true,
  });
};
const ref = (p) => ({ pluginId: p.manifest.id, digest: p.digest });
const change = (p, action) =>
  value("/api/plugins/state", { ...ref(p), action });
const protocol = () => value("/api/provider-operations/protocol");
let storeId;
const query = (input, expected = 200) =>
  value("/api/provider-presets", { storeId, ...input }, expected);
const metadataCounts = () => {
  const db = new DatabaseSync(path.join(data, "plugin-host.sqlite3"), {
    readOnly: true,
  });
  try {
    return db.prepare("SELECT count(*) AS n FROM provider_presets").get().n;
  } finally {
    db.close();
  }
};
try {
  await boot();
  sdk("keygen", key);
  storeId = (await protocol()).storeId;
  const schema = readCoreSchema(path.join(data, "local-agent.sqlite3"));
  assert.equal(schema.version, 15);
  const originalProviders = (await value("/api/snapshot")).providers;
  const builtin = await query({ kind: "list" });
  assert.equal(builtin.kind, "listed");
  assert.equal(builtin.items.length, 8);
  assert(builtin.items.every((p) => p.origin.kind === "builtin"));
  assert.deepEqual(builtin.issues, []);
  await api("/api/provider-presets", { kind: "list", storeId }, 401, {
    authorization: "Bearer invalid",
  });
  await api("/api/provider-presets", { kind: "list", storeId }, 401, {
    origin: "https://invalid.example",
  });
  await value(
    "/api/provider-presets",
    { kind: "list", storeId: randomUUID() },
    409,
  );
  await query({ kind: "list", force: true }, 422);
  assert.equal((await protocol()).sequence, 0);
  assert.equal(network.length, 0);
  passed(
    "原八个内置预设保持；鉴权、错误数据库与额外字段先拒绝，没有目录 / 业务副作用",
  );
  let plugin = await install(pack());
  assert.equal((await query({ kind: "list" })).items.length, 8);
  plugin = await change(plugin, "enable");
  const inventory = await query({ kind: "list" }),
    external = inventory.items.filter((p) => p.origin.kind === "external");
  assert.equal(external.length, 2);
  assert.deepEqual(inventory.issues, []);
  const advice = external.find((p) => p.origin.presetId === "responses"),
    origin = advice.origin;
  assert.equal(advice.protocol, "responses");
  assert.equal(advice.endpoints.responses, endpoint);
  const beforeSequence = (await protocol()).sequence;
  for (let i = 0; i < 3; i++)
    assert.deepEqual((await query({ kind: "resolve", origin })).advice, advice);
  assert.equal(metadataCounts(), 0);
  assert.equal((await protocol()).sequence, beforeSequence);
  assert.deepEqual((await value("/api/snapshot")).providers, originalProviders);
  assert.equal(network.length, 0);
  assert(!fs.existsSync(path.join(data, "plugins/provider-catalog")));
  assert(!fs.existsSync(path.join(data, "plugins/draft-catalog-owners")));
  passed(
    "实际签名声明式包进入统一契约；重复核查不分配 Provider ID、不创建引用、凭据或连接",
  );
  const credential = await value("/api/provider-credentials", {
    label: "Owned Preset Key",
    secret,
  });
  const draft = {
    id: "custom-" + randomUUID(),
    name: advice.name,
    vendor: "custom",
    endpoint,
    protocol: advice.protocol,
    model: "",
    models: [],
    credentialRef: credential.credentialRef,
    credentialLabel: credential.credentialLabel,
    configured: false,
    revision: 0,
    category: "local",
  };
  const catalog = await value("/api/provider-draft-catalog", {
    kind: "read",
    request: {
      origin: { kind: "new", storeId, providerId: draft.id },
      draft: {
        endpoint,
        protocol: draft.protocol,
        vendor: "custom",
        credentialRef: draft.credentialRef,
      },
      source: { kind: "builtin" },
    },
  });
  assert.deepEqual(
    catalog.models.map((p) => p.id),
    ["model-a", "model-b"],
  );
  assert.equal(network.length, 1);
  assert.equal(network[0].method, "GET");
  await change(plugin, "disable");
  assert.equal((await query({ kind: "list" })).items.length, 8);
  const denied = await api(
    "/api/provider-presets",
    { kind: "resolve", storeId, origin },
    null,
  );
  assert(denied.status >= 400);
  assert.equal(denied.value.code, "plugin_disabled");
  const save = {
    storeId,
    operationId: randomUUID(),
    expectedSequence: (await protocol()).sequence,
    kind: "save",
    provider: {
      ...draft,
      model: "model-b",
      models: catalog.models.map((p) => p.id),
    },
  };
  const saved = await value("/api/provider-operations", save);
  assert.equal(saved.status, "committed");
  assert.equal(saved.verification, null);
  assert.equal(saved.provider.vendor, "custom");
  assert.deepEqual(saved.provider.models, ["model-a", "model-b"]);
  assert.equal(saved.provider.model, "model-b");
  assert.equal(network.length, 1);
  assert.equal(metadataCounts(), 0);
  assert.deepEqual(
    await value(
      `/api/provider-operations/${save.operationId}?storeId=${storeId}`,
    ),
    saved,
  );
  passed(
    "预设复制后经原凭据 / Draft 读取 / 模型选择 / 保存链路；停用模板不改写用户数据或伪造目录授权",
  );
  plugin = await change(plugin, "enable");
  const legacyInput = {
    ...ref(plugin),
    action: { kind: "create-provider", preset: "responses" },
  };
  const legacy = await value("/api/plugins/action", legacyInput);
  assert.deepEqual(await value("/api/plugins/action", legacyInput), legacy);
  assert.equal(legacy.verified, false);
  assert.equal(metadataCounts(), 1);
  assert.equal(network.length, 1);
  const legacyProvider = (await value("/api/snapshot")).providers.find(
    (p) => p.id === legacy.providerId,
  );
  assert(legacyProvider);
  assert.equal(legacyProvider.endpoint, endpoint);
  assert.equal(legacyProvider.credentialRef, null);
  passed(
    "原 create-provider 入口保持稳定映射、幂等回复与原持久行为，不读取目录或进行验证",
  );
  const connect = {
    ...save,
    operationId: randomUUID(),
    expectedSequence: (await protocol()).sequence,
    kind: "connect",
    provider: {
      ...draft,
      id: "custom-" + randomUUID(),
      model: "model-b",
      models: ["model-a", "model-b"],
    },
  };
  const connected = await value("/api/provider-operations", connect);
  assert.equal(connected.status, "committed");
  assert.equal(connected.verification.state, "verified");
  // A copied response-format template is not an executable adapter binding.
  // The existing automatic onboarding still detects its original first format.
  assert.equal(connected.provider.protocol, "chat-completions");
  assert.equal(network.filter((r) => r.method === "POST").length, 1);
  const traffic = network.length;
  assert.deepEqual(await value("/api/provider-operations", connect), connected);
  assert.equal(network.length, traffic);
  passed(
    "原自动连接与独立验证可消费同一初始值；原编号重放不再次获取或发送模型",
  );
  const next = await install(pack("1.0.1"));
  await change(next, "enable");
  const old = await api(
    "/api/provider-presets",
    { kind: "resolve", storeId, origin },
    null,
  );
  assert(old.status >= 400);
  const current = (await query({ kind: "list" })).items.filter(
    (p) => p.origin.kind === "external",
  );
  assert(current.every((p) => p.origin.digest === next.digest));
  assert.equal(network.length, traffic);
  passed(
    "升级发布的新预设固定新 digest；旧选择拒绝，现有 Provider 和已保存回执不变",
  );
  const oldStore = storeId;
  await stop();
  await boot();
  storeId = (await protocol()).storeId;
  assert.equal(storeId, oldStore);
  const afterRestart = await query({ kind: "list" });
  assert.equal(
    afterRestart.items.filter((p) => p.origin.kind === "external").length,
    2,
  );
  assert.deepEqual(
    (await query({ kind: "resolve", origin: current[0].origin })).advice,
    current[0],
  );
  assert.equal(network.length, traffic);
  const statuses = (await value("/api/plugins")).items;
  assert(statuses.every((p) => !p.connectionAuthorized && !p.sourceAuthorized));
  assert.deepEqual(
    await value(
      `/api/provider-operations/${save.operationId}?storeId=${storeId}`,
    ),
    saved,
  );
  passed("真实重启保留静态预设和原 SQL 回执；不恢复网络授权或重做模型调用");
  await change(next, "remove");
  assert.equal((await query({ kind: "list" })).items.length, 8);
  assert.deepEqual(
    await value(
      `/api/provider-operations/${save.operationId}?storeId=${storeId}`,
    ),
    saved,
  );
  assert.equal(network.length, traffic);
  assert.deepEqual(
    readCoreSchema(path.join(data, "local-agent.sqlite3")),
    schema,
  );
  passed("卸载后原配置和操作结果独立可核查，原主库 schema 与数据格式保持");
  report.complete = true;
  report.catalogGets = network.filter((r) => r.method === "GET").length;
  report.syntheticModelPosts = network.filter(
    (r) => r.method === "POST",
  ).length;
  report.mainSchema = 15;
} finally {
  await stop();
  upstream.closeAllConnections();
  await new Promise((resolve) => upstream.close(resolve));
  report.ownedServiceStopped =
    !child || child.exitCode !== null || child.signalCode !== null;
  fs.rmSync(root, { recursive: true, force: true });
  report.temporaryDataRemoved = !fs.existsSync(root);
  fs.writeFileSync(output, JSON.stringify(report, null, 2) + "\n");
}
