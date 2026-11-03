import { providersApi } from "@/features/providers/api";
import type {
  Provider,
  ProviderUsage,
  ProviderUsageTotals,
} from "@/features/providers/types";
import { fixtureProvider } from "./data";
export function installTableProviders() {
  const usageTotals = (calls: number): ProviderUsageTotals => ({
    cost: {
      actual: calls >= 300 ? [{ currency: "USD", amount: 0, calls: 1 }] : [],
      estimated: calls
        ? [
            {
              currency: "USD",
              amount: (calls >= 300 ? calls - 2 : calls) * 0.01,
              calls: calls >= 300 ? calls - 2 : calls,
            },
          ]
        : [],
      unknownCalls: calls >= 300 ? 1 : 0,
    },
    calls,
    errors: 0,
    inProgress: 0,
    unknownOutcomes: 0,
    usage: {
      inputTokens: calls * 8,
      outputTokens: calls * 2,
      totalTokens: calls * 10,
      cachedInputTokens: 0,
      cacheWriteInputTokens: 0,
      reasoningOutputTokens: 0,
    },
    reported: {
      inputTokens: calls,
      outputTokens: calls,
      totalTokens: calls,
      cachedInputTokens: calls,
      cacheWriteInputTokens: calls,
      reasoningOutputTokens: calls,
    },
    lastSeen: calls ? "2026-10-19T00:00:00Z" : null,
    latencyP95Ms: calls ? 1000 : null,
    latencySamples: calls,
    ttftP95Ms: null,
    ttftSamples: 0,
  });
  providersApi.providerUsage = async (query): Promise<ProviderUsage> => {
    window.scopeQueries.push({
      feature: "provider-usage",
      query: { ...query },
    });
    if (window.failProviderUsage) throw Error("统计接口暂不可用");
    const active =
      new URLSearchParams(location.search).get("mode") === "provider-history";
    const disabled =
      query.source === "native" &&
      new URLSearchParams(location.search).has("sourceDisabled");
    const all =
      active && !disabled ? (query.source === "native" ? 20 : 625) : 0;
    const unknown = active ? 2 : 0;
    const count =
      query.providerId === "_unknown"
        ? unknown
        : query.providerId === "fixture-1"
          ? active
            ? 3
            : 0
          : all;
    const models = Array.from({ length: active ? 325 : 0 }, (_, i) => ({
      model: `model-${String(i).padStart(3, "0")}`,
      totals: usageTotals(i === 0 ? 301 : 1),
    }));
    const filtered =
      query.model === undefined
        ? models
        : models.filter((m) => m.model === query.model);
    const selectedCount =
      query.model === undefined
        ? count
        : filtered.reduce((n, m) => n + m.totals.calls, 0);
    const offset = query.cursor ? Number(query.cursor) : 0;
    return {
      query,
      asOf: new Date().toISOString(),
      sourceAsOf: new Date().toISOString(),
      sourceState: disabled ? "disabled" : "ready",
      summary: usageTotals(all + (active ? 3 : 0) + unknown),
      providers: [
        { providerId: "fixture-0", totals: usageTotals(all) },
        { providerId: "fixture-1", totals: usageTotals(active ? 3 : 0) },
      ],
      unattributed: usageTotals(unknown),
      selected: query.providerId ? usageTotals(selectedCount) : null,
      modelOptions: models.map((m) => m.model),
      models: query.providerId ? filtered : [],
      calls: query.providerId
        ? Array.from(
            { length: Math.min(40, Math.max(0, selectedCount - offset)) },
            (_, n) => ({
              id: `call-${offset + n}`,
              providerId:
                query.providerId === "_unknown" ? null : query.providerId!,
              provider: "原始连接名称",
              attribution:
                query.providerId === "_unknown" ? "unknown" : "route_snapshot",
              agentId: "opencode",
              agentProductId: "opencode",
              model: query.model ?? "model-000",
              time: "2026-10-19T00:00:00Z",
              status: "success",
              usage: usageTotals(1).usage,
              cost:
                offset + n === 1
                  ? {
                      kind: "unknown",
                      amount: null,
                      currency: null,
                      reason: "missing_cache_usage",
                      source: null,
                      price: null,
                      components: [],
                    }
                  : offset + n === 0
                    ? {
                        kind: "actual",
                        amount: 0,
                        currency: "USD",
                        reason: null,
                        source:
                          "https://openrouter.ai/docs/cookbook/administration/usage-accounting",
                        price: null,
                        components: [],
                      }
                    : {
                        kind: "estimated",
                        amount: 0.01,
                        currency: "USD",
                        reason: null,
                        source:
                          "https://api-docs.deepseek.com/quick_start/pricing/",
                        price: {
                          version: "fixture-price",
                          checkedAt: "2026-10-19",
                          validFrom: "2026-10-17T00:00:00Z",
                          validUntil: "2026-11-18T00:00:00Z",
                          model: query.model ?? "model-000",
                          period: "off_peak",
                          inputPerMillion: 0.15,
                          cachedInputPerMillion: 0.003,
                          outputPerMillion: 0.6,
                        },
                        components: [
                          {
                            kind: "input",
                            tokens: 8,
                            perMillion: 0.15,
                            amount: 0.0000012,
                          },
                          {
                            kind: "cached_input",
                            tokens: 0,
                            perMillion: 0.003,
                            amount: 0,
                          },
                          {
                            kind: "output",
                            tokens: 2,
                            perMillion: 0.6,
                            amount: 0.0000012,
                          },
                        ],
                      },
              durationMs: 1000,
              ttftMs: 50,
              sessionId: null,
              requestId: `call-${offset + n}`,
            }),
          )
        : [],
      totalCalls: query.providerId ? selectedCount : 0,
      nextCursor: offset + 40 < selectedCount ? String(offset + 40) : null,
      missingTime: 0,
    };
  };

  let provider = fixtureProvider(0);
  const listeners = new Set<() => void>();
  providersApi.saveProvider = async (saved: Provider) => {
    window.savedProviders.push(saved);
    provider = { ...saved, revision: saved.revision + 1 };
    for (const listener of listeners) listener();
  };
  return {
    getProvider: () => provider,
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}
