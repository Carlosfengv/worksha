import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn, execFileSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { once } from "node:events";
import { createInterface } from "node:readline";

// Service-level metadata capacity. No real sources, credentials, agents or model
// calls. This does not establish native WebView memory/interaction acceptance.
const option = (name, fallback) => {
  const n = process.argv.indexOf(name);
  return n < 0 ? fallback : process.argv[n + 1];
};
const binary = fs.realpathSync(
  option("--binary", "target/debug/local-agent-service"),
);
const output = path.resolve(
  option("--report", ".work/domain-fact-capacity.json"),
);
const files = Number(option("--files", "500"));
const calls = Number(option("--calls", "1000"));
const samples = Number(option("--samples", "20"));
assert(Number.isInteger(files) && files >= 1 && files <= 1000);
assert(Number.isInteger(calls) && calls >= 1 && calls <= 1000);
assert(Number.isInteger(samples) && samples >= 2 && samples <= 100);
const fixtureReport = option("--fixture-report", null);
const previous = fixtureReport
  ? JSON.parse(fs.readFileSync(fixtureReport, "utf8"))
  : null;
const root = fs.realpathSync(
  previous
    ? previous.root
    : fs.mkdtempSync(path.join(tmpdir(), "local-agent-fact-capacity-")),
);
if (previous) {
  assert(
    previous.processesExited &&
      previous.nativeFiles === files * 2 &&
      previous.nativeResponses === files * calls * 2,
  );
  assert(
    path.dirname(root) === fs.realpathSync(tmpdir()) &&
      path.basename(root).startsWith("local-agent-fact-capacity-"),
  );
  assert(
    fs.lstatSync(root).isDirectory() &&
      fs.lstatSync(root).uid === process.getuid(),
  );
  assert(
    !execFileSync("/bin/ps", ["-axo", "command="], { encoding: "utf8" })
      .split("\n")
      .some((cmd) => cmd.trim().endsWith(` --data-dir ${root}/data`)),
  );
}
const home = path.join(root, "home"),
  data = path.join(root, "data"),
  workspace = path.join(root, "workspace");
if (!previous) {
  fs.mkdirSync(workspace);
  fs.mkdirSync(path.join(workspace, ".git"));
}
const sha = (b) => createHash("sha256").update(b).digest("hex");
const report = {
  complete: false,
  phaseAccepted: false,
  root,
  binarySha256: sha(fs.readFileSync(binary)),
  harnessSha256: sha(fs.readFileSync(import.meta.filename)),
  fixtureReportSha256: fixtureReport
    ? sha(fs.readFileSync(fixtureReport))
    : null,
  nativeFiles: files * 2,
  nativeResponses: files * calls * 2,
  externalResponses: 83,
  timedSamplesPerSelection: samples,
  measurements: {},
  resources: [],
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
  scope:
    "Pinned debug service; complete native and combined metadata summaries and bounded pages, not native WebView or a full D0 comparative p95/throughput gate. Legacy call pages below compare primary IDs/count only, not all fact summary semantics.",
};
fs.mkdirSync(path.dirname(output), { recursive: true });
const save = () =>
  fs.writeFileSync(output, JSON.stringify(report, null, 2) + "\n");
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const at = new Date(Date.now() - 60000).toISOString(),
  externalAt = new Date(Date.parse(at) + 1000).toISOString();
