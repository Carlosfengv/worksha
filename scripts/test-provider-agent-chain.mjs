// Full business flow on owned files, MemoryVault, actual signed packages and loopback HTTP.
// This does not launch an Agent or use SystemVault / a real model.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import http from "node:http";
import { spawn, execFileSync } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { createHash, randomUUID } from "node:crypto";
import { readCoreSchema } from "./lib/core-schema.mjs";
const binary = fs.realpathSync(
  process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
    "target/debug/local-agent-service",
);
const output = process.env.PROVIDER_AGENT_CHAIN_REPORT;
assert(output, "Explicit report required");
const sha = (p) =>
  createHash("sha256").update(fs.readFileSync(p)).digest("hex");
const root = fs.realpathSync(
    fs.mkdtempSync(path.join(os.tmpdir(), "local-agent-provider-agent-chain-")),
  ),
  home = path.join(root, "home"),
  data = path.join(root, "data"),
  nativePath = path.join(home, ".config/opencode/opencode.json"),
  claudePath = path.join(home, ".claude/settings.json"),
  finitePath = path.join(root, "selected.json");
const originalNative =
    '{"model":"old/model","provider":{"old":{"npm":"@ai-sdk/anthropic","options":{"baseURL":"http://127.0.0.1:9/old"}}},"permission":{"read":"ask"},"unknown":{"keep":true}}\n',
  originalClaude =
    '{"env":{"KEEP":"old"},"permissions":{"deny":["Read(.env)"]}}\n',
  originalFinite = '{"env":{"KEEP":"old"},"unknown":[1,2,3]}\n',
  secret = "owned-provider-chain-key-3648";
fs.mkdirSync(path.dirname(nativePath), { recursive: true });
fs.mkdirSync(path.dirname(claudePath), { recursive: true });
fs.writeFileSync(nativePath, originalNative, { mode: 0o640 });
fs.writeFileSync(claudePath, originalClaude, { mode: 0o600 });
fs.writeFileSync(finitePath, originalFinite, { mode: 0o640 });
const report = {
  complete: false,
  binary,
  binarySha256: sha(binary),
  harnessSha256: sha(import.meta.filename),
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
  actualAgentLaunches: 0,
};
const traffic = [],
  upstreamErrors = [],
  held = new Set();
let child,
  base,
  token,
  hold = false,
  arrived;
const reply = (model) => ({
  id: "chain-response",
  object: "response",
  status: "completed",
  model,
  output: [
    {
      type: "message",
      id: "chain-message",
      role: "assistant",
      content: [{ type: "output_text", text: "CHAIN_你好", annotations: [] }],
    },
  ],
  usage: { input_tokens: 7, output_tokens: 3, total_tokens: 10 },
});
const packet = (p) =>
  JSON.stringify({ payload: `data: ${JSON.stringify(p)}\n\n` }) + "\n";
const terminal = (model) =>
  packet({ type: "response.completed", response: reply(model) });
