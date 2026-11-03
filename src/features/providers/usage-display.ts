import type { ProviderCostTotals, ProviderUsageTotals } from "./types";
import type { Usage } from "@/shared/types/usage";

export function providerTokens(
  totals: ProviderUsageTotals | null | undefined,
  field: keyof Usage = "totalTokens",
) {
  if (!totals) return "—";
  if (!totals.calls) return "0";
  const value = totals.usage[field];
  if (value == null) return "未报告";
  return `${value.toLocaleString("zh-CN")}${(totals.reported[field] ?? 0) < totals.calls ? " · 部分" : ""}`;
}
export const providerLatency = (value: number | null | undefined) =>
  value == null
    ? "未知"
    : `${(value / 1000).toLocaleString("zh-CN", { maximumFractionDigits: 3 })} 秒`;
export const attributionLabel = (value: string) =>
  ({
    provider_id: "请求连接 ID",
    route_snapshot: "历史路由快照",
    native_provider: "原生服务商标识",
    native_model: "专用模型标识",
    removed_provider: "原连接已移除",
    conflicting_evidence: "来源证据冲突",
  })[value] ?? "缺少连接归属证据";

export { money as providerMoney } from "../../shared/lib/money.ts";
import { money as providerMoney } from "../../shared/lib/money.ts";
import { summaryMoney } from "../../shared/lib/money.ts";
export function providerCosts(
  cost: ProviderCostTotals | null | undefined,
  compact = false,
) {
  if (!cost) return "—";
  const format = compact ? summaryMoney : providerMoney;
  const values = [
    ...cost.actual.map((m) => `${format(m.amount, m.currency)} · 实际`),
    ...cost.estimated.map((m) => `${format(m.amount, m.currency)} · 估算`),
    ...(cost.unknownCalls
      ? [`${cost.unknownCalls.toLocaleString("zh-CN")} 次未知`]
      : []),
  ];
  return values.join(" / ") || "—";
}
export const costReason = (reason: string | null) =>
  ({
    in_progress: "调用尚未结束",
    unconfirmed_usage: "调用中断，最终用量未确认",
    unknown_endpoint: "历史 Endpoint 未确认",
    native_price_unconfirmed: "原生记录未包含可核实的价格和扣费依据",
    missing_price: "缺少该 Endpoint 与模型的价格",
    unknown_time: "调用时间未知",
    price_outside_period: "调用不在已核实价格的适用期内",
    missing_usage: "输入或输出用量未报告",
    missing_usage_upstream_rejected:
      "上游拒绝跨协议请求，最终 Token 用量未报告",
    missing_usage_unsupported_content:
      "上游返回当时无法转换的思考或拒绝内容，最终 Token 用量未报告",
    missing_usage_client_cancelled:
      "客户端中断，未收到完整结束事件和最终 Token 用量",
    missing_cache_usage: "缓存命中用量未报告",
    inconsistent_usage: "用量明细不完整或不一致",
    rate_boundary: "调用跨越计价时段",
  })[reason ?? ""] ?? "费用依据未知";
