import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { once } from "node:events";
import { writeFileSync, readFileSync, rmSync } from "node:fs";
import { createInterface } from "node:readline";
import { createAntigravityFixture } from "./fixtures/antigravity.mjs";

const { directory, home, db } = createAntigravityFixture();
const token = randomUUID();
let child;
async function start() {
  child = spawn(
    process.env.LOCAL_AGENT_ANTIGRAVITY_BINARY ??
      "target/debug/local-agent-service",
    ["--data-dir", directory],
    {
      env: {
        ...process.env,
        LOCAL_AGENT_SERVICE_TOKEN: token,
        LOCAL_AGENT_SOURCE_HOME: home,
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  child.stderr.on("data", () => {});
  const lines = createInterface({ input: child.stdout });
  const ready = await Promise.race([
    once(lines, "line").then(([line]) => JSON.parse(line)),
    once(child, "exit").then(() => {
      throw Error("Service exited before ready");
    }),
    new Promise((_, reject) => {
      const timer = setTimeout(
        () => reject(Error("Service readiness timeout")),
        10000,
      );
      timer.unref();
    }),
  ]);
  return `http://127.0.0.1:${ready.port}`;
}
async function stop() {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const exited = once(child, "exit");
  child.kill();
  await exited;
}
try {
  let base = await start();
  const request = async (route, method = "GET", body) => {
    const response = await fetch(base + route, {
      method,
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    assert.equal(response.status, 200, `${method} ${route}`);
    return response.json();
  };
  const snapshot = await request("/api/snapshot");
  assert(snapshot.agents.some((agent) => agent.id === "antigravity"));
  assert.equal(
    snapshot.sessionSources.find((source) => source.agentId === "antigravity")
      .enabled,
    false,
  );
  assert.equal((await request("/api/sessions?agent=antigravity")).total, 0);
  await request("/api/session-sources", "PUT", {
    agentId: "antigravity",
    enabled: true,
  });
  await request("/api/sessions/refresh", "POST");
  let sessions;
  for (let attempt = 0; attempt < 100; attempt++) {
    sessions = await request("/api/sessions?agent=antigravity");
    if (sessions.total === 1) break;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  assert.equal(sessions.total, 1);
  assert.equal(sessions.items[0].usage.totalTokens, 120);
  const sessionId = sessions.items[0].id;
  const conversation = await request(
    `/api/sessions/${encodeURIComponent(sessionId)}`,
  );
  assert(
    conversation.items.some((event) => event.output === "Fixture MCP output"),
  );
  assert(!JSON.stringify(conversation).includes("redact-this-secret"));
  assert(!JSON.stringify(conversation).includes("private-thinking-sentinel"));
  const mcp = await request("/api/mcp?agent=antigravity");
  assert.equal(mcp.totalCalls, 1);
  assert(
    mcp.services.some(
      (service) =>
        service.name === "fixture-mcp" && service.configuredEnabled === true,
    ),
  );
  assert(!JSON.stringify(mcp).includes("never-expose-config"));
  const usage = await request(
    "/api/agent-usage?agent=antigravity&source=native",
  );
  assert.equal(usage.summary.usage.totalTokens, 120);
  const requests = await request(
    "/api/request-dashboard?source=native&agent=antigravity",
  );
  assert.equal(requests.total, 1);
  assert.equal(requests.summary.usage.totalTokens, 120);
  const projects = await request("/api/projects?scope=all&agent=antigravity");
  assert.equal(projects.total, 1);
  assert.equal(projects.summary.usage.totalTokens, 120);
  const memory = await request("/api/agents/antigravity/memory");
  assert(memory.files.some((file) => file.name === "GEMINI.md"));
  const calls = await request("/api/calls?agent=antigravity");
  const keys = (value) => Object.keys(value).sort();
  const pick = (value, fields) =>
    Object.fromEntries(fields.filter((key) => value[key] !== undefined).map((key) => [key, value[key]]));
  const failures = {};
  for (const [name, route, headers] of [
    ["unauthorized", "/api/snapshot", {}],
    [
      "origin",
      "/api/snapshot",
      { Authorization: `Bearer ${token}`, Origin: "https://invalid.example" },
    ],
    ["filter", "/api/sessions?days=2", { Authorization: `Bearer ${token}` }],
    ["missing", "/api/sessions/missing", { Authorization: `Bearer ${token}` }],
  ]) {
    const response = await fetch(base + route, { headers });
    failures[name] = { status: response.status, body: await response.json() };
  }
  assert.deepEqual(
    Object.values(failures).map((failure) => failure.status),
    [401, 401, 400, 404],
  );
  const report = {
    schema: 1,
    semantics: {
      agents: snapshot.agents.map((agent) =>
        pick(agent, ["id", "family", "configId", "name"]),
      ),
      responseFields: {
        snapshot: keys(snapshot),
        source: keys(snapshot.sessionSources[0]),
        sessions: keys(sessions),
        session: keys(sessions.items[0]),
        conversation: keys(conversation),
        event: keys(conversation.items[0]),
        calls: keys(calls),
        call: keys(calls.items[0]),
        usage: keys(usage),
        requests: keys(requests),
        projects: keys(projects),
      },
      sessions: {
        total: sessions.total,
        coverage: sessions.coverage,
        summary: sessions.summary,
        items: sessions.items.map((item) =>
          pick(item, [
            "title",
            "titleSource",
            "agentId",
            "agentProductId",
            "agentProducts",
            "hasUnknownClient",
            "messageCount",
            "turnCount",
            "modelCalls",
            "toolCalls",
            "mcpCalls",
            "skillCalls",
            "usage",
            "partial",
          ]),
        ),
      },
      conversation: {
        total: conversation.total,
        coverage: conversation.coverage,
        items: conversation.items.map((item) =>
          pick(item, [
            "kind",
            "role",
            "name",
            "status",
            "text",
            "input",
            "output",
            "model",
            "usage",
            "rawUsage",
            "durationMs",
            "truncated",
            "sourceLine",
            "failure",
          ]),
        ),
      },
      calls: {
        total: calls.total,
        coverage: calls.coverage,
        items: calls.items.map((item) =>
          pick(item, [
            "agentId",
            "agentProductId",
            "kind",
            "name",
            "status",
            "model",
            "usage",
            "server",
            "toolName",
            "durationMs",
          ]),
        ),
      },
      mcp: {
        totalCalls: mcp.totalCalls,
        services: mcp.services.map((service) =>
          pick(service, ["name", "configuredEnabled"]),
        ).sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b))),
      },
      usage: usage.summary,
      requests: { total: requests.total, summary: requests.summary },
      projects: { total: projects.total, summary: projects.summary },
      memory: memory.files.map((file) =>
        pick(file, ["name", "kind", "status"]),
      ),
      failures,
    },
  };
  // Preserve full old serialized responses for inspection, including volatile IDs.
  // Only the explicitly selected semantic fields are compared across temp directories.
  const raw = {
    snapshot,
    sessions,
    conversation,
    calls,
    mcp,
    usage,
    requests,
    projects,
    memory,
  };
  report.raw = JSON.parse(
    JSON.stringify(raw).replaceAll(directory, "<fixture>"),
  );
  if (process.env.LOCAL_AGENT_ANTIGRAVITY_BASELINE) {
    const baseline = JSON.parse(
      readFileSync(process.env.LOCAL_AGENT_ANTIGRAVITY_BASELINE, "utf8"),
    );
    assert.deepEqual(
      report.semantics,
      baseline.semantics,
      "M0/M1 original serialized fields and semantic behavior",
    );
  }
  await stop();
  base = await start();
  assert.equal(
    (await request("/api/snapshot")).sessionSources.find(
      (source) => source.agentId === "antigravity",
    ).enabled,
    true,
  );
  await request("/api/session-sources", "PUT", {
    agentId: "antigravity",
    enabled: false,
  });
  assert.equal((await request("/api/sessions?agent=antigravity")).total, 0);
  assert.equal(
    (await request("/api/request-dashboard?source=native&agent=antigravity"))
      .total,
    0,
  );
  assert.notEqual(
    (
      await fetch(`${base}/api/sessions/${encodeURIComponent(sessionId)}`, {
        headers: { Authorization: `Bearer ${token}` },
      })
    ).status,
    200,
  );
  report.restartConsentPreserved = true;
  report.revocationVerified = true;
  if (process.env.LOCAL_AGENT_ANTIGRAVITY_REPORT)
    writeFileSync(
      process.env.LOCAL_AGENT_ANTIGRAVITY_REPORT,
      JSON.stringify(report, null, 2) + "\n",
    );
  console.log(
    "通过：Antigravity 独立服务发现、默认未授权、WAL 会话、用量、MCP、记忆、项目 / 请求对账、敏感内容隐藏、重启保留与来源撤销。",
  );
} finally {
  await stop();
  db.close();
  rmSync(directory, { recursive: true, force: true });
}