const upstream = http.createServer(async (req, res) => {
  try {
    assert.equal(req.headers.authorization, "Bearer " + secret);
    const u = new URL(req.url, "http://127.0.0.1");
    traffic.push({ method: req.method, path: u.pathname });
    if (req.method === "GET") {
      res.setHeader("content-type", "application/json");
      if (u.pathname === "/v1/catalog")
        res.end(
          JSON.stringify({
            entries: [
              { key: "model-a", tools: true },
              { key: "model-b", tools: true },
            ],
            more: false,
          }),
        );
      else {
        assert.equal(u.pathname, "/v1/models");
        res.end(
          JSON.stringify({ data: [{ id: "model-a" }, { id: "model-b" }] }),
        );
      }
      return;
    }
    assert.equal(req.method, "POST");
    let raw = "";
    for await (const bytes of req) raw += bytes;
    const wireBody = JSON.parse(raw);
    const body = u.pathname === "/v1/envelope" ? wireBody.payload : wireBody;
    assert(["model-a", "model-b"].includes(body.model));
    if (u.pathname === "/v1/responses") {
      if (body.stream) {
        res.writeHead(200, { "content-type": "text/event-stream" });
        res.end(
          [
            {
              type: "response.created",
              response: {
                id: "chain-response",
                model: body.model,
                status: "in_progress",
              },
            },
            {
              type: "response.output_text.delta",
              output_index: 0,
              delta: "CHAIN_你好",
            },
            { type: "response.completed", response: reply(body.model) },
          ]
            .map((event) => `data: ${JSON.stringify(event)}\n\n`)
            .join(""),
        );
        return;
      }
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify(reply(body.model)));
      return;
    }
    assert.equal(u.pathname, "/v1/envelope");
    assert.equal(body.stream, true);
    res.writeHead(200, { "content-type": "application/x-ndjson" });
    const first = packet({
      type: "response.output_text.delta",
      output_index: 0,
      delta: "CHAIN_你好",
    });
    // Arbitrary byte boundaries, including inside UTF-8, go through the real worker.
    const bytes = Buffer.from(first);
    for (let i = 0; i < bytes.length; i += 3)
      res.write(bytes.subarray(i, i + 3));
    if (hold) {
      held.add({ res, model: body.model });
      arrived?.();
      return;
    }
    res.end(terminal(body.model));
  } catch (e) {
    upstreamErrors.push(e.code ?? "unexpected_request");
    if (!res.headersSent) res.writeHead(500);
    res.end();
  }
});
await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));
const endpoint = `http://127.0.0.1:${upstream.address().port}/v1`;
const sdk = (...args) =>
  execFileSync(binary, ["--plugin-sdk", ...args], { stdio: "pipe" });
