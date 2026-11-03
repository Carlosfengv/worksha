import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { useBackendReadEpoch } from "@/shared/hooks/use-backend-read-epoch";
import { backendReadEpoch } from "@/shared/lib/backend-recovery";
import { recordAgentKey } from "@/shared/types/agent";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  DetailList,
  DetailListItem,
  DetailListLabel,
  DetailListValue,
} from "@/components/ui/detail-list";
import { mcpApi } from "@/features/mcp/api";
import { sessionsApi } from "@/features/sessions/api";
import { useEffect, useState } from "react";

import type { McpGroup } from "@/features/mcp/types";
import {
  clientAttributionLabel,
  date,
  sessionLink,
  status,
} from "@/features/sessions/display";
import type {
  Conversation,
  ConversationEvent,
  NativeCall,
} from "@/features/sessions/types";

export const mcpElapsed = (ms: number | null) =>
  ms == null ? "未知" : `${(ms / 1000).toFixed(3)} 秒`;

export function McpLatency({
  service,
  scope,
  asOf,
}: {
  service: Pick<McpGroup, "name" | "p95Ms" | "durationSamples">;
  scope: string;
  asOf: string;
}) {
  return (
    <div className="space-y-1 text-label">
      <p>
        所属服务 P95：
        <span className="font-medium">{mcpElapsed(service.p95Ms)}</span> ·{" "}
        {service.durationSamples} 个有效耗时样本
      </p>
      <p className="text-fg-muted">
        {service.name} · {scope} · 快照 {date(asOf)}
      </p>
      <p className="text-fg-subtle">
        P95
        为该服务在此范围的原生终止耗时统计，包含正常返回和原生错误；不计取消、超时、拒绝、未完成或缺失耗时。来源未知的分组统计不代表某个已确认服务。
      </p>
    </div>
  );
}

/** Reuse the same call lookup for MCP details opened inside a Session. */
export function SessionMcpLatency({
  event,
  agentId,
}: {
  event: ConversationEvent;
  agentId: NativeCall["agentId"];
}) {
  const { agentNames } = useAgentDirectory();
  const [state, setState] = useState<{
    service?: McpGroup;
    asOf?: string;
    error?: string;
  }>({});
  const [retry, setRetry] = useState(0);
  const epoch = useBackendReadEpoch();
  const agent = recordAgentKey({
    agentId,
    agentProductId: event.agentProductId,
  });
  useEffect(() => {
    let alive = true;
    mcpApi
      .mcp({ agent, call: event.id })
      .then((data) => {
        if (alive && epoch === backendReadEpoch())
          setState({
            service: data.groups.find((s) => s.id === data.selectedGroupId),
            asOf: data.asOf,
          });
      })
      .catch((reason) => {
        if (alive && epoch === backendReadEpoch())
          setState({
            error: reason instanceof Error ? reason.message : String(reason),
          });
      });
    return () => {
      alive = false;
    };
  }, [agent, event.id, retry, epoch]);
  return state.service && state.asOf ? (
    <McpLatency
      service={state.service}
      scope={`${agentNames[agent]} · 全部已授权会话`}
      asOf={state.asOf}
    />
  ) : (
    <div className="space-y-1 text-label text-fg-muted">
      <p>所属服务 P95：{state.error ? `未知 · ${state.error}` : "正在读取…"}</p>
      {state.error && (
        <Button
          size="sm"
          variant="ghost"
          onClick={() => {
            setState({});
            setRetry((n) => n + 1);
          }}
        >
          重试服务统计
        </Button>
      )}
    </div>
  );
}

function Payload({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0 space-y-2">
      <h4 className="text-body font-medium">{label}</h4>
      <pre className="max-h-96 max-w-full overflow-auto whitespace-pre-wrap break-words rounded-lg border border-border p-3 font-mono text-label">
        {value}
      </pre>
    </div>
  );
}

