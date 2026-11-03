import assert from "node:assert/strict";
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { once } from "node:events";
import { randomUUID } from "node:crypto";
import { mkdtempSync, realpathSync, mkdirSync, readFileSync, writeFileSync, statSync, existsSync } from "node:fs";
import { tmpdir, networkInterfaces } from "node:os";
import path from "node:path";
import net from "node:net";
import http from "node:http";
import { createInterface } from "node:readline";
import { DatabaseSync } from "node:sqlite";

const run = promisify(execFile);
const binary = path.resolve(process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ?? "target/debug/local-agent-service");
const directory = realpathSync(mkdtempSync(path.join(tmpdir(), "local-agent-lan-")));
const home = path.join(directory, "source-home");
mkdirSync(home, { mode: 0o700 });
const evidence = path.resolve(process.env.LOCAL_AGENT_LAN_REPORT_DIR ?? ".work/lan-gateway");
mkdirSync(evidence, { recursive: true });
const env = { ...process.env, LOCAL_AGENT_SOURCE_HOME: home, LOCAL_AGENT_TEST_MEMORY_VAULT: "1", LOCAL_AGENT_TEST_IDLE_AGENTS: "1", NO_PROXY: "*" };
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const checks = [];
const recordPath = path.join(directory, "runtime/service.json");
const record = () => JSON.parse(readFileSync(recordPath, "utf8"));
let child;
let installed = false;
let stderr = "";
const holds = new Set();
const upstreamHeaders = [];
const upstream = http.createServer(async (req, res) => {
  let raw = "";
  for await (const chunk of req) raw += chunk;
  const body = JSON.parse(raw);
  upstreamHeaders.push(req.headers);
  if (body.model === "broken") { res.writeHead(503); res.end("fixture unavailable"); return; }
  const id = randomUUID();
  const answer = { id, type: "message", role: "assistant", model: body.model, content: [{ type: "text", text: "LAN_OK" }], stop_reason: "end_turn", usage: { input_tokens: 10, output_tokens: 2 } };
  if (!body.stream) { res.setHeader("content-type", "application/json"); res.end(JSON.stringify(answer)); return; }
  const event = (v) => `data: ${JSON.stringify(v)}\n\n`;
  res.writeHead(200, { "content-type": "text/event-stream" });
  res.write(event({ type: "message_start", message: { ...answer, content: [], stop_reason: null, usage: { input_tokens: 10, output_tokens: 0 } } }));
  const finish = () => { if (res.destroyed) return; res.write(event({ type: "content_block_start", index: 0, content_block: { type: "text", text: "" } })); res.write(event({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "LAN_OK" } })); res.write(event({ type: "content_block_stop", index: 0 })); res.write(event({ type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 2 } })); res.end(event({ type: "message_stop" })); holds.delete(finish); };
  if (body.model === "hold") { holds.add(finish); res.on("close", () => holds.delete(finish)); } else finish();
});
await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));
const lanIp = Object.values(networkInterfaces()).flat().find((n) => n && n.family === "IPv4" && !n.internal && /^(10\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.)/.test(n.address))?.address ?? "127.0.0.1";
const reservation = net.createServer();
await new Promise((resolve) => reservation.listen(0, lanIp, resolve));
const lanPort = reservation.address().port;
await new Promise((resolve) => reservation.close(resolve));
const lanBase = `http://${lanIp}:${lanPort}`;
async function cli(verb, extra = []) {
  const result = await run(binary, ["gateway", verb, "--data-dir", directory, ...extra], { env, timeout: 30000, maxBuffer: 1024 * 1024 });
  return JSON.parse(result.stdout);
}
async function api(route, options = {}) {
  const r = record();
  const response = await fetch(`http://127.0.0.1:${r.port}${route}`, { ...options, headers: { authorization: `Bearer ${r.token}`, "content-type": "application/json", ...options.headers }, signal: AbortSignal.timeout(15000) });
  assert.equal(response.status, 200, await response.clone().text());
  return response.json();
}
async function start() {
  stderr = "";
  child = spawn(binary, ["gateway", "serve", "--data-dir", directory], { env, stdio: ["ignore", "pipe", "pipe"] });
  child.stderr.on("data", (b) => { stderr += b; });
  const lines = createInterface({ input: child.stdout });
  await Promise.race([once(lines, "line"), once(child, "exit").then(() => { throw Error(stderr); }), delay(10000).then(() => { throw Error("startup timeout"); })]);
}
async function stop() {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const done = once(child, "exit");
  await api("/api/runtime/shutdown", { method: "POST" });
  await Promise.race([done, delay(16000).then(() => { throw Error("drain timeout"); })]);
}
const sessionId = randomUUID();
const headers = (key) => ({ "content-type": "application/json", authorization: `Bearer ${key}`, "x-local-agent-session-id": sessionId });
const body = (model = "fast", stream = true) => ({ model, stream, max_tokens: 16, messages: [{ role: "user", content: "local fixture" }] });
async function call(key, model = "fast", options = {}) { return fetch(`${lanBase}/v1/messages`, { method: "POST", headers: headers(key), body: JSON.stringify(body(model)), signal: AbortSignal.timeout(20000), ...options }); }
async function expectOk(response) { assert.equal(response.status, 200, await response.clone().text()); assert.match(await response.text(), /LAN_OK/); }
async function poll(test, message, attempts = 100) { for (let n = 0; n < attempts; n++) { if (await test()) return; await delay(50); } throw Error(message); }
try {
  await cli("configure", ["--listen", `${lanIp}:${lanPort}`, "--concurrency", "2"]);
  const blocker = net.createServer();
  await new Promise((resolve) => blocker.listen(lanPort, lanIp, resolve));
  try {
    await assert.rejects(start(), /局域网固定端口被占用/);
    assert.ok(blocker.listening, "conflict must not stop the existing listener");
    assert.equal(existsSync(recordPath), false, "failed start must not publish a usable service");
  } finally {
    await new Promise((resolve) => blocker.close(resolve));
  }
  await start();
  assert.equal(record().identity.persistent, true);
  const managementPort = record().port;
  const localPort = JSON.parse(readFileSync(path.join(directory, "gateway/listener.json"), "utf8"));
  assert.equal(new Set([managementPort, localPort, lanPort]).size, 3);
  await assert.rejects(cli("configure", ["--listen", `${lanIp}:${lanPort}`, "--concurrency", "2"]));
  checks.push("persistent identity, three separate listeners, live config overwrite refused; LAN port conflict preserves the existing listener and publishes no runtime");
  await api("/api/providers", { method: "PUT", body: JSON.stringify({ id: "anthropic", name: "LAN fixture", endpoint: `http://127.0.0.1:${upstream.address().port}/v1`, protocol: "messages", model: "fast", models: ["fast", "hold", "broken"], configured: false }) });
  const create = (name, agent = "claude") => api("/api/gateway-lan/clients", { method: "POST", body: JSON.stringify({ name, agent, providerId: "anthropic", models: ["fast", "hold", "broken"], concurrency: 1 }) });
  const a = await create("Laptop A");
  const b = await cli("client-add", ["--name", "Laptop B", "--agent", "claude", "--provider", "anthropic", "--models", "fast,hold,broken", "--concurrency", "1"]);
  const c = await create("Responses replay", "codex");
  const d = await create("OpenCode replay", "opencode");
  assert.notEqual(a.key, b.key);
  assert.equal(statSync(path.join(directory, "gateway-lan/clients.json")).mode & 0o777, 0o600);
  const saved = readFileSync(path.join(directory, "gateway-lan/clients.json"), "utf8");
  assert.ok(!saved.includes(a.key) && !saved.includes(b.key));
  assert.ok(!JSON.stringify(await cli("status")).includes(a.key));
  assert.equal((await call("bad")).status, 401);
  assert.equal((await call(record().token)).status, 401);
  assert.equal((await call(a.key, "fast", { headers: { ...headers(a.key), origin: "https://untrusted.invalid" } })).status, 401);
  const badHost = await new Promise((resolve, reject) => {
    const req = http.request(`${lanBase}/v1/models`, { headers: { ...headers(a.key), host: "localhost:1" } }, (res) => { res.resume(); resolve(res.statusCode); });
    req.on("error", reject); req.end();
  });
  assert.equal(badHost, 401);
  assert.equal((await fetch(`${lanBase}/api/snapshot`, { headers: headers(a.key) })).status, 404);
  assert.equal((await call(a.key, "not-allowed")).status, 400);
  assert.equal((await call(c.key)).status, 403);
  checks.push("independent client keys, hashed persistence, management and model planes isolated, origin/host/model/protocol restrictions");
  const catalog = await fetch(`${lanBase}/v1/models`, { headers: headers(a.key) });
  assert.equal(catalog.status, 200);
  assert.equal((await catalog.json()).data.length, 3);
  const codexCatalog = await fetch(`${lanBase}/v1/codex/models.json?client_version=0.163.0`, { headers: headers(c.key) });
  assert.equal(codexCatalog.status, 200);
  assert.equal((await codexCatalog.json()).models.length, 3);
  assert.equal((await fetch(`${lanBase}/v1/codex/models.json?client_version=0.100.0`, { headers: headers(c.key) })).status, 400);
  await expectOk(await call(a.key, "fast", { headers: { "content-type": "application/json", "x-api-key": a.key } }));
  await expectOk(await call(d.key));
  await expectOk(await call(a.key));
  const replay = await fetch(`${lanBase}/v1/responses`, { method: "POST", headers: headers(c.key), body: JSON.stringify({ model: "fast", stream: true, input: "offline protocol replay" }) });
  await expectOk(replay);
  assert.ok(upstreamHeaders.every((h) => !String(h.authorization ?? "").includes("la_")), "client keys never reach the upstream");
  checks.push("Messages and Responses streaming via real TCP fixture, restricted catalog, client secrets not forwarded");
  await api("/api/observation/bindings", { method: "PUT", body: JSON.stringify({ agentId: "claude", providerId: "anthropic", expectedRevision: 0, enabled: true }) });
  const heldA = await call(a.key, "hold");
  assert.equal(heldA.status, 200);
  assert.equal((await call(a.key)).status, 429);
  await expectOk(await call(b.key));
  const heldB = await call(b.key, "hold");
  assert.equal(heldB.status, 200);
  assert.equal((await fetch(`${lanBase}/v1/responses`, { method: "POST", headers: headers(c.key), body: JSON.stringify({ model: "fast", input: "saturated" }) })).status, 429);
  const binding = JSON.parse(readFileSync(path.join(directory, "observation/bindings.json"), "utf8")).claude;
  const localCall = (model = "fast") => fetch(`http://127.0.0.1:${managementPort}/capture/claude/${binding.secret}/v1/messages`, { method: "POST", headers: { "content-type": "application/json", "x-api-key": "LOCAL_FIXTURE" }, body: JSON.stringify(body(model)), signal: AbortSignal.timeout(20000) });
  await expectOk(await localCall());
  await cli("client-revoke", ["--id", a.id]);
  assert.equal((await call(a.key)).status, 401);
  for (const finish of [...holds]) finish();
  await Promise.all([heldA.text(), heldB.text()]);
  await poll(async () => (await cli("status")).active === 0, "LAN slots were not released");
  const localHolds = await Promise.all(Array.from({ length: 64 }, () => localCall("hold")));
  assert.ok(localHolds.every((r) => r.status === 200));
  assert.equal((await localCall()).status, 429);
  await expectOk(await call(b.key));
  for (const finish of [...holds]) finish();
  await Promise.all(localHolds.map((r) => r.text()));
  checks.push("per-client admission, LAN aggregate admission, local/remote pools isolated in both directions; revocation preserves in-flight and other clients");
  const controller = new AbortController();
  const cancel = await call(b.key, "hold", { signal: controller.signal });
  assert.equal(cancel.status, 200);
  controller.abort();
  await poll(async () => (await cli("status")).active === 0, "cancel did not release slots");
  assert.equal((await call(b.key, "broken")).status, 503);
  await expectOk(await call(b.key));
  checks.push("cancellation and upstream failure release capacity and remain observable");
  await poll(async () => (await api("/api/requests")).items.some((r) => r.client?.id === b.id && r.status === "success"), "request metadata missing");
  let rows = (await api("/api/requests")).items;
  const remote = rows.find((r) => r.client?.id === b.id && r.status === "success");
  assert.equal(remote.client.name, "Laptop B");
  assert.equal(remote.client.peer, lanIp);
  assert.ok(remote.sessionId.startsWith(`lan:${b.id}:`));
  const detail = await api(`/api/request-detail?id=${remote.id}`);
  assert.equal(detail.request.client.id, b.id);
  const dashboard = await api("/api/request-dashboard?search=Laptop%20B");
  assert.ok(dashboard.items.length > 0);
  assert.ok(dashboard.items.every((r) => r.client?.id === b.id));
  const csv = await api(`/api/request-export?snapshot=${dashboard.snapshot}`);
  assert.match(csv.csv, /局域网客户端名称/);
  assert.match(csv.csv, /Laptop B/);
  const count = rows.length;
  await stop();
  await start();
  assert.equal(JSON.parse(readFileSync(path.join(directory, "gateway/listener.json"), "utf8")), localPort);
  assert.equal((await call(a.key)).status, 401);
  await expectOk(await call(b.key));
  rows = (await api("/api/requests")).items;
  assert.ok(rows.length >= count);
  assert.equal(rows.find((r) => r.id === remote.id).client.id, b.id);
  checks.push("client attribution, peer address, session namespace, search/export, revoked/valid keys and request metadata survive restart");
  await stop();
  if (process.argv.includes("--launchd")) {
    await cli("install"); installed = true;
    const before = record();
    assert.equal(before.identity.persistent, true);
    await expectOk(await call(b.key));
    process.kill(before.identity.pid, "SIGKILL");
    await poll(async () => { try { return record().identity.pid !== before.identity.pid && (await cli("status")).persistent; } catch { return false; } }, "launchd did not restart failed service", 400);
    await expectOk(await call(b.key));
    await cli("stop");
    assert.equal(existsSync(recordPath), false);
    await cli("start");
    await expectOk(await call(b.key));
    await cli("uninstall"); installed = false;
    assert.equal(existsSync(recordPath), false);
    checks.push("actual macOS launchd install, no parent process required, crash restart, stop/start and scoped uninstall");
  }
  const persisted = new DatabaseSync(path.join(directory, "observation/observations.sqlite3"), { readOnly: true });
  try { rows = persisted.prepare("SELECT value FROM model_observations ORDER BY started_at DESC,id DESC").all().map((row) => JSON.parse(row.value)); }
  finally { persisted.close(); }
  writeFileSync(path.join(evidence, "request-records.json"), JSON.stringify(rows, null, 2) + "\n");
  writeFileSync(path.join(evidence, "integration.json"), JSON.stringify({ passed: true, directory, listen: `${lanIp}:${lanPort}`, networkPeer: lanIp, secondDevice: false, totalObservations: rows.length, remoteObservations: rows.filter((r) => r.client).length, localObservations: rows.filter((r) => !r.client).length, checks }, null, 2) + "\n");
  console.log(JSON.stringify({ passed: true, directory, listen: `${lanIp}:${lanPort}`, checks }, null, 2));
} finally {
  for (const finish of [...holds]) finish();
  if (installed) { try { await cli("uninstall"); } catch (e) { console.error(String(e)); } }
  await stop().catch(() => {});
  if (child && child.exitCode === null && child.signalCode === null) child.kill("SIGTERM");
  upstream.closeAllConnections();
  await new Promise((resolve) => upstream.close(resolve));
}