const key = path.join(root, "publisher.key");
const ref = (p) => ({ pluginId: p.manifest.id, digest: p.digest });
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
      throw Error("owned service exited before ready");
    }),
  ]);
  base = "http://127.0.0.1:" + JSON.parse(line).port;
}
async function stop() {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const done = once(child, "exit"),
    timer = setTimeout(() => child.kill("SIGKILL"), 5000);
  child.kill("SIGINT");
  await done.finally(() => clearTimeout(timer));
}
async function api(route, body, status = 200) {
  const r = await fetch(base + route, {
    method: body === undefined ? "GET" : "POST",
    headers: {
      authorization: "Bearer " + token,
      "content-type": "application/json",
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(20000),
  });
  const raw = await r.text();
  assert(!raw.includes(secret));
  assert.equal(r.status, status, route + " " + raw);
  return JSON.parse(raw);
}
const protocol = () => api("/api/provider-operations/protocol");
const action = (p, action, status = 200) =>
  api("/api/plugin-extensions/action", { ...ref(p), action }, status);
const change = (p, action) => api("/api/plugins/state", { ...ref(p), action });
async function install(example, edit) {
  const folder = path.join(root, example),
    archive = folder + ".laplug";
  fs.cpSync("sdk/plugins/examples/" + example + "/package", folder, {
    recursive: true,
  });
  if (edit) {
    const p = path.join(folder, "manifest.json"),
      m = JSON.parse(fs.readFileSync(p));
    edit(m);
    fs.writeFileSync(p, JSON.stringify(m));
  }
  sdk("pack", folder, archive, key);
  const review = await api("/api/plugins/inspect", { path: archive });
  const p = await api("/api/plugins/install", {
    packageRef: review.packageRef,
    digest: review.digest,
    approvePublisher: true,
  });
  report.packages ??= [];
  report.packages.push({
    ...ref(p),
    archiveSha256: sha(archive),
    wasmSha256: fs.existsSync(path.join(folder, "plugin.wasm"))
      ? sha(path.join(folder, "plugin.wasm"))
      : null,
  });
  return change(p, "enable");
}
const passed = (name) => {
  report.cases.push(name);
  console.log("通过：" + name);
};
const posts = () => traffic.filter((t) => t.method === "POST").length;
async function observedAfter(known) {
  for (let i = 0; i < 100; i++) {
    const rows = (await api("/api/requests")).items.filter(
      (r) => !known.has(r.id),
    );
    const row = rows.find((r) => !["running", "pending"].includes(r.status));
    if (row) return row;
    await new Promise((resolve) => setTimeout(resolve, 30));
  }
  throw Error("Owned gateway result did not settle");
}
try {
  await boot();
  sdk("keygen", key);
  const schema = readCoreSchema(path.join(data, "local-agent.sqlite3"));
  assert.equal(schema.version, 15);
  const presets = await install("provider-presets", (m) =>
      m.providers.forEach((p) => {
        p.endpoint = endpoint;
      }),
    ),
    catalog = await install("model-catalog"),
    wire = await install("envelope-protocol"),
    config = await install("json-config");
  assert.equal(traffic.length, 0);
  const start = await protocol(),
    storeId = start.storeId;
  const inventory = await api("/api/provider-presets", {
    kind: "list",
    storeId,
  });
  const origin = inventory.items.find(
    (p) => p.origin.kind === "external" && p.origin.presetId === "responses",
  ).origin;
  const advice = (
    await api("/api/provider-presets", { kind: "resolve", storeId, origin })
  ).advice;
  assert.equal(advice.endpoints.responses, endpoint);
  const credential = await api("/api/provider-credentials", {
    label: "Owned full chain",
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
    disabledModels: [],
    ...credential,
    configured: false,
    revision: 0,
    category: "local",
  };
  const { owner } = await api("/api/provider-draft-grant", {
    kind: "open",
    storeId,
    scope: {
      catalog: ref(catalog),
      draft: {
        endpoint,
        protocol: draft.protocol,
        vendor: draft.vendor,
        credentialRef: draft.credentialRef,
      },
      label: draft.name,
      allowPrivateNetwork: true,
    },
  });
  const { selection } = await api("/api/provider-draft-grant", {
    kind: "acquire",
    owner,
  });
  assert.equal(traffic.length, 0);
  const directory = await api("/api/provider-draft-catalog", {
    kind: "read",
    request: {
      origin: { kind: "new", storeId, providerId: draft.id },
      draft: {
        endpoint,
        protocol: draft.protocol,
        vendor: draft.vendor,
        credentialRef: draft.credentialRef,
      },
      source: { kind: "external", catalog: selection },
    },
  });
  assert.deepEqual(
    directory.models.map((m) => m.id),
    ["model-a", "model-b"],
  );
  assert.equal(posts(), 0);
  passed(
    "四个真实签名包分别授权；统一预设到新 Key、宿主 Draft 授权和实际 guest 目录，尚无模型发送或文件写入",
  );
  const sourceRequest = {
    operation: {
      storeId,
      operationId: randomUUID(),
      expectedSequence: (await protocol()).sequence,
      kind: "save",
      provider: {
        ...draft,
        model: "model-b",
        models: directory.models.map((m) => m.id),
      },
    },
    source: { kind: "external", catalog: selection },
    expectedSourceRevision: null,
  };
  await api("/api/provider-draft-grant", {
    kind: "attach",
    owner,
    request: sourceRequest,
  });
  const saved = await api("/api/provider-source-operations", {
    kind: "commit",
    request: sourceRequest,
  });
  assert.equal(saved.status, "committed");
  assert.equal(saved.verification, null);
  assert.equal(posts(), 0);
  const provider = saved.provider,
    providerBytes = JSON.stringify(provider);
  await api("/api/provider-draft-grant", { kind: "release", owner });
  assert.deepEqual(
    await api("/api/provider-source-operations", {
      kind: "query",
      request: sourceRequest,
    }),
    saved,
  );
  assert.deepEqual(
    await api("/api/provider-operations", sourceRequest.operation),
    saved,
  );
  for (const model of provider.models)
    assert.equal(
      (
        await api(`/api/providers/${provider.id}/verify`, {
          expectedRevision: provider.revision,
          model,
        })
      ).state,
      "verified",
    );
  assert.equal(posts(), 2);
  assert.equal((await protocol()).sequence, saved.sequence);
  await action(wire, {
    kind: "bind-protocol",
    providerId: provider.id,
    allowPrivateNetwork: true,
  });
  passed(
    "原来源保存和两个独立模型验证保持，原编号在新旧入口重放不重做；协议绑定单独明确授权",
  );
  const before = await api("/api/provider-status"),
    nativeDraft = {
      agentId: "opencode",
      providerId: provider.id,
      expectedProviderRevision: provider.revision,
      model: "model-b",
      expectedRevision: before.configs.find((c) => c.agentId === "opencode")
        .revision,
    };
  const transaction = await api("/api/provider-switches", nativeDraft);
  assert.equal(fs.readFileSync(nativePath, "utf8"), originalNative);
  assert.equal(
    fs.readFileSync(transaction.backupPath).subarray(0, 4).toString(),
    "LAC2",
  );
  assert.equal(fs.statSync(transaction.backupPath).mode & 0o777, 0o600);
  const applied = await api(
    `/api/provider-switches/${transaction.id}/apply`,
    {},
  );
  assert.equal(applied.state, "applied");
  const generated = fs.readFileSync(nativePath),
    current = JSON.parse(generated),
    group = current.model.split("/")[0],
    connection = current.provider[group].options;
  assert.deepEqual(
    Object.keys(current.provider[group].models),
    provider.models,
  );
  assert.equal(current.unknown.keep, true);
  assert.equal(current.permission.read, "ask");
  assert.equal(current.provider.old.options.baseURL, "http://127.0.0.1:9/old");
  assert(!generated.includes(Buffer.from(secret)));
  assert(connection.apiKey && connection.apiKey !== secret);
  assert.equal(connection.baseURL, transaction.routing.localEndpoint + "/v1");
  const status = await api("/api/provider-status"),
    native = status.configs.find((c) => c.agentId === "opencode");
  assert.equal(native.model, current.model);
  assert.equal(native.revision, sha(nativePath));
  const afterApply = sha(nativePath);
  await api(`/api/provider-switches/${transaction.id}/apply`, {});
  assert.equal(sha(nativePath), afterApply);
  passed(
    "统一 ConfigPlanner 进入真实 OpenCode 预览、加密备份、幂等应用和回读；完整模型分组和无关配置保留，上游 Key 未写入原生配置",
  );
  const call = () =>
    fetch(connection.baseURL + "/messages", {
      method: "POST",
      headers: {
        "x-api-key": connection.apiKey,
        "anthropic-version": "2023-06-01",
        "content-type": "application/json",
      },
      body: JSON.stringify({
        model: "model-b",
        stream: true,
        max_tokens: 50,
        messages: [{ role: "user", content: "owned synthetic chain" }],
      }),
      signal: AbortSignal.timeout(15000),
    });
  let known = new Set((await api("/api/requests")).items.map((r) => r.id)),
    count = posts();
  const response = await call();
  if (response.status !== 200) {
    const failure = await response.text();
    assert(!failure.includes(secret) && !failure.includes(connection.apiKey));
    report.gatewayFailure = { status: response.status, body: failure };
  }
  assert.equal(response.status, 200, JSON.stringify(report.gatewayFailure));
  const body = await response.text();
  assert(body.includes("CHAIN_你好") && body.includes("event: message_stop"));
  assert.equal(posts(), count + 1);
  let observed = await observedAfter(known);
  assert.equal(observed.status, "success");
  assert.equal(observed.providerId, provider.id);
  assert.equal(observed.agentId, "opencode");
  assert.equal(observed.usage.inputTokens, 7);
  assert.equal(observed.usage.outputTokens, 3);
  assert.equal(observed.sessionId, null);
  const dashboard = await api("/api/request-dashboard?agent=opencode");
  const record = dashboard.items.find((row) =>
    row.attempts.some((attempt) => attempt.id === observed.id),
  );
  assert(record);
  assert.equal(record.attempts.length, 1);
  assert.equal(record.attempts[0].providerId, provider.id);
  assert.equal(record.usage.inputTokens, 7);
  assert.equal(record.usage.outputTokens, 3);
  const detail = await api(
    "/api/request-detail?id=" + encodeURIComponent(record.id),
  );
  assert.equal(detail.request.id, record.id);
  report.successfulRequestId = record.id;
  passed(
    "受管客户端实际消费生成配置及本机入口凭据，经 v2 协议 Wasm 和跨协议流返回 UTF-8 文本；同一 Provider/Agent 的用量与请求详情可核查，单次发送",
  );
  await api("/api/plugin-extensions/resource", {
    ...ref(config),
    path: finitePath,
  });
  const plan = await action(config, {
    kind: "prepare-config",
    providerId: provider.id,
  });
  assert.equal(fs.readFileSync(finitePath, "utf8"), originalFinite);
  assert.equal(plan.credentialPlaceholder, "${HOST_CREDENTIAL}");
  await action(config, {
    kind: "apply-config",
    planId: plan.planId,
    revision: plan.revision,
  });
  const finite = JSON.parse(fs.readFileSync(finitePath));
  assert.equal(finite.env.API_KEY, secret);
  assert.equal(finite.env.API_MODEL, provider.model);
  assert.equal(finite.env.KEEP, "old");
  assert.equal(fs.statSync(finitePath).mode & 0o777, 0o600);
  passed(
    "同一已保存 Provider 经 v2 有限 ConfigPlanner 在明确授权文件应用；guest 仅给占位候选，原未知字段保留",
  );
  hold = true;
  known = new Set((await api("/api/requests")).items.map((r) => r.id));
  count = posts();
  const entered = new Promise((resolve) => {
    arrived = resolve;
  });
  const pending = call();
  await entered;
  const inFlight = await pending,
    text = inFlight.text().then(
      (value) => ({ value, failed: false }),
      () => ({ value: "", failed: true }),
    );
  await change(wire, "revoke-grants");
  for (const item of held) {
    item.res.end(terminal(item.model));
  }
  held.clear();
  hold = false;
  const ended = await text;
  assert(ended.failed || !ended.value.includes("event: message_stop"));
  assert.equal(posts(), count + 1);
  observed = await observedAfter(known);
  assert.notEqual(observed.status, "success");
  report.interruptedRequestId = observed.id;
  await change(wire, "disable");
  count = posts();
  const denied = await call();
  assert(denied.status >= 400);
  await denied.text();
  assert.equal(posts(), count);
  assert.equal(
    (await api("/api/plugins/state", { ...ref(wire), action: "remove" }, 400))
      .code,
    "protocol_binding_active",
  );
  await action(wire, { kind: "unbind-protocol" });
  await change(wire, "remove");
  passed(
    "流已发送后撤权拒绝成功终态；停用后的旧绑定明确失败、零回退或重发；遵守原保护，显式解除协议后才卸载",
  );
  for (const p of [presets, catalog, config]) await change(p, "remove");
  assert.deepEqual(
    await api("/api/provider-source-operations", {
      kind: "query",
      request: sourceRequest,
    }),
    saved,
  );
  assert.equal(
    JSON.stringify(
      (await api("/api/snapshot")).providers.find((p) => p.id === provider.id),
    ),
    providerBytes,
  );
  const trafficBeforeRecovery = traffic.length;
  await action(config, { kind: "restore-config", operationId: plan.planId });
  assert.equal(fs.readFileSync(finitePath, "utf8"), originalFinite);
  assert.equal(fs.statSync(finitePath).mode & 0o777, 0o640);
  fs.writeFileSync(nativePath, Buffer.concat([generated, Buffer.from("\n")]));
  await api(`/api/provider-switches/${transaction.id}/restore`, {}, 409);
  assert.equal(
    sha(nativePath),
    createHash("sha256")
      .update(Buffer.concat([generated, Buffer.from("\n")]))
      .digest("hex"),
  );
  fs.writeFileSync(nativePath, generated);
  const restore = await api(
    `/api/provider-switches/${transaction.id}/restore`,
    {},
  );
  assert.equal(sha(nativePath), afterApply);
  assert.equal(
    (await api(`/api/provider-switches/${restore.id}/apply`, {})).state,
    "applied",
  );
  assert.equal(fs.readFileSync(nativePath, "utf8"), originalNative);
  assert.equal(fs.statSync(nativePath).mode & 0o777, 0o640);
  await api(`/api/provider-switches/${restore.id}/apply`, {});
  assert.equal(fs.readFileSync(nativePath, "utf8"), originalNative);
  assert.equal(traffic.length, trafficBeforeRecovery);
  assert.equal(fs.readFileSync(claudePath, "utf8"), originalClaude);
  assert.deepEqual(
    readCoreSchema(path.join(data, "local-agent.sqlite3")),
    schema,
  );
  assert.deepEqual(upstreamErrors, []);
  assert.equal(posts(), 4);
  assert.equal(
    traffic.filter((t) => t.method === "GET" && t.path.endsWith("/models"))
      .length,
    0,
  );
  passed(
    "全部四个插件移除后，原 Provider 和 SQL 回执仍可核查，有限配置及原生配置均由宿主独立精确恢复；外部修改先拒绝，重复应用不再写入或发送",
  );
  const builtinAdvice = (
    await api("/api/provider-presets", {
      kind: "resolve",
      storeId,
      origin: { kind: "builtin", presetId: "openai" },
    })
  ).advice;
  const builtinDraft = {
    ...draft,
    id: "custom-" + randomUUID(),
    name: builtinAdvice.name,
    vendor: "openai",
    model: "",
    models: [],
  };
  const beforeBuiltinDirectory = traffic.length;
  const builtinDirectory = await api("/api/provider-draft-catalog", {
    kind: "read",
    request: {
      origin: { kind: "new", storeId, providerId: builtinDraft.id },
      draft: {
        endpoint,
        protocol: builtinAdvice.protocol,
        vendor: builtinDraft.vendor,
        credentialRef: credential.credentialRef,
      },
      source: { kind: "builtin" },
    },
  });
  assert.deepEqual(
    builtinDirectory.models.map((m) => m.id),
    ["model-a", "model-b"],
  );
  assert.deepEqual(traffic.slice(beforeBuiltinDirectory), [
    { method: "GET", path: "/v1/models" },
  ]);
  const builtinReceipt = await api("/api/provider-operations", {
    storeId,
    operationId: randomUUID(),
    expectedSequence: (await protocol()).sequence,
    kind: "save",
    provider: {
      ...builtinDraft,
      model: "model-b",
      models: builtinDirectory.models.map((m) => m.id),
    },
  });
  const builtinProvider = builtinReceipt.provider;
  for (const model of builtinProvider.models)
    await api(`/api/providers/${builtinProvider.id}/verify`, {
      expectedRevision: builtinProvider.revision,
      model,
    });
  const nativeConfig = (await api("/api/provider-status")).configs.find(
    (c) => c.agentId === "opencode",
  );
  const builtinTransaction = await api("/api/provider-switches", {
    agentId: "opencode",
    providerId: builtinProvider.id,
    expectedProviderRevision: builtinProvider.revision,
    model: "model-b",
    expectedRevision: nativeConfig.revision,
  });
  await api(`/api/provider-switches/${builtinTransaction.id}/apply`, {});
  const builtinFile = JSON.parse(fs.readFileSync(nativePath)),
    builtinConnection =
      builtinFile.provider[builtinFile.model.split("/")[0]].options;
  known = new Set((await api("/api/requests")).items.map((r) => r.id));
  count = posts();
  const builtinResponse = await fetch(builtinConnection.baseURL + "/messages", {
    method: "POST",
    headers: {
      "x-api-key": builtinConnection.apiKey,
      "content-type": "application/json",
    },
    body: JSON.stringify({
      model: "model-b",
      stream: true,
      max_tokens: 50,
      messages: [{ role: "user", content: "owned builtin chain" }],
    }),
    signal: AbortSignal.timeout(15000),
  });
  assert.equal(builtinResponse.status, 200);
  const builtinText = await builtinResponse.text();
  assert(
    builtinText.includes("CHAIN_你好") &&
      builtinText.includes("event: message_stop"),
  );
  assert.equal(posts(), count + 1);
  const builtinObserved = await observedAfter(known);
  assert.equal(builtinObserved.status, "success");
  assert.equal(builtinObserved.providerId, builtinProvider.id);
  assert.equal(builtinObserved.agentId, "opencode");
  const builtinRestore = await api(
    `/api/provider-switches/${builtinTransaction.id}/restore`,
    {},
  );
  await api(`/api/provider-switches/${builtinRestore.id}/apply`, {});
  assert.equal(fs.readFileSync(nativePath, "utf8"), originalNative);
  passed(
    "移除全部外部实现后，显式内置预设、目录、两个模型验证和真实 OpenCode 配置请求链路保持；新连接不覆盖原外部来源",
  );
  const claudeConfig = (await api("/api/provider-status")).configs.find(
    (c) => c.agentId === "claude",
  );
  const claudeTransaction = await api("/api/provider-switches", {
    agentId: "claude",
    providerId: builtinProvider.id,
    expectedProviderRevision: builtinProvider.revision,
    model: "model-b",
    expectedRevision: claudeConfig.revision,
  });
  assert.equal(fs.readFileSync(claudePath, "utf8"), originalClaude);
  await api(`/api/provider-switches/${claudeTransaction.id}/apply`, {});
  const claudeFile = JSON.parse(fs.readFileSync(claudePath));
  assert.equal(claudeFile.env.KEEP, "old");
  assert.deepEqual(claudeFile.permissions.deny, ["Read(.env)"]);
  assert.equal(
    claudeFile.env.ANTHROPIC_BASE_URL,
    claudeTransaction.routing.localEndpoint,
  );
  assert(claudeFile.apiKeyHelper.includes("--credential"));
  assert(!fs.readFileSync(claudePath, "utf8").includes(secret));
  const claudeRestore = await api(
    `/api/provider-switches/${claudeTransaction.id}/restore`,
    {},
  );
  await api(`/api/provider-switches/${claudeRestore.id}/apply`, {});
  assert.equal(fs.readFileSync(claudePath, "utf8"), originalClaude);
  const rememberedSource = await api("/api/provider-catalog-source", {
    kind: "state",
    query: {
      storeId,
      providerId: provider.id,
      expectedRevision: String(provider.revision),
    },
  });
  assert.equal(rememberedSource.kind, "external");
  assert.equal(rememberedSource.authorized, false);
  assert.deepEqual(
    await api("/api/provider-source-operations", {
      kind: "query",
      request: sourceRequest,
    }),
    saved,
  );
  assert.deepEqual(
    readCoreSchema(path.join(data, "local-agent.sqlite3")),
    schema,
  );
  assert.deepEqual(upstreamErrors, []);
  report.syntheticModelPosts = posts();
  report.catalogGets = traffic.filter(
    (t) => t.method === "GET" && t.path.endsWith("/catalog"),
  ).length;
  report.explicitBuiltinDirectoryGets = traffic.filter(
    (t) => t.method === "GET" && t.path.endsWith("/models"),
  ).length;
  report.implicitBuiltinFallbackGets = report.explicitBuiltinDirectoryGets - 1;
  assert.equal(report.syntheticModelPosts, 7);
  assert.equal(report.explicitBuiltinDirectoryGets, 1);
  assert.equal(report.implicitBuiltinFallbackGets, 0);
  assert.equal(report.catalogGets, 1);
  passed(
    "Claude Code 的同一内置 ConfigPlanner 预览、应用及精确恢复保持；未执行钥匙串 helper 或启动 Agent，原外部来源与旧回执始终独立保留",
  );
  report.mainSchema = schema.version;
  report.restoredFileHashes = {
    opencode: sha(nativePath),
    claude: sha(claudePath),
    finiteJson: sha(finitePath),
  };
  report.restoredFileModes = Object.fromEntries(
    [
      ["opencode", nativePath],
      ["claude", claudePath],
      ["finiteJson", finitePath],
    ].map(([name, file]) => [name, fs.statSync(file).mode & 0o777]),
  );
  report.complete = true;
} finally {
  report.traffic = traffic;
  report.upstreamErrors = upstreamErrors;
  for (const item of held) item.res.destroy();
  await stop();
  upstream.closeAllConnections();
  await new Promise((resolve) => upstream.close(resolve));
  report.ownedServiceStopped =
    !child || child.exitCode !== null || child.signalCode !== null;
  fs.rmSync(root, { recursive: true, force: true });
  report.temporaryDataRemoved = !fs.existsSync(root);
  fs.writeFileSync(output, JSON.stringify(report, null, 2) + "\n");
}
