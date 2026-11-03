import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn, execFileSync } from "node:child_process";
import { createInterface } from "node:readline";
import { once } from "node:events";
import { randomUUID, createHash } from "node:crypto";
import { DatabaseSync } from "node:sqlite";
import ts from "typescript";

const { parseDomain } = await import(
  `data:text/javascript;base64,${Buffer.from(ts.transpileModule(fs.readFileSync("src/features/domain/contract.ts", "utf8"), { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } }).outputText).toString("base64")}`
);
const requestSchema = JSON.parse(
  fs.readFileSync(
    "contracts/domain/facts/unified-request-page.schema.json",
    "utf8",
  ),
);
let previousRequestSchema = null,
  previousShapeFrames = 0;

const binary = path.resolve(
  process.env.PLUGIN_SOURCE_BINARY ?? "target/debug/local-agent-service",
);
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "local-agent-source-host-")),
);
const data = path.join(root, "data"),
  home = path.join(root, "home");
fs.mkdirSync(home);
const sha = (b) => createHash("sha256").update(b).digest("hex");
const report = {
  complete: false,
  binarySha256: sha(fs.readFileSync(binary)),
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
};
let child, base, token;
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
          reject(Error("owned service did not stop"));
        }, 8000);
      }),
    ]);
  } finally {
    clearTimeout(timer);
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
  status = 200,
  method = body === undefined ? "GET" : "POST",
) {
  const r = await fetch(base + route, {
    method,
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(40000),
  });
  const text = await r.text();
  assert.equal(r.status, status, `${route}: ${text}`);
  const value = JSON.parse(text);
  if (route === "/api/domain/requests" && status === 200) {
    parseDomain(requestSchema, value);
    if (
      previousRequestSchema &&
      body?.sort === undefined &&
      body?.includeEvidence === undefined
    ) {
      parseDomain(previousRequestSchema, value);
      previousShapeFrames++;
    }
  }
  return value;
}
const sdk = (...args) =>
  execFileSync(binary, ["--plugin-sdk", ...args], {
    encoding: "utf8",
    timeout: 30000,
    stdio: ["ignore", "pipe", "pipe"],
  });
