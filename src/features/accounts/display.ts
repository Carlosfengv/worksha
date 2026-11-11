import type { BillingAccount, RoutingQuotaStatus } from "./types";

export const routingQuotaLabels: Record<RoutingQuotaStatus, string> = {
  exhausted: "已知额度已用尽",
  recovery: "待额度恢复探测",
  recovering: "正在探测额度恢复",
  recoveryCooling: "额度恢复探测冷却中",
  recovered: "额度恢复探测通过",
  unknown: "额度状态未知",
};

export function accountsForProvider(
  accounts: readonly BillingAccount[],
  providerId?: string,
): readonly BillingAccount[] {
  return providerId === undefined
    ? accounts
    : accounts.filter((account) => account.providerId === providerId);
}

export function selectBillingAccount(
  accounts: readonly BillingAccount[],
  selected: string,
  agentId?: string,
): BillingAccount | undefined {
  return (
    accounts.find((account) => account.id === selected) ??
    (agentId
      ? accounts.find((account) => account.declaredFor?.includes(agentId))
      : accounts[0])
  );
}

export function resetCountdown(resetsAt: number | null, now: number): string {
  if (resetsAt === null) return "来源未提供重置时间";
  const seconds = Math.max(0, Math.ceil(resetsAt - now / 1000));
  if (!seconds) return "已到重置时间 · 等待用量更新";
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  return `${days ? `${days} 天 ` : ""}${hours.toString().padStart(2, "0")}:${minutes.toString().padStart(2, "0")}:${(seconds % 60).toString().padStart(2, "0")}`;
}
