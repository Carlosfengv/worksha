// Actual service + signed full-source worker; owned data, no model or Keychain.
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn, execFileSync } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { request as httpRequest } from "node:http";
import { randomUUID, createHash } from "node:crypto";
import { workerEntryProbe } from "./lib/worker-entry-probe.mjs";

const binary = path.resolve(
  process.env.UNIFIED_EXPORT_BINARY ?? "target/debug/local-agent-service",
);
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(os.tmpdir(), "local-agent-unified-export-")),
);
const home = path.join(root, "home"),
  data = path.join(root, "data");
const sha = (p) =>
  createHash("sha256").update(fs.readFileSync(p)).digest("hex");
const report = {
  complete: false,
  binarySha256: sha(binary),
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
};
const passed = (s) => {
  report.cases.push(s);
  console.log(s);
};
const token = randomUUID(),
  key = path.join(root, "publisher.key"),
  archive = path.join(root, "full.laplug");
const source = path.join(root, "history.jsonl"),
  native = path.join(home, ".claude/projects/fixture/owned.jsonl");
const sourceId = "org.example.full-history:history";
const usage = {
  inputTokens: "9007199254740993",
  outputTokens: "2",
  totalTokens: "9007199254740995",
  cachedInputTokens: null,
  cacheWriteInputTokens: null,
  reasoningOutputTokens: null,
};
const fact = (kind, value) => ({ fact: { kind, value } });
const rows = [
  fact("project", {
    nativeId: "p1",
    name: "Owned export project",
    pathHint: path.join(root, "never-authorized"),
  }),
  {
    ...fact("session", {
      nativeId: "s1",
      title: '=HYPERLINK("bad"),quoted',
      productIds: ["org.example.full-history:terminal"],
      unknownClient: false,
      createdAt: null,
      updatedAt: null,
      projectRef: "p1",
      usage,
      partial: false,
    }),
    conversation: [
      {
        id: "e1",
        nativeId: "e1",
        turnId: "t1",
        kind: "message",
        role: "assistant",
        status: "recorded",
        text: "BODY_MUST_NOT_BE_EXPORTED",
        input: null,
        output: null,
        model: null,
        responseId: null,
        usage: null,
      },
    ],
  },
  ...Array.from({ length: 300 }, (_, n) =>
    fact("response", {
      nativeId: `r${String(n).padStart(3, "0")}`,
      sessionRef: "s1",
      productId: "org.example.full-history:terminal",
      occurredAt: null,
      model: "fixture",
      outcome: "unknown",
      usage: n === 299 ? { ...usage, totalTokens: null } : usage,
      reportedResponseId: `claimed-${n}`,
      durationMs: "12",
    }),
  ),
];
fs.writeFileSync(source, rows.map(JSON.stringify).join("\n") + "\n");
fs.mkdirSync(path.dirname(native), { recursive: true });
fs.writeFileSync(
  native,
  Array.from({ length: 130 }, (_, n) =>
    JSON.stringify({
      type: "assistant",
      sessionId: "native-fixture",
      uuid: `entry-${n}`,
      entrypoint: "cli",
      timestamp: "2026-10-01T00:00:00Z",
      message: {
        id: `native-${n}`,
        model: "fixture",
        content: [{ type: "text", text: "NATIVE_BODY_MUST_NOT_BE_EXPORTED" }],
        usage: { input_tokens: 10, output_tokens: 3 },
      },
    }),
  ).join("\n") + "\n",
);
const originalSource = sha(source),
  originalNative = sha(native);
const sdk = (...args) =>
  execFileSync(binary, ["--plugin-sdk", ...args], {
    encoding: "utf8",
    timeout: 30000,
  });
let child,
  exited,
  base,
  observer,
  stderr = "",
  query;
async function api(
  route,
  body,
  expected = 200,
  method = body === undefined ? "GET" : "POST",
  headers = {},
) {
  const response = await fetch(base + route, {
    method,
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
      ...headers,
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(20000),
  });
  const value = await response.json();
  assert.equal(
    response.status,
    expected,
    `${route}: ${JSON.stringify(value).slice(0, 500)}`,
  );
  return value;
}
const fresh = (extra = {}) =>
  api("/api/domain/requests", {
    scope: { kind: "combined", sourceIds: ["claude", "codex", sourceId] },
    unit: "model-response",
    descending: false,
    sort: "tokens",
    includeEvidence: true,
    filter: {},
    snapshotToken: null,
    cursor: null,
    ...extra,
  });
const begin = (snapshotToken, operationId = randomUUID()) =>
  api("/api/domain/request-exports", { snapshotToken, operationId });
const cancel = (id) =>
  api(`/api/domain/request-exports/${id}`, undefined, 200, "DELETE");
