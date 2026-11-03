import assert from "node:assert/strict";
import {
  mkdtempSync,
  realpathSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
  readdirSync,
  statSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { randomUUID } from "node:crypto";
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { once } from "node:events";
import http from "node:http";

const directory = realpathSync(
  mkdtempSync(path.join(tmpdir(), "local-agent-observation-")),
);
const sourceHome = path.join(directory, "home");
mkdirSync(sourceHome);
const token = randomUUID();
const claudeSession = randomUUID();
const codexSession = randomUUID();
const seen = [];
let child;
let stderr = "";
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const server = http.createServer(async (req, res) => {
  let raw = "";
  for await (const chunk of req) raw += chunk;
  const body = JSON.parse(raw);
  seen.push({ path: req.url, body, headers: req.headers });
  if (body.model === "http-error") {
    res.writeHead(500, { "content-type": "application/json" });
    res.end('{"error":{"message":"fixture"}}');
    return;
  }
  if (body.model === "redirect") {
    res.writeHead(307, {
      location: `http://127.0.0.1:${server.address().port}/credential-trap`,
    });
    res.end();
    return;
  }
  const claude = req.url.endsWith("messages");
  if (!body.stream) {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(
      JSON.stringify(
        claude
          ? {
              content: [],
              model: body.model,
              usage: { input_tokens: 10, output_tokens: 5 },
            }
          : {
              object: "response",
              status: "completed",
              model: body.model,
              usage: { input_tokens: 10, output_tokens: 5 },
            },
      ),
    );
    return;
  }
  res.writeHead(200, {
    "content-type": "text/event-stream",
    "request-id": "request-fixture",
    "set-cookie": "must-not-forward=1",
  });
  res.flushHeaders();
  res.write(": heartbeat\r\n\r\n");
  const send = (value) => res.write(`data: ${JSON.stringify(value)}\n\n`);
  if (claude)
    send({
      type: "message_start",
      message: {
        model: body.model,
        usage: {
          input_tokens: 10,
          cache_read_input_tokens: 20,
          cache_creation_input_tokens: 5,
        },
      },
    });
  else send({ type: "response.created", response: { model: body.model } });
  await delay(160);
  if (res.destroyed) return;
  if (claude)
    send({
      type: "content_block_delta",
      delta: { type: "text_delta", text: "fixture-output-sentinel-314159" },
    });
  else
    send({
      type: "response.output_text.delta",
      delta: "fixture-output-sentinel-314159",
    });
  if (body.model === "truncated") {
    res.end();
    return;
  }
  if (body.model === "cancel") {
    const timer = setTimeout(() => res.end(), 15000);
    res.on("close", () => clearTimeout(timer));
    return;
  }
  await delay(650);
  if (res.destroyed) return;
  if (claude)
    send({
      type: "content_block_delta",
      delta: { type: "text_delta", text: "second-visible-chunk" },
    });
  else
    send({ type: "response.output_text.delta", delta: "second-visible-chunk" });
  if (claude) {
    send({ type: "message_delta", usage: { output_tokens: 8 } });
    send({ type: "message_stop" });
  } else
    send({
      type: "response.completed",
      response: {
        model: body.model,
        usage:
          body.model === "unknown-usage"
            ? undefined
            : {
                input_tokens: 100,
                output_tokens: 20,
                input_tokens_details: { cached_tokens: 50 },
                output_tokens_details: { reasoning_tokens: 10 },
              },
      },
    });
  res.end();
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const upstream = `http://127.0.0.1:${server.address().port}/v1`;
async function start() {
  child = spawn(
    process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
      "target/debug/local-agent-service",
    ["--data-dir", directory],
    {
      env: {
        ...process.env,
        LOCAL_AGENT_SERVICE_TOKEN: token,
        LOCAL_AGENT_SOURCE_HOME: sourceHome,
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  child.stderr.on("data", (b) => {
    stderr += b;
  });
  const lines = createInterface({ input: child.stdout });
  const ready = await Promise.race([
    once(lines, "line").then(([line]) => JSON.parse(line)),
    once(child, "exit").then(() => {
      throw Error("Service exited");
    }),
    delay(10000).then(() => {
      throw Error("Service timeout");
    }),
  ]);
  return `http://127.0.0.1:${ready.port}`;
}
async function stop() {
  if (child?.exitCode === null) {
    const done = once(child, "exit");
    child.kill();
    await done;
  }
}
let base;
const api = (route, init = {}) =>
  fetch(base + route, {
    ...init,
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
      ...init.headers,
    },
  });
const jsonApi = async (route) => {
  const res = await api(route);
  assert.equal(res.status, 200);
  return res.json();
};
const put = (route, value) =>
  api(route, { method: "PUT", body: JSON.stringify(value) });
const unifiedActivity = async (includeGateway = true) => {
  const response = await api("/api/domain/activity", {
    method: "POST",
    body: JSON.stringify({
      scope: { kind: "builtin", sourceIds: ["claude", "codex"] },
      rangeSeconds: 86400,
      productId: null,
      sessionId: null,
      includeGateway,
      includeHistorical: true,
    }),
  });
  assert.equal(response.status, 200);
  return response.json();
};
const overview = (options = {}) =>
  jsonApi(
    "/api/overview?" +
      new URLSearchParams({
        range: "24",
        metric: "calls",
        dimension: "agent",
        source: "gateway",
        ...options,
      }),
  );
const launchSecret = (binding) => {
  const script = readFileSync(binding.launcherPath, "utf8");
  const match = script.match(
    /http:\/\/127\.0\.0\.1:\d+\/capture\/(claude|codex)\/([a-f0-9]+)/,
  );
  assert.ok(match);
  assert.ok(!script.includes("fixture-api-secret"));
  assert.equal(statSync(binding.launcherPath).mode & 0o777, 0o700);
  return `${base}/capture/${match[1]}/${match[2]}/v1/${binding.agentId === "claude" ? "messages" : "responses"}`;
};
async function waitFor(fn, description) {
  for (let i = 0; i < 80; i++) {
    const result = await fn();
    if (result) return result;
    await delay(50);
  }
  throw Error(description);
}
const body = (model) => ({
  model,
  stream: true,
  input: "fixture-request-body-sentinel-271828",
});
const run = (url, model, extra = {}) =>
  fetch(url, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: "Bearer fixture-api-secret",
      "x-api-key": "fixture-api-secret",
      ...extra,
    },
    body: JSON.stringify(body(model)),
  });
try {
  base = await start();
  for (const [id, protocol] of [
    ["anthropic", "messages"],
    ["openai", "responses"],
  ])
    assert.equal(
      (
        await put("/api/providers", {
          id,
          name: id,
          endpoint: upstream,
          protocol,
          model: "fixture",
          models: ["fixture"],
          configured: false,
        })
      ).status,
      200,
    );
  for (const [agentId, providerId] of [
    ["claude", "anthropic"],
    ["codex", "openai"],
  ])
    assert.equal(
      (
        await put("/api/observation/bindings", {
          agentId,
          providerId,
          expectedRevision: 0,
          enabled: true,
        })
      ).status,
      200,
    );
  let snap = await jsonApi("/api/snapshot");
  const claudeBinding = snap.observationBindings.find(
    (b) => b.agentId === "claude",
  );
  const codexBinding = snap.observationBindings.find(
    (b) => b.agentId === "codex",
  );
  assert.equal(claudeBinding.secret, undefined);
  const claude = launchSecret(claudeBinding),
    codex = launchSecret(codexBinding);
  assert.equal(
    (
      await fetch(
        codex.replace(/capture\/codex\/[^/]+/, "capture/codex/invalid"),
        { method: "POST" },
      )
    ).status,
    401,
  );
  assert.equal(
    (await run(codex, "blocked", { Origin: "https://example.com" })).status,
    401,
  );
  assert.equal(
    (await run(codex + "?target=https://example.com", "blocked")).status,
    401,
  );
  assert.equal(
    (
      await put("/api/observation/bindings", {
        agentId: "claude",
        providerId: "openai",
        expectedRevision: 1,
        enabled: true,
      })
    ).status,
    400,
  );
  const claudePromise = run(claude, "claude-fixture", {
    "x-local-agent-session-id": claudeSession,
  }).then(async (res) => {
    assert.equal(res.headers.get("set-cookie"), null);
    return res.text();
  });
  const codexPromise = run(codex, "codex-fixture", {
    session_id: codexSession,
  }).then((res) => res.text());
  const waiting = await waitFor(async () => {
    const o = await overview();
    return o.gatewayActivities.length === 2 ? o : null;
  }, "Concurrent requests not observed");
  assert.ok(waiting.gatewayActivities.every((a) => a.request.ttftMs === null));
  const connectionSnapshot = await jsonApi("/api/snapshot");
  for (const agentId of ["claude", "codex"]) {
    const connection = connectionSnapshot.agentConnections.items.find(
      (item) => item.agentId === agentId,
    );
    assert.equal(
      connection.mode,
      "unknown",
      "launcher does not change the user's default connection",
    );
    assert.equal(
      connection.activeRequests,
      1,
      "real gateway traffic is reported independently of the default configuration",
    );
  }
  const live = await waitFor(async () => {
    const o = await overview();
    return o.gatewayActivities.every((a) => a.textTokensPerSecond > 0)
      ? o
      : null;
  }, "No real stream speed");
  assert.equal(live.gatewayActivities.length, 2);
  const domainLive = await unifiedActivity();
  const gatewayLive = domainLive.items.filter(
    (r) => r.kind === "gateway-attempt",
  );
  assert.equal(gatewayLive.length, 2);
  assert(
    gatewayLive.every(
      (r) =>
        r.state === "current" && r.measurements.liveRate?.tokensPerSecond > 0,
    ),
  );
  assert.deepEqual(
    new Set(gatewayLive.map((r) => r.id)),
    new Set(live.gatewayActivities.map((r) => r.request.id)),
  );
  assert(!JSON.stringify(domainLive).includes(upstream));
  assert(!JSON.stringify(domainLive).includes("fixture-api-secret"));
  assert(
    !JSON.stringify(domainLive).includes(
      "fixture-request-body-sentinel-271828",
    ),
  );
  assert(
    domainLive.sourceStates
      .filter((s) => s.kind === "native-message")
      .every((s) => s.state.authorization !== "authorized"),
  );
  assert(
    (await unifiedActivity(false)).items.every(
      (r) => r.kind !== "gateway-attempt",
    ),
  );
  assert.ok(live.gatewayActivities.every((a) => a.request.ttftMs >= 130));
  assert.deepEqual(
    new Set(live.gatewayActivities.map((a) => a.request.sessionId)),
    new Set([`claude:${claudeSession}`, `codex:${codexSession}`]),
  );
  assert.equal(
    (
      await put("/api/observation/bindings", {
        agentId: "codex",
        providerId: "openai",
        expectedRevision: 1,
        enabled: true,
      })
    ).status,
    400,
  );
  const [claudeOutput, codexOutput] = await Promise.all([
    claudePromise,
    codexPromise,
  ]);
  assert.ok(
    claudeOutput.includes("fixture-output-sentinel-314159") &&
      codexOutput.includes("fixture-output-sentinel-314159"),
  );
  const completed = await waitFor(async () => {
    const o = await overview();
    return o.timings.success === 2 ? o : null;
  }, "Completion not recorded");
  assert.ok(
    completed.gatewayActivities.every((a) => a.textTokensPerSecond === null),
  );
  assert.ok(
    completed.gatewayActivities.every((a) => a.averageTextTokensPerSecond > 0),
  );
  assert.ok(
    completed.gatewayActivities.every(
      (a) => a.request.lastTextMs > a.request.firstTextMs,
    ),
  );
  assert.equal(completed.timings.ttftSamples, 2);
  let records = (await jsonApi("/api/requests")).items;
  const completedActivity = await unifiedActivity();
  assert(
    completedActivity.items
      .filter((r) => r.kind === "gateway-attempt")
      .every(
        (r) => r.state === "historical" && r.measurements.liveRate === null,
      ),
  );
  for (const s of completedActivity.sourceSummaries.filter(
    (s) => s.kind === "gateway-attempt",
  )) {
    assert.equal(s.observedRecords, "1");
    assert.equal(s.logicalRequests, "1");
  }
  const modelUsage = await jsonApi(
    "/api/agent-usage?agent=claude&source=gateway",
  );
  assert.equal(modelUsage.summary.attempts, 1);
  assert.equal(modelUsage.summary.usage.totalTokens, 43);
  assert.equal(modelUsage.summary.usage.cachedInputTokens, 20);
  assert.equal(modelUsage.items[0].endpoint, upstream);
  assert.equal(modelUsage.items[0].model, "claude-fixture");
  assert.equal(
    modelUsage.items[0].requestId,
    records.find((row) => row.agentId === "claude").id,
  );
  assert.equal(
    (await jsonApi("/api/agent-usage?agent=opencode&source=gateway")).summary
      .attempts,
    0,
  );
  assert.equal(
    records.find((r) => r.agentId === "claude").usage.totalTokens,
    43,
  );
  assert.equal(
    records.find((r) => r.agentId === "codex").usage.totalTokens,
    120,
  );
  assert.ok(
    records.every(
      (r) =>
        r.durationMs >= 750 &&
        r.ttftMs < r.durationMs &&
        (r.usage.outputTokens * 1000) / r.durationMs > 0,
    ),
  );
  for (const model of [
    "truncated",
    "http-error",
    "unknown-usage",
    "redirect",
  ]) {
    const res = await run(codex, model);
    await res.text();
  }
  const controller = new AbortController();
  const cancelled = await fetch(codex, {
    method: "POST",
    signal: controller.signal,
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body("cancel")),
  });
  const reader = cancelled.body.getReader();
  await waitFor(async () => {
    const o = await overview();
    return o.gatewayActivities.some(
      (a) => a.request.model === "cancel" && a.request.ttftMs != null,
    );
  }, "Cancellation stream not observed");
  controller.abort();
  await reader.cancel().catch(() => {});
  await waitFor(
    async () =>
      (await jsonApi("/api/requests")).items.find(
        (r) => r.model === "cancel" && r.status === "cancelled",
      ),
    "Cancellation not propagated",
  );
  const nonstream = await fetch(codex, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ model: "nonstream", stream: false }),
  });
  await nonstream.text();
  for (let i = 0; i < 2; i++)
    await (
      await run(codex, "retry-fixture", {
        "idempotency-key": "same-logical-fixture",
      })
    ).text();
  const a = await overview({ metric: "tokens", dimension: "model" });
  assert.equal(a.analysis.missing, 5);
  assert.equal(
    a.analysis.agents
      .filter((v) => v.value !== null)
      .reduce((n, v) => n + v.value, 0),
    a.analysis.total,
  );
  assert.equal(
    a.analysis.models
      .filter((v) => v.value !== null)
      .reduce((n, v) => n + v.value, 0),
    a.analysis.total,
  );
  assert.equal(
    a.analysis.buckets
      .filter((v) => v.value !== null)
      .reduce((n, v) => n + v.value, 0),
    a.analysis.total,
  );
  const calls = await overview();
  const all = await jsonApi("/api/requests");
  const retryActivity = await unifiedActivity();
  const retryAttempts = retryActivity.items.filter(
    (r) => r.model === "retry-fixture",
  );
  assert.equal(retryAttempts.length, 2);
  assert.equal(new Set(retryAttempts.map((r) => r.logicalRequestId)).size, 1);
  assert(retryAttempts.every((r) => r.kind === "gateway-attempt"));
  const codexSummary = retryActivity.sourceSummaries.find(
    (s) => s.kind === "gateway-attempt" && s.sourceId === "codex",
  );
  assert.equal(
    BigInt(codexSummary.observedRecords) - BigInt(codexSummary.logicalRequests),
    1n,
  );
  assert.equal(calls.analysis.samples, all.total - 1); // two attempts share explicit ID
  const costs = await overview({ metric: "cost" });
  assert.equal(costs.analysis.total, null);
  assert.equal(costs.analysis.missing, costs.analysis.samples);
  records = all.items;
  assert.equal(
    records.find((r) => r.model === "truncated").status,
    "interrupted",
  );
  assert.equal(records.find((r) => r.model === "nonstream").ttftMs, null);
  assert.ok(!seen.some((r) => r.path === "/credential-trap"));
  assert.ok(
    seen
      .filter((r) => r.body.model.endsWith("fixture"))
      .every((r) => r.body.input === "fixture-request-body-sentinel-271828"),
  );
  assert.equal(
    (
      await put("/api/observation/bindings", {
        agentId: "claude",
        providerId: "anthropic",
        expectedRevision: 0,
        enabled: false,
      })
    ).status,
    409,
  );
  const persistedCount = all.total;
  await delay(150);
  await stop();
  base = await start();
  const restart = await jsonApi("/api/requests");
  assert.equal(restart.total, persistedCount);
  assert.ok(
    restart.items.find((r) => r.model === "claude-fixture")
      .estimatedTextTokens > 0,
  );
  assert.ok(
    restart.items.find((r) => r.model === "claude-fixture").lastTextMs >
      restart.items.find((r) => r.model === "claude-fixture").firstTextMs,
  );
  snap = await jsonApi("/api/snapshot");
  assert.ok(
    snap.agentConnections.items.every((item) => item.activeRequests === 0),
    "historical requests never imply current gateway activity",
  );
  launchSecret(snap.observationBindings.find((b) => b.agentId === "codex"));
  const observationDir = path.join(directory, "observation");
  const restoredActivity = await overview();
  assert(
    (await unifiedActivity()).items.every(
      (r) => r.state === "historical" && r.measurements.liveRate === null,
    ),
  );
  const codexActivity = restoredActivity.gatewayActivities.find(
    (activity) => activity.request.agentId === "codex" && activity.userInput,
  );
  assert.equal(
    codexActivity.userInput.text,
    "fixture-request-body-sentinel-271828",
  );
  const summaries = JSON.parse(
    readFileSync(
      path.join(observationDir, "gateway-activity-inputs.json"),
      "utf8",
    ),
  );
  assert.ok(summaries.rows.length > 0 && summaries.rows.length <= 512);
  assert.ok(
    summaries.rows.every((row) => Array.from(row.input.text).length <= 241),
  );
  assert.ok(!JSON.stringify(summaries).includes("fixture-api-secret"));
  assert.ok(
    !JSON.stringify(summaries).includes("fixture-output-sentinel-314159"),
  );
  for (const name of readdirSync(observationDir).filter((n) =>
    n.startsWith("observations.sqlite3"),
  )) {
    const bytes = readFileSync(path.join(observationDir, name));
    for (const text of [
      "fixture-api-secret",
      "fixture-request-body-sentinel-271828",
      "fixture-output-sentinel-314159",
      "second-visible-chunk",
    ])
      assert.ok(!bytes.includes(Buffer.from(text)), "Content/secret persisted");
  }
  assert.equal(stderr, "");
  console.log(
    "通过：双 Agent 流式转发、心跳排除、TTFT / 1 秒速度、原样正文、取消 / 断流 / HTTP 错误、非流式降级、明确重试去重、筛选守恒、费用未知、配置冲突、持久化及正文 / 凭据不落盘。",
  );
} finally {
  await stop();
  server.closeAllConnections();
  await new Promise((resolve) => server.close(resolve));
  rmSync(directory, { recursive: true, force: true });
}
