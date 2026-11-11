import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { IconProvider } from "@/lib/icon-context";
import { RequestsPage } from "@/features/observation/requests-page";
import { observationApi } from "@/features/observation/api";
import { workbenchApi } from "@/features/workbench/api";
import type { LiveAction } from "@/features/workbench/types";
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
const fixtureNativeLink = {
  eventId: "native-0",
  sessionId: "session-native",
  responseId: "response-0",
  attemptId: rows[1].id,
  usageConflict: true,
};
rows[1].nativeLinks = [fixtureNativeLink];
if (new URLSearchParams(location.search).has("trace")) {
  const row = rows[1];
  const first = {
    ...row.attempts[0],
    id: "trace-first",
    status: "failed",
    httpStatus: 429,
    issue: "rate_limit",
    clockId: "fixture-clock",
    startTickMs: 4000,
    durationMs: 200,
  };
  row.status = "recovered";
  row.durationMs = null;
  row.attempts = [
    first,
    {
      ...first,
      id: "trace-unknown",
      status: "interrupted",
      httpStatus: null,
      issue: null,
      startTickMs: 4550,
      durationMs: null,
      backoffMs: 350,
    },
    {
      ...first,
      id: "trace-success",
      status: "success",
      httpStatus: 200,
      issue: null,
      startTickMs: 5200,
      durationMs: 800,
      responseId: "response-0",
    },
    {
      ...first,
      id: "trace-different-clock",
      clockId: "other-clock",
      startTickMs: 9000,
    },
  ];
  row.nativeLinks = [{ ...fixtureNativeLink, attemptId: "trace-success" }];
}
function sourceDashboard(query: RequestQuery): RequestDashboard {
  const linkedRows = new URLSearchParams(location.search).has("noExactMatch")
    ? []
    : [rows[1]];
  const availableNative = new URLSearchParams(location.search).has("noConsent")
    ? []
    : nativeRows;
  const all =
    query.source === "native"
      ? availableNative
      : query.source === "linked"
        ? linkedRows
        : rows;
  const result = requestDashboard(query, all);
  const native = requestDashboard(query, availableNative);
  result.sourceReport = {
    gateway: requestDashboard(query, rows).summary,
    native: native.summary,
    linked: requestDashboard(query, linkedRows).summary,
    matchedNative: linkedRows.length,
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
    requestWorkbenchSelections: LiveAction[];
  }
}
window.requestQueries = [];
window.requestExports = [];
window.requestFail = false;
window.requestWorkbenchSelections = [];
const workbenchMode = new URLSearchParams(location.search).get("workbench");
if (workbenchMode) {
  const claude = workbenchMode.startsWith("claude");
  const session = "00000000-0000-4000-8000-000000000002";
  const native = "ses_00000000000040008000000000000001";
  rows[1].agentId = claude ? "claude" : "opencode";
  rows[1].agentProductId = null;
  rows[1].sessionId = claude ? `claude:${session}` : `opencode:${native}`;
  workbenchApi.findSession = async (id) => {
    if (workbenchMode === "claude-slow")
      await new Promise((resolve) => setTimeout(resolve, 250));
    if (workbenchMode === "claude-offline")
      throw new Error("Local service offline");
    return {
      taskId:
        id === `claude:${session}` &&
        !["claude-missing", "claude-desktop"].includes(workbenchMode)
          ? "00000000-0000-4000-8000-000000000003"
          : null,
    };
  };
  workbenchApi.readLiveBoard = async () => {
    if (workbenchMode === "slow")
      await new Promise((resolve) => setTimeout(resolve, 250));
    if (workbenchMode === "offline") throw new Error("Desktop offline");
    return {
      connectionId: "current-connection",
      sessions:
        workbenchMode === "missing"
          ? []
          : [
              {
                id: native,
                title: "原任务",
                directory: "/fixture",
                updated: 1,
                phase: "ended",
              },
            ],
    };
  };
  workbenchApi.controlLive = async (action) => {
    window.requestWorkbenchSelections.push(action);
  };
}
observationApi.requestDashboard = async (query) => {
  window.requestQueries.push(query);
  const search = query.search;
  await new Promise((resolve) =>
    setTimeout(resolve, search === "slow" ? 350 : 10),
  );
  if (window.requestFail) throw new Error("请求快照已过期，请刷新后重试");
  return sourceDashboard(query);
};
observationApi.requestDetail = async (id) => {
  const row = [...rows, ...nativeRows].find((r) => r.id === id) ?? null;
  const routeDecisions = new URLSearchParams(location.search).get(
    "route-decisions",
  );
  const unknownResult = routeDecisions === "unknown";
  return {
    request:
      row && routeDecisions
        ? unknownResult
          ? { ...row, status: "interrupted" }
          : { ...row, attempts: [], status: "failed" }
        : row,
    ...(routeDecisions
      ? {
          routeTrace: {
            decisions: [
              {
                path: [
                  {
                    service: "主服务",
                    model: "子服务入口",
                    versionId: "root-fixture",
                  },
                  {
                    service: "子服务",
                    model: "模型 A",
                    versionId: "child-fixture",
                    modelId: "child-model-fixture",
                  },
                ],
                accountPosition: 2,
                reason: unknownResult
                  ? "upstream-outcome-unconfirmed"
                  : "access-model-denied",
              },
              {
                path: [
                  {
                    service: "主服务",
                    model: "备用模型",
                    versionId: "root-fixture",
                  },
                ],
                reason: unknownResult
                  ? "outcome-unknown"
                  : "cross-model-fallback-disabled",
              },
            ],
            truncated: false,
            httpStatus: unknownResult ? 504 : 403,
          },
        }
      : {}),
    asOf: "2026-10-19T12:00:00Z",
    sourceError: null,
  };
};
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
      <main className="flex h-dvh min-w-0 flex-col">
        <RequestsPage selectedId={id} routeSearch={search ?? ""} />
      </main>
    </IconProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Harness />);
