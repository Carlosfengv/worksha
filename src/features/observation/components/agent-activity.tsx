import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { useId, useRef, useState, type KeyboardEvent } from "react";
import type { Snapshot } from "@/app/types";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { AgentConnectionStatus } from "@/features/agents/components/agent-connection-status";
import { connectionView } from "@/features/agents/connection-display";
import {
  clientAttributionLabel,
  date,
  sessionLink,
} from "@/features/sessions/display";
import { type AgentDiscovery } from "@/shared/types/agent";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import {
  activityGroup,
  liveMeasurements,
  nativeMeasurements,
  nativeRunningPhases,
  type ActivityGroup,
} from "../activity-group";
import { activityMetrics, phaseNames, seconds, speed } from "../display";
import type { Overview, SessionContextSample } from "../types";
import { ActivityContext } from "./activity-context";
import { ActivityTime } from "./activity-time";
import { messageFirstRequest, messageGroups } from "../message-groups";
import { elapsedLabel, messagePerformance } from "../activity-performance";

const recordedNumber = (value: number | null | undefined) =>
  value != null && Number.isFinite(value) && value >= 0 ? value : null;
const activityTime = (value: string) => {
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) ? parsed : 0;
};
function gatewayEventTime(activity: ActivityGroup["gateway"][number]) {
  const { request } = activity;
  if (request.endedAt) return request.endedAt;
  const lastText = recordedNumber(request.lastTextMs);
  const timestamp = Date.parse(request.startedAt) + (lastText ?? 0);
  return Number.isFinite(timestamp) && Math.abs(timestamp) <= 8.64e15
    ? new Date(timestamp).toISOString()
    : request.startedAt;
}

function messageTime(activity: ActivityGroup["native"][number] | undefined) {
  const time =
    activity?.messageStartedAt === undefined
      ? activity?.userInput?.at
      : activity.messageStartedAt;
  return time && Number.isFinite(Date.parse(time)) ? time : null;
}

