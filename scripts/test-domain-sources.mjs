// Offline process comparison of D2 and the migrated built-in domain sources.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { createHash, randomUUID } from "node:crypto";
const binary = path.resolve(
  process.env.DOMAIN_SOURCES_BINARY ?? "target/debug/local-agent-service",
);
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "local-agent-domain-sources-")),
);
const home = path.join(root, "home"),
  data = path.join(root, "data");
const now = new Date(Date.now() - 60000).toISOString();
const project = path.join(home, "managed-project");
fs.mkdirSync(project, { recursive: true });
const fixtures = [
  [
    ".claude/projects/fixture/claude.jsonl",
    [
      {
        type: "user",
        sessionId: "domain-claude",
        uuid: "u",
        entrypoint: "cli",
        cwd: project,
        timestamp: now,
        message: { role: "user", content: "Claude fixture question" },
      },
      {
        type: "assistant",
        sessionId: "domain-claude",
        uuid: "a",
        entrypoint: "cli",
        timestamp: now,
        message: {
          id: "r-claude",
          model: "fixture",
          content: [
            { type: "text", text: "Claude fixture reply" },
            ...Array.from({ length: 70 }, (_, n) => ({
              type: "tool_use",
              id: `mcp-${n}`,
              name: "mcp__fixture__read",
              input: {},
            })),
          ],
          usage: { input_tokens: 10, output_tokens: 2 },
        },
      },
      {
        type: "user",
        sessionId: "domain-claude",
        uuid: "mcp-results",
        entrypoint: "cli",
        timestamp: now,
        message: {
          content: Array.from({ length: 70 }, (_, n) => ({
            type: "tool_result",
            tool_use_id: `mcp-${n}`,
            content: "Managed MCP result",
          })),
        },
      },
    ],
  ],
  [
    ".codex/sessions/2026/10/31/codex.jsonl",
    [
      {
        type: "session_meta",
        timestamp: now,
        payload: {
          id: "domain-codex",
          originator: "codex_cli_rs",
          cwd: project,
        },
      },
      {
        type: "turn_context",
        timestamp: now,
        payload: { turn_id: "t", model: "fixture" },
      },
      {
        type: "response_item",
        timestamp: now,
        payload: {
          id: "u",
          type: "message",
          role: "user",
          content: [{ type: "input_text", text: "Codex fixture question" }],
        },
      },
      {
        type: "token_usage_record",
        timestamp: now,
        payload: {
          response_id: "r-codex",
          usage: { input_tokens: 20, output_tokens: 3 },
        },
      },
      {
        type: "response_item",
        timestamp: now,
        payload: {
          id: "a",
          type: "message",
          role: "assistant",
          content: [{ type: "output_text", text: "Codex fixture reply" }],
        },
      },
      {
        type: "response_item",
        timestamp: now,
        originator: "codex_cli_rs",
        payload: {
          type: "function_call",
          call_id: "codex-mcp",
          name: "mcp__fixture__read",
          arguments: "{}",
        },
      },
      {
        type: "response_item",
        timestamp: now,
        originator: "codex_cli_rs",
        payload: {
          type: "function_call_output",
          call_id: "codex-mcp",
          output: "Managed Codex MCP result",
        },
      },
    ],
  ],
];
for (const [name, rows] of fixtures) {
  const f = path.join(home, name);
  fs.mkdirSync(path.dirname(f), { recursive: true });
  fs.writeFileSync(f, rows.map((r) => JSON.stringify(r) + "\n").join(""));
}
const copied = path.join(home, ".codex/sessions/2026/10/31/copy.jsonl");
fs.copyFileSync(path.join(home, fixtures[1][0]), copied);
const mcpConfigs = [
  path.join(home, ".claude.json"),
  path.join(home, ".codex/config.toml"),
];
fs.writeFileSync(
  mcpConfigs[0],
  JSON.stringify({
    mcpServers: {
      fixture: {
        command: "never-run",
        env: { TOKEN: "private-mcp-config-fixture" },
      },
      ...Object.fromEntries(
        Array.from({ length: 81 }, (_, n) => [
          `configuration-${String(n).padStart(3, "0")}`,
          { command: "never-run" },
        ]),
      ),
    },
  }),
);
fs.writeFileSync(mcpConfigs[1], "[mcp_servers.fixture]\ncommand='never-run'\n");
const memoryFixtures = [
  ".claude/CLAUDE.md",
  ".codex/memories/MEMORY.md",
  ".config/opencode/AGENTS.md",
  ".gemini/config/GEMINI.md",
].map((name) => path.join(home, name));
for (const file of memoryFixtures) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(
    file,
    "api_key=fixturememorysecret123456\n" + "只读记忆🙂".repeat(5000),
  );
}
const memoryProducts = [
  "claude-code-cli",
  "claude-code-desktop",
  "codex-cli",
  "codex-desktop",
  "opencode",
  "antigravity",
];
const sha = (p) =>
  createHash("sha256").update(fs.readFileSync(p)).digest("hex");
