import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { once } from "node:events";
import {
  appendFileSync,
  cpSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { createInterface } from "node:readline";
import { DatabaseSync } from "node:sqlite";
import { diskSample, physicalVolume } from "./lib/capacity-evidence.mjs";
import {
  assertHistoryConsistency,
  stableStatistics,
} from "./lib/history-consistency.mjs";
import {
  historyWriter,
  assertWritingPage,
  coherentWritingPages,
} from "./lib/history-writer.mjs";

// No real agents, models, credentials, gateway listeners, or user data. The same
// installed signed Wasm is tested against an independently generated native ledger.
const option = (name, fallback) => {
  const index = process.argv.indexOf(name);
  return index < 0 ? fallback : process.argv[index + 1];
};
const binary = realpathSync(
  option("--binary", "target/release/local-agent-service"),
);
const reportPath = path.resolve(
  option("--report", ".work/plugin-stability.json"),
);
const scratch = path.resolve(option("--scratch", ".work/stability-scratch"));
const minutes = Number(option("--minutes", "30"));
const concurrency = Number(option("--concurrency", "8"));
const files = Number(option("--files", "1000"));
const calls = Number(option("--calls", "500"));
const gatewayCalls = Number(option("--gateway-calls", "5005"));
const cycleSeconds = Number(option("--cycle-seconds", "60"));
assert(minutes > 0 && minutes <= 1440);
assert(Number.isInteger(concurrency) && concurrency >= 2 && concurrency <= 32);
assert(Number.isInteger(files) && files > 0 && files <= 5000);
assert(Number.isInteger(calls) && calls > 0 && calls <= 1000);
assert(
  Number.isInteger(gatewayCalls) &&
    gatewayCalls >= 3 &&
    gatewayCalls <= 1000000,
);
assert(cycleSeconds >= 1);
const writeInterval = Number(option("--write-interval-ms", "0"));
const writeBatch = Number(option("--write-batch", "2"));
const writeTime = option("--write-time", "current");
assert(["current", "fixture"].includes(writeTime));
assert(
  Number.isInteger(writeInterval) &&
    (writeInterval === 0 || writeInterval >= 100),
);
assert(Number.isInteger(writeBatch) && writeBatch > 0 && writeBatch <= 100);
const writeOptions = { files, calls, batch: writeBatch };
mkdirSync(scratch, { recursive: true });
mkdirSync(path.dirname(reportPath), { recursive: true });
const root = realpathSync(mkdtempSync(path.join(scratch, "run-")));
const home = path.join(root, "home"),
  data = path.join(root, "data"),
  temp = path.join(root, "tmp");
for (const directory of [home, data, temp]) mkdirSync(directory);
const report = {
  schema: 1,
  complete: false,
  binary,
  binarySha256: createHash("sha256").update(readFileSync(binary)).digest("hex"),
  harnessSha256: createHash("sha256")
    .update(readFileSync(import.meta.filename))
    .digest("hex"),
  writerOracleSha256: createHash("sha256")
    .update(readFileSync(new URL("./lib/history-writer.mjs", import.meta.url)))
    .digest("hex"),
  root,
  volume: physicalVolume(root),
  files: files * 2,
  nativeCalls: files * calls * 2,
  gatewayCalls,
  requestedSeconds: minutes * 60,
  profile: minutes >= 30 ? "soak" : "smoke",
  concurrency,
  writeTimestampMode: writeTime,
  realModelCalls: 0,
  checkpoints: [],
  restarts: [],
  routes: {},
  failures: [],
  resources: [],
  lifecycleCycles: 0,
  restartPolicy:
    "Drain readers before planned graceful exit or SIGKILL; resume concurrent load after readiness and exact reconciliation. Planned restart gaps are excluded from active load duration.",
  diskScope:
    "Sources, databases, TMPDIR and SQLITE_TMPDIR use the internal physical APFS volume. Sampled volume free space includes unrelated I/O; OS caches are not flushed.",
};
const save = () =>
  writeFileSync(reportPath, JSON.stringify(report, null, 2) + "\n");
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const nativePaths = [];
const timestamp = new Date(Date.now() - 60000).toISOString();
const project = path.join(home, "workspace");
mkdirSync(path.join(project, ".git"), { recursive: true });
for (const agent of ["claude", "codex"]) {
  const directory = path.join(
    home,
    agent === "claude"
      ? ".claude/projects/project"
      : ".codex/sessions/2026/10/24",
  );
  mkdirSync(directory, { recursive: true });
  for (let file = 0; file < files; file++) {
    const id = `soak-${file}`;
    const rows =
      agent === "claude"
        ? [
            {
              type: "user",
              sessionId: id,
              uuid: "user",
              cwd: project,
              timestamp,
              message: { content: "offline capacity fixture" },
            },
          ]
        : [
            {
              type: "session_meta",
              timestamp,
              payload: { id, cwd: project, originator: "codex_cli_rs" },
            },
            {
              type: "turn_context",
              timestamp,
              payload: { model: "soak-codex", turn_id: "turn" },
            },
          ];
    for (let call = 0; call < calls; call++) {
      const response = `soak-response-${file}-${call}`;
      rows.push(
        agent === "claude"
          ? {
              type: "assistant",
              sessionId: id,
              uuid: response,
              timestamp,
              message: {
                id: response,
                model: `soak-${call % 37}`,
                content: [{ type: "text", text: "offline-body-sentinel" }],
                usage: { input_tokens: 10, output_tokens: 2 },
              },
            }
          : {
              type: "token_usage_record",
              timestamp,
              payload: {
                response_id: response,
                usage: { input_tokens: 10, output_tokens: 2 },
              },
            },
      );
    }
    const filePath = path.join(directory, `${id}.jsonl`);
    writeFileSync(
      filePath,
      rows.map((r) => JSON.stringify(r)).join("\n") + "\n",
    );
    nativePaths.push(filePath);
  }
}
const source = path.join(root, "aurora.jsonl");
cpSync("sdk/plugins/fixtures/aurora.jsonl", source);
const sourceDigest = () => {
  const hash = createHash("sha256");
  for (const file of [...nativePaths, source])
    hash.update(file.slice(root.length)).update(readFileSync(file));
  return hash.digest("hex");
};
let concurrentActive = 0,
  peakConcurrent = 0;
const writer = writeInterval
  ? historyWriter({
      paths: nativePaths,
      files,
      journal: `${reportPath}.writes.jsonl`,
      timestamp:
        writeTime === "current" ? () => new Date().toISOString() : timestamp,
      readers: () => concurrentActive,
    })
  : null;
let writing = false,
  writeTimer;
let overlappingWrites = 0;
const writeOnce = () => {
  try {
    const event = writer.append(writeBatch);
    if (event.activeReaders > 0) overlappingWrites++;
    report.writes = {
      ...writer.committed,
      overlappingWrites,
      intervalMs: writeInterval,
      batch: writeBatch,
    };
  } catch (error) {
    fatal = error;
  }
};
const startWriting = () => {
  if (!writer) return;
  writing = true;
  writeOnce();
  writeTimer = setInterval(writeOnce, writeInterval);
};
const stopWriting = () => {
  clearInterval(writeTimer);
  writing = false;
};
const initialPluginDigest = createHash("sha256")
  .update(readFileSync(source))
  .digest("hex");
const initialDigest = sourceDigest();
report.initialSourceSha256 = initialDigest;
mkdirSync(path.join(data, "observation"));
const db = new DatabaseSync(
  path.join(data, "observation/observations.sqlite3"),
);
db.exec(
  "PRAGMA journal_mode=WAL;CREATE TABLE model_observations(id TEXT PRIMARY KEY,started_at TEXT NOT NULL,revision INTEGER NOT NULL,value TEXT NOT NULL);BEGIN",
);
const insert = db.prepare("INSERT INTO model_observations VALUES(?,?,?,?)");
for (let n = 0; n < gatewayCalls; n++) {
  const row = {
    id: `gateway-soak-${n}`,
    agentId: "claude",
    agentProductId: "claude-code-cli",
    provider: "Offline archived fixture",
    providerId: "archived",
    endpoint: "https://fixture.invalid",
    bindingRevision: 1,
    model: `gateway-${n % 37}`,
    protocol: "messages",
    revision: 1,
    startedAt: timestamp,
    endedAt: timestamp,
    status: "success",
    durationMs: 1000,
    ttftMs: 100,
    usage: {
      inputTokens: 10,
      outputTokens: 2,
      totalTokens: 12,
      cachedInputTokens: 0,
    },
  };
  if (n < 3) {
    row.endpoint = "https://openrouter.ai/api/v1";
    row.reportedCost = {
      amount: n === 0 ? 0 : 0.000001,
      currency: n === 2 ? "CNY" : "USD",
      source: "openrouter.usage.cost",
    };
  }
  insert.run(row.id, timestamp, 1, JSON.stringify(row));
}
db.exec("COMMIT");
db.close();
const rawHistoryDigest = () => {
  const history = new DatabaseSync(
    path.join(data, "observation/observations.sqlite3"),
    { readOnly: true },
  );
  try {
    const hash = createHash("sha256");
    for (const row of history
      .prepare("SELECT value FROM model_observations ORDER BY id")
      .iterate())
      hash.update(row.value);
    return hash.digest("hex");
  } finally {
    history.close();
  }
};
const initialGatewayDigest = rawHistoryDigest();
report.initialGatewayRowsSha256 = initialGatewayDigest;
const key = path.join(root, "publisher.key");
const sdk = (...args) =>
  execFileSync(binary, ["--plugin-sdk", ...args], { encoding: "utf8" });
let child,
  exited,
  base,
  token,
  fatal,
  sampler,
  initialStatistics,
  initialConfiguration,
  query;
let active = 0,
  peakActive = 0,
  activeLoadSeconds = 0;
let warming = false;
let samplingPaused = false;
let generation = 0;
const routes = {
  overview:
    "/api/overview?source=native&range=24&metric=tokens&dimension=agent",
  providers: "/api/provider-usage?source=native&providerId=_unknown",
  claude: "/api/agent-usage?source=native&agent=claude",
  codex: "/api/agent-usage?source=native&agent=codex",
  sessions: "/api/sessions",
  projects: "/api/projects?scope=all",
  requests:
    "/api/request-dashboard?" +
    new URLSearchParams({
      source: "native",
      metric: "calls",
      start: new Date(Date.now() - 3600000).toISOString(),
      end: new Date(Date.now() + 48 * 3600000).toISOString(),
    }),
  gatewayOverview:
    "/api/overview?source=gateway&range=24&metric=tokens&dimension=agent",
  gatewayProviders: "/api/provider-usage?source=gateway&providerId=_unknown",
  gatewayModels: "/api/agent-usage?source=gateway&agent=claude",
  gatewayRequests:
    "/api/request-dashboard?" +
    new URLSearchParams({
      source: "gateway",
      metric: "calls",
      start: new Date(Date.now() - 3600000).toISOString(),
      end: new Date(Date.now() + 48 * 3600000).toISOString(),
    }),
};
async function api(
  route,
  body,
  method = body === undefined ? "GET" : "POST",
  expected = 200,
  timeout = 10000,
  concurrentRead = false,
) {
  const started = performance.now();
  active++;
  if (concurrentRead) {
    concurrentActive++;
    peakConcurrent = Math.max(peakConcurrent, concurrentActive);
  }
  peakActive = Math.max(peakActive, active);
  try {
    const response = await fetch(base + route, {
      method,
      headers: {
        authorization: `Bearer ${token}`,
        "content-type": "application/json",
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(timeout),
    });
    const value = await response.json();
    const name = route.split("?")[0];
    const metric = (report.routes[name] ??= {
      requests: 0,
      statuses: {},
      latencyBuckets: {},
      maxMs: 0,
    });
    const ms = performance.now() - started;
    metric.requests++;
    metric.statuses[response.status] =
      (metric.statuses[response.status] ?? 0) + 1;
    const bucket = 2 ** (Math.ceil(Math.log2(Math.max(1, ms)) * 4) / 4);
    metric.latencyBuckets[bucket] = (metric.latencyBuckets[bucket] ?? 0) + 1;
    metric.maxMs = Math.max(metric.maxMs, ms);
    if (concurrentRead) {
      metric.concurrentRequests = (metric.concurrentRequests ?? 0) + 1;
      metric.maxConcurrentReadMs = Math.max(
        metric.maxConcurrentReadMs ?? 0,
        ms,
      );
      const reads = (report.concurrentReads ??= {
        requests: 0,
        statuses: {},
        latencyBuckets: {},
        maxMs: 0,
      });
      reads.requests++;
      reads.statuses[response.status] =
        (reads.statuses[response.status] ?? 0) + 1;
      reads.latencyBuckets[bucket] = (reads.latencyBuckets[bucket] ?? 0) + 1;
      reads.maxMs = Math.max(reads.maxMs, ms);
    }
    if (response.status === 200 && ms >= 500) {
      const slow = (report.slowRequests ??= []);
      if (slow.length === 128) slow.shift();
      slow.push({
        at: new Date().toISOString(),
        route,
        elapsedMs: ms,
        concurrentRead,
      });
    }
    if (
      warming &&
      response.status === 400 &&
      ["原生来源已更新，请刷新统计", "历史查询正在更新，请稍后重试"].includes(
        value.message,
      )
    ) {
      report.warmupRejections = (report.warmupRejections ?? 0) + 1;
      throw Object.assign(Error(value.message), { warmupStale: true });
    }
    assert.equal(
      response.status,
      expected,
      `${route}: ${JSON.stringify(value)}`,
    );
    return value;
  } catch (error) {
    (report.failedRequests ??= []).push({
      at: new Date().toISOString(),
      route,
      method,
      elapsedMs: performance.now() - started,
      concurrentRead,
      expectedWarmupRejection: error.warmupStale === true,
      error: error.message,
    });
    throw error;
  } finally {
    if (concurrentRead) concurrentActive--;
    active--;
  }
}
async function boot() {
  samplingPaused = true;
  generation++;
  token = randomUUID();
  const env = {
    ...process.env,
    PATH: "/usr/bin:/bin:/usr/sbin:/sbin",
    LOCAL_AGENT_SOURCE_HOME: home,
    LOCAL_AGENT_SERVICE_TOKEN: token,
    LOCAL_AGENT_VAULT_WORKER: "1",
    TMPDIR: temp,
    SQLITE_TMPDIR: temp,
  };
  delete env.LOCAL_AGENT_TEST_MEMORY_VAULT;
  delete env.LOCAL_AGENT_TEST_IDLE_AGENTS;
  delete env.LOCAL_AGENT_PLUGIN_DEVELOPMENT;
  child = spawn(binary, ["--data-dir", data], {
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  exited = once(child, "exit");
  const current = child;
  child.stderr.on("data", (bytes) =>
    appendFileSync(`${reportPath}.service.log`, bytes),
  );
  const lines = createInterface({ input: child.stdout });
  const [line] = await Promise.race([
    once(lines, "line", { signal: AbortSignal.timeout(30000) }),
    exited.then(() => {
      throw Error("Service exited before readiness");
    }),
  ]);
  base = `http://127.0.0.1:${JSON.parse(line).port}`;
  samplingPaused = false;
  (report.boots ??= []).push({
    generation,
    pid: child.pid,
    at: new Date().toISOString(),
  });
  current.once("exit", (code, signal) => {
    report.lastExit = { code, signal };
  });
}
async function stop(crash = false) {
  samplingPaused = true;
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  try {
    if (crash) child.kill("SIGKILL");
    else await api("/api/runtime/shutdown", {});
    await Promise.race([
      exited,
      delay(15000).then(() => {
        throw Error("Service exit deadline exceeded");
      }),
    ]);
  } finally {
    if (child.exitCode === null && child.signalCode === null) {
      child.kill("SIGKILL");
      await exited;
    }
  }
}
const state = (action) => api("/api/plugins/state", { ...query, action });
const action = (value, expected = 200) =>
  api("/api/plugins/action", { ...query, action: value }, "POST", expected);
const view = () =>
  api("/api/plugins/view", {
    pluginId: query.pluginId,
    viewId: "history",
    cursor: null,
  });
async function reconcile(label, paging = false) {
  const start = performance.now();
  let pages = {},
    movingTotals,
    generationRetries = 0;
  if (warming) {
    const deadline = Date.now() + 240000;
    for (;;) {
      const sources = (await api("/api/snapshot")).sessionSources.filter(
        (s) => s.enabled,
      );
      if (
        sources.every((s) => s.status === "ready" && s.sessionCount === files)
      )
        break;
      assert(
        Date.now() < deadline,
        "Restart native sources did not become ready",
      );
      await delay(50);
    }
  }
  // Initial aggregation uses the desktop's 120-second budget; warmed concurrent
  // requests use an 8-second deadline and must all return 200.
  const cohortDeadline = Date.now() + 240000;
  for (;;) {
    pages = {};
    for (const [name, route] of Object.entries(routes)) {
      const deadline = Date.now() + 240000;
      for (;;) {
        try {
          pages[name] = await api(route, undefined, "GET", 200, 120000);
          if (warming || label === "initial-history")
            console.log(JSON.stringify({ warmupPage: name, label }));
          break;
        } catch (error) {
          if (!error.warmupStale || Date.now() >= deadline) throw error;
          await delay(100);
        }
      }
    }
    if (!writer) break;
    const checked = coherentWritingPages(
      pages,
      writeOptions,
      writer.committed,
      !writing,
    );
    movingTotals = checked.native;
    if (checked.coherent) break;
    generationRetries++;
    assert(
      Date.now() < cohortDeadline,
      "Cross-page statistics never reached a coherent committed generation",
    );
    await delay(100);
  }
  if (writer && !report.initialNativePages)
    report.initialNativePages = Object.fromEntries(
      Object.entries(pages).filter(([name]) => !name.startsWith("gateway")),
    );
  const totals = writer
    ? {
        native: movingTotals,
        gateway: assertHistoryConsistency(
          { ...pages, ...report.initialNativePages },
          { files, calls, gatewayCalls },
        ).gateway,
      }
    : assertHistoryConsistency(pages, {
        files,
        calls,
        gatewayCalls,
      });
  if (gatewayCalls >= 2) {
    assert.deepEqual(
      pages.gatewayModels.summary.cost.actual,
      [{ currency: "USD", amount: 0.000001, calls: 2 }],
      "Confirmed zero and precise USD charges must survive lifecycle changes",
    );
    assert.equal(
      pages.gatewayModels.summary.cost.unknownCalls,
      gatewayCalls - 2,
    );
  }
  const statistics = stableStatistics(pages);
  if (initialStatistics) {
    if (writer) {
      for (const name of Object.keys(statistics).filter((n) =>
        n.startsWith("gateway"),
      ))
        assert.deepEqual(
          statistics[name],
          initialStatistics[name],
          `${label}: ${name}`,
        );
    } else assert.deepEqual(statistics, initialStatistics, label);
  } else initialStatistics = statistics;
  const snapshot = await api("/api/snapshot");
  const healthySources = snapshot.sessionSources.filter(
    (source) => source.enabled,
  );
  assert.equal(healthySources.length, 2);
  for (const source of healthySources) {
    assert(
      ["ready", "scanning"].includes(source.status),
      "Native source became unhealthy",
    );
    assert.equal(
      source.error ?? null,
      null,
      "Native source reported a read error",
    );
    assert.equal(source.sessionCount, files);
  }
  assert.equal(pages.overview.collector.persistenceError ?? null, null);
  const sourceMetadata = {
    provider: pages.providers.sourceState,
    models: {
      claude: pages.claude.sourceState,
      codex: pages.codex.sourceState,
    },
    fees: {
      sessions: pages.sessions.associatedCosts.sourceState,
      projects: pages.projects.summary.associatedCosts.sourceState,
    },
    sources: healthySources.map(({ agentId, status, error }) => ({
      agentId,
      status,
      error,
    })),
  };
  const configuration = {
    providers: snapshot.providers,
    settings: snapshot.settings,
    sessionSources: snapshot.sessionSources.map(
      ({ agentId, enabled, consented }) => ({ agentId, enabled, consented }),
    ),
  };
  if (initialConfiguration)
    assert.deepEqual(
      configuration,
      initialConfiguration,
      `${label}: original configuration`,
    );
  else initialConfiguration = configuration;
  if (paging) {
    for (const name of [
      "providers",
      "claude",
      "codex",
      "sessions",
      "requests",
      "gatewayProviders",
      "gatewayModels",
      "gatewayRequests",
    ]) {
      const first = pages[name];
      if (!first.nextCursor) continue;
      let url =
        routes[name] +
        (routes[name].includes("?") ? "&" : "?") +
        "cursor=" +
        encodeURIComponent(first.nextCursor);
      if (name.endsWith("Requests") || name === "requests")
        url += "&snapshot=" + encodeURIComponent(first.snapshot);
      const next = await api(url);
      assert.deepEqual(next.summary, first.summary, `${name} page summary`);
      if (first.selected) assert.deepEqual(next.selected, first.selected);
      assert.equal(
        next.total ?? next.totalCalls,
        first.total ?? first.totalCalls,
      );
      const firstItems = first.items ?? first.calls,
        nextItems = next.items ?? next.calls;
      const identity = (item) =>
        item.id ??
        item.requestId ??
        item.responseId ??
        JSON.stringify([item.providerId, item.model]);
      const before = new Set(firstItems.map(identity));
      assert(
        nextItems.every((item) => !before.has(identity(item))),
        `${name}: duplicated page identities`,
      );
    }
    // All model groups, not just the first page, reconcile to each agent total.
    for (const name of ["claude", "codex", "gatewayModels"]) {
      let page = pages[name],
        attempts = 0,
        tokens = 0,
        groups = 0;
      for (;;) {
        for (const item of page.items) {
          attempts += item.totals.attempts;
          tokens += item.totals.usage.totalTokens;
          groups++;
        }
        if (!page.nextCursor) break;
        page = await api(
          routes[name] + "&cursor=" + encodeURIComponent(page.nextCursor),
        );
      }
      assert.equal(attempts, pages[name].summary.attempts);
      assert.equal(tokens, pages[name].summary.usage.totalTokens);
      assert.equal(groups, pages[name].total);
    }
  }
  const checkpoint = {
    label,
    at: new Date().toISOString(),
    elapsedMs: performance.now() - start,
    totals,
    sourceMetadata,
    statisticsSha256: createHash("sha256")
      .update(JSON.stringify(statistics))
      .digest("hex"),
    paging,
    generationRetries,
    committed: writer ? { ...writer.committed } : undefined,
  };
  report.checkpoints.push(checkpoint);
  save();
  console.log(
    JSON.stringify({
      checkpoint: label,
      elapsedMs: checkpoint.elapsedMs,
      nativeCalls: totals.native.providers.calls,
    }),
  );
}
async function lifecycle(cycle) {
  await state("enable");
  await api("/api/plugins/source", { ...query, path: source });
  await action({ kind: "refresh" });
  const available = await view();
  assert.equal(available.total, 2);
  assert.equal(available.summary.totalTokens, "9007199254741000");
  await reconcile(`cycle-${cycle}-enabled`);
  await state("revoke-grants");
  const revoked = await view();
  assert(revoked.needsRefresh && !revoked.confirmedEmpty);
  assert.deepEqual(
    revoked.items,
    [],
    "Revoked history must not expose old rows",
  );
  const revokedDiagnostic = (await api("/api/plugins")).diagnostics.items.find(
    (p) => p.digest === query.digest,
  );
  assert.equal(revokedDiagnostic.snapshot.state, "awaiting-authorization");
  assert.equal(revokedDiagnostic.snapshot.records, null);
  assert.equal(revokedDiagnostic.snapshot.observedAt, null);
  const stale = await action(
    {
      kind: "read-session",
      sessionId: available.items[0].sessionId,
      snapshot: available.snapshot,
    },
    409,
  );
  assert.equal(stale.code, "stale_snapshot");
  assert.equal(
    (await action({ kind: "refresh" }, 403)).code,
    "permission_denied",
  );
  await reconcile(`cycle-${cycle}-revoked`);
  await api("/api/plugins/source", { ...query, path: source });
  await action({ kind: "refresh" });
  await state("disable");
  assert.equal(
    (await action({ kind: "refresh" }, 400)).code,
    "plugin_disabled",
  );
  await reconcile(`cycle-${cycle}-disabled`);
  await state("enable");
  const enabled = (await api("/api/plugins")).items.find(
    (p) => p.digest === query.digest,
  );
  assert(
    !enabled.sourceAuthorized,
    "Disable / enable must not inherit the old grant",
  );
  await api("/api/plugins/source", { ...query, path: source });
  await action({ kind: "refresh" });
  await reconcile(`cycle-${cycle}-rebound`);
}
async function loadWindow(seconds) {
  let done = false;
  const start = performance.now();
  const windowStart = writer ? { ...writer.committed } : null;
  const readRoutes = [
    ...Object.values(routes),
    "/api/runtime",
    "/api/snapshot",
    "/api/plugins",
  ];
  const workers = Array.from({ length: concurrency }, (_, worker) =>
    (async () => {
      let n = worker;
      while (!done && !fatal) {
        try {
          const route = readRoutes[n++ % readRoutes.length];
          const page = await api(route, undefined, "GET", 200, 8000, true);
          if (writer) {
            const name = Object.entries(routes).find(
              ([, value]) => value === route,
            )?.[0];
            if (name?.startsWith("gateway"))
              assert.deepEqual(
                stableStatistics({ [name]: page })[name],
                initialStatistics[name],
              );
            else if (name) {
              const checked = assertWritingPage(
                name,
                page,
                writeOptions,
                writer.committed,
              );
              report.validatedMovingReads =
                (report.validatedMovingReads ?? 0) + 1;
              const initial = ["claude", "codex"].includes(name)
                ? windowStart[name]
                : windowStart.claude + windowStart.codex;
              if (checked.extra > initial) {
                const published = (report.livePublishedReads ??= {});
                published[name] = (published[name] ?? 0) + 1;
              }
            }
          }
        } catch (error) {
          fatal = error;
          report.failures.push({
            at: new Date().toISOString(),
            worker,
            error: error.message,
          });
        }
        await delay(100);
      }
    })(),
  );
  startWriting();
  try {
    const deadline = performance.now() + seconds * 1000;
    while (performance.now() < deadline && !fatal) await delay(200);
  } finally {
    stopWriting();
    done = true;
    const issuingSeconds = Math.min(
      seconds,
      (performance.now() - start) / 1000,
    );
    await Promise.all(workers);
    activeLoadSeconds += issuingSeconds;
  }
  if (fatal) throw fatal;
}
try {
  save();
  sdk("keygen", key);
  const pkg = path.join(root, "package");
  cpSync("sdk/plugins/examples/aurora-agent/package", pkg, { recursive: true });
  const archive = path.join(root, "aurora.laplug");
  sdk("pack", pkg, archive, key);
  assert(JSON.parse(sdk("check", archive)).valid);
  await boot();
  for (const agentId of ["claude", "codex"]) {
    await api("/api/session-sources", { agentId, enabled: true }, "PUT");
    const deadline = Date.now() + 240000;
    for (;;) {
      const current = (await api("/api/snapshot")).sessionSources.find(
        (s) => s.agentId === agentId,
      );
      if (current.status === "ready") {
        assert.equal(current.sessionCount, files);
        break;
      }
      assert(
        Date.now() < deadline,
        `Source readiness deadline: ${JSON.stringify(current)}`,
      );
      await delay(50);
    }
  }
  const review = await api("/api/plugins/inspect", { path: archive });
  const installed = await api("/api/plugins/install", {
    packageRef: review.packageRef,
    digest: review.digest,
    approvePublisher: true,
  });
  query = { pluginId: installed.manifest.id, digest: installed.digest };
  const resource = () => {
    try {
      const disk = diskSample(root);
      const rssKiB =
        !samplingPaused &&
        child?.exitCode === null &&
        child?.signalCode === null
          ? Number(
              execFileSync("/bin/ps", ["-o", "rss=", "-p", String(child.pid)], {
                encoding: "utf8",
              }).trim(),
            )
          : 0;
      report.resources.push({
        at: new Date().toISOString(),
        generation,
        pid: child?.pid,
        rssBytes: rssKiB * 1024,
        ...disk,
      });
      assert(
        rssKiB * 1024 <= 2048 * 1024 ** 2,
        "Service exceeds existing 2 GiB memory budget",
      );
      assert(
        disk.volumeAvailableBytes > 1024 ** 3,
        "Less than 1 GiB free disk remains",
      );
    } catch (error) {
      fatal = error;
    }
  };
  resource();
  sampler = setInterval(resource, 5000);
  await reconcile("initial-history", true);
  report.initialStatistics = initialStatistics;
  if (fatal) throw fatal;
  report.soakStartedAt = new Date().toISOString();
  const duration = minutes * 60;
  let cycle = 0,
    nextRestart = duration / 3;
  while (activeLoadSeconds < duration) {
    cycle++;
    // Lifecycle operations and their exact reconciliation run concurrently with
    // all readers. A failed branch cannot leave unobserved background workers.
    const operations = await Promise.allSettled([
      loadWindow(Math.min(cycleSeconds, duration - activeLoadSeconds)),
      lifecycle(cycle).catch((error) => {
        fatal = error;
        throw error;
      }),
    ]);
    const rejected = operations.find((r) => r.status === "rejected");
    if (rejected) throw rejected.reason;
    report.lifecycleCycles = cycle;
    report.activeLoadSeconds = activeLoadSeconds;
    if (activeLoadSeconds >= nextRestart && report.restarts.length < 2) {
      const crash = report.restarts.length === 1;
      const before = (await api("/api/plugins")).items.find(
        (p) => p.digest === query.digest,
      );
      assert(before.enabled && before.sourceAuthorized);
      const beforeView = await view();
      await reconcile(`before-${crash ? "crash" : "graceful"}-restart`);
      const started = performance.now();
      await stop(crash);
      await boot();
      const listing = await api("/api/plugins");
      const resumed = listing.items.find((p) => p.digest === query.digest);
      assert(
        resumed.enabled && resumed.sourceAuthorized,
        "Persisted authorization changed at restart",
      );
      const diagnostic = listing.diagnostics.items.find(
        (p) => p.digest === query.digest,
      );
      assert.equal(diagnostic.activity, null);
      assert.equal(diagnostic.snapshot.state, "not-read");
      const unknown = await view();
      assert(unknown.needsRefresh && !unknown.confirmedEmpty);
      assert.equal(
        (
          await action(
            {
              kind: "read-session",
              sessionId: beforeView.items[0].sessionId,
              snapshot: beforeView.snapshot,
            },
            409,
          )
        ).code,
        "stale_snapshot",
      );
      warming = true;
      await reconcile(`after-${crash ? "crash" : "graceful"}-restart`, true);
      warming = false;
      await action({ kind: "refresh" });
      assert.equal((await view()).summary.totalTokens, "9007199254741000");
      report.restarts.push({
        kind: crash ? "SIGKILL" : "graceful",
        elapsedMs: performance.now() - started,
        installationAndGrantsPreserved: true,
        runtimeSnapshotResetToUnknown: true,
      });
      nextRestart += duration / 3;
    }
    save();
    console.log(
      JSON.stringify({
        progress: true,
        activeLoadSeconds,
        cycles: cycle,
        requests: Object.values(report.routes).reduce(
          (n, r) => n + r.requests,
          0,
        ),
      }),
    );
  }
  await state("revoke-grants");
  await reconcile("final-revoked-before-restart");
  await stop();
  await boot();
  const revokedRestart = (await api("/api/plugins")).items.find(
    (p) => p.digest === query.digest,
  );
  assert(revokedRestart.enabled && !revokedRestart.sourceAuthorized);
  const revokedView = await view();
  assert(revokedView.needsRefresh && !revokedView.confirmedEmpty);
  assert.deepEqual(revokedView.items, []);
  assert.equal(
    (await action({ kind: "refresh" }, 403)).code,
    "permission_denied",
  );
  warming = true;
  await reconcile("final-revoked-after-restart");
  warming = false;
  report.restarts.push({
    kind: "graceful-revoked",
    revokedAuthorizationStayedRevoked: true,
  });
  await state("disable");
  await stop(true);
  await boot();
  const disabledRestart = (await api("/api/plugins")).items.find(
    (p) => p.digest === query.digest,
  );
  assert(!disabledRestart.enabled && !disabledRestart.sourceAuthorized);
  assert.equal(
    (await action({ kind: "refresh" }, 400)).code,
    "plugin_disabled",
  );
  warming = true;
  await reconcile("final-revoked-disabled", true);
  warming = false;
  report.restarts.push({
    kind: "SIGKILL-disabled",
    disabledStayedDisabled: true,
  });
  if (writer) {
    report.writes = { ...report.writes, ...writer.verify() };
    assert(overlappingWrites > 0, "No source writes overlapped active readers");
    if (report.profile === "soak")
      for (const name of [
        "overview",
        "providers",
        "claude",
        "codex",
        "sessions",
        "projects",
        "requests",
      ])
        assert(
          report.livePublishedReads?.[name] > 0,
          `${name}: no new generation was read during continuous writing`,
        );
    assert.equal(
      createHash("sha256").update(readFileSync(source)).digest("hex"),
      initialPluginDigest,
    );
    report.sourceWritesMatchLedger = true;
    delete report.initialNativePages;
  } else
    assert.equal(sourceDigest(), initialDigest, "Source file bytes changed");
  assert.equal(
    rawHistoryDigest(),
    initialGatewayDigest,
    "Gateway history changed",
  );
  await stop();
  report.integrity = {};
  for (const file of [
    "local-agent.sqlite3",
    "plugin-host.sqlite3",
    "observation/observations.sqlite3",
  ]) {
    const checked = new DatabaseSync(path.join(data, file), { readOnly: true });
    try {
      assert.equal(
        checked.prepare("PRAGMA integrity_check").get().integrity_check,
        "ok",
        file,
      );
      report.integrity[file] = {
        integrity: "ok",
        userVersion: checked.prepare("PRAGMA user_version").get().user_version,
      };
    } finally {
      checked.close();
    }
  }
  assert.equal(report.integrity["local-agent.sqlite3"].userVersion, 2);
  assert(peakActive >= concurrency, "Requested concurrency was not exercised");
  assert(
    peakConcurrent >= concurrency,
    "Requested reader concurrency was not exercised",
  );
  report.peakConcurrentReaders = peakConcurrent;
  report.peakActiveRequests = peakActive;
  report.activeLoadSeconds = activeLoadSeconds;
  report.soakEndedAt = new Date().toISOString();
  report.sourceFilesUnchanged = !writer;
  report.gatewayHistoryUnchanged = true;
  report.complete = true;
} catch (error) {
  report.error = error.stack;
  throw error;
} finally {
  stopWriting();
  clearInterval(sampler);
  await stop().catch((error) => {
    report.cleanupError = error.message;
  });
  rmSync(key, { force: true });
  report.temporarySigningKeyRemoved = true;
  report.processesExited =
    !child || child.exitCode !== null || child.signalCode !== null;
  report.activeLoadSeconds = activeLoadSeconds;
  report.peakActiveRequests = peakActive;
  save();
  // Keep failed fixtures for diagnosis; successful generated sources and derived
  // data are disposable. Reports and raw logs remain outside the scratch root.
  if (report.complete) rmSync(root, { recursive: true, force: true });
}
