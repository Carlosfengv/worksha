// Real basic Wasm + service-process proof; all resources are owned synthetic fixtures.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn, execFileSync } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { randomUUID, createHash } from "node:crypto";
import { readCoreSchema } from "./lib/core-schema.mjs";

const binary = path.resolve(
  process.env.DOMAIN_TEST_BINARY ?? "target/debug/local-agent-service",
);
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "local-agent-domain-d2-")),
);
const home = path.join(root, "home"),
  data = path.join(root, "data"),
  source = path.join(root, "aurora.jsonl");
fs.mkdirSync(home);
const native = path.join(home, ".claude/projects/fixture/native.jsonl");
fs.mkdirSync(path.dirname(native), { recursive: true });
fs.writeFileSync(
  native,
  [
    {
      type: "user",
      sessionId: "native-domain",
      uuid: "n0",
      entrypoint: "cli",
      timestamp: "2026-10-31T01:00:00Z",
      message: { role: "user", content: "native domain fixture" },
    },
    {
      type: "assistant",
      sessionId: "native-domain",
      uuid: "n1",
      timestamp: "2026-10-31T01:00:01Z",
      message: {
        id: "response-domain",
        model: "fixture",
        content: [{ type: "text", text: "native body" }],
        usage: { input_tokens: 3, output_tokens: 1 },
      },
    },
  ]
    .map(JSON.stringify)
    .join("\n") + "\n",
);
const rows = JSON.parse(
  JSON.stringify(
    fs
      .readFileSync("sdk/plugins/fixtures/aurora.jsonl", "utf8")
      .trim()
      .split("\n")
      .map(JSON.parse),
  ),
);
rows[0].messages = Array.from({ length: 120 }, (_, n) => ({
  role: n % 2 ? "assistant" : "user",
  text: `domain body ${n}`,
}));
const initial = rows.map(JSON.stringify).join("\n") + "\n";
fs.writeFileSync(source, initial);
const digest = (p) =>
  createHash("sha256").update(fs.readFileSync(p)).digest("hex");
const nativeHash = digest(native);
const report = {
  complete: false,
  binary,
  binarySha256: digest(binary),
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
};
const passed = (name) => {
  report.cases.push(name);
  console.log(name);
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
          reject(Error("owned service failed to stop"));
        }, 8000);
        timer.unref();
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}
async function boot(executable = binary) {
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
  child.stderr.on("data", (b) => (stderr = (stderr + b).slice(-4000)));
  const lines = createInterface({ input: child.stdout });
  const [line] = await Promise.race([
    once(lines, "line", { signal: AbortSignal.timeout(20000) }),
    once(child, "exit").then(() => {
      throw Error(stderr);
    }),
  ]);
  base = `http://127.0.0.1:${JSON.parse(line).port}`;
}
async function api(
  route,
  body,
  expected = 200,
  method = body === undefined ? "GET" : "POST",
) {
  const response = await fetch(base + route, {
    method,
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(40000),
  });
  const text = await response.text();
  assert(text, `${route}: empty HTTP ${response.status}`);
  let value;
  try {
    value = JSON.parse(text);
  } catch {
    throw Error(
      `${route}: HTTP ${response.status} invalid JSON: ${text.slice(0, 200)}`,
    );
  }
  assert.equal(response.status, expected, `${route}: ${JSON.stringify(value)}`);
  return value;
}
let plugin;
const sourceId = "org.example.aurora-agent:source";
const query = (extra = {}) => ({
  scope: { kind: "external", sourceIds: [sourceId] },
  filter: { query: null, productId: null, days: null },
  snapshotToken: null,
  cursor: null,
  ...extra,
});
const action = (a) =>
  api("/api/plugins/action", {
    pluginId: plugin.manifest.id,
    digest: plugin.digest,
    action: a,
  });
const reviewConsent = () =>
  api("/api/domain/consent-review", {
    pluginId: plugin.manifest.id,
    digest: plugin.digest,
  });
const consent = async (enabled, persistMetadata = false) =>
  api("/api/domain/consent", {
    pluginId: plugin.manifest.id,
    digest: plugin.digest,
    reviewRef: (await reviewConsent()).reviewRef,
    enabled,
    persistMetadata,
  });
