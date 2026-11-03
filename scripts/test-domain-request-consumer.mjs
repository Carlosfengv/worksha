// Same-data legacy IPC comparison for the real request projection consumer.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { randomUUID, createHash } from "node:crypto";
import { DatabaseSync } from "node:sqlite";

const binary = path.resolve(
  process.env.DOMAIN_REQUEST_BINARY ?? "target/debug/local-agent-service",
);
const baseline = process.env.DOMAIN_REQUEST_BASELINE_BINARY
  ? path.resolve(process.env.DOMAIN_REQUEST_BASELINE_BINARY)
  : null;
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "local-agent-domain-request-consumer-")),
);
const home = path.join(root, "home"),
  data = path.join(root, "data"),
  project = path.join(home, "owned-workspace");
fs.mkdirSync(project, { recursive: true });
const source = path.join(home, ".claude/projects/fixture/history.jsonl");
fs.mkdirSync(path.dirname(source), { recursive: true });
const at = "2026-10-19T01:00:00Z";
const native = Array.from({ length: 48 }, (_, i) => ({
  type: "assistant",
  sessionId: "native-fixture",
  uuid: `line-${i}`,
  entrypoint: "cli",
  cwd: project,
  timestamp: i === 47 ? undefined : at,
  message: {
    id: i === 2 ? "ambiguous" : `response-${i}`,
    model: "deepseek-flash",
    content: [{ type: "text", text: "owned native response" }],
    usage: {
      input_tokens: i === 1 ? 99 : 10,
      cache_read_input_tokens: 20,
      cache_creation_input_tokens: 5,
      output_tokens: 2,
    },
  },
}));
fs.writeFileSync(source, native.map((r) => JSON.stringify(r) + "\n").join(""));
fs.mkdirSync(path.join(data, "observation"), { recursive: true });
const db = new DatabaseSync(
  path.join(data, "observation/observations.sqlite3"),
);
db.exec(
  "CREATE TABLE model_observations(id TEXT PRIMARY KEY,started_at TEXT NOT NULL,revision INTEGER NOT NULL,value TEXT NOT NULL)",
);
const insert = db.prepare("INSERT INTO model_observations VALUES(?,?,?,?)");
const observations = Array.from({ length: 87 }, (_, i) => ({
  id: `gateway-${String(i).padStart(3, "0")}`,
  logicalId: i < 3 ? "retry-fixture" : null,
  agentId: "claude",
  agentProductId: "claude-code-cli",
  sessionId: "native-fixture",
  responseId: i === 3 || i === 4 ? "ambiguous" : `response-${i}`,
  model: "deepseek-flash",
  requestedModel: "requested-alias",
  routedModel: "deepseek-flash",
  reportedModel: "deepseek-flash",
  provider: "Fixture supplier",
  providerId: "fixture-provider",
  endpoint: "https://api.deepseek.com/v1",
  bindingRevision: 1,
  startedAt: new Date(Date.parse(at) + i * 2000).toISOString(),
  endedAt: new Date(Date.parse(at) + i * 2000 + 1000).toISOString(),
  clockId: "fixture-clock",
  startTickMs: i * 2000,
  status: i < 2 ? "error" : "success",
  httpStatus: i < 2 ? 429 : 200,
  ttftMs: i < 2 ? null : 100,
  firstTextMs: i < 2 ? null : 150,
  durationMs: 1000,
  usage: {
    inputTokens: 35,
    outputTokens: 2,
    totalTokens: 37,
    cachedInputTokens: 20,
    cacheWriteInputTokens: 5,
  },
  protocol: "messages",
  timingIssue: null,
  revision: 1,
}));
for (const row of observations)
  insert.run(row.id, row.startedAt, row.revision, JSON.stringify(row));
db.close();
const sha = (p) =>
  createHash("sha256").update(fs.readFileSync(p)).digest("hex");
