// Isolated workbench subprocesses, a fake CLI, and an authenticated loopback OpenCode fixture.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { once } from "node:events";
import fs from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";

const binary = path.resolve(
  process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
    "target/debug/local-agent-service",
);
const legacy = process.env.WORKBENCH_LEGACY_BINARY;
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "workbench-p2-")),
);
const home = path.join(root, "home");
const project = path.join(root, "project");
const data = path.join(root, "data");
const bin = path.join(root, "bin");
const config = path.join(home, ".config/opencode/opencode.json");
const cliRuns = path.join(root, "cli-runs");
fs.mkdirSync(path.dirname(config), { recursive: true });
fs.mkdirSync(project);
fs.mkdirSync(bin);
fs.writeFileSync(config, JSON.stringify({ model: "fixture/model" }));
fs.writeFileSync(
  path.join(bin, "opencode"),
  `#!/bin/sh\nprintf 'spawned\\n' >> '${cliRuns}'\nexit 1\n`,
  { mode: 0o700 },
);

const processes = new Set();
async function boot(directory, executable = binary) {
  const token = randomUUID();
  const child = spawn(executable, ["--data-dir", directory], {
    env: {
      ...process.env,
      HOME: root,
      XDG_STATE_HOME: path.join(home, "state"),
      PATH: `${bin}:${process.env.PATH}`,
      LOCAL_AGENT_SOURCE_HOME: home,
      LOCAL_AGENT_SERVICE_TOKEN: token,
      LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
      LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  processes.add(child);
  let errors = "";
  child.stderr.on("data", (bytes) => {
    errors += bytes;
  });
  const reader = createInterface({ input: child.stdout });
  const [line] = await Promise.race([
    once(reader, "line", { signal: AbortSignal.timeout(15_000) }),
    once(child, "exit").then(() => {
      throw new Error(errors);
    }),
  ]);
  reader.close();
  const base = `http://127.0.0.1:${JSON.parse(line).port}/api/workbench`;
  const call = async (endpoint, body, status = 200) => {
    const response = await fetch(base + endpoint, {
      method: body === undefined ? "GET" : "POST",
      headers: {
        authorization: `Bearer ${token}`,
        "content-type": "application/json",
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(15_000),
    });
    const value = await response.json();
    assert.equal(
      response.status,
      status,
      `${endpoint}: ${JSON.stringify(value)}`,
    );
    return value;
  };
  return { child, call };
}

async function stop(child) {
  processes.delete(child);
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exit = once(child, "exit");
  child.kill();
  const kill = setTimeout(() => child.kill("SIGKILL"), 8000);
  try {
    await exit;
  } finally {
    clearTimeout(kill);
  }
}

const original = {
  taskId: randomUUID(),
  directory: project,
  route: "fixture/model",
  agent: "opencode",
};
const op = (kind, operationId, { storeId }) =>
  "/operation?" + new URLSearchParams({ kind, operationId, storeId });
const password = randomUUID();
const native = new Map();
let nativeCreates = 0;
let nativePrompts = 0;
let dropCreateReply = true;
let dropPromptReply = true;
let proveMessage = true;
const readyStreams = new Set();
const openCode = createServer(async (req, res) => {
  if (
    req.headers.authorization !==
    "Basic " + Buffer.from("opencode:" + password).toString("base64")
  ) {
    res.writeHead(401).end();
    return;
  }
  const reply = (value) =>
    res
      .writeHead(200, { "content-type": "application/json" })
      .end(JSON.stringify({ data: value }));
  try {
    if (req.url.startsWith("/api/event?")) {
      res.writeHead(200, { "content-type": "text/event-stream" });
      res.write('data: {"type":"server.connected"}\n\n');
      readyStreams.add(res);
      res.on("close", () => readyStreams.delete(res));
      return;
    }
    if (req.url === "/api/model")
      return reply([{ providerID: "fixture", id: "model", enabled: true }]);
    if (req.url === "/api/model/default")
      return reply({ providerID: "fixture", id: "model" });
    if (req.url === "/api/provider") return reply([{ id: "fixture" }]);
    if (req.url === "/api/provider/fixture") return reply({ id: "fixture" });
    if (req.url === "/api/session/active") return reply({});
    let body = "";
    for await (const chunk of req) body += chunk;
    if (req.url === "/api/session" && req.method === "POST") {
      const input = JSON.parse(body);
      nativeCreates++;
      const session = { ...input, time: { created: 1, updated: 2 } };
      native.set(input.id, { session, messages: [] });
      if (dropCreateReply) {
        dropCreateReply = false;
        res.destroy();
        return;
      }
      return reply(session);
    }
    const match =
      /^\/api\/session\/(ses_[a-f0-9]+)(?:\/(message|prompt|permission))?$/.exec(
        req.url,
      );
    if (!match || !native.has(match[1])) {
      res.writeHead(404).end();
      return;
    }
    const record = native.get(match[1]);
    if (!match[2]) return reply(record.session);
    if (match[2] === "message") return reply(record.messages);
    if (match[2] === "permission") return reply([]);
    nativePrompts++;
    if (proveMessage)
      record.messages.push({ ...JSON.parse(body), type: "user" });
    if (dropPromptReply) {
      dropPromptReply = false;
      res.destroy();
      return;
    }
    return reply({});
  } catch {
    res.writeHead(500).end();
  }
});
const passed = (message) => console.log(message);

try {
  let saved;
  if (legacy) {
    const { child, call } = await boot(data, legacy);
    saved = await call("/tasks", original);
    await stop(child);
  }
  let service = await boot(data);
  let call = service.call;
  const scope = (await call("/routes?agent=opencode")).operationScope;
  const created = await call("/tasks", { ...original, scope });
  assert.equal(created.id, original.taskId);
  assert.equal(created.runtimeBinding.productId, "opencode");
  assert.equal(created.runtimeBinding.implementationVersion, 1);
  if (saved) {
    assert.equal(created.revision, saved.revision);
    assert.equal(created.directory, saved.directory);
    assert.equal(created.route, saved.route);
    const backup = fs
      .readdirSync(path.join(data, "workbench"))
      .find((file) => file.startsWith("tasks.pre-v1."));
    assert(backup);
    const restore = path.join(root, "restore");
    fs.mkdirSync(path.join(restore, "workbench"), {
      recursive: true,
      mode: 0o700,
    });
    fs.copyFileSync(
      path.join(data, "workbench", backup),
      path.join(restore, "workbench/tasks.sqlite3"),
    );
    const restored = await boot(restore);
    const copy = (await restored.call(`/tasks?taskId=${original.taskId}`)).task;
    assert.equal(copy.id, saved.id);
    assert.equal(copy.route, saved.route);
    assert.equal(copy.directory, saved.directory);
    assert.deepEqual(copy.parts, saved.parts);
    assert.notEqual(copy.operationScope.storeId, scope.storeId);
    await stop(restored.child);
    passed(
      "An actual P1 task opened in P2 with the original ID, route, directory and revision. The migration backup restored the same task in a separate store.",
    );
  }
  for (let i = 0; i < 100; i++) {
    assert.equal(
      (await call("/tasks", { ...original, scope })).revision,
      created.revision,
    );
  }
  assert.equal((await call("/tasks")).tasks.length, 1);
  await call("/tasks", { ...original, route: "fixture/different", scope }, 409);
  assert.equal(
    (await call(op("create", original.taskId, scope))).receipt.accepted,
    true,
  );

  const message = {
    taskId: original.taskId,
    turnId: randomUUID(),
    text: "owned fixture",
    scope,
    expectedRevision: created.revision,
  };
  await call(
    "/send",
    { ...message, expectedRevision: created.revision + 1 },
    409,
  );
  assert.equal((await call(op("send", message.turnId, scope))).receipt, null);
  await Promise.all(Array.from({ length: 100 }, () => call("/send", message)));
  await call("/send", { ...message, text: "changed" }, 409);
  const accepted = (await call(op("send", message.turnId, scope))).receipt;
  assert.equal(accepted.committedRevision, created.revision + 1);
  assert.equal(
    (await call(`/tasks?taskId=${original.taskId}`)).task.parts.filter(
      (part) => part.id === message.turnId,
    ).length,
    1,
  );
  for (let i = 0; !fs.existsSync(cliRuns) && i < 200; i++)
    await new Promise((done) => setTimeout(done, 25));
  assert(
    fs.existsSync(cliRuns),
    JSON.stringify((await call(`/tasks?taskId=${original.taskId}`)).task),
  );
  await stop(service.child);
  assert.equal(fs.readFileSync(cliRuns, "utf8"), "spawned\n");
  passed(
    "100 exact creates and 100 concurrent sends each admitted once. Changed payloads and stale task revisions were rejected.",
  );

  service = await boot(data);
  call = service.call;
  assert.deepEqual(
    (await call(op("send", message.turnId, scope))).receipt,
    accepted,
  );
  await call("/send", { ...message, turnId: randomUUID() }, 409);
  const other = await boot(path.join(root, "other-data"));
  await other.call("/tasks", { ...original, scope }, 409);
  await other.call(op("send", message.turnId, scope), undefined, 409);
  assert.equal((await other.call("/tasks")).tasks.length, 0);
  await stop(other.child);
  passed(
    "Read-only recovery found the original committed result after restart. Stale authorization and another database accepted no write.",
  );

  openCode.listen(0, "127.0.0.1");
  await once(openCode, "listening");
  const state = path.join(home, "state/opencode");
  fs.mkdirSync(state, { recursive: true });
  fs.writeFileSync(
    path.join(state, "service.json"),
    JSON.stringify({
      url: `http://127.0.0.1:${openCode.address().port}`,
      pid: process.pid,
      password,
      version: "2.0.0",
    }),
    { mode: 0o600 },
  );
  const route = await call("/shared-routes");
  assert.equal(route.error, null);
  const shared = {
    taskId: randomUUID(),
    directory: project,
    route: "fixture/model",
    text: "first message",
    scope: route.operationScope,
    connectionId: route.connectionId,
  };
  assert.equal((await call("/shared", shared, 503)).outcomeUnknown, true);
  assert.equal(
    (await call(op("sharedCreate", shared.taskId, shared.scope))).receipt
      .accepted,
    false,
  );
  assert.equal((await call("/shared", shared)).delivered, false);
  const recovered = await call("/shared", shared);
  assert.equal(recovered.delivered, true);
  assert.equal((await call("/shared", shared)).sessionId, recovered.sessionId);
  assert.equal(
    (await call(op("sharedCreate", shared.taskId, shared.scope))).receipt
      .accepted,
    true,
  );
  await call("/shared", { ...shared, text: "different" }, 409);
  assert.equal(nativeCreates, 1);
  assert.equal(nativePrompts, 1);
  passed(
    "Shared OpenCode reconciled lost create and first-prompt replies against the original native IDs with one request each.",
  );

  await call("/live", {
    kind: "select",
    connectionId: route.connectionId,
    sessionId: recovered.sessionId,
  });
  for (let i = 0; !readyStreams.size && i < 100; i++)
    await new Promise((done) => setTimeout(done, 20));
  assert(readyStreams.size > 0);
  await new Promise((done) => setTimeout(done, 150));
  const live = {
    kind: "send",
    connectionId: route.connectionId,
    sessionId: recovered.sessionId,
    text: "live message",
    messageId: randomUUID(),
    scope: route.operationScope,
  };
  dropPromptReply = true;
  await call("/live", live, 503);
  assert.equal(
    (await call(op("liveSend", live.messageId, live.scope))).receipt.accepted,
    false,
  );
  assert.equal((await call("/live", live)).accepted, true);
  assert.equal(
    (await call(op("liveSend", live.messageId, live.scope))).receipt.accepted,
    true,
  );
  await call("/live", { ...live, text: "changed" }, 409);
  dropPromptReply = true;
  proveMessage = false;
  const unknown = { ...live, messageId: randomUUID() };
  await call("/live", unknown, 503);
  await call("/live", unknown, 503);
  assert.equal(nativePrompts, 3);
  passed(
    "Live OpenCode rechecked a lost reply from history. An unproven reply stayed pending without a second prompt.",
  );
} finally {
  for (const child of processes) await stop(child);
  if (openCode.listening) {
    openCode.closeAllConnections();
    openCode.close();
  }
  fs.rmSync(root, { recursive: true, force: true });
}