const hashes = Object.fromEntries(
  [
    ...fixtures.map(([n]) => path.join(home, n)),
    copied,
    ...memoryFixtures,
    ...mcpConfigs,
  ].map((p) => [p, sha(p)]),
);
const report = {
  complete: false,
  binarySha256: sha(binary),
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
};
let child, base, token;
async function stop() {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const done = once(child, "exit");
  child.kill();
  await Promise.race([
    done,
    new Promise((_, reject) => {
      const t = setTimeout(() => {
        child.kill("SIGKILL");
        reject(Error("owned process did not stop"));
      }, 8000);
      t.unref();
      done.finally(() => clearTimeout(t));
    }),
  ]);
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
  let error = "";
  child.stderr.on("data", (b) => {
    error = (error + b).slice(-2000);
  });
  const reader = createInterface({ input: child.stdout });
  const [line] = await Promise.race([
    once(reader, "line", { signal: AbortSignal.timeout(20000) }),
    once(child, "exit").then(() => {
      throw Error(error);
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
    signal: AbortSignal.timeout(20000),
  });
  const text = await response.text();
  if (Array.isArray(expected)) {
    assert(
      expected.includes(response.status),
      `${route}: ${response.status}: ${text}`,
    );
  } else {
    assert.equal(response.status, expected, `${route}: ${text}`);
  }
  return JSON.parse(text);
}
function stable(v) {
  v = structuredClone(v);
  function visit(v) {
    if (v && typeof v === "object") {
      for (const k of ["asOf", "historyAsOf", "lastScan", "lastSuccessfulScan"])
        delete v[k];
      // Both accepted D2 and the new reader bind opaque cursors to process-local
      // index generations. Compare their presence; traverse/replay them below.
      for (const k of ["nextCursor", "cursor"])
        if (typeof v[k] === "string") v[k] = "<runtime-bound-cursor>";
      Object.values(v).forEach(visit);
    }
  }
  visit(v);
  return v;
}
const query = (sourceIds, kind = "builtin", extra = {}) => ({
  scope: { kind, sourceIds },
  filter: { query: null, productId: null, days: null },
  snapshotToken: null,
  cursor: null,
  ...extra,
});
const page = (q) => api("/api/domain/sessions", q);
async function freshFacts(q) {
  assert(
    !q.snapshotToken && !q.cursor,
    "frozen queries must never be retried as fresh",
  );
  for (let n = 0; n < 150; n++) {
    const value = await api("/api/domain/facts", q, [200, 409]);
    if (value.code !== "stale_snapshot") return value;
    report.freshFactRetries = (report.freshFactRetries ?? 0) + 1;
    await new Promise((r) => setTimeout(r, 40));
  }
  throw Error("Native publication did not settle for a fresh fact query");
}
async function legacy() {
  let sessions;
  for (let n = 0; n < 150; n++) {
    sessions = await api("/api/sessions");
    if (sessions.total === 2) break;
    await new Promise((r) => setTimeout(r, 40));
  }
  assert.equal(sessions.total, 2, "both persisted sources finish reopening");
  const calls = await api("/api/calls");
  const mcp = await api("/api/mcp");
  assert.equal(mcp.totalCalls, 71);
  assert(!JSON.stringify(mcp).includes("private-mcp-config-fixture"));
  const mcpPages = [];
  const mcpIds = [];
  let cursor = null;
  do {
    const page = await api(
      "/api/mcp" + (cursor ? `?cursor=${encodeURIComponent(cursor)}` : ""),
    );
    assert.equal(page.totalCalls, 71);
    assert.deepEqual(page.groups, mcp.groups);
    assert.deepEqual(page.usageSummary, mcp.usageSummary);
    for (const row of page.calls) mcpIds.push(row.call.id);
    mcpPages.push(page);
    if (cursor) {
      const replay = await api(`/api/mcp?cursor=${encodeURIComponent(cursor)}`);
      assert.deepEqual(stable(replay), stable(page));
      await api(
        `/api/mcp?agent=claude-code-cli&cursor=${encodeURIComponent(cursor)}`,
        undefined,
        409,
      );
    }
    cursor = page.nextCursor;
  } while (cursor);
  assert.deepEqual(
    mcpPages.map((p) => p.calls.length),
    [40, 31],
  );
  assert.equal(new Set(mcpIds).size, 71);
  const accounts = await api("/api/accounts");
  const memory = [];
  for (const product of memoryProducts) {
    const route = `/api/agents/${product}/memory`;
    const metadata = await api(route);
    assert(metadata.files.length > 0);
    assert.equal(metadata.selected, null);
    assert(!JSON.stringify(metadata).includes("fixturememorysecret123456"));
    const previews = [];
    for (const file of metadata.files) {
      const preview = await api(`${route}?fileId=${file.id}`);
      assert.equal(preview.selected.id, file.id);
      assert.equal(preview.selected.truncated, true);
      assert.equal([...preview.selected.text].length, 16384);
      assert(!preview.selected.text.includes("fixturememorysecret123456"));
      previews.push(preview);
    }
    await api(`${route}?fileId=not-an-authorized-memory-id`, undefined, 404);
    memory.push({ metadata, previews });
  }
  report.runtimeRevisions ??= [];
  report.runtimeRevisions.push(
    mcp.sources.map((s) => {
      assert(
        Number.isSafeInteger(s.authorizationRevision) &&
          s.authorizationRevision >= 1,
      );
      return {
        sourceId: s.agentId,
        revision: s.authorizationRevision,
        enabled: s.enabled,
      };
    }),
  );
  // These are process-local generation counters in accepted D2 too. Compare
  // persisted authorization flags, and retain the actual counters in the report.
  // The live revocation/frozen-query check above tests their effect separately.
  for (const frame of [mcp, ...mcpPages])
    for (const source of frame.sources) delete source.authorizationRevision;
  const bodies = [];
  for (const s of sessions.items)
    bodies.push(await api("/api/sessions/" + encodeURIComponent(s.id)));
  let nativeActivities;
  for (let n = 0; n < 150; n++) {
    const overview = await api(
      "/api/overview?source=native&range=24&metric=calls&dimension=agent",
    );
    nativeActivities = overview.nativeActivities;
    if (
      ["claude", "codex"].every((agent) =>
        nativeActivities.some((row) => row.agentId === agent),
      ) &&
      sessions.items.every((session) =>
        nativeActivities.some(
          (row) =>
            row.sessionId === session.id &&
            row.title === session.title &&
            row.project === session.projectName,
        ),
      )
    )
      break;
    await new Promise((r) => setTimeout(r, 40));
  }
  assert.deepEqual(
    [...new Set(nativeActivities.map((row) => row.agentId))].sort(),
    ["claude", "codex"],
  );
  assert(
    !JSON.stringify(nativeActivities).includes("private-mcp-config-fixture"),
  );
  for (const session of sessions.items) {
    const row = nativeActivities.find((row) => row.sessionId === session.id);
    assert.equal(
      row.title,
      session.title,
      "activity metadata catches up with the published index",
    );
    assert.equal(row.project, session.projectName);
  }
  nativeActivities.sort((a, b) => a.id.localeCompare(b.id));
  return stable({
    sessions,
    calls,
    bodies,
    mcp,
    mcpPages,
    accounts,
    memory,
    nativeActivities,
  });
}
try {
  await boot();
  const capabilities = await api("/api/domain/capabilities");
  assert.equal(capabilities.entries.length, 34);
  assert.equal(capabilities.externalProblem, null);
  assert.equal(
    capabilities.entries.filter(
      (e) => e.provider.capability.name === "domain.session-source",
    ).length,
    4,
  );
  assert.equal(
    capabilities.entries.filter(
      (e) => e.provider.capability.name === "domain.mcp-source",
    ).length,
    5,
  );
  assert.equal(
    capabilities.entries.filter(
      (e) => e.provider.capability.name === "domain.account-source",
    ).length,
    6,
  );
  assert(
    capabilities.entries
      .filter((e) => e.provider.capability.name === "domain.account-source")
      .every(
        (e) =>
          e.status.authorization === "required" &&
          e.status.freshness === "unread",
      ),
  );
  const directory = await api("/api/domain/directory");
  assert.equal(directory.catalog.sources.length, 4);
  assert.equal(directory.catalog.products.length, 6);
  assert(
    directory.catalog.sources.every(
      (s) => s.status.authorization === "required",
    ),
  );
  const unavailable = await page(query(["codex", "opencode"]));
  assert.equal(unavailable.coverage, "unavailable");
  assert.equal(unavailable.sourceStates.length, 2);
  assert.deepEqual(unavailable.sourceSummaries, []);
  await api("/api/domain/sessions", query(["codex"], "external"), 400);
  await api(
    "/api/domain/sessions",
    query(["codex", "opencode"], "combined"),
    400,
  );
  for (const agentId of ["claude", "codex"])
    await api("/api/session-sources", { agentId, enabled: true }, 200, "PUT");
  for (let n = 0; n < 150; n++) {
    if ((await api("/api/sessions")).total === 2) break;
    await new Promise((r) => setTimeout(r, 40));
  }
  const old = await api("/api/sessions");
  assert.equal(old.total, 2);
  const both = await page(query(["claude", "codex"]));
  const activityQuery = {
    scope: query(["claude", "codex"]).scope,
    rangeSeconds: 86400,
    productId: null,
    sessionId: null,
    includeGateway: false,
    includeHistorical: true,
  };
  let publishedActivity;
  for (let n = 0; n < 150; n++) {
    publishedActivity = await api("/api/domain/activity", activityQuery);
    if (
      ["claude", "codex"].every((id) =>
        publishedActivity.items.some((r) => r.sourceId === id),
      )
    )
      break;
    await new Promise((r) => setTimeout(r, 40));
  }
  assert.equal(publishedActivity.sourceSummaries.length, 2);
  assert.deepEqual(
    [...new Set(publishedActivity.items.map((r) => r.sourceId))].sort(),
    ["claude", "codex"],
  );
  assert(
    publishedActivity.items.every(
      (r) => r.kind === "native-message" && r.logicalRequestId === null,
    ),
  );
  assert(!JSON.stringify(publishedActivity).includes("fixture reply"));
  assert(!JSON.stringify(publishedActivity).includes(project));
  assert(
    publishedActivity.items.every((r) => r.measurements.liveRate === null),
  );
  await api("/api/domain/activity", { ...activityQuery, rangeSeconds: 0 }, 400);
  const invalidActivity = await fetch(base + "/api/domain/activity", {
    method: "POST",
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({ ...activityQuery, unexpected: true }),
  });
  assert.equal(invalidActivity.status, 422);
  assert((await invalidActivity.text()).includes("unknown field"));
  report.cases.push(
    "unified activity reads published native metadata with independent message units, bounded sampling and no transcript or path; invalid ranges and unknown fields are rejected",
  );
  const factQuery = {
    scope: query(["claude", "codex"]).scope,
    kind: "response",
    projectId: null,
    sessionId: null,
    responseId: null,
    snapshotToken: null,
    cursor: null,
  };
  const factPage = await api("/api/domain/facts", factQuery);
  const nativeCalls = (await api("/api/calls?kind=model")).items.filter(
    (c) => c.kind === "model",
  );
  assert.deepEqual(
    factPage.items.map((r) => r.id).sort(),
    nativeCalls.map((c) => c.id).sort(),
  );
  assert.equal(factPage.sourceSummaries.length, 2);
  for (const record of factPage.items) {
    const original = nativeCalls.find((c) => c.id === record.id);
    assert.equal(record.identity.evidence, "builtin-native");
    assert.equal(record.value.value.sessionRef, original.sessionId);
    assert.equal(record.value.value.productId, original.agentProductId);
    assert.equal(
      record.value.value.usage.inputTokens,
      original.usage.inputTokens == null
        ? null
        : String(original.usage.inputTokens),
    );
  }
  const nativeSessionFacts = await api("/api/domain/facts", {
    ...factQuery,
    kind: "session",
  });
  assert.deepEqual(
    nativeSessionFacts.items.map((r) => r.id).sort(),
    both.items.map((r) => r.record.id).sort(),
  );
  const factBody = await api("/api/domain/fact-conversation", {
    sessionId: nativeSessionFacts.items.find(
      (r) => r.identity.sourceId === "codex",
    ).id,
    snapshotToken: nativeSessionFacts.snapshotToken,
    cursor: null,
    anchor: null,
  });
  assert(
    factBody.conversation.events.some((e) =>
      e.text?.includes("Codex fixture reply"),
    ),
  );
  report.cases.push(
    "native response and session facts preserve original IDs, exact links, product attribution and usage; common query reads the existing index without a new scan",
  );
  const catalogQuery = { ...factQuery, kind: "mcp-catalog" };
  const waitCatalog = async (condition) => {
    let result;
    for (let n = 0; n < 150; n++) {
      result = await api("/api/domain/facts", catalogQuery, [200, 409]);
      if (result.code === "stale_snapshot") {
        // Only a fresh selection may retry a concurrent publication. Frozen
        // cursor/body checks below still require explicit rejection, never retry.
        report.freshCatalogRetries = (report.freshCatalogRetries ?? 0) + 1;
        await new Promise((r) => setTimeout(r, 40));
        continue;
      }
      if (condition(result)) return result;
      await new Promise((r) => setTimeout(r, 40));
    }
    assert.fail(
      `MCP catalog did not publish expected state: ${JSON.stringify(result.sourceStates)}`,
    );
  };
  const completeCatalog = (page) =>
    page.sourceStates.every((s) => s.coverage === "complete") &&
    page.sourceSummaries.find((s) => s.sourceId === "claude")?.mcpCatalogs ===
      "82";
  const catalogFirst = await waitCatalog(completeCatalog);
  const catalogPages = [catalogFirst];
  while (catalogPages.at(-1).nextCursor) {
    const previous = catalogPages.at(-1);
    const q = {
      ...catalogQuery,
      snapshotToken: previous.snapshotToken,
      cursor: previous.nextCursor,
    };
    const next = await api("/api/domain/facts", q);
    assert.deepEqual(await api("/api/domain/facts", q), next);
    catalogPages.push(next);
  }
  assert.deepEqual(
    catalogPages.map((p) => p.items.length),
    [40, 40, 3],
  );
  assert.equal(
    new Set(catalogPages.flatMap((p) => p.items.map((r) => r.id))).size,
    83,
  );
  for (const row of catalogPages.flatMap((p) => p.items)) {
    assert.equal(row.value.value.toolNames, null);
    assert.equal(
      row.identity.providerKey,
      "builtin:domain.mcp-source@1:native",
    );
  }
  assert(!JSON.stringify(catalogPages).includes("private-mcp-config-fixture"));
  const scopedCatalog = await api("/api/domain/facts", {
    ...catalogQuery,
    sessionId: nativeSessionFacts.items.find(
      (r) => r.identity.sourceId === "codex",
    ).id,
  });
  assert.equal(scopedCatalog.items.length, 0);
  report.cases.push(
    "83 native configuration observations page 40/40/3 through the common fact consumer without inventing tools, execution, connection or historical links",
  );
  assert.equal(both.items.length, 2);
  assert.deepEqual(
    both.sourceSummaries.map((s) => [s.sourceId, s.sessions]).sort(),
    [
      ["claude", "1"],
      ["codex", "1"],
    ],
  );
  assert.equal((await api("/api/sessions")).total, old.total);
  assert(both.overlapUnknown);
  const codex = both.items.find((i) => i.sourceId === "codex");
  assert(codex);
  assert.deepEqual(codex.record.productIds, ["codex-cli"]);
  const body = await api("/api/domain/conversation", {
    sessionId: codex.record.id,
    snapshotToken: both.snapshotToken,
    cursor: null,
    anchor: null,
  });
  assert(
    body.conversation.events.some((e) =>
      e.text?.includes("Codex fixture reply"),
    ),
  );
  const onlyClaude = await page(query(["claude"]));
  await api(
    "/api/domain/conversation",
    {
      sessionId: codex.record.id,
      snapshotToken: onlyClaude.snapshotToken,
      cursor: null,
      anchor: null,
    },
    403,
  );
  await api(
    "/api/domain/sessions",
    query(["codex"], "builtin", {
      filter: { query: null, productId: "claude-code-cli", days: null },
    }),
    400,
  );
  report.cases.push(
    "four mounted sources represent six products; explicit multi-native scope, deduplication, body and cross-scope denial preserve the original reader",
    "a separate 34-entry read-only capability catalog includes actual source, project, Git, listener, account, MCP, activity and native/gateway request projection facades without refreshing resources",
  );
  await api(
    "/api/session-sources",
    { agentId: "codex", enabled: false },
    200,
    "PUT",
  );
  await api("/api/agents/codex-cli/memory", undefined, 403);
  await api(
    "/api/domain/sessions",
    { ...query(["claude", "codex"]), snapshotToken: both.snapshotToken },
    409,
  );
  const partial = await page(query(["claude", "codex"]));
  await api(
    "/api/domain/facts",
    { ...factQuery, snapshotToken: factPage.snapshotToken },
    409,
  );
  const remainingFacts = await freshFacts(factQuery);
  const revokedActivity = await api("/api/domain/activity", activityQuery);
  assert(revokedActivity.items.every((r) => r.sourceId === "claude"));
  const revokedCount = revokedActivity.sourceSummaries.find(
    (s) => s.sourceId === "codex",
  );
  assert.equal(revokedCount.observedRecords, null);
  assert.equal(
    revokedActivity.sourceStates.find((s) => s.state.sourceId === "codex").state
      .coverage,
    "unavailable",
  );
  assert(remainingFacts.items.every((r) => r.identity.sourceId === "claude"));
  assert.equal(remainingFacts.sourceSummaries.length, 1);
  assert.equal(partial.coverage, "partial");
  assert.equal(partial.items.length, 1);
  assert.equal(partial.sourceSummaries.length, 1);
  await api(
    "/api/session-sources",
    { agentId: "codex", enabled: true },
    200,
    "PUT",
  );
  for (let n = 0; n < 150; n++) {
    if ((await api("/api/sessions")).total === 2) break;
    await new Promise((r) => setTimeout(r, 40));
  }
  report.cases.push(
    "revocation invalidates frozen generations and retains healthy source results without fabricated zero totals",
  );
  if (process.env.DOMAIN_SOURCES_BASELINE_BINARY) {
    for (const agentId of ["opencode", "antigravity"])
      await api("/api/session-sources", { agentId, enabled: true }, 200, "PUT");
    const expected = await legacy();
    await stop();
    await boot(path.resolve(process.env.DOMAIN_SOURCES_BASELINE_BINARY));
    assert.deepEqual(await legacy(), expected);
    await stop();
    await boot();
    assert.deepEqual(await legacy(), expected);
    report.cases.push(
      "the same managed data reopens under accepted D2 and D3 with complete original session, call, body, MCP, account and six-product memory responses; metadata, explicit Unicode previews, redaction and exact not-found behavior are preserved",
      "71 native MCP executions retain complete name groups, response usage and 40/31 pagination across D2/current reopen; cursor replay works in its own generation and cross-filter reuse is rejected; opaque cursor bytes are process-bound and are compared by presence",
      "complete native Claude and Codex activity responses survive common-contract consumption and accepted D2/current round trips on the same managed files, without real agents or model calls",
    );
  }
  // A catalog publication changes the fact generation even when transcript metadata is unchanged.
  const originalConfiguration = fs.readFileSync(mcpConfigs[0]);
  const oldCatalog = await waitCatalog(completeCatalog);
  const bodyFacts = await api("/api/domain/facts", {
    ...factQuery,
    kind: "session",
  });
  const frozenBody = {
    sessionId: bodyFacts.items.find((r) => r.identity.sourceId === "codex").id,
    snapshotToken: bodyFacts.snapshotToken,
    cursor: null,
    anchor: null,
  };
  await api("/api/domain/fact-conversation", frozenBody);
  fs.writeFileSync(
    mcpConfigs[0],
    JSON.stringify({
      mcpServers: { "after-refresh": { command: "never-run" } },
    }),
  );
  await api("/api/sessions/refresh", {});
  const changedCatalog = await waitCatalog((p) =>
    p.items.some((r) => r.value.value.serverName === "after-refresh"),
  );
  assert.equal(
    changedCatalog.sourceSummaries.find((s) => s.sourceId === "claude")
      .mcpCatalogs,
    "1",
  );
  await api(
    "/api/domain/facts",
    {
      ...catalogQuery,
      snapshotToken: oldCatalog.snapshotToken,
      cursor: oldCatalog.nextCursor,
    },
    409,
  );
  await api("/api/domain/fact-conversation", frozenBody, 409);
  fs.writeFileSync(mcpConfigs[0], originalConfiguration);
  await api("/api/sessions/refresh", {});
  const restoredCatalog = await waitCatalog(completeCatalog);
  const sameNameConfig = JSON.parse(originalConfiguration);
  sameNameConfig.mcpServers.fixture.command = "changed-command-never-run";
  fs.writeFileSync(mcpConfigs[0], JSON.stringify(sameNameConfig));
  await api("/api/sessions/refresh", {});
  const sameNameCatalog = await waitCatalog(
    (p) =>
      completeCatalog(p) &&
      p.items[0].sourceRevision !== restoredCatalog.items[0].sourceRevision,
  );
  assert.deepEqual(
    sameNameCatalog.items.map((r) => r.value),
    restoredCatalog.items.map((r) => r.value),
  );
  await api(
    "/api/domain/facts",
    { ...catalogQuery, snapshotToken: restoredCatalog.snapshotToken },
    409,
  );
  await api("/api/domain/facts", {
    ...catalogQuery,
    snapshotToken: sameNameCatalog.snapshotToken,
  });
  await api(
    "/api/domain/facts",
    { ...catalogQuery, snapshotToken: changedCatalog.snapshotToken },
    409,
  );
  fs.writeFileSync(mcpConfigs[0], "{invalid");
  await api("/api/sessions/refresh", {});
  const failedCatalog = await waitCatalog((p) =>
    p.sourceStates.some(
      (s) => s.sourceId === "claude" && s.coverage === "partial",
    ),
  );
  assert.equal(
    failedCatalog.sourceSummaries.find((s) => s.sourceId === "claude")
      .mcpCatalogs,
    "82",
  );
  assert.equal(
    failedCatalog.sourceSummaries.find((s) => s.sourceId === "codex")
      .mcpCatalogs,
    "1",
  );
  await api(
    "/api/domain/facts",
    { ...catalogQuery, snapshotToken: sameNameCatalog.snapshotToken },
    409,
  );
  fs.writeFileSync(mcpConfigs[0], originalConfiguration);
  await api("/api/sessions/refresh", {});
  await waitCatalog(completeCatalog);
  report.cases.push(
    "configuration publication invalidates frozen catalog cursors and fact bodies, including same-name command changes; damaged configuration retains the previous complete source as partial, preserves healthy sources and recovers after an explicit refresh",
  );
  for (const [p, h] of Object.entries(hashes)) assert.equal(sha(p), h);
  report.complete = true;
} finally {
  await stop();
  fs.rmSync(root, { recursive: true, force: true });
  report.ownedServiceStopped =
    !child || child.exitCode !== null || child.signalCode !== null;
  report.temporaryDataRemoved = !fs.existsSync(root);
  if (process.env.DOMAIN_SOURCES_REPORT)
    fs.writeFileSync(
      process.env.DOMAIN_SOURCES_REPORT,
      JSON.stringify(report, null, 2) + "\n",
    );
}
console.log(JSON.stringify(report, null, 2));
