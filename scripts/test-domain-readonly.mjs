// D0/D1 comparison uses only synthetic sources, MemoryVault and owned processes.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { randomUUID, createHash } from "node:crypto";
import { performance } from "node:perf_hooks";
import { DatabaseSync } from "node:sqlite";
import { readCoreSchema } from "./lib/core-schema.mjs";

const binary = path.resolve(
  process.env.DOMAIN_TEST_BINARY ?? "target/debug/local-agent-service",
);
const previous =
  process.env.DOMAIN_BASELINE_BINARY &&
  path.resolve(process.env.DOMAIN_BASELINE_BINARY);
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "local-agent-domain-d01-")),
);
const home = path.join(root, "home");
const data = path.join(root, "data");
const source = path.join(home, ".claude/projects/project/fixture.jsonl");
fs.mkdirSync(path.dirname(source), { recursive: true });
const rows = Array.from({ length: 96 }, (_, n) =>
  n % 2 === 0
    ? {
        type: "user",
        entrypoint: "cli",
        sessionId: "domain-fixture",
        uuid: `u${n}`,
        promptId: `t${n}`,
        timestamp: "2026-10-30T10:00:00Z",
        message: { role: "user", content: `body ${n}` },
      }
    : {
        type: "assistant",
        sessionId: "domain-fixture",
        uuid: `a${n}`,
        timestamp: "2026-10-30T10:00:01Z",
        message: {
          id: `r${n}`,
          model: "fixture",
          content: [{ type: "text", text: `body ${n}` }],
          usage: { input_tokens: 10, output_tokens: 3 },
        },
      },
);
fs.writeFileSync(source, rows.map(JSON.stringify).join("\n") + "\n");
const digest = (f) =>
  createHash("sha256").update(fs.readFileSync(f)).digest("hex");