try {
  if (process.env.DOMAIN_REQUEST_QUERY_BASELINE_BINARY) {
    const previous = path.resolve(
        process.env.DOMAIN_REQUEST_QUERY_BASELINE_BINARY,
      ),
      schemas = path.join(root, "previous-query-schemas");
    execFileSync(previous, ["--plugin-sdk", "schemas-domain-facts", schemas], {
      timeout: 30000,
      stdio: "pipe",
    });
    previousRequestSchema = JSON.parse(
      fs.readFileSync(
        path.join(schemas, "unified-request-page.schema.json"),
        "utf8",
      ),
    );
    report.previousQueryBaselineSha256 = sha(fs.readFileSync(previous));
  }
  const key = path.join(root, "key");
  sdk("keygen", key);
  const pkg = path.join(root, "full.laplug");
  sdk(
    "pack",
    path.resolve("sdk/plugins/examples/full-history/package"),
    pkg,
    key,
  );
  const manifest = JSON.parse(sdk("check", pkg)).manifest;
  const usage = (input, output) => ({
    inputTokens: String(input),
    outputTokens: String(output),
    cachedInputTokens: null,
    cacheWriteInputTokens: null,
    reasoningOutputTokens: null,
    totalTokens: String(input + output),
  });
  const events = Array.from({ length: 120 }, (_, i) => ({
    id: `e${i}`,
    nativeId: `e${i}`,
    turnId: `t${i}`,
    kind: "message",
    role: i % 2 ? "assistant" : "user",
    status: "recorded",
    text: `Managed fixture message ${i}`,
    input: null,
    output: null,
    model: null,
    responseId: null,
    usage: null,
  }));
  const rows = [
    {
      fact: {
        kind: "project",
        value: {
          nativeId: "p1",
          name: "来源报告项目",
          pathHint: path.join(root, "never-authorized-workspace"),
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
          title: "完整只读会话",
          createdAt: null,
          updatedAt: null,
          projectRef: "p1",
          usage: usage(15, 4),
          partial: false,
        },
      },
      conversation: events,
    },
    {
      fact: {
        kind: "response",
        value: {
          nativeId: "r1",
          sessionRef: "s1",
          productId: `${manifest.id}:terminal`,
          occurredAt: null,
          model: "fixture",
          outcome: "success",
          usage: usage(10, 2),
          reportedResponseId: "unverified-external-id",
          durationMs: "12",
        },
      },
    },
    {
      fact: {
        kind: "activity",
        value: {
          nativeId: "a1",
          sessionRef: "s1",
          responseRef: "r1",
          occurredAt: null,
          phase: "generating",
          durationMs: "10",
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
          responseRef: "r1",
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
          responseRef: "r1",
          toolName: "lookup",
          occurredAt: null,
          durationMs: null,
          outcome: "unknown",
        },
      },
    },
  ];
  const file = path.join(root, "history.jsonl");
  fs.writeFileSync(file, rows.map((r) => JSON.stringify(r) + "\n").join(""));
  const original = sha(fs.readFileSync(file));
  await boot();
  assert.equal((await api("/api/plugin-sources")).items.length, 0);
  assert.equal((await api("/api/plugins")).items.length, 0);
  const nativeBefore = await api("/api/sessions");
  assert.equal(nativeBefore.total, 0);
  const review = await api("/api/plugin-sources/inspect", { path: pkg });
  assert.equal(review.trustedPublisher, false);
  const install = {
    packageRef: review.packageRef,
    digest: review.digest,
    approvePublisher: false,
    expectedSigningKey: review.signingKey,
  };
  assert.equal(
    (await api("/api/plugin-sources/install", install, 400)).code,
    "publisher_untrusted",
  );
  const storeFile = path.join(data, "plugin-source-host.sqlite3");
  const installFault = new DatabaseSync(storeFile);
  installFault.exec(
    "CREATE TRIGGER reject_package BEFORE INSERT ON full_packages BEGIN SELECT RAISE(ABORT,'storage failure fixture'); END;",
  );
  assert.equal(
    (
      await api(
        "/api/plugin-sources/install",
        { ...install, approvePublisher: true },
        400,
      )
    ).code,
    "unavailable",
  );
  assert.equal((await api("/api/plugin-sources")).items.length, 0);
  assert(
    !fs.existsSync(
      path.join(
        data,
        "plugin-sources/packages",
        manifest.id,
        manifest.version,
        review.digest,
        "package.laplug",
      ),
    ),
  );
  installFault.exec("DROP TRIGGER reject_package");
  installFault.close();
  report.cases.push(
    "failed package persistence removes the new owned archive, keeps the package unselected and allows the exact reviewed install to retry",
  );
  let status = await api("/api/plugin-sources/install", {
    ...install,
    approvePublisher: true,
  });
  assert(!status.enabled && !status.sourceAuthorized && !status.runtimeActive);
  assert.equal(status.factSummary, null);
  const q = { pluginId: manifest.id, digest: review.digest };
  status = await api("/api/plugin-sources/state", { package: q, action: "enable" });
  assert(status.enabled && status.runtimeActive);
  assert(!status.sourceAuthorized && !status.unifiedAuthorized);
  assert.equal(status.factSummary, null);
  await api("/api/plugin-sources/refresh", q, 403);
  status = await api("/api/plugin-sources/source", { package: q, path: file });
  assert(
    status.sourceAuthorized &&
      !status.unifiedAuthorized &&
      !status.persistMetadata,
  );
  assert.equal(status.factSummary, null);
  status = await api("/api/plugin-sources/refresh", q);
  assert.equal(status.freshness, "current");
  assert.equal(status.factSummary.projects, "1");
  assert.equal(status.factSummary.sessions.unit, "session-cumulative");
  assert.equal(status.factSummary.sessions.usage.inputTokens, "15");
  assert.equal(status.factSummary.responses.unit, "model-response");
  assert.equal(status.factSummary.responses.usage.inputTokens, "10");
  assert.equal(status.factSummary.mcpCatalogs, "1");
  assert.equal(status.factSummary.mcpExecutions, "1");
  assert(status.factSummary.crossSourceOverlapUnknown);
  const sourceId = `${manifest.id}:history`;
  const scope = { kind: "external", sourceIds: [sourceId] };
  const sessionQuery = {
    scope,
    filter: { query: null, productId: null, days: null },
    snapshotToken: null,
    cursor: null,
  };
  const beforeConsent = await api("/api/domain/sessions", sessionQuery);
  assert.equal(beforeConsent.coverage, "unavailable");
  assert.equal(beforeConsent.sourceSummaries.length, 0);
  const consent = await api("/api/domain/consent-review", q);
  await api("/api/domain/consent", {
    ...q,
    enabled: true,
    persistMetadata: false,
    reviewRef: consent.reviewRef,
  });
  const directory = await api("/api/domain/directory");
  assert(
    directory.catalog.products.some((p) => p.id === `${manifest.id}:terminal`),
  );
  const unified = await api("/api/domain/sessions", sessionQuery);
  assert.equal(unified.items.length, 1);
  assert.equal(unified.sourceSummaries[0].usage.inputTokens, "15");
  const factQuery = {
    scope,
    kind: "project",
    projectId: null,
    sessionId: null,
    responseId: null,
    snapshotToken: null,
    cursor: null,
  };
  const projects = await api("/api/domain/facts", factQuery);
  assert.equal(projects.items.length, 1);
  assert.equal(projects.sourceSummaries[0].sessions.usage.inputTokens, "15");
  assert.equal(projects.sourceSummaries[0].responses.usage.inputTokens, "10");
  const projectId = projects.items[0].id;
  const requestQuery = {
    scope,
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
  };
  const reportedRequests = await api("/api/domain/requests", requestQuery);
  assert.equal(reportedRequests.items.length, 1);
  assert.equal(reportedRequests.sourceSummaries[0].records, "1");
  assert.equal(reportedRequests.sourceSummaries[0].usage.totalTokens, "12");
  assert.equal(reportedRequests.sourceSummaries[0].unknownTime, "1");
  const claimed = reportedRequests.items[0];
  assert.equal(claimed.evidence, "source-reported");
  assert.equal(claimed.hostRequestId, null);
  assert.equal(claimed.record.attempts, null);
  assert.equal(claimed.record.durationMs, null);
  assert.equal(claimed.record.ttftMs, null);
  assert.equal(claimed.reportedDurationMs, "12");
  assert.equal(claimed.reportedResponseId, "unverified-external-id");
  const enhancedReported = await api("/api/domain/requests", {
    ...requestQuery,
    sort: "tokens",
    includeEvidence: true,
  });
  assert.equal(
    enhancedReported.items[0].evidenceDetail.matching.kind,
    "source-reported",
  );
  assert.equal(enhancedReported.items[0].evidenceDetail.billing, null);
  assert.equal(
    enhancedReported.sourceSummaries[0].evidenceSummary.matching
      .unverifiedRecords,
    "1",
  );
  assert.equal(
    enhancedReported.sourceSummaries[0].evidenceSummary.billing,
    null,
  );
  if (previousRequestSchema)
    assert.throws(() => parseDomain(previousRequestSchema, enhancedReported));
  const datedRequests = await api("/api/domain/requests", {
    ...requestQuery,
    filter: { ...requestQuery.filter, start: "2026-10-01T00:00:00Z" },
  });
  assert.equal(datedRequests.items.length, 0);
  assert.equal(datedRequests.sourceSummaries[0].records, "0");
  assert.equal(datedRequests.sourceSummaries[0].unknownTime, "1");
  const unsupportedRequests = await api("/api/domain/requests", {
    ...requestQuery,
    unit: "logical-request",
  });
  assert.equal(unsupportedRequests.items.length, 0);
  assert.equal(unsupportedRequests.sourceSummaries[0].records, null);
  await api(
    "/api/domain/requests",
    {
      ...requestQuery,
      snapshotToken: reportedRequests.snapshotToken,
      filter: { ...requestQuery.filter, model: "unrelated" },
    },
    409,
  );
  await api(
    "/api/domain/requests",
    {
      ...requestQuery,
      snapshotToken: reportedRequests.snapshotToken,
      cursor: "foreign:40",
    },
    409,
  );
  report.cases.push(
    "unified response query consumes published source facts with full filtered summaries, explicit unknown attempts/host timing, half-open dated exclusion of unknown event time and foreign selection/cursor rejection",
  );
  const projectSessions = await api("/api/domain/facts", {
    ...factQuery,
    kind: "session",
    projectId,
  });
  assert.equal(projectSessions.items[0].id, unified.items[0].record.id);
  const factBodyRequest = {
    sessionId: unified.items[0].record.id,
    snapshotToken: projectSessions.snapshotToken,
    cursor: null,
    anchor: null,
  };
  const factBody = await api("/api/domain/fact-conversation", factBodyRequest);
  assert.equal(factBody.conversation.events.length, 40);
  assert.equal(factBody.snapshotToken, projectSessions.snapshotToken);
  await api("/api/domain/conversation", factBodyRequest, 409);
  const responses = await api("/api/domain/facts", {
    ...factQuery,
    kind: "response",
    sessionId: unified.items[0].record.id,
  });
  assert.equal(responses.items.length, 1);
  assert.equal(
    responses.items[0].value.value.sessionRef,
    unified.items[0].record.id,
  );
  assert.equal(
    responses.items[0].value.value.reportedResponseId,
    "unverified-external-id",
  );
  const responseId = responses.items[0].id;
  const activities = await api("/api/domain/facts", {
    ...factQuery,
    kind: "activity",
    responseId,
  });
  assert.equal(activities.items.length, 1);
  assert.equal(activities.sourceSummaries[0].responses.records, "1");
  const activityQuery = {
    scope,
    rangeSeconds: 86400,
    productId: null,
    sessionId: null,
    includeGateway: false,
    includeHistorical: true,
  };
  const sourceBeforeActivity = sha(fs.readFileSync(file));
  const unifiedActivity = await api("/api/domain/activity", activityQuery);
  assert.equal(unifiedActivity.items.length, 1);
  assert.equal(unifiedActivity.items[0].kind, "source-event");
  assert.equal(unifiedActivity.items[0].state, "historical");
  assert.equal(unifiedActivity.items[0].measurements, null);
  assert.equal(unifiedActivity.items[0].logicalRequestId, null);
  assert.equal(unifiedActivity.items[0].sessionId, unified.items[0].record.id);
  assert.equal(unifiedActivity.sourceSummaries[0].historical, "1");
  assert.equal(unifiedActivity.sourceSummaries[0].unknownTime, "1");
  assert.equal(unifiedActivity.sourceStates[0].state.coverage, "partial");
  assert.equal(
    unifiedActivity.sourceStates[0].state.problem,
    "unknown_event_time",
  );
  assert.equal(unifiedActivity.items[0].title, unified.items[0].record.title);
  assert.deepEqual(
    unifiedActivity.items[0].productIds,
    unified.items[0].record.productIds,
  );
  assert.equal(unifiedActivity.sourceSummaries[0].logicalRequests, null);
  assert.equal(sha(fs.readFileSync(file)), sourceBeforeActivity);
  assert.equal(
    (
      await api("/api/domain/activity", {
        ...activityQuery,
        productId: `${manifest.id}:terminal`,
      })
    ).items.length,
    1,
  );
  assert.equal(
    (
      await api("/api/domain/activity", {
        ...activityQuery,
        sessionId: "unverified-session",
      })
    ).items.length,
    0,
  );
  const omittedHistory = await api("/api/domain/activity", {
    ...activityQuery,
    includeHistorical: false,
  });
  assert.equal(omittedHistory.items.length, 0);
  assert.equal(omittedHistory.sourceSummaries[0].observedRecords, null);
  await api(
    "/api/domain/activity",
    { ...activityQuery, includeGateway: true },
    400,
  );
  report.cases.push(
    "unified activity consumes the authorized complete external graph without a resource read, keeps source events historical with no host realtime or logical-request identity, and preserves unknown excluded counts",
  );
  const unrelated = await api("/api/domain/facts", {
    ...factQuery,
    kind: "response",
    responseId: "unverified-gateway-id",
  });
  assert.equal(unrelated.items.length, 0);
  await api(
    "/api/domain/fact-conversation",
    { ...factBodyRequest, snapshotToken: unrelated.snapshotToken },
    403,
  );
  await api(
    "/api/domain/facts",
    { ...factQuery, kind: "response", snapshotToken: projects.snapshotToken },
    409,
  );
  await api("/api/domain/facts", { ...factQuery, cursor: "forged" }, 409);
  const bodyRequest = {
    sessionId: unified.items[0].record.id,
    snapshotToken: unified.snapshotToken,
    cursor: null,
    anchor: null,
  };
  const body = [];
  for (let cursor = null; ;) {
    const page = await api("/api/domain/conversation", {
      ...bodyRequest,
      cursor,
    });
    assert.equal(page.conversation.sessionId, bodyRequest.sessionId);
    body.push(...page.conversation.events);
    cursor = page.conversation.nextCursor;
    if (cursor === null) break;
  }
  assert.equal(body.length, 120);
  assert.equal(new Set(body.map((e) => e.id)).size, 120);
  const anchored = await api("/api/domain/conversation", {
    ...bodyRequest,
    anchor: "e80",
  });
  assert.equal(anchored.conversation.events[0].nativeId, "e80");
  await api("/api/domain/project-visibility", {
    sourceId,
    projectId,
    hidden: true,
  });
  await api("/api/domain/fact-conversation", factBodyRequest, 409);
  await api(
    "/api/domain/facts",
    { ...factQuery, snapshotToken: projects.snapshotToken },
    409,
  );
  await api("/api/domain/conversation", bodyRequest, 409);
  const hiddenPage = await api("/api/domain/facts", factQuery);
  assert.equal(hiddenPage.items.length, 0);
  assert.equal(hiddenPage.hiddenProjects[0].id, projectId);
  assert.equal(hiddenPage.sourceSummaries[0].projects, "0");
  assert.equal(hiddenPage.sourceSummaries[0].sessions.records, "1");
  assert.equal(hiddenPage.sourceSummaries[0].responses.records, "1");
  const redactedSession = await api("/api/domain/sessions", sessionQuery);
  assert.equal(redactedSession.items[0].record.projectRef, null);
  const redactedGraph = await api("/api/domain/facts", {
    ...factQuery,
    kind: "session",
  });
  assert.equal(redactedGraph.items[0].value.value.projectRef, null);
  await api(
    "/api/domain/project-visibility",
    { sourceId, projectId: "unverified-path", hidden: true },
    403,
  );
  await api("/api/domain/project-visibility", {
    sourceId,
    projectId,
    hidden: false,
  });
  assert.equal(
    (await api("/api/domain/facts", factQuery)).items[0].id,
    projectId,
  );
  const publishedSource = fs.readFileSync(file);
  fs.appendFileSync(file, "malformed-unread-source\n");
  try {
    const cachedActivity = await api("/api/domain/activity", activityQuery);
    assert.deepEqual(cachedActivity.items, unifiedActivity.items);
    assert.equal(cachedActivity.sourceStates[0].state.freshness, "stale");
    assert.equal(
      cachedActivity.sourceStates[0].state.problem,
      "unknown_event_time",
    );
    const cachedRequests = await api("/api/domain/requests", requestQuery);
    assert.deepEqual(cachedRequests.items, reportedRequests.items);
    assert.equal(cachedRequests.sourceStates[0].freshness, "stale");
  } finally {
    fs.writeFileSync(file, publishedSource);
  }
  const beforeRevokeFacts = await api("/api/domain/facts", factQuery);
  const beforeRevokeSessions = await api("/api/domain/sessions", sessionQuery);
  const beforeRevokeRequests = await api("/api/domain/requests", requestQuery);
  const revokeReview = await api("/api/domain/consent-review", q);
  await api("/api/domain/consent", {
    ...q,
    enabled: false,
    persistMetadata: false,
    reviewRef: revokeReview.reviewRef,
  });
  await api(
    "/api/domain/conversation",
    { ...bodyRequest, snapshotToken: beforeRevokeSessions.snapshotToken },
    409,
  );
  await api(
    "/api/domain/facts",
    { ...factQuery, snapshotToken: beforeRevokeFacts.snapshotToken },
    409,
  );
  assert.equal(
    (await api("/api/domain/facts", factQuery)).sourceSummaries.length,
    0,
  );
  assert.equal(
    (await api("/api/domain/sessions", sessionQuery)).coverage,
    "unavailable",
  );
  const revokedActivity = await api("/api/domain/activity", activityQuery);
  await api(
    "/api/domain/requests",
    { ...requestQuery, snapshotToken: beforeRevokeRequests.snapshotToken },
    409,
  );
  const revokedRequests = await api("/api/domain/requests", requestQuery);
  assert.equal(revokedRequests.items.length, 0);
  assert.equal(revokedRequests.sourceSummaries[0].records, null);
  assert.equal(revokedActivity.items.length, 0);
  assert(
    revokedActivity.sourceSummaries.every((s) => s.observedRecords === null),
  );
  report.cases.push(
    "external project hiding redacts references and removes project/write evidence without removing independently authorized sessions or responses; hide and restore invalidate both fact and body snapshots",
    "project, session, response, historical activity and independent unit summaries traverse one complete graph; upstream response claims never establish host links; changed query or revoked grants invalidate fact snapshots",
    "history source participates in the existing unified directory, scoped session query and explicit paged body only after independent display consent; revoke invalidates the frozen body without revoking the resource",
  );
  const validSource = fs.readFileSync(file);
  const invalidRows = structuredClone(rows);
  invalidRows[2].fact.value.sessionRef = "missing-session";
  fs.writeFileSync(
    file,
    invalidRows.map((r) => JSON.stringify(r) + "\n").join(""),
  );
  assert.equal(
    (await api("/api/plugin-sources/refresh", q, 400)).code,
    "invalid_record",
  );
  let degraded = (await api("/api/plugin-sources")).items[0];
  assert.equal(degraded.freshness, "failed");
  assert.equal(degraded.factSummary.sessions.usage.inputTokens, "15");
  assert.equal(degraded.factSummary.responses.usage.inputTokens, "10");
  const overflow = structuredClone(rows);
  overflow[2].fact.value.usage.inputTokens = "18446744073709551615";
  const extra = structuredClone(overflow[2]);
  extra.fact.value.nativeId = "overflow-response";
  overflow.push(extra);
  fs.writeFileSync(
    file,
    overflow.map((r) => JSON.stringify(r) + "\n").join(""),
  );
  assert.equal(
    (await api("/api/plugin-sources/refresh", q, 400)).code,
    "resource_limit",
  );
  degraded = (await api("/api/plugin-sources")).items[0];
  assert.equal(degraded.freshness, "failed");
  assert.equal(degraded.factSummary.responses.usage.inputTokens, "10");
  fs.writeFileSync(file, validSource);
  status = await api("/api/plugin-sources/refresh", q);
  assert.equal(status.freshness, "current");
  assert.equal((await api("/api/sessions")).total, nativeBefore.total);
  assert.equal((await api("/api/plugins")).items.length, 0);
  assert(!fs.existsSync(path.join(root, "never-authorized-workspace")));
  report.cases.push(
    "separate install and resource-free enable do not authorize or read a source; manual grant and refresh publish complete facts with independent units",
    "a dangling response reference rejects the entire generation and preserves previous complete facts with an explicit failed freshness state",
  );
  const marker = path.join(data, "plugin-sources", "selected", q.pluginId);
  for (const contents of [null, "f".repeat(64)]) {
    const markerConsent = await api("/api/domain/consent-review", q);
    await api("/api/domain/consent", {
      ...q,
      enabled: true,
      persistMetadata: false,
      reviewRef: markerConsent.reviewRef,
    });
    const markerQuery = { ...factQuery, kind: "session" };
    const visibleBefore = await api("/api/domain/facts", markerQuery);
    assert.equal(visibleBefore.items.length, 1);
    if (contents === null) fs.rmSync(marker);
    else fs.writeFileSync(marker, contents);
    const interrupted = (await api("/api/plugin-sources")).items[0];
    assert(interrupted.enabled);
    assert(!interrupted.runtimeActive && !interrupted.sourceAuthorized);
    assert.equal(interrupted.unifiedAuthorized, false);
    assert.equal(interrupted.factSummary, null);
    assert.equal(interrupted.problem, "grant_revoked");
    const hiddenAfter = await api("/api/domain/facts", markerQuery);
    assert.equal(hiddenAfter.sourceStates.length, 1);
    assert.equal(hiddenAfter.sourceStates[0].coverage, "unavailable");
    assert.deepEqual(hiddenAfter.items, []);
    assert.deepEqual(hiddenAfter.sourceSummaries, []);
    await api(
      "/api/domain/facts",
      {
        ...markerQuery,
        snapshotToken: visibleBefore.snapshotToken,
      },
      409,
    );
    await api(
      "/api/domain/fact-conversation",
      {
        sessionId: visibleBefore.items[0].id,
        snapshotToken: visibleBefore.snapshotToken,
        cursor: null,
        anchor: null,
      },
      409,
    );
    assert.equal(
      (await api("/api/plugin-sources/refresh", q, 403)).code,
      "grant_revoked",
    );
    status = await api("/api/plugin-sources/state", {
      package: q,
      action: "enable",
    });
    assert(status.runtimeActive && !status.sourceAuthorized);
    assert.equal(status.factSummary, null);
    assert.notEqual(status.activationEpoch, interrupted.activationEpoch);
    await api("/api/plugin-sources/source", { package: q, path: file });
    status = await api("/api/plugin-sources/refresh", q);
    assert(status.sourceAuthorized);
    assert.equal(status.freshness, "current");
  }
  report.cases.push(
    "missing or changed selection markers fail status and cached data delivery closed; explicit resource-free re-enable repairs the marker with a new epoch and requires a new source grant",
  );
  const firstEpoch = status.activationEpoch;
  status = await api("/api/plugin-sources/state", { package: q, action: "enable" });
  assert.equal(status.activationEpoch, firstEpoch);
  assert(status.sourceAuthorized);
  const pagedRows = structuredClone(rows);
  for (let n = 0; n < 82; n++) {
    const extra = structuredClone(rows[2]);
    extra.fact.value.nativeId = `paged-response-${String(n).padStart(3, "0")}`;
    pagedRows.push(extra);
  }
  fs.writeFileSync(
    file,
    pagedRows.map((r) => JSON.stringify(r) + "\n").join(""),
  );
  await api("/api/plugin-sources/refresh", q);
  const pageConsent = await api("/api/domain/consent-review", q);
  await api("/api/domain/consent", {
    ...q,
    enabled: true,
    persistMetadata: false,
    reviewRef: pageConsent.reviewRef,
  });
  const responseQuery = { ...factQuery, kind: "response" };
  let first = null;
  const pagedIds = [];
  const pageSizes = [];
  for (let cursor = null; ;) {
    const page = await api("/api/domain/facts", {
      ...responseQuery,
      snapshotToken: first?.snapshotToken ?? null,
      cursor,
    });
    first ??= page;
    assert.equal(page.sourceSummaries[0].responses.records, "83");
    assert.equal(page.sourceSummaries[0].responses.usage.inputTokens, "830");
    assert.equal(page.sourceSummaries[0].sessions.usage.inputTokens, "15");
    pageSizes.push(page.items.length);
    pagedIds.push(...page.items.map((r) => r.id));
    cursor = page.nextCursor;
    if (!cursor) break;
  }
  assert.deepEqual(pageSizes, [40, 40, 3]);
  assert.equal(new Set(pagedIds).size, 83);
  let requestFirst = null;
  const requestIds = [],
    requestSizes = [];
  for (let cursor = null; ;) {
    const page = await api("/api/domain/requests", {
      ...requestQuery,
      snapshotToken: requestFirst?.snapshotToken ?? null,
      cursor,
    });
    requestFirst ??= page;
    assert.equal(page.sourceSummaries[0].records, "83");
    assert.equal(page.sourceSummaries[0].usage.totalTokens, "996");
    assert(
      page.items.every(
        (r) => r.record.attempts === null && r.hostRequestId === null,
      ),
    );
    requestSizes.push(page.items.length);
    requestIds.push(...page.items.map((r) => r.record.id));
    cursor = page.nextCursor;
    if (!cursor) break;
  }
  assert.deepEqual(requestSizes, [40, 40, 3]);
  assert.equal(new Set(requestIds).size, 83);
  assert.deepEqual(
    (
      await api("/api/domain/requests", {
        ...requestQuery,
        snapshotToken: requestFirst.snapshotToken,
      })
    ).items,
    requestFirst.items,
  );
  const nativeFile = path.join(
    home,
    ".claude/projects/unified-request-fixture/native.jsonl",
  );
  fs.mkdirSync(path.dirname(nativeFile), { recursive: true });
  fs.writeFileSync(
    nativeFile,
    [
      {
        type: "user",
        sessionId: "unified-request-native",
        uuid: "u",
        entrypoint: "cli",
        timestamp: "2026-10-01T00:00:00Z",
        message: { role: "user", content: "owned request fixture" },
      },
      {
        type: "assistant",
        sessionId: "unified-request-native",
        uuid: "a",
        timestamp: "2026-10-01T00:00:01Z",
        message: {
          id: "unverified-external-id",
          model: "fixture",
          content: [{ type: "text", text: "private native body sentinel" }],
          usage: { input_tokens: 3, output_tokens: 1 },
        },
      },
    ]
      .map(JSON.stringify)
      .join("\n") + "\n",
  );
  const nativeFileHash = sha(fs.readFileSync(nativeFile));
  await api(
    "/api/session-sources",
    { agentId: "claude", enabled: true },
    200,
    "PUT",
  );
  let nativeReady = false;
  for (let i = 0; i < 150; i++) {
    const native = await api("/api/domain/requests", {
      ...requestQuery,
      scope: { kind: "builtin", sourceIds: ["claude"] },
    });
    if (native.items.length === 1) {
      nativeReady = true;
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 40));
  }
  assert(nativeReady, "owned native fixture must publish");
  const gatewayStore = new DatabaseSync(
    path.join(data, "observation/observations.sqlite3"),
  );
  const gatewayObserved = {
    id: "gateway-unified-evidence",
    agentId: "claude",
    agentProductId: "claude-code-cli",
    sessionId: null,
    responseId: "unverified-external-id",
    model: "fixture",
    provider: "Fixture",
    providerId: "fixture",
    endpoint: "https://openrouter.ai/api/v1",
    bindingRevision: 1,
    startedAt: "2026-10-01T00:00:00Z",
    endedAt: "2026-10-01T00:00:01Z",
    status: "success",
    httpStatus: 200,
    ttftMs: 0,
    durationMs: 1000,
    usage: { inputTokens: 3, outputTokens: 1, totalTokens: 4 },
    reportedCost: {
      amount: 0,
      currency: "USD",
      source: "openrouter.usage.cost",
    },
    protocol: "messages",
    revision: 1,
  };
  const gatewayRaw = JSON.stringify(gatewayObserved);
  gatewayStore
    .prepare("INSERT INTO model_observations VALUES(?,?,?,?)")
    .run(gatewayObserved.id, gatewayObserved.startedAt, 1, gatewayRaw);
  gatewayStore.close();
  const combinedQuery = {
    ...requestQuery,
    scope: { kind: "combined", sourceIds: ["claude", sourceId] },
  };
  const combined = await api("/api/domain/requests", combinedQuery);
  assert.equal(combined.items.length, 40);
  assert.equal(combined.items[0].evidence, "host-observed");
  assert.equal(
    combined.sourceSummaries.find((s) => s.sourceId === "claude").records,
    "1",
  );
  assert.equal(
    combined.sourceSummaries.find((s) => s.sourceId === sourceId).records,
    "83",
  );
  const combinedIds = combined.items.map((r) => r.record.id);
  for (let cursor = combined.nextCursor; cursor;) {
    const page = await api("/api/domain/requests", {
      ...combinedQuery,
      snapshotToken: combined.snapshotToken,
      cursor,
    });
    combinedIds.push(...page.items.map((r) => r.record.id));
    cursor = page.nextCursor;
  }
  assert.equal(combinedIds.length, 84);
  assert.equal(new Set(combinedIds).size, 84);
  for (const sort of ["time", "tokens", "duration", "ttft", "cost-usd"]) {
    for (const descending of [false, true]) {
      const selection = {
        ...combinedQuery,
        sort,
        descending,
        includeEvidence: true,
      };
      let first = null,
        items = [];
      for (let cursor = null; ;) {
        const page = await api("/api/domain/requests", {
          ...selection,
          snapshotToken: first?.snapshotToken ?? null,
          cursor,
        });
        first ??= page;
        assert.deepEqual(page.sourceSummaries, first.sourceSummaries);
        items.push(...page.items);
        cursor = page.nextCursor;
        if (!cursor) break;
      }
      assert.equal(items.length, 84);
      assert.equal(new Set(items.map((r) => r.record.id)).size, 84);
      assert(
        items
          .filter((r) => r.evidence === "source-reported")
          .every(
            (r) =>
              r.evidenceDetail.matching.kind === "source-reported" &&
              r.evidenceDetail.billing === null,
          ),
      );
      const host = items.find((r) => r.evidence === "host-observed");
      assert.equal(host.evidenceDetail.matching.kind, "native-response");
      assert.equal(host.evidenceDetail.matching.state, "exact-response-id");
      assert.equal(
        host.evidenceDetail.matching.logicalRequestId,
        gatewayObserved.id,
      );
      assert.equal(
        first.sourceSummaries.find((s) => s.sourceId === "claude")
          .evidenceSummary.matching.exact,
        "1",
      );
    }
  }
  const gatewayQuery = {
    ...requestQuery,
    scope: { kind: "builtin", sourceIds: ["claude"] },
    unit: "logical-request",
    sort: "cost-usd",
    includeEvidence: true,
  };
  const frozenGateway = await api("/api/domain/requests", gatewayQuery);
  assert.equal(
    frozenGateway.items[0].evidenceDetail.matching.matchedResponses,
    "1",
  );
  assert.equal(
    frozenGateway.items[0].evidenceDetail.billing.actual[0].amount,
    "0",
  );
  assert.equal(
    frozenGateway.items[0].evidenceDetail.billing.actual[0].currency,
    "USD",
  );
  assert.equal(
    frozenGateway.sourceSummaries[0].evidenceSummary.matching.withExactLinks,
    "1",
  );
  assert(!JSON.stringify(combined).includes("private native body sentinel"));
  const linkedNativeSession =
    frozenGateway.items[0].evidenceDetail.matching.links[0].sessionId;
  const linkedSessionFacts = await api("/api/domain/facts", {
    scope: { kind: "builtin", sourceIds: ["claude"] },
    kind: "session",
    projectId: null,
    sessionId: linkedNativeSession,
    responseId: null,
    snapshotToken: null,
    cursor: null,
  });
  assert(linkedSessionFacts.items.some((r) => r.id === linkedNativeSession));
  const explicitNativeBody = await api("/api/domain/fact-conversation", {
    sessionId: linkedNativeSession,
    snapshotToken: linkedSessionFacts.snapshotToken,
    cursor: null,
    anchor: null,
  });
  assert(
    explicitNativeBody.conversation.events.some((r) =>
      r.text?.includes("private native body sentinel"),
    ),
  );
  // The ten sort queries can evict the earlier eight-slot lease. Use and
  // replay a current lease so revocation, rather than eviction, is exercised.
  const beforeNativeRevoke = await api("/api/domain/requests", combinedQuery);
  assert.deepEqual(
    (
      await api("/api/domain/requests", {
        ...combinedQuery,
        snapshotToken: beforeNativeRevoke.snapshotToken,
      })
    ).items,
    beforeNativeRevoke.items,
  );
  assert.deepEqual(
    (
      await api("/api/domain/requests", {
        ...gatewayQuery,
        snapshotToken: frozenGateway.snapshotToken,
      })
    ).items,
    frozenGateway.items,
  );
  await api(
    "/api/session-sources",
    { agentId: "claude", enabled: false },
    200,
    "PUT",
  );
  await api(
    "/api/domain/requests",
    { ...combinedQuery, snapshotToken: beforeNativeRevoke.snapshotToken },
    409,
  );
  const degradedRequests = await api("/api/domain/requests", combinedQuery);
  await api(
    "/api/domain/requests",
    { ...gatewayQuery, snapshotToken: frozenGateway.snapshotToken },
    409,
  );
  const independentGateway = await api("/api/domain/requests", gatewayQuery);
  assert.equal(independentGateway.items.length, 1);
  assert.equal(
    independentGateway.items[0].evidenceDetail.matching.matchedResponses,
    "0",
  );
  assert.equal(
    independentGateway.items[0].evidenceDetail.billing.actual[0].amount,
    "0",
  );
  const retainedGatewayStore = new DatabaseSync(
    path.join(data, "observation/observations.sqlite3"),
  );
  assert.equal(
    retainedGatewayStore
      .prepare("SELECT value FROM model_observations WHERE id=?")
      .get(gatewayObserved.id).value,
    gatewayRaw,
  );
  retainedGatewayStore.close();
  report.cases.push(
    "all five unified sorts in both directions preserve 84-response full-scope pages and host-only exact matching; zero actual USD billing is retained, native revocation invalidates old enrichment while independently captured gateway history and raw observations survive",
  );
  assert.equal(
    degradedRequests.sourceSummaries.find((s) => s.sourceId === "claude")
      .records,
    null,
  );
  assert.equal(
    degradedRequests.sourceSummaries.find((s) => s.sourceId === sourceId)
      .records,
    "83",
  );
  assert.equal(sha(fs.readFileSync(nativeFile)), nativeFileHash);
  report.cases.push(
    "unified requests page external responses 40/40/3 with full 996-token scope, merge native+external as 84 distinct responses despite matching claimed IDs, and preserve the healthy external source after native revocation without reading bodies or rewriting native fixtures",
  );
  const replay = await api("/api/domain/facts", {
    ...responseQuery,
    snapshotToken: first.snapshotToken,
  });
  assert.deepEqual(
    replay.items.map((r) => r.id),
    pagedIds.slice(0, 40),
  );
  fs.writeFileSync(file, validSource);
  await api("/api/plugin-sources/refresh", q);
  await api(
    "/api/domain/requests",
    { ...requestQuery, snapshotToken: requestFirst.snapshotToken },
    409,
  );
  await api(
    "/api/domain/facts",
    { ...responseQuery, snapshotToken: first.snapshotToken },
    409,
  );
  report.cases.push(
    "83 response facts page 40/40/3 without skips or duplicates; every page retains full filtered independent-unit summaries; summary overflow rejects publication and marks refresh failed",
  );
  const persistReview = await api("/api/domain/consent-review", q);
  await api("/api/domain/consent", {
    ...q,
    enabled: true,
    persistMetadata: true,
    reviewRef: persistReview.reviewRef,
  });
  assert.equal((await api("/api/plugin-sources")).items[0].persistMetadata, true);
  status = await api("/api/plugin-sources/source", { package: q, path: file });
  assert(
    status.sourceAuthorized &&
      !status.unifiedAuthorized &&
      !status.persistMetadata,
  );
  assert.equal(status.factSummary, null);
  assert.equal(status.freshness, "unread");
  report.cases.push(
    "selecting a resource again clears old persisted derived facts and display consent atomically; unread never inherits the old saved generation",
  );
  status = await api("/api/plugin-sources/state", {
    package: q,
    action: "revoke-grants",
  });
  assert(status.runtimeActive && !status.sourceAuthorized);
  assert.equal(status.factSummary, null);
  await api("/api/plugin-sources/refresh", q, 403);
  report.cases.push(
    "identical enable preserves grants, explicit revoke removes queryable facts without stopping the registered reader",
  );
  const orphan = path.join(
    data,
    "plugin-sources/packages",
    manifest.id,
    "9.9.9",
    "f".repeat(64),
    "package.laplug",
  );
  fs.mkdirSync(path.dirname(orphan), { recursive: true });
  fs.copyFileSync(pkg, orphan);
  fs.writeFileSync(marker, "f".repeat(64));
  await stop("SIGKILL");
  const coldLock = new DatabaseSync(storeFile);
  coldLock.exec("BEGIN IMMEDIATE");
  await boot();
  const lockStarted = performance.now();
  assert.equal(
    (await api("/api/plugin-sources", undefined, 400)).code,
    "unavailable",
  );
  assert(performance.now() - lockStarted < 1000);
  assert.equal((await api("/api/sessions")).total, nativeBefore.total);
  coldLock.exec("ROLLBACK");
  coldLock.close();
  status = (await api("/api/plugin-sources")).items[0];
  assert(!fs.existsSync(orphan));
  assert.equal(fs.readFileSync(marker, "utf8"), q.digest);
  assert(
    status.enabled &&
      !status.runtimeActive &&
      !status.sourceAuthorized &&
      !status.unifiedAuthorized,
  );
  assert.equal(status.factSummary, null);
  report.cases.push(
    "forced owned-service death leaves recoverable marker/archive mismatches; restart reaps only uncommitted cache and restores the committed selection fence without a runtime or source grant; a cold optional-store lock returns within one second and retries without another restart",
  );
  status = await api("/api/plugin-sources/state", { package: q, action: "enable" });
  assert(status.runtimeActive && !status.sourceAuthorized);
  await api("/api/plugin-sources/source", { package: q, path: file });
  await api("/api/plugin-sources/refresh", q);
  const upgradeConsent = await api("/api/domain/consent-review", q);
  await api("/api/domain/consent", {
    ...q,
    enabled: true,
    persistMetadata: true,
    reviewRef: upgradeConsent.reviewRef,
  });
  const oldStatus = (await api("/api/plugin-sources")).items[0];
  const upgradeFolder = path.join(root, "upgrade-package");
  fs.cpSync(
    path.resolve("sdk/plugins/examples/full-history/package"),
    upgradeFolder,
    { recursive: true },
  );
  fs.writeFileSync(
    path.join(upgradeFolder, "manifest.json"),
    JSON.stringify({ ...manifest, version: "1.0.1" }),
  );
  const upgradeArchive = path.join(root, "upgrade.laplug");
  sdk("pack", upgradeFolder, upgradeArchive, key);
  const upgradeReview = await api("/api/plugin-sources/inspect", {
    path: upgradeArchive,
  });
  const upgradeInstall = {
    packageRef: upgradeReview.packageRef,
    digest: upgradeReview.digest,
    approvePublisher: false,
    expectedSigningKey: upgradeReview.signingKey,
  };
  await api("/api/plugin-sources/install", upgradeInstall);
  const upgraded = { pluginId: manifest.id, digest: upgradeReview.digest };
  const beforeUpgrade = await api("/api/domain/facts", {
    ...factQuery,
    kind: "session",
  });
  const commitFault = new DatabaseSync(storeFile);
  const projectionBefore = commitFault
    .prepare("SELECT record FROM full_projections WHERE plugin_id=?")
    .get(manifest.id).record;
  commitFault.exec(
    "CREATE TABLE commit_parent(id INTEGER PRIMARY KEY);CREATE TABLE commit_child(parent INTEGER REFERENCES commit_parent(id) DEFERRABLE INITIALLY DEFERRED);CREATE TRIGGER reject_commit AFTER INSERT ON full_selections BEGIN INSERT INTO commit_child VALUES(42); END;",
  );
  assert.equal(
    (
      await api(
        "/api/plugin-sources/state",
        { package: upgraded, action: "enable" },
        400,
      )
    ).code,
    "unavailable",
  );
  const preserved = (await api("/api/plugin-sources")).items.find(
    (p) => p.digest === q.digest,
  );
  assert(
    preserved.runtimeActive &&
      preserved.sourceAuthorized &&
      preserved.unifiedAuthorized,
  );
  assert.equal(preserved.activationEpoch, oldStatus.activationEpoch);
  assert.deepEqual(preserved.factSummary, oldStatus.factSummary);
  assert.equal(fs.readFileSync(marker, "utf8"), q.digest);
  assert.equal(
    commitFault
      .prepare("SELECT record FROM full_projections WHERE plugin_id=?")
      .get(manifest.id).record,
    projectionBefore,
  );
  await api("/api/domain/facts", {
    ...factQuery,
    kind: "session",
    snapshotToken: beforeUpgrade.snapshotToken,
  });
  commitFault.exec("DROP TRIGGER reject_commit");
  commitFault.close();
  status = await api("/api/plugin-sources/state", {
    package: upgraded,
    action: "enable",
  });
  assert(
    status.runtimeActive &&
      !status.sourceAuthorized &&
      !status.unifiedAuthorized,
  );
  assert.equal(fs.readFileSync(marker, "utf8"), upgraded.digest);
  await api(
    "/api/domain/facts",
    {
      ...factQuery,
      kind: "session",
      snapshotToken: beforeUpgrade.snapshotToken,
    },
    409,
  );
  status = await api("/api/plugin-sources/state", {
    package: upgraded,
    action: "rollback",
  });
  assert.equal(status.digest, q.digest);
  assert(status.runtimeActive && !status.sourceAuthorized);
  await api("/api/plugin-sources/state", { package: upgraded, action: "remove" });
  assert.equal((await api("/api/plugin-sources")).items.length, 1);
  await api("/api/plugin-sources/install", upgradeInstall);
  await api("/api/plugin-sources/state", { package: upgraded, action: "enable" });
  await api("/api/plugin-sources/state", { package: upgraded, action: "remove" });
  status = await api("/api/plugin-sources/state", { package: q, action: "enable" });
  assert(status.runtimeActive && !status.sourceAuthorized);
  assert.equal(
    (
      await api(
        "/api/plugin-sources/state",
        { package: q, action: "rollback" },
        404,
      )
    ).code,
    "not_found",
  );
  assert.equal((await api("/api/plugin-sources")).items.length, 1);
  report.cases.push(
    "a real upgrade whose SQLite commit fails restores the original marker, runtime, grants, saved facts and frozen queries; retry and rollback switch exact versions without inheriting source/display consent; removing an unselected version preserves the current selection, while removing the selected version never becomes a dangling rollback target after reactivation",
  );
  status = await api("/api/plugin-sources/state", {
    package: q,
    action: "disable",
  });
  assert(!status.enabled && !status.runtimeActive);
  report.cases.push(
    "restart retains package and enable intent but never restores runtime resource grants or starts the guest implicitly",
  );
  assert.equal(sha(fs.readFileSync(file)), original);
  report.previousQueryShapeFrames = previousShapeFrames;
  if (previousRequestSchema) {
    assert(previousShapeFrames > 10);
    report.cases.push(
      "previous query schema validates every ordinary query frame with no new fields; enhanced evidence is explicit opt-in and does not change the original query response shape",
    );
  }
  report.complete = true;
} finally {
  await stop();
  fs.rmSync(root, { recursive: true, force: true });
  report.ownedServiceStopped =
    !child || child.exitCode !== null || child.signalCode !== null;
  report.temporaryDataRemoved = !fs.existsSync(root);
  if (process.env.PLUGIN_SOURCE_REPORT)
    fs.writeFileSync(
      process.env.PLUGIN_SOURCE_REPORT,
      JSON.stringify(report, null, 2) + "\n",
    );
}
console.log(JSON.stringify(report, null, 2));
