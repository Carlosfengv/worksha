import type { ModelObservation } from "@/features/observation/types";
import type {
  RequestRecord,
  RequestDashboard,
  RequestQuery,
} from "@/features/observation/request-types";
export function requestRecord(row: ModelObservation): RequestRecord {
  const cost = { actual: [], estimated: [], unknownCalls: 1 };
  return {
    ...row,
    agentProductId: row.agentProductId ?? null,
    sessionTitle: null,
    projectId: null,
    projectName: null,
    providerKey: row.providerId ?? "fixture",
    usageMissing: 0,
    cost,
    throughput:
      row.durationMs && row.usage.outputTokens
        ? (row.usage.outputTokens * 1000) / row.durationMs
        : null,
    timingNote: null,
    attempts: [
      {
        ...row,
        cost: {
          kind: "unknown",
          amount: null,
          currency: null,
          reason: "missing_price",
          source: null,
          price: null,
          components: [],
        },
        issue: null,
        providerKey: "fixture",
        backoffMs: null,
      },
    ],
  };
}
export function requestDashboard(
  query: RequestQuery,
  rows: RequestRecord[],
): RequestDashboard {
  const filtered = rows.filter(
    (r) =>
      (!query.result || r.status === query.result) &&
      (!query.search || r.id.includes(query.search)) &&
      (!query.agent || r.agentProductId === query.agent) &&
      (!query.start ||
        (r.startedAt && Date.parse(r.startedAt) >= Date.parse(query.start))) &&
      (!query.end ||
        (r.startedAt && Date.parse(r.startedAt) < Date.parse(query.end))),
  );
  return {
    query,
    asOf: "2026-10-19T12:00:00Z",
    snapshot: "fixture-snapshot",
    summary: {
      requests: filtered.length,
      attempts: filtered.length,
      statuses: Object.fromEntries(
        ["success", "failed"].map((status) => [
          status,
          filtered.filter((r) => r.status === status).length,
        ]),
      ),
      issues: {},
      successRate: filtered.length ? 1 : null,
      usage: {
        inputTokens: 100,
        outputTokens: 20,
        totalTokens: 120,
        cachedInputTokens: 80,
        cacheWriteInputTokens: 0,
        reasoningOutputTokens: null,
      },
      usageMissing: 0,
      provisional: 0,
      cost: { actual: [], estimated: [], unknownCalls: filtered.length },
      cacheRatio: 0.8,
      cacheSamples: filtered.length,
      ttft: { samples: filtered.length, p50: 100, p95: 200 },
      duration: { samples: filtered.length, p50: 1000, p95: 2000 },
      throughput: { samples: filtered.length, p50: 20, p95: 30 },
    },
    buckets: [
      {
        groups: [],
        start: "2026-10-19T00:00:00Z",
        end: "2026-10-19T01:00:00Z",
        value: filtered.length,
        p95: null,
        samples: filtered.length,
        missing: 0,
      },
    ],
    groups: [
      {
        id: "codex-cli",
        label: "codex-cli",
        value: filtered.length,
        samples: filtered.length,
        missing: 0,
      },
    ],
    items: filtered.slice(query.cursor ? 40 : 0, query.cursor ? 80 : 40),
    total: filtered.length,
    nextCursor:
      !query.cursor && filtered.length > 40 ? "fixture-snapshot:40" : null,
    models: ["model"],
    providers: [{ id: "fixture", label: "测试 Provider" }],
    sourceError: null,
    coverage: "仅覆盖已记录的网关请求",
    recordedStart: rows[0]?.startedAt ?? null,
    recordedEnd: rows.at(-1)?.startedAt ?? null,
  };
}