const consent = async (enabled) => {
  const review = await api("/api/domain/consent-review", query);
  await api("/api/domain/consent", {
    ...query,
    enabled,
    persistMetadata: false,
    reviewRef: review.reviewRef,
  });
};
function parseCsv(text) {
  assert(text.startsWith("\uFEFF"));
  const records = [],
    row = [];
  let field = "",
    quoted = false;
  for (let i = 1; i < text.length; i++) {
    const c = text[i];
    if (c === '"') {
      if (quoted && text[i + 1] === '"') {
        field += '"';
        i++;
      } else quoted = !quoted;
    } else if (!quoted && c === ",") {
      row.push(field);
      field = "";
    } else if (!quoted && c === "\r" && text[i + 1] === "\n") {
      row.push(field);
      records.push(row.splice(0));
      field = "";
      i++;
    } else field += c;
  }
  assert(!quoted && !field && !row.length, "incomplete CSV");
  assert(records.every((r) => r.length === 37));
  return records;
}
async function exportAll(page) {
  const info = await begin(page.snapshotToken);
  let offset = 0n,
    text = "",
    batches = 0;
  while (true) {
    const chunk = await api(
      `/api/domain/request-exports/${info.operationId}?offset=${offset}`,
    );
    assert.equal(chunk.operationId, info.operationId);
    assert.equal(chunk.offset, String(offset));
    assert(BigInt(chunk.rows) <= 128n);
    assert(Buffer.byteLength(chunk.csv) <= 8 * 1024 * 1024 * 2 + 65536);
    offset += BigInt(chunk.rows);
    text += chunk.csv;
    batches++;
    if (chunk.nextOffset === null) break;
    assert.equal(chunk.nextOffset, String(offset));
  }
  assert.equal(String(offset), info.count);
  assert.deepEqual(
    await api(`/api/domain/request-exports/${info.operationId}/finish`, {
      nextOffset: String(offset),
      rows: String(offset),
    }),
    info,
  );
  return { info, csv: parseCsv(text), batches, text };
}
try {
  sdk("keygen", key);
  sdk(
    "pack",
    path.resolve("sdk/plugins/examples/full-history/package"),
    archive,
    key,
  );
  observer = workerEntryProbe(binary, root);
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
  exited = once(child, "exit");
  child.stderr.on("data", (b) => {
    stderr = (stderr + b).slice(-4000);
  });
  const [line] = await Promise.race([
    once(createInterface({ input: child.stdout }), "line", {
      signal: AbortSignal.timeout(20000),
    }),
    exited.then(() => {
      throw Error(stderr);
    }),
  ]);
  base = `http://127.0.0.1:${JSON.parse(line).port}`;
  observer.install();
  assert.equal(sha(observer.real), report.binarySha256);
  const original = await api("/api/snapshot");
  await api(
    "/api/session-sources",
    { agentId: "claude", enabled: true },
    200,
    "PUT",
  );
  await api("/api/sessions/refresh", {});
  const review = await api("/api/plugin-sources/inspect", { path: archive });
  await api("/api/plugin-sources/install", {
    packageRef: review.packageRef,
    digest: review.digest,
    approvePublisher: true,
    expectedSigningKey: review.signingKey,
  });
  query = { pluginId: "org.example.full-history", digest: review.digest };
  await api("/api/plugin-sources/state", { package: query, action: "enable" });
  await api("/api/plugin-sources/source", { package: query, path: source });
  await api("/api/plugin-sources/refresh", query);
  await consent(true);
  assert.equal(
    observer.events().length,
    2,
    "enable and explicit scan are positive worker controls",
  );
  assert(
    observer.events().every((e) => e.phase === "entry" && e.kind === "plugin"),
  );
  let page = await fresh();
  assert.equal(
    page.sourceSummaries.find((s) => s.sourceId === "claude").records,
    "130",
  );
  assert.equal(
    page.sourceSummaries.find((s) => s.sourceId === sourceId).records,
    "300",
  );
  assert.equal(
    page.sourceSummaries.find((s) => s.sourceId === "codex").records,
    null,
  );
  const selected = [...page.items];
  while (page.nextCursor) {
    page = await fresh({
      ...page.appliedQuery,
      snapshotToken: page.snapshotToken,
      cursor: page.nextCursor,
    });
    selected.push(...page.items);
  }
  assert.equal(selected.length, 430);
  const result = await exportAll(page);
  assert.equal(result.info.count, "430");
  assert(result.batches >= 4);
  const scope = JSON.parse(result.csv[1][1]);
  assert.deepEqual(scope, result.info);
  assert.equal(
    scope.sourceSummaries.find((s) => s.sourceId === "codex").records,
    null,
  );
  assert.deepEqual(result.info.appliedQuery, {
    ...page.appliedQuery,
    snapshotToken: null,
    cursor: null,
  });
  assert.deepEqual(
    result.csv.slice(2).map((r) => [r[3], r[2]]),
    selected.map((r) => [r.record.sourceId, r.record.id]),
  );
  const reported = result.csv.slice(2).filter((r) => r[3] === sourceId);
  assert.equal(reported.length, 300);
  assert(
    reported.every(
      (r) =>
        r[5] === "source-reported" &&
        r[24] === "未知" &&
        r[25] === "未知" &&
        r[28] === "12",
    ),
  );
  assert(reported.every((r) => r[8] === "'\t" + rows[1].fact.value.title));
  assert.equal(
    reported.filter((r) => r[19] === "9007199254740995").length,
    299,
  );
  assert.equal(reported.filter((r) => r[19] === "未知").length, 1);
  assert(!result.text.includes("BODY_MUST_NOT_BE_EXPORTED"));
  assert.equal(observer.events().length, 2);
  report.rows = result.info.count;
  report.batches = result.batches;
  report.csvSha256 = createHash("sha256").update(result.text).digest("hex");
  passed(
    "430 builtin/external rows match all frozen pages in order; formula, quoted fields, BigInt, unknown and source-only evidence preserved; zero export workers",
  );
  const empty = await exportAll(
    await fresh({ filter: { search: "no-such-owned-record" } }),
  );
  assert.equal(empty.info.count, "0");
  assert.equal(empty.csv.length, 2);
  assert.equal(
    empty.info.sourceSummaries.find((s) => s.sourceId === "codex").records,
    null,
  );
  passed(
    "empty readable selection still exports scope and unavailable source totals remain unknown",
  );
  const held = await begin(page.snapshotToken);
  const old = await api("/api/request-dashboard?source=gateway");
  const oldId = randomUUID();
  await api("/api/request-exports", {
    operationId: oldId,
    snapshot: old.snapshot,
  });
  assert.equal(
    (
      await api(
        "/api/domain/request-exports",
        { operationId: randomUUID(), snapshotToken: page.snapshotToken },
        503,
      )
    ).code,
    "busy",
  );
  await api(`/api/request-exports/${oldId}`, undefined, 200, "DELETE");
  await cancel(held.operationId);
  const legacy = await api(`/api/request-export?snapshot=${old.snapshot}`);
  assert.equal(typeof legacy.csv, "string");
  assert.equal(typeof legacy.count, "number");
  passed(
    "legacy and unified exports share two slots and retain original legacy JSON and CSV contracts",
  );
  const revoked = [
    await begin(page.snapshotToken),
    await begin(page.snapshotToken),
  ];
  await api(`/api/domain/request-exports/${revoked[0].operationId}?offset=0`);
  await consent(false);
  for (const info of revoked)
    await api(
      `/api/domain/request-exports/${info.operationId}/finish`,
      { nextOffset: info.count, rows: info.count },
      409,
    );
  for (const info of revoked)
    await api(
      `/api/domain/request-exports/${info.operationId}?offset=0`,
      undefined,
      409,
    );
  await consent(true);
  await api(
    "/api/domain/request-exports",
    { operationId: randomUUID(), snapshotToken: page.snapshotToken },
    409,
  );
  assert.equal(observer.events().length, 2);
  passed(
    "withdrawal invalidates final confirmation and chunks; regrant cannot revive the old frozen token",
  );
  const route = "/api/domain/request-exports";
  const body = { operationId: randomUUID(), snapshotToken: page.snapshotToken };
  await api(route, body, 401, "POST", { authorization: "" });
  // Fetch normalizes Host; send the real malformed header with the HTTP client.
  const wrongHost = await new Promise((resolve, reject) => {
    const request = httpRequest(
      base + route,
      {
        method: "POST",
        headers: {
          Host: "example.invalid",
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
        },
      },
      (response) => {
        response.resume();
        response.on("end", () => resolve(response.statusCode));
      },
    );
    request.on("error", reject);
    request.setTimeout(10000, () =>
      request.destroy(Error("Host check timed out")),
    );
    request.end(JSON.stringify(body));
  });
  assert.equal(wrongHost, 401);
  await api(route, body, 401, "POST", { origin: "https://example.invalid" });
  const after = await api("/api/snapshot");
  assert.deepEqual(after.providers, original.providers);
  assert.deepEqual(after.settings, original.settings);
  assert.equal(sha(source), originalSource);
  assert.equal(sha(native), originalNative);
  assert(!fs.existsSync(path.join(root, "never-authorized")));
  assert(!stderr.includes("panicked"));
  passed(
    "new routes retain token/Host/Origin checks, do not read reported paths or alter original data",
  );
  report.complete = true;
} finally {
  if (child && child.exitCode === null && child.signalCode === null) {
    try {
      await api("/api/runtime/shutdown", {});
      await Promise.race([
        exited,
        new Promise((_, reject) => {
          const timer = setTimeout(
            () => reject(Error("shutdown deadline")),
            10000,
          );
          timer.unref();
        }),
      ]);
    } catch (error) {
      report.cleanupError = String(error);
      child.kill("SIGTERM");
      await exited;
    }
  }
  report.ownedServiceStopped =
    !child || child.exitCode !== null || child.signalCode !== null;
  fs.rmSync(key, { force: true });
  report.temporarySigningKeyRemoved = !fs.existsSync(key);
  report.workerEntries = observer?.events().length;
  fs.rmSync(root, { recursive: true, force: true });
  report.temporaryDataRemoved = !fs.existsSync(root);
  if (process.env.UNIFIED_EXPORT_REPORT)
    fs.writeFileSync(
      process.env.UNIFIED_EXPORT_REPORT,
      JSON.stringify(report, null, 2) + "\n",
    );
}
console.log(JSON.stringify(report, null, 2));
