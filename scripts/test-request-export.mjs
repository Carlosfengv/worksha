import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { once } from "node:events";
import {
  mkdirSync,
  mkdtempSync,
  realpathSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { DatabaseSync } from "node:sqlite";

const root = realpathSync(mkdtempSync(path.join(tmpdir(), "request-export-")));
const home = path.join(root, "home");
mkdirSync(home);
mkdirSync(path.join(root, "observation"));
const db = new DatabaseSync(
  path.join(root, "observation", "observations.sqlite3"),
);
let dbClosed = false;
db.exec(
  "CREATE TABLE model_observations(id TEXT PRIMARY KEY, started_at TEXT NOT NULL, revision INTEGER NOT NULL, value TEXT NOT NULL)",
);
db.exec(
  "CREATE TABLE observation_content_epoch(id INTEGER PRIMARY KEY CHECK(id=1),format INTEGER NOT NULL,value TEXT NOT NULL); PRAGMA user_version=1",
);
db.prepare("INSERT INTO observation_content_epoch VALUES(1,1,?)").run(
  randomUUID(),
);
const insert = db.prepare("INSERT INTO model_observations VALUES(?,?,?,?)");
const count = Number(process.env.EXPORT_TEST_ROWS ?? 5000);
assert(Number.isSafeInteger(count) && count >= 500 && count <= 100000);
const row = (i) => ({
  id: `export-${String(i).padStart(8, "0")}`,
  logicalId: null,
  agentId: "claude",
  agentProductId: "claude-code-cli",
  sessionId: null,
  model: "offline-model",
  provider: i === 0 ? '=SUM(1,2)\r\n"quoted"' : "provider ".repeat(64),
  providerId: "p",
  endpoint: "https://offline.invalid",
  bindingRevision: 1,
  startedAt: new Date(Date.UTC(2026, 9, 19, 0, 0, i)).toISOString(),
  endedAt: null,
  status: "success",
  httpStatus: 200,
  ttftMs: null,
  durationMs: null,
  usage: { inputTokens: 0, outputTokens: null, totalTokens: null },
  protocol: "messages",
  timingIssue: null,
  revision: 1,
});
db.exec("BEGIN");
for (let i = 0; i < count; i++) {
  const value = row(i);
  insert.run(value.id, value.startedAt, 1, JSON.stringify(value));
}
db.exec("COMMIT");
const token = randomUUID();
let child,
  base,
  stderr = "",
  rssTimer;
const request = (route, options = {}) =>
  fetch(base + route, {
    ...options,
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
      ...options.headers,
    },
    signal: AbortSignal.timeout(30000),
  });
