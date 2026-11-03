import type { ModelUsageTotals } from "@/features/agents/types";
import type { Usage } from "@/shared/types/usage";

export function usageNumber(
  totals: ModelUsageTotals | undefined,
  key: keyof Usage,
) {
  const value = totals?.usage[key];
  if (value == null || !Number.isFinite(value) || value < 0) return "未报告";
  const partial = (totals?.reported[key] ?? 0) < (totals?.attempts ?? 0);
  return `${value.toLocaleString("zh-CN")}${partial ? " · 部分" : ""}`;
}
