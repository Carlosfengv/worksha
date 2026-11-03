import type { CallAnalysis, OverviewQuery } from "@/features/observation/types";
import type { RequestDashboard } from "@/features/observation/request-types";
import type { McpGroup } from "@/features/mcp/types";
import { agentProducts } from "@/shared/types/agent";

export function visualizationAnalysis(query: OverviewQuery): CallAnalysis {
  const now = Date.parse("2026-10-23T04:00:00Z");
  const weights = [48, 36, 24, 18, 12, 9, 6, 3, 0];
  const models = weights
    .map((count, index) => ({
      id: `fixture-model-${index + 1}`,
      count,
      agent: agentProducts[index % agentProducts.length].id,
    }))
    .filter((model) => !query.agent || model.agent === query.agent);
  const factor =
    query.metric === "calls" ? 1 : query.metric === "tokens" ? 120 : 7.5;
  const total = models.reduce((sum, model) => sum + model.count, 0);
  const groups = (dimension: "agent" | "model", bucket?: number) => {
    const counts = new Map<string, number>();
    models.forEach((model, modelIndex) => {
      const id = dimension === "agent" ? model.agent : model.id;
      let count = 0;
      for (let index = 0; index < model.count; index++)
        if (bucket === undefined || (modelIndex * 3 + index) % 24 === bucket)
          count++;
      counts.set(id, (counts.get(id) ?? 0) + count);
    });
    return [...counts].map(([id, count]) => ({
      id,
      value: count * factor,
      missing: 0,
      samples: count,
    }));
  };
  const step = (query.range * 3600000) / 24;
  return {
    asOf: new Date(now).toISOString(),
    windowStart: new Date(now - query.range * 3600000).toISOString(),
    query,
    total: total * factor,
    samples: total + 4,
    reported: total,
    missing: 4,
    missingTime: 1,
    provisional: 2,
    agents: [
      ...groups("agent"),
      { id: "unmeasured", value: null, missing: 4, samples: 4 },
    ],
    models: [
      ...groups("model"),
      { id: "unknown-model", value: null, missing: 4, samples: 4 },
    ],
    buckets: Array.from({ length: 24 }, (_, index) => {
      const values = groups(query.dimension, index);
      return {
        start: new Date(now - (24 - index) * step).toISOString(),
        end: new Date(now - (23 - index) * step).toISOString(),
        groups: values,
        value: values.reduce((sum, group) => sum + group.value, 0),
        missing: 0,
      };
    }),
    coverage: "partial",
    unit:
      query.metric === "cost"
        ? "USD"
        : query.metric === "tokens"
          ? "Token"
          : "次",
    ...(query.metric === "cost"
      ? {
          cost: {
            actualUsd: 0,
            estimatedUsd: (total * factor) / 1e6,
            unknownAttempts: 4,
          },
        }
      : {}),
  };
}

export function visualizationRequests(data: RequestDashboard) {
  const metric = data.query.metric ?? "calls";
  const native = data.query.source === "native";
  const performance = ["ttft", "duration", "throughput"].includes(metric);
  const distribution =
    metric === "ttft"
      ? data.summary.ttft
      : metric === "duration"
        ? data.summary.duration
        : data.summary.throughput;
  const base =
    metric === "calls"
      ? data.total
      : metric === "tokens"
        ? data.total * 120
        : null;
  if (!native) {
    const completed =
      (data.summary.statuses.success ?? 0) +
      (data.summary.statuses.recovered ?? 0) +
      (data.summary.statuses.failed ?? 0);
    data.summary.successRate = completed
      ? ((data.summary.statuses.success ?? 0) +
          (data.summary.statuses.recovered ?? 0)) /
        completed
      : null;
  }
  data.summary.usage = {
    inputTokens: data.total * 100,
    outputTokens: data.total * 20,
    totalTokens: data.total * 120,
    cachedInputTokens: data.total * 80,
    cacheWriteInputTokens: 0,
    reasoningOutputTokens: null,
  };
  data.buckets = Array.from({ length: 24 }, (_, index) => {
    const samples =
      Math.floor(data.total / 24) + (index < data.total % 24 ? 1 : 0);
    const value = performance
      ? native
        ? null
        : distribution.p50
      : base === null
        ? null
        : metric === "calls"
          ? samples
          : samples * 120;
    return {
      start: new Date(Date.UTC(2026, 9, 19, index)).toISOString(),
      end: new Date(Date.UTC(2026, 9, 19, index + 1)).toISOString(),
      value,
      p95: performance && !native ? distribution.p95 : null,
      samples: native && performance ? 0 : samples,
      missing: 0,
      groups: [],
    };
  });
  data.groups = [
    {
      id: "codex-cli",
      label: "codex-cli",
      value: performance ? (native ? null : distribution.p50) : base,
      samples: native && performance ? 0 : data.total,
      missing: 0,
    },
    { id: "missing", label: "未报告指标", value: null, samples: 1, missing: 1 },
  ];
  return data;
}

export function visualizationMcp(base: McpGroup): McpGroup[] {
  return [
    base,
    ...Array.from({ length: 8 }, (_, index) => ({
      ...base,
      id: `visual-mcp-${index}`,
      name: `MCP ${index + 1} · ${index === 6 ? "较长的服务名称用于检查窄屏显示" : "工具服务"}`,
      callCount: index === 7 ? null : 100 - index * 10,
      returnedCount: index === 7 ? 0 : 90 - index * 10,
      errorCount: index === 7 ? 0 : 5,
      unknownCount: index === 7 ? 0 : 5,
      durationSamples: index === 7 ? 0 : 40,
      p95Ms: index === 7 ? null : (index + 1) * 250,
    })),
  ];
}
