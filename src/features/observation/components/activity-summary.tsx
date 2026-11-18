import { useEffect, useId, useRef, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import { AiBrain03Icon } from "@hugeicons/core-free-icons";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { MetricCardState } from "@/components/ui/metric-card";
import {
  LiveLine,
  LiveLineChart,
  LiveXAxis,
  LiveYAxis,
  type LiveLinePoint,
} from "@/components/ui/live-line-chart";
import type { ActivitySummaryData } from "../activity-summary";
import { ActivityTime } from "./activity-time";
import { seconds, speed } from "../display";
import {
  assignModelColors,
  loadModelColors,
  modelLogo,
  saveModelColors,
} from "../model-appearance";

/** Business-only flow: Sessions and missing text samples are not request/s. */
function ModelActivityFlow({
  models,
  paused,
  colors,
}: {
  models: ActivitySummaryData["models"];
  paused: boolean;
  colors: ReadonlyMap<string, string>;
}) {
  const id = useId();
  const root = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(500);
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(([entry]) => {
      if (entry.contentRect.width > 0) setWidth(entry.contentRect.width);
    });
    if (canvas.current) observer.observe(canvas.current);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    if (typeof IntersectionObserver === "undefined") return;
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    let intersecting = false;
    const update = () =>
      setVisible(intersecting && !document.hidden && !media.matches);
    const observer = new IntersectionObserver(([entry]) => {
      intersecting = entry.isIntersecting;
      update();
    });
    if (root.current) observer.observe(root.current);
    media.addEventListener("change", update);
    document.addEventListener("visibilitychange", update);
    return () => {
      observer.disconnect();
      media.removeEventListener("change", update);
      document.removeEventListener("visibilitychange", update);
    };
  }, []);
  const rowHeight = 48;
  const height = models.length * rowHeight;
  const center = height / 2;
  return (
    <div
      ref={root}
      className="flex max-h-36 min-w-0 items-start gap-3 overflow-y-auto overscroll-contain"
    >
      <div
        ref={canvas}
        className="relative hidden min-w-0 flex-1 sm:block"
        aria-hidden="true"
      >
        <span
          className="absolute left-0 -translate-y-7 text-label text-fg-subtle"
          style={{ top: center }}
        >
          当前任务
        </span>
        <svg
          className="block w-full"
          height={height}
          viewBox={`0 0 ${width} ${height}`}
          preserveAspectRatio="xMinYMid meet"
        >
          {models.map((model, index) => {
            const y = (index + 0.5) * rowHeight;
            const path = `M 8 ${center} H ${width * 0.2} C ${width * 0.34} ${center} ${width * 0.332} ${y} ${width * 0.46} ${y} H ${width - 8}`;
            const color = colors.get(model.id);
            return (
              <g key={model.id} fill={color}>
                <path
                  id={`${id}-${index}`}
                  d={path}
                  stroke={color}
                  strokeWidth="5"
                  strokeOpacity="0.08"
                  fill="none"
                  vectorEffect="non-scaling-stroke"
                />
                <path
                  d={path}
                  stroke={color}
                  strokeWidth="2"
                  strokeOpacity="0.35"
                  fill="none"
                  vectorEffect="non-scaling-stroke"
                />
                {visible &&
                  !paused &&
                  model.rate !== null &&
                  model.rate > 0 &&
                  [0, 1, 2, 3].map((particle) => (
                    <circle key={particle} r="2.5">
                      <animateMotion
                        dur="4s"
                        begin={`${-particle - index * 0.3}s`}
                        repeatCount="indefinite"
                      >
                        <mpath href={`#${id}-${index}`} />
                      </animateMotion>
                    </circle>
                  ))}
                <circle cx={width - 8} cy={y} r="4" />
              </g>
            );
          })}
          <circle
            cx="8"
            cy={center}
            r="4"
            className="fill-surface-floating stroke-fg-subtle"
            strokeWidth="1.5"
            vectorEffect="non-scaling-stroke"
          />
        </svg>
      </div>
      <ul
        aria-label="执行中的模型"
        className="min-w-0 flex-1 sm:w-80 sm:flex-none"
      >
        {models.map((model) => {
          const logo = modelLogo(model.name);
          return (
            <li key={model.id} className="flex h-12 min-w-0 items-center gap-3">
              {logo ? (
                <span
                  aria-hidden="true"
                  className="size-5 shrink-0"
                  style={{
                    backgroundColor: colors.get(model.id),
                    mask: `url("${logo}") center / contain no-repeat`,
                    WebkitMask: `url("${logo}") center / contain no-repeat`,
                  }}
                />
              ) : (
                <HugeiconsIcon
                  icon={AiBrain03Icon}
                  size={20}
                  aria-hidden="true"
                  className="size-5 shrink-0"
                  style={{ color: colors.get(model.id) }}
                />
              )}
              <div className="min-w-0 flex-1">
                <p
                  className="truncate text-body font-medium"
                  title={model.name}
                >
                  {model.name}
                </p>
                <p className="text-label text-fg-muted">
                  {model.sessions} 个 Session
                  {model.unlinked ? ` · ${model.unlinked} 个请求待关联` : ""}
                </p>
              </div>
              <div className="shrink-0 text-right text-label tabular-nums">
                <p>
                  {model.rate === null
                    ? model.recentSample
                      ? `最近 ${speed(model.recentSample.tokensPerSecond)} Token/s`
                      : "— Token/s"
                    : `${speed(model.rate)} Token/s`}
                </p>
                <p className="text-fg-muted">
                  {model.rate === null ? (
                    model.recentSample ? (
                      <>
                        单个来源 ·{" "}
                        <ActivityTime
                          value={new Date(model.recentSample.at).toISOString()}
                          kind="sample"
                        />
                      </>
                    ) : (
                      (model.pendingLabel ?? "暂无实时输出采样")
                    )
                  ) : model.partial ? (
                    "部分可测"
                  ) : (
                    "实时输出估算"
                  )}
                </p>
              </div>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

export function ActivitySummary({
  summary,
  state,
  activeCount,
  at,
  ttftP50,
  ttftP95,
  gatewayReady,
  nativeSourceEnabled,
  nativeObservationPaused,
  reportedOnly,
  sourceHref,
  reportedRates = [],
  reportedTtfts = [],
  reportedSourceNames = [],
}: {
  summary: ActivitySummaryData;
  state: MetricCardState;
  activeCount: number;
  at?: string;
  ttftP50?: number | null;
  ttftP95?: number | null;
  gatewayReady: boolean;
  nativeSourceEnabled: boolean;
  nativeObservationPaused?: boolean;
  reportedOnly?: boolean;
  sourceHref?: string;
  reportedRates?: number[];
  reportedTtfts?: number[];
  reportedSourceNames?: string[];
}) {
  const [paused, setPaused] = useState(false);
  const [points, setPoints] = useState<LiveLinePoint[]>([]);
  const [colors, setColors] =
    useState<ReadonlyMap<string, string>>(loadModelColors);
  const nextColors = assignModelColors(
    colors,
    summary.models.map((model) => model.id),
  );
  if (nextColors !== colors) setColors(nextColors);
  useEffect(() => saveModelColors(colors), [colors]);
  const latest = summary.samples ? summary.rate : null;
  useEffect(() => {
    if (!at || latest == null || state !== "ready" || !activeCount) return;
    const time = Date.parse(at) / 1000;
    if (!Number.isFinite(time)) return;
    setPoints((previous) => [
      ...previous.filter(
        (point) => point.time >= time - 30 && point.time < time,
      ),
      { time, value: latest },
    ]);
  }, [activeCount, at, latest, state]);
  const timing = `成功 TTFT P50 / P95 ${seconds(ttftP50)} / ${seconds(ttftP95)} 秒`;
  const ready = state === "ready";
  const nativeOrActive = nativeSourceEnabled || activeCount > 0;
  const known = ready && nativeOrActive;
  const mean = (values: number[]) =>
    values.length
      ? values.reduce((sum, value) => sum + value / values.length, 0)
      : null;
  const reportedRate = summary.sources === 0 ? mean(reportedRates) : null;
  const reportedTtft = summary.sources === 0 ? mean(reportedTtfts) : null;
  const source =
    reportedSourceNames.length === 1 ? reportedSourceNames[0] : "来源上报";
  const rate =
    reportedRate ??
    (summary.reportedSessions && !summary.sources ? null : summary.rate);
  const rateLabel =
    reportedRate !== null
      ? "上报的当前步骤均速"
      : rate != null && summary.partial
        ? "已测 Token 速率"
        : "总 Token 速率";
  const ttft = summary.ttft ?? reportedTtft;
  const gatewayOnly =
    ready && !nativeSourceEnabled && !reportedOnly && !summary.reportedSessions;
  const metrics = (
    <dl className="grid gap-2 text-label sm:grid-cols-3">
      <div role="group" aria-label="执行中的 Session">
        <dt className="text-fg-muted">执行中的 Session</dt>
        <dd className="font-medium">
          <span aria-label="指标值">{known ? summary.sessions : "—"} 个</span>
        </dd>
        {known && (summary.unlinked > 0 || summary.reportedSessions > 0) && (
          <dd className="text-fg-muted">
            {summary.unlinked
              ? `另有 ${summary.unlinked} 个网关请求待关联 Session`
              : `含 ${summary.reportedSessions} 个 Agent 上报的 Session`}
          </dd>
        )}
      </div>
      <div role="group" aria-label="当前平均 TTFT">
        <dt className="text-fg-muted">当前平均 TTFT</dt>
        <dd className="font-medium">
          <span aria-label="指标值">
            {known && ttft != null ? `${seconds(ttft)} 秒` : "—"}
          </span>
        </dd>
        {known && summary.sources > 0 && (
          <dd className="text-fg-muted">
            {summary.ttftSamples}/{summary.sources} 个活动来源已记录
            {summary.ttftPendingLabel ? ` · ${summary.ttftPendingLabel}` : ""}
          </dd>
        )}
        {known && reportedTtft != null && (
          <dd className="text-fg-muted">
            {source} · {reportedTtfts.length} 个会话已上报
          </dd>
        )}
      </div>
      <div role="group" aria-label={rateLabel}>
        <dt className="text-fg-muted">{rateLabel}</dt>
        <dd className="font-medium">
          <span aria-label="指标值">
            {known && rate != null ? speed(rate) : "—"} Token/s
          </span>
        </dd>
        {known && summary.sources > 0 && (
          <dd className="text-fg-muted">
            {summary.samples}/{summary.sources} 个活动来源可测
            {summary.samples && summary.partial ? " · 部分可测" : ""}
            {rate == null && " · 当前来源未提供可测的 Token 增量"}
          </dd>
        )}
        {known && reportedRate != null && (
          <dd className="text-fg-muted">
            {source} · {reportedRates.length} 个会话
          </dd>
        )}
      </div>
    </dl>
  );
  if (state !== "ready" || !activeCount) {
    const visibility =
      reportedOnly || nativeSourceEnabled
        ? "当前无在途模型调用 · 当前没有观测到正在执行的任务。"
        : nativeObservationPaused
          ? "当前无在途网关调用 · 原生活动采集已暂停，当前执行状态待确认。"
          : "当前无在途网关调用 · 原生观测未授权，当前执行状态待确认。";
    return (
      <section
        aria-label="实时任务汇总"
        className="col-span-full space-y-1 rounded-lg border border-border px-3 py-2"
      >
        <div
          role="status"
          aria-label="实时任务汇总"
          className="flex flex-wrap items-center gap-2 text-label text-fg-muted"
        >
          {state === "ready" ? (
            <>
              <Badge size="sm" status={gatewayReady ? "success" : "neutral"}>
                {gatewayOnly
                  ? "仅网关"
                  : gatewayReady
                    ? "网关就绪"
                    : "网关未就绪"}
              </Badge>
              {visibility} · {timing}
            </>
          ) : state === "loading" ? (
            "正在读取当前任务…"
          ) : state === "stale" ? (
            "快照已过期，当前任务与速率待更新。"
          ) : (
            "暂时无法读取当前任务，恢复后自动更新。"
          )}
          {ready && nativeObservationPaused && (
            <a
              href="#/plugins?scene=activity"
              className="text-fg-link underline"
            >
              管理活动采集
            </a>
          )}
          {gatewayOnly && !nativeObservationPaused && sourceHref && (
            <a href={"#" + sourceHref} className="text-fg-link underline">
              开启原生观测
            </a>
          )}
        </div>
        {metrics}
      </section>
    );
  }
  return (
    <section
      aria-label="实时任务汇总"
      className="min-w-0 space-y-2 rounded-xl border border-border bg-surface-floating p-3"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-body font-medium">实时模型调用与速率</h3>
        {gatewayOnly && (
          <p className="text-label text-fg-muted">
            仅网关 · 当前仅统计经本应用的网关请求
          </p>
        )}
        <Button
          variant="ghost"
          size="xs"
          aria-pressed={paused}
          aria-label={paused ? "播放活动流动效果" : "暂停活动流动效果"}
          onClick={() => setPaused(!paused)}
        >
          {paused ? "播放动效" : "暂停动效"}
        </Button>
      </div>
      {metrics}
      <section
        aria-label="实时模型流向"
        className="grid min-w-0 gap-3 2xl:grid-cols-2"
      >
        {summary.models.length > 0 && (
          <ModelActivityFlow
            models={summary.models}
            paused={paused}
            colors={nextColors}
          />
        )}
        <div className="min-w-0" aria-label="最近 30 秒 Token 速率">
          <p className="text-label text-fg-muted">最近 30 秒 Token/s</p>
          {latest != null && points.length > 0 ? (
            <LiveLineChart
              data={points}
              value={latest}
              window={30}
              paused={paused}
              className="h-28"
            >
              <LiveLine dataKey="value" badge={false} pulse={!paused} />
              <LiveXAxis />
              <LiveYAxis />
            </LiveLineChart>
          ) : (
            <p className="py-3 text-label text-fg-muted">暂无可测速率样本</p>
          )}
        </div>
      </section>
    </section>
  );
}
