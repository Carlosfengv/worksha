import { useEffect, useMemo, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { AccountQuota } from "@/features/accounts/components/account-quota";
import { ProviderQuotaForecast } from "@/features/accounts/components/provider-quota-forecast";
import {
  balanceDuration,
  estimateProviderBalance,
} from "@/features/accounts/balance-estimate";
import {
  accountQuotaForecasts,
  quotaPercent,
  type QuotaForecast,
} from "@/features/accounts/forecast";
import { date } from "@/features/sessions/display";
import { useSharedRead, useSharedQuery } from "@/shared/hooks/use-shared-read";
import { accountsResource } from "@/features/accounts/queries";
import { providerUsageResource } from "../queries";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import type { Provider } from "../types";
import {
  overviewAccountState,
  overviewMoney,
  overviewListUsage,
  overviewUsage,
  providerOverviewEntries,
  type ProviderOverviewEntry,
} from "../overview";
import { costReason, providerCosts, providerTokens } from "../usage-display";

const typeName = {
  subscription: "订阅",
  api: "按量计费",
  unknown: "类型待确认",
};
type OverviewRow = ProviderOverviewEntry & {
  status: string;
  balance: string;
  windows: (QuotaForecast & { resetLabel: string })[];
  cost: string;
  fullCost: string;
  tokens: string;
  usageStatus: string | null;
};
const columns: ColumnDef<OverviewRow>[] = [
  {
    id: "account",
    header: "Provider / 账户",
    cell: ({ row }) => (
      <div className="max-w-52 space-y-0.5 whitespace-normal break-words">
        <p className="font-medium">{row.original.name}</p>
        <p className="text-label text-fg-subtle">
          {typeName[row.original.type]}
        </p>
      </div>
    ),
  },
  {
    id: "balance",
    header: "余额 / 剩余额度",
    cell: ({ row: { original: entry } }) =>
      entry.type === "subscription" ? (
        <div className="min-w-36 max-w-64 space-y-1 whitespace-normal text-label">
          {entry.windows.length ? (
            entry.windows.map((window) => (
              <div key={window.id} className="tabular-nums">
                <p>
                  {window.label} · 剩余 {quotaPercent(window.remainingPercent)}
                </p>
                <p className="text-fg-subtle">{window.resetLabel}</p>
              </div>
            ))
          ) : (
            <>
              <p>剩余额度未知</p>
              <p className="text-fg-subtle">重置时间未知</p>
            </>
          )}
        </div>
      ) : (
        <span className="block max-w-48 whitespace-normal break-words tabular-nums">
          {entry.balance}
        </span>
      ),
  },
  {
    id: "cost",
    header: "近 7 天费用",
    cell: ({ row }) => (
      <span
        className="block max-w-52 whitespace-normal break-words text-label tabular-nums"
        title={row.original.fullCost}
      >
        {row.original.cost}
      </span>
    ),
  },
  {
    id: "tokens",
    header: "近 7 天 Token",
    cell: ({ row }) => (
      <span className="block max-w-40 whitespace-normal break-words text-label tabular-nums">
        {row.original.tokens}
      </span>
    ),
  },
  {
    id: "status",
    header: "状态",
    cell: ({ row }) => (
      <div className="max-w-40 whitespace-normal text-label text-fg-muted">
        <p>{row.original.status}</p>
        {row.original.usageStatus && <p>{row.original.usageStatus}</p>}
      </div>
    ),
  },
];

export function ProviderOverview({
  providers,
  providerId,
}: {
  providers: Provider[];
  providerId?: string;
}) {
  const accounts = useSharedRead(accountsResource);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [accountDialog, setAccountDialog] = useState<string | null>(null);
  const [now, setNow] = useState(Date.now());
  const entries = useMemo(
    () =>
      providerOverviewEntries(
        providers,
        accounts.data?.accounts ?? [],
        providerId,
      ),
    [providers, accounts.data, providerId],
  );
  const selected =
    entries.find((e) => e.id === selectedId) ??
    (providerId !== undefined
      ? (entries.find((e) => e.account?.snapshot) ?? entries[0])
      : undefined);
  const listUsage = useSharedQuery(
    providerUsageResource,
    { source: "gateway", days: 7 },
    providerId === undefined && providers.some((p) => p.configured),
  );
  const selectedProviderId = selected?.provider?.id;
  const usage = useSharedQuery(
    providerUsageResource,
    { source: "gateway", days: 7, providerId: selectedProviderId },
    !!selectedProviderId && selected?.type !== "subscription",
  );
  const totals = overviewUsage(usage.data, selectedProviderId);
  const hasSnapshots =
    accounts.data?.accounts.some((a) => !!a.snapshot) ?? false;
  useEffect(() => {
    if (!hasSnapshots) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [hasSnapshots]);
  const rowData = providerId === undefined ? listUsage.data : usage.data;
  const rowError = providerId === undefined ? listUsage.error : usage.error;
  const rowLoading =
    providerId === undefined ? listUsage.loading : usage.loading;
  const rows = useMemo(
    () =>
      entries.map((entry) => {
        const totals =
          providerId === undefined
            ? overviewListUsage(rowData, entry.provider?.id)
            : overviewUsage(rowData, entry.provider?.id);
        const pending = rowLoading && !rowData;
        const unknown = pending ? "加载中…" : "未知";
        const subscription = entry.type === "subscription";
        return {
          ...entry,
          status: overviewAccountState(entry.account, now, !!accounts.error),
          balance: overviewMoney(entry.account, "balance"),
          windows: entry.account
            ? accountQuotaForecasts(entry.account, now, !!accounts.error).map(
                (window) => ({
                  ...window,
                  resetLabel:
                    window.resetsAt === null
                      ? "重置时间未知"
                      : window.resetsAt <= now
                        ? "已到重置时间 · 待更新"
                        : `${date(new Date(window.resetsAt).toISOString())} 重置`,
                }),
              )
            : [],
          cost: subscription
            ? "—"
            : totals
              ? totals.calls === 0
                ? "0 · 无调用"
                : providerCosts(totals.cost, true)
              : unknown,
          fullCost: subscription
            ? "订阅按额度窗口展示"
            : totals
              ? providerCosts(totals.cost)
              : unknown,
          tokens: subscription
            ? "—"
            : totals
              ? providerTokens(totals)
              : unknown,
          usageStatus:
            !subscription && entry.provider && rowError
              ? rowData
                ? "调用未更新 · 上次统计"
                : "调用统计读取失败"
              : !subscription && entry.provider && !pending && !totals
                ? "调用范围未确认"
                : null,
        };
      }),
    [entries, now, accounts.error, providerId, rowData, rowError, rowLoading],
  );
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (row) => row.id,
    enableRowSelection: false,
    enableHiding: false,
    autoResetPageIndex: false,
    state: { pagination: { pageIndex: 0, pageSize: Math.max(1, rows.length) } },
  });
  const account = selected?.account ?? null;
  const accountStatus = overviewAccountState(account, now, !!accounts.error);
  const balance =
    account && selectedProviderId
      ? estimateProviderBalance(
          account,
          selectedProviderId,
          usage.data,
          now,
          !!accounts.error,
          !!usage.error,
        )
      : null;
  const unknownReasons = Object.entries(
    totals?.unknownCostReasons ?? {},
  ).filter(([, count]) => count > 0);
  return (
    <section aria-label="Provider 用量概览" className="min-w-0 space-y-3 pt-3">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <h2 className="text-label text-fg-muted">
          {entries.length} 个账户 · 费用与 Token：近 7 天网关记录
        </h2>
        <Button
          variant="tertiary"
          size="sm"
          loading={accounts.loading || listUsage.loading || usage.loading}
          onClick={() => {
            accounts.reload();
            listUsage.reload();
            usage.reload();
          }}
        >
          刷新概览
        </Button>
      </div>
      {accounts.error && (
        <Alert status="warning">
          <AlertTitle>账户目录未更新</AlertTitle>
          <AlertDescription>
            {accounts.error}。保留上次数据，暂停预测。
          </AlertDescription>
        </Alert>
      )}
      {listUsage.error && (
        <Alert status="warning">
          <AlertTitle>列表调用统计未更新</AlertTitle>
          <AlertDescription>
            {listUsage.error}。
            {listUsage.data ? "保留上次统计。" : "费用与 Token 保持未知。"}
          </AlertDescription>
        </Alert>
      )}
      {accounts.loading && !accounts.data && (
        <p role="status" className="text-body text-fg-muted">
          正在读取账户目录…
        </p>
      )}
      {entries.length > 0 && (
        <>
          <div className="space-y-1">
            <DataTable
              table={table}
              aria-label="Provider 与计费账户概览"
              activeRowId={selected?.id}
              onRowActivate={(row) => setSelectedId(row.original.id)}
              showPagination={false}
            />
            <p className="text-label text-fg-subtle">
              选择账户查看预测与明细，支持 Enter 或空格键。金额按币种分别展示。
            </p>
          </div>
          {selected && (
            <section
              aria-label={`${selected.name}用量概览`}
              className="min-w-0 space-y-3 border-t border-border pt-3"
            >
              <div className="flex flex-wrap items-start justify-between gap-3">
                <div className="min-w-0 space-y-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <h3 className="break-words text-body font-medium">
                      {providerId && selected.name === selected.provider?.name
                        ? selected.type === "subscription"
                          ? "订阅预测与明细"
                          : "金额预测与明细"
                        : selected.name}
                    </h3>
                    <Badge>{typeName[selected.type]}</Badge>
                    <Badge>{accountStatus}</Badge>
                  </div>
                </div>
                <div className="flex flex-wrap gap-2">
                  {account && (
                    <Button
                      variant="secondary"
                      size="sm"
                      onClick={() => setAccountDialog(account.id)}
                    >
                      查看账户与读取用量
                    </Button>
                  )}
                  {selected.provider && providerId === undefined && (
                    <Button variant="tertiary" size="sm" asChild>
                      <a
                        href={`#/providers/${encodeURIComponent(selected.provider.id)}`}
                      >
                        连接详情
                      </a>
                    </Button>
                  )}
                </div>
              </div>
              {selected.type === "subscription" && account ? (
                <>
                  <dl className="flex flex-wrap gap-x-6 gap-y-2 text-label">
                    <div>
                      <dt className="text-label text-fg-muted">订阅方案</dt>
                      <dd className="mt-1 font-medium">
                        {account.snapshot?.plan ?? "未知"}
                      </dd>
                    </div>
                    <div>
                      <dt className="text-label text-fg-muted">额度采集</dt>
                      <dd className="mt-1">
                        {account.snapshot
                          ? date(account.snapshot.asOf)
                          : "尚未读取"}
                      </dd>
                    </div>
                  </dl>
                  <ProviderQuotaForecast
                    account={account}
                    now={now}
                    directoryError={!!accounts.error}
                  />
                  {!account.snapshot && (
                    <p className="text-body text-fg-muted">
                      尚未取得订阅额度。通过“查看账户与读取用量”读取或接入来源后，将显示各周期已用、剩余、重置时间与预测。
                    </p>
                  )}
                </>
              ) : (
                <>
                  {usage.error && (
                    <Alert status="warning">
                      <AlertTitle>调用统计未更新</AlertTitle>
                      <AlertDescription>
                        {usage.error}。
                        {usage.data
                          ? "以下为上次统计，暂停余额预测。"
                          : "费用与用量保持未知。"}
                      </AlertDescription>
                    </Alert>
                  )}
                  <dl className="flex flex-wrap gap-x-6 gap-y-2 text-label">
                    <div>
                      <dt className="text-fg-muted">近 7 天费用明细</dt>
                      <dd>{totals ? providerCosts(totals.cost) : "未知"}</dd>
                    </div>
                    <div>
                      <dt className="text-fg-muted">调用 / 错误 / 进行中</dt>
                      <dd>
                        {totals
                          ? `${totals.calls.toLocaleString("zh-CN")} / ${totals.errors} / ${totals.inProgress}`
                          : "未知"}
                      </dd>
                    </div>
                    <div>
                      <dt className="text-fg-muted">输入 / 输出 Token</dt>
                      <dd>
                        {totals
                          ? `${providerTokens(totals, "inputTokens")} / ${providerTokens(totals, "outputTokens")}`
                          : "未知"}
                      </dd>
                    </div>
                  </dl>
                  <dl className="grid gap-3 sm:grid-cols-2">
                    <div>
                      <dt className="text-label text-fg-muted">
                        本期已用金额 · 账户来源
                      </dt>
                      <dd className="mt-1 break-words text-body">
                        {overviewMoney(account, "period_spend")}
                      </dd>
                      <p className="mt-1 text-label text-fg-subtle">
                        {account?.snapshot?.money
                          .filter((m) => m.kind === "period_spend")
                          .map((m) => m.period ?? "账期未提供")
                          .join(" / ") || "来源未提供账期消费，不以余额倒推"}
                      </p>
                    </div>
                    <div>
                      <dt className="text-label text-fg-muted">
                        余额消耗估算 · 剩余时长
                      </dt>
                      <dd className="mt-1 text-body">
                        {balanceDuration(balance?.remainingDays ?? null)}
                      </dd>
                      <p className="mt-1 text-label text-fg-subtle">
                        {balance?.reasons.join(" ") ||
                          (balance?.remainingDays != null
                            ? "按近 7 天该连接记录的同币种费用估算"
                            : "缺少可关联的账户与费用依据")}
                      </p>
                    </div>
                  </dl>
                  {unknownReasons.length > 0 && (
                    <div className="space-y-1 text-label text-fg-muted">
                      <p>费用未知原因</p>
                      <ul className="list-disc space-y-1 pl-5">
                        {unknownReasons.map(([reason, count]) => (
                          <li key={reason}>
                            {count} 次：{costReason(reason)}
                          </li>
                        ))}
                      </ul>
                    </div>
                  )}
                  <p className="text-label text-fg-subtle">
                    调用统计仅覆盖此连接近 7 天已保留的网关请求，包含全部 Agent
                    与模型；不代表完整官方账单，不与原生消费相加，不自动换汇。更新于{" "}
                    {usage.data ? date(usage.data.asOf) : "未知"}。
                  </p>
                </>
              )}
              {account?.scope && (
                <p className="text-label text-fg-subtle">
                  {account.scope}
                  {account.snapshot
                    ? ` · 账户采集于 ${date(account.snapshot.asOf)}`
                    : ""}
                </p>
              )}
            </section>
          )}
        </>
      )}
      {!accounts.loading && !accounts.error && !entries.length && (
        <ConnectionEmpty
          compact
          title="暂无 Provider 账户"
          description="添加服务商连接或接入本机订阅后，这里会按账户类型展示额度、金额与用量。"
        />
      )}
      <Dialog
        open={accountDialog !== null}
        onOpenChange={(open) => {
          if (!open) {
            setAccountDialog(null);
            void accounts.invalidate();
          }
        }}
      >
        <DialogContent
          size="lg"
          className="max-h-[calc(100dvh-2rem)] overflow-y-auto"
        >
          <DialogHeader>
            <DialogTitle>账户用量与读取</DialogTitle>
          </DialogHeader>
          {accountDialog && (
            <AccountQuota
              key={accountDialog}
              accountId={accountDialog}
              providerId={providerId}
            />
          )}
        </DialogContent>
      </Dialog>
    </section>
  );
}
