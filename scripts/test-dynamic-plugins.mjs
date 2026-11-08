import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn, execFileSync } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { randomUUID } from "node:crypto";
import { normalizeSession } from "../sdk/plugins/examples/dsh-desktop/bridge/normalize.js";
const binary = path.resolve(process.env.PLUGIN_TEST_BINARY ?? "target/debug/local-agent-service");
const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "dynamic-plugins-")));
const home = path.join(root, "home"), data = path.join(root, "data"), key = path.join(root, "publisher.key");
fs.mkdirSync(home);
const sdk = (...args) => execFileSync(binary, ["--plugin-sdk", ...args], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
sdk("keygen", key);
const report = { complete: false, cases: [], realModelCalls: 0, realKeychainCalls: 0 };
const pass = (name) => { report.cases.push(name); console.log("通过：" + name); };
function pack(example, name = example, edit = () => {}) {
  const folder = path.join(root, name);
  fs.cpSync(`sdk/plugins/examples/${example}/package`, folder, { recursive: true });
  edit(folder);
  const archive = folder + ".laplug"; sdk("pack", folder, archive, key); return archive;
}
let child, exited, base;
const token = randomUUID();
async function api(route, body, status = 200) {
  const r = await fetch(base + route, { method: body === undefined ? "GET" : "POST", headers: { authorization: "Bearer " + token, "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(20000) });
  const value = await r.json(); assert.equal(r.status, status, route + ": " + JSON.stringify(value)); return value;
}
const sourceId = "net.worksha.dsh-desktop:desktop", productId = sourceId;
const secondSourceId = "org.example.full-history:history";
const secondProductId = "org.example.full-history:terminal";
const query = { scope: { kind: "external", sourceIds: [sourceId] }, filter: { query: null, productId: null, days: null }, snapshotToken: null, cursor: null };
const history = path.join(root, "history.jsonl");
const rows = (count, product = productId) => Array.from({ length: count }, (_, i) => ({ fact: { kind: "session", value: { nativeId: "s" + i, productIds: [product], unknownClient: false, title: "Owned fixture " + i, createdAt: null, updatedAt: null, projectRef: null, usage: { inputTokens: "15", outputTokens: "4", cachedInputTokens: null, cacheWriteInputTokens: null, reasoningOutputTokens: null, totalTokens: "19" }, partial: true } }, conversation: [{ id: "m0", nativeId: "m0", turnId: "t0", kind: "message", role: "user", status: "recorded", text: "Snapshot " + count, input: null, output: null, model: null, responseId: null, usage: null }] }));
const writeRows = (count, torn = false) => {
  const records = rows(count);
  records.push({ fact: { kind: "activity", value: { nativeId: "a1", sessionRef: "s0", responseRef: null, occurredAt: new Date().toISOString(), phase: "running", durationMs: null } } });
  const snapshot = randomUUID();
  fs.writeFileSync(history, [{ snapshot, records: records.length }, ...records, { commit: torn ? randomUUID() : snapshot }].map((r) => JSON.stringify(r)).join("\n") + "\n");
};
const writeDshObservation = () => {
  const time = Date.now();
  const event = (seq, type, data) => ({ type: "event", event: { seq, time: time + seq, type, data } });
  const records = normalizeSession({ sessionId: "s0", updatedAt: time, running: false }, { hasMore: false, records: [
    event(1, "user/message", { content: "Owned prompt" }),
    event(2, "user/message", { source: { kind: "runtime-context" }, content: "Owned context" }),
    event(3, "tool/call", { callId: "probe", name: "mcp__owned__probe", arguments: "private arguments" }),
    event(4, "tool/result", { message: { toolCallId: "probe", isError: false, content: "private result" } }),
  ] }, new Date(time).toISOString());
  const snapshot = randomUUID();
  fs.writeFileSync(history, [{ snapshot, records: records.length }, ...records, { commit: snapshot }].map((r) => JSON.stringify(r)).join("\n") + "\n");
};
try {
  child = spawn(binary, ["--data-dir", data], { env: { ...process.env, LOCAL_AGENT_SOURCE_HOME: home, LOCAL_AGENT_SERVICE_TOKEN: token, LOCAL_AGENT_TEST_MEMORY_VAULT: "1", LOCAL_AGENT_TEST_IDLE_AGENTS: "1" }, stdio: ["ignore", "pipe", "pipe"] });
  exited = once(child, "exit"); let error = ""; child.stderr.on("data", (v) => error += v);
  const [line] = await Promise.race([once(createInterface({ input: child.stdout }), "line", { signal: AbortSignal.timeout(15000) }), exited.then(() => { throw Error(error); })]); base = "http://127.0.0.1:" + JSON.parse(line).port;
  const initial = await api("/api/snapshot");
  const reportFile = pack("source-report", "source-report", (folder) => {
    const file = path.join(folder, "manifest.json");
    const manifest = JSON.parse(fs.readFileSync(file));
    manifest.views.push({ ...manifest.views[0], id: "unbound", title: "No source binding" });
    manifest.views.push({ ...manifest.views[0], id: "second", title: "Second source" });
    fs.writeFileSync(file, JSON.stringify(manifest));
    const declaration = path.join(folder, "contributions.json");
    const contributions = JSON.parse(fs.readFileSync(declaration));
    contributions.dataQueries.push({ ...contributions.dataQueries[0], viewId: "second", sourceId: secondSourceId });
    fs.writeFileSync(declaration, JSON.stringify(contributions));
  });
  const inspect = await api("/api/plugins/inspect", { path: reportFile });
  const installed = await api("/api/plugins/install", { packageRef: inspect.packageRef, digest: inspect.digest, approvePublisher: true });
  const consumer = { pluginId: installed.manifest.id, digest: installed.digest };
  await api("/api/plugins/state", { ...consumer, action: "enable" });
  const view = (viewId = "sessions", status = 200) =>
    api("/api/plugins/view", { pluginId: consumer.pluginId, viewId, cursor: null }, status);
  const refresh = (status = 200, viewId = "sessions") =>
    api("/api/plugins/action", { ...consumer, action: { kind: "refresh", viewId } }, status);
  const authorize = () => api("/api/plugins/action", { ...consumer, action: { kind: "authorize-report" } });
  assert.equal((await refresh(403)).code, "permission_denied");
  await authorize(); assert.equal((await refresh(400)).code, "dependency_missing");
  const contribution = (await api("/api/plugins")).contributions.items.find((p) => p.pluginId === consumer.pluginId);
  assert.equal(contribution.navigation[0].title, "Agent 会话报表");
  pass("签名导航已注册；未授权和缺少来源时跨插件调用明确拒绝");
  const sourceFile = pack("dsh-desktop");
  const review = await api("/api/plugin-sources/inspect", { path: sourceFile });
  await api("/api/plugin-sources/install", { packageRef: review.packageRef, digest: review.digest, approvePublisher: true, expectedSigningKey: review.signingKey });
  const source = { pluginId: "net.worksha.dsh-desktop", digest: review.digest };
  const assertSourceVisible = async (visible) => {
    const snapshot = await api("/api/snapshot");
    assert.equal(snapshot.agents.some((agent) => agent.id === productId), visible);
    const { catalog } = await api("/api/domain/directory");
    assert.equal(catalog.products.some((product) => product.id === productId), visible);
    assert.equal(catalog.sources.some((item) => item.id === sourceId), visible);
  };
  await assertSourceVisible(false);
  await api("/api/plugin-sources/state", { package: source, action: "enable" });
  await assertSourceVisible(true);
  await refresh(403);
  writeRows(1); await api("/api/plugin-sources/source", { package: source, path: history }); await api("/api/plugin-sources/refresh", source);
  await refresh(403);
  const consent = await api("/api/domain/consent-review", source);
  await api("/api/domain/consent", { ...source, enabled: true, persistMetadata: false, reviewRef: consent.reviewRef });
  await refresh(); let reportView = await view(); assert.equal(reportView.report.sources[0].sessions, "1"); assert.equal(reportView.report.sources[0].totalTokens, "19");
  assert.equal((await refresh(400, "second")).code, "dependency_missing");
  assert.equal((await view("second")).snapshot, null);
  assert.equal((await view()).snapshot, reportView.snapshot);
  await api("/api/plugins/action", { ...consumer, action: { kind: "refresh" } });
  reportView = await view();
  assert.equal(reportView.report.sources[0].sessions, "1");
  pass("来源缺失只影响绑定视图，旧刷新请求也能发布其他可用视图");
  const unbound = await view("unbound");
  assert.equal(unbound.report, null);
  assert.equal(unbound.confirmedEmpty, false);
  assert.equal(unbound.needsRefresh, true);
  pass("声明的其他页面不能读取仅绑定到报表页面的来源汇总");
  pass("消费者授权与来源授权独立；报表真实调用另一插件的会话能力并得到精确计数");
  const snapshot = await api("/api/snapshot"); assert.equal(snapshot.agents.length, initial.agents.length + 1); assert(snapshot.agents.some((p) => p.id === productId && p.readonly && p.sourceId === sourceId && p.capabilities.includes("domain.activity-source@1")));
  assert.deepEqual(snapshot.agents.filter((p) => !p.readonly), initial.agents);
  writeDshObservation();
  await api("/api/plugin-sources/refresh", source);
  const observation = await api("/api/domain/sessions", query);
  const sessionId = observation.items[0].record.id;
  const factQuery = { scope: query.scope, sessionId, projectId: null, responseId: null, cursor: null, snapshotToken: null };
  const executions = await api("/api/domain/facts", { ...factQuery, kind: "mcp-execution" });
  assert.equal(executions.items.length, 1);
  const execution = executions.items[0].value.value;
  assert.equal(execution.sessionRef, sessionId);
  assert.equal(execution.outcome, "success");
  assert.equal(execution.toolName, "mcp__owned__probe");
  assert.equal(execution.serverRef, null);
  assert(!JSON.stringify(executions).includes("private "));
  const catalog = await api("/api/domain/facts", { ...factQuery, kind: "mcp-catalog" });
  assert.equal(catalog.items.length, 0);
  const body = await api("/api/domain/conversation", { sessionId, snapshotToken: observation.snapshotToken, cursor: null, anchor: null });
  assert.deepEqual(body.conversation.events.map(({ kind, role }) => [kind, role]), [["message", "user"], ["runtime-context", null]]);
  writeRows(1);
  await api("/api/plugin-sources/refresh", source);
  pass("DSH 签名源发布 MCP 执行与关联会话，运行上下文独立，参数和结果保持私有");
  const secondReview = await api("/api/plugin-sources/inspect", { path: pack("full-history") });
  await api("/api/plugin-sources/install", { packageRef: secondReview.packageRef, digest: secondReview.digest, approvePublisher: true, expectedSigningKey: secondReview.signingKey });
  const secondSource = { pluginId: "org.example.full-history", digest: secondReview.digest };
  const secondHistory = path.join(root, "second.jsonl");
  fs.writeFileSync(secondHistory, rows(3, secondProductId).map((row) => JSON.stringify(row)).join("\n") + "\n");
  await api("/api/plugin-sources/state", { package: secondSource, action: "enable" });
  await api("/api/plugin-sources/source", { package: secondSource, path: secondHistory });
  await api("/api/plugin-sources/refresh", secondSource);
  const secondConsent = await api("/api/domain/consent-review", secondSource);
  await api("/api/domain/consent", { ...secondSource, enabled: true, persistMetadata: false, reviewRef: secondConsent.reviewRef });
  await refresh(200, "second");
  const secondView = await view("second");
  assert.deepEqual(secondView.report.sources.map((s) => [s.sourceId, s.sessions]), [[secondSourceId, "3"]]);
  assert.notEqual(secondView.snapshot, reportView.snapshot);
  assert.equal((await view()).snapshot, reportView.snapshot);
  const activity = () => api("/api/domain/activity", { scope: query.scope, rangeSeconds: 3600, productId, sessionId: null, includeGateway: false, includeHistorical: false });
  const current = (await activity()).items[0]; assert.equal(current.state, "current"); assert.equal(current.kind, "source-event"); assert.equal(current.measurements, null);
  writeRows(2);
  let found = false;
  for (let i = 0; i < 70; i++) { if ((await api("/api/domain/sessions", query)).items.length === 2) { found = true; break; } await new Promise((r) => setTimeout(r, 200)); }
  assert(found, "authorized source watcher did not publish the changed file");
  const frozen = await api("/api/domain/sessions", query);
  writeRows(3, true);
  const conversation = await api("/api/domain/conversation", { sessionId: frozen.items[0].record.id, snapshotToken: frozen.snapshotToken, cursor: null, anchor: null });
  assert.equal(conversation.conversation.events[0].text, "Snapshot 2");
  pass("文件下一次写入时会话正文仍来自已授权的同一完整快照");
  assert.equal((await api("/api/plugin-sources/refresh", source, 409)).code, "stale_snapshot");
  assert.equal((await api("/api/domain/sessions", query)).items.length, 2);
  writeRows(2); await api("/api/plugin-sources/refresh", source);
  await refresh(); assert.equal((await view()).report.sources[0].sessions, "2");
  assert.equal((await view("second")).snapshot, secondView.snapshot);
  pass("新 Agent 自动进入主目录；定期来源读取发布变化，原客户端目录保持");
  await api("/api/domain/consent", { ...source, enabled: false, persistMetadata: false, reviewRef: "" });
  await view("sessions", 403); await refresh(403);
  assert.equal((await view("second")).snapshot, secondView.snapshot);
  await refresh(200, "second");
  assert.notEqual((await view("second")).snapshot, secondView.snapshot);
  await view("sessions", 403);
  pass("报表快照按视图独立更新，一方来源撤权后另一方仍可读取和刷新");
  const renewed = await api("/api/domain/consent-review", source);
  await api("/api/domain/consent", { ...source, enabled: true, persistMetadata: false, reviewRef: renewed.reviewRef });
  await api("/api/plugins/view", { pluginId: consumer.pluginId, viewId: "sessions", cursor: null }, 409);
  await refresh(); assert.equal((await view()).report.sources[0].sessions, "2");
  pass("来源重新授权不会复活旧报表，需重新调用能力");
  fs.utimesSync(history, (Date.now() - 60000) / 1000, (Date.now() - 60000) / 1000);
  assert.equal((await activity()).items[0].state, "expired");
  assert.equal((await api("/api/plugin-sources")).items.find((item) => item.digest === source.digest).problem, "source_stalled");
  pass("来源停止发布后实时状态过期，保留已读数据；损坏快照不能覆盖完整结果");
  await api("/api/plugin-sources/state", { package: source, action: "disable" }); writeRows(3);
  await assertSourceVisible(false);
  assert.equal((await api("/api/plugin-sources")).items.find((item) => item.digest === source.digest).enabled, false);
  await new Promise((r) => setTimeout(r, 5500)); assert.equal((await api("/api/domain/sessions", query)).items.length, 0);
  await api("/api/plugin-sources/state", { package: source, action: "enable" });
  await assertSourceVisible(true);
  assert.equal((await api("/api/domain/sessions", query)).items.length, 0);
  await api("/api/plugin-sources/state", { package: source, action: "disable" });
  await api("/api/plugins/state", { ...consumer, action: "disable" }); await api("/api/plugins/view", { pluginId: consumer.pluginId, viewId: "sessions", cursor: null }, 400);
  pass("停用后从 Agent 和业务来源目录隐藏，插件中心仍可启用，重新启用不恢复旧授权");
  const malicious = path.join(root, "invalid"); fs.cpSync("sdk/plugins/examples/source-report/package", malicious, { recursive: true });
  const bad = JSON.parse(fs.readFileSync(path.join(malicious, "contributions.json"))); bad.navigation[0].viewId = "../../settings"; fs.writeFileSync(path.join(malicious, "contributions.json"), JSON.stringify(bad)); assert.throws(() => sdk("pack", malicious, malicious + ".laplug", key));
  bad.navigation[0].viewId = "sessions"; bad.dataQueries[0].capability = "domain.session-source@2"; fs.writeFileSync(path.join(malicious, "contributions.json"), JSON.stringify(bad)); assert.throws(() => sdk("pack", malicious, malicious + ".laplug", key));
  pass("导航越界和未知能力主版本拒绝打包");
  report.complete = true;
} finally {
  if (child?.exitCode === null) { try { await api("/api/runtime/shutdown", {}); await exited; } catch { child.kill("SIGTERM"); await exited; } }
  fs.rmSync(root, { recursive: true, force: true }); report.temporaryDataRemoved = !fs.existsSync(root);
  if (process.env.DYNAMIC_PLUGIN_REPORT) fs.writeFileSync(process.env.DYNAMIC_PLUGIN_REPORT, JSON.stringify(report, null, 2) + "\n");
}
