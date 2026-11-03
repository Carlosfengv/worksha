import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn, execFileSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { once } from "node:events";
import { createInterface } from "node:readline";
import http from "node:http";
import { workerEntryProbe } from "./lib/worker-entry-probe.mjs";

const option = (name, fallback) => {
  const index = process.argv.indexOf(name);
  return index < 0 ? fallback : process.argv[index + 1];
};
const binary = fs.realpathSync(
  option("--binary", "target/debug/local-agent-service"),
);
const output = path.resolve(option("--report", ".work/domain-isolation.json"));
const probeOnly = process.argv.includes("--probe-only");
const hostileComponent = fs.realpathSync(
  option("--hostile-component", ".work/plugin-m4-hostile.wasm"),
);
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "local-agent-domain-isolation-")),
);
const home = path.join(root, "home"),
  data = path.join(root, "data");
fs.mkdirSync(home);
fs.mkdirSync(path.dirname(output), { recursive: true });
const sha = (file) =>
  createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const report = {
  complete: false,
  phaseAccepted: false,
  binarySha256: sha(binary),
  harnessSha256: sha(import.meta.filename),
  probeOnly,
  root,
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
};
const token = randomUUID();
let child,
  base,
  observer,
  active = 0,
  upstream,
  gateway,
  configFile,
  configBefore,
  sampler;
const waitingWorkers = [];
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const metrics = {};
report.resources = [];
const fixtures = [];
function sampleResource(label) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const rss =
    Number(
      execFileSync("/bin/ps", ["-p", String(child.pid), "-o", "rss="], {
        encoding: "utf8",
      }).trim(),
    ) * 1024;
  assert(Number.isSafeInteger(rss) && rss > 0);
  report.resources.push({
    at: new Date().toISOString(),
    label,
    serviceRssBytes: rss,
    workerEntries: observer.events().length,
  });
}
const save = () =>
  fs.writeFileSync(output, JSON.stringify(report, null, 2) + "\n");
const sdk = (...args) =>
  execFileSync(binary, ["--plugin-sdk", ...args], {
    encoding: "utf8",
    timeout: 30000,
  });
