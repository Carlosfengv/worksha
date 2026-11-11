import { observationApi } from "@/features/observation/api";
import { appApi } from "@/app/api";
import { bundledDshAgent } from "@/features/agents/bundled-source";
import { domainApi } from "@/features/domain/api";
import { sessionsApi } from "@/features/sessions/api";
import { agentsApi } from "@/features/agents/api";
import { DesktopError } from "@/shared/lib/desktop-command";
import {
  activityOverview,
  activityQuery,
  activitySessions,
  activityConversation,
} from "../../activity-fixture";
import { requestRecord, requestDashboard } from "../../request-fixture";
import { memoryFixture, contextFixture } from "../../memory-context-fixture";
import { visualizationAnalysis } from "../../visualization-fixture";
import type { TableFixtureContext } from "./context";
declare global {
  interface Window {
    dshActivity?: "running" | "idle" | "expired" | "revoked";
  }
}

export function installTableActivity({
  sourceState,
  params,
}: TableFixtureContext) {
  observationApi.requestDetail = async (id) => ({
    request:
      activityOverview(activityQuery)
        .gatewayActivities.map((a) => requestRecord(a.request))
        .find((r) => r.id === id) ?? null,
    asOf: new Date().toISOString(),
    sourceError: null,
  });
  observationApi.requestDashboard = async (query) =>
    requestDashboard(
      query,
      activityOverview(activityQuery).gatewayActivities.map((a) =>
        requestRecord(a.request),
      ),
    );
  agentsApi.memory = memoryFixture;
  observationApi.overview = async (query) => {
    window.overviewReads.push(query);
    const data = activityOverview(query);
    if (params.has("dshActivity")) {
      if (query.agent === bundledDshAgent.id) throw Error("调用分析筛选无效");
      data.nativeActivities = [];
      data.gatewayActivities = [];
      data.sessionContexts = [];
    }
    if (sourceState === "disabled") {
      data.nativeActivities = [];
      data.sessionContexts = [];
    }
    if (new URLSearchParams(location.search).has("visuals"))
      data.analysis = visualizationAnalysis(query);
    if (window.overviewHeldAgent && window.overviewHeldAgent === query.agent) {
      await new Promise<void>((resolve) => {
        window.overviewRelease = () => resolve();
      });
      window.overviewHeldDone = true;
    }
    if (window.overviewDelay)
      await new Promise((resolve) => setTimeout(resolve, window.overviewDelay));
    return data;
  };
  if (params.get("dshActivity") === "connected") {
    const messageStartedAt = new Date(Date.now() - 10000).toISOString();
    const snapshot = appApi.snapshot;
    appApi.snapshot = async () => {
      const data = await snapshot();
      return {
        ...data,
        sessionSources: [],
        agents: [
          ...data.agents,
          { ...bundledDshAgent, found: true, connected: true },
        ],
      };
    };
    domainApi.activity = async (query) => {
      const sourceId = bundledDshAgent.sourceId!;
      window.scopeQueries.push({ feature: "dsh-activity", query });
      if (
        query.scope.kind !== "external" ||
        query.scope.sourceIds.join() !== sourceId ||
        query.includeGateway ||
        !query.includeHistorical
      )
        throw Error("Unexpected source activity scope");
      const now = new Date().toISOString();
      const state = window.dshActivity ?? "running";
      return {
        asOf: now,
        appliedQuery: query,
        limited: false,
        overlapUnknown: true,
        sourceSummaries: [],
        sourceStates: [
          {
            kind: "source-event",
            state: {
              sourceId,
              authorization: state === "revoked" ? "revoked" : "authorized",
              coverage: state === "revoked" ? "unavailable" : "partial",
              freshness: state === "expired" ? "stale" : "current",
              problem: state === "expired" ? "source_stalled" : null,
            },
          },
        ],
        items:
          state === "revoked"
            ? []
            : [
                {
                  id: "dsh-activity",
                  kind: "source-event",
                  providerKey: "dsh-fixture",
                  sourceRevision: "1",
                  sourceId,
                  sessionId: "dsh-fixture-session",
                  productIds: [bundledDshAgent.id],
                  title: "DSH 直连任务",
                  phase: state === "idle" ? "idle" : "running",
                  occurredAt: now,
                  messageStartedAt,
                  historicalDurationMs: state === "idle" ? "12000" : null,
                  observedAt: now,
                  state:
                    state === "running"
                      ? "current"
                      : state === "idle"
                        ? "historical"
                        : "expired",
                  toolNames: [],
                  unknownClient: false,
                  measurements: null,
                  ...(params.has("dshMetrics")
                    ? {
                        model: "deepseek-flash",
                        reportedMeasurements: {
                          firstTokenMs: 850,
                          runningTokensPerSecond:
                            state === "running" ? 24 : null,
                          completedTokensPerSecond:
                            state === "idle" ? 30 : null,
                          outputTokens: state === "idle" ? "120" : null,
                        },
                      }
                    : {}),
                },
              ],
      };
    };
  }
  observationApi.requests = async (_agent, _offset, id) => {
    const items = activityOverview(activityQuery)
      .gatewayActivities.map((a) => a.request)
      .filter((r) => !id || r.id === id);
    return {
      items,
      total: items.length,
      nextOffset: null,
      asOf: new Date().toISOString(),
    };
  };
  sessionsApi.sessions = async () => {
    const items = activitySessions();
    return {
      items,
      total: items.length,
      nextCursor: null,
      summary: null,
      asOf: new Date().toISOString(),
      coverage: "native_history",
      missingCounts: 0,
    };
  };
  sessionsApi.refreshSessions = async () => {
    window.conversationRefreshes++;
  };
  sessionsApi.conversation = async (id) => {
    window.conversationReads.push(id);
    const params = new URLSearchParams(location.search);
    const failures = Number(params.get("conversationFailures") ?? 0);
    if (
      id === (params.get("lateConversation") ?? "claude:b") &&
      window.conversationReads.filter((value) => value === id).length <=
        failures
    )
      throw new DesktopError({
        code: params.get("conversationFailureCode") ?? "not_found",
        message: "会话尚未就绪",
      });
    return {
      ...activityConversation(id),
      context: {
        ...contextFixture(),
        ...(!window.contextMissing
          ? activityOverview(activityQuery).sessionContexts?.find(
              (s) => s.sessionId === id,
            )?.context
          : {}),
      },
    };
  };
}
