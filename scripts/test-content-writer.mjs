import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { randomUUID, createHash } from "node:crypto";
import { once } from "node:events";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  existsSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { DatabaseSync } from "node:sqlite";

const root = realpathSync(mkdtempSync(path.join(tmpdir(), "content-writer-")));
const home = path.join(root, "home");
mkdirSync(home);
const token = randomUUID();
const binary =
  process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
  "target/debug/local-agent-service";
let child,
  base,
  db,
  binding,
  stderr = "",
  mockCalls = 0;
const mock = http.createServer(async (request, response) => {
  let raw = "";
  for await (const bytes of request) raw += bytes;
  const body = JSON.parse(raw);
  mockCalls++;
  response.writeHead(200, { "content-type": "application/json" });
  response.end(
    JSON.stringify({
      id: `msg_${body.model}`,
      type: "message",
      role: "assistant",
      model: body.model,
      content: [{ type: "text", text: "synthetic response" }],
      stop_reason: "end_turn",
      usage: { input_tokens: 10, output_tokens: 2 },
    }),
  );
});
await new Promise((resolve) => mock.listen(0, "127.0.0.1", resolve));

async function api(route, body) {
  const result = await fetch(base + route, {
    method: body === undefined ? "GET" : "PUT",
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(10000),
  });
  assert.equal(result.status, 200, await result.clone().text());
  return result.json();
}
async function start() {
  child = spawn(binary, ["--data-dir", root], {
    env: {
      ...process.env,
      LOCAL_AGENT_SOURCE_HOME: home,
      LOCAL_AGENT_SERVICE_TOKEN: token,
      LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
      LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const launched = child;
  stderr = "";
  child.stderr.on("data", (bytes) => {
    if (child === launched) stderr = (stderr + bytes).slice(-262144);
  });
  const [line] = await Promise.race([
    once(createInterface({ input: child.stdout }), "line", {
      signal: AbortSignal.timeout(15000),
    }),
    once(child, "exit").then(() => {
      throw Error(stderr);
    }),
  ]);
  base = `http://127.0.0.1:${JSON.parse(line).port}`;
}
async function stop() {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const exited = once(child, "exit");
  child.kill("SIGTERM");
  try {
    await Promise.race([
      exited,
      new Promise((_, reject) =>
        setTimeout(
          () => reject(Error("owned service did not exit")),
          20000,
        ).unref(),
      ),
    ]);
  } catch (error) {
    // A timed-out observation is not terminal. Only terminate this verified
    // live, owned fixture before removing its database directory.
    if (child.exitCode === null && child.signalCode === null) {
      process.kill(child.pid, 0);
      child.kill("SIGKILL");
      await exited;
    }
    throw error;
  }
}
const health = async () =>
  (
    await api(
      "/api/overview?range=24&metric=calls&dimension=agent&source=gateway",
    )
  ).collector.persistence;
async function waitFor(check, budget = 5000) {
  const deadline = performance.now() + budget;
  while (!(await check())) {
    assert(
      performance.now() < deadline,
      "fixture condition did not become true; do not restart a live process",
    );
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}
async function forward(model) {
  const response = await fetch(
    `${base}/capture/claude/${binding.secret}/v1/messages`,
    {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-api-key": "synthetic-key",
      },
      body: JSON.stringify({
        model,
        stream: false,
        messages: [{ role: "user", content: "synthetic user input" }],
      }),
      signal: AbortSignal.timeout(10000),
    },
  );
  assert.equal(response.status, 200, await response.clone().text());
  assert.equal((await response.json()).model, model);
}
const count = () =>
  db.prepare("SELECT count(*) AS n FROM model_observations").get().n;
const cachePath = path.join(
  root,
  "observation",
  "gateway-activity-inputs.json",
);
const cache = () =>
  existsSync(cachePath) ? readFileSync(cachePath, "utf8") : null;
try {
  await start();
  db = new DatabaseSync(path.join(root, "observation", "observations.sqlite3"));
  await api("/api/providers", {
    id: "anthropic",
    name: "fixture",
    endpoint: `http://127.0.0.1:${mock.address().port}/v1`,
    protocol: "messages",
    model: "fixture",
    models: ["fixture"],
    configured: false,
  });
  await api("/api/observation/bindings", {
    agentId: "claude",
    providerId: "anthropic",
    expectedRevision: 0,
    enabled: true,
  });
  binding = JSON.parse(
    readFileSync(path.join(root, "observation", "bindings.json"), "utf8"),
  ).claude;
  await forward("warm");
  await waitFor(async () => count() === 1 && (await health()).pending === 0);
  const epoch = db
    .prepare("SELECT value FROM observation_content_epoch WHERE id=1")
    .get().value;
  db.exec(
    "CREATE TRIGGER fail_writer BEFORE INSERT ON model_observations BEGIN SELECT RAISE(ABORT,'synthetic storage failure');END;",
  );
  await forward("queued");
  await waitFor(async () => {
    const value = await health();
    return value.pending > 0 && value.error;
  });
  const queued = (await api("/api/requests")).items.find(
    (item) => item.model === "queued",
  );
  assert(
    queued,
    "the current lifetime may show its legitimate pending observation",
  );
  db.exec(
    "BEGIN IMMEDIATE; DROP TRIGGER fail_writer; DELETE FROM model_observations;",
  );
  assert.equal(
    db
      .prepare(
        "UPDATE observation_content_epoch SET value=? WHERE id=1 AND value=?",
      )
      .run(randomUUID(), epoch).changes,
    1,
  );
  db.exec("COMMIT");
  let blocked;
  await waitFor(async () => {
    assert.equal(
      count(),
      0,
      "an old queued observation reappeared after the epoch commit",
    );
    const value = await health();
    if (value.pending > 0 && value.error?.includes("版本")) {
      blocked = value;
      return true;
    }
    return false;
  });
  // An input-only batch from "queued" may legitimately have committed before
  // the observation-row trigger failed, while the original epoch was active.
  // Once the writer reports the epoch mismatch, no further cache commit may run.
  const blockedCache = cache();
  assert.equal(
    (await api(`/api/request-detail?id=${encodeURIComponent(queued.id)}`))
      .request,
    null,
  );
  assert.equal(
    (await api("/api/requests")).items.length,
    0,
    "old in-memory pending rows cannot seed the new SQL projection",
  );
  await forward("after-rotation");
  assert.equal(count(), 0);
  assert.equal(
    cache(),
    blockedCache,
    "inputs cannot change the JSON cache after the writer reports its epoch mismatch",
  );
  assert.equal(mockCalls, 3, "storage recovery never resends upstream work");
  assert((await health()).pending >= blocked.pending);
  await stop();
  await start();
  await forward("new-owner");
  await waitFor(async () => count() === 1 && (await health()).pending === 0);
  const stored = JSON.parse(
    db.prepare("SELECT value FROM model_observations").get().value,
  );
  assert.equal(stored.model, "new-owner");
  assert.equal(mockCalls, 4);
  // An idle queue is not proof that the worker still owns the current source.
  // Exercise the real shutdown consumer without introducing a test-only API.
  stderr = "";
  db.prepare("UPDATE observation_content_epoch SET value=? WHERE id=1").run(
    randomUUID(),
  );
  const closed = once(child, "close", {
    signal: AbortSignal.timeout(20000),
  });
  const shutdown = await fetch(base + "/api/runtime/shutdown", {
    method: "POST",
    headers: { authorization: `Bearer ${token}` },
    signal: AbortSignal.timeout(10000),
  });
  assert.equal(shutdown.status, 200, await shutdown.text());
  const [code, signal] = await closed;
  assert.equal(code, 0, "owned service must finish its actual shutdown");
  assert.equal(signal, null);
  assert.match(
    stderr,
    /观测内容版本已变化/,
    "idle shutdown reported no persistence failure after the source epoch changed",
  );
  assert.equal(count(), 1);
  assert.equal(
    mockCalls,
    4,
    "checkpoint verification never resends upstream work",
  );
  const report = {
    passed: true,
    binarySha256: createHash("sha256")
      .update(readFileSync(binary))
      .digest("hex"),
    queuedRetryRejected: true,
    projectionNotRepopulated: true,
    rejectedInputsNotCommitted: true,
    forwardingKeptProgressing: true,
    newProcessBoundCurrentEpoch: true,
    idleChangedEpochNotReportedDrained: true,
    pendingBeforeStop: blocked.pending,
    fixtureUpstreamCalls: mockCalls,
    realModelCalls: 0,
    realKeychainCalls: 0,
    scope:
      "known writer epoch across failure/reopen, explicit restart and idle shutdown checkpoint; not an active cleanup coordinator or cross-file atomicity proof",
  };
  if (process.env.CONTENT_WRITER_REPORT)
    writeFileSync(
      process.env.CONTENT_WRITER_REPORT,
      JSON.stringify(report, null, 2) + "\n",
    );
  console.log(JSON.stringify(report));
} finally {
  await stop();
  db?.close();
  mock.closeAllConnections();
  await new Promise((resolve) => mock.close(resolve));
  rmSync(root, { recursive: true, force: true });
}
