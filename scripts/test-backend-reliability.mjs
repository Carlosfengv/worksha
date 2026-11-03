import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { once } from "node:events";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
} from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { DatabaseSync } from "node:sqlite";

const directory = realpathSync(
  mkdtempSync(path.join(tmpdir(), "backend-reliability-")),
);
const home = path.join(directory, "home");
mkdirSync(home);
mkdirSync(path.join(directory, "observation"));
const databasePath = path.join(
  directory,
  "observation",
  "observations.sqlite3",
);
const db = new DatabaseSync(databasePath);
db.exec(
  "CREATE TABLE model_observations(id TEXT PRIMARY KEY,started_at TEXT NOT NULL,revision INTEGER NOT NULL,value TEXT NOT NULL)",
);
db.prepare("INSERT INTO model_observations VALUES(?,?,?,?)").run(
  "damaged",
  new Date().toISOString(),
  1,
  "{damaged",
);
const token = randomUUID();
let child;
let base;
let stderr = "";
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const event = (value) => `data: ${JSON.stringify(value)}\n\n`;
const server = http.createServer(async (req, res) => {
  let raw = "";
  for await (const chunk of req) raw += chunk;
  const body = JSON.parse(raw);
  if (body.model === "invalid-json") {
    res.writeHead(200, { "content-type": "application/json" });
    res.end("{not-json");
    return;
  }
  if (body.model === "json-error") {
    res.writeHead(200, { "content-type": "application/json" });
    res.end('{"error":{"type":"server_error","message":"fixture"}}');
    return;
  }
  res.writeHead(200, { "content-type": "text/event-stream" });
  res.write(
    event({
      type: "message_start",
      message: {
        id: `msg_${body.model}`,
        model: body.model,
        usage: { input_tokens: 10 },
      },
    }),
  );
  res.write(
    event({
      type: "content_block_delta",
      delta: { type: "text_delta", text: "PRIVATE_RESPONSE_SENTINEL" },
    }),
  );
  if (body.model === "late-error") {
    res.end(
      'event: error\ndata: {"type":"error","error":{"type":"server_error"}}',
    );
    return;
  }
  if (body.model === "truncated") {
    res.end();
    return;
  }
  res.write(
    event({
      type: "message_delta",
      delta: {
        stop_reason: body.model === "max-tokens" ? "max_tokens" : "end_turn",
      },
      usage: { output_tokens: 8 },
    }),
  );
  res.end(event({ type: "message_stop" }));
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const api = async (route, options = {}) => {
  const response = await fetch(base + route, {
    ...options,
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
  });
  assert.equal(response.status, 200, await response.clone().text());
  return response.json();
};
async function start() {
  child = spawn("target/debug/local-agent-service", ["--data-dir", directory], {
    env: {
      ...process.env,
      LOCAL_AGENT_SERVICE_TOKEN: token,
      LOCAL_AGENT_SOURCE_HOME: home,
      LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stderr.on("data", (bytes) => {
    stderr += bytes;
  });
  const lines = createInterface({ input: child.stdout });
  const [line] = await Promise.race([
    once(lines, "line"),
    once(child, "exit").then(() => {
      throw Error(stderr);
    }),
    new Promise((_, reject) =>
      setTimeout(() => reject(Error("service startup timeout")), 10000).unref(),
    ),
  ]);
  base = `http://127.0.0.1:${JSON.parse(line).port}`;
}
async function stop() {
  if (!child || child.exitCode !== null) return;
  const exited = once(child, "exit");
  await api("/api/runtime/shutdown", { method: "POST" });
  await Promise.race([
    exited,
    new Promise((_, reject) =>
      setTimeout(() => reject(Error("service drain timeout")), 16000).unref(),
    ),
  ]);
}
try {
  await start();
  await api("/api/providers", {
    method: "PUT",
    body: JSON.stringify({
      id: "anthropic",
      name: "fixture",
      endpoint: `http://127.0.0.1:${server.address().port}/v1`,
      protocol: "messages",
      model: "fixture",
      models: ["fixture"],
      configured: false,
    }),
  });
  await api("/api/observation/bindings", {
    method: "PUT",
    body: JSON.stringify({
      agentId: "claude",
      providerId: "anthropic",
      expectedRevision: 0,
      enabled: true,
    }),
  });
  const binding = JSON.parse(
    readFileSync(path.join(directory, "observation", "bindings.json"), "utf8"),
  ).claude;
  const expected = {
    "max-tokens": "incomplete",
    "invalid-json": "interrupted",
    "json-error": "error",
    truncated: "interrupted",
    "late-error": "error",
    normal: "success",
  };
  for (const model of Object.keys(expected)) {
    const response = await fetch(
      `${base}/capture/claude/${binding.secret}/v1/messages`,
      {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "x-api-key": "PRIVATE_KEY_SENTINEL",
        },
        body: JSON.stringify({
          model,
          stream: !["invalid-json", "json-error"].includes(model),
          messages: [{ role: "user", content: "PRIVATE_REQUEST_SENTINEL" }],
        }),
      },
    );
    assert.equal(response.status, 200);
    const text = await response.text();
    if (model === "invalid-json")
      assert.equal(
        text,
        "{not-json",
        "pass-through preserves the upstream body",
      );
  }
  let rows;
  for (let n = 0; n < 100; n++) {
    rows = (await api("/api/requests")).items;
    if (rows.length === 6 && rows.every((r) => r.status !== "in_progress"))
      break;
    await delay(20);
  }
  assert.equal(rows.length, 6);
  for (const [model, status] of Object.entries(expected))
    assert.equal(rows.find((r) => r.model === model)?.status, status, model);
  assert.equal(
    rows.find((r) => r.model === "max-tokens").stopReason,
    "max_tokens",
  );
  const overview = await api(
    "/api/overview?range=24&metric=calls&dimension=agent&source=gateway",
  );
  assert.equal(overview.analysis.samples, 6);
  assert.match(overview.collector.persistenceError, /1.*历史观测/);
  assert.ok(overview.collector.persistence);
  await stop();
  const committed = db
    .prepare("SELECT value FROM model_observations WHERE id<>'damaged'")
    .all()
    .map((r) => JSON.parse(r.value));
  assert.equal(committed.length, 6);
  assert.ok(committed.every((r) => r.status !== "in_progress"));
  assert.equal(
    db.prepare("SELECT value FROM model_observations WHERE id='damaged'").get()
      .value,
    "{damaged",
  );
  assert.ok(
    !JSON.stringify(committed).includes("PRIVATE_"),
    "bodies and credentials never enter observation rows",
  );
  await start();
  const reloaded = (await api("/api/requests")).items;
  assert.equal(reloaded.length, 6);
  for (const [model, status] of Object.entries(expected))
    assert.equal(reloaded.find((r) => r.model === model)?.status, status);
  await stop();
  console.log(
    "通过：坏历史隔离启动、HTTP 200 协议错误与截断、末尾无换行错误、原样转发、终态提交后退出、重启一致及正文 / 凭据不落盘。",
  );
} finally {
  if (child?.exitCode === null) {
    const exited = once(child, "exit");
    child.kill("SIGKILL");
    await exited;
  }
  await new Promise((resolve) => server.close(resolve));
  db.close();
  rmSync(directory, { recursive: true, force: true });
}
