import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { useBackendReadEpoch } from "@/shared/hooks/use-backend-read-epoch";
import { backendReadEpoch } from "@/shared/lib/backend-recovery";
import { recordAgentKey } from "@/shared/types/agent";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
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
  const [result, setResult] = useState<{
    epoch: symbol;
    service?: McpGroup;
    asOf?: string;
    error?: string;
  } | null>(null);
  const [retry, setRetry] = useState(0);
  const epoch = useBackendReadEpoch();
  const state = result?.epoch === epoch ? result : null;
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
          setResult({
            epoch,
            service: data.groups.find((s) => s.id === data.selectedGroupId),
            asOf: data.asOf,
          });
      })
      .catch((reason) => {
        if (alive && epoch === backendReadEpoch())
          setResult({
            epoch,
            error: reason instanceof Error ? reason.message : String(reason),
          });
      });
    return () => {
      alive = false;
    };
  }, [agent, event.id, retry, epoch]);
  return state?.service && state.asOf ? (
    <McpLatency
      service={state.service}
      scope={`${agentNames[agent]} · 全部已授权会话`}
      asOf={state.asOf}
    />
  ) : (
    <div className="space-y-1 text-label text-fg-muted">
      <p>
        所属服务 P95：
        {state?.error
          ? `未知 · ${state.error}`
          : state
            ? "暂无可核对的服务统计"
            : "正在读取…"}
      </p>
      {state?.error && (
        <Button
          size="sm"
          variant="ghost"
          onClick={() => {
            setResult(null);
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
  const [result, setResult] = useState<{
    epoch: symbol;
    data?: Conversation;
    error?: string;
  } | null>(null);
  const [retry, setRetry] = useState(0);
  const epoch = useBackendReadEpoch();
  const state = result?.epoch === epoch ? result : null;
  useEffect(() => {
    let alive = true;
    sessionsApi
      .conversation(call.sessionId, undefined, call.id)
      .then((data) => {
        if (alive && epoch === backendReadEpoch())
          setResult(
            data.items.some((e) => e.id === call.id)
              ? { epoch, data }
              : {
                  epoch,
                  error: "原生调用记录已变化或无法定位，请刷新调用列表。",
                },
          );
      })
      .catch((reason) => {
        if (alive && epoch === backendReadEpoch())
          setResult({
            epoch,
            error: reason instanceof Error ? reason.message : String(reason),
          });
      });
    return () => {
      alive = false;
    };
  }, [call.id, call.sessionId, retry, epoch]);
  const event = state?.data?.items.find((e) => e.id === call.id);
  const outcome = event?.status ?? call.status;
  return (
    <section aria-label="MCP 调用详情" className="min-w-0 space-y-4 p-4">
      <p className="text-body font-medium">
        {outcome === "error" ? "原生错误（细分未知）" : status(outcome)} · 耗时{" "}
        {mcpElapsed(event ? event.durationMs : call.durationMs)}
      </p>
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
        查看会话
      </Button>
      {state?.error ? (
        <Alert status="danger">
          <AlertTitle>调用内容暂不可读</AlertTitle>
          <AlertDescription>{state.error}</AlertDescription>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              setResult(null);
              setRetry((n) => n + 1);
            }}
          >
            重试读取内容
          </Button>
        </Alert>
      ) : !event ? (
        <p className="text-label text-fg-muted" role="status">
          正在读取原生调用内容…
        </p>
      ) : (
        <>
          <Payload
            label={outcome === "error" ? "原始错误" : "返回结果"}
            value={
              event.output ??
              (["returned", "error"].includes(outcome)
                ? "原生记录未提供可读结果。"
                : "原生记录没有结果，执行状态未知。")
            }
          />
          <Payload
            label="输入参数"
            value={event.input ?? "原生记录未提供可读参数。"}
          />
          {event.truncated && (
            <p className="text-label text-fg-muted">
              已截断。可在原生会话中查看完整内容。
            </p>
          )}
          {state?.data?.warnings.map((w) => (
            <p key={w} className="break-all text-label text-fg-muted">
              {w}
            </p>
          ))}
        </>
      )}
      <Accordion>
        <AccordionItem value="source">
          <AccordionTrigger>来源与统计</AccordionTrigger>
          <AccordionContent className="space-y-3">
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
          </AccordionContent>
        </AccordionItem>
      </Accordion>
    </section>
  );
}
