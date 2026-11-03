import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import {
  mkdtempSync,
  realpathSync,
  rmSync,
  mkdirSync,
  writeFileSync,
  readFileSync,
  readdirSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { randomUUID } from "node:crypto";
import { createInterface } from "node:readline";
import { once } from "node:events";
import http from "node:http";
import { DatabaseSync } from "node:sqlite";

const directory = realpathSync(
  mkdtempSync(path.join(tmpdir(), "local-agent-service-test-")),
);
const token = randomUUID();
const sourceHome = path.join(directory, "source-home");
const sourceRoot = path.join(sourceHome, ".claude", "projects", "project");
mkdirSync(sourceRoot, { recursive: true });
const nativeFile = path.join(sourceRoot, "source-test.jsonl");
const fixtureStart = Date.now() - 3000;
const memoryFile = path.join(sourceHome, ".claude", "CLAUDE.md");
writeFileSync(memoryFile, "memory-sentinel-42\napi_key=abcdefghijklmnop\n");
const fixture =
  [
    {
      type: "user",
      sessionId: "native-fixture",
      uuid: "user-1",
      promptId: "turn-1",
      cwd: "/projects/service-test",
      version: "2.1.286",
      timestamp: new Date(fixtureStart).toISOString(),
      message: { content: "native-message-sentinel-42" },
    },
    {
      type: "assistant",
      sessionId: "native-fixture",
      uuid: "assistant-1",
      timestamp: new Date(fixtureStart + 1000).toISOString(),
      message: {
        id: "message-1",
        model: "claude-test",
        content: [
          { type: "text", text: "native-reply-sentinel-42" },
          {
            type: "tool_use",
            id: "call-1",
            name: "mcp__test__read",
            input: { query: "fixture" },
          },
        ],
        usage: { input_tokens: 10, output_tokens: 3 },
      },
    },
    {
      type: "user",
      sessionId: "native-fixture",
      uuid: "result-1",
      timestamp: new Date(fixtureStart + 2000).toISOString(),
      message: {
        content: [
          {
            type: "tool_result",
            tool_use_id: "call-1",
            content: "native-output-sentinel-42",
            is_error: true,
          },
        ],
      },
    },
  ]
    .map((row) => JSON.stringify(row))
    .join("\n") + "\n";
writeFileSync(nativeFile, fixture);
const opencodeRoot = path.join(sourceHome, ".local/share/opencode");
writeFileSync(
  path.join(sourceHome, ".claude.json"),
  JSON.stringify({
    mcpServers: {
      test: {
        type: "http",
        url: "https://private-config-sentinel",
        headers: { Authorization: "mcp-secret-sentinel" },
      },
    },
  }),
);
mkdirSync(path.join(sourceHome, ".codex"), { recursive: true });
writeFileSync(
  path.join(sourceHome, ".codex/config.toml"),
  "[mcp_servers.codex_config_only]\ncommand='never-execute-config-only'\nenabled=false\n",
);
mkdirSync(opencodeRoot, { recursive: true });
const opencode = new DatabaseSync(path.join(opencodeRoot, "opencode.db"));
opencode.exec(`PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
  CREATE TABLE session(id TEXT PRIMARY KEY,title TEXT,directory TEXT,version TEXT,time_created INTEGER,time_updated INTEGER);
  CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,time_created INTEGER,time_updated INTEGER,data TEXT);
  CREATE TABLE part(id TEXT PRIMARY KEY,message_id TEXT,session_id TEXT,time_created INTEGER,time_updated INTEGER,data TEXT);
  INSERT INTO session VALUES('ses_fixture','OpenCode SQLite fixture','/projects/opencode-test','1.18.15',1000,3000);`);
const opencodeProject = path.join(sourceHome, "opencode-project");
mkdirSync(path.join(opencodeRoot, "log"), { recursive: true });
writeFileSync(
  path.join(opencodeRoot, "log/opencode.log"),
  'timestamp=2026-10-18T15:00:00Z level=ERROR message="stream error" providerID=privatemode-ai modelID=glm-flash-latest session.id=ses_fixture error.error="AI_APICallError: Cannot connect to API: AggregateError apiKey=diagnostic-private-sentinel"\n',
);
mkdirSync(opencodeProject, { recursive: true });
writeFileSync(
  path.join(opencodeProject, "opencode.jsonc"),
  `{
  // configuration association; do not start this command
  "mcp": {"fixture":{"type":"local","command":["never-execute-mcp-sentinel"],"environment":{"KEY":"config-secret-sentinel"}}},
}`,
);
opencode.prepare("UPDATE session SET directory=?").run(opencodeProject);
opencode
  .prepare("INSERT INTO message VALUES(?, 'ses_fixture', ?, ?, ?)")
  .run("msg_user", 1000, 1000, JSON.stringify({ role: "user" }));
opencode.prepare("INSERT INTO message VALUES(?, 'ses_fixture', ?, ?, ?)").run(
  "msg_reply",
  2000,
  2000,
  JSON.stringify({
    role: "assistant",
    parentID: "msg_user",
    modelID: "fixture-model",
    providerID: "fixture-provider",
    tokens: {
      input: 10,
      output: 3,
      reasoning: 0,
      cache: { read: 0, write: 0 },
    },
  }),
);
const addPart = opencode.prepare(
  "INSERT INTO part VALUES(?,?,'ses_fixture',?,?,?)",
);
addPart.run(
  "prt_user",
  "msg_user",
  1000,
  1000,
  JSON.stringify({ type: "text", text: "opencode-private-body-sentinel-42" }),
);
addPart.run(
  "prt_tool",
  "msg_reply",
  2100,
  2100,
  JSON.stringify({
    type: "tool",
    tool: "fixture_read",
    callID: "fixture-call",
    state: {
      status: "completed",
      input: { query: "safe" },
      output: "opencode-tool-output-sentinel-42",
      time: { start: 2100, end: 2300 },
    },
  }),
);
let child;
async function start() {
  child = spawn(
    process.env.LOCAL_AGENT_SERVICE_TEST_BINARY ??
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
  const lines = createInterface({ input: child.stdout });
  const ready = await Promise.race([
    once(lines, "line").then(([line]) => JSON.parse(line)),
    once(child, "exit").then(() => {
      throw new Error("Service exited before ready");
    }),
    new Promise((_, reject) => {
      const timer = setTimeout(
        () => reject(new Error("Service readiness timeout")),
        8000,
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
  const request = (route, options = {}) =>
    fetch(base + route, {
      ...options,
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
        ...options.headers,
      },
    });
  assert.equal((await fetch(base + "/api/snapshot")).status, 401);
  assert.equal((await fetch(base + "/api/mcp")).status, 401);
  assert.equal(
    (await fetch(base + "/api/provider-usage?source=gateway")).status,
    401,
  );
  assert.equal((await request("/api/provider-usage?source=all")).status, 400);
  const providerUsageDisabled = await (
    await request("/api/provider-usage?source=native")
  ).json();
  assert.equal(providerUsageDisabled.sourceState, "disabled");
  assert.equal(providerUsageDisabled.summary.calls, 0);
  const mcpInitial = await (await request("/api/mcp")).json();
  assert.ok(
    mcpInitial.services.every(
      (s) => s.callCount === null && s.connectionStatus === "unknown",
    ),
  );
  assert.equal(
    (
      await request("/api/snapshot", {
        headers: { Origin: "https://example.com" },
      })
    ).status,
    401,
  );
  const badHostStatus = await new Promise((resolve, reject) => {
    const request = http.get(
      base + "/api/snapshot",
      {
        headers: { Host: "attacker.invalid", Authorization: `Bearer ${token}` },
      },
      (response) => {
        response.resume();
        resolve(response.statusCode);
      },
    );
    request.on("error", reject);
  });
  assert.equal(badHostStatus, 401);
  let data = await (await request("/api/snapshot")).json();
  assert.equal(data.gatewayStatus, "not_configured");
  assert.equal(data.providers.length, 7);
  assert.ok(data.agents.every((agent) => agent.connected === false));
  assert.deepEqual(
    data.agents.map((agent) => agent.id),
    [
      "claude-code-cli",
      "claude-code-desktop",
      "codex-cli",
      "codex-desktop",
      "opencode",
      "antigravity",
    ],
  );
  assert.deepEqual(
    data.agents
      .filter((agent) => agent.family === "codex")
      .map((agent) => agent.configId),
    ["codex", "codex"],
  );
  assert.ok(data.sessionSources.every((source) => !source.enabled));
  assert.equal(data.agentConnections.error, null);
  assert.equal(data.agentConnections.items.length, 5);
  assert.equal(
    data.agentConnections.items.find((c) => c.agentId === "antigravity").mode,
    "unknown",
  );
  assert.equal(
    data.agentConnections.items.find((item) => item.agentId === "codex").mode,
    "official_login",
  );
  assert.equal(
    data.agentConnections.items.find((item) => item.agentId === "claude").mode,
    "unknown",
  );
  assert.ok(
    data.agentConnections.items.every((item) => item.activeRequests === 0),
  );
  assert.equal((await fetch(base + "/api/sessions")).status, 401);
  assert.equal(
    (await fetch(base + "/api/agents/claude-code-cli/memory")).status,
    401,
  );
  assert.equal(
    (await request("/api/agents/claude-code-cli/memory")).status,
    403,
  );
  assert.equal((await (await request("/api/sessions")).json()).total, 0);
  assert.equal(
    (
      await request("/api/session-sources", {
        method: "PUT",
        body: JSON.stringify({ agentId: "claude", enabled: true }),
      })
    ).status,
    200,
  );
  const waitForIndex = async (agent = "claude") => {
    for (let attempt = 0; attempt < 80; attempt++) {
      const response = await (
        await request("/api/sessions?agent=" + agent)
      ).json();
      if (response.total === 1) return response;
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    throw new Error("Native session index timeout");
  };
  let sessions = await waitForIndex();
  assert.equal(sessions.items[0].title, "native-message-sentinel-42");
  assert.equal(sessions.summary.usage.totalTokens, 13);
  const sessionId = sessions.items[0].id;
  assert.equal(
    (await fetch(base + "/api/agent-usage?agent=claude&source=native")).status,
    401,
  );
  assert.equal(
    (await request("/api/agent-usage?agent=not-an-agent&source=native")).status,
    400,
  );
  const nativeUsage = await (
    await request("/api/agent-usage?agent=claude&source=native")
  ).json();
  assert.equal(nativeUsage.summary.attempts, 1);
  assert.equal(nativeUsage.summary.usage.totalTokens, 13);
  for (const product of ["claude-code-cli", "claude-code-desktop"]) {
    const page = await (await request(`/api/sessions?agent=${product}`)).json();
    assert.equal(
      page.total,
      0,
      "a legacy family record is not implicitly CLI or desktop",
    );
    const usage = await (
      await request(`/api/agent-usage?agent=${product}&source=native`)
    ).json();
    assert.equal(usage.summary.attempts, 0);
  }
  const unknownUsage = await (
    await request("/api/agent-usage?agent=claude-unknown&source=native")
  ).json();
  assert.equal(
    unknownUsage.summary.usage.totalTokens,
    13,
    "unknown usage is retained once",
  );
  const unknownSessions = await (
    await request("/api/sessions?agent=unknown")
  ).json();
  assert.equal(unknownSessions.total, 1);
  assert.equal(nativeUsage.items[0].model, "claude-test");
  assert.equal(nativeUsage.items[0].provider, null);
  assert.equal(nativeUsage.items[0].sessionId, sessionId);
  assert.equal(
    nativeUsage.items[0].endpoint,
    null,
    "current routing is not applied to historical native usage",
  );
  const conversation = await (
    await request("/api/sessions/" + encodeURIComponent(sessionId))
  ).json();
  const mcp = conversation.items.find((event) => event.kind === "mcp");
  assert.equal(conversation.context.inputTokens, 10);
  assert.equal(conversation.context.windowTokens, null);
  assert.equal(conversation.context.usedPercentage, null);
  let overviewContext;
  for (let attempt = 0; attempt < 40; attempt++) {
    const overview = await (
      await request(
        "/api/overview?range=24&metric=calls&dimension=agent&source=native&agent=claude",
      )
    ).json();
    overviewContext = overview.sessionContexts.find(
      (s) => s.sessionId === sessionId,
    );
    assert(
      !JSON.stringify(overview.sessionContexts).includes(
        "native-output-sentinel-42",
      ),
    );
    if (overviewContext) break;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  assert.equal(overviewContext?.context?.inputTokens, 10);
  assert.equal(overviewContext.context.windowTokens, null);
  assert.equal(overviewContext.context.usedPercentage, null);
  assert.equal(overviewContext.context.sampleAgentProductId, null);
  const memories = await (
    await request("/api/agents/claude-code-cli/memory")
  ).json();
  assert.equal(memories.selected, null);
  assert.equal(memories.files.length, 1);
  assert.equal(memories.sharedAgents.length, 2);
  const memory = await (
    await request(
      `/api/agents/claude-code-desktop/memory?fileId=${memories.files[0].id}`,
    )
  ).json();
  assert.match(memory.selected.text, /memory-sentinel-42/);
  assert.ok(!memory.selected.text.includes("abcdefghijklmnop"));
  assert.match(readFileSync(memoryFile, "utf8"), /abcdefghijklmnop/);
  assert.equal(
    (await request("/api/agents/claude-code-cli/memory?fileId=unknown")).status,
    404,
  );
  assert.equal(mcp.output, "native-output-sentinel-42");
  assert.equal(mcp.status, "error");
  const calls = await (
    await request("/api/calls?agent=claude&kind=mcp")
  ).json();
  assert.equal(calls.total, 1);
  assert.equal(calls.items[0].id, mcp.id);
  assert.equal(
    data.sessionSources.find((s) => s.agentId === "opencode").enabled,
    false,
  );
  assert.equal(
    (await (await request("/api/sessions?agent=opencode")).json()).total,
    0,
  );
  assert.equal(
    (
      await request("/api/session-sources", {
        method: "PUT",
        body: JSON.stringify({ agentId: "opencode", enabled: true }),
      })
    ).status,
    200,
  );
  const opencodePage = await waitForIndex("opencode");
  assert.equal(opencodePage.items[0].title, "OpenCode SQLite fixture");
  assert.equal(opencodePage.items[0].diskScope, "native_database_payload");
  assert.equal(opencodePage.items[0].diskAllocatedBytes, null);
  assert.equal(opencodePage.summary.usage.totalTokens, 13);
  assert.equal(opencodePage.items[0].modelErrorCount, 1);
  const opencodeId = opencodePage.items[0].id;
  const opencodeConversation = await (
    await request("/api/sessions/" + encodeURIComponent(opencodeId))
  ).json();
  const opencodeCall = opencodeConversation.items.find((e) => e.kind === "mcp");
  const diagnostic = opencodeConversation.items.find(
    (e) => e.kind === "diagnostic",
  );
  assert.equal(diagnostic.failure.category, "connection");
  assert.equal(diagnostic.failure.source, "opencode_log");
  assert.equal(diagnostic.failure.httpStatus, null);
  assert.ok(diagnostic.turnId.endsWith(":turn:unknown"));
  assert.ok(diagnostic.failure.message.includes("Cannot connect to API"));
  assert.ok(
    !JSON.stringify(opencodeConversation).includes(
      "diagnostic-private-sentinel",
    ),
  );
  const diagnosticCalls = await (
    await request("/api/calls?agent=opencode&kind=diagnostic")
  ).json();
  assert.equal(diagnosticCalls.total, 1);
  assert.equal(diagnosticCalls.items[0].id, diagnostic.id);
  assert.equal(opencodePage.items[0].latestFailureId, diagnostic.id);
  assert.equal(opencodeCall.output, "opencode-tool-output-sentinel-42");
  assert.equal(opencodeCall.durationMs, 200);
  assert.equal(opencodeCall.server, "fixture");
  assert.equal(opencodeCall.toolSource.status, "configuration");
  assert.equal(opencodeCall.toolSource.toolName, "read");
  assert.equal(opencodeCall.toolSource.toolNameQuality, "normalized");
  assert.equal(opencodeCall.toolSource.sources[0].scope, "project");
  assert.equal(
    opencodeCall.toolSource.sources[0].path,
    path.join(opencodeProject, "opencode.jsonc"),
  );
  assert.ok(
    !JSON.stringify(opencodeConversation).includes("config-secret-sentinel"),
  );
  assert.ok(
    !JSON.stringify(opencodeConversation).includes(
      "never-execute-mcp-sentinel",
    ),
  );
  assert.equal(
    (await (await request("/api/calls?agent=opencode&kind=mcp")).json())
      .items[0].id,
    opencodeCall.id,
  );
  const mcpAll = await (await request("/api/mcp")).json();
  const mcpService = mcpAll.services.find(
    (s) => s.agentId === "opencode" && s.name === "fixture",
  );
  assert.ok(mcpService);
  assert.equal(mcpService.callCount, 1);
  assert.equal(mcpService.p95Ms, 200);
  assert.equal(mcpService.errorRate, 0);
  assert.equal(mcpService.toolCatalogCount, null);
  assert.equal(mcpService.connectionStatus, "unknown");
  const mcpGroup = mcpAll.groups.find((g) => g.name === "fixture");
  assert.equal(mcpGroup.callCount, 1);
  assert.equal(mcpGroup.relatedTokens, 13);
  assert.equal(mcpGroup.relatedResponses, 1);
  assert.equal(mcpGroup.tokenCoveredCalls, 1);
  assert.equal(mcpGroup.tokenMissingCalls, 0);
  const groupDetail = await (
    await request(`/api/mcp?service=${mcpGroup.id}`)
  ).json();
  assert.equal(groupDetail.totalCalls, 1);
  const modelFacet = groupDetail.facets.models.find(
    (f) => f.label === "fixture-model",
  );
  const projectFacet = groupDetail.facets.projects.find(
    (f) => f.label === opencodeProject,
  );
  assert.ok(modelFacet);
  assert.ok(projectFacet);
  const combinedFilter = await (
    await request(
      `/api/mcp?service=${mcpGroup.id}&agent=opencode&model=${modelFacet.id}&project=${projectFacet.id}`,
    )
  ).json();
  assert.equal(combinedFilter.totalCalls, 1);
  assert.equal(combinedFilter.calls[0].call.model, "fixture-model");
  assert.equal(combinedFilter.calls[0].relatedUsage.totalTokens, 13);
  assert.equal((await request("/api/mcp?project=invalid")).status, 400);
  const claudeMcp = mcpAll.services.find(
    (s) => s.agentId === "claude" && s.name === "test",
  );
  assert.equal(claudeMcp.errorCount, 1);
  assert.equal(claudeMcp.p95Ms, null);
  assert.equal(claudeMcp.errorRate, 1);
  assert.equal(
    mcpAll.services.find((s) => s.agentId === "codex").callCount,
    null,
  );
  const selectedMcp = await (
    await request(`/api/mcp?agent=opencode&service=${mcpService.id}`)
  ).json();
  assert.equal(selectedMcp.totalCalls, 1);
  assert.equal(selectedMcp.calls[0].call.id, opencodeCall.id);
  assert.equal(selectedMcp.calls[0].call.sessionId, opencodeId);
  const anchoredMcp = await (
    await request(
      `/api/sessions/${encodeURIComponent(opencodeId)}?anchor=${encodeURIComponent(selectedMcp.calls[0].call.id)}`,
    )
  ).json();
  assert.ok(anchoredMcp.items.some((e) => e.id === opencodeCall.id));
  const detailEvent = anchoredMcp.items.find((e) => e.id === opencodeCall.id);
  assert.equal(detailEvent.input, opencodeCall.input);
  assert.equal(detailEvent.output, "opencode-tool-output-sentinel-42");
  assert.equal(detailEvent.durationMs, selectedMcp.calls[0].call.durationMs);
  const mcpLookup = await (
    await request(
      `/api/mcp?agent=opencode&call=${encodeURIComponent(opencodeCall.id)}`,
    )
  ).json();
  assert.equal(mcpLookup.selectedServiceId, mcpService.id);
  assert.equal(mcpLookup.calls[0].call.id, opencodeCall.id);
  assert.equal(
    mcpLookup.services.find((s) => s.id === mcpService.id).p95Ms,
    200,
  );
  assert.equal(
    (
      await request(
        `/api/mcp?agent=claude&call=${encodeURIComponent(opencodeCall.id)}`,
      )
    ).status,
    404,
  );
  assert.equal((await request("/api/mcp?call=")).status, 400);
  assert.equal(
    (
      await (
        await request(`/api/mcp?agent=opencode&days=7&service=${mcpService.id}`)
      ).json()
    ).totalCalls,
    0,
  );
  assert.equal((await request("/api/mcp?agent=bad")).status, 400);
  for (const secret of [
    "mcp-secret-sentinel",
    "private-config-sentinel",
    "never-execute-config-only",
    "config-secret-sentinel",
    "opencode-tool-output-sentinel-42",
  ]) {
    assert.ok(!JSON.stringify(mcpAll).includes(secret));
  }
  assert.equal((await request("/api/sessions/does-not-exist")).status, 404);
  assert.equal((await request("/api/sessions?agent=not-an-agent")).status, 400);
  const settings = { captureContent: false, retentionDays: 7 };
  const settingsUpdate = {
    storeId: data.settingsWriteProtocol.storeId,
    value: settings,
    expectedRevision: data.settingsRevision,
    operationId: randomUUID(),
  };
  assert.equal(
    (
      await request("/api/settings", {
        method: "PUT",
        body: JSON.stringify(settingsUpdate),
      })
    ).status,
    200,
  );
  assert.equal(
    (
      await request("/api/settings", {
        method: "PUT",
        body: JSON.stringify({ ...settingsUpdate, operationId: randomUUID(), value: { ...settings, retentionDays: 1 } }),
      })
    ).status,
    400,
  );
  const staleSettings = await request("/api/settings", {
    method: "PUT", body: JSON.stringify({ ...settingsUpdate, operationId: randomUUID() }),
  });
  assert.equal(staleSettings.status, 409);
  assert.equal((await staleSettings.json()).code, "conflict");
  const retrySettings = await request("/api/settings", { method: "PUT", body: JSON.stringify(settingsUpdate) });
  assert.equal(retrySettings.status, 200);
  const savedSettings = await retrySettings.json();
  assert.equal(savedSettings.operationId, settingsUpdate.operationId);
  assert.equal(savedSettings.revision, settingsUpdate.expectedRevision + 1);
  assert.deepEqual(savedSettings.value, settings);
  assert.equal(savedSettings.storeId, settingsUpdate.storeId);
  assert.deepEqual(await (await request(`/api/settings/scoped/operations/${settingsUpdate.operationId}?storeId=${settingsUpdate.storeId}`)).json(), savedSettings);
  const wrongStore = await request(`/api/settings/scoped/operations/${settingsUpdate.operationId}?storeId=${randomUUID()}`);
  assert.equal(wrongStore.status, 409);
  assert.equal((await wrongStore.json()).code, "operation_store_changed");
  const unscopedSettings = {...settingsUpdate}; delete unscopedSettings.storeId;
  const unscopedResponse = await request("/api/settings", {method:"PUT",body:JSON.stringify(unscopedSettings)});
  assert.equal(unscopedResponse.status, 400);
  assert.equal((await unscopedResponse.json()).code, "write_protocol_unavailable");
  assert.deepEqual(await (await request(`/api/settings/operations/${settingsUpdate.operationId}`)).json(), savedSettings);
  assert.equal((await request("/api/settings", { method: "PUT", body: JSON.stringify(settings) })).status, 422,
    "legacy writes without a revision must fail closed");
  const interruptedSettings = { ...settingsUpdate,
    expectedRevision: savedSettings.revision, operationId: randomUUID() };
  const settingsLock = new DatabaseSync(path.join(directory, "local-agent.sqlite3"));
  settingsLock.exec("BEGIN IMMEDIATE");
  try {
    await assert.rejects(request("/api/settings", { method: "PUT",
      body: JSON.stringify(interruptedSettings), signal: AbortSignal.timeout(200) }),
    (reason) => reason.name === "TimeoutError");
  } finally {
    settingsLock.exec("ROLLBACK");
    settingsLock.close();
  }
  let interruptedReceipt;
  const settingsDeadline = Date.now() + 5000;
  while (Date.now() < settingsDeadline) {
    const response = await request(`/api/settings/operations/${interruptedSettings.operationId}`);
    if (response.ok) { interruptedReceipt = await response.json(); break; }
    assert.equal(response.status, 404);
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  assert.equal(interruptedReceipt?.revision, savedSettings.revision + 1,
    "a disconnected write must have an authoritative queryable receipt");
  assert.deepEqual(interruptedReceipt.value, settings);
  const operationPath = `/api/settings/operations/${interruptedSettings.operationId}`;
  assert.equal((await fetch(base + operationPath)).status, 401);
  assert.equal((await request(operationPath, { headers: { Origin: "https://example.com" } })).status, 401);
  const provider = { ...data.providers[0], model: "service-test-model" };
  assert.equal(
    (
      await request("/api/providers", {
        method: "PUT",
        body: JSON.stringify(provider),
      })
    ).status,
    200,
  );
  await stop();
  // Persisted metadata fixtures, never real accounts or a model request.
  const observationDb = new DatabaseSync(
    path.join(directory, "observation", "observations.sqlite3"),
  );
  const insertObservation = observationDb.prepare(
    "INSERT INTO model_observations VALUES(?,?,?,?)",
  );
  for (let n = 0; n < 85; n++) {
    const value = {
      id: `usage-${n}`,
      sessionId: opencodeId,
      providerId: n < 82 ? provider.id : null,
      provider: "Old connection name",
      agentId: "opencode",
      agentProductId: "opencode",
      model: n % 2 ? "deepseek-flash" : "deepseek-v4-pro",
      endpoint:
        n < 40 || (n >= 80 && n < 82)
          ? "https://api.deepseek.com"
          : "https://openrouter.ai/api/v1",
      bindingRevision: 123,
      startedAt: "2026-10-18T00:00:00Z",
      endedAt: "2026-10-18T00:00:01Z",
      status: n === 0 ? "error" : "success",
      protocol: "responses",
      revision: 1,
      usage: {
        inputTokens: 10,
        outputTokens: 0,
        totalTokens: 10,
        cachedInputTokens: n < 80 ? 0 : null,
      },
      reportedCost:
        n >= 40 && n < 80
          ? {
              amount: n === 40 ? 0 : 0.002,
              currency: "USD",
              source: "openrouter.usage.cost",
            }
          : null,
      durationMs: 1000,
    };
    insertObservation.run(value.id, value.startedAt, 1, JSON.stringify(value));
  }
  observationDb.close();
  base = await start();
  data = await (await request("/api/snapshot")).json();
  assert.deepEqual(data.settings, settings);
  assert.equal(data.settingsRevision, interruptedReceipt.revision);
  assert.deepEqual(await (await request(`/api/settings/operations/${settingsUpdate.operationId}`)).json(), savedSettings);
  assert.deepEqual(await (await request(operationPath)).json(), interruptedReceipt);
  assert.equal(data.providers[0].model, "service-test-model");
  assert.equal(
    data.sessionSources.find((source) => source.agentId === "claude").enabled,
    true,
  );
  sessions = await waitForIndex();
  assert.equal(sessions.total, 1);
  assert.equal(
    data.sessionSources.find((s) => s.agentId === "opencode").enabled,
    true,
  );
  assert.equal((await waitForIndex("opencode")).total, 1);
  const usageProvider = {
    ...data.providers.find((p) => p.id === provider.id),
    configured: true,
    endpoint: "http://127.0.0.1:9/v1",
    protocol: "responses",
    credentialRef: null,
    credentialLabel: null,
  };
  assert.equal(
    (
      await request("/api/providers", {
        method: "PUT",
        body: JSON.stringify(usageProvider),
      })
    ).status,
    200,
  );
  const providerUsage = await (
    await request(
      `/api/provider-usage?source=gateway&providerId=${encodeURIComponent(provider.id)}`,
    )
  ).json();
  assert.equal(providerUsage.summary.calls, 85);
  assert.equal(providerUsage.unattributed.calls, 3);
  assert.equal(providerUsage.selected.calls, 82);
  assert.equal(providerUsage.selected.usage.totalTokens, 820);
  assert.equal(providerUsage.selected.errors, 1);
  assert.equal(providerUsage.calls.length, 40);
  assert.equal(providerUsage.models.length, 2);
  assert.equal(providerUsage.selected.cost.actual[0].calls, 40);
  assert.ok(
    Math.abs(providerUsage.selected.cost.actual[0].amount - 0.078) < 1e-12,
  );
  assert.equal(providerUsage.selected.cost.estimated[0].calls, 40);
  assert.equal(providerUsage.selected.cost.estimated[0].currency, "CNY");
  assert.ok(
    Math.abs(providerUsage.selected.cost.estimated[0].amount - 0.0011) < 1e-12,
  );
  assert.equal(providerUsage.selected.cost.unknownCalls, 2);
  assert.equal(providerUsage.summary.cost.unknownCalls, 5);
  // Provider pages use the independent CNY tariff. Existing Session, project,
  // request and Agent views retain the verified USD tariff without conversion.
  const usdCosts = {
    actual: [{ currency: "USD", amount: 0.078, calls: 40 }],
    estimated: [{ currency: "USD", amount: 0.000162, calls: 40 }],
    unknownCalls: 5,
  };
  const sameCosts = (actual, expected) => {
    assert.equal(actual.unknownCalls, expected.unknownCalls);
    for (const key of ["actual", "estimated"]) {
      assert.equal(actual[key].length, expected[key].length);
      for (const item of expected[key]) {
        const matched = actual[key].find((m) => m.currency === item.currency);
        assert.ok(matched);
        assert.equal(matched.calls, item.calls);
        assert.ok(Math.abs(matched.amount - item.amount) < 1e-12);
      }
    }
  };
  const feeConversation = await (
    await request("/api/sessions/" + encodeURIComponent(opencodeId))
  ).json();
  assert.equal(feeConversation.associatedCosts.gatewayAttempts, 85);
  assert.equal(feeConversation.associatedCosts.nativeResponses, 1);
  assert.equal(feeConversation.associatedCosts.unlinkedNativeResponses, 1);
  sameCosts(feeConversation.associatedCosts.cost, usdCosts);
  assert.ok(!("selectedSessionIds" in feeConversation));
  const feeSessions = await (
    await request("/api/sessions?agent=opencode")
  ).json();
  sameCosts(feeSessions.associatedCosts.cost, usdCosts);
  const feeProject = await (
    await request(
      "/api/projects/" + encodeURIComponent(feeConversation.session.projectId),
    )
  ).json();
  sameCosts(feeProject.project.totals.associatedCosts.cost, usdCosts);
  assert.equal(
    feeProject.project.agents[0].totals.associatedCosts.gatewayAttempts,
    85,
  );
  const feeProjects = await (
    await request("/api/projects?agent=opencode")
  ).json();
  sameCosts(feeProjects.summary.associatedCosts.cost, usdCosts);
  const modelFees = await (
    await request("/api/agent-usage?source=gateway&agent=opencode")
  ).json();
  sameCosts(modelFees.summary.cost, usdCosts);
  const nativeFees = await (
    await request("/api/agent-usage?source=native&agent=opencode")
  ).json();
  assert.equal(nativeFees.summary.cost.unknownCalls, 1);
  assert.deepEqual(nativeFees.summary.cost.actual, []);
  assert.deepEqual(nativeFees.summary.cost.estimated, []);

  const feeDetailPath = (q) =>
    "/api/associated-costs?" + new URLSearchParams(q);
  for (const scope of ["session", "project"]) {
    const q = {
      scope,
      id: scope === "session" ? opencodeId : feeConversation.session.projectId,
    };
    const first = await (await request(feeDetailPath(q))).json();
    assert.equal(first.total, 85);
    assert.equal(first.calls.length, 40);
    sameCosts(first.filteredCost, usdCosts);
    sameCosts(first.summary.cost, usdCosts);
    assert.equal(
      first.providers.reduce((n, g) => n + g.attempts, 0),
      85,
    );
    assert.equal(
      first.models.reduce((n, g) => n + g.attempts, 0),
      85,
    );
    const ids = new Set(first.calls.map((a) => a.id));
    let cursor = first.nextCursor;
    while (cursor) {
      const page = await (
        await request(feeDetailPath({ ...q, cursor }))
      ).json();
      assert.equal(page.total, 85);
      for (const a of page.calls) {
        assert.ok(!ids.has(a.id));
        ids.add(a.id);
      }
      cursor = page.nextCursor;
    }
    assert.equal(ids.size, 85);
    for (const group of first.providers) {
      const filtered = await (
        await request(feeDetailPath({ ...q, provider: group.providerKey }))
      ).json();
      assert.equal(filtered.total, group.attempts);
      sameCosts(filtered.filteredCost, group.cost);
      assert.equal(filtered.filteredProviders.length, 1);
    }
    const requestDetail = await (
      await request(
        "/api/request-detail?id=" + encodeURIComponent(first.calls[0].id),
      )
    ).json();
    assert.ok(
      requestDetail.request.attempts.some((a) => a.id === first.calls[0].id),
    );
  }
  assert.equal(
    (await request(feeDetailPath({ scope: "session", id: "missing" }))).status,
    404,
  );
  assert.equal(
    (await request(feeDetailPath({ scope: "all", id: opencodeId }))).status,
    400,
  );
  assert.equal(
    (
      await request(
        feeDetailPath({ scope: "session", id: opencodeId, agent: "opencode" }),
      )
    ).status,
    400,
  );

  const secondUsage = await (
    await request(
      `/api/provider-usage?source=gateway&providerId=${encodeURIComponent(provider.id)}&cursor=${encodeURIComponent(providerUsage.nextCursor)}`,
    )
  ).json();
  assert.equal(secondUsage.selected.calls, 82);
  assert.deepEqual(secondUsage.selected.cost, providerUsage.selected.cost);
  assert.notEqual(secondUsage.calls[0].id, providerUsage.calls[0].id);
  const byModel = await (
    await request(
      `/api/provider-usage?source=gateway&providerId=${encodeURIComponent(provider.id)}&model=deepseek-flash`,
    )
  ).json();
  assert.equal(byModel.selected.calls, 41);
  assert.equal(byModel.selected.cost.estimated[0].calls, 20);
  assert.equal(byModel.selected.cost.actual[0].calls, 20);
  assert.equal(byModel.selected.cost.unknownCalls, 1);
  assert.equal(byModel.modelOptions.length, 2);
  const nativeProviderUsage = await (
    await request(
      "/api/provider-usage?source=native&agent=opencode&providerId=_unknown",
    )
  ).json();
  assert.equal(nativeProviderUsage.selected.calls, 1);
  assert.equal(nativeProviderUsage.calls[0].sessionId, opencodeId);
  assert.equal(nativeProviderUsage.calls[0].providerId, null);
  assert.ok(
    !JSON.stringify(providerUsage).includes("native-message-sentinel-42"),
  );
  assert.equal(readFileSync(nativeFile, "utf8"), fixture);
  for (const filename of readdirSync(directory).filter((filename) =>
    filename.startsWith("local-agent.sqlite3"),
  )) {
    for (const sentinel of [
      "opencode-private-body-sentinel-42",
      "opencode-tool-output-sentinel-42",
    ])
      assert.equal(
        readFileSync(path.join(directory, filename)).includes(
          Buffer.from(sentinel),
        ),
        false,
      );
    assert.equal(
      readFileSync(path.join(directory, filename)).includes(
        Buffer.from("native-message-sentinel-42"),
      ),
      false,
    );
    assert.equal(
      readFileSync(path.join(directory, filename)).includes(
        Buffer.from("native-output-sentinel-42"),
      ),
      false,
    );
  }
  assert.equal(
    (
      await request("/api/session-sources", {
        method: "PUT",
        body: JSON.stringify({ agentId: "claude", enabled: false }),
      })
    ).status,
    200,
  );
  assert.equal(
    (await (await request("/api/sessions?agent=claude")).json()).total,
    0,
  );
  assert.equal(
    (await request("/api/sessions/" + encodeURIComponent(sessionId))).status,
    404,
  );
  const revokedUsage = await (
    await request("/api/agent-usage?agent=claude&source=native")
  ).json();
  assert.equal(revokedUsage.sourceState, "disabled");
  assert.equal(revokedUsage.total, 0);
  assert.equal(revokedUsage.summary.usage.totalTokens, null);
  assert.equal(
    (
      await request("/api/session-sources", {
        method: "PUT",
        body: JSON.stringify({ agentId: "opencode", enabled: false }),
      })
    ).status,
    200,
  );
  assert.equal((await (await request("/api/sessions")).json()).total, 0);
  assert.equal(
    (await request("/api/sessions/" + encodeURIComponent(opencodeId))).status,
    404,
  );
  assert.equal(
    (await request(feeDetailPath({ scope: "session", id: opencodeId }))).status,
    404,
  );
  assert.equal(
    opencode.prepare("SELECT count(*) as total FROM session").get().total,
    1,
  );
  console.log(
    "通过：服务鉴权、Host / Origin 拒绝、配置与来源授权持久化、原生会话与 MCP 详情、ID 关联、只读文件、正文不额外落盘及断开后的访问拒绝。",
  );
} finally {
  await stop();
  opencode.close();
  rmSync(directory, { recursive: true, force: true });
}
