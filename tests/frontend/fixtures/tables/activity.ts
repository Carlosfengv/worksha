import { observationApi } from "@/features/observation/api";
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
export function installTableActivity({ sourceState }: TableFixtureContext) {
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