const originalSource = sha(source);
const report = {
  complete: false,
  binarySha256: sha(binary),
  baselineSha256: baseline ? sha(baseline) : null,
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
};
let child, base, token;
async function stop() {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const done = once(child, "exit");
  child.kill();
  let timer;
  try {
    await Promise.race([
      done,
      new Promise((_, reject) => {
        timer = setTimeout(() => {
          child.kill("SIGKILL");
          reject(Error("owned service did not exit"));
        }, 8000);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}
async function boot(executable = binary) {
  token = randomUUID();
  let stderr = "";
  child = spawn(executable, ["--data-dir", data], {
    env: {
      ...process.env,
      LOCAL_AGENT_SERVICE_TOKEN: token,
      LOCAL_AGENT_SOURCE_HOME: home,
      LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
      LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stderr.on("data", (b) => {
    stderr = (stderr + b).slice(-4000);
  });
  const reader = createInterface({ input: child.stdout });
  const [line] = await Promise.race([
    once(reader, "line", { signal: AbortSignal.timeout(20000) }),
    once(child, "exit").then(() => {
      throw Error(stderr);
    }),
  ]);
  base = `http://127.0.0.1:${JSON.parse(line).port}`;
}
async function api(route, body, expected = 200) {
  const r = await fetch(base + route, {
    method: body ? "PUT" : "GET",
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
    body: body ? JSON.stringify(body) : undefined,
    signal: AbortSignal.timeout(20000),
  });
  const text = await r.text();
  assert.equal(r.status, expected, `${route}: ${text}`);
  return JSON.parse(text);
}
const fixed = { start: "2026-10-18T00:00:00Z", end: "2026-10-20T00:00:00Z" };
const page = (q) => api("/api/request-dashboard?" + new URLSearchParams(q));
function stable(value) {
  value = structuredClone(value);
  // An unbounded query ends its final bucket at this response's observation
  // clock. Preserve fixed windows and every recorded event time verbatim.
  if (value?.query && !value.query.end && Array.isArray(value.buckets)) {
    const last = value.buckets.at(-1);
    if (last?.end === value.asOf) last.end = "<open-window-observation-time>";
  }
  function visit(v) {
    if (!v || typeof v !== "object") return;
    for (const k of ["asOf", "sourceAsOf", "lastSuccessfulScan"])
      if (Object.hasOwn(v, k))
        v[k] = v[k] === null ? null : "<host-observation-time>";
    for (const k of ["snapshot", "nextCursor", "cursor"])
      if (typeof v[k] === "string") v[k] = "<process-bound-token>";
    Object.values(v).forEach(visit);
  }
  visit(value);
  return value;
}
async function enable() {
  await api("/api/session-sources", { agentId: "claude", enabled: false });
  await api("/api/session-sources", { agentId: "claude", enabled: true });
  for (let i = 0; i < 150; i++) {
    const p = await page({ source: "native" });
    if (p.total === 48) return;
    await new Promise((r) => setTimeout(r, 40));
  }
  throw Error("native fixture did not publish");
}
async function capture() {
  await enable();
  const all = await page({ source: "native" });
  assert.equal(all.total, 48);
  assert.equal(all.missingTime, 1);
  const selected = [];
  for (const source of ["gateway", "native", "linked"]) {
    for (const sort of ["time", "ttft", "duration", "tokens", "cost"]) {
      const q = {
        ...fixed,
        source,
        sort,
        descending: "false",
        metric: "tokens",
        dimension: "model",
        currency: "USD",
      };
      const first = await page(q);
      selected.push(stable(first));
      const ids = first.items.map((r) => r.id);
      for (let cursor = first.nextCursor; cursor;) {
        const current = await page({ ...q, snapshot: first.snapshot, cursor });
        assert.deepEqual(
          stable(await page({ ...q, snapshot: first.snapshot, cursor })),
          stable(current),
        );
        assert.deepEqual(current.summary, first.summary);
        ids.push(...current.items.map((r) => r.id));
        selected.push(stable(current));
        cursor = current.nextCursor;
      }
      assert.equal(ids.length, first.total);
      assert.equal(new Set(ids).size, ids.length);
      selected.push(
        stable(
          await api(
            "/api/request-export?snapshot=" +
              encodeURIComponent(first.snapshot),
          ),
        ),
      );
    }
  }
  const gateway = await page({ ...fixed, source: "gateway" });
  assert.equal(gateway.total, 85);
  assert.equal(gateway.summary.attempts, 87);
  assert.equal(gateway.summary.statuses.recovered, 1);
  assert(gateway.sourceReport.matchedNative > 0);
  assert(gateway.sourceReport.ambiguousNative > 0);
  assert(gateway.sourceReport.usageConflicts > 0);
  for (const row of gateway.items)
    selected.push(
      stable(await api("/api/request-detail?id=" + encodeURIComponent(row.id))),
    );
  const n = await page({ source: "native" });
  const nativeRows = [...n.items];
  for (let cursor = n.nextCursor; cursor;) {
    const next = await page({ source: "native", snapshot: n.snapshot, cursor });
    assert.deepEqual(next.summary, n.summary);
    selected.push(stable(next));
    nativeRows.push(...next.items);
    cursor = next.nextCursor;
  }
  assert.equal(nativeRows.length, 48);
  assert.equal(new Set(nativeRows.map((r) => r.id)).size, 48);
  const unknown = nativeRows.filter((r) => !r.startedAt);
  assert.equal(unknown.length, 1);
  assert.equal(unknown[0].ttftMs, null);
  assert.equal(unknown[0].durationMs, null);
  for (const row of nativeRows)
    selected.push(
      stable(await api("/api/request-detail?id=" + encodeURIComponent(row.id))),
    );
  selected.push(stable(n));
  for (const q of [
    {
      ...fixed,
      source: "gateway",
      model: "deepseek-flash",
      result: "recovered",
    },
    {
      ...fixed,
      source: "gateway",
      provider: "fixture-provider",
      search: "Fixture supplier",
    },
    { ...fixed, source: "native", issue: "unmeasurable" },
    { ...fixed, source: "linked", currency: "CNY", metric: "cost" },
  ])
    selected.push(stable(await page(q)));
  const nativeFrozen = await page({ source: "native" });
  await api("/api/session-sources", { agentId: "claude", enabled: false });
  await api(
    "/api/request-dashboard?" +
      new URLSearchParams({
        source: "native",
        snapshot: nativeFrozen.snapshot,
      }),
    undefined,
    409,
  );
  assert.equal((await page({ source: "native" })).total, 0);
  assert.equal((await page({ source: "gateway" })).total, 85);
  assert.equal(sha(source), originalSource);
  return selected;
}
try {
  await boot();
  await enable();
  await api("/api/session-sources", { agentId: "claude", enabled: false });
  await stop(); // every compared process starts with the same disabled intent
  await boot();
  const current = await capture();
  await stop();
  if (baseline) {
    await boot(baseline);
    assert.deepEqual(await capture(), current);
    await stop();
    await boot();
    assert.deepEqual(await capture(), current);
    await stop();
  }
  report.cases.push(
    "native responses, gateway logical requests and linked requests retain independent units, conflict/ambiguity evidence, original details, billing and unknown time",
    "five sorts, three source scopes, complete 40-row pagination and replay, full-scope summaries, filters and frozen CSV keep their original response values",
    "source revocation invalidates frozen native reads without hiding gateway history; original native source bytes are unchanged",
  );
  if (baseline)
    report.cases.push(
      "current -> accepted baseline -> current on the same managed data preserves complete legacy request payloads; only process-bound tokens and observation clocks normalized",
    );
  report.frames = current.length;
  report.complete = true;
} finally {
  await stop();
  fs.rmSync(root, { recursive: true, force: true });
  report.ownedServiceStopped =
    !child || child.exitCode !== null || child.signalCode !== null;
  report.temporaryDataRemoved = !fs.existsSync(root);
  if (process.env.DOMAIN_REQUEST_REPORT)
    fs.writeFileSync(
      process.env.DOMAIN_REQUEST_REPORT,
      JSON.stringify(report, null, 2) + "\n",
    );
}
console.log(JSON.stringify(report, null, 2));
