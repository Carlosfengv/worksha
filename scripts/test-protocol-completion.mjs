import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { once } from "node:events";
import { createInterface } from "node:readline";
import http from "node:http";
import net from "node:net";

// Real existing Wasm, isolated data, MemoryVault and synthetic upstream only.
const binary = path.resolve(
  process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
    "target/debug/local-agent-service",
);
const reportPath = process.env.PROTOCOL_COMPLETION_REPORT ?? ".work/protocol-completion.json";
fs.mkdirSync(path.dirname(reportPath), { recursive: true });
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "local-agent-protocol-completion-")),
);
const home = path.join(root, "home"),
  data = path.join(root, "data");
fs.mkdirSync(home);
const env = {
  ...process.env,
  LOCAL_AGENT_SOURCE_HOME: home,
  LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
  LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
};
const report = {
  complete: false,
  binary,
  binarySha256: createHash("sha256")
    .update(fs.readFileSync(binary))
    .digest("hex"),
  harnessSha256: createHash("sha256")
    .update(fs.readFileSync(new URL(import.meta.url)))
    .digest("hex"),
  wasmSha256: createHash("sha256")
    .update(
      fs.readFileSync(
        "sdk/plugins/examples/envelope-protocol/package/plugin.wasm",
      ),
    )
    .digest("hex"),
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
};
const reservation = net.createServer();
await new Promise((resolve) => reservation.listen(0, "127.0.0.1", resolve));
const port = reservation.address().port;
await new Promise((resolve) => reservation.close(resolve));
let truncated = false,
  posts = 0,
  child,
  base,
  token;
const reply = {
  id: "owned-response",
  object: "response",
  status: "completed",
  model: "fixture",
  output: [
    {
      type: "message",
      id: "owned-message",
      role: "assistant",
      content: [{ type: "output_text", text: "OFFLINE_你好", annotations: [] }],
    },
  ],
  usage: { input_tokens: 10, output_tokens: 3, total_tokens: 13 },
};
const packet = (value) =>
  JSON.stringify({ payload: `data: ${JSON.stringify(value)}\n\n` }) + "\n";
