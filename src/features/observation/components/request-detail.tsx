import { ObjectContributionSlots } from "@/features/plugins/object-detail-slots";
import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { useBackendReadEpoch } from "@/shared/hooks/use-backend-read-epoch";
import { backendReadEpoch } from "@/shared/lib/backend-recovery";
import { useEffect, useState, type Ref } from "react";
import { Button } from "@/components/ui/button";
import { PageActions, PageHeader } from "@/components/ui/page-layout";
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
  requestAgentName,
  requestStates,
  requestIssues,
} from "../request-display";
import type { RequestDetailResponse } from "../request-types";
import { RequestRetryWaterfall } from "./request-retry-waterfall";

const fallbackReasons: Record<string, string> = {
  upstream_failure: "首选来源临时故障，已回退",
  upstream_credential_rejected: "上游拒绝首选账号的凭据，已回退",
  upstream_model_access_denied: "上游拒绝首选账号访问模型，已回退",
  upstream_rate_limit: "首选来源限流，已切换",
  account_quota_exhausted: "候选账号预算已用尽，已切换",
  configured_rate_limit: "首选来源达到配置上限",
  rate_limit_cooldown: "首选来源限流冷却中",
  failure_cooldown: "首选来源故障冷却中",
  preferred_unavailable: "首选条目已停用",
  child_service_unavailable: "子服务没有可用路由，已切换",
  policy_order: "按外部策略排序",
  recovery_probe_in_progress: "首选来源正在恢复",
};

const decisionReasons: Record<string, string> = {
  "disabled-model": "模型已停用",
  "routing-disabled": "子服务只使用默认模型",
  "capability-mismatch": "模型能力与本次请求不匹配",
  "cross-model-fallback-disabled": "跨模型回退未开启",
  "strict-affinity": "受会话固定规则限制",
  "confirmed-turn-required": "等待可识别的新轮次",
  "policy-excluded": "外部策略排除此候选",
  "policy-unavailable": "外部策略不可用或授权已变化",
  "child-unavailable": "子服务没有可用路由",
  "account-unavailable": "账号不可用或版本已变化",
  "account-quota-exhausted": "已知额度已用尽",
  "access-credential-rejected": "上游拒绝此账号凭据，冷却中",
  "access-model-denied": "上游拒绝此账号访问模型，冷却中",
  "duplicate-target": "同轮已处理此上游目标",
  "rate-limit-cooldown": "上游限流冷却中",
  "failure-cooldown": "上游故障冷却中",
  "recovery-in-progress": "已有恢复探测在进行",
  "configured-rate-limit": "达到配置的并发或请求频率上限",
  "attempt-limit": "达到本轮尝试上限，未尝试",
  deadline: "达到本轮等待时限，未尝试",
  "fallback-stopped": "本轮路由已停止，未尝试",
  "earlier-candidate-accepted": "已有候选获选，未尝试",
  "admission-failed": "发送前检查未通过",
  "outcome-unknown": "已有请求结果待确认，未尝试",
  "upstream-outcome-unconfirmed": "已尝试发送，上游结果未确认",
};