const sourceFiles = [];
let child,
  base,
  token,
  sampler,
  resourceError,
  active = 0;
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
  active++;
  try {
    const r = await fetch(base + route, {
      method,
      headers: {
        authorization: `Bearer ${token}`,
        "content-type": "application/json",
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(120000),
    });
    const raw = await r.text();
    let value;
    try {
      value = JSON.parse(raw);
    } catch {
      throw Error(`${route}: HTTP ${r.status}: ${raw.slice(0, 300)}`);
    }
    assert.equal(r.status, expected, `${route}: ${raw.slice(0, 1200)}`);
    return {
      value,
      milliseconds: performance.now() - started,
      bytes: Buffer.byteLength(raw),
    };
  } finally {
    active--;
  }
}
async function fresh(query) {
  for (let n = 0; n < 8; n++) {
    try {
      return await api("/api/domain/facts", query);
    } catch (error) {
      if (
        !String(error.message).includes('"code":"stale_snapshot"') ||
        query.snapshotToken
      )
        throw error;
      report.concurrentPublicationRetries =
        (report.concurrentPublicationRetries ?? 0) + 1;
      await delay(100);
    }
  }
  throw Error("Native fact publication did not settle");
}
async function stop() {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
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
        }, 10000);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}
function resource(label) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const rssBytes =
    Number(
      execFileSync("/bin/ps", ["-o", "rss=", "-p", String(child.pid)], {
        encoding: "utf8",
      }).trim(),
    ) * 1024;
  report.resources.push({ at: new Date().toISOString(), label, rssBytes });
  assert(
    rssBytes <= 2 * 1024 ** 3,
    "service exceeds the existing 2 GiB budget",
  );
}
function digestSources() {
  const digest = createHash("sha256");
  for (const file of sourceFiles) digest.update(fs.readFileSync(file));
  return digest.digest("hex");
}
function factQuery(kind, sourceIds, extra = {}) {
  return {
    scope: {
      kind: sourceIds.every((id) => ["claude", "codex"].includes(id))
        ? "builtin"
        : sourceIds.length === 1
          ? "external"
          : "combined",
      sourceIds,
    },
    kind,
    projectId: null,
    sessionId: null,
    responseId: null,
    snapshotToken: null,
    cursor: null,
    ...extra,
  };
}
function totals(page, counts) {
  assert(page.items.length <= 40, "metadata must remain a bounded page");
  for (const [id, count] of Object.entries(counts)) {
    const summary = page.sourceSummaries.find((s) => s.sourceId === id);
    assert(summary, `missing summary ${id}`);
    assert.equal(summary.responses.records, String(count));
    assert.equal(summary.responses.usage.totalTokens, String(count * 12));
  }
}
async function measure(name, query, counts) {
  const times = [],
    bytes = [];
  for (let n = 0; n < samples + 2; n++) {
    const frame = await fresh(query);
    totals(frame.value, counts);
    if (n >= 2) {
      times.push(frame.milliseconds);
      bytes.push(frame.bytes);
    }
    if (resourceError) throw resourceError;
  }
  const sorted = [...times].sort((a, b) => a - b);
  report.measurements[name] = {
    samples: times.length,
    p50Ms: sorted[Math.ceil(times.length * 0.5) - 1],
    p95Ms: sorted[Math.ceil(times.length * 0.95) - 1],
    p99Ms: sorted[Math.ceil(times.length * 0.99) - 1],
    failures: 0,
    maxPayloadBytes: Math.max(...bytes),
    timesMs: times,
  };
  save();
  console.log(
    JSON.stringify({
      selection: name,
      ...report.measurements[name],
      timesMs: undefined,
    }),
  );
}
try {
  save();
  for (const agent of ["claude", "codex"]) {
    const directory = path.join(
      home,
      agent === "claude"
        ? ".claude/projects/fixture"
        : ".codex/sessions/2026/10/31",
    );
    fs.mkdirSync(directory, { recursive: true });
    for (let i = 0; i < files; i++) {
      const id = `capacity-${String(i).padStart(5, "0")}`,
        file = path.join(directory, `${id}.jsonl`),
        rows = [];
      if (previous) {
        const metadata = fs.lstatSync(file);
        assert(
          metadata.isFile() &&
            !metadata.isSymbolicLink() &&
            metadata.nlink === 1,
        );
        sourceFiles.push(file);
        continue;
      }
      if (agent === "claude")
        rows.push({
          type: "user",
          sessionId: id,
          uuid: "u1",
          entrypoint: "cli",
          cwd: workspace,
          timestamp: at,
          message: { role: "user", content: "offline capacity" },
        });
      else {
        rows.push({
          type: "session_meta",
          timestamp: at,
          payload: { id, cwd: workspace, originator: "codex_cli_rs" },
        });
        rows.push({
          type: "turn_context",
          timestamp: at,
          payload: { model: "fixture", turn_id: "turn" },
        });
      }
      for (let n = 0; n < calls; n++) {
        const response = `response-${i}-${n}`;
        rows.push(
          agent === "claude"
            ? {
                type: "assistant",
                sessionId: id,
                uuid: response,
                timestamp: at,
                message: {
                  id: response,
                  model: "fixture",
                  content: [{ type: "text", text: "offline" }],
                  usage: { input_tokens: 10, output_tokens: 2 },
                },
              }
            : {
                type: "token_usage_record",
                timestamp: at,
                payload: {
                  response_id: response,
                  usage: { input_tokens: 10, output_tokens: 2 },
                },
              },
        );
      }
      fs.writeFileSync(file, rows.map(JSON.stringify).join("\n") + "\n");
      sourceFiles.push(file);
    }
  }
  const key = path.join(root, "publisher.key"),
    archive = path.join(root, "full.laplug"),
    external = path.join(root, "external.jsonl");
  if (!previous) {
    sdk("keygen", key);
    sdk(
      "pack",
      path.resolve("sdk/plugins/examples/full-history/package"),
      archive,
      key,
    );
  }
  const manifest = JSON.parse(sdk("check", archive)).manifest,
    sourceId = `${manifest.id}:history`;
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
          name: "reported project",
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
          title: "reported session",
          createdAt: externalAt,
          updatedAt: externalAt,
          projectRef: "p1",
          usage,
          partial: false,
        },
      },
    },
    ...Array.from({ length: 83 }, (_, n) => ({
      fact: {
        kind: "response",
        value: {
          nativeId: `r${String(n).padStart(3, "0")}`,
          sessionRef: "s1",
          productId: `${manifest.id}:terminal`,
          occurredAt: externalAt,
          model: "fixture",
          outcome: "success",
          usage,
          reportedResponseId: "response-0-0",
          durationMs: null,
        },
      },
    })),
  ];
  if (!previous)
    fs.writeFileSync(external, rows.map(JSON.stringify).join("\n") + "\n");
  const externalMetadata = fs.lstatSync(external);
  assert(
    externalMetadata.isFile() &&
      !externalMetadata.isSymbolicLink() &&
      externalMetadata.nlink === 1,
  );
  sourceFiles.push(external);
  report.initialSourceSha256 = digestSources();
  if (previous)
    assert.equal(
      report.initialSourceSha256,
      previous.initialSourceSha256,
      "reused fixture bytes changed",
    );
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
  child.stderr.on("data", (b) => fs.appendFileSync(`${output}.service.log`, b));
  const [line] = await Promise.race([
    once(createInterface({ input: child.stdout }), "line", {
      signal: AbortSignal.timeout(30000),
    }),
    once(child, "exit").then(() => {
      throw Error("service exited before readiness");
    }),
  ]);
  base = `http://127.0.0.1:${JSON.parse(line).port}`;
  report.pid = child.pid;
  sampler = setInterval(() => {
    try {
      resource("sample");
      save();
    } catch (error) {
      resourceError = error;
    }
  }, 1000);
  for (const agentId of ["claude", "codex"])
    await api("/api/session-sources", { agentId, enabled: true }, 200, "PUT");
  const deadline = Date.now() + 600000;
  for (;;) {
    const status = (await api("/api/snapshot")).value.sessionSources;
    if (
      ["claude", "codex"].every(
        (id) => status.find((s) => s.agentId === id)?.status === "ready",
      )
    )
      break;
    if (resourceError) throw resourceError;
    assert(Date.now() < deadline, "native indexing readiness deadline");
    await delay(250);
  }
  report.readyAt = new Date().toISOString();
  resource("native-ready");
  save();
  const native = factQuery("response", ["claude", "codex"]),
    counts = { claude: files * calls, codex: files * calls };
  const legacy = await api("/api/calls?kind=model");
  assert.equal(legacy.value.total, files * calls * 2);
  const first = await fresh(native);
  totals(first.value, counts);
  assert.deepEqual(
    first.value.items.map((r) => r.id),
    legacy.value.items.map((r) => r.id),
  );
  report.legacyPrimaryPage = {
    milliseconds: legacy.milliseconds,
    bytes: legacy.bytes,
    total: legacy.value.total,
  };
  report.firstNativeFactPage = {
    milliseconds: first.milliseconds,
    bytes: first.bytes,
  };
  report.cases.push(
    "million native response total and bounded first page match the existing call API; complete per-source token totals match the generated oracle",
  );
  await measure("native-responses", native, counts);
  await measure(
    "native-sessions",
    factQuery("session", ["claude", "codex"]),
    counts,
  );
  const review = (await api("/api/plugin-sources/inspect", { path: archive }))
    .value;
  const installed = (
    await api("/api/plugin-sources/install", {
      packageRef: review.packageRef,
      digest: review.digest,
      approvePublisher: true,
      expectedSigningKey: review.signingKey,
    })
  ).value;
  const q = { pluginId: installed.manifest.id, digest: installed.digest };
  await api("/api/plugin-sources/state", { package: q, action: "enable" });
  await api("/api/plugin-sources/source", { package: q, path: external });
  await api("/api/plugin-sources/refresh", q);
  const consent = (await api("/api/domain/consent-review", q)).value;
  await api("/api/domain/consent", {
    ...q,
    enabled: true,
    persistMetadata: false,
    reviewRef: consent.reviewRef,
  });
  const combined = factQuery("response", ["claude", "codex", sourceId]),
    combinedCounts = { ...counts, [sourceId]: 83 };
  let page = (await fresh(combined)).value;
  const records = [];
  for (let n = 0; n < 3; n++) {
    totals(page, combinedCounts);
    records.push(...page.items);
    assert(page.nextCursor);
    if (n < 2)
      page = (
        await api("/api/domain/facts", {
          ...combined,
          snapshotToken: page.snapshotToken,
          cursor: page.nextCursor,
        })
      ).value;
  }
  assert.equal(records.length, 120);
  assert.equal(new Set(records.map((r) => r.id)).size, 120);
  assert(records.slice(0, 83).every((r) => r.identity.sourceId === sourceId));
  assert(
    records.slice(83).every((r) => r.identity.evidence === "builtin-native"),
  );
  report.cases.push(
    "three bounded combined pages cross 83 newer external reports into million-row native history with full-scope summaries and without claimed-ID deduplication",
  );
  await measure("combined-responses", combined, combinedCounts);
  const scoped = factQuery("response", ["claude"], {
    sessionId: "claude:capacity-00000",
  });
  await measure("one-native-session", scoped, { claude: calls });
  assert.equal(digestSources(), report.initialSourceSha256);
  assert(!fs.existsSync(path.join(root, "never-authorized")));
  if (resourceError) throw resourceError;
  report.unfinishedRequests = active;
  assert.equal(active, 0);
  resource("final");
  report.sourceFilesUnchanged = true;
  report.complete = true;
} catch (error) {
  report.error = error.stack;
  throw error;
} finally {
  clearInterval(sampler);
  await stop().catch((error) => {
    report.cleanupError = error.message;
  });
  report.processesExited =
    !child || child.exitCode !== null || child.signalCode !== null;
  if (
    report.complete &&
    report.processesExited &&
    !process.argv.includes("--keep-fixture")
  ) {
    fs.rmSync(root, { recursive: true, force: true });
    report.temporaryDataRemoved = true;
  } else {
    report.ownedFixtureRetained = true;
  }
  save();
}