const page = (q) => api("/api/domain/sessions", q ?? query());
const oldView = () =>
  api("/api/plugins/view", {
    pluginId: plugin.manifest.id,
    viewId: "history",
    cursor: null,
  });
const activity = async () =>
  JSON.stringify(
    (await api("/api/plugins")).diagnostics.items.map((i) => i.activity),
  );
async function samples(load) {
  for (let n = 0; n < 5; n++) await load();
  const values = [];
  for (let n = 0; n < 40; n++) {
    const start = performance.now();
    await load();
    values.push(performance.now() - start);
  }
  values.sort((a, b) => a - b);
  return {
    samples: 40,
    p50Ms: values[19],
    p95Ms: values[37],
    p99Ms: values[39],
    failures: 0,
  };
}
try {
  const schemas = path.join(root, "contract-preflight");
  fs.mkdirSync(schemas);
  execFileSync(binary, ["--plugin-sdk", "schemas-domain", schemas]);
  assert.equal(
    fs.readdirSync(schemas).filter((n) => n.endsWith(".schema.json")).length,
    16,
    "pinned binary must implement the complete D2 contract",
  );
  assert(
    JSON.parse(
      fs.readFileSync(path.join(schemas, "source-consent.schema.json")),
    ).required.includes("reviewRef"),
  );
  await boot();
  const originalSchema = readCoreSchema(path.join(data, "local-agent.sqlite3"));
  assert.equal(originalSchema.version, 15);
  assert.equal((await fetch(base + "/api/domain/directory")).status, 401);
  assert.equal(
    (
      await fetch(base + "/api/domain/directory", {
        headers: {
          authorization: `Bearer ${token}`,
          origin: "https://untrusted.invalid",
        },
      })
    ).status,
    401,
  );
  await api("/api/domain/directory");
  assert(!fs.existsSync(path.join(data, "domain-host.sqlite3")));
  const key = path.join(root, "publisher.key");
  execFileSync(binary, ["--plugin-sdk", "keygen", key]);
  const folder = path.join(root, "package");
  fs.cpSync("sdk/plugins/examples/aurora-agent/package", folder, {
    recursive: true,
  });
  const pkg = path.join(root, "aurora.laplug");
  execFileSync(binary, ["--plugin-sdk", "pack", folder, pkg, key]);
  const review = await api("/api/plugins/inspect", { path: pkg });
  plugin = await api("/api/plugins/install", {
    packageRef: review.packageRef,
    digest: review.digest,
    approvePublisher: true,
  });
  plugin = await api("/api/plugins/state", {
    pluginId: plugin.manifest.id,
    digest: plugin.digest,
    action: "enable",
  });
  await api("/api/plugins/source", {
    pluginId: plugin.manifest.id,
    digest: plugin.digest,
    path: source,
  });
  const sourceReview = await reviewConsent();
  await api("/api/plugins/source", {
    pluginId: plugin.manifest.id,
    digest: plugin.digest,
    path: source,
  });
  assert.equal(
    (
      await api(
        "/api/domain/consent",
        {
          pluginId: plugin.manifest.id,
          digest: plugin.digest,
          reviewRef: sourceReview.reviewRef,
          enabled: true,
          persistMetadata: false,
        },
        403,
      )
    ).code,
    "grant_revoked",
  );
  await action({ kind: "refresh" });
  const unapproved = await page();
  assert.equal(unapproved.coverage, "unavailable");
  assert.equal(unapproved.items.length, 0);
  assert.equal((await oldView()).total, 2);
  passed(
    "management authentication and lazy domain storage; original grant does not authorize unified display",
  );
  const firstReview = await reviewConsent();
  assert.equal(firstReview.resourceLabel, source);
  assert.equal(firstReview.sourceAuthorized, true);
  assert.equal(firstReview.unifiedAuthorized, false);
  const approve = { ...firstReview, enabled: true, persistMetadata: false };
  delete approve.resourceLabel;
  delete approve.sourceAuthorized;
  delete approve.unifiedAuthorized;
  await api("/api/domain/consent", approve);
  await api("/api/domain/consent", approve);
  assert.equal(
    (await api("/api/domain/consent", { ...approve, enabled: false }, 400))
      .code,
    "invalid_input",
  );
  let first = await page();
  assert.equal(first.items.length, 2);
  assert.equal(first.sourceSummaries[0].usage.totalTokens, "9007199254741000");
  assert.equal(first.sourceSummaries[0].usage.cachedInputTokens, "2");
  assert.equal(first.sourceSummaries[0].usage.reasoningOutputTokens, "1");
  assert.equal(first.items[0].record.id, (await oldView()).items[0].sessionId);
  assert.equal(first.items[0].record.projectRef, null);
  const attempts = await activity();
  for (let n = 0; n < 5; n++) {
    await page();
    await api("/api/domain/directory");
  }
  assert.equal(await activity(), attempts);
  passed(
    "unchanged basic package maps to common records with exact large usage and zero metadata-query worker activity",
  );
  const bodyReq = {
    sessionId: first.items[0].record.id,
    snapshotToken: first.snapshotToken,
    cursor: null,
    anchor: null,
  };
  let body = await api("/api/domain/conversation", bodyReq);
  const crossObjectCursor = body.conversation.nextCursor;
  assert.equal(
    (
      await api(
        "/api/domain/conversation",
        {
          ...bodyReq,
          sessionId: first.items[1].record.id,
          cursor: crossObjectCursor,
        },
        409,
      )
    ).code,
    "stale_snapshot",
  );
  let texts = body.conversation.events.map((e) => e.text);
  while (body.conversation.nextCursor) {
    body = await api("/api/domain/conversation", {
      ...bodyReq,
      cursor: body.conversation.nextCursor,
    });
    texts.push(...body.conversation.events.map((e) => e.text));
  }
  assert.deepEqual(
    texts,
    rows[0].messages.map((m) => m.text),
  );
  passed(
    "real Wasm conversation bridge reads all 120 messages in three pages without synthetic usage or calls",
  );
  await stop();
  await boot();
  const cold = await page();
  assert.equal(cold.coverage, "unavailable");
  assert.equal(cold.sourceStates[0].authorization, "authorized");
  assert.equal(cold.items.length, 0);
  assert.equal(cold.sourceSummaries.length, 0);
  await action({ kind: "refresh" });
  first = await page();
  assert.equal(first.items.length, 2);
  passed(
    "default metadata is transient; restart retains consent but neither scans nor fabricates an empty successful snapshot",
  );
  await api(
    "/api/session-sources",
    { agentId: "claude", enabled: true },
    200,
    "PUT",
  );
  for (let n = 0; n < 100; n++) {
    if ((await api("/api/sessions?agent=claude")).total === 1) break;
    await new Promise((r) => setTimeout(r, 50));
  }
  const mixed = await page(
    query({ scope: { kind: "combined", sourceIds: ["claude", sourceId] } }),
  );
  assert.equal(mixed.items.length, 3);
  assert.equal(mixed.sourceSummaries.length, 2);
  assert.equal(mixed.overlapUnknown, true);
  assert.equal((await api("/api/sessions?agent=claude")).total, 1);
  const nativeRow = mixed.items.find((i) => i.sourceId === "claude");
  assert(nativeRow);
  assert.equal(
    (
      await api("/api/domain/conversation", {
        sessionId: nativeRow.record.id,
        snapshotToken: mixed.snapshotToken,
        cursor: null,
        anchor: null,
      })
    ).conversation.events[1].text,
    "native body",
  );
  passed(
    "combined scope includes chosen builtin and external sources while old native totals stay unchanged",
  );
  const many = Array.from({ length: 513 }, (_, n) => ({
    ...rows[1],
    nativeId: `paged-${String(n).padStart(3, "0")}`,
    title: `Paged ${n}`,
    usage: {
      inputTokens: "2",
      outputTokens: "1",
      cacheReadTokens: null,
      reasoningTokens: null,
    },
  }));
  const manyText = many.map(JSON.stringify).join("\n") + "\n";
  fs.writeFileSync(source, manyText);
  await action({ kind: "refresh" });
  let paged = await page(),
    ids = [];
  const obsolete = {
    snapshotToken: paged.snapshotToken,
    cursor: paged.nextCursor,
  };
  for (;;) {
    ids.push(...paged.items.map((i) => i.record.id));
    assert.equal(paged.sourceSummaries[0].sessions, "513");
    assert.equal(paged.sourceSummaries[0].usage.totalTokens, "1539");
    if (!paged.nextCursor) break;
    paged = await page(
      query({ snapshotToken: paged.snapshotToken, cursor: paged.nextCursor }),
    );
  }
  assert.equal(ids.length, 513);
  assert.equal(new Set(ids).size, 513);
  report.metadataLatency = {
    legacy: await samples(oldView),
    unified: await samples(() => page()),
  };
  await action({ kind: "refresh" });
  assert.equal(
    (await api("/api/domain/sessions", query(obsolete), 409)).code,
    "stale_snapshot",
  );
  passed(
    "513-record frozen keyset pagination preserves full-scope summaries and invalidates old generations",
  );
  const staleReview = await reviewConsent();
  await consent(false);
  assert.equal(
    (
      await api(
        "/api/domain/consent",
        {
          pluginId: plugin.manifest.id,
          digest: plugin.digest,
          reviewRef: staleReview.reviewRef,
          enabled: true,
          persistMetadata: false,
        },
        409,
      )
    ).code,
    "stale_snapshot",
  );
  assert.equal((await page()).coverage, "unavailable");
  assert.equal((await oldView()).total, 513);
  assert.equal(
    (await api("/api/plugins")).items.find((p) => p.digest === plugin.digest)
      .sourceAuthorized,
    true,
  );
  assert.equal(
    (await api("/api/domain/conversation", bodyReq, 409)).code,
    "stale_snapshot",
  );
  passed(
    "unified consent withdrawal invalidates cursors and body refs without revoking original resource authorization",
  );
  await consent(true, true);
  let persisted = await page();
  const restartReview = await reviewConsent();
  await stop();
  await boot();
  assert.equal(
    (
      await api(
        "/api/domain/consent",
        {
          pluginId: plugin.manifest.id,
          digest: plugin.digest,
          reviewRef: restartReview.reviewRef,
          enabled: false,
          persistMetadata: false,
        },
        409,
      )
    ).code,
    "stale_snapshot",
  );
  passed(
    "consent reviews bind the visible resource and current consent; identical replay is idempotent and changed, revoked or restarted reviews fail closed",
  );
  let restored = await page();
  assert.equal(restored.items.length, 40);
  assert.equal(restored.sourceSummaries[0].sessions, "513");
  assert.equal(restored.sourceStates[0].freshness, "stale");
  assert.equal(restored.coverage, "partial");
  assert.equal(
    (
      await api("/api/domain/conversation", {
        sessionId: restored.items[0].record.id,
        snapshotToken: restored.snapshotToken,
        cursor: null,
        anchor: null,
      })
    ).conversation.events.length,
    0,
  );
  assert.equal(
    (
      await api(
        "/api/domain/sessions",
        query({
          snapshotToken: persisted.snapshotToken,
          cursor: persisted.nextCursor,
        }),
        409,
      )
    ).code,
    "stale_snapshot",
  );
  passed(
    "explicit metadata persistence survives restart as stale metadata; old process snapshot tokens do not survive",
  );
  fs.writeFileSync(source, manyText + '{"incomplete":');
  const failed = await api(
    "/api/plugins/action",
    {
      pluginId: plugin.manifest.id,
      digest: plugin.digest,
      action: { kind: "refresh" },
    },
    400,
  );
  assert(failed.code);
  const retained = await page();
  assert.equal(retained.sourceSummaries[0].sessions, "513");
  assert.equal(retained.coverage, "partial");
  assert.equal(
    (
      await api(
        "/api/domain/conversation",
        {
          sessionId: retained.items[0].record.id,
          snapshotToken: retained.snapshotToken,
          cursor: null,
          anchor: null,
        },
        409,
      )
    ).code,
    "stale_snapshot",
  );
  fs.writeFileSync(source, manyText);
  passed(
    "failed incomplete scans retain the last complete metadata and changed source content cannot be read under an old snapshot",
  );
  await api("/api/plugins/state", {
    pluginId: plugin.manifest.id,
    digest: plugin.digest,
    action: "revoke-grants",
  });
  assert.equal((await page()).coverage, "unavailable");
  assert.equal(digest(native), nativeHash);
  assert.deepEqual(
    readCoreSchema(path.join(data, "local-agent.sqlite3")),
    originalSchema,
  );
  passed(
    "resource revocation disables unified projection; native source bytes and schema 15 are preserved",
  );
  const oldDigest = plugin.digest;
  async function reconnect() {
    await api("/api/plugins/source", {
      pluginId: plugin.manifest.id,
      digest: plugin.digest,
      path: source,
    });
    await consent(true, true);
    await action({ kind: "refresh" });
    return page();
  }
  const prior = await reconnect();
  const stableIds = prior.items.map((i) => i.record.id);
  const manifestPath = path.join(folder, "manifest.json");
  const manifest = JSON.parse(fs.readFileSync(manifestPath));
  manifest.version = "1.1.0";
  fs.writeFileSync(manifestPath, JSON.stringify(manifest));
  const upgradedFile = path.join(root, "aurora-upgrade.laplug");
  execFileSync(binary, ["--plugin-sdk", "pack", folder, upgradedFile, key]);
  const upgradedReview = await api("/api/plugins/inspect", {
    path: upgradedFile,
  });
  const upgraded = await api("/api/plugins/install", {
    packageRef: upgradedReview.packageRef,
    digest: upgradedReview.digest,
    approvePublisher: true,
  });
  plugin = await api("/api/plugins/state", {
    pluginId: upgraded.manifest.id,
    digest: upgraded.digest,
    action: "enable",
  });
  assert.equal((await page()).coverage, "unavailable");
  const newer = await reconnect();
  assert.deepEqual(
    newer.items.map((i) => i.record.id),
    stableIds,
  );
  plugin = await api("/api/plugins/state", {
    pluginId: plugin.manifest.id,
    digest: plugin.digest,
    action: "rollback",
  });
  assert.equal(plugin.digest, oldDigest);
  assert.equal((await page()).coverage, "unavailable");
  const rolled = await reconnect();
  assert.deepEqual(
    rolled.items.map((i) => i.record.id),
    stableIds,
  );
  passed(
    "upgrade and package rollback require fresh source and unified consent while preserving original session IDs",
  );
  const baseline =
    process.env.DOMAIN_BASELINE_BINARY &&
    path.resolve(process.env.DOMAIN_BASELINE_BINARY);
  if (baseline) {
    await stop();
    await boot(baseline);
    assert.equal(
      (await api("/api/plugins")).items.find((p) => p.digest === oldDigest)
        .enabled,
      true,
    );
    assert.equal((await oldView()).needsRefresh, true);
    assert.deepEqual(
      readCoreSchema(path.join(data, "local-agent.sqlite3")),
      originalSchema,
    );
    await stop();
    await boot();
    const again = await page();
    assert.equal(again.sourceSummaries[0].sessions, "513");
    assert.deepEqual(
      again.items.map((i) => i.record.id),
      stableIds,
    );
    passed(
      "same managed data reopens under D1 and D2 without modifying main or legacy plugin formats",
    );
  }
  const installedPackage = path.join(
    data,
    "plugins/packages",
    plugin.manifest.id,
    plugin.manifest.version,
    plugin.digest,
    "package.laplug",
  );
  const originalPackage = fs.readFileSync(installedPackage);
  const cachedPage = await page();
  fs.writeFileSync(
    installedPackage,
    Buffer.concat([originalPackage, Buffer.from([0])]),
  );
  assert.equal((await page()).sourceSummaries[0].sessions, "513");
  const execution = await fetch(base + "/api/domain/conversation", {
    method: "POST",
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({
      sessionId: cachedPage.items[0].record.id,
      snapshotToken: cachedPage.snapshotToken,
      cursor: null,
      anchor: null,
    }),
    signal: AbortSignal.timeout(10000),
  });
  assert.notEqual(execution.status, 200);
  const executionError = await execution.json();
  assert(["signature_invalid", "invalid_input"].includes(executionError.code));
  await consent(false);
  assert.equal((await page()).coverage, "unavailable");
  fs.writeFileSync(installedPackage, originalPackage);
  passed(
    "metadata reuses its approved complete snapshot; modified executable packages are rejected before body execution",
  );
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
