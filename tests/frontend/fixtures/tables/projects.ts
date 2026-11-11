import type { ProjectDetail, ProjectTotals } from "@/features/projects/types";
import { mcpApi } from "@/features/mcp/api";
import { projectsApi } from "@/features/projects/api";
import { sessionsApi } from "@/features/sessions/api";
import { agentProducts } from "@/shared/types/agent";
import { DesktopError } from "@/shared/lib/desktop-command";
import { visualizationMcp } from "../../visualization-fixture";
import { mcpGroup, mcpCalls, requests } from "./data";
import type { TableFixtureContext } from "./context";
export function installTableProjects(context: TableFixtureContext) {
  const totals: ProjectTotals = {
    sessionCount: 1,
    usage: requests[0].usage,
    usageScope: "model_responses",
    modelCalls: 1,
    usageMissingCalls: 0,
    usageUnknownSessions: 0,
    dateUnknownSessions: 0,
    diskBytes: 10,
    diskAllocatedBytes: 4096,
    diskUnknownSessions: 0,
    writeCount: 0,
    distinctFiles: 0,
  };
  const project: ProjectDetail["project"] = {
    id: "fixture-project",
    projectKind: "workspace",
    name: "Fixture Project",
    path: "/tmp/fixture",
    gitRoot: null,
    locationState: "available",
    totals,
    agents: [
      { agentId: "codex-cli", totals },
      { agentId: "codex-desktop", totals },
      { agentId: "codex-unknown", totals },
    ],
    lastActivity: null,
    listenerCount: null,
    knownListenerCount: 0,
  };
  const listeners: ProjectDetail["listeners"] = {
    asOf: null,
    state: "unknown",
    items: [],
    unresolvedProcesses: 0,
    error: null,
  };
  const git: ProjectDetail["git"] = {
    asOf: "2026-10-19T00:00:00Z",
    state: "not_git",
    branch: null,
    head: null,
    remote: null,
    gitDir: null,
    commonDir: null,
    repositoryId: null,
    worktreeId: null,
    files: [],
    commits: [],
    error: null,
  };
  mcpApi.mcp = async (query) => {
    window.scopeQueries.push({ feature: "mcp", query: { ...query } });
    if (query.project === "hidden-project")
      throw new DesktopError({
        code: "project_hidden",
        message: "所选项目已隐藏，请清除项目筛选或在项目页恢复。",
      });
    const group = {
      ...mcpGroup,
      id: "a".repeat(64),
      configuredAgentIds: new URLSearchParams(location.search).has("longAgents")
        ? agentProducts.map((a) => a.id)
        : ["codex-cli", "codex-desktop"],
      agentIds: new URLSearchParams(location.search).has("longAgents")
        ? ["claude-unknown", ...agentProducts.map((a) => a.id), "codex-unknown"]
        : [query.agent ?? "codex-unknown"],
    };
    const visualGroups = new URLSearchParams(location.search).has("visuals")
      ? visualizationMcp(group)
      : [group];
    const paging = Boolean(query.service && context.params.has("mcpPaging"));
    if (paging && !query.cursor && window.holdMcpRefresh)
      await new Promise<void>((resolve) => {
        window.releaseMcpRefresh = resolve;
      });
    const nextCall = {
      ...mcpCalls[0],
      call: { ...mcpCalls[0].call, id: "next-call", toolName: "next-read" },
    };
    return {
      services: [],
      groups: visualGroups,
      usageSummary: group,
      facets: { models: [], projects: [] },
      calls: paging
        ? query.cursor
          ? [...mcpCalls, nextCall, nextCall]
          : mcpCalls
        : [],
      totalCalls: paging ? 2 : 0,
      nextCursor: paging && !query.cursor ? "next-page" : null,
      selectedServiceId: null,
      selectedGroupId: query.service
        ? (visualGroups.find((item) => item.id === query.service)?.id ??
          group.id)
        : null,
      sources: [],
      asOf: "2026-10-19T00:00:00Z",
      historyAsOf: "2026-10-19T00:00:00Z",
      coverage: "partial",
      warnings: [],
      appliedFilter: query,
    };
  };
  projectsApi.projects = async (query) => {
    window.scopeQueries.push({ feature: "projects", query: { ...query } });
    const temporary = {
      ...project,
      id: "fixture-temporary",
      name: "Temporary fixture",
      path: "/private/tmp/fixture",
      projectKind: "temporary" as const,
    };
    const items = (
      query.scope === "temporary"
        ? [temporary]
        : query.scope === "all"
          ? [project, temporary]
          : [project]
    ).filter((p) => !context.hiddenProjects.some((h) => h.id === p.id));
    return {
      items,
      total: items.length,
      nextCursor: null,
      summary: totals,
      unassignedSessions: 0,
      dateUnknownExcluded: 0,
      missingCounts: 0,
      asOf: git.asOf,
      coverage: "native_history",
      appliedFilter: query,
      listeners,
    };
  };
  projectsApi.setHidden = async (id, hidden) => {
    window.projectVisibilityChanges.push({ id, hidden });
    if (window.failProjectVisibility) throw new Error("显示设置保存失败");
    context.hiddenProjects = hidden
      ? [
          ...context.hiddenProjects.filter((p) => p.id !== id),
          {
            id,
            name: project.name,
            path: project.path,
            hiddenAt: "2026-10-19T00:00:00Z",
          },
        ]
      : context.hiddenProjects.filter((p) => p.id !== id);
    return { hiddenProjects: context.hiddenProjects };
  };
  projectsApi.project = async (id, query) => {
    window.scopeQueries.push({ feature: "project", query: { ...query } });
    if (context.hiddenProjects.some((p) => p.id === id))
      throw new DesktopError({
        code: "project_hidden",
        message: "此项目已隐藏",
      });
    return {
      project:
        id === "fixture-temporary"
          ? {
              ...project,
              id,
              name: "Temporary fixture",
              path: "/private/tmp/fixture",
              projectKind: "temporary",
            }
          : project,
      sessions: [],
      writes: [],
      git,
      listeners,
      asOf: git.asOf,
      coverage: "native_history",
      appliedFilter: query,
      activities: { native: [], gateway: [] },
    };
  };
  sessionsApi.calls = async (query) => {
    window.scopeQueries.push({ feature: "calls", query });
    const item = {
      ...mcpCalls[0].call,
      agentId: "codex" as const,
      agentProductId:
        query.agent === "codex-cli" ? ("codex-cli" as const) : null,
    };
    return {
      items: [item],
      total: 1,
      nextCursor: null,
      asOf: git.asOf,
      coverage: "native_history",
      missingCounts: 0,
      summary: null,
      appliedFilter: query,
    };
  };
}
