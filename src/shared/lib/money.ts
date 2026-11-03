import type { CostTotals } from "@/shared/types/cost";

export function money(amount: number, currency: string, precision = 6) {
  const minimum = 10 ** -precision;
  if (amount > 0 && amount < minimum)
    return `< ${minimum.toFixed(precision)} ${currency}`;
  return `${amount.toLocaleString("zh-CN", { maximumFractionDigits: precision })} ${currency}`;
}
export function summaryMoney(amount: number, currency: string) {
  return money(amount, currency, Math.abs(amount) >= 1 ? 2 : 4);
}
export function costText(cost: CostTotals | null | undefined) {
  if (!cost) return "未知";
  return (
    [
      ...cost.actual.map((m) => `${money(m.amount, m.currency)} · 实际`),
      ...cost.estimated.map((m) => `${money(m.amount, m.currency)} · 估算`),
      ...(cost.unknownCalls ? [`${cost.unknownCalls} 次计价未知`] : []),
    ].join(" / ") || "未知"
  );
}
