import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { once } from "node:events";
import { createInterface } from "node:readline";
import http from "node:http";
import { readCoreSchema } from "./lib/core-schema.mjs";
import {catalogProbe,catalogConnectionCases} from "./lib/catalog-connection-cases.mjs";
import {catalogSourceOperationCases} from "./lib/catalog-source-operation-cases.mjs";
import {catalogSourceCases} from "./lib/catalog-source-cases.mjs";
import {catalogDraftCases} from "./lib/catalog-draft-cases.mjs";
import {catalogRecoveryCases} from "./lib/catalog-recovery-cases.mjs";
import {catalogOwnerCases} from "./lib/catalog-owner-cases.mjs";

const connectionMode=process.env.PROVIDER_CATALOG_CONNECTION==="1";
const sourceMode=process.env.PROVIDER_CATALOG_SOURCE==="1";
assert(!sourceMode||connectionMode,"Source cases require connection cases");
let sourceCases, ownerCases, recoveryCases;
const modelControl={mode:"ok",held:new Set(),waiting:null};

const binary = path.resolve(
  process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
    "target/debug/local-agent-service",
);
const output = process.env.PROVIDER_CATALOG_REPORT;
assert(output, "Explicit report required");
const hostileWasm = process.env.PROVIDER_CATALOG_HOSTILE_WASM;
assert(hostileWasm && fs.existsSync(hostileWasm), "Explicit hostile fixture required");
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "local-agent-provider-catalog-")),
);
const data = path.join(root, "data"),
  home = path.join(root, "home");
fs.mkdirSync(home);
const hash = (p) =>
  createHash("sha256").update(fs.readFileSync(p)).digest("hex");
const report = {
  complete: false,
  binary,
  binarySha256: hash(binary),
  harnessSha256: hash(new URL(import.meta.url)),
  connectionHarnessSha256: connectionMode?hash(new URL("./lib/catalog-connection-cases.mjs",import.meta.url)):null,
  sourceHarnessSha256: sourceMode?hash(new URL("./lib/catalog-source-cases.mjs",import.meta.url)):null,
  draftHarnessSha256: process.env.PROVIDER_DRAFT_CATALOG==="1"?hash(new URL("./lib/catalog-draft-cases.mjs",import.meta.url)):null,
  recoveryHarnessSha256: process.env.PROVIDER_DRAFT_RECOVERY==="1"?hash(new URL("./lib/catalog-recovery-cases.mjs",import.meta.url)):null,
  ownerHarnessSha256: process.env.PROVIDER_DRAFT_OWNERS==="1"?hash(new URL("./lib/catalog-owner-cases.mjs",import.meta.url)):null,
  wasmSha256: hash("sdk/plugins/examples/model-catalog/package/plugin.wasm"),
  hostileWasmSha256: hash(hostileWasm),
  cases: [],
  adversarialCases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
};
const passed = (name) => {
  report.cases.push(name);
  console.log("通过：" + name);
};
const secret = "catalog-fixture-key-7126";
let mode = "normal",
  posts = 0,
  nativeCalls = 0,
  trapCalls = 0,
  started,
  child,
  base,
  token;
const requests = [];
const held = new Set();
const upstream = http.createServer(async (req, res) => {
  if (req.method !== "GET") {
    posts++;
    if(connectionMode) {await catalogProbe(req,res,modelControl,secret);return;}
    res.writeHead(500);
    res.end();
    return;
  }
  if (req.url.startsWith("/trap")) {
    trapCalls++;
    res.writeHead(500);
    res.end();
    return;
  }
  assert.equal(req.headers.authorization, "Bearer " + secret);
  const url = new URL(req.url, "http://127.0.0.1");
  if (url.pathname === "/custom/v1/models") {
    nativeCalls++;
    res.end(JSON.stringify({ data: [{ id: "native-only" }] }));
    return;
  }
  assert.equal(url.pathname, "/custom/v1/catalog");
  requests.push(req.url);
  if (mode === "hold") {
    held.add(res);
    res.on("close", () => held.delete(res));
    started?.();
    return;
  }
  if (mode === "redirect") {
    res.writeHead(307, { location: "/trap" });
    res.end();
    return;
  }
  if (mode === "large") {
    res.end(JSON.stringify({ entries: [], padding: "x".repeat(262145) }));
    return;
  }
  if (mode === "echo") {
    res.end(JSON.stringify({ entries: [], echo: secret }));
    return;
  }
  if (mode === "escaped") {
    res.end(
      '{"entries":[],"echo":"' +
        [...secret]
          .map((c) => "\\u" + c.charCodeAt(0).toString(16).padStart(4, "0"))
          .join("") +
        '"}',
    );
    return;
  }
  if (mode === "metadata") {
    res.end(JSON.stringify({ entries: [{ key: "bad", window: 0 }] }));
    return;
  }
  if (mode === "repeat") {
    res.end(JSON.stringify({ entries: [], more: true, cursor: "repeat" }));
    return;
  }
  const next = url.searchParams.get("after_id");
  res.end(
    JSON.stringify(
      next
        ? {
            entries: [
              { key: "model-a", label: "最终名称", window: 16384 },
              { key: "model-b", tools: true, outputLimit: 512 },
            ],
            more: false,
          }
        : {
            entries: [
              { key: "model-a", label: "旧名称", window: 8192 },
              { key: "model-false", tools: false },
            ],
            more: true,
            cursor: "下一页_你好",
          },
    ),
  );
});
await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));
const endpoint = `http://127.0.0.1:${upstream.address().port}/custom/v1/responses`;
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
  const r = await fetch(base + route, {
    method,
    headers: {
      authorization: "Bearer " + token,
      "content-type": "application/json",
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(25000),
  });
  const value = expected === 422 ? { rejectedText: await r.text() } : await r.json();
  if (expected === 422) assert.match(value.rejectedText, /Failed to deserialize/);
  assert.equal(r.status, expected, `${route}:${body?.pluginId ?? ""}:${body?.action?.kind ?? ""} ${JSON.stringify(value)}`);
  return value;
}
const sdk = (...args) =>
  execFileSync(binary, ["--plugin-sdk", ...args], { stdio: "pipe" });
