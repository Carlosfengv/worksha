import { costsApi } from "@/features/costs/api";
import type { CostCall, CostGroup } from "@/features/costs/types";
import { agentsApi } from "@/features/agents/api";
import { sessionsApi } from "@/features/sessions/api";
import { projectsApi } from "@/features/projects/api";
import type { AssociatedCosts } from "@/shared/types/cost";

export function installAssociatedCosts() {
  const fee = (unknown = false): AssociatedCosts => ({
    cost: unknown
      ? { actual: [], estimated: [], unknownCalls: 0 }
      : {
          actual: [{ currency: "USD", amount: 0, calls: 1 }],
          estimated: [{ currency: "USD", amount: 0.000000004, calls: 1 }],
          unknownCalls: 1,
        },
    gatewayAttempts: unknown ? 0 : 3,
    responseLinkedAttempts: unknown ? 0 : 1,
    sessionLinkedAttempts: unknown ? 0 : 2,
    nativeResponses: 4,
    matchedNativeResponses: unknown ? 0 : 1,
    unlinkedNativeResponses: unknown ? 4 : 3,
    conflictingNativeResponses: 0,
    sourceState: "ready",
  });
  costsApi.details = async (q) => {
    if (new URLSearchParams(window.location.search).has("feeError"))
      throw new Error("费用来源不可用");
    const unknown = q.id.endsWith(":b") || q.agent?.endsWith("unknown");
    const base = {
      requestId: "logical-a",
      sessionId: q.scope === "session" ? q.id : "claude:a",
      time: "2026-10-20T01:00:00Z",
      status: "success",
      association: "session_id" as const,
      usage: {
        inputTokens: 10,
        outputTokens: 5,
        totalTokens: 15,
        cachedInputTokens: null,
        cacheWriteInputTokens: null,
        reasoningOutputTokens: null,
      },
      cost: {
        kind: "unknown" as const,
        amount: null,
        currency: null,
        reason: "missing_usage",
        source: null,
        price: null,
        components: [],
      },
    };
    const all: CostCall[] = unknown
      ? []
      : [
          {
            ...base,
            id: "fee-a",
            providerKey: "saved-a",
            provider: "Saved A",
            model: "model-one",
            cost: {
              ...base.cost,
              kind: "actual",
              amount: 0,
              currency: "USD",
              reason: null,
            },
          },
          {
            ...base,
            id: "fee-b",
            providerKey: "saved-a",
            provider: "Saved A",
            model: "model-two",
            cost: {
              ...base.cost,
              kind: "estimated",
              amount: 0.000000004,
              currency: "USD",
              reason: null,
            },
          },
          {
            ...base,
            id: "fee-c",
            providerKey: "saved-b",
            provider: "Saved B",
            model: null,
          },
        ];
    const totals = (rows: CostCall[]) => ({
      actual: rows
        .filter((a) => a.cost.kind === "actual")
        .map(() => ({ currency: "USD", amount: 0, calls: 1 })),
      estimated: rows
        .filter((a) => a.cost.kind === "estimated")
        .map(() => ({ currency: "USD", amount: 0.000000004, calls: 1 })),
      unknownCalls: rows.filter((a) => a.cost.kind === "unknown").length,
    });
    const groups = (rows: CostCall[], byModel: boolean): CostGroup[] =>
      [
        ...new Set(
          rows.map((a) =>
            JSON.stringify([a.providerKey, byModel ? a.model : null]),
          ),
        ),
      ].map((key) => {
        const [p, m] = JSON.parse(key) as [string, string | null];
        const selected = rows.filter(
          (a) => a.providerKey === p && (!byModel || a.model === m),
        );
        return {
          providerKey: p,
          provider: selected[0].provider,
          model: m,
          attempts: selected.length,
          cost: totals(selected),
        };
      });
    const filtered = all.filter(
      (a) =>
        (!q.provider || q.provider === a.providerKey) &&
        (!q.model || q.model === (a.model ?? "_unknown")),
    );
    const offset = q.cursor ? 2 : 0;
    return {
      summary: fee(unknown),
      filteredCost: totals(filtered),
      total: filtered.length,
      providers: groups(all, false),
      models: groups(all, true),
      filteredProviders: groups(filtered, false),
      filteredModels: groups(filtered, true),
      calls: filtered.slice(offset, offset + 2),
      nextCursor: filtered.length > offset + 2 ? "fixture-next" : null,
    };
  };
  const readSessions = sessionsApi.sessions;
  sessionsApi.sessions = async (q) => ({
    ...(await readSessions(q)),
    associatedCosts: fee(q?.agent?.endsWith("desktop")),
  });
  const readConversation = sessionsApi.conversation;
  sessionsApi.conversation = async (id, cursor, anchor) => ({
    ...(await readConversation(id, cursor, anchor)),
    associatedCosts: fee(id.endsWith(":b")),
  });
  const readProjects = projectsApi.projects;
  projectsApi.projects = async (q) => {
    const data = await readProjects(q);
    const costs = fee(q?.agent?.endsWith("unknown"));
    return {
      ...data,
      summary: { ...data.summary, associatedCosts: costs },
      items: data.items.map((p) => ({
        ...p,
        totals: { ...p.totals, associatedCosts: costs },
      })),
    };
  };
  const readProject = projectsApi.project;
  projectsApi.project = async (id, q) => {
    const data = await readProject(id, q);
    const costs = fee(q?.agent?.endsWith("unknown"));
    return {
      ...data,
      project: {
        ...data.project,
        totals: { ...data.project.totals, associatedCosts: costs },
        agents: data.project.agents.map((a) => ({
          ...a,
          totals: {
            ...a.totals,
            associatedCosts: fee(a.agentId.endsWith("unknown")),
          },
        })),
      },
    };
  };
  agentsApi.agentUsage = async (q) => {
    const cost =
      q.source === "native"
        ? { actual: [], estimated: [], unknownCalls: 2 }
        : fee().cost;
    const totals = {
      attempts: 2,
      missingUsage: 0,
      cost,
      usage: {
        inputTokens: 10,
        outputTokens: 5,
        totalTokens: 15,
        cachedInputTokens: null,
        cacheWriteInputTokens: null,
        reasoningOutputTokens: null,
      },
      reported: {
        inputTokens: 2,
        outputTokens: 2,
        totalTokens: 2,
        cachedInputTokens: 0,
        cacheWriteInputTokens: 0,
        reasoningOutputTokens: 0,
      },
    };
    return {
      query: q,
      asOf: new Date().toISOString(),
      sourceAsOf: new Date().toISOString(),
      sourceState: "ready",
      summary: totals,
      providers: [],
      items: [
        {
          providerId: "a".repeat(32),
          provider: null,
          endpoint: null,
          routeRevision: null,
          model: "model-cost",
          totals,
          lastSeen: null,
          sessionId: null,
          requestId: null,
        },
      ],
      total: 1,
      nextCursor: null,
      missingTime: 0,
    };
  };
}
