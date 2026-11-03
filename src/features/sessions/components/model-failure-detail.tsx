import { date } from "@/features/sessions/display";
import type { ConversationEvent } from "@/features/sessions/types";

export const failureLabels: Record<string, string> = {
  connection: "连接异常",
  authentication: "认证 / 权限异常",
  rate_limit: "额度 / 频率限制",
  timeout: "超时",
  upstream: "上游服务异常",
  context_limit: "上下文超限",
  cancelled: "已中止",
  unknown: "原因分类未知",
};
const sourceLabels: Record<string, string> = {
  opencode_log: "OpenCode 当前诊断日志",
  opencode_message: "OpenCode 原生消息",
  claude_api_error: "Claude Code 原生 API 错误事件",
  claude_api_message: "Claude Code 原生 API 错误消息",
  codex_event: "Codex 原生错误事件",
  codex_turn: "Codex 原生轮次结果",
};

export function ModelFailureDetail({ event }: { event: ConversationEvent }) {
  const failure = event.failure;
  if (!failure) return null;
  return (
    <div className="min-w-0 space-y-3 rounded-lg border border-border p-3">
      <h4 className="text-body font-medium">
        {failureLabels[failure.category] ?? failureLabels.unknown}
      </h4>
      <dl className="grid gap-2 text-label sm:grid-cols-2">
        <div>
          <dt className="text-fg-muted">来源</dt>
          <dd>{sourceLabels[failure.source] ?? "原生来源"}</dd>
        </div>
        <div>
          <dt className="text-fg-muted">错误代码</dt>
          <dd className="break-all">{failure.code ?? "来源未提供"}</dd>
        </div>
        <div>
          <dt className="text-fg-muted">HTTP 状态</dt>
          <dd>{failure.httpStatus ?? "未知（不推断状态码）"}</dd>
        </div>
        <div>
          <dt className="text-fg-muted">来源是否标记可重试</dt>
          <dd>
            {failure.retryable === null
              ? "未知"
              : failure.retryable
                ? "是"
                : "否"}
          </dd>
        </div>
      </dl>
      <pre className="max-w-full overflow-x-auto whitespace-pre-wrap break-words font-mono text-label">
        {failure.message ?? "来源未提供可读错误原因。"}
      </pre>
      {failure.source === "opencode_log" && (
        <>
          <p className="text-label text-fg-muted">
            同会话同类日志 {failure.occurrences} 条 · 最近一次{" "}
            {date(failure.lastObservedAt)}。
            日志记录了调用异常，无法仅据此确认本轮最终结果；请求数、重试次数、Token
            和费用未知。
          </p>
          {event.turnId.endsWith(":turn:unknown") && (
            <p className="text-label text-fg-muted">
              日志只确认了所属 Session，轮次关联未知。
            </p>
          )}
        </>
      )}
      {event.provider === "privatemode-ai" &&
        failure.category === "connection" && (
          <p className="text-label text-fg-muted">
            Privatemode 默认需要本机代理
            http://localhost:8080/v1。请检查代理是否运行或自定义地址是否正确；模型出现在目录中不代表端点可用。
          </p>
        )}
    </div>
  );
}
