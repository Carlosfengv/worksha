import type {
  ActivitySampling,
  GatewayActivity,
  NativeActivity,
  MessageUsage,
  RateSample,
} from "./types";

export const rateWindowMs = 5000;
const number = (n: number | null | undefined): n is number =>
  n != null && Number.isFinite(n) && n >= 0;

export function samplingRate(
  sampling: ActivitySampling,
  now = Date.now(),
  startedAt?: string | null,
) {
  const recent = recentRate(sampling.recent, startedAt, now);
  return sampling.windowMs === rateWindowMs &&
    number(sampling.at) &&
    now - sampling.at >= -1000 &&
    now - sampling.at <= 5000 &&
    number(sampling.tokensPerSecond) &&
    sampling.tokensPerSecond > 0 &&
    recent !== null &&
    sampling.at >= recent.at &&
    sampling.at - recent.at < rateWindowMs
    ? sampling.tokensPerSecond
    : null;
}

export function recentRate(
  sample: RateSample | null | undefined,
  startedAt?: string | null,
  now = Date.now(),
) {
  const start = startedAt ? Date.parse(startedAt) : NaN;
  return sample &&
    sample.windowMs === rateWindowMs &&
    number(sample.tokensPerSecond) &&
    sample.tokensPerSecond > 0 &&
    number(sample.at) &&
    sample.at > 0 &&
    sample.at <= now + 1000 &&
    (!Number.isFinite(start) || sample.at >= start)
    ? sample
    : null;
}

export function messagePerformance(
  gateways: GatewayActivity[],
  natives: NativeActivity[],
  startedAt?: string | null,
  now = Date.now(),
) {
  const requests = [
    ...new Map(gateways.map((a) => [a.request.id, a])).values(),
  ];
  const recent =
    [
      ...requests.map((a) =>
        recentRate(
          a.sampling?.recent ?? a.request.lastRateSample,
          a.request.startedAt,
          now,
        ),
      ),
      ...natives.map((a) =>
        recentRate(a.realtime?.sampling?.recent, a.messageStartedAt, now),
      ),
    ]
      .filter(
        (a): a is RateSample =>
          a !== null && (!startedAt || a.at >= Date.parse(startedAt)),
      )
      .sort((a, b) => b.at - a.at)[0] ?? null;
  // Exact message groups choose one source family; native and gateway totals
  // for a linked message must never be added together.
  const nativeUsage = natives
    .map((a) => a.messageUsage ?? a.realtime?.sampling?.reported)
    .filter(
      (a): a is MessageUsage =>
        a != null &&
        Number.isSafeInteger(a.outputTokens) &&
        a.outputTokens >= 0 &&
        (a.reasoningTokens == null ||
          (Number.isSafeInteger(a.reasoningTokens) &&
            a.reasoningTokens >= 0 &&
            a.reasoningTokens <= a.outputTokens)) &&
        Number.isFinite(Date.parse(a.at)) &&
        Date.parse(a.at) <= now + 1000 &&
        (!startedAt || Date.parse(a.at) >= Date.parse(startedAt)),
    )
    .sort((a, b) => Date.parse(b.at) - Date.parse(a.at))[0];
  const reported = requests.filter((a) =>
    number(a.request.usage?.outputTokens),
  );
  const reasoning = reported.filter((a) =>
    number(a.request.usage?.reasoningOutputTokens),
  );
  const estimated = requests
    .map((a) => a.request.estimatedTextTokens)
    .filter(number);
  const body = requests.filter(
    (a) =>
      !a.request.timingIssue &&
      number(a.request.estimatedTextTokens) &&
      number(a.request.firstTextMs) &&
      number(a.request.lastTextMs) &&
      a.request.lastTextMs! - a.request.firstTextMs! >= 100,
  );
  const bodyMs = body.reduce(
    (sum, a) => sum + a.request.lastTextMs! - a.request.firstTextMs!,
    0,
  );
  const nativeAverage = natives
    .map((a) => a.realtime?.averageTextTokensPerSecond)
    .find((n) => number(n) && n > 0);
  return {
    recent,
    averageTextRate:
      bodyMs > 0
        ? (body.reduce((sum, a) => sum + a.request.estimatedTextTokens!, 0) *
            1000) /
          bodyMs
        : requests.length
          ? null
          : (nativeAverage ?? null),
    outputTokens: reported.length
      ? reported.reduce((n, a) => n + (a.request.usage?.outputTokens ?? 0), 0)
      : (nativeUsage?.outputTokens ??
        (estimated.length ? estimated.reduce((a, b) => a + b, 0) : null)),
    reasoningTokens: reported.length
      ? reasoning.length
        ? reasoning.reduce(
            (n, a) => n + (a.request.usage?.reasoningOutputTokens ?? 0),
            0,
          )
        : null
      : (nativeUsage?.reasoningTokens ?? null),
    estimated:
      reported.length === 0 && nativeUsage == null && estimated.length > 0,
    partial: reported.length > 0 && reported.length < requests.length,
  };
}

export function elapsedLabel(
  startedAt: string | null | undefined,
  endedAt: string | null | undefined,
  now = Date.now(),
) {
  const start = Date.parse(startedAt ?? "");
  const end = endedAt ? Date.parse(endedAt) : now;
  return durationLabel(end - start);
}

export function durationLabel(milliseconds: number) {
  if (!Number.isFinite(milliseconds) || milliseconds < 0) return null;
  const seconds = Math.floor(milliseconds / 1000);
  if (seconds < 60) return `${seconds} 秒`;
  if (seconds < 3600)
    return `${Math.floor(seconds / 60)} 分 ${seconds % 60} 秒`;
  return `${Math.floor(seconds / 3600)} 时 ${Math.floor((seconds % 3600) / 60)} 分`;
}