async function api(
  route,
  body,
  expected = 200,
  method = body === undefined ? "GET" : "POST",
) {
  const started = performance.now();
  const metric = (metrics[`${method} ${route} -> ${expected}`] ??= {
    timesMs: [],
    unexpectedFailures: 0,
  });
  active++;
  try {
    const response = await fetch(base + route, {
      method,
      headers: {
        authorization: `Bearer ${token}`,
        "content-type": "application/json",
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(40000),
    });
    const raw = await response.text();
    assert.equal(response.status, expected, `${route}: ${raw}`);
    return JSON.parse(raw);
  } catch (error) {
    metric.unexpectedFailures++;
    throw error;
  } finally {
    active--;
    metric.timesMs.push(performance.now() - started);
  }
}
const factQuery = (scope, kind = "session") => ({
  scope,
  kind,
  projectId: null,
  sessionId: null,
  responseId: null,
  snapshotToken: null,
  cursor: null,
});
const emptyUsage = {
  inputTokens: null,
  outputTokens: null,
  cachedInputTokens: null,
  cacheWriteInputTokens: null,
  reasoningOutputTokens: null,
  totalTokens: null,
};
const events = Array.from({ length: 83 }, (_, n) => ({
  id: `e${n}`,
  nativeId: `e${n}`,
  turnId: `t${n}`,
  kind: "message",
  role: n % 2 ? "assistant" : "user",
  status: "recorded",
  text: `Owned message ${n}`,
  input: null,
  output: null,
  model: null,
  responseId: null,
  usage: null,
}));
let key;
function command(pid) {
  return observer.command(pid);
}
async function waitFor(test, milliseconds = 5000) {
  const deadline = Date.now() + milliseconds;
  while (Date.now() < deadline) {
    const result = test();
    if (result) return result;
    await delay(2);
  }
  throw Error("owned process observation deadline");
}
async function pausedWorker(route, request, expected) {
  const count = observer.events().length,
    started = performance.now();
  const settled = api(route, request, expected).then(
    (value) => ({ value }),
    (error) => ({ error }),
  );
  const task = { settled, started, pid: null };
  waitingWorkers.push(task);
  const entry = await waitFor(() =>
    observer
      .events()
      .slice(count)
      .find((e) => e.phase === "entry" && e.kind === "plugin"),
  );
  task.pid = entry.pid;
  await waitFor(() =>
    command(entry.pid)?.startsWith(`${observer.real} --plugin-worker `),
  );
  process.kill(entry.pid, "SIGSTOP");
  await waitFor(() =>
    execFileSync("/bin/ps", ["-p", String(entry.pid), "-o", "stat="], {
      encoding: "utf8",
    }).includes("T"),
  );
  task.stoppedAt = performance.now();
  assert(
    command(entry.pid)?.startsWith(`${observer.real} --plugin-worker `),
  );
  return task;
}
const pausedRefresh = (plugin, expected = 400) =>
  pausedWorker("/api/plugin-sources/refresh", plugin.query, expected);
async function outcome(task) {
  const result = await task.settled;
  if (result.error) throw result.error;
  await waitFor(() => !command(task.pid));
  return {
    code: result.value.code,
    elapsedMs: performance.now() - task.started,
    pid: task.pid,
  };
}
async function setupGateway() {
  upstream = http.createServer(async (request, response) => {
    let raw = "";
    for await (const bytes of request) raw += bytes;
    const input = JSON.parse(raw);
    report.syntheticUpstreamRequests =
      (report.syntheticUpstreamRequests ?? 0) + 1;
    await delay(2);
    response.writeHead(200, { "content-type": "application/json" });
    response.end(
      JSON.stringify({
        id: randomUUID(),
        type: "message",
        role: "assistant",
        model: input.model,
        content: [{ type: "text", text: "OWNED_FIXTURE" }],
        stop_reason: "end_turn",
        usage: { input_tokens: 1, output_tokens: 1 },
      }),
    );
  });
  await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));
  const provider = {
    id: `custom-${randomUUID()}`,
    name: "Owned isolation upstream",
    vendor: "custom",
    protocol: "messages",
    endpoint: `http://127.0.0.1:${upstream.address().port}`,
    model: "fixture",
    models: ["fixture"],
    configured: true,
    credentialRef: null,
    credentialLabel: null,
    revision: 0,
    category: "local",
  };
  await api("/api/providers", provider, 200, "PUT");
  assert.equal(
    (
      await api(`/api/providers/${provider.id}/verify`, {
        expectedRevision: 1,
        model: "fixture",
      })
    ).state,
    "verified",
  );
  const config = (await api("/api/provider-status")).configs.find(
    (c) => c.agentId === "opencode",
  );
  const transaction = await api("/api/dynamic-connections", {
    agentId: "opencode",
    expectedRevision: config.revision,
    providerId: provider.id,
    expectedProviderRevision: 1,
    model: "fixture",
  });
  await api(`/api/provider-switches/${transaction.id}/apply`, {});
  const installed = JSON.parse(fs.readFileSync(configFile, "utf8"));
  gateway = {
    ...installed.provider[installed.model.split("/")[0]].options,
    transaction,
  };
}
async function modelRequest() {
  const response = await fetch(gateway.baseURL + "/messages", {
    method: "POST",
    headers: {
      authorization: `Bearer ${gateway.apiKey}`,
      "content-type": "application/json",
      "x-opencode-session": `ses_${randomUUID().replaceAll("-", "")}`,
    },
    body: JSON.stringify({
      model: "local-agent-default",
      max_tokens: 10,
      messages: [{ role: "user", content: "Owned offline request" }],
    }),
    signal: AbortSignal.timeout(5000),
  });
  const raw = await response.text();
  assert.equal(response.status, 200, raw);
  assert(JSON.parse(raw).content.some((p) => p.text === "OWNED_FIXTURE"));
}
async function measured(group, run) {
  const start = performance.now();
  const value = await run();
  (report.measurements[group] ??= []).push(performance.now() - start);
  return value;
}
async function displayConsent(plugin, enabled) {
  const review = await api("/api/domain/consent-review", plugin.query);
  await api("/api/domain/consent", {
    ...plugin.query,
    enabled,
    persistMetadata: false,
    reviewRef: review.reviewRef,
  });
}
async function grant(plugin) {
  await api("/api/plugin-sources/source", {
    package: plugin.query,
    path: plugin.source,
  });
  await api("/api/plugin-sources/refresh", plugin.query);
  await displayConsent(plugin, true);
}
async function fuelBudgetCase() {
  const folder = path.join(root, "fuel-package");
  fs.cpSync("sdk/plugins/examples/aurora-agent/package", folder, {
    recursive: true,
  });
  const manifest = JSON.parse(
    fs.readFileSync(path.join(folder, "manifest.json"), "utf8"),
  );
  manifest.id = "org.example.isolation-fuel";
  manifest.agent.productId = `${manifest.id}:cli`;
  manifest.settings = [
    {
      key: "behavior",
      label: "Owned fixture behavior",
      options: ["loop"],
      default: "loop",
    },
  ];
  fs.writeFileSync(
    path.join(folder, "manifest.json"),
    JSON.stringify(manifest),
  );
  fs.copyFileSync(hostileComponent, path.join(folder, "plugin.wasm"));
  report.hostileComponentSha256 = sha(hostileComponent);
  const archive = path.join(root, "fuel.laplug");
  sdk("pack", folder, archive, key);
  const review = await api("/api/plugins/inspect", { path: archive });
  await api("/api/plugins/install", {
    packageRef: review.packageRef,
    digest: review.digest,
    approvePublisher: true,
  });
  const query = { pluginId: manifest.id, digest: review.digest };
  await api("/api/plugins/state", { ...query, action: "enable" });
  const source = path.join(root, "fuel-source.jsonl");
  fs.copyFileSync("sdk/plugins/fixtures/aurora.jsonl", source);
  await api("/api/plugins/source", { ...query, path: source });
  const count = observer.events().length,
    start = performance.now();
  const failure = await api(
    "/api/plugins/action",
    { ...query, action: { kind: "refresh" } },
    400,
  );
  assert.equal(failure.code, "resource_limit");
  const entries = observer.events().slice(count);
  assert.equal(entries.length, 1);
  assert.equal(entries[0].kind, "plugin");
  const diagnostics = (await api("/api/plugins")).diagnostics.items.find(
    (item) => item.digest === query.digest,
  ).activity;
  assert.equal(diagnostics.resourceLimits, "1");
  assert.equal(diagnostics.failed, "1");
  report.fuelBudget = {
    code: failure.code,
    elapsedMs: performance.now() - start,
    actualWorkerEntries: 1,
    diagnostics,
  };
  await api("/api/plugins/state", { ...query, action: "disable" });
  report.cases.push(
    "a real signed hostile Wasm exhausts its 100-million fuel budget, reports resource_limit and one actual worker entry, and leaves the core and history host usable",
  );
}
async function legacyMetadataCase(first) {
  const folder = path.join(root, "legacy-package");
  fs.cpSync("sdk/plugins/examples/aurora-agent/package", folder, {
    recursive: true,
  });
  const manifest = JSON.parse(
    fs.readFileSync(path.join(folder, "manifest.json"), "utf8"),
  );
  manifest.id = "org.example.isolation-legacy";
  manifest.agent.productId = `${manifest.id}:cli`;
  fs.writeFileSync(
    path.join(folder, "manifest.json"),
    JSON.stringify(manifest),
  );
  const archive = path.join(root, "legacy.laplug");
  sdk("pack", folder, archive, key);
  const review = await api("/api/plugins/inspect", { path: archive });
  await api("/api/plugins/install", {
    packageRef: review.packageRef,
    digest: review.digest,
    approvePublisher: true,
  });
  const query = { pluginId: manifest.id, digest: review.digest },
    source = path.join(root, "legacy-source.jsonl");
  fs.copyFileSync("sdk/plugins/fixtures/aurora.jsonl", source);
  const original = sha(source);
  const entriesBefore = observer.events().length;
  await api("/api/plugins/state", { ...query, action: "enable" });
  await api("/api/plugins/source", { ...query, path: source });
  await api("/api/plugins/action", { ...query, action: { kind: "refresh" } });
  assert(
    observer
      .events()
      .slice(entriesBefore)
      .every((entry) => entry.kind === "plugin"),
  );
  assert(observer.events().length > entriesBefore);
  await displayConsent({ query }, true);
  await displayConsent(first, true);
  const legacySource = `${manifest.id}:source`;
  const scope = {
    kind: "combined",
    sourceIds: ["claude", first.scope.sourceIds[0], legacySource],
  };
  let sessions;
  const before = observer.events().length;
  for (let round = 0; round < 10; round++) {
    sessions = await api("/api/domain/sessions", {
      scope,
      filter: { query: null, productId: null, days: null },
      snapshotToken: null,
      cursor: null,
    });
    assert.deepEqual(
      [...new Set(sessions.items.map((item) => item.sourceId))].sort(),
      [...scope.sourceIds].sort(),
    );
    await api("/api/domain/facts", factQuery(scope));
    const responses = await api(
      "/api/domain/facts",
      factQuery(scope, "response"),
    );
    assert.equal(
      responses.sourceSummaries.find((source) => source.sourceId === "claude")
        .responses.records,
      "80",
    );
    assert.equal(
      responses.sourceSummaries.find(
        (source) => source.sourceId === first.scope.sourceIds[0],
      ).responses.records,
      "1",
    );
    assert.equal(
      responses.sourceSummaries.find(
        (source) => source.sourceId === legacySource,
      ).responses.records,
      null,
    );
    await api("/api/plugins/view", {
      pluginId: manifest.id,
      viewId: "history",
      cursor: null,
    });
  }
  assert.equal(observer.events().length, before);
  const selected = sessions.items.find(
    (item) => item.sourceId === legacySource,
  );
  const body = await api("/api/domain/conversation", {
    sessionId: selected.record.id,
    snapshotToken: sessions.snapshotToken,
    cursor: null,
    anchor: null,
  });
  assert(body.conversation.events.length > 0);
  assert.equal(observer.events().length, before + 1);
  assert.equal(observer.events().at(-1).kind, "plugin");
  assert.equal(sha(source), original);
  report.mixedMetadataQueries = 40;
  report.mixedMetadataWorkerStarts = 0;
  report.cases.push(
    "40 builtin/basic/history combined metadata queries start zero workers, preserve independent response units and unsupported legacy totals, then an explicit legacy body read is observed as a positive control",
  );
}
async function install(index) {
  const folder = path.join(root, `package-${index}`);
  fs.cpSync("sdk/plugins/examples/full-history/package", folder, {
    recursive: true,
  });
  const manifest = JSON.parse(
    fs.readFileSync(path.join(folder, "manifest.json"), "utf8"),
  );
  manifest.id = `org.example.isolation-${index}`;
  fs.writeFileSync(
    path.join(folder, "manifest.json"),
    JSON.stringify(manifest),
  );
  const archive = path.join(root, `package-${index}.laplug`);
  sdk("pack", folder, archive, key);
  const review = await api("/api/plugin-sources/inspect", { path: archive });
  await api("/api/plugin-sources/install", {
    packageRef: review.packageRef,
    digest: review.digest,
    approvePublisher: true,
    expectedSigningKey: review.signingKey,
  });
  const query = { pluginId: manifest.id, digest: review.digest };
  await api("/api/plugin-sources/state", { package: query, action: "enable" });
  const source = path.join(root, `source-${index}.jsonl`);
  const rows = [
    {
      fact: {
        kind: "project",
        value: {
          nativeId: "p1",
          name: "Owned reported project",
          pathHint: null,
        },
      },
    },
    {
      fact: {
        kind: "session",
        value: {
          nativeId: "s1",
          productIds: [],
          unknownClient: true,
          title: "Owned isolated history",
          createdAt: null,
          updatedAt: null,
          projectRef: "p1",
          usage: emptyUsage,
          partial: false,
        },
      },
      conversation: events,
    },
    {
      fact: {
        kind: "response",
        value: {
          nativeId: "r1",
          sessionRef: "s1",
          productId: null,
          occurredAt: null,
          model: "fixture",
          outcome: "success",
          usage: emptyUsage,
          reportedResponseId: null,
          durationMs: null,
        },
      },
    },
    {
      fact: {
        kind: "activity",
        value: {
          nativeId: "a1",
          sessionRef: "s1",
          responseRef: "r1",
          occurredAt: null,
          phase: "generating",
          durationMs: null,
        },
      },
    },
    {
      fact: {
        kind: "write",
        value: {
          nativeId: "w1",
          sessionRef: "s1",
          projectRef: "p1",
          responseRef: "r1",
          relativePath: "owned.txt",
          occurredAt: null,
          outcome: "success",
        },
      },
    },
    {
      fact: {
        kind: "mcp-catalog",
        value: { nativeId: "m1", serverName: "owned", toolNames: ["lookup"] },
      },
    },
    {
      fact: {
        kind: "mcp-execution",
        value: {
          nativeId: "x1",
          serverRef: "m1",
          sessionRef: "s1",
          responseRef: "r1",
          toolName: "lookup",
          occurredAt: null,
          durationMs: null,
          outcome: "success",
        },
      },
    },
  ];
  fs.writeFileSync(source, rows.map(JSON.stringify).join("\n") + "\n");
  await api("/api/plugin-sources/source", { package: query, path: source });
  await api("/api/plugin-sources/refresh", query);
  const reviewConsent = await api("/api/domain/consent-review", query);
  await api("/api/domain/consent", {
    ...query,
    enabled: true,
    persistMetadata: false,
    reviewRef: reviewConsent.reviewRef,
  });
  return {
    query,
    source,
    sourceSha256: sha(source),
    scope: { kind: "external", sourceIds: [`${manifest.id}:history`] },
  };
}
try {
  const nativeDir = path.join(home, ".claude/projects/fixture"),
    workspace = path.join(root, "workspace");
  fs.mkdirSync(nativeDir, { recursive: true });
  fs.mkdirSync(workspace);
  const at = new Date(Date.now() - 60000).toISOString();
  for (let n = 0; n < 2; n++) {
    const id = `native-${n}`,
      rows = [
        {
          type: "user",
          sessionId: id,
          uuid: "u",
          cwd: workspace,
          timestamp: at,
          message: { content: "Owned native" },
        },
      ];
    for (let c = 0; c < 40; c++)
      rows.push({
        type: "assistant",
        sessionId: id,
        uuid: `a${c}`,
        timestamp: at,
        message: {
          id: `r${c}`,
          model: "fixture",
          content: [{ type: "text", text: "Owned" }],
          usage: { input_tokens: 1, output_tokens: 1 },
        },
      });
    fs.writeFileSync(
      path.join(nativeDir, `${n}.jsonl`),
      rows.map(JSON.stringify).join("\n") + "\n",
    );
  }
  const nativeHashes = Object.fromEntries(
    fs.readdirSync(nativeDir).map((name) => {
      const file = path.join(nativeDir, name);
      return [file, sha(file)];
    }),
  );
  configFile = path.join(home, ".config/opencode/opencode.json");
  fs.mkdirSync(path.dirname(configFile), { recursive: true });
  configBefore = '{"keep":"owned","permission":{"*":"deny"}}\n';
  fs.writeFileSync(configFile, configBefore, { mode: 0o600 });
  observer = workerEntryProbe(binary, root);
  report.probeSourceSha256 = sha(path.join(root, "worker-launcher.c"));
  report.probeHelperSha256 = sha("scripts/lib/worker-entry-probe.mjs");
  child = spawn(observer.entry, ["--data-dir", data], {
    env: {
      ...process.env,
      LOCAL_AGENT_SOURCE_HOME: home,
      LOCAL_AGENT_SERVICE_TOKEN: token,
      LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
      LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let error = "";
  child.stderr.on("data", (bytes) => {
    error = (error + bytes).slice(-3000);
  });
  const [line] = await Promise.race([
    once(createInterface({ input: child.stdout }), "line", {
      signal: AbortSignal.timeout(30000),
    }),
    once(child, "exit").then(() => {
      throw Error(error);
    }),
  ]);
  base = `http://127.0.0.1:${JSON.parse(line).port}`;
  observer.install();
  assert.equal(sha(observer.real), report.binarySha256);
  sampleResource("ready");
  sampler = setInterval(() => {
    try {
      sampleResource("sample");
    } catch (error) {
      report.resourceObservationError = error.message;
    }
  }, 1000);
  key = path.join(root, "publisher.key");
  sdk("keygen", key);
  const first = await install(0);
  fixtures.push(first);
  const entries = observer.events();
  assert.equal(
    entries.length,
    2,
    "positive controls must intercept enable and scan workers",
  );
  assert(entries.every((e) => e.phase === "entry" && e.kind === "plugin"));
  report.cases.push(
    "positive controls intercept real history enable and refresh worker entries while the parent and executed worker bytes match the pinned binary",
  );
  const before = entries.length;
  for (let round = 0; round < 10; round++) {
    await api("/api/plugin-sources");
    await api("/api/domain/directory");
    for (const kind of [
      "project",
      "session",
      "response",
      "activity",
      "write",
      "mcp-catalog",
      "mcp-execution",
    ])
      await api("/api/domain/facts", factQuery(first.scope, kind));
    const sessions = await api("/api/domain/sessions", {
      scope: first.scope,
      filter: { query: null, productId: null, days: null },
      snapshotToken: null,
      cursor: null,
    });
    assert.equal(sessions.items.length, 1);
    await api("/api/domain/activity", {
      scope: first.scope,
      rangeSeconds: 86400,
      productId: null,
      sessionId: null,
      includeGateway: false,
      includeHistorical: true,
    });
    await api("/api/domain/requests", {
      scope: first.scope,
      unit: "model-response",
      descending: true,
      filter: {
        start: null,
        end: null,
        productId: null,
        sessionId: null,
        projectId: null,
        providerKey: null,
        model: null,
        outcome: null,
        search: null,
      },
      snapshotToken: null,
      cursor: null,
    });
    for (const route of [
      "/api/domain/catalog",
      "/api/domain/capabilities",
      "/api/projects",
      "/api/accounts",
      "/api/mcp",
    ])
      await api(route);
  }
  assert.equal(
    observer.events().length,
    before,
    "pure metadata queries must not enter any worker process",
  );
  const sessions = await api("/api/domain/facts", factQuery(first.scope));
  const body = await api("/api/domain/fact-conversation", {
    sessionId: sessions.items[0].id,
    snapshotToken: sessions.snapshotToken,
    cursor: null,
    anchor: null,
  });
  assert.equal(body.conversation.events.length, 40);
  assert.equal(
    observer.events().length,
    before + 1,
    "explicit body read must be observed as a positive control after the zero-start interval",
  );
  report.metadataWorkerStarts = 0;
  report.metadataQueries = 170;
  report.cases.push(
    "170 real cached metadata queries across facts, sessions, activity, requests and original read facades start zero workers, with an observed explicit body worker afterward",
  );
  if (!probeOnly) {
    report.measurements = {};
    await api(
      "/api/session-sources",
      { agentId: "claude", enabled: true },
      200,
      "PUT",
    );
    const deadline = Date.now() + 30000;
    while (
      (await api("/api/snapshot")).sessionSources.find(
        (s) => s.agentId === "claude",
      ).status !== "ready"
    ) {
      assert(Date.now() < deadline);
      await delay(20);
    }
    await setupGateway();
    const plugins = [first];
    for (let n = 1; n < 5; n++) plugins.push(await install(n));
    fixtures.push(...plugins.slice(1));
    report.fixtureScale = {
      nativeFiles: 2,
      nativeResponses: 80,
      externalSources: 5,
      factsPerExternalSource: 7,
      bodyEventsPerSource: 83,
    };
    const initial = observer.events().length;
    const builtin = { kind: "builtin", sourceIds: ["claude"] };
    const tasks = [];
    for (const plugin of plugins.slice(0, 4))
      tasks.push(await pausedRefresh(plugin));
    assert.equal(observer.events().length, initial + 4);
    report.stalledWorkers = tasks.map((task) => task.pid);
    for (let n = 0; n < 20; n++) {
      const failure = await measured("saturatedRefreshRejection", () =>
        api("/api/plugin-sources/refresh", plugins[4].query, 400),
      );
      assert.equal(failure.code, "resource_limit");
    }
    const workersBeforeReads = observer.events().length;
    for (let n = 0; n < 100; n++) {
      await measured("snapshotWhileWorkersStopped", () => api("/api/snapshot"));
      await measured("runtimeWhileWorkersStopped", () => api("/api/runtime"));
      const native = await measured("nativeFactsWhileWorkersStopped", () =>
        api("/api/domain/facts", factQuery(builtin, "response")),
      );
      assert.equal(native.sourceSummaries[0].responses.records, "80");
      await measured("gatewayWhileWorkersStopped", modelRequest);
      assert(
        tasks.every((task) => command(task.pid)),
        "health samples must overlap all four stalled workers",
      );
      await delay(200);
    }
    assert.equal(observer.events().length, workersBeforeReads);
    report.wallBudgetFailures = await Promise.all(tasks.map(outcome));
    assert(
      report.wallBudgetFailures.every(
        (r) =>
          r.code === "deadline_exceeded" &&
          r.elapsedMs >= 29000 &&
          r.elapsedMs < 35000,
      ),
    );
    report.cases.push(
      "four actual history workers stopped after exec reach the 30-second parent budget; saturated refreshes fail without a fifth worker while snapshots, native facts and an unbound local gateway stay responsive",
    );
    await api("/api/plugin-sources/refresh", first.query);
    const frozen = await api("/api/domain/facts", factQuery(first.scope));
    const revoking = await pausedRefresh(first, 403);
    const revoke = await measured("resourceRevokeWhileWorkerStopped", () =>
      api("/api/plugin-sources/state", {
        package: first.query,
        action: "revoke-grants",
      }),
    );
    assert(!revoke.sourceAuthorized && !revoke.unifiedAuthorized);
    report.workerAliveAtRevokeReturn = Boolean(command(revoking.pid));
    assert.equal((await outcome(revoking)).code, "grant_revoked");
    assert.equal(
      (await api("/api/domain/facts", factQuery(first.scope))).items.length,
      0,
    );
    assert.equal(
      (
        await api(
          "/api/domain/facts",
          { ...factQuery(first.scope), snapshotToken: frozen.snapshotToken },
          409,
        )
      ).code,
      "stale_snapshot",
    );
    report.cases.push(
      "resource revocation while an unresponsive worker is pending returns without guest acknowledgement, stops data delivery, invalidates the frozen generation and prevents a late publication",
    );
    await api("/api/plugin-sources/source", {
      package: first.query,
      path: first.source,
    });
    await api("/api/plugin-sources/refresh", first.query);
    const changed = await pausedRefresh(first, 403);
    const selectedAgain = await api("/api/plugin-sources/source", {
      package: first.query,
      path: first.source,
    });
    assert(selectedAgain.sourceAuthorized && !selectedAgain.unifiedAuthorized);
    assert.equal((await outcome(changed)).code, "grant_revoked");
    assert.equal(
      (await api("/api/domain/facts", factQuery(first.scope))).items.length,
      0,
    );
    report.cases.push(
      "reselecting the resource while the old worker is stopped invalidates its generation and does not inherit display authorization or republish its old facts",
    );
    await grant(first);
    const beforeChange = await api("/api/domain/facts", factQuery(first.scope));
    const sourceBefore = fs.readFileSync(first.source, "utf8"),
      sourceHashBefore = sha(first.source);
    const mutating = await pausedRefresh(first, 409);
    fs.writeFileSync(
      first.source,
      sourceBefore.replace(
        "Owned isolated history",
        "UNPUBLISHED_SOURCE_CHANGE",
      ),
    );
    assert.equal((await outcome(mutating)).code, "stale_snapshot");
    const retained = await api("/api/domain/facts", factQuery(first.scope));
    assert(
      retained.items.every(
        (item) => item.value.value.title !== "UNPUBLISHED_SOURCE_CHANGE",
      ),
    );
    assert.equal(
      (
        await api(
          "/api/domain/facts",
          {
            ...factQuery(first.scope),
            snapshotToken: beforeChange.snapshotToken,
          },
          409,
        )
      ).code,
      "stale_snapshot",
    );
    fs.writeFileSync(first.source, sourceBefore);
    assert.equal(sha(first.source), sourceHashBefore);
    report.cases.push(
      "file mutation while a worker is stopped cancels the old scan without publishing changed facts; the previous frozen generation is invalidated and the owned source is restored byte for byte",
    );
    await grant(first);
    const bodyScope = await api("/api/domain/facts", factQuery(first.scope));
    const pendingBody = await pausedWorker(
      "/api/domain/fact-conversation",
      {
        sessionId: bodyScope.items[0].id,
        snapshotToken: bodyScope.snapshotToken,
        cursor: null,
        anchor: null,
      },
      403,
    );
    await measured("displayRevokeWhileBodyWorkerStopped", () =>
      displayConsent(first, false),
    );
    assert.equal((await outcome(pendingBody)).code, "grant_revoked");
    assert.equal(
      (
        await api(
          "/api/domain/fact-conversation",
          {
            sessionId: bodyScope.items[0].id,
            snapshotToken: bodyScope.snapshotToken,
            cursor: null,
            anchor: null,
          },
          409,
        )
      ).code,
      "stale_snapshot",
    );
    assert(
      (await api("/api/plugin-sources")).items.find(
        (item) => item.digest === first.query.digest,
      ).sourceAuthorized,
    );
    report.cases.push(
      "display-only revocation cancels a pending real body worker, rejects the old body token, keeps the separate resource grant and delivers no revoked body",
    );
    await fuelBudgetCase();
    await legacyMetadataCase(first);
    const beforeIdle = observer.events().length;
    for (let n = 0; n < 64; n++) {
      await api("/api/plugin-sources/source", {
        package: first.query,
        path: first.source,
      });
      await measured("idleResourceRevoke", () =>
        api("/api/plugin-sources/state", {
          package: first.query,
          action: "revoke-grants",
        }),
      );
      await measured("idleStatus", () => api("/api/plugin-sources"));
    }
    assert.equal(observer.events().length, beforeIdle);
    report.cases.push(
      "64 idle resource revocations and status reads start no workers and retain their full latency samples",
    );
    const restore = await api(
      `/api/provider-switches/${gateway.transaction.id}/restore`,
      {},
    );
    await api(`/api/provider-switches/${restore.id}/apply`, {});
    assert.equal(fs.readFileSync(configFile, "utf8"), configBefore);
    report.ownedConfigurationRestored = true;
  }
  assert(
    fixtures.every((fixture) => sha(fixture.source) === fixture.sourceSha256),
  );
  assert(
    Object.entries(nativeHashes).every(([file, hash]) => sha(file) === hash),
  );
  assert.equal(sha(observer.real), report.binarySha256);
  report.ownedSourcesUnchanged = true;
  sampleResource("finished");
  report.complete = true;
} catch (error) {
  report.error = error.stack;
  process.exitCode = 1;
} finally {
  clearInterval(sampler);
  if (observer)
    for (const entry of observer.events()) {
      if (
        entry.phase === "entry" &&
        command(entry.pid)?.startsWith(`${observer.real} --plugin`)
      )
        process.kill(entry.pid, "SIGKILL");
    }
  if (child && child.exitCode === null && child.signalCode === null) {
    const done = once(child, "exit");
    child.kill("SIGTERM");
    let timer;
    try {
      await Promise.race([
        done,
        new Promise((_, reject) => {
          timer = setTimeout(() => {
            child.kill("SIGKILL");
            reject(Error("owned service stop deadline"));
          }, 8000);
        }),
      ]);
    } finally {
      clearTimeout(timer);
    }
  }
  await Promise.all(waitingWorkers.map((task) => task.settled));
  if (upstream) {
    upstream.closeAllConnections();
    await new Promise((resolve) => upstream.close(resolve));
  }
  report.unfinishedRequests = active;
  report.workerEntries = observer?.events();
  if (observer) {
    await waitFor(() =>
      observer
        .events()
        .filter((e) => e.phase === "entry")
        .every((e) => !command(e.pid)?.startsWith(`${observer.real} --plugin`)),
    );
    report.ownedWorkersExited = true;
    if (
      observer
        .events()
        .some((e) => e.phase !== "entry" || e.kind === "unexpected")
    ) {
      report.complete = false;
      report.workerEntryFailure = true;
      process.exitCode = 1;
    }
  }
  if (report.resourceObservationError) {
    report.complete = false;
    process.exitCode = 1;
  }
  report.peakSampledServiceRssBytes = Math.max(
    0,
    ...report.resources.map((r) => r.serviceRssBytes),
  );
  report.observerScope =
    "Actual process-entry events before exec, with successful enable/scan/body positive controls; covers successful child starts, not failed OS spawn attempts. Stopped history workers exercise the parent wall budget during worker startup; the separate real hostile Wasm exercises guest fuel. RSS is sampled service-only, not a worker peak or a new long-term leak test.";
  const quantiles = (values) => {
    const sorted = [...values].sort((a, b) => a - b);
    return {
      samples: sorted.length,
      p50Ms: sorted[Math.ceil(sorted.length * 0.5) - 1],
      p95Ms: sorted[Math.ceil(sorted.length * 0.95) - 1],
      p99Ms: sorted[Math.ceil(sorted.length * 0.99) - 1],
    };
  };
  report.metrics = Object.fromEntries(
    Object.entries(metrics).map(([name, metric]) => [
      name,
      { ...metric, ...quantiles(metric.timesMs) },
    ]),
  );
  if (report.measurements)
    report.measurementSummary = Object.fromEntries(
      Object.entries(report.measurements).map(([name, values]) => [
        name,
        quantiles(values),
      ]),
    );
  if (report.measurementSummary) {
    report.allObservedP95BelowOneSecond = Object.values(
      report.measurementSummary,
    ).every((m) => m.p95Ms < 1000);
    if (!report.allObservedP95BelowOneSecond) {
      report.complete = false;
      process.exitCode = 1;
    }
  }
  report.processesExited =
    !child || child.exitCode !== null || child.signalCode !== null;
  if (report.processesExited) {
    fs.rmSync(root, { recursive: true, force: true });
    report.temporaryDataRemoved = true;
  }
  save();
  console.log(JSON.stringify(report));
}