const key = path.join(root, "publisher.key");
function pack(version = "1.0.0", behavior = null) {
  const folder = path.join(root, "package-" + randomUUID());
  fs.cpSync("sdk/plugins/examples/model-catalog/package", folder, {
    recursive: true,
  });
  const m = JSON.parse(fs.readFileSync(path.join(folder, "manifest.json")));
  m.version = version;
  if (behavior) {
    m.id = "org.example." + behavior;
    m.settings = [
      {
        key: "behavior",
        label: "Owned test fixture",
        options: [behavior],
        default: behavior,
      },
    ];
    fs.copyFileSync(
      hostileWasm,
      path.join(folder, "plugin.wasm"),
    );
  }
  fs.writeFileSync(path.join(folder, "manifest.json"), JSON.stringify(m));
  const file = folder + ".laplug";
  sdk("pack", folder, file, key);
  return file;
}
async function install(file) {
  const review = await api("/api/plugins/inspect", { path: file });
  return api("/api/plugins/install", {
    packageRef: review.packageRef,
    digest: review.digest,
    approvePublisher: true,
  });
}
const ref = (p) => ({ pluginId: p.manifest.id, digest: p.digest });
const state = (p, action, expected = 200) =>
  api("/api/plugins/state", { ...ref(p), action }, expected);
const status = (p) => api("/api/provider-catalog/status", ref(p));
const action = (p, action, expected = 200) =>
  api("/api/provider-catalog/action", { ...ref(p), action }, expected);
const saved = (p) => ({
  kind: "saved",
  providerId: p.id,
  expectedRevision: String(p.revision),
});
const bind = (p, target, allow = true) =>
  action(p, { kind: "bind", target, allowPrivateNetwork: allow });
const read = (p, b, target, expected = 200) =>
  action(p, { kind: "read", bindingId: b.bindingId, target }, expected);
