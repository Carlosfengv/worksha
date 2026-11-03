import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { costReason, providerCosts } from "@/features/providers/usage-display";
import { date } from "@/features/sessions/display";
import { money } from "@/shared/lib/money";
import { useSharedQuery } from "@/shared/hooks/use-shared-read";
import { providerUsageResource } from "@/features/providers/queries";
import {
  balanceDuration,
  estimateProviderBalance,
  isBalanceUsageScope,
} from "../balance-estimate";
import type { BillingAccount } from "../types";

export function ProviderBalanceEstimate({
  account,
  providerId,
  now,
  directoryError,
}: {
  account: BillingAccount;
  providerId: string;
  now: number;
  directoryError: boolean;
}) {
  const enabled =
    !!providerId &&
    account.providerId === providerId &&
    account.accountType === "api";
  const resource = useSharedQuery(
    providerUsageResource,
    { source: "gateway", days: 7, providerId },
    enabled,
  );
  const result = estimateProviderBalance(
    account,
    providerId,
    resource.data,
    now,
    directoryError,
    !!resource.error,
  );
  if (!enabled) return null;
  const sameScope = isBalanceUsageScope(resource.data, providerId);
  const costs = sameScope ? resource.data?.selected?.cost : null;
  const unknownReasons = sameScope
    ? Object.entries(resource.data?.selected?.unknownCostReasons ?? {}).filter(
        ([, calls]) => Number.isSafeInteger(calls) && calls > 0,
      )
    : [];
  return (
    <section
      aria-label="余额消耗估算"
      className="min-w-0 space-y-3 rounded-xl border border-border p-4"
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="space-y-1">
          <div className="flex flex-wrap items-center gap-2">
            <h4 className="text-body font-medium">余额消耗估算</h4>
            <Badge>估算</Badge>
          </div>
          <p className="text-label text-fg-muted">
            近 7 天已保留的该连接网关请求 · 全部 Agent 与模型
          </p>
        </div>
        <Button
          variant="tertiary"
          size="sm"
          loading={resource.loading}
          onClick={resource.reload}
        >
          重新读取调用费用
        </Button>
      </div>
      <dl className="grid min-w-0 gap-4 sm:grid-cols-3">
        <div className="min-w-0">
          <dt className="text-label text-fg-muted">剩余时长估算</dt>
          <dd
            className="mt-1 text-title font-medium"
            role="status"
            aria-label="剩余时长估算"
          >
            {balanceDuration(result.remainingDays)}
          </dd>
        </div>
        <div className="min-w-0">
          <dt className="text-label text-fg-muted">
            近 7 天调用费用{costs?.unknownCalls ? " · 已知部分" : ""}
          </dt>
          <dd className="mt-1 break-words text-body">{providerCosts(costs)}</dd>
        </div>
        <div className="min-w-0">
          <dt className="text-label text-fg-muted">日均消耗估算</dt>
          <dd className="mt-1 break-words text-body">
            {result.dailySpend === null || !result.balance
              ? "未知"
              : `${money(result.dailySpend, result.balance.currency)} / 天`}
          </dd>
        </div>
      </dl>
      {resource.loading && !resource.data && (
        <p role="status" className="text-label text-fg-muted">
          正在读取近 7 天调用费用…
        </p>
      )}
      {resource.error && (
        <p role="alert" className="break-words text-label text-fg-muted">
          调用费用读取失败：{resource.error}
          {resource.data ? " · 以下费用为上次统计。" : ""}
        </p>
      )}
      {result.reasons.length > 0 && (
        <ul className="list-disc space-y-1 pl-5 text-label text-fg-muted">
          {result.reasons.map((reason) => (
            <li key={reason}>{reason}</li>
          ))}
        </ul>
      )}
      {unknownReasons.length > 0 && (
        <div className="space-y-1 text-label text-fg-muted">
          <p>费用未知原因{resource.error ? "（上次统计）" : ""}</p>
          <ul className="list-disc space-y-1 pl-5">
            {unknownReasons.map(([reason, calls]) => (
              <li key={reason}>
                {calls.toLocaleString("zh-CN")} 次：{costReason(reason)}
              </li>
            ))}
          </ul>
        </div>
      )}
      <p className="text-label text-fg-subtle">
        剩余天数 = 余额 ÷（近 7 天同币种调用费用 ÷ 7）。至少需要 2
        次费用明确的调用；实际与估算费用每次调用只计一种，含估算费用时结果同样为估算。
      </p>
      <p className="text-label text-fg-subtle">
        仅按该连接已记录的消耗速度估算，不能代表完整账单。直连、其他连接共用余额或未采集的消费可能使余额更早用完；不自动换汇，不与原生响应费用相加。
      </p>
      <p className="text-label text-fg-subtle">
        余额采集：{account.snapshot ? date(account.snapshot.asOf) : "未知"} ·
        费用统计：
        {sameScope && resource.data ? date(resource.data.asOf) : "未知"}
      </p>
    </section>
  );
}