export function RequestDetail({
  id,
  returnSearch,
  titleRef,
}: {
  id: string;
  returnSearch: string;
  titleRef: Ref<HTMLHeadingElement>;
}) {
  const { agentNames } = useAgentDirectory();
  const epoch = useBackendReadEpoch();
  const [state, setState] = useState<{
    id: string;
    epoch: symbol;
    data: RequestDetailResponse | null;
    error: string | null;
  }>({ id, epoch, data: null, error: null });
  useEffect(() => {
    let alive = true,
      running = false;
    const load = async () => {
      if (running) return;
      running = true;
      try {
        const data = await observationApi.requestDetail(id);
        if (alive && epoch === backendReadEpoch())
          setState({ id, epoch, data, error: null });
      } catch (reason) {
        if (alive && epoch === backendReadEpoch())
          setState((old) => ({
            id,
            epoch,
            data: old.id === id && old.epoch === epoch ? old.data : null,
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
  const current = state.id === id && state.epoch === epoch;
  const data = current ? state.data : null,
    r = data?.request;
  const error = current ? state.error : null;
  const returnTo = `/requests/${encodeURIComponent(id)}${returnSearch ? `?${returnSearch}` : ""}`;
  const result = r && (requestStates[r.status] ?? requestStates.unknown);
  const summary: [string, string][] = r
    ? [
        [
          "结果",
          r.native
            ? `${result} · 原生标记 ${({ recorded: "已记录响应", unknown: "未知", error: "失败", failed: "失败", cancelled: "已取消" } as Record<string, string>)[r.native.reportedStatus] ?? "未知"}`
            : `${result} · ${r.attempts.length} 次尝试`,
        ],
        [r.native ? "上报模型" : "首次目标模型", r.model ?? "未报告"],
        ...(!r.native && r.attempts.length > 1
          ? ([
              ["最终路由模型", r.attempts.at(-1)?.routedModel ?? "未记录"],
              ["最终 Provider", r.attempts.at(-1)?.provider ?? "未记录"],
            ] as [string, string][])
          : []),
        ["Agent", requestAgentName(r, agentNames)],
        [r.native ? "原生记录 ID" : "请求 ID", r.id],
        ["观察 TTFT", durationLabel(r.ttftMs)],
        [
          "总耗时",
          ["waiting", "streaming"].includes(r.status)
            ? "计时中"
            : r.durationMs == null
              ? "未知"
              : durationLabel(r.durationMs),
        ],
        [
          "输入 / 输出 Token",
          `${number(r.usage.inputTokens)} / ${number(r.usage.outputTokens)}`,
        ],
        ["费用", providerCosts(r.cost)],
      ]
    : [];
  const fields: [string, string][] = r
    ? [
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
        [
          "端到端输出吞吐",
          r.throughput === null
            ? "不可测"
            : `${r.throughput.toFixed(1)} Token/s`,
        ],
        [
          "缓存读取 / 写入",
          `${number(r.usage.cachedInputTokens)} / ${number(r.usage.cacheWriteInputTokens)}`,
        ],
        ["推理输出子集", number(r.usage.reasoningOutputTokens)],
      ]
    : [];
  return (
    <section className="space-y-4" aria-label="请求详情">
      <PageHeader className="px-0">
        <h2 ref={titleRef} tabIndex={-1} className="text-title font-medium">
          请求详情
        </h2>
        <PageActions>
          <Button
            variant="tertiary"
            onClick={() => {
              window.location.hash = `/requests${returnSearch ? `?${returnSearch}` : ""}`;
            }}
          >
            返回请求列表
          </Button>
        </PageActions>
      </PageHeader>
      {error && (
        <p role="alert">
          {data ? "更新失败，保留上次快照：" : "请求详情暂不可读取："}
          {error}
        </p>
      )}
      {data?.sourceError && <p role="alert">{data.sourceError}</p>}
      {!data ? (
        !error && <p role="status">正在读取请求详情…</p>
      ) : !r ? (
        <p>找不到此请求，可能已到期或未被记录。</p>
      ) : (
        <>
          {!r.native && r.attempts.length > 0 && (
            <RequestRetryWaterfall request={r} returnTo={returnTo} />
          )}
          {!!data.routeTrace?.decisions.length && (
            <section
              aria-label="候选路由决策"
              className="space-y-3 rounded-xl border border-border p-4"
            >
              <h3 className="text-title font-medium">候选路由决策</h3>
              <p className="text-label text-fg-muted">
                {r.attempts.length
                  ? `本轮发送 ${r.attempts.length} 次`
                  : "本轮未向候选发送请求"}
                {data.routeTrace.httpStatus
                  ? ` · 网关返回 HTTP ${data.routeTrace.httpStatus}`
                  : ""}
              </p>
              <ol className="list-inside list-decimal space-y-2">
                {data.routeTrace.decisions.map((decision, index) => (
                  <li key={index} className="break-all text-body">
                    {decision.path
                      .map(
                        (step) =>
                          `${step.service} / ${step.model} · 版本 ${step.versionId.slice(0, 8)}${step.modelId ? ` · 条目 ${step.modelId.slice(0, 8)}` : ""}`,
                      )
                      .join(" → ")}
                    <p className="text-label text-fg-muted">
                      {decision.accountPosition
                        ? `账号池第 ${decision.accountPosition} 位 · `
                        : ""}
                      {decisionReasons[decision.reason] ?? "原因未知"}
                    </p>
                  </li>
                ))}
              </ol>
              {data.routeTrace.truncated && (
                <p className="text-label text-fg-muted">
                  记录达到容量上限，部分候选决策未展示。
                </p>
              )}
            </section>
          )}
          <div className="rounded-xl border border-border p-4">
            <DetailList>
              {summary.map(([label, value]) => (
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
                      returnTo,
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
                    r.agentProductId && recordAgentKey(r) !== "opencode-unknown"
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
            <Accordion type="multiple" className="mt-3">
              <AccordionItem value="metadata">
                <AccordionTrigger>请求标识与采集字段</AccordionTrigger>
                <AccordionContent>
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
                </AccordionContent>
              </AccordionItem>
            </Accordion>
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
          <Accordion className="w-full" type="multiple">
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
                      {requestAgentName(a, agentNames)} → 本地网关 →{" "}
                      {a.provider}
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
                                      a.routing.service.nestedRoute?.at(-1)
                                        ?.modelName ??
                                        a.routing.service.modelName,
                                    ],
                                    ...(a.routing.service.nestedRoute?.length
                                      ? [
                                          [
                                            "子服务路径",
                                            a.routing.service.nestedRoute
                                              .map(
                                                (hop) =>
                                                  `${hop.serviceName} / ${hop.modelName}${hop.routingRule ? `（${hop.routingRule.name}）` : ""}`,
                                              )
                                              .join(" → "),
                                          ],
                                        ]
                                      : []),
                                    ...(a.routing.service.routingRule
                                      ? [
                                          [
                                            "匹配规则",
                                            a.routing.service.routingRule.name,
                                          ],
                                        ]
                                      : []),
                                    ...(a.routing.service.routingIntent
                                      ? [
                                          [
                                            "意图分类",
                                            a.routing.service.routingIntent ===
                                            "unknown"
                                              ? "未识别到适用意图"
                                              : a.routing.service
                                                    .routingIntent ===
                                                  "unavailable"
                                                ? "分类暂不可用，继续按其他条件路由"
                                                : a.routing.service
                                                    .routingIntent.matched,
                                          ],
                                        ]
                                      : []),
                                    ...(a.routing.service.upstreamAccountLabel
                                      ? [
                                          [
                                            "上游账号",
                                            a.routing.service
                                              .upstreamAccountLabel,
                                          ],
                                        ]
                                      : []),
                                    ...(a.routing.service.accountSelectionReason
                                      ? [
                                          [
                                            "账号选择",
                                            {
                                              round_robin_accounts: "依次轮换",
                                              weighted_accounts: "按权重分配",
                                              account_affinity:
                                                "优先沿用同会话账号",
                                              strict_affinity: "固定同会话账号",
                                              quota_reset: "按额度重置时间排序",
                                              quota_remaining:
                                                "按剩余额度比例排序",
                                              quota_pace:
                                                "按每小时剩余额度比例排序",
                                              quota_unknown:
                                                "缺少可排序的额度信息，按配置顺序尝试",
                                            }[
                                              a.routing.service
                                                .accountSelectionReason
                                            ] ?? "按账号池策略分配",
                                          ],
                                        ]
                                      : []),
                                    [
                                      "路由尝试",
                                      `${a.routing.service.attempt ?? 1} · ${a.routing.service.fallbackReason ? (fallbackReasons[a.routing.service.fallbackReason] ?? "按服务策略回退") : "首选或原轮次目标"}`,
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
                          `${date(a.endedAt)} / ${a.durationMs == null ? "未知" : durationLabel(a.durationMs)}`,
                        ],
                        [
                          "前次结束至本次开始",
                          a.backoffMs === null
                            ? index === 0
                              ? "首次尝试"
                              : "未知"
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
