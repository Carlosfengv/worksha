import type {
  GatewayActivity,
  NativeActivity,
} from "@/features/observation/types";
import {
  liveMeasurements,
  nativeMeasurements,
  nativeSamplingLabel,
} from "./activity-group.ts";

export const phaseNames: Record<string, string> = {
  waiting_first: "等待首出",
  streaming: "正在输出正文",
  public_reasoning: "思考中",
  thinking: "思考中",
  tool_arguments: "生成工具参数",
  success: "请求已完成",
  error: "请求失败",
  interrupted: "连接中断",
  cancelled: "已取消",
  incomplete: "响应未完整完成",
  processing: "本轮已开始",
  tool_requested: "已提出工具调用",
  tool_returned: "工具结果已返回",
  reply_recorded: "收到模型回复",
  waiting_input: "等待输入",
  ended: "已结束",
};
export function seconds(ms: number | null | undefined) {
  return ms == null ? "—" : (ms / 1000).toFixed(ms < 1000 ? 3 : 2);
}
export function speed(value: number | null | undefined) {
  return value == null ? "—" : value.toFixed(1);
}
export function relativeTime(
  value: string | null | undefined,
  now = Date.now(),
) {
  const timestamp = value ? Date.parse(value) : NaN;
  if (!Number.isFinite(timestamp)) return "时间未知";
  const elapsed = Math.max(0, Math.floor((now - timestamp) / 1000));
  if (elapsed < 60) return `${elapsed} 秒前`;
  if (elapsed < 3600) return `${Math.floor(elapsed / 60)} 分钟前`;
  return `${Math.floor(elapsed / 3600)} 小时前`;
}

export function timingNote(issue: string | null | undefined) {
  if (!issue) return null;
  if (issue.includes("非流式")) return "非流式响应未提供时序";
  if (issue.includes("部分流")) return "部分流事件缺失";
  if (issue.includes("响应身份")) return "响应关联异常";
  if (issue.includes("中断") || issue.includes("没有该历史")) return "采集中断";
  return "时序采集异常";
}

/** Keep live measurements separate from saved request timings and native activity. */
export function activityMetrics({
  active,
  latest,
  native,
  stale,
}: {
  active?: GatewayActivity;
  latest?: GatewayActivity;
  native?: NativeActivity;
  stale: boolean;
}) {
  const measured = (
    label: string,
    value: number | null | undefined,
    unit: string,
    placeholder: string,
  ) => {
    const known = value != null && Number.isFinite(value) && value >= 0;
    return {
      label,
      value: known
        ? unit === "秒"
          ? seconds(value)
          : speed(value)
        : placeholder,
      unit: known ? unit : "",
    };
  };
  if (active) {
    const issue = active.request.timingIssue;
    const expired = stale || active.stale;
    const waiting =
      active.phase === "tool_arguments"
        ? "生成工具参数"
        : active.phase === "public_reasoning"
          ? "推理中"
          : "等待文本";
    return {
      speed: measured(
        active.outputTokensPerSecond != null ? "当前输出速度" : "当前文本速度",
        expired ? null : liveMeasurements(active).speed,
        "Token/s",
        expired ? "等待更新" : issue ? "采样恢复中" : waiting,
      ),
      ttft: measured(
        "当前请求 TTFT",
        active.request.ttftMs,
        "秒",
        issue ? "首出未记录" : expired ? "等待更新" : "等待首出",
      ),
      note: expired
        ? "实时采集已过期，首出仅保留已记录值。"
        : issue ||
          `速度为近 ${(active.sampling?.windowMs ?? 1000) / 1000} 秒输出 Token 估算，包含正文、公开思考和工具参数；首出按有效生成内容计时。`,
    };
  }
  if (latest) {
    const average = latest.averageTextTokensPerSecond;
    const hasAverage =
      average != null && Number.isFinite(average) && average > 0;
    return {
      speed: measured(
        hasAverage ? "最近文本均速 · 估算" : "最近请求吞吐",
        hasAverage ? average : latest.throughput,
        "Token/s",
        latest.request.status === "success" ? "未记录" : "未完成测量",
      ),
      ttft: measured(
        "最近请求 TTFT",
        latest.request.ttftMs,
        "秒",
        latest.request.timingIssue ? "首出未记录" : "未记录",
      ),
      note: hasAverage
        ? "已完成请求的正文均速，按可见文本估算。"
        : latest.request.timingIssue ||
          "吞吐按输出 Token ÷ 完整请求时长计算，包含首出等待；旧记录未采集正文均速。",
    };
  }
  if (native?.realtime) {
    const realtime = native.realtime;
    const value = nativeMeasurements(native);
    const finished = ["waiting_input", "error", "cancelled"].includes(
      native.phase,
    );
    const reported = native.turnTtftMs != null;
    return {
      speed: measured(
        finished
          ? "原生正文均速 · 估算"
          : realtime.outputTokensPerSecond !== undefined
            ? "原生实时输出速度"
            : "原生实时正文速度",
        stale
          ? null
          : finished
            ? realtime.averageTextTokensPerSecond
            : value.speed,
        "Token/s",
        stale ? "等待更新" : (nativeSamplingLabel(native) ?? "等待正文"),
      ),
      ttft: measured(
        reported
          ? "Agent 上报 TTFT"
          : realtime.firstTokenMs != null
            ? "原生实时 TTFT"
            : value.firstText != null
              ? "原生正文首出"
              : "原生 TTFT",
        value.ttft ?? value.firstText,
        "秒",
        realtime.firstTokenMissed ? "中途接入 · 待原生补报" : "等待首 Token",
      ),
      note: `直接订阅 Codex 桌面会话；首个推理或正文增量返回后显示观测 TTFT，包含桌面事件传递延迟，完成后以 Agent 上报值为准。速度为近 ${(realtime.sampling?.windowMs ?? 1000) / 1000} 秒可观测正文与思考增量估算，工具执行结果不计入。`,
    };
  }
  return {
    speed: measured("文本速度", null, "Token/s", "未采集"),
    ttft: measured(
      native?.turnTtftMs != null ? "Agent 上报首出" : "首出耗时",
      native?.turnTtftMs,
      "秒",
      "未采集",
    ),
    note: "原生会话记录通常不提供流式 Token 时序。经本地网关发起的新请求才可采集实时速度和 TTFT。",
  };
}