export function McpCallDetail({
  row,
  service,
  scope,
  asOf,
}: {
  row: {
    call: NativeCall;
    projectPath: string | null;
    relatedUsage?: { totalTokens: number | null } | null;
    relatedUsageState?: string;
    relatedAgentProductId?: string | null;
  };
  service: Pick<McpGroup, "name" | "p95Ms" | "durationSamples">;
  scope: string;
  asOf: string;
}) {
  const { agentNames } = useAgentDirectory();
  const { call } = row;
  const [state, setState] = useState<{ data?: Conversation; error?: string }>(
    {},
  );
  const [retry, setRetry] = useState(0);
  const epoch = useBackendReadEpoch();
  useEffect(() => {
    let alive = true;
    sessionsApi
      .conversation(call.sessionId, undefined, call.id)
      .then((data) => {
        if (alive && epoch === backendReadEpoch())
          setState(
            data.items.some((e) => e.id === call.id)
              ? { data }
              : { error: "原生调用记录已变化或无法定位，请刷新调用列表。" },
          );
      })
      .catch((reason) => {
        if (alive && epoch === backendReadEpoch())
          setState({
            error: reason instanceof Error ? reason.message : String(reason),
          });
      });
    return () => {
      alive = false;
    };
  }, [call.id, call.sessionId, retry, epoch]);
  const event = state.data?.items.find((e) => e.id === call.id);
  return (
    <section aria-label="MCP 调用详情" className="min-w-0 space-y-4 p-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="break-all text-title font-medium">
          调用详情 · {call.toolName ?? call.name}
        </h3>
        <Button
          size="sm"
          variant="ghost"
          onClick={() => {
            window.location.hash = sessionLink(call.sessionId, {
              agent: recordAgentKey(call),
              anchor: call.id,
              returnTo: window.location.hash.replace(/^#/, ""),
            });
          }}
        >
          查看 Session
        </Button>
      </div>
      <DetailList>
        {[
          [
            "MCP 服务 / 工具",
            `${call.server ?? "未知"} / ${call.toolName ?? "未知（保留原调用名）"}`,
          ],
          [
            "Agent / Session",
            `${agentNames[recordAgentKey(call)]} / ${call.sessionTitle}`,
          ],
          ["项目", row.projectPath ?? "未知"],
          ["模型", call.model ?? "未知"],
          [
            "客户端归属依据",
            clientAttributionLabel(call.agentAttribution, call.agentId),
          ],
          [
            "关联模型响应 Token",
            row.relatedUsage?.totalTokens == null
              ? row.relatedUsageState === "outside_agent_scope"
                ? "未计入：关联响应来源不在所选 Agent 范围"
                : "未知：没有明确关联的模型响应 usage"
              : `${row.relatedUsage.totalTokens.toLocaleString()} · 整次关联响应，非工具独立计费`,
          ],
          [
            "关联响应 Agent",
            row.relatedAgentProductId
              ? agentNames[row.relatedAgentProductId]
              : "客户端未知",
          ],
          ["原生调用 ID", event?.callId ?? event?.nativeId ?? call.id],
          [
            "开始 / 结束",
            `${date(event?.timestamp ?? call.timestamp)} / ${date(event?.endTime)}`,
          ],
          [
            "本次调用延迟",
            mcpElapsed(event ? event.durationMs : call.durationMs),
          ],
          [
            "结果",
            (event?.status ?? call.status) === "error"
              ? "原生错误（细分未知）"
              : status(event?.status ?? call.status),
          ],
        ].map(([label, value]) => (
          <DetailListItem key={label}>
            <DetailListLabel>{label}</DetailListLabel>
            <DetailListValue>
              <span className="break-all">{value}</span>
            </DetailListValue>
          </DetailListItem>
        ))}
      </DetailList>
      <McpLatency service={service} scope={scope} asOf={asOf} />
      {state.error ? (
        <Alert status="danger">
          <AlertTitle>调用内容暂不可读</AlertTitle>
          <AlertDescription>{state.error}</AlertDescription>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              setState({});
              setRetry((n) => n + 1);
            }}
          >
            重试读取内容
          </Button>
        </Alert>
      ) : !event ? (
        <p className="text-label text-fg-muted" role="status">
          正在读取原生 Input / Output…
        </p>
      ) : (
        <>
          {event.status === "error" && (
            <Alert status="danger">
              <AlertTitle>原生调用错误</AlertTitle>
              <AlertDescription>
                原始错误保留在
                Output；来源未细分时，协议、传输或工具错误类型保持未知。
              </AlertDescription>
            </Alert>
          )}
          <div className="grid min-w-0 gap-4 xl:grid-cols-2">
            <Payload
              label="Input · 输入参数"
              value={event.input ?? "未知：原生记录未提供可读参数。"}
            />
            <Payload
              label="Output · 返回结果 / 原始错误"
              value={
                event.output ??
                (["returned", "error"].includes(event.status)
                  ? "原生调用已返回，但来源未提供可读结果。"
                  : "尚未记录结果，不能据此认定调用仍在执行。")
              }
            />
          </div>
          {event.truncated && (
            <p className="text-label text-fg-muted">
              内容超过读取上限，展示已截断；完整内容仍保留在原生会话中。
            </p>
          )}
          {state.data?.warnings.map((w) => (
            <p key={w} className="break-all text-label text-fg-muted">
              {w}
            </p>
          ))}
          <p className="text-label text-fg-subtle">
            只读原生记录 · 敏感凭据已脱敏 ·
            延迟仅取原生明确耗时，不从消息间隔估算 · 此处不会重新执行调用
          </p>
        </>
      )}
    </section>
  );
}
