import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { IconProvider } from "@/lib/icon-context";
import { UnifiedWorkspace } from "@/features/domain/session-workspace";
import { DomainConsentControls } from "@/features/domain/consent-controls";
import { domainApi } from "@/features/domain/api";
import { domainDirectoryResource } from "@/features/domain/queries";
import { parseDomain } from "@/features/domain/contract";
import type {
  UnifiedCatalog,
  UnifiedConversation,
  UnifiedSessionPage,
  UnifiedSessionQuery,
  SessionItem,
} from "@/features/domain/types";
import pageSchema from "../../contracts/domain/unified-session-page.schema.json";
import type {
  FactQuery,
  FactRecord,
  UnifiedActivityQuery,
  UnifiedActivityPage,
} from "../../contracts/domain/facts/types";
import activitySchema from "../../contracts/domain/facts/unified-activity-page.schema.json";
import requestSchema from "../../contracts/domain/facts/unified-request-page.schema.json";
import type {
  UnifiedRequestPage,
  UnifiedRequestQuery,
  UnifiedRequestItem,
} from "../../contracts/domain/facts/types";
import "@/styles/globals.css";

const params = new URLSearchParams(window.location.search);
const source = "org.example.reader:source",
  plugin = "org.example.reader";
let authorized = params.get("unapproved") !== "1",
  revision = 1,
  persisted = false;
