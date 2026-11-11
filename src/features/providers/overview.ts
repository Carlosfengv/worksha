import type { BillingAccount } from "@/features/accounts/types";
import type { Provider, ProviderUsage } from "./types";

export interface ProviderOverviewEntry {
  id: string;
  name: string;
  type: "subscription" | "api" | "unknown";
  provider: Provider | null;
  account: BillingAccount | null;
}

export function providerOverviewEntries(
  providers: readonly Provider[],
  accounts: readonly BillingAccount[],
  providerId?: string,
): ProviderOverviewEntry[] {
  const entries: ProviderOverviewEntry[] = [];
  for (const provider of providers.filter((p) =>
    providerId === undefined ? p.configured : p.id === providerId,
  )) {
    const linked = accounts.filter((a) => a.providerId === provider.id);
    if (!linked.length) {
      entries.push({
        id: `connection:${provider.id}`,
        name: provider.name,
        type: "unknown",
        provider,
        account: null,
      });
    }
    for (const account of linked) {
      entries.push({
        id: `account:${account.id}`,
        name: account.name,
        type: account.accountType,
        provider,
        account,
      });
    }
  }
  if (providerId === undefined) {
    // Native subscriptions are first-class accounts, not inferred API links.
    for (const account of accounts.filter((a) => a.providerId === null)) {
      entries.push({
        id: `account:${account.id}`,
        name: account.name,
        type: account.accountType,
        provider: null,
        account,
      });
    }
  }
  return entries;
}

export function overviewAccountState(
  account: BillingAccount | null,
  now: number,
  directoryError = false,
): string {
  if (directoryError) return "目录读取失败 · 上次数据";
  if (!account) return "未关联计费账户";
  if (account.state === "unsupported") return "读取能力未接入";
  if (account.state === "not_available") return "来源未提供额度";
  if (account.state === "subscription_authorization_failed")
    return "订阅授权未通过";
  if (account.state === "subscription_unconfirmed") return "订阅状态待确认";
  if (account.error || account.state === "refresh_failed")
    return account.snapshot ? "刷新失败 · 上次快照" : "读取失败";
  const snapshot = account.snapshot;
  if (
    snapshot &&
    (!Number.isFinite(Date.parse(snapshot.asOf)) ||
      Date.parse(snapshot.asOf) > now ||
      !Number.isFinite(snapshot.validForSeconds) ||
      snapshot.validForSeconds < 0)
  )
    return "快照有效期未知";
  if (
    account.state === "stale" ||
    (snapshot &&
      now - Date.parse(snapshot.asOf) > snapshot.validForSeconds * 1000)
  )
    return "快照已过期";
  return (
    {
      not_loaded: "尚未读取",
      ready: "已读取",
      refreshing: "正在刷新",
      unsupported: "读取能力未接入",
      not_available: "来源未提供额度",
      failed: "读取失败",
    }[account.state] ?? "状态未知"
  );
}

export function overviewMoney(account: BillingAccount | null, kind: string) {
  const values = account?.snapshot?.money.filter((m) => m.kind === kind) ?? [];
  return values.length
    ? values
        .map((m) =>
          m.amount !== null &&
          /^(?:0|[1-9]\d*)(?:\.\d+)?$/.test(m.amount) &&
          Number.isFinite(Number(m.amount)) &&
          Number(m.amount) <= Number.MAX_SAFE_INTEGER &&
          /^[A-Z]{3}$/.test(m.currency)
            ? `${m.amount} ${m.currency}`
            : "未知",
        )
        .join(" / ")
    : "未知";
}

export function overviewUsage(
  data: ProviderUsage | null,
  providerId: string | undefined,
) {
  return data &&
    providerId &&
    data.query.providerId === providerId &&
    data.query.source === "gateway" &&
    data.query.days === 7 &&
    !data.query.agent &&
    !data.query.model &&
    data.sourceState === "ready"
    ? data.selected
    : null;
}

/** The list uses the full seven-day connection aggregates, never the call page. */
export function overviewListUsage(
  data: ProviderUsage | null,
  providerId: string | undefined,
) {
  return data &&
    providerId &&
    data.query.providerId == null &&
    data.query.source === "gateway" &&
    data.query.days === 7 &&
    data.query.agent == null &&
    data.query.model == null &&
    data.query.cursor == null &&
    data.sourceState === "ready"
    ? (data.providers.find((p) => p.providerId === providerId)?.totals ?? null)
    : null;
}
