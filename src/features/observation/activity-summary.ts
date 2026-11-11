import { recordAgentKey, type AgentProduct } from "../../shared/types/agent.ts";
import {
  liveMeasurements,
  gatewaySamplingLabel,
  nativeMeasurements,
  nativeRunningPhases,
  nativeSamplingLabel,
} from "./activity-group.ts";
import {
  messageFirstRequest,
  messageGroups,
  nativeMessageTime,
} from "./message-groups.ts";
import type { GatewayActivity, NativeActivity, RateSample } from "./types";
import { messagePerformance, recentRate } from "./activity-performance.ts";

interface ModelActivity {
  id: string;
  name: string;
  sessions: Set<string>;
  unlinked: number;
  samples: number;
  sources: number;
  rate: number | null;
  recentSample: RateSample | null;
  pendingLabels: Set<string>;
}

/** Session totals and simultaneous text rates have different deduplication keys. */
export function summarizeActivity(
  gateways: GatewayActivity[],
  natives: NativeActivity[],
  filter: string,
  products: readonly AgentProduct[],
  now: number,
  reportedSessionIds: string[] = [],
) {
  const matches = (record: {
    agentId: string;
    agentProductId?: string | null;
  }) => {
    const id = recordAgentKey(record, products);
    return (
      filter === "all" ||
      (filter === "unknown" ? id.endsWith("-unknown") : id === filter)
    );
  };
  const requests = [
    ...new Map(
      gateways.filter((a) => matches(a.request)).map((a) => [a.request.id, a]),
    ).values(),
  ].filter((a) => a.request.status === "in_progress" && !a.stale);
  // A later message or terminal native event supersedes an older running row.
  const latest = new Map<string, NativeActivity>();
  const order = (a: NativeActivity) =>
    Date.parse(nativeMessageTime(a) ?? a.eventAt) || 0;
  for (const a of natives.filter(matches)) {
    const previous = latest.get(a.sessionId);
    if (
      !previous ||
      order(a) > order(previous) ||
      (order(a) === order(previous) &&
        Date.parse(a.eventAt) >= Date.parse(previous.eventAt))
    )
      latest.set(a.sessionId, a);
  }
  const activeNative = [...latest.values()].filter((a) => {
    const age = now - Date.parse(a.eventAt);
    return (
      !a.stale &&
      age >= 0 &&
      age <= 30000 &&
      nativeRunningPhases.includes(a.phase)
    );
  });
  const sessions = new Set(activeNative.map((a) => a.sessionId));
  const linked = new Set(
    requests.flatMap((a) => (a.request.sessionId ? [a.request.sessionId] : [])),
  );
  for (const session of linked) sessions.add(session);
  const reportedSessions = new Set(reportedSessionIds);
  for (const session of reportedSessions) sessions.add(session);
  const models = new Map<string, ModelActivity>();
  const windowMs =
    requests.some((a) => a.sampling?.windowMs === 5000) ||
    activeNative.some((a) => a.realtime?.sampling?.windowMs === 5000)
      ? 5000
      : 1000;
  const recentByRequest = new Map<string, RateSample>();
  const windowAt = Math.max(
    0,
    ...[
      ...requests.map((a) => a.sampling?.at),
      ...activeNative.map((a) => a.realtime?.sampling?.at),
    ].filter(
      (at): at is number =>
        at != null &&
        Number.isSafeInteger(at) &&
        now - at >= -1000 &&
        now - at <= 5000,
    ),
  );
  const ttfts: number[] = [];
  const ttftPending = new Set<string>();
  const firstTtfts = new Map<string, number>();
  for (const message of messageGroups(
    gateways.filter((a) => matches(a.request)),
    natives.filter(matches),
  )) {
    for (const a of message.gateways) {
      const sample = messagePerformance(
        message.gateways.filter((b) => b.request.model === a.request.model),
        [],
        message.startedAt,
        now,
      ).recent;
      if (sample) recentByRequest.set(a.request.id, sample);
    }
    const first = messageFirstRequest(message.gateways);
    const ttft = first?.request.ttftMs;
    if (ttft != null && Number.isFinite(ttft) && ttft >= 0)
      for (const a of message.gateways) firstTtfts.set(a.request.id, ttft);
  }
  const modelFor = (name: string | null) => {
    const normalized = name?.trim() || null;
    const id = normalized ? `model:${normalized}` : "unknown";
    let model = models.get(id);
    if (!model) {
      model = {
        id,
        name: normalized ?? "模型待确认",
        sessions: new Set(),
        unlinked: 0,
        samples: 0,
        sources: 0,
        rate: null,
        recentSample: null,
        pendingLabels: new Set(),
      };
      models.set(id, model);
    }
    return model;
  };
  for (const a of requests) {
    const model = modelFor(a.request.model);
    if (a.request.sessionId) model.sessions.add(a.request.sessionId);
    else model.unlinked++;
    model.sources++;
    const measurements = liveMeasurements(a, now);
    const rate =
      measurements.windowMs === windowMs &&
      (windowMs !== 5000 || Math.abs((a.sampling?.at ?? 0) - windowAt) <= 1000)
        ? measurements.speed
        : null;
    const sample = recentByRequest.get(a.request.id);
    if (sample && (!model.recentSample || sample.at > model.recentSample.at))
      model.recentSample = sample;
    const ttft = firstTtfts.get(a.request.id) ?? liveMeasurements(a).ttft;
    if (ttft !== null) ttfts.push(ttft);
    else ttftPending.add(a.request.timingIssue ? "首出未记录" : "等待首 Token");
    if (rate === null) model.pendingLabels.add(gatewaySamplingLabel(a));
    if (rate !== null) {
      model.samples++;
      model.rate = (model.rate ?? 0) + rate;
    }
  }
  for (const a of activeNative) {
    // An exact Session link prevents counting the native and gateway twice.
    if (linked.has(a.sessionId)) continue;
    const model = modelFor(a.model);
    model.sessions.add(a.sessionId);
    model.sources++;
    const measurements = nativeMeasurements(a, now);
    const rate =
      measurements.windowMs === windowMs &&
      (windowMs !== 5000 ||
        Math.abs((a.realtime?.sampling?.at ?? 0) - windowAt) <= 1000)
        ? measurements.speed
        : null;
    const sample = recentRate(
      a.realtime?.sampling?.recent,
      a.messageStartedAt,
      now,
    );
    if (sample && (!model.recentSample || sample.at > model.recentSample.at))
      model.recentSample = sample;
    const ttft = nativeMeasurements(a).ttft;
    if (ttft !== null) ttfts.push(ttft);
    else
      ttftPending.add(
        a.realtime?.firstTokenMissed ? "待原生补报" : "等待首 Token",
      );
    const pending = nativeSamplingLabel(a);
    if (pending) model.pendingLabels.add(pending);
    if (rate !== null) {
      model.samples++;
      model.rate = (model.rate ?? 0) + rate;
    }
  }
  const rows = [...models.values()]
    .sort((a, b) => a.id.localeCompare(b.id))
    .map((model) => ({
      ...model,
      sessions: model.sessions.size,
      pendingLabel:
        model.pendingLabels.size === 1
          ? [...model.pendingLabels][0]
          : model.pendingLabels.size
            ? "执行中"
            : null,
      // A finite input must not turn an overflowing total into a visible Infinity.
      rate:
        model.rate !== null && Number.isFinite(model.rate) ? model.rate : null,
      partial: model.samples < model.sources,
    }));
  const samples = rows.reduce((total, model) => total + model.samples, 0);
  const sources = rows.reduce((total, model) => total + model.sources, 0);
  const measured = rows.filter((model) => model.rate !== null);
  const sum = measured.reduce((total, model) => total + (model.rate ?? 0), 0);
  const pendingLabels = new Set(
    rows.flatMap((model) => (model.pendingLabel ? [model.pendingLabel] : [])),
  );
  return {
    windowMs,
    recentSample:
      rows
        .map((model) => model.recentSample)
        .filter((sample): sample is RateSample => sample !== null)
        .sort((a, b) => b.at - a.at)[0] ?? null,
    sessions: sessions.size,
    reportedSessions: reportedSessions.size,
    unlinked: requests.filter((a) => !a.request.sessionId).length,
    models: rows,
    rate:
      sources === 0 ? 0 : measured.length && Number.isFinite(sum) ? sum : null,
    partial: samples < sources || measured.length !== rows.length,
    samples,
    sources,
    ttft: ttfts.length
      ? ttfts.reduce((total, value) => total + value / ttfts.length, 0)
      : null,
    ttftSamples: ttfts.length,
    ttftPendingLabel:
      ttftPending.size === 1
        ? [...ttftPending][0]
        : ttftPending.size
          ? "部分首出待记录"
          : null,
    pendingLabel:
      pendingLabels.size === 1
        ? [...pendingLabels][0]
        : pendingLabels.size
          ? "执行中"
          : null,
  };
}
export type ActivitySummaryData = ReturnType<typeof summarizeActivity>;
