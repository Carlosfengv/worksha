import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn, execFileSync } from "node:child_process";
import { createInterface } from "node:readline";
import { once } from "node:events";
import { createHash, randomUUID } from "node:crypto";
import { DatabaseSync } from "node:sqlite";

// Owned offline fixtures only. Long-run success is separate from phase acceptance,
// native-window verification, comparative performance and million-history gates.
const option = (name, fallback) => {
  const index = process.argv.indexOf(name);
  return index < 0 ? fallback : process.argv[index + 1];
};
const binary = fs.realpathSync(
  option("--binary", "target/debug/local-agent-service"),
);
const reportFile = path.resolve(
  option("--report", ".work/domain-stability.json"),
);
const seconds = Number(option("--seconds", "1800"));
const minimumCycles = Number(option("--cycles", "100"));
const concurrency = Number(option("--concurrency", "6"));
const nativeCalls = Number(option("--native-calls", "10000"));
assert(Number.isFinite(seconds) && seconds > 0 && seconds <= 86400);
assert(
  Number.isInteger(minimumCycles) &&
    minimumCycles > 0 &&
    minimumCycles <= 10000,
);
assert(Number.isInteger(concurrency) && concurrency >= 2 && concurrency <= 16);
assert(
  Number.isInteger(nativeCalls) && nativeCalls > 0 && nativeCalls <= 1000000,
);
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "local-agent-domain-stability-")),
);
const home = path.join(root, "home"),
  data = path.join(root, "data");
fs.mkdirSync(home);
const sha = (b) => createHash("sha256").update(b).digest("hex");
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const report = {
  complete: false,
  phaseAccepted: false,
  root,
  binarySha256: sha(fs.readFileSync(binary)),
  harnessSha256: sha(fs.readFileSync(import.meta.filename)),
  requestedSeconds: seconds,
  minimumCycles,
  concurrency,
  nativeCalls,
  externalResponses: 83,
  externalBodyEvents: 120,
  realModelCalls: 0,
  realKeychainCalls: 0,
  routes: {},
  resources: [],
  cycles: 0,
  activeLoadSeconds: 0,
  restarts: [],
  failures: [],
  expectedRejections: 0,
  resourceScope:
    "Service RSS and sampled open files/TCP descriptors and children; sampling cannot exclude transient workers or prove no worker-start attempts.",
  loadScope:
    "Offline dense native history, complete history graph, independent session/response units, scoped frozen requests, paged body, failed refresh, display/resource revoke, reselect, disable/enable and graceful/crash restart. No live gateway forwards or million-history/comparative performance claim.",
};
fs.mkdirSync(path.dirname(reportFile), { recursive: true });
const save = () =>
  fs.writeFileSync(reportFile, JSON.stringify(report, null, 2) + "\n");
let child,
  base,
  token,
  fatal,
  active = 0,
  peakActive = 0,
  readers = [],
  reading = false,
  sampler;
let query, sourceId;
let transition = 0,
  mutating = false;
const sdk = (...args) =>
  execFileSync(binary, ["--plugin-sdk", ...args], {
    encoding: "utf8",
    timeout: 30000,
  });