const sourceHash = digest(source);
const report = {
  complete: false,
  binary,
  binarySha256: digest(binary),
  baselineSha256: previous ? digest(previous) : null,
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
};
let child, base, token;
async function stop() {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const owned = child;
  const done = once(owned, "exit");
  owned.kill();
  let timer;
  try {
    await Promise.race([
      done,
      new Promise((_, reject) => {
        timer = setTimeout(() => {
          owned.kill("SIGKILL");
          reject(new Error("owned service did not stop"));
        }, 8000);
        timer.unref();
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}
async function boot(executable) {
  token = randomUUID();
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
  let stderr = "";
  child.stderr.on("data", (b) => {
    stderr = (stderr + b).slice(-4000);
  });
  const lines = createInterface({ input: child.stdout });
  const [line] = await Promise.race([
    once(lines, "line", { signal: AbortSignal.timeout(15000) }),
    once(child, "exit").then(() => {
      throw Error(stderr);
    }),
  ]);
  base = `http://127.0.0.1:${JSON.parse(line).port}`;
}
async function request(route, options = {}) {
  return fetch(base + route, {
    ...options,
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
      ...options.headers,
    },
    signal: AbortSignal.timeout(10000),
  });
}
async function api(route, options) {
  const response = await request(route, options);
  assert.equal(response.status, 200, route);
  return response.json();
}
async function indexed() {
  for (let n = 0; n < 100; n++) {
    const page = await api("/api/sessions?agent=claude");
    if (page.total === 1 && page.items[0].modelCalls === 48) return page;
    await new Promise((r) => setTimeout(r, 50));
  }
  throw Error("fixture did not finish indexing");
}
function stable(value) {
  return JSON.parse(
    JSON.stringify(value, (key, item) =>
      ["asOf", "lastScan", "lastSuccessfulScan"].includes(key)
        ? undefined
        : item,
    ),
  );
}
async function capture() {
  const page = await indexed();
  const session = page.items[0];
  const conversation = await api(
    `/api/sessions/${encodeURIComponent(session.id)}`,
  );
  assert.ok(
    conversation.nextCursor,
    "fixture exercises more than one body page",
  );
  const next = await api(
    `/api/sessions/${encodeURIComponent(session.id)}?cursor=${encodeURIComponent(conversation.nextCursor)}`,
  );
  const calls = await api("/api/calls?agent=claude");
  let nextCalls = null;
  if (calls.nextCursor) {
    nextCalls = await api(
      `/api/calls?agent=claude&cursor=${encodeURIComponent(calls.nextCursor)}`,
    );
    assert.equal(calls.items.length + nextCalls.items.length, calls.total);
    assert.equal(
      new Set([...calls.items, ...nextCalls.items].map((c) => c.id)).size,
      calls.total,
    );
    // Call-list cursors intentionally bind to the process catalog generation.
    // Compare every returned field and exercise pagination in each process,
    // without asserting an opaque process cursor survives a service restart.
    nextCalls.appliedFilter.cursor = "process-bound-cursor";
  }
  calls.nextCursor = calls.nextCursor ? "process-bound-cursor" : null;
  return stable({ page, conversation, next, calls, nextCalls });
}
async function samples(route) {
  for (let n = 0; n < 5; n++) await api(route);
  const values = [];
  for (let n = 0; n < 40; n++) {
    const start = performance.now();
    await api(route);
    values.push(performance.now() - start);
  }
  values.sort((a, b) => a - b);
  return {
    samples: values.length,
    p50Ms: values[19],
    p95Ms: values[37],
    p99Ms: values[39],
  };
}
try {
  await boot(previous ?? binary);
  await api("/api/session-sources", {
    method: "PUT",
    body: JSON.stringify({ agentId: "claude", enabled: true }),
  });
  const old = await capture();
  const database = path.join(data, "local-agent.sqlite3");
  const beforeSchema = readCoreSchema(database);
  assert.equal(beforeSchema.version, 15);
  report.coreSchema = beforeSchema;
  report.legacyResponseBaseline = old;
  report.baselineLatency = await samples("/api/sessions?agent=claude");
  await stop();
  const db = new DatabaseSync(database);
  const backup = path.join(root, "core-backup.sqlite3");
  db.prepare("VACUUM INTO ?").run(backup);
  db.close();
  const check = new DatabaseSync(backup, { readOnly: true });
  assert.equal(check.prepare("PRAGMA quick_check").get().quick_check, "ok");
  check.close();
  report.cases.push("consistent managed backup and schema-15 inventory");
  await boot(binary);
  assert.equal((await fetch(base + "/api/domain/catalog")).status, 401);
  assert.equal(
    (
      await request("/api/domain/catalog", {
        headers: { origin: "https://example.test" },
      })
    ).status,
    401,
  );
  const catalog = await api("/api/domain/catalog");
  assert.equal(catalog.providers.length, 1);
  assert.equal(catalog.providers[0].capability.instanceId, "claude");
  assert.deepEqual(
    catalog.products.map((p) => p.id),
    ["claude-code-cli", "claude-code-desktop"],
  );
  assert.equal(JSON.stringify(catalog).includes(home), false);
  assert.deepEqual(await capture(), old);
  report.catalog = catalog;
  assert.deepEqual(readCoreSchema(database), beforeSchema);
  report.cases.push(
    "baseline-to-pilot restart preserves full legacy page, statistics, identity and body cursors",
  );
  const beforeQuery = await api("/api/snapshot");
  for (let n = 0; n < 12; n++) await api("/api/domain/catalog");
  const afterQuery = await api("/api/snapshot");
  assert.deepEqual(
    afterQuery.sessionSources.map((s) => [
      s.authorizationRevision,
      s.lastScan,
      s.scannedFiles,
    ]),
    beforeQuery.sessionSources.map((s) => [
      s.authorizationRevision,
      s.lastScan,
      s.scannedFiles,
    ]),
  );
  assert.equal(digest(source), sourceHash);
  report.cases.push(
    "authenticated catalog reads do not scan, grant permissions, expose paths or rewrite source files",
  );
  report.currentLatency = await samples("/api/sessions?agent=claude");
  report.catalogLatency = await samples("/api/domain/catalog");
  await api("/api/session-sources", {
    method: "PUT",
    body: JSON.stringify({ agentId: "claude", enabled: false }),
  });
  const revoked = await api("/api/domain/catalog");
  assert.equal(revoked.sources[0].status.authorization, "revoked");
  assert.equal(revoked.sources[0].status.freshness, "unread");
  assert.equal((await api("/api/sessions?agent=claude")).total, 0);
  assert.notEqual(
    (await request(`/api/sessions/${encodeURIComponent(old.page.items[0].id)}`))
      .status,
    200,
  );
  report.cases.push(
    "revocation removes sessions and rejects existing body references without changing the IPC",
  );
  await api("/api/session-sources", {
    method: "PUT",
    body: JSON.stringify({ agentId: "claude", enabled: true }),
  });
  await indexed();
  await stop();
  if (previous) {
    await boot(previous);
    assert.deepEqual(await capture(), old);
    await stop();
    await boot(binary);
    assert.deepEqual(await capture(), old);
    await stop();
    report.cases.push(
      "same-data baseline to pilot to baseline to pilot preserves behavior and source authorization",
    );
  }
  assert.equal(digest(source), sourceHash);
  assert.deepEqual(readCoreSchema(database), beforeSchema);
  report.complete = true;
} finally {
  await stop();
  report.ownedServiceStopped =
    !child || child.exitCode !== null || child.signalCode !== null;
  fs.rmSync(root, { recursive: true, force: true });
  report.temporaryDataRemoved = !fs.existsSync(root);
  if (process.env.DOMAIN_TEST_REPORT)
    fs.writeFileSync(
      process.env.DOMAIN_TEST_REPORT,
      JSON.stringify(report, null, 2) + "\n",
    );
}
console.log(JSON.stringify(report, null, 2));
