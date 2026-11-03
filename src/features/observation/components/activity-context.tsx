import { date, number, sessionLink } from "@/features/sessions/display";
import type { SessionContextSample } from "../types";
import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";

function contextValue(
  agentNames: Record<string, string>,
  sample?: SessionContextSample,
) {
  const context = ["ready", "partial"].includes(sample?.state ?? "")
    ? sample?.context
    : null;
  const input = context?.inputTokens;
  const window = context?.windowTokens;
  const known = input != null && Number.isFinite(input) && input >= 0;
  const percentage =
    known && window != null && Number.isFinite(window) && window > 0
      ? (input / window) * 100
      : null;
  return {
    percentage,
    text:
      percentage != null
        ? `${percentage.toFixed(1)}% · 估算`
        : known
          ? `${number(input)} Token · 真实采集`
          : "未知",
    detail: [
      sample?.state === "pending"
        ? "会话有更新，等待采集"
        : sample?.state === "unavailable"
          ? "会话来源未授权或不可读取"
          : known
            ? `最近输入 ${number(input)} Token · 真实采集`
            : "尚无可确认的上下文用量",
      window != null ? `原生窗口 ${number(window)} Token` : "窗口上限未知",
      percentage != null
        ? "占比按最近输入 / 原生窗口估算，不代表此刻精确占用"
        : null,
      context?.source,
      context?.model,
      context?.sampleAt ? date(context.sampleAt) : null,
      context
        ? context.sampleAgentProductId
          ? agentNames[context.sampleAgentProductId]
          : "客户端来源未知"
        : null,
      sample?.state === "partial" ? "部分采集快照" : null,
    ]
      .filter(Boolean)
      .join(" · "),
  };
}

export function ActivityContext({
  sessionId,
  sample,
  onNavigate,
  label,
  compact = false,
}: {
  sessionId: string | null;
  sample?: SessionContextSample;
  onNavigate?: () => void;
  label?: string;
  compact?: boolean;
}) {
  const { agentNames } = useAgentDirectory();
  const value = contextValue(agentNames, sample);
  return sessionId ? (
    <a
      href={`#${sessionLink(sessionId, { view: "context" })}`}
      aria-label={`查看会话上下文：${sessionId}`}
      title={value.detail}
      onClick={onNavigate}
      className={`${compact ? "flex flex-wrap items-center gap-x-2 gap-y-1" : "block space-y-1"} rounded-lg p-2 text-label text-fg-muted outline-none hover:bg-hover hover:text-fg-default focus-visible:ring-1 focus-visible:ring-focus-ring`}
    >
      {label && (
        <span
          className={`${compact ? "min-w-0 flex-1 truncate" : "block line-clamp-1"} text-fg-default`}
          title={label}
        >
          {label}
        </span>
      )}
      <span className={compact ? "max-w-full shrink-0 truncate" : undefined}>
        上下文 {value.text}
        {sample?.state === "partial" ? " · 部分采集" : ""}
      </span>
      {value.percentage !== null && (
        <span
          aria-hidden="true"
          className="block h-1 w-full overflow-hidden rounded-full bg-surface-raised"
        >
          <span
            className="block h-full rounded-full bg-chart-1"
            style={{
              width: `${Math.min(100, Math.max(0, value.percentage))}%`,
            }}
          />
        </span>
      )}
    </a>
  ) : (
    <p
      className="p-2 text-label text-fg-subtle"
      title="请求未关联 Session，无法确认上下文"
    >
      {label && (
        <span className="block line-clamp-1 text-fg-default">{label}</span>
      )}
      上下文 未知 · 会话未关联
    </p>
  );
}
