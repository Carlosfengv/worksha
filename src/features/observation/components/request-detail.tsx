import { ObjectContributionSlots } from "@/features/plugins/object-detail-slots";
import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { useBackendReadEpoch } from "@/shared/hooks/use-backend-read-epoch";
import { backendReadEpoch } from "@/shared/lib/backend-recovery";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  DetailList,
  DetailListItem,
  DetailListLabel,
  DetailListValue,
} from "@/components/ui/detail-list";
import {
  Accordion,
  AccordionItem,
  AccordionTrigger,
  AccordionContent,
} from "@/components/ui/accordion";
import { recordAgentKey } from "@/shared/types/agent";
import { date, number, sessionLink } from "@/features/sessions/display";
import {
  providerCosts,
  providerMoney,
  costReason,
} from "@/features/providers/usage-display";
import { observationApi } from "../api";
import {
  durationLabel,
  requestStates,
  requestIssues,
} from "../request-display";
import type { RequestDetailResponse } from "../request-types";

export function RequestDetail({
  id,
  returnSearch,
}: {
  id: string;
  returnSearch: string;
}) {
  const { agentNames } = useAgentDirectory();
  const epoch = useBackendReadEpoch();
  const [state, setState] = useState<{
    id: string;
    data: RequestDetailResponse | null;
    error: string | null;
  }>({ id, data: null, error: null });
  useEffect(() => {
    let alive = true,
      running = false;
    const load = async () => {
      if (running) return;
      running = true;
      try {
        const data = await observationApi.requestDetail(id);
        if (alive && epoch === backendReadEpoch())
          setState({ id, data, error: null });
      } catch (reason) {
        if (alive && epoch === backendReadEpoch())
          setState((old) => ({
            id,
            data: old.id === id ? old.data : null,
            error: reason instanceof Error ? reason.message : String(reason),
          }));
      } finally {
        running = false;
      }
    };
    void load();
    const timer = window.setInterval(() => {
      if (!document.hidden) void load();
    }, 5000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [id, epoch]);
  const data = state.id === id ? state.data : null,
    r = data?.request;
  const fields: [string, string][] = r
    ? [
        [r.native ? "原生记录 ID" : "请求 ID", r.id],
        [r.native ? "上报模型" : "首次目标模型", r.model ?? "未报告"],
        ...(!r.native && r.attempts.length > 1
          ? ([
              ["最终路由模型", r.attempts.at(-1)?.routedModel ?? "未记录"],
              ["最终 Provider", r.attempts.at(-1)?.provider ?? "未记录"],
            ] as [string, string][])
          : []),
        [
          "结果",
          r.native
            ? `${requestStates[r.status]} · 原生标记 ${({ recorded: "已记录响应", unknown: "未知", error: "失败", failed: "失败", cancelled: "已取消" } as Record<string, string>)[r.native.reportedStatus] ?? "未知"}`
            : `${requestStates[r.status]} · ${r.attempts.length} 次尝试`,
        ],
        ["Agent", agentNames[recordAgentKey(r)]],
        ["调用来源", r.client ? `局域网 · ${r.client.name}` : "本机"],
        ...(r.client
          ? ([
              ["客户端 ID", r.client.id],
              ["连接地址", r.client.peer],
            ] as [string, string][])
          : []),
        ["Session", r.sessionTitle ?? r.sessionId ?? "缺少可靠关联"],
        ["项目", r.projectName ?? "未关联"],
        [
          r.native ? "原生记录时间" : "开始 / 结束",
          r.native
            ? date(r.startedAt)
            : `${date(r.startedAt)} / ${date(r.endedAt)}`,
        ],
        ["观察 TTFT", durationLabel(r.ttftMs)],
        [
          "总耗时",
          ["waiting", "streaming"].includes(r.status)
            ? "计时中"
            : durationLabel(r.durationMs),
        ],
        [
          "端到端输出吞吐",
          r.throughput === null
            ? "不可测"
            : `${r.throughput.toFixed(1)} Token/s`,
        ],
        [
          "输入 / 输出 Token",
          `${number(r.usage.inputTokens)} / ${number(r.usage.outputTokens)}`,
        ],
        [
          "缓存读取 / 写入",
          `${number(r.usage.cachedInputTokens)} / ${number(r.usage.cacheWriteInputTokens)}`,
        ],
        ["推理输出子集", number(r.usage.reasoningOutputTokens)],
        ["费用", providerCosts(r.cost)],
      ]
    : [];
  return (
    <section className="space-y-4" aria-label="请求详情">
      <Button
        variant="tertiary"
        onClick={() => {
          window.location.hash = `/requests${returnSearch ? `?${returnSearch}` : ""}`;
        }}
      >
        返回请求列表
      </Button>
      {state.id === id && state.error && (
        <p role="alert">读取失败，保留上次快照：{state.error}</p>
      )}
      {data?.sourceError && <p role="alert">{data.sourceError}</p>}
      {!data ? (
        <p role="status">正在读取请求详情…</p>
      ) : !r ? (
        <p>找不到此请求，可能已到期或未被记录。</p>
      ) : (
        <>
          <div className="rounded-xl border border-border p-4">
            <h2 className="mb-4 text-title font-medium">请求详情</h2>
            <DetailList>
              {fields.map(([label, value]) => (
                <DetailListItem key={label}>
                  <DetailListLabel>{label}</DetailListLabel>
                  <DetailListValue>
                    <span className="break-all">{value}</span>
                  </DetailListValue>
                </DetailListItem>
              ))}
            </DetailList>
            {r.usageMissing > 0 && (
              <p className="mt-3 text-label text-fg-muted">
                {r.usageMissing} {r.native ? "条响应" : "次尝试"}
                未完整报告用量，当前数字为已知小计。
              </p>
            )}
            {r.timingNote && (
              <p className="mt-3 text-label text-fg-muted">{r.timingNote}</p>
            )}
            <div className="mt-4 flex flex-wrap gap-2">
              {r.sessionId && !r.client && (
                <Button
                  variant="tertiary"
                  onClick={() => {
                    window.location.hash = sessionLink(r.sessionId!, {
                      anchor: r.native?.eventId,
                      returnTo: `/requests/${encodeURIComponent(id)}?${returnSearch}`,
                      view: r.native ? undefined : "requests",
                    });
                  }}
                >
                  打开所属会话
                </Button>
              )}
              <Button
                variant="tertiary"
                onClick={() => {
                  window.location.hash =
                    r.agentProductId || r.agentId === "opencode"
                      ? `/agents/${encodeURIComponent(recordAgentKey(r))}`
                      : "/agents";
                }}
              >
                打开 Agent
              </Button>
              {r.attempts[0]?.providerId && (
                <Button
                  variant="tertiary"
                  onClick={() => {
                    window.location.hash = `/providers/${encodeURIComponent(r.attempts[0].providerId!)}`;
                  }}
                >
                  打开 Provider
                </Button>
              )}
              {r.projectId && (
                <Button
                  variant="tertiary"
                  onClick={() => {
                    window.location.hash = `/projects/${encodeURIComponent(r.projectId!)}`;
                  }}
                >
                  打开项目
                </Button>
              )}
            </div>
          </div>
          {r.native && (
            <section
              className="space-y-3 rounded-xl border border-border p-4"
              aria-label="原生证据"
            >
              <h3 className="text-title font-medium">原生响应与跨源关联</h3>
              <p className="break-all text-label">
                响应 ID：{r.native.responseId ?? "未提供可关联的上游响应 ID"}
              </p>
              <p className="text-label">
                关联：
                {(
                  {
                    response_id: "明确响应 ID 一对一关联",
                    unlinked: "路径未确认，不能认定未经过网关",
                    ambiguous: "同一响应 ID 存在多个候选，未合并",
                    identity_conflict: "客户端或 Session 身份冲突，未合并",
                  } as Record<string, string>
                )[r.native.association] ?? "未知"}
              </p>
              {r.native.failure && (
                <p className="text-label">
                  原生异常：{r.native.failure.category} ·{" "}
                  {r.native.failure.code ?? "无错误码"} ·{" "}
                  {r.native.failure.httpStatus
                    ? `已报告 HTTP ${r.native.failure.httpStatus}`
                    : "HTTP 状态未知"}{" "}
                  · 来源 {r.native.failure.source}
                  。详细内容按授权在所属会话中查看。
                </p>
              )}
              {r.native.usageConflict && (
                <p className="text-label">
                  网关与原生用量字段冲突，两份证据分别保留。跨来源精确匹配统计采用网关用量。
                </p>
              )}
              {r.native.gatewayRequestId && (
                <Button
                  variant="tertiary"
                  onClick={() => {
                    window.location.hash = `/requests/${encodeURIComponent(r.native!.gatewayRequestId!)}?${returnSearch}`;
                  }}
                >
                  查看关联的网关请求
                </Button>
              )}
            </section>
          )}
          {!!r.nativeLinks?.length && (
            <section
              className="space-y-3 rounded-xl border border-border p-4"
              aria-label="跨源关联证据"
            >
              <h3 className="text-title font-medium">跨源关联证据</h3>
              <p className="text-label text-fg-muted">
                明确响应 ID 关联，消费只采用网关记录，不叠加原生用量。
              </p>
              {r.nativeLinks.map((link) => (
                <div key={link.eventId} className="space-y-1">
                  <p className="break-all text-label">
                    {link.responseId} · 尝试 {link.attemptId}
                    {link.usageConflict ? " · 用量冲突" : ""}
                  </p>
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => {
                      window.location.hash = `/requests/${encodeURIComponent(link.eventId)}?${returnSearch}`;
                    }}
                  >
                    查看原生证据
                  </Button>
                </div>
              ))}
            </section>
          )}
          {!r.native && (
            <h3 className="text-title font-medium">路由、时序与尝试</h3>
          )}
          <Accordion
            className="w-full"
            type="multiple"
            defaultValue={r.attempts.map((a) => a.id)}
          >
            {r.attempts.map((a, index) => (
              <AccordionItem key={a.id} value={a.id}>
                <AccordionTrigger>
                  尝试 {index + 1} · {a.provider} ·{" "}
                  {a.status === "success"
                    ? "成功"
                    : a.status === "in_progress"
                      ? "进行中"
                      : a.status === "cancelled"
                        ? "已取消"
                        : a.status === "interrupted"
                          ? "结果未知"
                          : "失败"}
                  {a.httpStatus ? ` · HTTP ${a.httpStatus}` : ""}
                </AccordionTrigger>
                <AccordionContent>
                  <div className="space-y-4">
                    <p className="text-body">
                      {agentNames[recordAgentKey(a)]} → 本地网关 → {a.provider}
                    </p>
                    <DetailList>
                      {[
                        ["尝试 ID", a.id],
                        ["上游 request ID", a.upstreamRequestId ?? "未报告"],
                        ["上游 response ID", a.responseId ?? "历史未采集"],
                        ["请求模型", a.requestedModel ?? "历史未记录"],
                        ["路由模型", a.routedModel ?? "历史未单独记录"],
                        ["历史记录模型", a.model ?? "未知"],
                        [
                          "上游报告模型",
                          a.reportedModel ?? "历史未记录，不能据发送模型确认",
                        ],
                        [
                          "端点 / 连接版本",
                          `${a.endpoint} · v${a.bindingRevision}`,
                        ],
                        ...(a.routing
                          ? [
                              ["动态绑定", a.routing.bindingId],
                              ["实际路由版本", a.routing.routeVersionId],
                              ...(a.routing.service
                                ? [
                                    ["模型服务", a.routing.service.serviceName],
                                    [
                                      "服务版本",
                                      a.routing.service.serviceVersionId,
                                    ],
                                    [
                                      "模型选择",
                                      `${a.routing.service.mode === "default" ? "跟随服务默认" : "优先指定模型"} · ${a.routing.service.preferredModelName ?? a.routing.service.modelName}`,
                                    ],
                                    [
                                      "本次实际模型",
                                      a.routing.service.modelName,
                                    ],
                                    [
                                      "路由尝试",
                                      `${a.routing.service.attempt ?? 1} · ${a.routing.service.fallbackReason ? ({ upstream_failure: "首选来源临时故障，已回退", upstream_rate_limit: "首选来源限流，已切换", configured_rate_limit: "首选来源达到配置上限", rate_limit_cooldown: "首选来源限流冷却中", failure_cooldown: "首选来源故障冷却中", preferred_unavailable: "首选条目已停用", policy_order: "按外部策略排序", recovery_probe_in_progress: "首选来源正在恢复" }[a.routing.service.fallbackReason] ?? "按服务策略回退") : "首选或原轮次目标"}`,
                                    ],
                                    ...(a.routing.service.budget
                                      ? [
                                          [
                                            "输入计数 / 输出预留",
                                            `${a.routing.service.budget.inputTokens.toLocaleString()} / ${a.routing.service.budget.outputTokens.toLocaleString()} Token · 含协议余量的估计`,
                                          ],
                                        ]
                                      : []),
                                    [
                                      "入口上下文 / 输出",
                                      `${a.routing.service.capabilities.contextWindow} / ${a.routing.service.capabilities.maxOutputTokens} Token`,
                                    ],
                                  ]
                                : []),
                              [
                                "Provider 快照版本",
                                `v${a.routing.providerRevision}`,
                              ],
                              [
                                "轮次准入依据",
                                ({
                                  new_session: "新会话",
                                  new_user_turn: "已确认的新用户轮次",
                                  history_requires_original_route:
                                    "历史需保留原路由",
                                  session_boundary_unconfirmed:
                                    "边界未确认，保留原路由",
                                }[a.routing.boundary] ??
                                  "未识别，保留原始证据") +
                                  "；同轮续调与重试沿用首次判定",
                              ],
                            ]
                          : []),
                        [
                          "客户端 → 上游协议",
                          `${a.clientProtocol ?? a.protocol} → ${a.protocol}`,
                        ],
                        ["开始", date(a.startedAt)],
                        ["发送至上游", durationLabel(a.sentMs)],
                        [
                          "首个有效输出",
                          `${durationLabel(a.ttftMs)} · ${a.firstOutputKind ? ({ text: "正文", tool_arguments: "工具参数", public_reasoning: "公开推理" }[a.firstOutputKind] ?? a.firstOutputKind) : "类型未记录"}`,
                        ],
                        [
                          "正文首出 / 最后正文",
                          `${durationLabel(a.firstTextMs)} / ${durationLabel(a.lastTextMs)}`,
                        ],
                        [
                          "结束 / 耗时",
                          `${date(a.endedAt)} / ${durationLabel(a.durationMs)}`,
                        ],
                        [
                          "前次结束至本次开始",
                          a.backoffMs === null
                            ? index === 0
                              ? "首次尝试"
                              : "不可测"
                            : `${durationLabel(a.backoffMs)}，可能包含客户端退避及处理`,
                        ],
                        [
                          "HTTP / 异常",
                          `${a.httpStatus ?? "未收到 HTTP 状态"}${a.issue ? ` · ${requestIssues[a.issue] ?? a.issue}` : ""}`,
                        ],
                        [
                          "输入 / 输出",
                          `${number(a.usage.inputTokens)} / ${number(a.usage.outputTokens)} Token`,
                        ],
                        [
                          "缓存读取 / 写入",
                          `${number(a.usage.cachedInputTokens)} / ${number(a.usage.cacheWriteInputTokens)} Token`,
                        ],
                        [
                          "费用",
                          a.cost.amount === null
                            ? `未知 · ${costReason(a.cost.reason)}`
                            : `${providerMoney(a.cost.amount, a.cost.currency!, 9)} · ${a.cost.kind === "actual" ? "实际扣费" : "估算"}`,
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
                    {a.timingIssue && (
                      <p className="text-label text-fg-muted">
                        {a.timingIssue}
                      </p>
                    )}
                    {a.cost.components.map((c) => (
                      <p key={c.kind} className="text-label">
                        {{
                          input: "未缓存输入",
                          cached_input: "缓存输入",
                          output: "输出",
                        }[c.kind] ?? c.kind}
                        ：{number(c.tokens)} Token ×{" "}
                        {providerMoney(c.perMillion, a.cost.currency!)} / 百万 ={" "}
                        {providerMoney(c.amount, a.cost.currency!, 9)}
                      </p>
                    ))}
                    {a.cost.price && (
                      <p className="text-label text-fg-muted">
                        价格版本 {a.cost.price.version} · 核实于{" "}
                        {a.cost.price.checkedAt} · 适用期{" "}
                        {date(a.cost.price.validFrom)} 至{" "}
                        {date(a.cost.price.validUntil)}（不含结束）
                      </p>
                    )}
                    {a.cost.source && (
                      <p className="text-label">
                        <a
                          href={a.cost.source}
                          target="_blank"
                          rel="noreferrer"
                          className="underline"
                        >
                          费用来源
                        </a>
                      </p>
                    )}
                    <p className="text-label text-fg-subtle">
                      用量来自上游已报告字段；缓存与推理为子集时不重复相加。未记录的网络阶段保留未知。
                    </p>
                  </div>
                </AccordionContent>
              </AccordionItem>
            ))}
          </Accordion>
          <p className="text-label text-fg-subtle">
            更新于 {date(data.asOf)}
            。仅展示诊断元数据，不额外保存请求正文、回复或认证头。下钻不会重发模型请求。
          </p>
          <ObjectContributionSlots
            object={{
              target: "request",
              id: r.id,
              unit: r.native ? "model-response" : "logical-request",
              origin: { kind: "builtin" },
            }}
          />
        </>
      )}
    </section>
  );
}
