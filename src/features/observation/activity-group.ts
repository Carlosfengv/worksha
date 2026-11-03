import type { GatewayActivity, NativeActivity } from "./types";
import { agentProducts, recordAgentKey } from "../../shared/types/agent.ts";
import { messageFirstRequest, messageGroups } from "./message-groups.ts";
import { samplingRate } from "./activity-performance.ts";

export const nativeRunningPhases = [
  "processing",
  "thinking",
  "tool_requested",
  "tool_returned",
  "streaming",
];
const known = (n: number | null | undefined): n is number =>
  n != null && Number.isFinite(n) && n >= 0;
function mean(values: number[]) {
  // Divide before adding to avoid overflowing a sum of finite samples.
  return values.length
    ? values.reduce((sum, n) => sum + n / values.length, 0)
    : null;
}
export function liveMeasurements(activity: GatewayActivity, now = Date.now()) {
  const usable = !activity.stale;
  return {
    speed:
      usable &&
      ["streaming", "public_reasoning", "tool_arguments"].includes(
        activity.phase,
      )
        ? activity.sampling
          ? samplingRate(activity.sampling, now, activity.request.startedAt)
          : known(activity.outputTokensPerSecond)
            ? activity.outputTokensPerSecond
            : activity.outputTokensPerSecond === undefined &&
                activity.phase === "streaming" &&
                known(activity.textTokensPerSecond)
              ? activity.textTokensPerSecond
              : null
        : null,
    ttft:
      usable && known(activity.request.ttftMs) ? activity.request.ttftMs : null,
    elapsed: known(activity.elapsedMs) ? activity.elapsedMs : null,
    windowMs: activity.sampling?.windowMs ?? 1000,
  };
}
export function gatewaySamplingLabel(activity: Pick<GatewayActivity, "phase">) {
  if (activity.phase === "public_reasoning") return "思考中";
  if (activity.phase === "tool_arguments") return "生成工具参数";
  if (activity.phase === "waiting_first") return "等待模型响应";
  return "等待输出采样";
}
export function nativeMeasurements(activity: NativeActivity, now = Date.now()) {
  const realtime = activity.realtime;
  // An empty native sampling window cannot establish that hidden output is zero.
  return {
    speed:
      !activity.stale &&
      realtime?.state === "connected" &&
      ["streaming", "thinking", "tool_requested", "tool_returned"].includes(
        activity.phase,
      )
        ? realtime.sampling
          ? samplingRate(realtime.sampling, now, activity.messageStartedAt)
          : known(realtime.outputTokensPerSecond) &&
              realtime.outputTokensPerSecond > 0
            ? realtime.outputTokensPerSecond
            : realtime.outputTokensPerSecond === undefined &&
                activity.phase === "streaming" &&
                known(realtime.textTokensPerSecond) &&
                realtime.textTokensPerSecond > 0
              ? realtime.textTokensPerSecond
              : null
        : null,
    ttft: known(activity.turnTtftMs)
      ? activity.turnTtftMs
      : known(realtime?.firstTokenMs)
        ? realtime.firstTokenMs
        : null,
    firstText: known(realtime?.firstTextMs) ? realtime.firstTextMs : null,
    windowMs: realtime?.sampling?.windowMs ?? 1000,
  };
}
export function nativeSamplingLabel(
  activity: Pick<NativeActivity, "phase" | "realtime">,
) {
  const state = activity.realtime?.state;
  if (!state) return null;
  if (state === "incompatible") return "原生版本不兼容";
  if (state === "unavailable") return "原生实时未接入";
  if (state !== "connected") return "原生重连中";
  if (activity.phase === "thinking") return "思考中";
  if (["tool_requested", "tool_returned"].includes(activity.phase))
    return "工具处理中";
  return "原生未上报";
}

/** IDs and exact Session links are the only cross-source deduplication evidence. */
export function activityGroup(
  product: string,
  gateways: GatewayActivity[],
  natives: NativeActivity[],
  stale: boolean,
  now = Date.now(),
  products = agentProducts,
) {
  const gateway = [
    ...new Map(
      gateways
        .filter((a) => recordAgentKey(a.request, products) === product)
        .sort(
          (a, b) =>
            Date.parse(a.request.startedAt) - Date.parse(b.request.startedAt),
        )
        .map((a) => [a.request.id, a]),
    ).values(),
  ].sort(
    (a, b) => Date.parse(b.request.startedAt) - Date.parse(a.request.startedAt),
  );
  const native = [
    ...new Map(
      natives
        .filter((a) => recordAgentKey(a, products) === product)
        .sort((a, b) => Date.parse(a.eventAt) - Date.parse(b.eventAt))
        .map((a) => [
          a.messageId
            ? `${a.sessionId}:${a.messageId}`
            : a.messageStartedAt
              ? `${a.sessionId}:${a.messageStartedAt}`
              : a.sessionId,
          a,
        ]),
    ).values(),
  ].sort((a, b) => Date.parse(b.eventAt) - Date.parse(a.eventAt));
  const activeGateway = stale
    ? []
    : gateway.filter((a) => a.request.status === "in_progress" && !a.stale);
  const freshNative = stale
    ? []
    : native.filter((a) => {
        const age = now - Date.parse(a.eventAt);
        return (
          !a.stale &&
          age >= 0 &&
          age <= 30000 &&
          nativeRunningPhases.includes(a.phase)
        );
      });
  const messages = messageGroups(activeGateway, freshNative);
  const linked = new Set(
    messages.filter((m) => m.gateways.length > 0).flatMap((m) => m.natives),
  );
  const activeNative = freshNative.filter((a) => !linked.has(a));
  const unlinked = activeGateway.filter((a) => !a.request.sessionId).length;
  const overlapUnknown =
    (unlinked > 0 && activeNative.length > 0) ||
    messages.some(
      (m) =>
        m.uncertain &&
        (m.gateways.length > 1 ||
          (m.gateways.length > 0 && m.natives.length > 0)),
    );
  // Each unlinked request can overlap at most one native Session. Do not add
  // both collections and falsely claim a precise activity count.
  const count = messages.length - Math.min(unlinked, activeNative.length);
  const values = activeGateway.map((a) => liveMeasurements(a, now));
  const firstTtfts = new Map<string, number>();
  for (const message of messageGroups(gateway, native)) {
    const first = messageFirstRequest(message.gateways);
    if (first && known(first.request.ttftMs))
      for (const a of message.gateways)
        firstTtfts.set(a.request.id, first.request.ttftMs);
  }
  activeGateway.forEach((a, index) => {
    values[index].ttft = firstTtfts.get(a.request.id) ?? values[index].ttft;
  });
  const nativeValues = activeNative.map((a) => nativeMeasurements(a, now));
  const windowMs = [...values, ...nativeValues].some((v) => v.windowMs === 5000)
    ? 5000
    : 1000;
  const speeds = [...values, ...nativeValues].flatMap((v) =>
    v.speed == null || v.windowMs !== windowMs ? [] : [v.speed],
  );
  const ttfts = [...values, ...nativeValues].flatMap((v) =>
    v.ttft == null ? [] : [v.ttft],
  );
  const elapsed = values.flatMap((v) => (v.elapsed == null ? [] : [v.elapsed]));
  return {
    gateway,
    native,
    activeGateway,
    activeNative,
    freshNative,
    count,
    overlapUnknown,
    speed: mean(speeds),
    speedSamples: speeds.length,
    ttft: mean(ttfts),
    ttftSamples: ttfts.length,
    elapsed: mean(elapsed),
  };
}
export type ActivityGroup = ReturnType<typeof activityGroup>;