async function api(
  route,
  body,
  status = 200,
  method = body === undefined ? "GET" : "POST",
) {
  const response = await request(route, {
    method,
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const value = await response.json();
  assert.equal(response.status, status, JSON.stringify(value));
  return value;
}
const dashboard = () => api("/api/request-dashboard?sort=time");
try {
  child = spawn(
    process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
      "target/debug/local-agent-service",
    ["--data-dir", root],
    {
      env: {
        ...process.env,
        LOCAL_AGENT_SOURCE_HOME: home,
        LOCAL_AGENT_SERVICE_TOKEN: token,
        LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
        LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  child.stderr.on("data", (bytes) => {
    stderr += bytes;
  });
  const [line] = await Promise.race([
    once(createInterface({ input: child.stdout }), "line", {
      signal: AbortSignal.timeout(15000),
    }),
    once(child, "exit").then(() => {
      throw Error(stderr);
    }),
  ]);
  base = `http://127.0.0.1:${JSON.parse(line).port}`;
  const warmupStarted = performance.now();
  let frozen;
  let warmupBusyResponses = 0;
  for (let attempt = 0; attempt < 12; attempt++) {
    const response = await request("/api/request-dashboard?sort=time");
    const value = await response.json();
    if (response.status === 200) {
      frozen = value;
      break;
    }
    assert.equal(response.status, 400, JSON.stringify(value));
    assert.equal(value.message, "历史查询正在更新，请稍后刷新");
    assert.equal(
      child.exitCode,
      null,
      "do not restart a live service while its index is warming",
    );
    warmupBusyResponses++;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  assert(
    frozen,
    "fixture history index did not become ready within the bounded setup budget",
  );
  const warmupElapsedMs = performance.now() - warmupStarted;
  assert.equal(frozen.total, count);
  const rssSamples = [];
  const sampleRss = () => {
    if (process.env.EXPORT_TEST_RSS !== "1") return;
    const value = Number(
      execFileSync("ps", ["-o", "rss=", "-p", String(child.pid)], {
        encoding: "utf8",
      }).trim(),
    );
    if (Number.isFinite(value)) rssSamples.push(value);
  };
  sampleRss();
  if (process.env.EXPORT_TEST_RSS === "1")
    rssTimer = setInterval(sampleRss, 100);
  const legacy = await api(`/api/request-export?snapshot=${frozen.snapshot}`);
  assert.equal(legacy.count, count);
  const ids = Array.from({ length: 3 }, () => randomUUID());
  const begin = (id, status = 200) =>
    api(
      "/api/request-exports",
      { operationId: id, snapshot: frozen.snapshot },
      status,
    );
  const info = await begin(ids[0]);
  await begin(ids[1]);
  await begin(ids[2], 503);
  assert.deepEqual(await begin(ids[0]), info);
  assert.equal(
    (
      await request(`/api/request-exports/${ids[0]}?after=-1`, {
        headers: { origin: "https://untrusted.invalid" },
      })
    ).status,
    401,
  );
  assert.equal(
    (await fetch(`${base}/api/request-exports/${ids[0]}?after=-1`)).status,
    401,
  );
  await api(`/api/request-exports/${ids[1]}`, undefined, 200, "DELETE");
  await api(
    "/api/request-exports",
    {
      operationId: ids[2],
      snapshot: frozen.snapshot,
      path: "/not-a-service-write-target",
    },
    400,
  );
  let part = await api(`/api/request-exports/${ids[0]}?after=-1`);
  let csv = "",
    rows = 0,
    after = -1,
    batches = 0,
    maximumBatchBytes = 0;
  const health = [];
  const started = performance.now();
  const changed = { ...row(0), provider: "changed after freeze", revision: 2 };
  db.prepare("UPDATE model_observations SET value=?,revision=2 WHERE id=?").run(
    JSON.stringify(changed),
    changed.id,
  );
  db.prepare("DELETE FROM model_observations WHERE id=?").run(row(1).id);
  for (let i = 0; i < 12; i++) await dashboard();
  assert.deepEqual(await begin(ids[0]), info);
  await begin(ids[2], 409);
  for (;;) {
    assert.equal(part.operationId, ids[0]);
    assert.equal(part.after, after);
    assert(part.rows <= 128);
    csv += part.csv;
    rows += part.rows;
    after += part.rows;
    batches++;
    maximumBatchBytes = Math.max(
      maximumBatchBytes,
      Buffer.byteLength(part.csv),
    );
    if (part.nextAfter === null) break;
    assert.equal(part.nextAfter, after);
    [part] = await Promise.all([
      api(`/api/request-exports/${ids[0]}?after=${after}`),
      (async () => {
        const t = performance.now();
        await api("/api/runtime");
        health.push(performance.now() - t);
      })(),
    ]);
  }
  assert.equal(rows, count);
  assert(batches > 2);
  assert(maximumBatchBytes < 100000);
  assert.equal(csv, legacy.csv);
  assert.equal(csv.split("\r\n").filter(Boolean).length, count + 2); // embedded CSV field newline is retained
  assert.deepEqual(
    await api(`/api/request-exports/${ids[0]}/finish`, { after, rows }),
    info,
  );
  await api(`/api/request-exports/${ids[0]}?after=-1`, undefined, 409);
  let fresh = await dashboard();
  const cancelled = randomUUID();
  await api("/api/request-exports", {
    operationId: cancelled,
    snapshot: fresh.snapshot,
  });
  await api(`/api/request-exports/${cancelled}`, undefined, 200, "DELETE");
  await api(`/api/request-exports/${cancelled}?after=-1`, undefined, 409);
  health.sort((a, b) => a - b);
  assert(
    health.at(-1) < 5000,
    "runtime requests must keep progressing during export",
  );
  const expired = [randomUUID(), randomUUID()];
  for (const id of expired)
    await api("/api/request-exports", {
      operationId: id,
      snapshot: fresh.snapshot,
    });
  const oldChunk = await api(`/api/request-exports/${expired[0]}?after=-1`);
  assert(oldChunk.rows > 0);
  const oldEpoch = db
    .prepare("SELECT value FROM observation_content_epoch WHERE id=1")
    .get().value;
  const nextEpoch = randomUUID();
  db.exec("BEGIN IMMEDIATE");
  assert.equal(
    db.prepare("DELETE FROM model_observations WHERE id=?").run(row(0).id)
      .changes,
    1,
  );
  assert.equal(
    db
      .prepare(
        "UPDATE observation_content_epoch SET value=? WHERE id=1 AND value=?",
      )
      .run(nextEpoch, oldEpoch).changes,
    1,
  );
  db.exec("COMMIT");
  const contentFailures = [
    await api(
      `/api/request-exports/${expired[0]}?after=${oldChunk.rows - 1}`,
      undefined,
      503,
    ),
    await api(
      `/api/request-exports/${expired[1]}/finish`,
      { after: fresh.total - 1, rows: fresh.total },
      503,
    ),
    await api(
      "/api/request-exports",
      { operationId: randomUUID(), snapshot: fresh.snapshot },
      503,
    ),
    await api(`/api/request-export?snapshot=${fresh.snapshot}`, undefined, 503),
    await api(
      `/api/request-dashboard?snapshot=${fresh.snapshot}`,
      undefined,
      409,
    ),
  ];
  for (const failure of contentFailures) {
    assert.equal("csv" in failure, false);
    assert.equal("rows" in failure, false);
  }
  fresh = await dashboard();
  assert.equal(fresh.total, count - 2);
  assert.equal(
    (await api(`/api/request-detail?id=${row(0).id}`)).request,
    null,
  );
  assert.equal(
    db.prepare("SELECT value FROM observation_content_epoch WHERE id=1").get()
      .value,
    nextEpoch,
  );
  // A replacement is a fault injection in this isolated fixture, not a
  // supported way to restore a live SQLite store. Keep its WAL files together.
  const interrupted = [randomUUID(), randomUUID()];
  for (const id of interrupted)
    await api("/api/request-exports", {
      operationId: id,
      snapshot: fresh.snapshot,
    });
  const firstChunk = await api(
    `/api/request-exports/${interrupted[0]}?after=-1`,
  );
  assert(firstChunk.rows > 0);
  db.close();
  dbClosed = true;
  renameSync(
    path.join(root, "observation"),
    path.join(root, "old-observation"),
  );
  mkdirSync(path.join(root, "observation"));
  const replacement = new DatabaseSync(
    path.join(root, "observation", "observations.sqlite3"),
  );
  replacement.exec(
    "CREATE TABLE model_observations(id TEXT PRIMARY KEY, started_at TEXT NOT NULL, revision INTEGER NOT NULL, value TEXT NOT NULL)",
  );
  const replacementRow = {
    ...row(0),
    id: "replacement-only",
    provider: "replacement-store",
  };
  replacement
    .prepare("INSERT INTO model_observations VALUES(?,?,?,?)")
    .run(
      replacementRow.id,
      replacementRow.startedAt,
      1,
      JSON.stringify(replacementRow),
    );
  replacement.close();
  const invalidated = [
    await api(
      `/api/request-exports/${interrupted[0]}?after=${firstChunk.rows - 1}`,
      undefined,
      503,
    ),
    await api(
      `/api/request-exports/${interrupted[1]}/finish`,
      { after: fresh.total - 1, rows: fresh.total },
      503,
    ),
    await api(
      "/api/request-exports",
      { operationId: randomUUID(), snapshot: fresh.snapshot },
      503,
    ),
    await api(`/api/request-export?snapshot=${fresh.snapshot}`, undefined, 503),
    await api(
      `/api/request-dashboard?snapshot=${fresh.snapshot}`,
      undefined,
      409,
    ),
  ];
  for (const failure of invalidated) {
    assert.equal("csv" in failure, false);
    assert.equal("rows" in failure, false);
  }
  assert.equal(invalidated.at(-1).code, "stale_snapshot");
  const replacementPage = await dashboard();
  assert.equal(replacementPage.total, 1);
  const replacementExport = await api(
    `/api/request-export?snapshot=${replacementPage.snapshot}`,
  );
  assert.equal(replacementExport.count, 1);
  assert(replacementExport.csv.includes("replacement-store"));
  assert.equal(replacementExport.csv.includes("changed after freeze"), false);
  assert.equal(replacementExport.csv.includes("provider provider"), false);
  const report = {
    passed: true,
    records: count,
    warmupBusyResponses,
    warmupElapsedMs,
    batches,
    csvBytes: Buffer.byteLength(csv),
    maximumBatchBytes,
    elapsedMs: performance.now() - started,
    runtimeP95Ms: health[Math.floor(health.length * 0.95)],
    frozenParity: true,
    boundedAdmission: true,
    evictionSurvived: true,
    authenticationChecked: true,
    cancelledResourceUnavailable: true,
    contentEpochInvalidatesCachedReads: true,
    clearedDetailNotRebuiltFromTempPayload: true,
    replacedStoreInvalidatesCachedReads: true,
    replacementStoreCanBeQueried: true,
    realModelCalls: 0,
    sampledServiceRss: rssSamples.length
      ? {
          intervalMs: 100,
          samples: rssSamples.length,
          baselineKiB: rssSamples[0],
          maximumKiB: Math.max(...rssSamples),
          maximumIncreaseKiB: Math.max(...rssSamples) - rssSamples[0],
          scope:
            "sampled after dashboard initialization, includes legacy streaming and frozen-view churn; not an exact peak",
        }
      : undefined,
    scope: "service export batches; not a native WebView memory benchmark",
  };
  if (process.env.EXPORT_TEST_REPORT)
    writeFileSync(
      process.env.EXPORT_TEST_REPORT,
      JSON.stringify(report, null, 2) + "\n",
    );
  console.log(JSON.stringify(report));
} finally {
  clearInterval(rssTimer);
  if (!dbClosed) db.close();
  if (child?.exitCode === null) {
    const exited = once(child, "exit");
    child.kill("SIGTERM");
    await exited;
  }
  rmSync(root, { recursive: true, force: true });
}
