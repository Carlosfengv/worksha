import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { IconProvider } from "@/lib/icon-context";
import { RequestsPage } from "@/features/observation/requests-page";
import { observationApi } from "@/features/observation/api";
import type { ModelObservation } from "@/features/observation/types";
import type {
  RequestQuery,
  RequestRecord,
  RequestDashboard,
} from "@/features/observation/request-types";
import { requestRecord, requestDashboard } from "./request-fixture";
import { visualizationRequests } from "./visualization-fixture";
import "@/styles/globals.css";
const usage = {
  inputTokens: 100,
  outputTokens: 20,
  totalTokens: 120,
  cachedInputTokens: 80,
  cacheWriteInputTokens: 0,
  reasoningOutputTokens: null,
};
const rows = Array.from({ length: 85 }, (_, i) =>
  requestRecord({
    id: `request-${String(i).padStart(3, "0")}`,
    agentId: "codex",
    agentProductId: "codex-cli",
    logicalId: null,
    sessionId: null,
    model: "model",
    provider: "测试 Provider",
    providerId: "fixture",
    endpoint: "https://example.test/v1",
    bindingRevision: 1,
    routing:
      i === 40
        ? {
            bindingId: "fixture-dynamic-binding",
            routeVersionId: "fixture-immutable-route-version",
            providerRevision: 7,
            boundary: "new_user_turn",
          }
        : null,
    startedAt: "2026-10-19T01:00:00Z",
    endedAt: "2026-10-19T01:00:01Z",
    status: i === 0 ? "failed" : "success",
    httpStatus: i === 0 ? 429 : 200,
    ttftMs: i === 0 ? null : 100,
    firstTextMs: 100,
    durationMs: 1000,
    usage,
    protocol: "responses",
    timingIssue: null,
  } satisfies ModelObservation),
);
const nativeRows: RequestRecord[] = Array.from({ length: 45 }, (_, i) => ({
  ...rows[i],
  id: `native-${i}`,
  source: "native",
  status: "unknown",
  attempts: [],
  startedAt:
    i === 44 ? "" : i < 20 ? "2026-10-18T01:00:00Z" : "2026-10-19T01:00:00Z",
  endedAt: null,
  ttftMs: null,
  durationMs: null,
  throughput: null,
  native: {
    eventId: `native-${i}`,
    turnId: "turn",
    responseId: `response-${i}`,
    reportedStatus: "recorded",
    association: i === 0 ? "response_id" : i === 1 ? "ambiguous" : "unlinked",
    gatewayRequestId: i === 0 ? rows[1].id : null,
    matchedAttemptId: i === 0 ? rows[1].id : null,
    usageConflict: i === 0,
  },
  timingNote: "原生记录时间不等于请求开始时间，计时不可测。",
}));
rows[1].nativeLinks = [
  {
    eventId: "native-0",
    sessionId: "session-native",
    responseId: "response-0",
    attemptId: rows[1].id,
    usageConflict: true,
  },
];
function sourceDashboard(query: RequestQuery): RequestDashboard {
  const availableNative = new URLSearchParams(location.search).has("noConsent")
    ? []
    : nativeRows;
  const all =
    query.source === "native"
      ? availableNative
      : query.source === "linked"
        ? [rows[1]]
        : rows;
  const result = requestDashboard(query, all);
  const native = requestDashboard(query, availableNative);
  result.sourceReport = {
    gateway: requestDashboard(query, rows).summary,
    native: native.summary,
    linked: requestDashboard(query, [rows[1]]).summary,
    matchedNative: 1,
    unlinkedNative: 44,
    ambiguousNative: 1,
    usageConflicts: 1,
    nativeMissingTime: 1,
    sourceAsOf: result.asOf,
    collectors: [
      {
        agentId: "codex",
        enabled: !new URLSearchParams(location.search).has("noConsent"),
        status: "ready",
        asOf: result.asOf,
        error: null,
        skippedFiles: 0,
      },
    ],
    scope: "未关联记录可能重叠，不相加为全部请求。",
  };
  result.missingTime = all.filter((r) => !r.startedAt).length;
  result.heatmap = {
    days: Array.from({ length: 371 }, (_, i) => {
      const d = new Date(Date.UTC(2025, 9, 20 + i));
      const day = d.toISOString().slice(0, 10);
      const records = all.filter(
        (r) =>
          r.startedAt.startsWith(day) &&
          (!query.start || r.startedAt >= query.start) &&
          (!query.end || r.startedAt < query.end),
      );
      return {
        date: day,
        start: day + "T00:00:00+08:00",
        end:
          new Date(d.getTime() + 86400000).toISOString().slice(0, 10) +
          "T00:00:00+08:00",
        state: records.length ? "recorded" : "unrecorded",
        records: records.length,
        tokens: records.length ? records.length * 120 : null,
        cost: { actual: [], estimated: [], unknownCalls: records.length },
        usageMissing: 0,
      };
    }),
    missingTime: result.missingTime,
    outsideWindow: 0,
  };
  if (query.source === "native") {
    result.summary.attempts = 0;
    result.summary.statuses = { unknown: result.total };
    result.summary.successRate = null;
    result.summary.ttft = { samples: 0, p50: null, p95: null };
    result.summary.duration = result.summary.ttft;
    result.summary.throughput = result.summary.ttft;
    result.coverage = "已授权原生模型响应记录，非 HTTP 请求数";
  }
  return new URLSearchParams(location.search).has("visuals")
    ? visualizationRequests(result)
    : result;
}
declare global {
  interface Window {
    requestQueries: RequestQuery[];
    requestExports: string[];
    requestFail: boolean;
    resolveRequestExport?: () => void;
  }
}
window.requestQueries = [];
window.requestExports = [];
window.requestFail = false;
observationApi.requestDashboard = async (query) => {
  window.requestQueries.push(query);
  const search = query.search;
  await new Promise((resolve) =>
    setTimeout(resolve, search === "slow" ? 350 : 10),
  );
  if (window.requestFail) throw new Error("请求快照已过期，请刷新后重试");
  return sourceDashboard(query);
};
observationApi.requestDetail = async (id) => ({
  request: [...rows, ...nativeRows].find((r) => r.id === id) ?? null,
  asOf: "2026-10-19T12:00:00Z",
  sourceError: null,
});
observationApi.exportRequests = async (snapshot) => {
  window.requestExports.push(snapshot);
  const outcome = new URLSearchParams(location.search).get("export");
  if (outcome === "wait")
    await new Promise<void>((resolve) => {
      window.resolveRequestExport = resolve;
    });
  if (outcome === "fail") throw new Error("磁盘空间不足");
  return {
    status: outcome === "cancel" ? "cancelled" : "saved",
    count: window.requestQueries.at(-1)?.source === "native" ? 45 : 85,
    fileName: "requests.csv",
  };
};
function Harness() {
  const [hash, setHash] = useState(location.hash);
  useEffect(() => {
    const changed = () => setHash(location.hash);
    addEventListener("hashchange", changed);
    return () => removeEventListener("hashchange", changed);
  }, []);
  const [path, search] = hash.replace(/^#\/?/, "").split("?");
  const id = path.split("/")[1];
  return (
    <IconProvider>
      <main className="min-w-0 p-4">
        <RequestsPage selectedId={id} routeSearch={search ?? ""} />
      </main>
    </IconProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Harness />);
