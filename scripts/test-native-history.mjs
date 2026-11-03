import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { once } from "node:events";
import {
  mkdtempSync,
  realpathSync,
  mkdirSync,
  writeFileSync,
  readFileSync,
  readdirSync,
  rmSync,
  appendFileSync,
  unlinkSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { DatabaseSync } from "node:sqlite";

// All sources are generated offline. Optional prior binaries verify behavior
// independently; neither service is pointed at the user's home or app data.
const root = realpathSync(mkdtempSync(path.join(tmpdir(), "native-history-")));
const files = Number(process.env.LOCAL_AGENT_NATIVE_FILES ?? 45);
const calls = Number(process.env.LOCAL_AGENT_NATIVE_CALLS ?? 7);
assert.ok(Number.isInteger(files) && files > 40 && files <= 5000);
assert.ok(Number.isInteger(calls) && calls > 0 && calls <= 1000);
const home = path.join(root, "home");
const project = path.join(home, "workspace");
mkdirSync(path.join(project, ".git"), { recursive: true });
const timestamp = new Date(Date.now() - 60000).toISOString();
const nativePaths = [];
function rows(agent, file) {
  const id = `fixture-${file}`;
  const result =
    agent === "claude"
      ? [
          {
            type: "user",
            sessionId: id,
            uuid: "user",
            cwd: project,
            timestamp,
            message: { content: `fixture title ${file}` },
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
            payload: { model: "fixture-model", turn_id: "turn" },
          },
        ];
  for (let call = 0; call < calls; call++) {
    const response = `response-${file}-${call}`;
    result.push(
      agent === "claude"
        ? {
            type: "assistant",
            sessionId: id,
            uuid: response,
            timestamp,
            message: {
              id: response,
              model: `fixture-${call % 7}`,
              content: [{ type: "text", text: "native-body-sentinel-728" }],
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
  return result.map((r) => JSON.stringify(r) + "\n").join("");
}
for (const agent of ["claude", "codex"]) {
  const directory = path.join(
    home,
    agent === "claude"
      ? ".claude/projects/project"
      : ".codex/sessions/2026/10/19",
  );
  mkdirSync(directory, { recursive: true });
  for (let file = 0; file < files; file++) {
    const name = path.join(directory, `fixture-${file}.jsonl`);
    writeFileSync(name, rows(agent, file));
    nativePaths.push(name);
  }
}
const digest = () => {
  const hash = createHash("sha256");
  for (const name of nativePaths)
    hash.update(name.slice(home.length)).update(readFileSync(name));
  return hash.digest("hex");
};
const initialDigest = digest();
const children = [];
async function stop(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = once(child, "exit");
  child.kill();
  await exited;
}
function normalize(value) {
  if (Array.isArray(value)) return value.map(normalize);
  if (!value || typeof value !== "object") return value;
  return Object.fromEntries(
    Object.entries(value)
      .filter(
        ([key]) =>
          ![
            "asOf",
            "sourceAsOf",
            "nextCursor",
            "snapshotId",
            "expiresAt",
            "collectedAt",
            "lastScan",
            "lastSuccessfulScan",
          ].includes(key),
      )
      .map(([key, val]) => [key, normalize(val)]),
  );
}
async function run(binary, name, baseline) {
  const directory = path.join(root, name);
  mkdirSync(directory);
  mkdirSync(path.join(directory, "observation"));
  const database = path.join(directory, "observation/observations.sqlite3");
  const seed = new DatabaseSync(database);
  seed.exec(
    "CREATE TABLE model_observations(id TEXT PRIMARY KEY,started_at TEXT NOT NULL,revision INTEGER NOT NULL,value TEXT NOT NULL)",
  );
  const gatewayRow = JSON.stringify({
    id: "confirmed-zero",
    agentId: "claude",
    provider: "fixture",
    endpoint: "https://openrouter.ai/api/v1",
    bindingRevision: 1,
    protocol: "messages",
    model: "fixture-0",
    sessionId: "claude:fixture-0",
    responseId: "response-0-0",
    startedAt: timestamp,
    endedAt: timestamp,
    status: "success",
    revision: 1,
    usage: { inputTokens: 10, outputTokens: 2, totalTokens: 12 },
    reportedCost: {
      amount: 0,
      currency: "USD",
      source: "openrouter.usage.cost",
    },
  });
  seed
    .prepare("INSERT INTO model_observations VALUES(?,?,?,?)")
    .run("confirmed-zero", timestamp, 1, gatewayRow);
  seed.close();
  const token = randomUUID();
  const child = spawn(binary, ["--data-dir", directory], {
    env: {
      ...process.env,
      LOCAL_AGENT_SERVICE_TOKEN: token,
      LOCAL_AGENT_SOURCE_HOME: home,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  children.push(child);
  let stderr = "";
  child.stderr.on("data", (b) => {
    stderr += b;
  });
  const lines = createInterface({ input: child.stdout });
  const [line] = await Promise.race([
    once(lines, "line"),
    once(child, "exit").then(() => {
      throw Error(stderr || "Service exited before readiness");
    }),
    new Promise((_, reject) => {
      setTimeout(
        () => reject(Error("Service readiness timeout")),
        15000,
      ).unref();
    }),
  ]);
  const base = `http://127.0.0.1:${JSON.parse(line).port}`;
  const request = async (route, body, method) => {
    const response = await fetch(base + route, {
      method: method ?? (body === undefined ? "GET" : "PUT"),
      headers: {
        authorization: `Bearer ${token}`,
        "content-type": "application/json",
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(240000),
    });
    return { status: response.status, value: await response.json() };
  };
  const api = async (route, body, method) => {
    const { status, value } = await request(route, body, method);
    assert.equal(status, 200, JSON.stringify({ route, value }));
    return value;
  };
  const metrics = {};
  const timed = async (key, operation) => {
    const start = performance.now();
    const value = await operation();
    metrics[key] = (performance.now() - start) / 1000;
    return value;
  };
  assert.equal((await api("/api/sessions")).total, 0);
  for (const agent of ["claude", "codex"]) {
    await timed(`scan-${agent}`, async () => {
      await api("/api/session-sources", { agentId: agent, enabled: true });
      const deadline = Date.now() + 180000;
      for (;;) {
        const source = (await api("/api/snapshot")).sessionSources.find(
          (s) => s.agentId === agent,
        );
        if (source.status === "ready") {
          assert.equal(source.sessionCount, files);
          assert.equal(source.scannedFiles, files);
          break;
        }
        assert.ok(Date.now() < deadline, JSON.stringify(source));
        await new Promise((resolve) => setTimeout(resolve, 25));
      }
    });
  }
  const model = await timed("model", () =>
    api("/api/agent-usage?source=native&agent=claude"),
  );
  assert.equal(model.summary.attempts, files * calls);
  assert.equal(model.summary.usage.totalTokens, files * calls * 12);
  assert.equal(model.summary.cost.unknownCalls, files * calls - 1);
  assert.deepEqual(model.summary.cost.actual, [
    { currency: "USD", amount: 0, calls: 1 },
  ]);
  const provider = await timed("provider", () =>
    api("/api/provider-usage?source=native&providerId=_unknown"),
  );
  assert.equal(provider.selected.calls, files * calls * 2);
  assert.equal(provider.selected.usage.totalTokens, files * calls * 24);
  const sessions = await timed("sessions", () => api("/api/sessions"));
  assert.equal(sessions.total, files * 2);
  assert.equal(sessions.summary.usage.totalTokens, files * calls * 24);
  const projects = await timed("projects", () =>
    api("/api/projects?scope=all"),
  );
  assert.equal(projects.summary.sessionCount, files * 2);
  assert.equal(projects.summary.modelCalls, files * calls * 2);
  assert.equal(projects.summary.usage.totalTokens, files * calls * 24);
  const overview = await timed("overview", () =>
    api("/api/overview?source=native&range=24&metric=tokens&dimension=agent"),
  );
  assert.equal(overview.analysis.samples, files * calls * 2);
  assert.equal(overview.analysis.total, files * calls * 24);
  const requestRoute =
    "/api/request-dashboard?" +
    new URLSearchParams({
      source: "native",
      metric: "calls",
      start: new Date(Date.now() - 3600000).toISOString(),
      end: new Date(Date.now() + 3600000).toISOString(),
    });
  const requests = await timed("requests", () => api(requestRoute));
  assert.equal(requests.total, files * calls * 2);
  assert.equal(requests.summary.usage.totalTokens, files * calls * 24);
  const frozen = {
    ...requests.query,
    snapshot: requests.snapshot,
    cursor: requests.nextCursor,
  };
  const requestNext = await api(
    "/api/request-dashboard?" +
      new URLSearchParams(Object.entries(frozen).filter(([, v]) => v !== null)),
  );
  assert.equal(requestNext.items.length, 40);
  assert.deepEqual(requestNext.summary, requests.summary);
  const csv = await api("/api/request-export?snapshot=" + requests.snapshot);
  assert.equal(csv.count, files * calls * 2);
  const next = await api(
    "/api/provider-usage?source=native&providerId=_unknown&cursor=" +
      encodeURIComponent(provider.nextCursor),
  );
  assert.equal(next.calls.length, 40);
  assert.deepEqual(next.selected, provider.selected);
  const sessionNext = await api(
    "/api/sessions?cursor=" + encodeURIComponent(sessions.nextCursor),
  );
  assert.equal(sessionNext.total, sessions.total);
  assert.equal(
    new Set([...sessions.items, ...sessionNext.items].map((s) => s.id)).size,
    80,
  );
  const results = {
    model,
    provider,
    sessions,
    projects,
    overview,
    requests,
    csv,
  };
  for (const [key, value] of Object.entries(results)) {
    assert.ok(!JSON.stringify(value).includes("native-body-sentinel-728"), key);
  }
  if (baseline) {
    // Catalog hash iteration can change tied row order; compare meaningful
    // complete summaries/groups plus sorted page identities separately.
    for (const key of ["model", "provider"]) {
      for (const field of [
        "summary",
        "selected",
        "providers",
        "models",
        "totalCalls",
        "totalModels",
      ])
        if (results[key][field] !== undefined)
          assert.deepEqual(
            normalize(results[key][field]),
            normalize(baseline[key][field]),
            `${key}.${field}`,
          );
    }
    assert.deepEqual(
      normalize(sessions.summary),
      normalize(baseline.sessions.summary),
    );
    assert.deepEqual(
      normalize(requests.summary),
      normalize(baseline.requests.summary),
    );
    assert.deepEqual(
      normalize(projects.summary),
      normalize(baseline.projects.summary),
    );
    assert.equal(csv.csv, baseline.csv.csv, "complete export bytes");
    assert.deepEqual(
      model.items.map(({ sessionId, requestId, ...item }) => item),
      baseline.model.items.map(({ sessionId, requestId, ...item }) => item),
    );
  }
  assert.equal(digest(), initialDigest);
  for (const entry of readdirSync(directory, {
    recursive: true,
    withFileTypes: true,
  })) {
    if (entry.isFile())
      assert.ok(
        !readFileSync(path.join(entry.parentPath, entry.name)).includes(
          Buffer.from("native-body-sentinel-728"),
        ),
        "native bodies must not be persisted in app data",
      );
  }
  if (files <= 100) {
    const changed = nativePaths[0];
    const original = readFileSync(changed, "utf8");
    const waitFor = async (expectedCalls, expectedTokens, status = "ready") => {
      await api("/api/sessions/refresh", undefined, "POST");
      const deadline = Date.now() + 20000;
      for (;;) {
        const value = await request(
          "/api/agent-usage?source=native&agent=claude",
        );
        const source = (await api("/api/snapshot")).sessionSources.find(
          (s) => s.agentId === "claude",
        );
        if (
          value.status === 200 &&
          value.value.summary.attempts === expectedCalls &&
          value.value.summary.usage.totalTokens === expectedTokens &&
          source.status === status
        )
          return;
        assert.ok(Date.now() < deadline, JSON.stringify(value));
        await new Promise((resolve) => setTimeout(resolve, 25));
      }
    };
    const appended = {
      type: "assistant",
      sessionId: "fixture-0",
      uuid: "late",
      timestamp,
      message: {
        id: "late",
        model: "fixture-0",
        content: [],
        usage: { input_tokens: 1, output_tokens: 1 },
      },
    };
    appendFileSync(changed, JSON.stringify(appended) + "\n");
    await waitFor(files * calls + 1, files * calls * 12 + 2);
    writeFileSync(changed, '{"type":');
    await waitFor(files * calls + 1, files * calls * 12 + 2, "partial");
    // The last successful metadata remains visible, explicitly partial.
    assert.equal(
      (await api("/api/snapshot")).sessionSources.find(
        (s) => s.agentId === "claude",
      ).status,
      "partial",
    );
    writeFileSync(changed, original);
    await waitFor(files * calls, files * calls * 12);
    unlinkSync(changed);
    await waitFor((files - 1) * calls, (files - 1) * calls * 12);
    writeFileSync(changed, original);
    await waitFor(files * calls, files * calls * 12);
    assert.equal(digest(), initialDigest);
  }
  // Revocation invalidates indexes and snapshots immediately.
  await api("/api/session-sources", { agentId: "claude", enabled: false });
  assert.equal(
    (await api("/api/agent-usage?source=native&agent=claude")).summary.attempts,
    0,
  );
  assert.equal(
    (await api("/api/provider-usage?source=native&providerId=_unknown"))
      .selected.calls,
    files * calls,
  );
  assert.equal(
    (await request("/api/request-export?snapshot=" + requests.snapshot)).status,
    409,
  );
  await stop(child);
  const saved = new DatabaseSync(database, { readOnly: true });
  assert.deepEqual(
    saved
      .prepare("SELECT value FROM model_observations ORDER BY id")
      .all()
      .map((r) => r.value),
    [gatewayRow],
  );
  saved.close();
  return { results, metrics };
}
try {
  const previous = process.env.LOCAL_AGENT_NATIVE_BASELINE;
  const baseline = previous ? await run(previous, "baseline") : undefined;
  const current = await run(
    process.env.LOCAL_AGENT_NATIVE_BINARY ?? "target/debug/local-agent-service",
    "current",
    baseline?.results,
  );
  if (process.env.LOCAL_AGENT_NATIVE_REPORT) {
    writeFileSync(
      process.env.LOCAL_AGENT_NATIVE_REPORT,
      JSON.stringify(
        {
          schema: 1,
          files: files * 2,
          calls: files * calls * 2,
          originalRowsUnchanged: true,
          baseline: baseline?.metrics,
          current: current.metrics,
          results: current.results,
        },
        null,
        2,
      ) + "\n",
    );
  }
  console.log(
    JSON.stringify({
      files: files * 2,
      calls: files * calls * 2,
      rawUnchanged: true,
      baseline: baseline?.metrics,
      current: current.metrics,
    }),
  );
} finally {
  for (const child of children) await stop(child);
  if (process.env.LOCAL_AGENT_NATIVE_KEEP === "1")
    console.log(JSON.stringify({ fixture: root }));
  else rmSync(root, { recursive: true, force: true });
}
