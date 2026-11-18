import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { createReadResource } from "@/shared/lib/read-resource";
import { observationApi } from "@/features/observation/api";
import { providerCosts } from "../usage-display";

const scopedCosts = createReadResource(
  async () => {
    const now = Date.now();
    const end = new Date(now).toISOString();
    const start = new Date(now - 7 * 86_400_000).toISOString();
    return Promise.all(
      (["local", "lan"] as const).map(async (clientScope) => ({
        clientScope,
        dashboard: await observationApi.requestDashboard({
          source: "gateway",
          clientScope,
          start,
          end,
          metric: "cost",
          dimension: "provider",
        }),
      })),
    );
  },
  { interval: false },
);

export function GatewayCostComparison() {
  const result = useSharedRead(scopedCosts);
  return (
    <section
      className="space-y-2 rounded-xl border border-border p-3"
      aria-label="网关近七天费用归属"
    >
      <h3 className="text-body font-medium">网关近 7 天费用</h3>
      {result.error ? (
        <p role="alert" className="text-label">
          费用归属暂不可读 · {result.error}
        </p>
      ) : result.data ? (
        <dl className="grid gap-2 sm:grid-cols-2">
          {result.data.map(({ clientScope, dashboard }) => (
            <div key={clientScope} className="space-y-1">
              <dt className="text-label text-fg-muted">
                {clientScope === "lan" ? "局域网客户端" : "本机客户端"}
              </dt>
              <dd className="text-body tabular-nums">
                {dashboard.sourceError
                  ? "范围不完整"
                  : dashboard.total === 0
                    ? "无请求"
                    : providerCosts(dashboard.summary.cost)}
              </dd>
            </div>
          ))}
        </dl>
      ) : (
        <p role="status" className="text-label text-fg-muted">
          正在读取费用归属…
        </p>
      )}
    </section>
  );
}