async function stop(signal = "SIGTERM") {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const done = once(child, "exit");
  child.kill(signal);
  let timer;
  try {
    await Promise.race([
      done,
      new Promise((_, reject) => {
        timer = setTimeout(() => {
          child.kill("SIGKILL");
          reject(Error("Owned service stop deadline"));
        }, 10000);
      }),
    ]);
  } finally {
    clearTimeout(timer);
    if (child.exitCode === null && child.signalCode === null) {
      child.kill("SIGKILL");
      await done;
    }
  }
}
async function boot() {
  token = randomUUID();
  child = spawn(binary, ["--data-dir", data], {
    env: {
      ...process.env,
      LOCAL_AGENT_SERVICE_TOKEN: token,
      LOCAL_AGENT_SOURCE_HOME: home,
      LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
      LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stderr.on("data", (b) =>
    fs.appendFileSync(`${reportFile}.service.log`, b),
  );
  const lines = createInterface({ input: child.stdout });
  const [line] = await Promise.race([
    once(lines, "line", { signal: AbortSignal.timeout(30000) }),
    once(child, "exit").then(() => {
      throw Error("Service exited before ready");
    }),
  ]);
  base = `http://127.0.0.1:${JSON.parse(line).port}`;
  (report.boots ??= []).push({ pid: child.pid, at: new Date().toISOString() });
}
async function api(
  route,
  body,
  expected = 200,
  method = body === undefined ? "GET" : "POST",
  allowTransition = false,
) {
  const startTransition = transition;
  const start = performance.now();
  active++;
  peakActive = Math.max(peakActive, active);
  try {
    const response = await fetch(base + route, {
      method,
      headers: {
        authorization: `Bearer ${token}`,
        "content-type": "application/json",
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(15000),
    });
    const text = await response.text();
    let value;
    try {
      value = JSON.parse(text);
    } catch {
      throw Error(`${route}: HTTP ${response.status}: ${text.slice(0, 1000)}`);
    }
    const ms = performance.now() - start;
    const key = `${method} ${route.split("?")[0]} ${expected === 200 ? "read-or-success" : "expected-denial"}`;
    const metric = (report.routes[key] ??= {
      samples: 0,
      statuses: {},
      latencyBuckets: {},
      maxMs: 0,
    });
    metric.samples++;
    metric.statuses[response.status] =
      (metric.statuses[response.status] ?? 0) + 1;
    const bucket = 2 ** (Math.ceil(Math.log2(Math.max(1, ms)) * 8) / 8);
    metric.latencyBuckets[bucket] = (metric.latencyBuckets[bucket] ?? 0) + 1;
    metric.maxMs = Math.max(metric.maxMs, ms);
    if (
      allowTransition &&
      response.status === 409 &&
      value.code === "stale_snapshot" &&
      (mutating || startTransition !== transition || body?.snapshotToken)
    ) {
      const field = body?.snapshotToken
        ? "frozenCacheRejections"
        : "concurrentEpochRejections";
      report[field] = (report[field] ?? 0) + 1;
      return null;
    }
    assert.equal(
      response.status,
      expected,
      `${route}: ${JSON.stringify(value)}`,
    );
    if (expected !== 200) report.expectedRejections++;
    return value;
  } finally {
    active--;
  }
}
const external = () => ({ kind: "external", sourceIds: [sourceId] });
const builtin = { kind: "builtin", sourceIds: ["claude"] };
const facts = (kind = "response", scope = external()) => ({
  scope,
  kind,
  projectId: null,
  sessionId: null,
  responseId: null,
  snapshotToken: null,
  cursor: null,
});
const sessions = () => ({
  scope: external(),
  filter: { query: null, productId: null, days: null },
  snapshotToken: null,
  cursor: null,
});
const requests = () => ({
  scope: external(),
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
const state = (action) =>
  api("/api/plugin-sources/state", { package: query, action });
async function consent(enabled) {
  const review = await api("/api/domain/consent-review", query);
  return api("/api/domain/consent", {
    ...query,
    enabled,
    persistMetadata: false,
    reviewRef: review.reviewRef,
  });
}
const source = path.join(root, "history.jsonl");
let validSource;
async function grant() {
  await api("/api/plugin-sources/source", { package: query, path: source });
  await api("/api/plugin-sources/refresh", query);
  await consent(true);
}
async function bodyPage(page, expected = 200) {
  return api(
    "/api/domain/fact-conversation",
    {
      sessionId: page.items[0].id,
      snapshotToken: page.snapshotToken,
      cursor: null,
      anchor: null,
    },
    expected,
  );
}
async function lifecycle() {
  await state("enable");
  await grant();
  const page = await api("/api/domain/facts", facts("session"));
  const responses = await api("/api/domain/facts", facts());
  assert.equal(responses.sourceSummaries[0].responses.records, "83");
  let body = (await bodyPage(page)).conversation,
    events = body.events.length;
  while (body.nextCursor) {
    body = (
      await api("/api/domain/fact-conversation", {
        sessionId: page.items[0].id,
        snapshotToken: page.snapshotToken,
        cursor: body.nextCursor,
        anchor: null,
      })
    ).conversation;
    events += body.events.length;
  }
  assert.equal(events, 120);
  const requestQuery = requests(),
    frozen = await api("/api/domain/requests", requestQuery);
  assert.equal(frozen.sourceSummaries[0].records, "83");
  const invalid = validSource.replace(
    '"sessionRef":"s1"',
    '"sessionRef":"missing-session"',
  );
  assert.notEqual(invalid, validSource);
  fs.writeFileSync(source, invalid);
  try {
    assert.equal(
      (await api("/api/plugin-sources/refresh", query, 400)).code,
      "invalid_record",
    );
    const retained = await api("/api/domain/facts", facts());
    assert.equal(retained.sourceSummaries[0].responses.records, "83");
    const status = (await api("/api/plugin-sources")).items.find(
      (p) => p.digest === query.digest,
    );
    assert.equal(status.freshness, "failed");
  } finally {
    fs.writeFileSync(source, validSource);
  }
  await api("/api/plugin-sources/refresh", query);
  const beforeRevoke = await api("/api/domain/facts", facts("session"));
  await consent(false);
  await bodyPage(beforeRevoke, 409);
  await api(
    "/api/domain/requests",
    { ...requestQuery, snapshotToken: frozen.snapshotToken },
    409,
  );
  assert.equal((await api("/api/domain/facts", facts())).items.length, 0);
  const displayRevoked = (await api("/api/plugin-sources")).items.find(
    (p) => p.digest === query.digest,
  );
  assert(displayRevoked.sourceAuthorized && !displayRevoked.unifiedAuthorized);
  await consent(true);
  const beforeReselect = await api("/api/domain/facts", facts("session"));
  await api("/api/plugin-sources/source", { package: query, path: source });
  await bodyPage(beforeReselect, 409);
  await api("/api/plugin-sources/refresh", query);
  await state("revoke-grants");
  assert.equal(
    (await api("/api/plugin-sources/refresh", query, 403)).code,
    "permission_denied",
  );
  assert.equal((await api("/api/domain/facts", facts())).items.length, 0);
  await state("disable");
  const enabled = await state("enable");
  assert(
    enabled.runtimeActive &&
      !enabled.sourceAuthorized &&
      !enabled.unifiedAuthorized,
  );
  await grant();
  report.cycles++;
}
function resource(label) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const rss =
    Number(
      execFileSync("/bin/ps", ["-o", "rss=", "-p", String(child.pid)], {
        encoding: "utf8",
      }).trim(),
    ) * 1024;
  const processRows = execFileSync("/bin/ps", ["-axo", "pid=,ppid=,command="], {
    encoding: "utf8",
  }).split("\n");
  const children = processRows
    .filter((line) => Number(line.trim().split(/\s+/)[1]) === child.pid)
    .map((line) => line.trim());
  const descriptors = execFileSync(
    "/usr/sbin/lsof",
    ["-nP", "-FfPtnT", "-p", String(child.pid)],
    { encoding: "utf8", timeout: 5000 },
  ).split("\n");
  const files = [];
  let current;
  for (const field of descriptors) {
    if (/^f\d/.test(field)) {
      current = {};
      files.push(current);
    } else if (current && field.startsWith("P"))
      current.protocol = field.slice(1);
    else if (current && field.startsWith("TST="))
      current.state = field.slice(4);
  }
  const tcpStates = {};
  for (const file of files.filter((f) => f.protocol === "TCP")) {
    const state = file.state ?? "unknown";
    tcpStates[state] = (tcpStates[state] ?? 0) + 1;
  }
  report.resources.push({
    label,
    at: new Date().toISOString(),
    pid: child.pid,
    cycles: report.cycles,
    rssBytes: rss,
    numericFileDescriptors: files.length,
    tcpStates,
    children,
  });
  assert(rss <= 2 * 1024 ** 3, "Service exceeds 2 GiB");
  if (label === "quiet-final")
    assert.equal(children.length, 0, "Owned child remains after drain");
}
function startReaders() {
  reading = true;
  // Reuse a page's frozen view while its source is unchanged. Continually
  // creating new views would deliberately exhaust the existing eight-view
  // budget and evict a reader's own paged body, rather than model normal use.
  const cache = new Map();
  const read = (route, body) => {
    if (!body || !("snapshotToken" in body))
      return api(route, body, 200, body === undefined ? "GET" : "POST", true);
    const key = JSON.stringify([route, body]);
    let entry = cache.get(key);
    if (!entry) {
      entry = { token: null, pending: null };
      cache.set(key, entry);
    }
    if (!entry.pending)
      entry.pending = api(
        route,
        { ...body, snapshotToken: entry.token },
        200,
        "POST",
        true,
      )
        .then((page) => {
          entry.token = page?.snapshotToken ?? null;
          return page;
        })
        .finally(() => {
          entry.pending = null;
        });
    return entry.pending;
  };
  const jobs = [
    () => read("/api/domain/facts", facts()),
    () => read("/api/domain/sessions", sessions()),
    () =>
      read("/api/domain/activity", {
        scope: external(),
        rangeSeconds: 86400,
        productId: null,
        sessionId: null,
        includeGateway: false,
        includeHistorical: true,
      }),
    () => read("/api/domain/directory"),
    async () => {
      const page = await read("/api/domain/facts", facts("response", builtin));
      if (!page) return;
      assert.equal(
        page.sourceSummaries[0].responses.records,
        String(nativeCalls),
      );
    },
    () => api("/api/sessions"),
    () => api("/api/provider-usage?source=native&providerId=_unknown"),
    () => api("/api/snapshot"),
  ];
  readers = Array.from({ length: concurrency }, (_, n) =>
    (async () => {
      while (reading && !fatal) {
        try {
          await jobs[n++ % jobs.length]();
        } catch (error) {
          fatal = error;
          report.failures.push({
            at: new Date().toISOString(),
            error: error.stack,
          });
        }
        await delay(100);
      }
    })(),
  );
}
async function drain() {
  reading = false;
  await Promise.all(readers);
  if (fatal) throw fatal;
}
try {
  save();
  const nativeDir = path.join(home, ".claude/projects/fixture");
  fs.mkdirSync(nativeDir, { recursive: true });
  const nativeFile = path.join(nativeDir, "history.jsonl");
  const nativeRows = [
    JSON.stringify({
      type: "user",
      sessionId: "dense-fixture",
      uuid: "u1",
      cwd: path.join(root, "workspace"),
      timestamp: "2026-10-31T10:00:00Z",
      message: { role: "user", content: "offline fixture" },
    }),
  ];
  for (let i = 0; i < nativeCalls; i++)
    nativeRows.push(
      JSON.stringify({
        type: "assistant",
        sessionId: "dense-fixture",
        uuid: `a${i}`,
        timestamp: "2026-10-31T10:00:01Z",
        message: {
          id: `r${i}`,
          model: "fixture",
          content: [{ type: "text", text: "offline" }],
          usage: { input_tokens: 10, output_tokens: 2 },
        },
      }),
    );
  fs.writeFileSync(nativeFile, nativeRows.join("\n") + "\n");
  const nativeSha = sha(fs.readFileSync(nativeFile));
  const key = path.join(root, "publisher.key"),
    archive = path.join(root, "full.laplug");
  sdk("keygen", key);
  sdk(
    "pack",
    path.resolve("sdk/plugins/examples/full-history/package"),
    archive,
    key,
  );
  const manifest = JSON.parse(sdk("check", archive)).manifest;
  const usage = {
    inputTokens: "10",
    outputTokens: "2",
    cachedInputTokens: null,
    cacheWriteInputTokens: null,
    reasoningOutputTokens: null,
    totalTokens: "12",
  };
  const rows = [
    {
      fact: {
        kind: "project",
        value: {
          nativeId: "p1",
          name: "fixture",
          pathHint: path.join(root, "never-authorized"),
        },
      },
    },
    {
      fact: {
        kind: "session",
        value: {
          nativeId: "s1",
          productIds: [`${manifest.id}:terminal`],
          unknownClient: false,
          title: "fixture",
          createdAt: null,
          updatedAt: null,
          projectRef: "p1",
          usage,
          partial: false,
        },
      },
      conversation: Array.from({ length: 120 }, (_, i) => ({
        id: `e${i}`,
        nativeId: `e${i}`,
        turnId: `t${i}`,
        kind: "message",
        role: "assistant",
        status: "recorded",
        text: `fixture ${i}`,
        input: null,
        output: null,
        model: null,
        responseId: null,
        usage: null,
      })),
    },
    ...Array.from({ length: 83 }, (_, i) => ({
      fact: {
        kind: "response",
        value: {
          nativeId: `r${i}`,
          sessionRef: "s1",
          productId: `${manifest.id}:terminal`,
          occurredAt: null,
          model: "fixture",
          outcome: "success",
          usage,
          reportedResponseId: "untrusted",
          durationMs: "12",
        },
      },
    })),
    {
      fact: {
        kind: "activity",
        value: {
          nativeId: "a1",
          sessionRef: "s1",
          responseRef: "r0",
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
          responseRef: "r0",
          relativePath: "src/example.txt",
          occurredAt: null,
          outcome: "success",
        },
      },
    },
    {
      fact: {
        kind: "mcp-catalog",
        value: { nativeId: "m1", serverName: "fixture", toolNames: ["lookup"] },
      },
    },
    {
      fact: {
        kind: "mcp-execution",
        value: {
          nativeId: "x1",
          serverRef: "m1",
          sessionRef: "s1",
          responseRef: "r0",
          toolName: "lookup",
          occurredAt: null,
          durationMs: null,
          outcome: "unknown",
        },
      },
    },
  ];
  validSource = rows.map((r) => JSON.stringify(r) + "\n").join("");
  fs.writeFileSync(source, validSource);
  report.sourceSha256 = sha(Buffer.from(validSource));
  report.nativeSha256 = nativeSha;
  await boot();
  await api(
    "/api/session-sources",
    { agentId: "claude", enabled: true },
    200,
    "PUT",
  );
  const deadline = Date.now() + 240000;
  for (;;) {
    const status = (await api("/api/snapshot")).sessionSources.find(
      (s) => s.agentId === "claude",
    );
    if (status.status === "ready") break;
    assert(Date.now() < deadline, "Dense native source readiness deadline");
    await delay(100);
  }
  const review = await api("/api/plugin-sources/inspect", { path: archive });
  const installed = await api("/api/plugin-sources/install", {
    packageRef: review.packageRef,
    digest: review.digest,
    approvePublisher: true,
    expectedSigningKey: review.signingKey,
  });
  query = { pluginId: installed.manifest.id, digest: installed.digest };
  sourceId = `${manifest.id}:history`;
  await state("enable");
  await grant();
  resource("baseline");
  sampler = setInterval(() => {
    try {
      resource("load");
      save();
    } catch (error) {
      fatal = error;
    }
  }, 15000);
  report.startedAt = new Date().toISOString();
  let restartAt = 1;
  while (report.activeLoadSeconds < seconds || report.cycles < minimumCycles) {
    startReaders();
    const start = performance.now();
    try {
      transition++;
      mutating = true;
      try {
        await lifecycle();
      } finally {
        mutating = false;
        transition++;
      }
      const windowSeconds = Math.min(
        18,
        Math.max(0, seconds - report.activeLoadSeconds),
      );
      while ((performance.now() - start) / 1000 < windowSeconds && !fatal)
        await delay(100);
    } finally {
      await drain();
      report.activeLoadSeconds += (performance.now() - start) / 1000;
    }
    if (fatal) throw fatal;
    if (
      restartAt <= 2 &&
      report.activeLoadSeconds >= (seconds * restartAt) / 3
    ) {
      const signal = restartAt === 1 ? "SIGTERM" : "SIGKILL";
      clearInterval(sampler);
      await stop(signal);
      await boot();
      const restored = (await api("/api/plugin-sources")).items.find(
        (p) => p.digest === query.digest,
      );
      assert(
        restored.enabled &&
          !restored.runtimeActive &&
          !restored.sourceAuthorized &&
          !restored.unifiedAuthorized,
      );
      await state("enable");
      await grant();
      report.restarts.push({
        signal,
        grantsNotRestored: true,
        atCycle: report.cycles,
      });
      restartAt++;
      sampler = setInterval(() => {
        try {
          resource("load");
          save();
        } catch (error) {
          fatal = error;
        }
      }, 15000);
    }
    save();
    console.log(
      JSON.stringify({
        cycles: report.cycles,
        activeLoadSeconds: report.activeLoadSeconds,
        failures: report.failures.length,
      }),
    );
  }
  clearInterval(sampler);
  await state("revoke-grants");
  await state("disable");
  await delay(1200);
  resource("quiet-final");
  assert.equal(sha(fs.readFileSync(source)), report.sourceSha256);
  assert.equal(sha(fs.readFileSync(nativeFile)), nativeSha);
  assert(!fs.existsSync(path.join(root, "never-authorized")));
  assert.equal(active, 0);
  report.unfinishedRequests = active;
  report.peakActiveRequests = peakActive;
  await stop();
  report.integrity = {};
  for (const name of [
    "local-agent.sqlite3",
    "plugin-host.sqlite3",
    "plugin-source-host.sqlite3",
    "observation/observations.sqlite3",
  ]) {
    const db = new DatabaseSync(path.join(data, name), { readOnly: true });
    try {
      assert.equal(
        db.prepare("PRAGMA integrity_check").get().integrity_check,
        "ok",
      );
      report.integrity[name] = "ok";
    } finally {
      db.close();
    }
  }
  report.sourceFilesUnchanged = true;
  report.longRunThresholdMet =
    report.activeLoadSeconds >= 1800 && report.cycles >= 100;
  report.complete = true;
  report.endedAt = new Date().toISOString();
} catch (error) {
  report.error = error.stack;
  throw error;
} finally {
  reading = false;
  await Promise.allSettled(readers);
  clearInterval(sampler);
  await stop().catch((error) => {
    report.cleanupError = error.message;
  });
  report.processesExited =
    !child || child.exitCode !== null || child.signalCode !== null;
  for (const metric of Object.values(report.routes)) {
    const buckets = Object.entries(metric.latencyBuckets)
      .map(([ms, count]) => [Number(ms), count])
      .sort((a, b) => a[0] - b[0]);
    for (const q of [50, 95, 99]) {
      let n = 0;
      metric[`p${q}UpperBoundMs`] =
        buckets.find(
          ([, count]) => (n += count) >= Math.ceil((metric.samples * q) / 100),
        )?.[0] ?? null;
    }
  }
  if (report.complete && report.processesExited) {
    fs.rmSync(root, { recursive: true, force: true });
    report.temporaryDataRemoved = true;
  }
  save();
}
