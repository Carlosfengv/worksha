import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { mkdtempSync, realpathSync, mkdirSync, readFileSync, writeFileSync, cpSync, symlinkSync, unlinkSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import http from "node:http";
import { randomUUID, createHash } from "node:crypto";
import { DatabaseSync } from "node:sqlite";
import { readCoreSchema } from "./lib/core-schema.mjs";

const binary = path.resolve(process.env.PLUGIN_TEST_BINARY ?? "target/debug/local-agent-service");
const production = process.env.PLUGIN_TEST_PRODUCTION === "1";
const root = realpathSync(mkdtempSync(path.join(tmpdir(), "local-agent-plugin-m4-")));
const home = path.join(root, "home"); mkdirSync(home);
const data = path.join(root, "data");
const report = { complete: false, binary, root, cases: [] };
const passed = (name) => { report.cases.push(name); console.log(`通过：${name}`); };
const sdk = (...args) => execFileSync(binary, ["--plugin-sdk", ...args], { encoding: "utf8" });
const key = path.join(root, "publisher.key"); sdk("keygen", key);
function pack(example, version = "1.0.0", edit = () => {}) {
  const folder = path.join(root, `${example}-${version}-${randomUUID()}`);
  cpSync(`sdk/plugins/examples/${example}/package`, folder, { recursive: true });
  const manifest = JSON.parse(readFileSync(path.join(folder, "manifest.json"))); manifest.version = version; edit(manifest);
  writeFileSync(path.join(folder, "manifest.json"), JSON.stringify(manifest));
  const output = folder + ".laplug"; sdk("pack", folder, output, key); return output;
}
const source = path.join(root, "aurora.jsonl"); cpSync("sdk/plugins/fixtures/aurora.jsonl", source);
const originalSource = readFileSync(source);
const sourceHash = () => createHash("sha256").update(readFileSync(source)).digest("hex");
const initialHash = sourceHash();
const secret = "plugin-credential-fixture-827401";
const requests = []; let behavior = "ok"; let delayedStarted;
const upstream = http.createServer((req, res) => {
  requests.push({ method: req.method, path: req.url });
  assert.equal(req.headers.authorization, `Bearer ${secret}`);
  assert.equal(req.url, "/v1/quota"); assert.equal(req.method, "GET");
  const reply = () => {
    if(behavior === "echo-secret-escaped"){res.writeHead(200,{"content-type":"application/json"});res.end('{"balance":"'+[...secret].map(c=>'\\u'+c.charCodeAt(0).toString(16).padStart(4,'0')).join('')+'"}');}
    else if (behavior === "redirect") { res.writeHead(307, { Location: "http://127.0.0.1:1/never" }); res.end(); }
    else { res.writeHead(200, { "content-type": "application/json" }); res.end(JSON.stringify(behavior === "echo-secret" ? { balance: secret } : { balance: "12.5000", limit: "100.0000", unit: "credits", expiresAt: "2026-12-01T00:00:00Z" })); }
  };
  if (behavior === "delay") { delayedStarted?.(); setTimeout(reply, 350); }
  else if(behavior==="hang"){delayedStarted?.();}
  else reply();
});
await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));
let child, base, token, stderr = "";
async function boot() {
  token = randomUUID();
  const env = { ...process.env, LOCAL_AGENT_SOURCE_HOME: home, LOCAL_AGENT_SERVICE_TOKEN: token, ...(production ? { LOCAL_AGENT_VAULT_WORKER: "1" } : { LOCAL_AGENT_TEST_MEMORY_VAULT: "1", LOCAL_AGENT_TEST_IDLE_AGENTS: "1" }) };
  if (production) { delete env.LOCAL_AGENT_TEST_MEMORY_VAULT; delete env.LOCAL_AGENT_TEST_IDLE_AGENTS; }
  child = spawn(binary, ["--data-dir", data], { env, stdio: ["ignore", "pipe", "pipe"] });
  child.stderr.on("data", (b) => { stderr += b; });
  const reader = createInterface({ input: child.stdout });
  const [line] = await Promise.race([once(reader, "line", { signal: AbortSignal.timeout(20000) }), once(child, "exit").then(() => { throw new Error("Service exited before ready: " + stderr.replaceAll(secret, "[redacted]")); })]); base = `http://127.0.0.1:${JSON.parse(line).port}`;
}
async function api(route, body, expected = 200, method = body === undefined ? "GET" : "POST") {
  const response = await fetch(base + route, { method, headers: { authorization: `Bearer ${token}`, "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(45000) });
  const result = await response.json(); assert.equal(response.status, expected, `${route}: ${JSON.stringify(result)}`); return result;
}
async function stop() { if (!child || child.exitCode !== null) return; const exited = once(child, "exit"); await api("/api/runtime/shutdown", {}); await exited; }
const state = (p, action) => api("/api/plugins/state", { pluginId: p.manifest.id, digest: p.digest, action });
const action = (p, value, expected = 200) => api("/api/plugins/action", { pluginId: p.manifest.id, digest: p.digest, action: value }, expected);
const view = (p, viewId, cursor = null, expected = 200) => api("/api/plugins/view", { pluginId: p.manifest.id, viewId, cursor }, expected);
async function install(file, approve = true) {
  const review = await api("/api/plugins/inspect", { path: file });
  const result = await api("/api/plugins/install", { packageRef: review.packageRef, digest: review.digest, approvePublisher: approve }); return result;
}
try {
  await boot();
  const coreSchema = readCoreSchema(path.join(data, "local-agent.sqlite3"));
  assert(coreSchema.version > 0);
  assert.equal((await fetch(base + "/api/plugins")).status, 401);
  assert.equal((await fetch(base + "/api/plugins", { headers: { authorization: `Bearer ${token}`, origin: "https://untrusted.invalid" } })).status, 401);
  const auroraFile = pack("aurora-agent");
  assert(JSON.parse(sdk("check", auroraFile)).valid);
  const review = await api("/api/plugins/inspect", { path: auroraFile });
  assert.equal((await api("/api/plugins")).items.length, 0);
  assert.equal((await api("/api/plugins/install", { packageRef: review.packageRef, digest: review.digest, approvePublisher: false }, 400)).code, "publisher_untrusted");
  let aurora = await api("/api/plugins/install", { packageRef: review.packageRef, digest: review.digest, approvePublisher: true });
  assert(!aurora.enabled && !aurora.sourceAuthorized); assert.equal(requests.length, 0);
  aurora = await state(aurora, "enable"); assert(aurora.enabled); assert(!aurora.sourceAuthorized);
  assert.equal((await action(aurora, { kind: "refresh" }, 403)).code, "permission_denied");
  const unknown = await view(aurora, "history"); assert(unknown.needsRefresh && !unknown.confirmedEmpty);
  passed("签名、安装与执行分离；未授权来源、网络和 Origin 拒绝");
  await api("/api/plugins/source", { pluginId: aurora.manifest.id, digest: aurora.digest, path: source });
  await action(aurora, { kind: "refresh" });
  const first = await view(aurora, "history"); assert.equal(first.total, 2); assert.equal(first.summary.totalTokens, "9007199254741000");
  const firstDiagnostics = (await api("/api/plugins")).diagnostics;
  const diagnostic = firstDiagnostics.items.find(p => p.digest === aurora.digest);
  assert.equal(firstDiagnostics.scope, "service-lifetime");
  assert.equal(diagnostic.snapshot.state, "available"); assert.equal(diagnostic.snapshot.records, "2");
  assert.equal(diagnostic.activity.attempts, "3"); assert.equal(diagnostic.activity.failed, "1");
  assert.equal(diagnostic.activity.inFlight, "0"); assert(diagnostic.snapshot.observedAt);
  assert.deepEqual((await api("/api/plugins")).diagnostics, firstDiagnostics);
  assert(!JSON.stringify(firstDiagnostics).includes(source)); assert(!JSON.stringify(firstDiagnostics).includes("授权后读取"));
  const conversation = await action(aurora, { kind: "read-session", sessionId: first.items[0].sessionId, snapshot: first.snapshot });
  assert.equal(conversation.nativeId, "aurora-1"); assert(conversation.messages[1].text.includes("授权后读取")); assert.equal(sourceHash(), initialHash);
  passed("独立 Wasm JSONL 来源、按需正文与超过 2^53 的精确用量");
  const many = Array.from({ length: 513 }, (_, i) => ({ nativeId: `paged-${i}`, title: `Paged ${i}`, createdAt: null, model: null, usage: { inputTokens: "2", outputTokens: "1", cacheReadTokens: null, reasoningTokens: null }, messages: [] })).map((row) => JSON.stringify(row)).join("\n") + "\n";
  writeFileSync(source, many); await action(aurora, { kind: "refresh" });
  let page = await view(aurora, "history"), ids = [];
  const oldCursor = page.nextCursor;
  for (;;) { ids.push(...page.items.map((item) => item.sessionId)); if (!page.nextCursor) break; page = await view(aurora, "history", page.nextCursor); }
  assert.equal(ids.length, 513); assert.equal(new Set(ids).size, 513); assert.equal(page.summary.totalTokens, "1539");
  writeFileSync(source, many + '{"incomplete":'); const partialFailure = await action(aurora, { kind: "refresh" }, 400);
  assert.equal((await view(aurora, "history")).total, 513);
  const degradedDiagnostic = (await api("/api/plugins")).diagnostics.items.find(p => p.digest === aurora.digest);
  assert.equal(degradedDiagnostic.snapshot.state, "stale"); assert.equal(degradedDiagnostic.snapshot.records, "513");
  assert.equal(degradedDiagnostic.activity.lastErrorCode, partialFailure.code);
  writeFileSync(source, originalSource); await action(aurora, { kind: "refresh" });
  assert.equal((await view(aurora, "history", oldCursor, 409)).code, "stale_snapshot");
  passed("跨批次和冻结分页无重复；不完整尾部不删除旧快照；旧游标失效");
  const link = path.join(root, "link.jsonl"); symlinkSync(source, link);
  assert.equal((await api("/api/plugins/source", { pluginId: aurora.manifest.id, digest: aurora.digest, path: link }, 403)).code, "permission_denied");
  await state(aurora, "revoke-grants");
  assert.equal((await action(aurora, { kind: "read-session", sessionId: first.items[0].sessionId, snapshot: first.snapshot }, 409)).code, "stale_snapshot");
  assert((await view(aurora, "history")).needsRefresh);
  const revokedDiagnostic = (await api("/api/plugins")).diagnostics.items.find(p => p.digest === aurora.digest);
  assert.equal(revokedDiagnostic.snapshot.state, "awaiting-authorization");
  assert.equal(revokedDiagnostic.snapshot.records, null); assert.equal(revokedDiagnostic.snapshot.observedAt, null);
  passed("符号链接拒绝、撤权后正文 / 游标不可复活");
  const credential = await api("/api/provider-credentials", { label: "Plugin fixture", secret });
  const provider = { ...(await api("/api/snapshot")).providers.find((p) => p.id === "openai"), endpoint: `http://127.0.0.1:${upstream.address().port}/v1`, name: "Plugin local quota fixture", model: "fixture", models: ["fixture"], configured: true, ...credential };
  await api("/api/providers", provider, 200, "PUT");
  let quota = await install(pack("pulse-quota")); quota = await state(quota, "enable"); assert.equal(requests.length, 0);
  await action(quota, { kind: "bind-connection", providerId: "openai", allowPrivateNetwork: false }); await action(quota, { kind: "refresh" }, 403); assert.equal(requests.length, 0);
  await action(quota, { kind: "bind-connection", providerId: "openai", allowPrivateNetwork: true }); await action(quota, { kind: "refresh" });
  const quotaView = await view(quota, "balance"); assert.equal(quotaView.quota.remaining, "12.5000"); assert.equal(requests.length, 1); assert(!JSON.stringify(quotaView).includes(secret));
  behavior = "redirect"; await action(quota, { kind: "refresh" }, 400); assert.equal(requests.length, 2);
  behavior = "echo-secret"; await action(quota, { kind: "refresh" }, 403); assert(!JSON.stringify(await view(quota, "balance")).includes(secret));
  behavior = "echo-secret-escaped"; await action(quota, { kind: "refresh" }, 403); assert(!JSON.stringify(await view(quota, "balance")).includes(secret));
  behavior = "delay";
  const begun = new Promise((resolve) => { delayedStarted = resolve; });
  const pending = action(quota, { kind: "refresh" }, 403); await begun; await state(quota, "revoke-grants"); await pending;
  await action(quota,{kind:"bind-connection",providerId:"openai",allowPrivateNetwork:true});
  behavior="hang"; const blockedAt=performance.now();
  const blocking=action(quota,{kind:"refresh"},400); await api("/api/snapshot"); await blocking;
  assert(performance.now()-blockedAt<8000); behavior="ok";
  passed("额度 GET 绑定、私网显式授权、凭据宿主注入、重定向 / 密钥回显拒绝与在途撤权");
  const settings = { "title-style": "with-model" }; await action(aurora, { kind: "save-settings", values: settings });
  const newer = await install(pack("aurora-agent", "1.1.0")); assert.equal((await api("/api/plugins")).items.find((p) => p.digest === aurora.digest).enabled, true);
  await state(newer, "enable"); const switched = (await api("/api/plugins")).items.find((p) => p.digest === newer.digest); assert(!switched.sourceAuthorized);
  const rolledBack = await state(switched, "rollback"); assert.equal(rolledBack.digest, aurora.digest); assert.equal(rolledBack.settings["title-style"], "with-model"); assert(!rolledBack.sourceAuthorized);
  const beforeRepeat = rolledBack.activationEpoch; assert.equal((await state(rolledBack, "enable")).activationEpoch, beforeRepeat);
  const mismatch = pack("aurora-agent", "1.1.0", (m) => { m.displayName = "different same-version package"; }); const mismatchReview = await api("/api/plugins/inspect", { path: mismatch });
  assert.equal((await api("/api/plugins/install", { packageRef: mismatchReview.packageRef, digest: mismatchReview.digest, approvePublisher: false }, 400)).code, "capability_conflict");
  passed("更新不抢占旧版、显式切换 / 回退、旧设置快照、权限不继承和幂等启用");
  // Declarative contributions use host components and a fixed, explicitly granted query.
  let summaryPlugin = await install(pack("gateway-report"));
  summaryPlugin = await state(summaryPlugin,"enable");
  await action(summaryPlugin,{kind:"refresh"},403);
  await action(summaryPlugin,{kind:"authorize-report"});
  await action(summaryPlugin,{kind:"refresh"});
  const summaryView=await view(summaryPlugin,"summary");
  assert.equal(summaryView.report.requests,"0"); assert.equal(summaryView.report.attempts,"0");
  assert(!Object.keys(summaryView.report).some(k=>/body|secret|session|provider|requestId/i.test(k)));
  await state(summaryPlugin,"revoke-grants"); assert.equal((await view(summaryPlugin,"summary")).report??null,null);
  const preset1=await action(quota,{kind:"create-provider",preset:"pulse"});
  const preset2=await action(quota,{kind:"create-provider",preset:"pulse"});
  assert.equal(preset1.providerId,preset2.providerId); assert.equal(preset1.verified,false);
  passed("声明式报表单独授权 / 撤销；固定汇总无正文；供应商预设复制幂等且不验证");
  const modes=["loop","memory","oversized","trap","spoof-source","forged-batch","unknown-host","prepare-denied"];
  report.hostile=[];
  for(const mode of modes){
    const file=pack("aurora-agent","1.0.0",m=>{
      m.id=`org.example.hostile-${mode}`;m.displayName=`Hostile fixture ${mode}`;
      m.agent.productId=m.id+":cli";
      m.settings=[{key:"behavior",label:"Test behavior",options:modes,default:mode}];
    });
    const folder=file.slice(0,-7); cpSync(".work/plugin-m4-hostile.wasm",path.join(folder,"plugin.wasm")); unlinkSync(file); sdk("pack",folder,file,key);
    let hostile=await install(file);
    if(mode==="prepare-denied"){
      const failed=await api("/api/plugins/state",{pluginId:hostile.manifest.id,digest:hostile.digest,action:"enable"},403);
      assert.equal(failed.code,"permission_denied");
      assert(!(await api("/api/plugins")).items.find(p=>p.digest===hostile.digest).enabled);
    }else{
      hostile=await state(hostile,"enable");
      await api("/api/plugins/source",{pluginId:hostile.manifest.id,digest:hostile.digest,path:source});
      const start=performance.now();
      const failure=await action(hostile,{kind:"refresh"},["spoof-source","unknown-host"].includes(mode)?403:400);
      assert(performance.now()-start<32000); assert(["resource_limit","worker_crashed","invalid_input","permission_denied"].includes(failure.code),JSON.stringify(failure));
      const hostileActivity = (await api("/api/plugins")).diagnostics.items.find(p => p.digest === hostile.digest).activity;
      assert.equal(hostileActivity.failed, "1"); assert.equal(hostileActivity.lastErrorCode, failure.code);
      assert.equal(hostileActivity.resourceLimits, failure.code === "resource_limit" ? "1" : "0");
      assert.equal(hostileActivity.workerCrashes, failure.code === "worker_crashed" ? "1" : "0");
      report.hostile.push({mode,code:failure.code,elapsedMs:Math.round(performance.now()-start)});
      assert((await view(hostile,"history")).needsRefresh);
      await state(hostile,"disable");
    }
    assert.equal((await api("/api/snapshot")).providers.some(p=>p.id===preset1.providerId),true);
  }
  passed("真实 Wasm 无限循环 / 内存增长 / 过大帧 / trap / 伪造来源 / 伪造批次 / 越权 host 调用有界隔离；prepare 拒绝不激活");
  const failedUpgradeFile=pack("aurora-agent","1.2.0",m=>{m.settings=[{key:"behavior",label:"Test behavior",options:modes,default:"prepare-denied"}];});
  cpSync(".work/plugin-m4-hostile.wasm",path.join(failedUpgradeFile.slice(0,-7),"plugin.wasm"));unlinkSync(failedUpgradeFile);sdk("pack",failedUpgradeFile.slice(0,-7),failedUpgradeFile,key);
  const failedUpgrade=await install(failedUpgradeFile);
  await api("/api/plugins/state",{pluginId:failedUpgrade.manifest.id,digest:failedUpgrade.digest,action:"enable"},403);
  assert((await api("/api/plugins")).items.find(p=>p.digest===aurora.digest).enabled);
  await state(failedUpgrade,"remove");
  passed("升级 preflight 失败保持原版本；移除失败候选不影响活动版本");
  assert.deepEqual(readCoreSchema(path.join(data, "local-agent.sqlite3")), coreSchema);
  assert.equal(sourceHash(), initialHash);
  const metadata = readFileSync(path.join(data, "plugin-host.sqlite3")); assert(!metadata.includes(Buffer.from("这是虚构的离线会话"))); assert(!metadata.includes(Buffer.from(secret)));
  await stop(); await boot();
  const restarted = await api("/api/plugins"); assert(restarted.diagnostics.items.every(p => p.activity === null));
  const resumed = restarted.items.find((p) => p.digest === aurora.digest); assert(resumed.enabled); assert.equal(resumed.settings["title-style"], "with-model"); assert(!resumed.sourceAuthorized);
  const removed = await state(resumed, "remove"); assert.equal(removed.runtimeStatus, "removed"); assert.equal((await view(resumed, "history", null, 400)).code, "plugin_disabled");
  const reinstalled=await install(auroraFile); assert(!reinstalled.enabled);
  await state(reinstalled,"enable");
  await state(newer,"remove"); assert((await api("/api/plugins")).items.find(p=>p.digest===reinstalled.digest).enabled);
  await state(reinstalled,"remove");
  const revoked = await state(quota, "revoke-publisher"); assert(!revoked.enabled);
  assert((await api("/api/snapshot")).providers.some(p=>p.id===preset1.providerId));
  passed("同目录重开、卸载与发布者撤销；旧 schema、原始来源和凭据边界保留");
  report.complete = true; report.realModelCalls = 0; report.sourceUnchanged = sourceHash() === initialHash; report.requests = requests; report.schemaVersion = coreSchema.version;
} finally {
  if (child?.exitCode === null) { try { await stop(); } catch { child.kill("SIGTERM"); await once(child, "exit"); } }
  upstream.closeAllConnections(); await new Promise((resolve) => upstream.close(resolve));
  rmSync(key, { force: true }); report.temporarySigningKeyRemoved = true;
  if (production && existsSync(path.join(data, "local-agent.sqlite3"))) {
    const core = new DatabaseSync(path.join(data, "local-agent.sqlite3"), { readOnly: true });
    const refs = new Set(core.prepare("SELECT value FROM providers").all().map(({value}) => JSON.parse(value).credentialRef).filter(Boolean)); core.close();
    report.temporaryCredentialsRemoved = [...refs].map(account => JSON.parse(execFileSync(binary, ["--vault-worker"], { env: { ...process.env, LOCAL_AGENT_VAULT_WORKER: "1" }, input: JSON.stringify({ operation: "delete", account, bytes: [] }), encoding: "utf8" })).code === null);
    assert(report.temporaryCredentialsRemoved.every(Boolean));
  }
  report.processesExited = child?.exitCode !== null; report.stderrHadRawSecret = stderr.includes(secret); assert(!report.stderrHadRawSecret);
  const destination = process.env.PLUGIN_TEST_REPORT ?? ".work/plugins.json";
  mkdirSync(path.dirname(destination), { recursive: true });
  writeFileSync(destination, JSON.stringify(report, null, 2) + "\n");
  console.log("Evidence:", destination);
}