const unbind = (p, b) => action(p, { kind: "unbind", bindingId: b.bindingId });
let provider, plugin;
try {
  await boot();
  sdk("keygen", key);
  const schema = readCoreSchema(path.join(data, "local-agent.sqlite3"));
  assert.equal(schema.version, 16);
  assert.equal(
    (
      await fetch(base + "/api/provider-catalog/status", {
        method: "POST",
        body: "{}",
      })
    ).status,
    401,
  );
  const credential = await api("/api/provider-credentials", {
    label: "Owned catalog fixture",
    secret,
  });
  provider = (await api("/api/snapshot")).providers.find(
    (p) => p.id === "openai",
  );
  await api(
    "/api/providers",
    {
      ...provider,
      ...credential,
      endpoint,
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
  plugin = await state(await install(pack()), "enable");
  for (let n = 0; n < 3; n++)
    assert.equal((await status(plugin)).bindings.length, 0);
  assert.equal(requests.length, 0);
  passed("安装、启用和元数据读取均不请求目录，也不把待授权显示为连接成功");
  let target = saved(provider);
  let binding = (await bind(plugin, target, false)).bindings[0];
  const before = requests.length;
  assert.equal(
    (await read(plugin, binding, target, 403)).code,
    "permission_denied",
  );
  assert.equal(requests.length, before);
  binding = (await bind(plugin, target)).bindings[0];
  assert.equal(
    (await bind(plugin, target)).bindings[0].bindingId,
    binding.bindingId,
  );
  const recordFolder = path.join(data, "plugins/provider-catalog");
  const paths = fs
    .readdirSync(recordFolder)
    .map((name) => path.join(recordFolder, name));
  assert.equal(paths.length, 1);
  assert.equal(fs.statSync(paths[0]).mode & 0o777, 0o600);
  assert(!fs.readFileSync(paths[0], "utf8").includes(secret));
  const original = JSON.stringify(provider);
  const catalog = await read(plugin, binding, target);
  assert.equal(catalog.models.length, 3);
  assert.deepEqual(
    catalog.models.map((m) => m.id),
    ["model-a", "model-b", "model-false"],
  );
  assert.equal(catalog.models[0].name, "最终名称");
  assert.equal(catalog.models[0].contextWindow, 16384);
  assert.equal(catalog.models[0].capabilities.tools, null);
  assert.equal(catalog.models[2].capabilities.tools, false);
  assert.equal(catalog.models[1].maxOutputTokens, 512);
  assert.equal(new URL(catalog.sourceUrl).pathname, "/custom/v1/catalog");
  assert(
    requests.some(
      (p) =>
        new URL(p, "http://localhost").searchParams.get("after_id") ===
        "下一页_你好",
    ),
  );
  assert.equal(
    JSON.stringify(
      (await api("/api/snapshot")).providers.find((p) => p.id === provider.id),
    ),
    original,
  );
  passed(
    "真实签名 Wasm 与共享 ProviderAdapter 读取分页目录；未知、false、重复最后值、路径和原 Provider 保持",
  );
  for (const [behavior, code] of [
    ["metadata", 400],
    ["repeat", 400],
    ["large", 400],
    ["redirect", 400],
    ["echo", 403],
    ["escaped", 403],
  ]) {
    mode = behavior;
    const nativeBefore = nativeCalls;
    await read(plugin, binding, target, code);
    assert.equal(nativeCalls, nativeBefore);
  }
  assert.equal(trapCalls, 0);
  assert.equal(posts, 0);
  passed(
    "无效容量、重复游标、大小、重定向和原始 / 转义密钥回显被拒绝；不调用原生目录或模型",
  );
  mode = "hold";
  const waiting = new Promise((resolve) => {
    started = resolve;
  });
  const pending = read(plugin, binding, target, 403);
  await waiting;
  const t = performance.now();
  await api("/api/snapshot");
  assert(performance.now() - t < 1500);
  await state(plugin, "revoke-grants");
  const revoked = await pending;
  assert.equal(revoked.code, "grant_revoked");
  for (const res of held) res.destroy();
  assert(!(await status(plugin)).bindings[0].authorized);
  passed("目录等待期间核心仍响应，撤权立即拒绝在途结果并保留失效绑定");
  mode = "normal";
  binding = (await bind(plugin, target)).bindings[0];
  const draft = {
    kind: "draft",
    label: "Owned draft",
    endpoint,
    protocol: "responses",
    vendor: provider.vendor,
    credentialRef: provider.credentialRef,
  };
  const draftBinding = (await bind(plugin, draft)).bindings.find(
    (b) => !b.providerId,
  );
  assert(draftBinding);
  assert.equal((await read(plugin, draftBinding, draft)).models.length, 3);
  await read(plugin, draftBinding, { ...draft, label: "changed" }, 409);
  await unbind(plugin, draftBinding);
  passed("未保存草稿也可显式授权读取；范围变化拒绝，放弃授权不改原连接");
  await api(
    "/api/providers",
    { ...provider, name: "Changed saved revision" },
    200,
    "PUT",
  );
  provider = (await api("/api/snapshot")).providers.find(
    (p) => p.id === provider.id,
  );
  assert(!(await status(plugin)).bindings[0].authorized);
  const count = requests.length;
  await read(plugin, binding, target, 409);
  assert.equal(requests.length, count);
  target = saved(provider);
  binding = (await bind(plugin, target)).bindings[0];
  const newer = await state(await install(pack("1.1.0")), "enable");
  assert(!(await status(newer)).bindings[0].authorized);
  await read(newer, binding, target, 403);
  plugin = newer;
  binding = (await bind(plugin, target)).bindings[0];
  passed("已保存修订与插件 digest 变化均使旧绑定失效，不静默换 Key 或回退");
  {
    for (const behavior of [
      "catalog-foreign",
      "catalog-stale",
      "catalog-many",
      "catalog-host-call",
      "catalog-context",
    ]) {
      console.log("验证恶意目录场景：" + behavior);
      report.activeCase = behavior;
      const p = await state(await install(pack("1.0.0", behavior)), "enable");
      const b = (await bind(p, target)).bindings[0];
      const nativeBefore = nativeCalls;
      const getsBefore = requests.length;
      const c = await read(
        p,
        b,
        target,
        behavior === "catalog-context" ? 200 : ["catalog-foreign","catalog-host-call"].includes(behavior) ? 403 : 400,
      );
      if (behavior === "catalog-context") {
        const context = JSON.parse(c.models[0].name);
        assert.equal(context.basePath, "");
        assert.equal(context.vendor, "");
        assert.equal(context.officialCatalogRoot, false);
        assert(!c.models[0].name.includes(endpoint));
      }
      assert.equal(nativeCalls, nativeBefore);
      assert.equal(requests.length - getsBefore, behavior === "catalog-foreign" ? 0 : 1);
      assert.equal(c.code ?? null, {
        "catalog-foreign":"permission_denied", "catalog-stale":"invalid_input",
        "catalog-many":"invalid_input", "catalog-host-call":"permission_denied", "catalog-context":null,
      }[behavior]);
      report.adversarialCases.push({ behavior, code: c.code ?? null });
      await unbind(p, b);
      await state(p, "remove");
    }
    passed(
      "恶意路径、其他快照、超量候选和 guest 主动网络调用均被拒绝；guest 不收到 Endpoint 路径或凭据",
    );
  }
  report.adversarialCasesBeforeRestart = true;
  if(connectionMode) {
    const scoped=(storeId,request,expected=200)=>api("/api/catalog-provider-scope",{storeId,request},expected);
    if(process.env.PROVIDER_DRAFT_CATALOG==="1") await catalogDraftCases({api,plugin,provider,scoped,state,report,oldGets:()=>requests.length+nativeCalls});
    if(process.env.PROVIDER_DRAFT_OWNERS==="1") ownerCases=await catalogOwnerCases({api,plugin,provider,scoped,control:modelControl,gets:()=>requests.length,posts:()=>posts,data,report});
    if(process.env.PROVIDER_DRAFT_RECOVERY==="1") recoveryCases=await catalogRecoveryCases({api,plugin,provider,scoped,control:modelControl,gets:()=>requests.length,posts:()=>posts,data,report});
    const connected=await catalogConnectionCases({api,plugin,provider,posts:()=>posts,gets:()=>requests.length,state,control:modelControl,report,scoped,data});
    if(process.env.PROVIDER_SOURCE_OPERATIONS === "1") await catalogSourceOperationCases({api,plugin,provider,scoped,posts:()=>posts,gets:()=>requests.length,nativeGets:()=>nativeCalls,report,control:modelControl});
    if(sourceMode) sourceCases=await catalogSourceCases({api,plugin,...connected,scoped,state,gets:()=>requests.length,posts:()=>posts,nativeGets:()=>nativeCalls,data,report,
      hold:()=>{mode="hold";return new Promise((resolve)=>started=resolve);},
      release:()=>{for(const response of held)response.destroy();mode="normal";started=null;}});
    // Connection revocation invalidates the primary saved grant as expected.
    binding=(await bind(plugin,target)).bindings.find((b)=>b.providerId===provider.id);
  }
  await ownerCases?.beforeRestart();
  await recoveryCases?.beforeRestart();
  const beforeRestart = requests.length;
  await stop();
  await boot();
  assert(!(await status(plugin)).bindings[0].authorized);
  await read(plugin, binding, target, 403);
  assert.equal(requests.length, beforeRestart);
  await sourceCases?.afterRestart();
  await ownerCases?.afterRestart();
  await recoveryCases?.afterRestart();
  passed("实际服务重启不继承目录授权，旧绑定保留为失效状态且不发送请求");
  await state(plugin, "disable");
  await read(plugin, binding, target, 403);
  assert.equal(
    (await state(plugin, "remove", 400)).code,
    "catalog_binding_active",
  );
  await unbind(plugin, binding);
  assert.equal((await status(plugin)).bindings.length, 0);
  await state(plugin, "remove");
  await sourceCases?.afterRemove();
  passed("停用后读取拒绝，宿主可独立解除绑定，再卸载插件");

  assert.deepEqual(
    readCoreSchema(path.join(data, "local-agent.sqlite3")),
    schema,
  );
  if(!connectionMode) assert.equal(posts, 0);
  assert.equal(trapCalls, 0);
  report.complete = true;
  delete report.activeCase;
  report.catalogGets = requests.length;
  report.nativeFallbackGets = nativeCalls-(report.explicitNativeGets??0);
  report.syntheticModelPosts = posts;
} finally {
  await stop();
  for (const res of held) res.destroy();
  for (const res of modelControl.held) res.destroy();
  upstream.closeAllConnections();
  await new Promise((resolve) => upstream.close(resolve));
  report.ownedServiceStopped =
    !child || child.exitCode !== null || child.signalCode !== null;
  fs.rmSync(root, { recursive: true, force: true });
  report.temporaryDataRemoved = !fs.existsSync(root);
  fs.writeFileSync(output, JSON.stringify(report, null, 2) + "\n");
}
