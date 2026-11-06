import { createRoot } from "react-dom/client";
import { IconProvider } from "@/lib/icon-context";
import { projectsApi } from "@/features/projects/api";
import type { ProjectSummary, ProjectPage } from "@/features/projects/types";
import { WorkbenchPage } from "@/features/workbench/page";
import { workbenchApi } from "@/features/workbench/api";
import { McpPage } from "@/features/mcp/page";
import { mcpApi } from "@/features/mcp/api";
import type { McpSnapshot } from "@/features/mcp/types";
import { mcpGroup } from "./fixtures/tables/data";
import "@/styles/globals.css";

declare global {
  interface Window {
    taskProjectCalls: unknown[];
    taskProjectIds: string[];
  }
}
window.taskProjectCalls = [];
window.taskProjectIds = [];
const params = new URLSearchParams(location.search);
let mcpReads = 0;
mcpApi.mcp = async (query): Promise<McpSnapshot> => {
  window.taskProjectCalls.push({ kind: "read-mcp" });
  const read = ++mcpReads;
  if (read > 1) await new Promise((resolve) => setTimeout(resolve, 100));
  if (params.has("fail-refresh") && read === 2)
    throw new Error("配置重新读取失败");
  return {
    services: [],
    groups: read > 1 ? [{ ...mcpGroup, name: "pencil" }] : [],
    usageSummary: mcpGroup,
    facets: { models: [], projects: [] },
    calls: [],
    totalCalls: 0,
    nextCursor: null,
    selectedServiceId: null,
    selectedGroupId: null,
    sources: [],
    asOf: new Date().toISOString(),
    historyAsOf: new Date().toISOString(),
    coverage: "partial",
    warnings: [],
    appliedFilter: query,
  };
};
const firstId = `project:${"a".repeat(64)}`;
const secondId = `project:${"b".repeat(64)}`;
function project(id: string, name: string): ProjectSummary {
  return {
    id,
    name,
    path: `/temporary/${name}`,
    gitRoot: null,
    locationState: "available",
    projectKind: "temporary",
    agents: [],
    totals: {
      sessionCount: 0,
      usage: {
        inputTokens: null,
        outputTokens: null,
        cachedInputTokens: null,
        cacheWriteInputTokens: null,
        reasoningOutputTokens: null,
        totalTokens: null,
      },
      usageScope: "model_responses",
      modelCalls: 0,
      usageMissingCalls: 0,
      usageUnknownSessions: 0,
      dateUnknownSessions: 0,
      diskBytes: 0,
      diskAllocatedBytes: null,
      diskUnknownSessions: 0,
      writeCount: 0,
      distinctFiles: 0,
    },
    lastActivity: null,
    listenerCount: null,
    knownListenerCount: 0,
  };
}
const first = project(firstId, "已有项目"),
  second = project(secondId, "第二页项目");
projectsApi.projects = async (query) => {
  window.taskProjectCalls.push({ kind: "projects", query });
  if (query.query === "slow")
    await new Promise((resolve) => setTimeout(resolve, 350));
  if (query.query === "broken") throw new Error("项目来源读取失败");
  const page: ProjectPage = {
    items: query.query
      ? query.query === "fast"
        ? [second]
        : []
      : query.cursor
        ? [second]
        : Array.from({ length: 40 }, (_, i) =>
            i === 0
              ? first
              : project(
                  `project:${String(i).padStart(64, "0")}`,
                  `已有目录${i}`,
                ),
          ),
    total: 41,
    nextCursor: query.query || query.cursor ? null : "next",
    summary: first.totals,
    unassignedSessions: 0,
    dateUnknownExcluded: 0,
    missingCounts: 0,
    asOf: new Date().toISOString(),
    coverage: "complete",
    appliedFilter: query,
    listeners: {
      asOf: null,
      state: "unknown",
      items: [],
      unresolvedProcesses: 0,
      error: null,
    },
  };
  return page;
};
projectsApi.project = async (id) =>
  ({ project: id === firstId ? first : second }) as Awaited<
    ReturnType<typeof projectsApi.project>
  >;
workbenchApi.read = async () => ({ tasks: [], task: null, activeTaskId: null });
workbenchApi.sharedRoutes = async () => ({
  installed: true,
  routes: [
    {
      id: "current-model",
      label: "当前模型",
      isDefault: true,
      requestScope: "current",
    },
  ],
  error: null,
});
workbenchApi.routes = workbenchApi.sharedRoutes;
workbenchApi.create = async (_taskId, projectId, _route, agent) => {
  window.taskProjectCalls.push({ kind: "create-cli", projectId, agent });
  return null;
};
workbenchApi.createShared = async (_taskId, projectId) => {
  window.taskProjectIds.push(_taskId);
  window.taskProjectCalls.push({ kind: "create", projectId });
  return null;
};
createRoot(document.getElementById("root")!).render(
  <IconProvider>
    {params.has("mcp") ? (
      <McpPage />
    ) : (
      <WorkbenchPage
        newTask={!params.has("selected")}
        liveMode={false}
        projectId={params.has("selected") ? secondId : undefined}
      />
    )}
  </IconProvider>,
);
