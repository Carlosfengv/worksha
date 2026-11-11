import type { RequestQuery } from "./request-types";
import { recordAgentKey } from "@/shared/types/agent";

export function requestAgentName(
  record: { agentId: string; agentProductId?: string | null },
  names: Record<string, string>,
) {
  return record.agentId === "model-router"
    ? "模型服务 · 意图分类"
    : (names[recordAgentKey(record)] ?? "未知");
}
export const requestStates: Record<string, string> = {
  waiting: "等待输出",
  streaming: "输出中",
  success: "成功",
  recovered: "已恢复",
  failed: "失败",
  cancelled: "已取消",
  unknown: "结果未知",
};
export const requestIssues: Record<string, string> = {
  authentication: "鉴权 / 权限拒绝",
  upstream_rejection: "上游 HTTP 4xx",
  upstream_error: "上游 HTTP 5xx",
  rate_limit: "限流 429",
  timeout: "超时",
  connection: "连接失败",
  stream: "断流 / 响应不完整",
  protocol: "协议 / 解析失败",
  unmeasurable: "TTFT 不可测",
};
export const requestMetrics: Record<string, string> = {
  calls: "请求量",
  tokens: "Token",
  cost: "费用",
  ttft: "首次输出 TTFT",
  duration: "总耗时",
  throughput: "输出吞吐",
};
export const durationLabel = (ms: number | null | undefined) =>
  ms == null
    ? "不可测"
    : `${(ms / 1000).toLocaleString("zh-CN", { maximumFractionDigits: 3 })} 秒`;
export const requestValue = (value: number | null, metric: string) =>
  value == null
    ? "—"
    : ["ttft", "duration"].includes(metric)
      ? durationLabel(value)
      : metric === "throughput"
        ? `${value.toFixed(1)} Token/s`
        : metric === "cost" && value > 0 && value < 1e-9
          ? "<0.000000001"
          : value.toLocaleString("zh-CN", {
              maximumFractionDigits: metric === "cost" ? 9 : 0,
            });
export function calendarRange(range: string, now = Date.now()) {
  if (range === "all") return {};
  const day = 86_400_000;
  const start =
    Math.floor((now + 8 * 3_600_000) / day) * day -
    8 * 3_600_000 -
    (range === "7" ? 6 : range === "30" ? 29 : 0) * day;
  return {
    start: new Date(start).toISOString(),
    end: new Date(now).toISOString(),
  };
}
export function requestParams(
  query: RequestQuery,
  range: string,
  snapshot?: string,
  cursor?: string,
) {
  const params = new URLSearchParams({ range });
  Object.entries({ ...query, snapshot, cursor }).forEach(([key, value]) => {
    if (value != null) params.set(key, String(value));
  });
  return params.toString();
}