const upstream = http.createServer(async (req, res) => {
  for await (const ignored of req) {
    void ignored;
  }
  assert.equal(req.url, "/v1/envelope");
  assert.equal(req.headers.authorization, "Bearer fake-completion-key");
  posts++;
  res.writeHead(200, { "content-type": "application/x-ndjson" });
  res.end(
    packet({
      type: "response.output_text.delta",
      output_index: 0,
      delta: "OFFLINE_你好",
    }) +
      packet({ type: "response.completed", response: reply }) +
      (truncated ? '{"payload":"unfinished' : ""),
  );
});
await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));
async function api(route, body, method = body === undefined ? "GET" : "POST") {
  const r = await fetch(base + route, {
    method,
    headers: {
      authorization: "Bearer " + token,
      "content-type": "application/json",
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(15000),
  });
  const value = await r.json();
  assert.equal(r.status, 200, route + JSON.stringify(value));
  return value;
}
async function stop() {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const done = once(child, "exit");
  child.kill("SIGINT");
  const timer = setTimeout(() => child.kill("SIGKILL"), 5000);
  await done.finally(() => clearTimeout(timer));
}
try {
  execFileSync(
    binary,
    [
      "gateway",
      "configure",
      "--data-dir",
      data,
      "--listen",
      `127.0.0.1:${port}`,
      "--concurrency",
      "4",
    ],
    { env, stdio: "pipe" },
  );
  token = randomUUID();
  child = spawn(binary, ["gateway", "serve", "--data-dir", data], {
    env: { ...env, LOCAL_AGENT_SERVICE_TOKEN: token },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stderr.resume();
  const lines = createInterface({ input: child.stdout });
  const [ready] = await Promise.race([
    once(lines, "line", { signal: AbortSignal.timeout(20000) }),
    once(child, "exit").then(() => {
      throw Error("owned service exited");
    }),
  ]);
  base = "http://127.0.0.1:" + JSON.parse(ready).port;
  token = JSON.parse(
    fs.readFileSync(path.join(data, "runtime/service.json")),
  ).token;
  const sdk = (...args) =>
    execFileSync(binary, ["--plugin-sdk", ...args], { env, stdio: "pipe" });
  const key = path.join(root, "publisher.key"),
    file = path.join(root, "protocol.laplug");
  sdk("keygen", key);
  sdk(
    "pack",
    path.resolve("sdk/plugins/examples/envelope-protocol/package"),
    file,
    key,
  );
  const inspected = await api("/api/plugins/inspect", { path: file });
  let plugin = await api("/api/plugins/install", {
    packageRef: inspected.packageRef,
    digest: inspected.digest,
    approvePublisher: true,
  });
  const ref = { pluginId: plugin.manifest.id, digest: plugin.digest };
  plugin = await api("/api/plugins/state", { ...ref, action: "enable" });
  void plugin;
  const credential = await api("/api/provider-credentials", {
    label: "Owned completion fixture",
    secret: "fake-completion-key",
  });
  let provider = (await api("/api/snapshot")).providers.find(
    (p) => p.id === "openai",
  );
  await api(
    "/api/providers",
    {
      ...provider,
      ...credential,
      endpoint: `http://127.0.0.1:${upstream.address().port}/v1`,
      model: "fixture",
      models: ["fixture"],
      configured: true,
    },
    "PUT",
  );
  provider = (await api("/api/snapshot")).providers.find(
    (p) => p.id === "openai",
  );
  await api("/api/plugin-extensions/action", {
    ...ref,
    action: {
      kind: "bind-protocol",
      providerId: provider.id,
      allowPrivateNetwork: true,
    },
  });
  for (const agent of ["codex", "claude"]) {
    const client = await api("/api/gateway-lan/clients", {
      name: "owned-" + agent,
      agent,
      providerId: provider.id,
      models: ["fixture"],
      concurrency: 2,
    });
    for (const broken of [false, true]) {
      truncated = broken;
      const before = posts;
      const knownIds = new Set(
        (await api("/api/requests")).items.map((row) => row.id),
      );
      const body =
        agent === "codex"
          ? { model: "fixture", stream: true, input: "synthetic only" }
          : {
              model: "fixture",
              stream: true,
              max_tokens: 30,
              messages: [{ role: "user", content: "synthetic only" }],
            };
      const response = await fetch(
        `http://127.0.0.1:${port}/v1/${agent === "codex" ? "responses" : "messages"}`,
        {
          method: "POST",
          headers: {
            authorization: "Bearer " + client.key,
            "content-type": "application/json",
          },
          body: JSON.stringify(body),
          signal: AbortSignal.timeout(20000),
        },
      );
      assert.equal(response.status, 200);
      let text = "",
        readError = false;
      const decoder = new TextDecoder();
      const reader = response.body.getReader();
      try {
        for (;;) {
          const part = await reader.read();
          if (part.done) break;
          text += decoder.decode(part.value, { stream: true });
        }
      } catch {
        readError = true;
      }
      text += decoder.decode();
      assert.equal(posts - before, 1, "No second model send");
      const completed = text.includes(
        agent === "codex" ? "event: response.completed" : "event: message_stop",
      );
      // Responses pass-through does not add event: lines; retain that exact path.
      const terminal =
        completed ||
        (agent === "codex" && text.includes('"type":"response.completed"'));
      assert.equal(
        terminal,
        !broken,
        JSON.stringify({ agent, broken, terminal, readError, text }),
      );
      if (!broken) assert(!readError && text.includes("OFFLINE_你好"));
      let observed;
      for (let n = 0; n < 100; n++) {
        observed = (await api("/api/requests")).items
          .filter(
            (row) => row.client?.id === client.id && !knownIds.has(row.id),
          )
          .sort((a, b) => b.startedAt.localeCompare(a.startedAt))[0];
        if (observed && !["running", "pending"].includes(observed.status))
          break;
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
      assert(observed);
      if (broken) assert.notEqual(observed.status, "success");
      else assert.equal(observed.status, "success");
      report.cases.push({
        agent,
        broken,
        terminal,
        readError,
        observedStatus: observed.status,
        upstreamPosts: posts - before,
      });
    }
  }
  report.complete = true;
  report.syntheticModelRequests = posts;
} finally {
  await stop();
  upstream.closeAllConnections();
  await new Promise((resolve) => upstream.close(resolve));
  report.ownedServiceStopped =
    !child || child.exitCode !== null || child.signalCode !== null;
  fs.rmSync(root, { recursive: true, force: true });
  report.temporaryDataRemoved = !fs.existsSync(root);
  fs.writeFileSync(reportPath, JSON.stringify(report, null, 2) + "\n");
}
console.log(
  JSON.stringify({
    complete: report.complete,
    cases: report.cases.length,
    syntheticModelRequests: report.syntheticModelRequests,
  }),
);
