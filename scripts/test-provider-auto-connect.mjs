import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import http from "node:http";
import { randomUUID } from "node:crypto";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";

const directory = fs.realpathSync(
  fs.mkdtempSync(path.join(os.tmpdir(), "local-agent-auto-connect-")),
);
const home = path.join(directory, "home");
for (const folder of [".claude", ".config/opencode"])
  fs.mkdirSync(path.join(home, folder), { recursive: true });
const files = {
  claude: path.join(home, ".claude/settings.json"),
  opencode: path.join(home, ".config/opencode/opencode.json"),
};
for (const file of Object.values(files))
  fs.writeFileSync(file, "{}\n", { mode: 0o600 });
const originals = Object.fromEntries(
  Object.entries(files).map(([id, file]) => [
    id,
    fs.readFileSync(file, "utf8"),
  ]),
);
const token = randomUUID();
const key = "disposable-auto-fixture-key";
const seen = [];
let trapRequests = 0,
  child,
  stderr = "",
  arrive,
  release;
const trap = http.createServer((req, res) => {
  trapRequests++;
  res.end("unexpected");
});
await new Promise((resolve) => trap.listen(0, "127.0.0.1", resolve));
const upstream = http.createServer(async (req, res) => {
  let raw = "";
  for await (const bytes of req) raw += bytes;
  const parts = req.url.split("/").filter(Boolean);
  const kind = parts[0];
  const body = raw ? JSON.parse(raw) : null;
  seen.push({
    kind,
    method: req.method,
    path: req.url,
    body,
    hasKey: req.headers.authorization === `Bearer ${key}`,
    hasApiKey: req.headers["x-api-key"] === key,
  });
  const json = (status, value) => {
    res.writeHead(status, { "content-type": "application/json" });
    res.end(JSON.stringify(value));
  };
  if (kind === "redirect") {
    res.writeHead(302, {
      location: `http://127.0.0.1:${trap.address().port}/key-trap`,
    });
    res.end();
    return;
  }
  if (kind === "auth") {
    json(401, { error: { message: "not echoed upstream detail" } });
    return;
  }
  if (kind === "missing" && req.method === "GET") {
    json(404, {});
    return;
  }
  if (req.method === "GET") {
    if (!req.url.endsWith("/models")) {
      json(404, {});
      return;
    }
    if (kind === "header" && req.headers["x-api-key"] !== key) {
      json(401, {});
      return;
    }
    // Root-version repair must stay under the user's prefix.
    if (kind === "base" && !req.url.endsWith("/v1/models")) {
      json(404, {});
      return;
    }
    json(200, {
      data: [
        { id: "fixture-model", name: "Fixture model", context_window: 8000 },
      ],
    });
    return;
  }
  const protocol = req.url.endsWith("/chat/completions")
    ? "chat-completions"
    : req.url.endsWith("/responses")
      ? "responses"
      : req.url.endsWith("/messages")
        ? "messages"
        : "unknown";
  if (kind === "rate") {
    json(429, {});
    return;
  }
  if (kind === "outage") {
    json(503, {});
    return;
  }
  if (kind === "shape") {
    json(200, {
      id: "false-positive",
      model: "fixture-model",
      message: "not a completion",
    });
    return;
  }
  if (kind === "html") {
    res.writeHead(200, { "content-type": "text/html" });
    res.end("<html>Login</html>");
    return;
  }
  if (kind === "race") {
    arrive();
    await new Promise((resolve) => {
      release = resolve;
    });
  }
  const expected = {
    chat: "chat-completions",
    base: "chat-completions",
    missing: "chat-completions",
    race: "chat-completions",
    responses: "responses",
    messages: "messages",
    flat: "messages",
    header: "messages",
  }[kind];
  if (
    protocol !== expected ||
    (kind === "base" && !req.url.startsWith("/base/v1/"))
  ) {
    json(404, {});
    return;
  }
  if (expected === "messages" && req.headers["x-api-key"] !== key) {
    json(401, {});
    return;
  }
  if (kind === "flat" && req.url !== "/flat/messages") {
    json(404, {});
    return;
  }
  if (expected === "messages")
    json(200, {
      id: randomUUID(),
      type: "message",
      role: "assistant",
      model: body.model,
      content: [{ type: "text", text: "OK" }],
      stop_reason: "end_turn",
      usage: { input_tokens: 5, output_tokens: 2 },
    });
  else if (expected === "responses")
    json(200, {
      id: randomUUID(),
      object: "response",
      status: "completed",
      model: body.model,
      output: [
        {
          type: "message",
          id: randomUUID(),
          role: "assistant",
          status: "completed",
          content: [{ type: "output_text", text: "OK", annotations: [] }],
        },
      ],
      usage: { input_tokens: 5, output_tokens: 2, total_tokens: 7 },
    });
  else
    json(200, {
      id: randomUUID(),
      object: "chat.completion",
      model: body.model,
      choices: [
        {
          index: 0,
          message: { role: "assistant", content: "OK" },
          finish_reason: "stop",
        },
      ],
      usage: { prompt_tokens: 5, completion_tokens: 2, total_tokens: 7 },
    });
});
await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));
const origin = `http://127.0.0.1:${upstream.address().port}`;
const checks = [];
try {
  child = spawn(
    process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
      "target/debug/local-agent-service",
    ["--data-dir", path.join(directory, "app")],
    {
      env: {
        ...process.env,
        LOCAL_AGENT_SERVICE_TOKEN: token,
        LOCAL_AGENT_SOURCE_HOME: home,
        LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
        LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  child.stderr.on("data", (bytes) => (stderr += bytes));
  const ready = await Promise.race([
    once(createInterface({ input: child.stdout }), "line").then(([line]) =>
      JSON.parse(line),
    ),
    once(child, "exit").then(() => {
      throw Error("Isolated service exited before ready.");
    }),
  ]);
  const base = `http://127.0.0.1:${ready.port}`;
  const api = async (
    route,
    body,
    expected = 200,
    method = body === undefined ? "GET" : "POST",
  ) => {
    const response = await fetch(base + route, {
      method,
      headers: {
        authorization: `Bearer ${token}`,
        "content-type": "application/json",
      },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      signal: AbortSignal.timeout(55000),
    });
    const value = await response.json();
    assert.equal(response.status, expected, JSON.stringify(value));
    assert(!JSON.stringify(value).includes(key));
    return value;
  };
  const credential = await api("/api/provider-credentials", {
    label: "Disposable fixture",
    secret: key,
  });
  const draft = (url, model = "") => ({
    id: `custom-${randomUUID()}`,
    name: "",
    vendor: "custom",
    endpoint: url,
    protocol: "responses",
    model,
    models: [],
    disabledModels: [],
    configured: false,
    ...credential,
    revision: 0,
    category: "custom",
  });
  const connected = [];
  for (const [url, protocol, canonical] of [
    ["/chat/v1/chat/completions", "chat-completions", "/chat/v1"],
    ["/responses/v1/responses", "responses", "/responses/v1"],
    ["/messages/v1/messages", "messages", "/messages/v1"],
    ["/flat/messages", "messages", "/flat/messages"],
    ["/base/", "chat-completions", "/base/v1"],
    ["/header/v1", "messages", "/header/v1"],
    ["/chat/v1/models", "chat-completions", "/chat/v1"],
  ]) {
    const input = draft(origin + url);
    const result = await api("/api/provider-connect", input);
    assert.equal(result.provider.protocol, protocol);
    assert.equal(result.provider.endpoint, origin + canonical);
    assert.equal(result.provider.model, "fixture-model");
    assert.equal(result.provider.name, "127.0.0.1");
    assert.equal(result.provider.revision, 1);
    assert.equal(result.verification.state, "verified");
    assert.equal(result.catalog.models.length, 1);
    assert.equal(
      (await api("/api/provider-status")).verifications.find(
        (v) => v.providerId === input.id,
      ).state,
      "verified",
    );
    const count = seen.length;
    await api("/api/provider-connect", input, 409);
    assert.equal(
      seen.length,
      count,
      "stale revisions must fail before external requests",
    );
    connected.push(result.provider);
    checks.push(`connected ${url} as ${protocol}`);
  }
  const denied = await api(
    "/api/provider-connect",
    draft(origin + "/auth"),
    400,
  );
  assert.equal(denied.code, "connection_auth");
  const noModel = draft(origin + "/missing");
  assert.equal(
    (await api("/api/provider-connect", noModel, 400)).code,
    "needs_model",
  );
  assert(!seen.some((r) => r.kind === "missing" && r.method === "POST"));
  const manual = await api("/api/provider-connect", {
    ...noModel,
    model: "fixture-model",
  });
  assert.equal(manual.provider.protocol, "chat-completions");
  assert.equal(manual.catalog, null);
  checks.push(
    "missing directory requires only a model ID; format is still automatic",
  );
  for (const [kind, code] of [
    ["shape", "connection_unknown"],
    ["html", "connection_unknown"],
    ["rate", "probe_rate_limit"],
    ["outage", "probe_upstream"],
    ["redirect", "catalog_redirect"],
  ]) {
    const input = draft(origin + "/" + kind);
    assert.equal((await api("/api/provider-connect", input, 400)).code, code);
    assert(
      !(await api("/api/snapshot")).providers.some((p) => p.id === input.id),
    );
    const posts = seen.filter((r) => r.kind === kind && r.method === "POST");
    assert(posts.length <= 6);
    if (kind === "rate" || kind === "outage") assert.equal(posts.length, 1);
    checks.push(`${kind} remains unconnected without retries beyond bounds`);
  }
  assert.equal(trapRequests, 0);
  const before = seen.length;
  await api("/api/provider-connect", draft(origin + "/" + key), 400);
  await api(
    "/api/provider-connect",
    draft("https://user:pass@example.invalid"),
    400,
  );
  assert.equal(seen.length, before);
  assert.equal(
    (
      await fetch(base + "/api/provider-connect", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(draft(origin + "/chat")),
      })
    ).status,
    401,
  );
  assert.equal(
    (
      await fetch(base + "/api/provider-connect", {
        method: "POST",
        headers: {
          authorization: `Bearer ${token}`,
          origin: "https://attacker.invalid",
          "content-type": "application/json",
        },
        body: JSON.stringify(draft(origin + "/chat")),
      })
    ).status,
    401,
  );
  checks.push(
    "no key in URL, no redirects, admin auth and Origin checks retained",
  );
  const racing = draft(origin + "/race", "fixture-model");
  racing.name = "Original";
  const arrived = new Promise((resolve) => {
    arrive = resolve;
  });
  const pending = api("/api/provider-connect", racing, 409);
  await arrived;
  await api(
    "/api/providers",
    { ...racing, name: "Changed during detection" },
    200,
    "PUT",
  );
  release();
  await pending;
  assert.equal(
    (await api("/api/snapshot")).providers.find((p) => p.id === racing.id).name,
    "Changed during detection",
  );
  checks.push("concurrent edits are never overwritten by a delayed detection");
  assert(
    Object.entries(files).every(
      ([id, file]) => fs.readFileSync(file, "utf8") === originals[id],
    ),
  );
  // Exercise the detected formats through the existing gateway, using disposable client copies only.
  const sources = connected.slice(0, 3);
  const models = sources.map((p, index) => ({
    id: randomUUID(),
    name: `Auto ${index}`,
    providerId: p.id,
    model: p.model,
    capabilities: {
      contextWindow: 8000,
      maxOutputTokens: 128,
      tools: false,
      thinking: false,
      inputModalities: ["text"],
    },
  }));
  const service = await api(
    "/api/model-services",
    {
      id: randomUUID(),
      revision: 0,
      name: "Auto connected fixture",
      defaultModelId: models[0].id,
      models,
    },
    200,
    "PUT",
  );
  for (const agentId of ["opencode"]) {
    const config = (await api("/api/provider-status")).configs.find(
      (c) => c.agentId === agentId,
    );
    const connection = await api("/api/model-service-connections", {
      agentId,
      expectedRevision: config.revision,
      serviceId: service.id,
      expectedServiceRevision: service.revision,
    });
    await api(`/api/provider-switches/${connection.id}/apply`, {});
    const installed = JSON.parse(fs.readFileSync(files[agentId]));
    const native =
      agentId === "claude"
        ? {
            url: installed.env.ANTHROPIC_BASE_URL + "/v1/messages",
            key: installed.env.ANTHROPIC_AUTH_TOKEN,
          }
        : {
            url:
              installed.provider[installed.model.split("/")[0]].options
                .baseURL + "/messages",
            key: installed.provider[installed.model.split("/")[0]].options
              .apiKey,
          };
    for (let i = 0; i < models.length; i++) {
      const response = await fetch(native.url, {
        method: "POST",
        headers: {
          "x-api-key": native.key,
          "anthropic-version": "2023-06-01",
          "x-opencode-session-id": `fixture-${agentId}-${i}`,
          "content-type": "application/json",
        },
        body: JSON.stringify({
          model: `local-agent-model-${models[i].id}`,
          max_tokens: 16,
          messages: [{ role: "user", content: "Reply OK." }],
          stream: false,
        }),
      });
      const value = await response.json();
      assert.equal(response.status, 200, JSON.stringify(value));
      assert.equal(value.content[0].text, "OK");
    }
  }
  checks.push(
    "OpenCode Messages gateway entrypoint uses all three automatically selected upstream formats (offline HTTP fixtures)",
  );
  console.log(
    JSON.stringify({
      passed: true,
      checks: checks.length,
      details: checks,
      realProviderCalls: 0,
      noCodexInference: true,
      disposableSourcesOnly: true,
      trapRequests,
    }),
  );
} finally {
  if (release) release();
  if (child && child.exitCode === null) {
    child.kill("SIGTERM");
    await once(child, "exit");
  }
  await new Promise((resolve) => upstream.close(resolve));
  await new Promise((resolve) => trap.close(resolve));
  fs.rmSync(directory, { recursive: true, force: true });
}