function requestActivityRows(
  group: ActivityGroup,
  scope: "active" | "recent" = "active",
) {
  const recent = scope === "recent";
  const sessions = new Map(group.native.map((a) => [a.sessionId, a]));
  const activeGateway = new Set(group.activeGateway.map((a) => a.request.id));
  const activeNative = new Set(group.activeNative.map((a) => a.sessionId));
  const gateways = recent ? group.gateway : group.activeGateway;
  const latestRequest = new Map<string, (typeof gateways)[number]>();
  for (const activity of gateways) {
    const session = activity.request.sessionId;
    if (!session) continue;
    const previous = latestRequest.get(session);
    if (
      !previous ||
      activityTime(gatewayEventTime(activity)) >
        activityTime(gatewayEventTime(previous))
    )
      latestRequest.set(session, activity);
  }
  const activeSessions = new Set(
    group.activeGateway.map((a) => a.request.sessionId).filter(Boolean),
  );
  const replacedRequests = new Set<string>();
  const recentRequests = new Map<string, (typeof gateways)[number]>();
  const natives = recent
    ? group.native.filter((a) => {
        if (activeSessions.has(a.sessionId)) return false;
        const latest = latestRequest.get(a.sessionId);
        if (!latest) return true;
        // Keep one latest state for an explicitly linked Session. Older
        // requests remain independent history entries with their own IDs.
        // A tool step can follow a completed model request. Keep that request
        // as history rather than assigning its measurements to the live step.
        if (activeNative.has(a.sessionId)) return true;
        if (activityTime(a.eventAt) > activityTime(gatewayEventTime(latest))) {
          replacedRequests.add(latest.request.id);
          // Even an unfinished, expired request can have a measured TTFT.
          // Preserve its identity and timing when the native state takes over.
          recentRequests.set(a.sessionId, latest);
          return true;
        }
        return false;
      })
    : group.activeNative;
  return [
    ...gateways
      .filter((a) => !replacedRequests.has(a.request.id))
      .map((a) => {
        const native = a.request.sessionId
          ? sessions.get(a.request.sessionId)
          : undefined;
        const active = activeGateway.has(a.request.id);
        const ended = a.request.status !== "in_progress";
        const average = recordedNumber(a.averageTextTokensPerSecond);
        const throughput = recordedNumber(a.throughput);
        const linkedInput =
          native?.userInput &&
          Date.parse(native.userInput.at) <= Date.parse(a.request.startedAt)
            ? native.userInput
            : null;
        const nativeMessage = messageTime(native);
        const messageAt =
          nativeMessage &&
          activityTime(nativeMessage) <= activityTime(a.request.startedAt)
            ? nativeMessage
            : a.request.startedAt;
        return {
          agentId: a.request.agentId,
          agentProductId: a.request.agentProductId,
          id: `gateway:${a.request.id}`,
          sessionId: a.request.sessionId,
          href: a.request.sessionId
            ? sessionLink(a.request.sessionId)
            : `/requests/${encodeURIComponent(a.request.id)}`,
          destination: a.request.sessionId ? "查看会话" : "查看请求",
          title: native?.title ?? `模型请求 · ${a.request.id.slice(0, 8)}`,
          project: native?.project,
          model: a.request.model,
          provider: a.request.provider,
          phase: recent && ended ? a.request.status : a.phase,
          active,
          expired: !ended && !active,
          step: a.phase === "public_reasoning" ? "已观测到思考进度" : null,
          userInput: a.userInput ?? linkedInput,
          toolNames: a.toolNames ?? [],
          messageAt,
          time: recent ? gatewayEventTime(a) : a.request.startedAt,
          requestAt: a.request.startedAt,
          speed:
            recent && ended
              ? (average ?? throughput)
              : active
                ? liveMeasurements(a).speed
                : null,
          ttft: recordedNumber(a.request.ttftMs),
          speedKind:
            recent && ended
              ? average != null
                ? "average"
                : "throughput"
              : "live",
          outputRate: a.outputTokensPerSecond !== undefined,
          ttftSource: "gateway",
          metricRequest: null,
          source: "网关请求",
          issue: a.request.timingIssue,
          evidence: null,
          nativeRealtime: null,
        };
      }),
    ...natives.map((a) => {
      const request = recentRequests.get(a.sessionId);
      const average = recordedNumber(request?.averageTextTokensPerSecond);
      const reportedTtft = recordedNumber(a.turnTtftMs);
      const measured = nativeMeasurements(a);
      const nativeAverage = recordedNumber(
        a.realtime?.averageTextTokensPerSecond,
      );
      const finished = ["waiting_input", "cancelled", "error"].includes(
        a.phase,
      );
      return {
        agentId: a.agentId,
        agentProductId: a.agentProductId,
        id: `native:${a.sessionId}`,
        sessionId: a.sessionId,
        href: sessionLink(a.sessionId),
        destination: "查看会话",
        title: a.title,
        project: a.project,
        model: a.model,
        provider: null,
        phase: a.phase,
        active: activeNative.has(a.sessionId),
        expired:
          nativeRunningPhases.includes(a.phase) &&
          !activeNative.has(a.sessionId),
        step: a.step,
        userInput: a.userInput ?? null,
        toolNames: a.toolNames ?? [],
        messageAt: messageTime(a) ?? request?.request.startedAt ?? null,
        time: a.eventAt,
        requestAt: request?.request.startedAt ?? null,
        speed: a.realtime
          ? finished
            ? nativeAverage
            : measured.speed
          : request?.request.status === "in_progress"
            ? null
            : (average ?? recordedNumber(request?.throughput)),
        ttft:
          measured.ttft ??
          measured.firstText ??
          recordedNumber(request?.request.ttftMs),
        speedKind: a.realtime
          ? finished
            ? "average"
            : "live"
          : request?.request.status === "in_progress"
            ? "live"
            : average != null
              ? "average"
              : "throughput",
        outputRate: a.realtime?.outputTokensPerSecond !== undefined,
        ttftSource:
          reportedTtft != null
            ? "agent"
            : recordedNumber(a.realtime?.firstTokenMs) != null
              ? "native_token"
              : measured.firstText != null
                ? "native_text"
                : "gateway",
        nativeRealtime: a.realtime ?? null,
        metricRequest: request?.request ?? null,
        issue: request?.request.timingIssue ?? null,
        source: group.overlapUnknown ? "原生记录 · 关联待确认" : "原生记录",
        evidence: clientAttributionLabel(a.agentAttribution, a.agentId),
      };
    }),
  ].sort(
    (a, b) =>
      (a.sessionId ?? a.id).localeCompare(b.sessionId ?? b.id) ||
      a.id.localeCompare(b.id),
  );
}
export function activityRows(
  group: ActivityGroup,
  scope: "active" | "recent" = "active",
  now = Date.now(),
) {
  return messageGroups(group.gateway, group.native)
    .flatMap((message) => {
      const activeGateway = message.gateways.filter((a) =>
        group.activeGateway.includes(a),
      );
      const activeNative = message.natives.filter((a) =>
        group.freshNative.includes(a),
      );
      if (
        scope === "active" &&
        activeGateway.length === 0 &&
        activeNative.length === 0
      )
        return [];
      const base = {
        ...group,
        gateway: message.gateways,
        native: message.natives,
        activeGateway,
        activeNative,
      };
      const gateways = requestActivityRows(
        { ...base, native: [], activeNative: [] },
        "recent",
      );
      const natives = requestActivityRows(
        { ...base, gateway: [], activeGateway: [] },
        "recent",
      );
      const latestRequest = [...message.gateways].sort(
        (a, b) =>
          activityTime(b.request.startedAt) -
            activityTime(a.request.startedAt) ||
          b.request.id.localeCompare(a.request.id),
      )[0];
      const currentRequest =
        [...activeGateway].sort(
          (a, b) =>
            activityTime(b.request.startedAt) -
              activityTime(a.request.startedAt) ||
            b.request.id.localeCompare(a.request.id),
        )[0] ?? latestRequest;
      const gateway = currentRequest
        ? gateways.find((a) => a.id === `gateway:${currentRequest.request.id}`)
        : undefined;
      const native = natives.sort(
        (a, b) => activityTime(b.time) - activityTime(a.time),
      )[0];
      const winner =
        native &&
        (activeGateway.length === 0 || native.phase !== "processing") &&
        (!gateway ||
          (activityTime(native.time) > activityTime(gateway.time) &&
            (!message.uncertain ||
              !["waiting_input", "cancelled", "error"].includes(native.phase))))
          ? native
          : gateway;
      if (!winner) return [];
      const linkedNative = message.natives
        .slice()
        .sort((a, b) => activityTime(b.eventAt) - activityTime(a.eventAt))[0];
      const metrics =
        native === winner && currentRequest && !message.uncertain
          ? {
              ttft:
                winner.ttftSource === "agent"
                  ? winner.ttft
                  : recordedNumber(currentRequest.request.ttftMs),
              ttftSource: winner.ttftSource === "agent" ? "agent" : "gateway",
              speed:
                currentRequest.request.status === "in_progress"
                  ? activeGateway.includes(currentRequest)
                    ? liveMeasurements(currentRequest).speed
                    : null
                  : (recordedNumber(
                      currentRequest.averageTextTokensPerSecond,
                    ) ?? recordedNumber(currentRequest.throughput)),
              speedKind:
                currentRequest.request.status === "in_progress"
                  ? "live"
                  : currentRequest.averageTextTokensPerSecond != null
                    ? "average"
                    : "throughput",
              outputRate: currentRequest.outputTokensPerSecond !== undefined,
              metricRequest: currentRequest.request,
              issue: currentRequest.request.timingIssue,
              requestAt: currentRequest.request.startedAt,
            }
          : {};
      const hasTime = message.startedAt != null;
      const requestedTool =
        winner === gateway &&
        currentRequest?.request.status === "success" &&
        (currentRequest.toolNames?.length ?? 0) > 0;
      const userInput =
        native === winner || !message.uncertain
          ? (linkedNative?.userInput ?? winner.userInput)
          : winner.userInput;
      const sessionTitle = linkedNative?.title ?? null;
      const firstRequest = messageFirstRequest(message.gateways);
      const firstTtft = recordedNumber(firstRequest?.request.ttftMs);
      const messageTtftRequest =
        message.gateways.length > 1 &&
        firstTtft != null &&
        (metrics.ttftSource ?? winner.ttftSource) !== "agent"
          ? firstRequest!.request
          : null;
      const performance = messagePerformance(
        message.gateways,
        message.natives,
        message.startedAt,
        now,
      );
      return [
        {
          ...winner,
          ...metrics,
          recentSample: performance.recent,
          statistics: performance,
          rateWindowMs: currentRequest
            ? (currentRequest.sampling?.windowMs ?? 1000)
            : (linkedNative?.realtime?.sampling?.windowMs ?? 1000),
          taskElapsed: elapsedLabel(
            message.startedAt ?? (!message.sessionId ? winner.requestAt : null),
            winner.active ? null : winner.time,
            now,
          ),
          ...(messageTtftRequest
            ? { ttft: firstTtft, ttftSource: "gateway_message" }
            : {}),
          messageTtftRequest,
          ...(requestedTool
            ? {
                phase: "tool_requested",
                step: `已提出调用：${currentRequest.toolNames!.join("、")}`,
              }
            : {}),
          metricRequestActive:
            currentRequest != null && activeGateway.includes(currentRequest),
          id: message.id,
          messageUncertain: message.uncertain,
          messageAt:
            message.startedAt ??
            (!message.sessionId ? (gateway?.requestAt ?? null) : null),
          title: userInput?.text.trim() || sessionTitle || winner.title,
          sessionTitle,
          project: linkedNative?.project ?? winner.project,
          userInput,
          time:
            [...gateways.map((a) => a.time), ...natives.map((a) => a.time)]
              .filter((time): time is string => time != null)
              .sort((a, b) => activityTime(b) - activityTime(a))[0] ??
            winner.time,
          // Missing message evidence remains explicit; it never merges a Session.
          evidence:
            !hasTime && message.uncertain
              ? [winner.evidence, "消息归属待确认"].filter(Boolean).join(" · ")
              : winner.evidence,
        },
      ];
    })
    .sort((a, b) => a.id.localeCompare(b.id));
}
function ActiveActivities({
  group,
  name,
  contexts,
}: {
  group: ActivityGroup;
  name: string;
  contexts: Map<string, SessionContextSample>;
}) {
  const [open, setOpen] = useState(false);
  const triggerId = useId();
  const listRef = useRef<HTMLDivElement>(null);
  const scrollList = (event: KeyboardEvent) => {
    if (
      !open ||
      !["PageDown", "PageUp", "ArrowDown", "ArrowUp"].includes(event.key)
    )
      return;
    event.preventDefault();
    const distance = event.key.startsWith("Page") ? 200 : 40;
    listRef.current?.scrollBy({
      top: event.key.endsWith("Down") ? distance : -distance,
    });
  };
  return (
    <Popover
      trigger="hover"
      open={open}
      triggerId={triggerId}
      onOpenChange={setOpen}
    >
      <PopoverTrigger
        id={triggerId}
        render={<Button variant="ghost" size="sm" />}
        aria-label={`${name}，${group.overlapUnknown ? "至少 " : ""}${group.count} 个活动进行中，查看明细`}
        onKeyDown={scrollList}
      >
        {group.count === 1 && !group.overlapUnknown
          ? (phaseNames[
              group.activeGateway[0]?.phase ??
                group.activeNative[0]?.phase ??
                ""
            ] ?? "活动进行中")
          : `${group.overlapUnknown ? "至少 " : ""}${group.count} 个活动进行中`}
      </PopoverTrigger>
      <PopoverContent
        side="bottom"
        aria-label={`${name}活动明细`}
        onKeyDown={scrollList}
      >
        <div
          ref={listRef}
          className="max-h-80 space-y-3 overflow-y-auto overscroll-contain"
          aria-label={`${name}活动列表`}
        >
          <p>
            {group.overlapUnknown
              ? "原生与网关关联未确认，数量为下限。"
              : "当前活动"}
          </p>
          <ul className="space-y-3">
            {activityRows(group).map((a) => (
              <li key={a.id}>
                <a
                  href={`#${a.href}`}
                  onClick={() => setOpen(false)}
                  className="block rounded-lg p-2 text-fg-muted outline-none hover:bg-hover hover:text-fg-default focus-visible:ring-1 focus-visible:ring-focus-ring"
                >
                  <span className="block space-y-1 break-words">
                    <span className="flex items-start justify-between gap-2">
                      <span className="min-w-0 wrap-anywhere font-semibold text-fg-default">
                        {a.title}
                      </span>
                      <span className="shrink-0 text-fg-subtle">
                        {a.destination}
                      </span>
                    </span>
                    <span className="block">
                      {[a.project, a.provider, a.model]
                        .filter(Boolean)
                        .join(" · ") || "模型 / 项目未记录"}
                    </span>
                    <span className="block" title={a.evidence ?? undefined}>
                      {phaseNames[a.phase] ?? a.phase} · {a.source} ·{" "}
                      {date(a.time)}
                    </span>
                    {a.step && <span className="block">{a.step}</span>}
                    <span
                      className="block line-clamp-2"
                      title={a.userInput?.text}
                    >
                      最近输入：{a.userInput?.text ?? "来源未提供"}
                    </span>
                    {a.toolNames.length > 0 && (
                      <span className="block">
                        工具：{a.toolNames.join("、")}
                      </span>
                    )}
                    {a.issue && <span className="block">{a.issue}</span>}
                    <span className="block">
                      文本速度{" "}
                      {a.speed == null ? "未采集" : `${speed(a.speed)} Token/s`}{" "}
                      · {a.ttftSource === "agent" ? "Agent 上报首出" : "TTFT"}{" "}
                      {a.ttft == null ? "未采集" : `${seconds(a.ttft)} 秒`}
                    </span>
                  </span>
                </a>
                <ActivityContext
                  sessionId={a.sessionId}
                  sample={a.sessionId ? contexts.get(a.sessionId) : undefined}
                  onNavigate={() => setOpen(false)}
                />
              </li>
            ))}
          </ul>
          {group.activeGateway.length > 0 && (
            <p>
              网关 {group.activeGateway.length} 个请求：速度{" "}
              {group.speedSamples} 个样本，TTFT {group.ttftSamples}{" "}
              个样本。各项只对可测值取算术平均，未知不按 0 计。
            </p>
          )}
          {group.activeNative.length > 0 && (
            <p>原生记录未提供实时 Token/s 和请求 TTFT。</p>
          )}
        </div>
      </PopoverContent>
    </Popover>
  );
}
export function UnattributedActivities({
  overview,
  stale,
  filter,
}: {
  overview: Overview | null;
  stale: boolean;
  filter: string;
}) {
  const { agentNames, agentProducts, isSharedSource } = useAgentDirectory();
  if (!["all", "unknown"].includes(filter)) return null;
  const contexts = new Map(
    overview?.sessionContexts?.map((s) => [s.sessionId, s]),
  );
  const groups = [...new Set(agentProducts.map((product) => product.family))]
    .filter(isSharedSource)
    .map((family) => `${family}-unknown`)
    .map((product) => ({
      product,
      group: activityGroup(
        product,
        overview?.gatewayActivities ?? [],
        overview?.nativeActivities ?? [],
        stale,
        Date.now(),
        agentProducts,
      ),
    }))
    .filter(({ group }) => group.count > 0);
  if (!groups.length)
    return filter === "unknown" ? (
      <ConnectionEmpty
        compact
        title="暂无进行中活动"
        description="最近记录与调用统计保留在下方。"
      />
    ) : null;
  return groups.map(({ product, group }) => {
    const name = agentNames[product];
    const sessions = [
      ...new Map(
        activityRows(group).map((a) => [a.sessionId ?? a.id, a]),
      ).values(),
    ];
    return (
      <section
        key={product}
        aria-label={`${name}实时活动`}
        className="space-y-3 rounded-xl border border-border p-4 xl:col-span-full"
      >
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h3 className="text-title font-medium">{name}</h3>
          <ActiveActivities group={group} name={name} contexts={contexts} />
        </div>
        <ul
          aria-label={`${name}会话上下文`}
          className="max-h-60 space-y-2 overflow-y-auto overscroll-contain"
        >
          {sessions.map((a) => (
            <li key={a.sessionId ?? a.id}>
              <p
                className="truncate text-label"
                title={a.evidence ?? undefined}
              >
                {a.title} · {phaseNames[a.phase] ?? a.phase}
              </p>
              {a.step && <p className="text-label text-fg-muted">{a.step}</p>}
              <p
                className="line-clamp-2 text-label text-fg-muted"
                title={a.userInput?.text}
              >
                最近输入：{a.userInput?.text ?? "来源未提供"}
              </p>
              <ActivityContext
                sessionId={a.sessionId}
                sample={a.sessionId ? contexts.get(a.sessionId) : undefined}
              />
            </li>
          ))}
        </ul>
      </section>
    );
  });
}
export function AgentActivity({
  agent,
  data,
  overview,
  stale,
}: {
  agent: AgentDiscovery;
  data: Snapshot;
  overview: Overview | null;
  stale: boolean;
}) {
  const { configAgent } = useAgentDirectory();
  const group = activityGroup(
    agent.id,
    overview?.gatewayActivities ?? [],
    overview?.nativeActivities ?? [],
    stale,
    Date.now(),
    data.agents,
  );
  const rawActive = group.gateway.find(
    (a) => a.request.status === "in_progress",
  );
  const observed = rawActive ?? group.gateway[0];
  const native = group.native[0];
  const hasLive = group.count > 0;
  const rows = activityRows(group);
  const contexts = new Map(
    overview?.sessionContexts?.map((s) => [s.sessionId, s]),
  );
  const sessionRows = [
    ...new Map(rows.map((a) => [a.sessionId ?? a.id, a])).values(),
  ];
  const activityStale =
    stale || Boolean(rawActive?.stale || (!observed && native?.stale));
  const lastMetrics = activityMetrics({
    active: rawActive,
    latest: group.gateway[0],
    native,
    stale: activityStale,
  });
  const plural = group.count > 1;
  const allIssues =
    group.activeGateway.length > 0 &&
    group.activeGateway.every((a) => a.request.timingIssue);
  const metrics = hasLive
    ? {
        speed: {
          label: plural ? "当前平均文本速度" : "当前文本速度",
          value:
            group.speed == null
              ? !group.activeGateway.length
                ? "未采集"
                : allIssues
                  ? "采样恢复中"
                  : "等待文本"
              : speed(group.speed),
          unit: group.speed == null ? "" : "Token/s",
        },
        ttft: {
          label: plural ? "当前请求平均 TTFT" : "当前请求 TTFT",
          value:
            group.ttft == null
              ? !group.activeGateway.length
                ? "未采集"
                : allIssues
                  ? "首出未记录"
                  : "等待首出"
              : seconds(group.ttft),
          unit: group.ttft == null ? "" : "秒",
        },
        note: `当前可测请求的算术平均；按近 ${group.activeGateway.some((a) => a.sampling?.windowMs === 5000) || group.activeNative.some((a) => a.realtime?.sampling?.windowMs === 5000) ? 5 : 1} 秒可观测输出估算。`,
      }
    : lastMetrics;
  // For a single request keep its precise waiting/tool/reasoning placeholder.
  const currentMetrics =
    hasLive && !plural && group.activeGateway.length === 1
      ? activityMetrics({ active: group.activeGateway[0], stale: false })
      : hasLive && !plural && group.activeNative[0]?.realtime
        ? activityMetrics({
            native: group.activeNative[0],
            stale: activityStale,
          })
        : metrics;
  const title = hasLive
    ? [...new Set(rows.map((a) => a.title))].join("、")
    : (native?.title ?? (observed ? "最近模型请求" : "等待新活动"));
  const connection = connectionView({
    agentId: configAgent(agent.id),
    productId: agent.id,
    connections: data.agentConnections,
  });
  const subtitle = hasLive
    ? [
        ...new Set(
          rows.map(
            (a) =>
              [a.provider, a.model].filter(Boolean).join(" · ") ||
              a.project ||
              "模型未知",
          ),
        ),
      ].join("、")
    : connection.fresh && connection.mode !== "unknown"
      ? connection.mode === "gateway"
        ? observed
          ? `最近网关 · ${observed.request.provider} · ${observed.request.model ?? "模型未知"}`
          : "等待模型活动"
        : connection.config?.model
          ? `默认模型 · ${connection.config.model}`
          : "默认模型由 Agent 管理"
      : "默认模型待确认";
  const stage =
    (!native?.stale ? native?.phase : undefined) ??
    observed?.request.status ??
    native?.phase ??
    "unknown";
  const recentInput = hasLive
    ? rows.length === 1
      ? rows[0].userInput
      : null
    : (native?.userInput ?? observed?.userInput);
  const recent = observed?.request.startedAt;
  const sessionId = hasLive
    ? !plural && !group.overlapUnknown
      ? (group.activeGateway[0]?.request.sessionId ??
        group.activeNative[0]?.sessionId)
      : null
    : native?.sessionId;
  return (
    <article
      className="flex min-w-0 flex-col gap-3 rounded-xl border border-border p-4"
      aria-label={`${agent.name}实时活动`}
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-body font-medium">{agent.name}</h3>
        {hasLive ? (
          <ActiveActivities
            group={group}
            name={agent.name}
            contexts={contexts}
          />
        ) : (
          <Badge size="sm" variant="dot" status="neutral">
            {activityStale
              ? "最后快照 · 过期"
              : (phaseNames[stage] ?? "未接收事件")}
          </Badge>
        )}
      </div>
      <div className="min-w-0">
        <AgentConnectionStatus
          agentId={agent.id}
          data={data}
          compact
          live={{
            count: group.activeGateway.length,
            stale: stale || !overview,
          }}
        />
      </div>
      <div className="min-w-0">
        <p className="truncate text-body font-medium" title={title}>
          {title}
        </p>
        {recentInput && (
          <p className="mt-1 line-clamp-2 text-label" title={recentInput.text}>
            <span className="text-fg-muted">最近输入：</span>
            {recentInput.text}
          </p>
        )}
        {subtitle && (
          <p
            className="mt-1 truncate text-label text-fg-muted"
            title={subtitle}
          >
            {subtitle}
          </p>
        )}
      </div>
      <dl className="grid grid-cols-2 gap-3">
        {Object.entries({
          speed: currentMetrics.speed,
          ttft: currentMetrics.ttft,
        }).map(([key, m]) => (
          <div key={key} className="min-w-0">
            <dt className="text-label text-fg-subtle">{m.label}</dt>
            <dd
              className="mt-1 truncate text-title tabular-nums"
              title={`${m.value} ${m.unit}`}
            >
              {m.value}{" "}
              <span className="text-label text-fg-subtle">{m.unit}</span>
            </dd>
          </div>
        ))}
      </dl>
      <div className="min-w-0">
        {hasLive ? (
          <ul
            aria-label={`${agent.name}会话上下文`}
            className="max-h-32 space-y-1 overflow-y-auto overscroll-contain"
          >
            {sessionRows.map((a) => (
              <li key={a.sessionId ?? a.id}>
                <ActivityContext
                  sessionId={a.sessionId}
                  sample={a.sessionId ? contexts.get(a.sessionId) : undefined}
                  label={sessionRows.length > 1 ? a.title : undefined}
                  compact
                />
              </li>
            ))}
          </ul>
        ) : (
          <p className="p-2 text-label text-fg-subtle">
            {activityStale ? "等待活动更新" : "暂无进行中会话"}
          </p>
        )}
      </div>
      <div className="text-label text-fg-subtle">
        {hasLive
          ? group.elapsed != null && (
              <p
                className="text-label text-fg-subtle"
                title={currentMetrics.note}
              >
                {plural ? "请求平均持续" : "已持续"} {seconds(group.elapsed)} 秒
              </p>
            )
          : (recent || native) && (
              <p className="text-label text-fg-subtle">
                {recent ? (
                  <ActivityTime
                    value={recent}
                    kind="request"
                    title={`最近网关请求 · ${observed?.request.provider} · ${observed?.request.model ?? "模型未知"} · ${currentMetrics.note}`}
                  />
                ) : (
                  <ActivityTime value={native?.eventAt} kind="activity" />
                )}
              </p>
            )}
      </div>
      <div className="mt-auto flex flex-wrap gap-2">
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            window.location.hash = sessionId
              ? sessionLink(sessionId)
              : `/agents/${agent.id}`;
          }}
        >
          查看{sessionId ? "会话" : "Agent"}
        </Button>
        <Button
          variant="tertiary"
          size="sm"
          onClick={() => {
            window.location.hash = `/agents/${agent.id}?tab=connection`;
          }}
        >
          连接与观测
        </Button>
      </div>
    </article>
  );
}