let release: (() => void) | undefined;
let resourceLabel = "/temporary/reader.jsonl";
const calls: string[] = [];
let sessionRevision = 0;
const queries: UnifiedSessionQuery[] = [];
const activityQueries: UnifiedActivityQuery[] = [];
const requestQueries: UnifiedRequestQuery[] = [];
const factQueries: FactQuery[] = [];
const requestSnapshots = new Map<string, UnifiedRequestPage>();
if (params.has("requestExport")) {
  (globalThis as { isTauri?: boolean }).isTauri = true;
  mockIPC(async (name, args) => {
    if (name !== "export_unified_request_csv")
      throw Error(`Unexpected command ${name}`);
    const input = args as { snapshotToken: string };
    if (Object.keys(input).join(",") !== "snapshotToken")
      throw Error("Unexpected export arguments");
    calls.push(`export:${input.snapshotToken}`);
    const page = requestSnapshots.get(input.snapshotToken);
    if (!page) throw Error("Unknown frozen export");
    const requestedMode = params.get("requestExport") ?? "saved";
    const mode = requestedMode.replace(/^hold-/, "");
    if (requestedMode.startsWith("hold"))
      await new Promise<void>((resolve) => {
        release = resolve;
      });
    if (mode === "cancel") return { status: "cancelled" };
    if (mode === "revoke")
      throw JSON.stringify({
        code: "export_not_saved",
        outcomeUnknown: false,
        message: "来源授权已变化，导出未保存。",
      });
    const count = page.sourceSummaries
      .reduce((n, s) => n + BigInt(s.records ?? "0"), 0n)
      .toString();
    const info = {
      apiVersion: "1.0.0",
      operationId: "00000000-0000-4000-8000-000000000001",
      snapshotToken: input.snapshotToken,
      count: mode === "numeric" ? 83 : count,
      asOf: page.asOf,
      appliedQuery: { ...page.appliedQuery, snapshotToken: null, cursor: null },
      sourceStates: page.sourceStates,
      sourceSummaries: page.sourceSummaries,
      overlapUnknown: page.overlapUnknown,
    };
    return { status: "saved", info, fileName: "unified-requests.csv" };
  });
}
const activityAt = Date.now();
const usage = {
  inputTokens: "9007199254740993",
  outputTokens: "7",
  totalTokens: "9007199254741000",
  cachedInputTokens: null,
  cacheWriteInputTokens: null,
  reasoningOutputTokens: null,
};
const items: SessionItem[] = ["Alpha", "Beta"].map((name, n) => ({
  sourceId: source,
  record: {
    id: plugin + ":" + name,
    nativeId: name,
    productIds: [plugin + ":cli"],
    unknownClient: true,
    title: name,
    createdAt: null,
    updatedAt: null,
    projectRef: null,
    partial: false,
    usage: n
      ? { ...usage, inputTokens: "0", outputTokens: "0", totalTokens: "0" }
      : usage,
  },
}));
if (params.get("many") === "1") {
  const template = items[1];
  items.splice(
    0,
    items.length,
    ...Array.from({ length: 83 }, (_, n) => ({
      ...template,
      record: {
        ...template.record,
        id: `${plugin}:Paged-${String(n).padStart(3, "0")}`,
        nativeId: `Paged-${n}`,
        title: `Paged-${String(n).padStart(3, "0")}`,
      },
    })),
  );
}
const catalog = (): UnifiedCatalog => {
  const result: UnifiedCatalog = {
    catalog: {
      apiVersion: "1.0.0",
      revision: String(revision),
      providers: [
        {
          providerKey: "reader",
          pluginId: plugin,
          implementationVersion: "1.0.0",
          implementationIdentity: "a".repeat(64),
          executionKind: "wasm-component",
          capability: {
            name: "domain.session-source",
            major: 1,
            instanceId: source,
          },
        },
      ],
      products: [
        {
          id: plugin + ":cli",
          displayName: "Reader",
          sourceId: source,
          configId: null,
        },
      ],
      sources: [
        {
          id: source,
          providerKey: "reader",
          productIds: [plugin + ":cli"],
          status: {
            enabled: true,
            authorization: authorized ? "authorized" : "required",
            grantRevision: "1",
            runtimeActive: true,
            freshness: authorized ? "current" : "unread",
          },
        },
      ],
    },
    consents: [
      {
        pluginId: plugin,
        digest: "a".repeat(64),
        sourceId: source,
        enabled: authorized,
        persistMetadata: persisted,
      },
    ],
    externalProblem: null,
  };
  if (params.get("builtins") === "1")
    for (const [id, name] of [
      ["opencode", "OpenCode"],
      ["codex", "Codex CLI / Codex 桌面端"],
    ]) {
      const providerKey = `builtin:domain.session-source@1:${id}`;
      result.catalog.providers.push({
        providerKey,
        pluginId: `io.localagent.builtin.source.${id}`,
        implementationVersion: "0.1.8",
        implementationIdentity: `core-fixture:${id}`,
        executionKind: "builtin",
        capability: { name: "domain.session-source", major: 1, instanceId: id },
      });
      result.catalog.products.push({
        id,
        displayName: name,
        sourceId: id,
        configId: id,
      });
      result.catalog.sources.push({
        id,
        providerKey,
        productIds: [id],
        status: {
          enabled: params.get("requestHost") === "1" && id === "codex",
          authorization:
            params.get("requestHost") === "1" && id === "codex"
              ? "authorized"
              : "required",
          grantRevision: "1",
          runtimeActive: true,
          freshness:
            params.get("requestHost") === "1" && id === "codex"
              ? "current"
              : "unread",
        },
      });
    }
  return result;
};
domainApi.catalog = async () => {
  calls.push("catalog");
  return catalog();
};
domainApi.requests = async (query) => {
  requestQueries.push(structuredClone(query));
  calls.push(`requests:${query.unit}:${query.cursor ?? "first"}`);
  const snapshotToken = `requests-${revision}-${query.unit}-${query.sort ?? "time"}-${query.descending}-${query.includeEvidence === true}`;
  if (query.snapshotToken && query.snapshotToken !== snapshotToken)
    throw Error("请求快照已失效");
  const total =
    authorized &&
    query.unit === "model-response" &&
    query.scope.sourceIds.includes(source)
      ? 83
      : 0;
  const offset = query.cursor ? Number(query.cursor.split(":").at(-1)) : 0;
  const records: UnifiedRequestItem[] = Array.from(
    { length: total },
    (_, n) => ({
      record: {
        id: `reported-${String(n).padStart(3, "0")}`,
        sourceId: source,
        unit: query.unit,
        productId: plugin + ":cli",
        sessionRef: items[0].record.id,
        sessionTitle: `来源响应-${String(n).padStart(3, "0")}`,
        projectRef: null,
        projectName: null,
        occurredAt: null,
        endedAt: null,
        model: "fixture",
        providerKey: "reader",
        providerName: "Reader",
        outcome: "unknown",
        usage: {
          ...usage,
          totalTokens: (BigInt(usage.totalTokens) + BigInt(n)).toString(),
        },
        usageMissing: "0",
        attempts: null,
        durationMs: null,
        ttftMs: null,
        throughput: null,
      },
      evidence: "source-reported" as const,
      hostRequestId: null,
      reportedResponseId: "claim-only",
      reportedDurationMs: "9007199254740993",
      ...(query.includeEvidence
        ? {
            evidenceDetail: {
              matching: { kind: "source-reported" as const },
              billing: null,
              timingNote: null,
              overflowFields: [],
            },
          }
        : {}),
    }),
  ).filter(
    (r) =>
      !query.filter.search ||
      r.record.sessionTitle.includes(query.filter.search),
  );
  if (query.sort === "tokens")
    records.sort((a, b) => {
      const x = BigInt(a.record.usage.totalTokens ?? "0"),
        y = BigInt(b.record.usage.totalTokens ?? "0");
      return x === y ? 0 : (x < y ? -1 : 1) * (query.descending ? -1 : 1);
    });
  const available = authorized && query.unit === "model-response";
  const result: UnifiedRequestPage = {
    items: records.slice(offset, offset + 40),
    snapshotToken,
    nextCursor:
      offset + 40 < records.length ? `${snapshotToken}:${offset + 40}` : null,
    asOf: "2026-11-01T01:00:00Z",
    sourceStates: query.scope.sourceIds.includes(source)
      ? [
          {
            sourceId: source,
            authorization: authorized ? "authorized" : "required",
            freshness: authorized ? "current" : "unread",
            coverage: available ? "complete" : "unavailable",
            problem: available
              ? null
              : authorized
                ? "unsupported"
                : "permission_denied",
          },
        ]
      : [],
    sourceSummaries: query.scope.sourceIds.includes(source)
      ? [
          {
            sourceId: source,
            unit: query.unit,
            evidence: "source-reported",
            coverage: available ? "complete" : "unavailable",
            records: available ? String(records.length) : null,
            usage: available
              ? {
                  ...usage,
                  totalTokens: records
                    .reduce(
                      (s, r) => s + BigInt(r.record.usage.totalTokens ?? "0"),
                      0n,
                    )
                    .toString(),
                }
              : {
                  inputTokens: null,
                  outputTokens: null,
                  cachedInputTokens: null,
                  cacheWriteInputTokens: null,
                  reasoningOutputTokens: null,
                  totalTokens: null,
                },
            usageMissing: available ? "0" : null,
            attempts: null,
            unknownTime: available ? String(records.length) : null,
            ...(query.includeEvidence
              ? {
                  evidenceSummary: {
                    matching: {
                      kind: "source-reported" as const,
                      unverifiedRecords: available
                        ? String(records.length)
                        : null,
                    },
                    billing: null,
                    usageMissingParts: available ? "0" : null,
                    overflowFields: [],
                  },
                }
              : {}),
          },
        ]
      : [],
    appliedQuery: query,
    overlapUnknown: true,
  };
  if (
    params.get("requestHost") === "1" &&
    query.scope.sourceIds.includes("codex") &&
    query.unit === "logical-request"
  ) {
    const bill = {
      actual: [
        {
          currency: "USD",
          amount: params.get("invalidRequestMoney") === "1" ? "-1" : "0",
          parts: "1",
        },
      ],
      estimated: [{ currency: "CNY", amount: "2.5", parts: "1" }],
      unknownParts: "1",
    };
    result.items = [
      {
        record: {
          id: "gateway-fixture",
          sourceId: "codex",
          unit: "logical-request",
          productId: "codex",
          sessionRef: "codex:fixture",
          sessionTitle: "网关费用夹具",
          projectRef: null,
          projectName: null,
          occurredAt: null,
          endedAt: null,
          model: "fixture",
          providerKey: "gateway",
          providerName: "Fixture",
          outcome: "success",
          usage: {
            ...usage,
            inputTokens: "10",
            outputTokens: "2",
            totalTokens: "12",
          },
          usageMissing: "1",
          attempts: "3",
          durationMs: 10,
          ttftMs: 0,
          throughput: 2,
        },
        evidence: "host-observed",
        hostRequestId: "gateway-fixture",
        reportedResponseId: null,
        reportedDurationMs: null,
        evidenceDetail: {
          matching: {
            kind: "gateway-request",
            matchedResponses: "1",
            linksLimited: false,
            links: [
              {
                eventId: "e1",
                sessionId: "codex:fixture",
                responseId: "r1",
                attemptId: "a1",
                usageConflict: true,
              },
            ],
          },
          billing: bill,
          timingNote: "已记录 HTTP 计时",
          overflowFields: [],
        },
      },
    ];
    result.nextCursor = null;
    result.sourceStates.push({
      sourceId: "codex",
      authorization: "authorized",
      freshness: "current",
      coverage: "complete",
      problem: null,
    });
    result.sourceSummaries.push({
      sourceId: "codex",
      unit: "logical-request",
      evidence: "host-observed",
      coverage: "complete",
      records: "1",
      usage: result.items[0].record.usage,
      usageMissing: "1",
      attempts: "3",
      unknownTime: "1",
      evidenceSummary: {
        matching: {
          kind: "gateway-requests",
          withExactLinks: "1",
          withoutConfirmedLinks: "0",
          matchedResponses: "1",
          usageConflictLinks: "1",
        },
        billing: bill,
        usageMissingParts: "2",
        overflowFields: [],
      },
    });
  }
  const parsed = parseDomain<UnifiedRequestPage>(requestSchema, result);
  requestSnapshots.set(snapshotToken, structuredClone(parsed));
  return parsed;
};
domainApi.sessions = async (query) => {
  calls.push("sessions");
  queries.push(structuredClone(query));
  if (!query.snapshotToken) sessionRevision++;
  const snapshotToken = params.has("staleBody")
    ? `fixture-snapshot-${sessionRevision}`
    : "fixture-snapshot";
  if (params.get("staleBody") === "revoke" && sessionRevision > 1)
    authorized = false;
  const externalAvailable =
    authorized && query.scope.sourceIds.includes(source);
  const rows = externalAvailable
    ? items.filter(
        (i) =>
          !query.filter.query ||
          i.record.title
            .toLowerCase()
            .includes(query.filter.query.toLowerCase()),
      )
    : [];
  const result: UnifiedSessionPage = {
    items: rows.slice(
      Number(query.cursor ?? "0"),
      Number(query.cursor ?? "0") + 40,
    ),
    snapshotToken,
    nextCursor:
      Number(query.cursor ?? "0") + 40 < rows.length
        ? String(Number(query.cursor ?? "0") + 40)
        : null,
    asOf: "2026-10-31T00:00:00Z",
    coverage: externalAvailable
      ? query.scope.sourceIds.length === 1
        ? "complete"
        : "partial"
      : "unavailable",
    sourceStates: query.scope.sourceIds.map(
      (id): UnifiedSessionPage["sourceStates"][number] => ({
        sourceId: id,
        authorization: id === source && authorized ? "authorized" : "required",
        freshness: id === source && authorized ? "current" : "unread",
        coverage: id === source && authorized ? "complete" : "unavailable",
        problem: id === source && authorized ? null : "permission_denied",
      }),
    ),
    appliedFilter: query,
    summaryRef: snapshotToken,
    sourceSummaries: externalAvailable
      ? [
          {
            sourceId: source,
            sessions: String(rows.length),
            knownUsageSessions: String(rows.length),
            usage: {
              ...usage,
              inputTokens: rows
                .reduce(
                  (n, r) => n + BigInt(r.record.usage.inputTokens ?? "0"),
                  0n,
                )
                .toString(),
              outputTokens: rows
                .reduce(
                  (n, r) => n + BigInt(r.record.usage.outputTokens ?? "0"),
                  0n,
                )
                .toString(),
              totalTokens: rows
                .reduce(
                  (n, r) => n + BigInt(r.record.usage.totalTokens ?? "0"),
                  0n,
                )
                .toString(),
            },
          },
        ]
      : [],
    overlapUnknown: query.scope.sourceIds.length > 1,
  };
  return result;
};
domainApi.conversation = async (query) => {
  calls.push("body:" + query.sessionId);
  if (params.has("staleBody")) {
    calls.push(`snapshot:${query.snapshotToken}`);
    if (query.snapshotToken === "fixture-snapshot-1")
      throw new Error("快照已失效，请重新读取。");
  }
  const name = query.sessionId.slice(query.sessionId.lastIndexOf(":") + 1);
  const result: UnifiedConversation = {
    sourceId: source,
    snapshotToken: query.snapshotToken,
    conversation: {
      sessionId: query.sessionId,
      events: [
        {
          id: name,
          nativeId: name,
          turnId: name,
          kind: "message",
          role: "assistant",
          status: "recorded",
          text: name + " private body",
          input: null,
          output: null,
          model: null,
          responseId: null,
          usage: null,
        },
      ],
      nextCursor: null,
      complete: true,
    },
  };
  if (params.get("delay") === "1" && name === "Alpha")
    await new Promise<void>((r) => {
      release = r;
    });
  return result;
};
let factRevision = 1;
domainApi.factConversation = async (query) => {
  if (params.has("staleFactBody")) {
    calls.push(`fact-snapshot:${query.snapshotToken}`);
    if (query.snapshotToken === "facts-1") {
      factRevision++;
      throw new Error("快照已失效，请重新读取。");
    }
  }
  return domainApi.conversation(query);
};
domainApi.consent = async (input) => {
  calls.push("attempt:" + input.reviewRef);
  if (input.reviewRef !== `review-${revision}`)
    throw new Error("展示范围已变化，请重新核查");
  calls.push("consent:" + input.enabled + ":" + input.persistMetadata);
  authorized = input.enabled;
  persisted = input.persistMetadata;
  revision++;
  return { updated: true };
};
domainApi.prepareConsent = async (query) => ({
  ...query,
  reviewRef: `review-${revision}`,
  resourceLabel,
  sourceAuthorized: true,
  unifiedAuthorized: authorized,
  persistMetadata: persisted,
});
let hidden = false;
const factIdentity = {
  providerKey: "reader",
  sourceId: source,
  implementationIdentity: "a".repeat(64),
  epoch: "1",
  grantRevision: "1",
  observedAt: "2026-10-31T00:00:00Z",
  evidence: "source-reported" as const,
};
const factRecord = (id: string, value: FactRecord["value"]): FactRecord => ({
  id,
  identity: factIdentity,
  sourceRevision: "fixture-1",
  value,
});
const graph: FactRecord[] = [
  factRecord("project-one", {
    kind: "project",
    value: {
      nativeId: "p1",
      name: "Reported workspace",
      pathHint: "/reported-only/project",
    },
  }),
  factRecord(items[0].record.id, {
    kind: "session",
    value: { ...items[0].record, projectRef: "project-one" },
  }),
  ...(params.has("staleFactBody")
    ? [
        factRecord("execution-one", {
          kind: "mcp-execution",
          value: {
            nativeId: "execution-one",
            sessionRef: items[0].record.id,
            toolName: "mcp__fixture__probe",
            outcome: "success",
            durationMs: "7",
          },
        }),
      ]
    : []),
  ...Array.from({ length: params.get("factsMany") === "1" ? 83 : 1 }, (_, n) =>
    factRecord(`response-${n}`, {
      kind: "response",
      value: {
        nativeId: `r${n}`,
        sessionRef: items[0].record.id,
        productId: plugin + ":cli",
        occurredAt: null,
        model: `Fixture-${String(n).padStart(3, "0")}`,
        outcome: "success",
        usage: {
          ...usage,
          inputTokens: "10",
          outputTokens: "2",
          totalTokens: "12",
        },
        reportedResponseId: "source-claim-only",
        durationMs: null,
      },
    }),
  ),
  ...(params.has("mcpCatalog")
    ? [
        factRecord("catalog-one", {
          kind: "mcp-catalog",
          value: {
            nativeId: "configuration-one",
            serverName: "MCP configuration",
            toolNames:
              params.get("mcpCatalog") === "known"
                ? ["lookup"]
                : params.get("mcpCatalog") === "empty"
                  ? []
                  : null,
          },
        }),
      ]
    : []),
];
domainApi.facts = async (query: FactQuery) => {
  factQueries.push(structuredClone(query));
  calls.push(`facts:${query.kind}`);
  let records = authorized ? graph : [];
  if (
    (query.projectId && (query.projectId !== "project-one" || hidden)) ||
    (query.sessionId && query.sessionId !== items[0].record.id) ||
    (query.responseId &&
      !graph.some(
        (f) => f.id === query.responseId && f.value.kind === "response",
      ))
  )
    records = [];
  const responses = graph.filter((f) => f.value.kind === "response").length;
  const offset = Number(query.cursor ?? "0");
  const rows = records.filter(
    (r) =>
      r.value.kind === query.kind && (!hidden || r.value.kind !== "project"),
  );
  return {
    items: rows.slice(offset, offset + 40),
    snapshotToken: `facts-${revision + factRevision - 1}`,
    nextCursor: offset + 40 < rows.length ? String(offset + 40) : null,
    asOf: factIdentity.observedAt,
    sourceStates: [
      {
        sourceId: source,
        authorization: authorized ? "authorized" : "revoked",
        freshness: authorized ? "current" : "unread",
        coverage: authorized ? "complete" : "unavailable",
        problem: null,
      },
    ],
    sourceSummaries: authorized
      ? [
          {
            sourceId: source,
            coverage: "complete",
            projects: hidden ? "0" : "1",
            sessions: {
              unit: "session-cumulative",
              records: "1",
              usage,
              missingFields: [],
              coverage: "complete",
            },
            responses: {
              unit: "model-response",
              records: String(responses),
              usage: {
                ...usage,
                inputTokens: String(responses * 10),
                totalTokens: String(responses * 12),
              },
              missingFields: [],
              coverage: "complete",
            },
            activities: null,
            writeReports: null,
            successfulWriteReports: null,
            mcpCatalogs: params.has("mcpCatalog") ? "1" : null,
            mcpExecutions: null,
            crossSourceOverlapUnknown: true,
          },
        ]
      : [],
    appliedQuery: query,
    overlapUnknown: true,
    hiddenProjects:
      hidden && authorized
        ? [{ sourceId: source, id: "project-one", name: "Reported workspace" }]
        : [],
  };
};
domainApi.projectVisibility = async (input) => {
  calls.push(`hide:${input.hidden}`);
  hidden = input.hidden;
  revision++;
  return { updated: true };
};
domainApi.activity = async (query) => {
  calls.push("activity");
  activityQueries.push(structuredClone(query));
  if (params.get("activityDelay") === "1" && query.includeHistorical)
    await new Promise<void>((resolve) => {
      release = resolve;
    });
  const now = new Date().toISOString();
  const result: UnifiedActivityPage = {
    items: [],
    asOf: now,
    sourceStates: [],
    sourceSummaries: [],
    appliedQuery: query,
    limited: params.get("activityMany") === "1",
    overlapUnknown: true,
  };
  for (const sourceId of query.scope.sourceIds) {
    const external = sourceId === source;
    const failure = params.get("activityFailure");
    const readable =
      failure !== "unavailable" &&
      (external ? authorized && query.includeHistorical : true);
    const kind = external ? "source-event" : "native-message";
    result.sourceStates.push({
      kind,
      state: {
        sourceId,
        authorization: external && !authorized ? "revoked" : "authorized",
        freshness: failure ? "failed" : readable ? "current" : "unread",
        coverage: readable ? "partial" : "unavailable",
        problem: failure
          ? "partial_source"
          : readable
            ? external
              ? "unknown_event_time"
              : "bounded_observation"
            : "history_not_selected",
      },
    });
    const count = readable
      ? params.get("activityMany") === "1"
        ? "320"
        : "1"
      : null;
    result.sourceSummaries.push({
      sourceId,
      kind,
      observedRecords: count,
      current: readable ? (external ? "0" : "1") : null,
      expired: readable ? "0" : null,
      historical: readable ? (external ? count : "0") : null,
      unknownTime: readable ? (external ? count : "0") : null,
      logicalRequests: null,
    });
    if (!readable) continue;
    for (let n = 0; n < (result.limited ? 256 : 1); n++)
      result.items.push({
        id: `activity-${n}`,
        sourceId,
        kind,
        providerKey: external ? "reader" : "builtin:activity",
        sourceRevision: String(revision),
        observedAt: new Date(activityAt).toISOString(),
        occurredAt: external ? null : new Date(activityAt).toISOString(),
        sessionId: external ? items[0].record.id : "native-session",
        messageId: external ? null : "native-message",
        productIds: external ? [plugin + ":cli"] : [sourceId],
        unknownClient: external,
        title: `${external ? "历史事件" : "原生活动"}-${String(n).padStart(3, "0")}`,
        model: external ? null : "fixture",
        phase: "streaming",
        state: external ? "historical" : "current",
        toolNames: Array.from({ length: 32 }, (_, i) => `fixture_tool_${i}`),
        logicalRequestId: null,
        historicalDurationMs: external ? "9007199254740993" : null,
        measurements: external
          ? null
          : {
              reportedTtftMs: 55,
              runtimeState: "ready",
              firstTokenMs: 80,
              firstTextMs: 95,
              firstOutputMs: 80,
              usage: null,
              liveRate: {
                atMs: String(activityAt),
                windowMs: "5000",
                tokensPerSecond: 42,
              },
              recentRate: {
                atMs: String(activityAt),
                windowMs: "5000",
                tokensPerSecond: 42,
              },
              historicalAverageTextRate: 17,
            },
      });
    if (query.includeGateway && !external) {
      result.sourceStates.push({
        kind: "gateway-attempt",
        state: {
          sourceId,
          authorization: "authorized",
          freshness: "current",
          coverage: "partial",
          problem: "bounded_observation",
        },
      });
      result.sourceSummaries.push({
        sourceId,
        kind: "gateway-attempt",
        observedRecords: "2",
        current: "0",
        expired: "0",
        historical: "2",
        unknownTime: "0",
        logicalRequests: "1",
      });
    }
  }
  return parseDomain(activitySchema, result);
};
declare global {
  interface Window {
    domainFixture: {
      calls: string[];
      queries: UnifiedSessionQuery[];
      activityQueries: UnifiedActivityQuery[];
      requestQueries: UnifiedRequestQuery[];
      factQueries: FactQuery[];
      release: () => void;
      revoke: () => Promise<void>;
      changeResource: () => void;
      contractCheck: () => boolean[];
    };
  }
}
window.domainFixture = {
  calls,
  queries,
  activityQueries,
  requestQueries,
  factQueries,
  changeResource: () => {
    resourceLabel = "/temporary/changed-reader.jsonl";
    revision++;
  },
  release: () => release?.(),
  revoke: async () => {
    authorized = false;
    revision++;
    await domainDirectoryResource.invalidate();
  },
  contractCheck: () => {
    const base = {
      items: [],
      snapshotToken: "x",
      nextCursor: null,
      asOf: "2026-10-31T00:00:00Z",
      coverage: "complete",
      sourceStates: [],
      appliedFilter: {
        scope: { kind: "external", sourceIds: [source] },
        filter: { query: null, productId: null, days: null },
        snapshotToken: null,
        cursor: null,
      },
      summaryRef: "x",
      sourceSummaries: [
        { sourceId: source, sessions: "0", knownUsageSessions: "0", usage },
      ],
      overlapUnknown: false,
    };
    return [
      "9007199254740993",
      "18446744073709551615",
      "18446744073709551616",
      "01",
      9007199254740992,
    ].map((value) => {
      const copy = structuredClone(base);
      copy.sourceSummaries[0].usage.totalTokens = value as string;
      try {
        parseDomain(pageSchema, copy);
        return true;
      } catch {
        return false;
      }
    });
  },
};
createRoot(document.getElementById("root")!).render(
  <IconProvider>
    <main className="flex h-dvh min-h-0 flex-col">
      <div className="shrink-0 p-2">
        <DomainConsentControls
          pluginId={plugin}
          digest={"a".repeat(64)}
          fullFacts={params.get("fullFacts") === "1"}
          sourceId={source}
          onChanged={() => {}}
        />
      </div>
      <div className="min-h-0 flex-1">
        {params.get("unified") === "1" ? (
          <UnifiedWorkspace initialSource={source} />
        ) : (
          <p>原生会话保持</p>
        )}
      </div>
    </main>
  </IconProvider>,
);
