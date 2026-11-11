import { useEffect, useId, useRef, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import { AiBrain03Icon } from "@hugeicons/core-free-icons";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { MetricCard, type MetricCardState } from "@/components/ui/metric-card";
import type { ActivitySummaryData } from "../activity-summary";
import { seconds, speed } from "../display";
import { ActivityTime } from "./activity-time";
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
  nativeSourceEnabled,
  sourceHref,
  nativeObservationPaused = false,
  reportedOnly = false,
  reportedTtfts = [],
  reportedRates = [],
}: {
  summary: ActivitySummaryData;
  state: MetricCardState;
  nativeSourceEnabled: boolean;
  sourceHref: string;
  nativeObservationPaused?: boolean;
  reportedOnly?: boolean;
  reportedTtfts?: number[];
  reportedRates?: number[];
}) {
  const [paused, setPaused] = useState(false);
  const [colors, setColors] =
    useState<ReadonlyMap<string, string>>(loadModelColors);
  const nextColors = assignModelColors(
    colors,
    summary.models.map((model) => model.id),
  );
  if (nextColors !== colors) setColors(nextColors);
  useEffect(() => saveModelColors(colors), [colors]);
  const ready = state === "ready";
  const hasActivity = summary.sources > 0 || summary.reportedSessions > 0;
  const hasMetricSources = summary.sources > 0;
  const mean = (values: number[]) =>
    values.length
      ? values.reduce((total, value) => total + value / values.length, 0)
      : null;
  const sourceTtft = hasMetricSources ? null : mean(reportedTtfts);
  const sourceRate = hasMetricSources ? null : mean(reportedRates);
  const ttft = summary.ttft ?? sourceTtft;
  const rateAvailable =
    hasMetricSources || (!reportedOnly && summary.reportedSessions === 0);
  const metricsAvailable = ready && (hasActivity || nativeSourceEnabled);
  const metricState = ready && !metricsAvailable ? "unavailable" : state;
  const message =
    state === "loading"
      ? "正在读取当前任务…"
      : state === "stale"
        ? "快照已过期，当前任务与速率待更新。"
        : state === "error"
          ? "暂时无法读取当前任务，恢复后自动更新。"
          : !hasActivity
            ? nativeSourceEnabled
              ? "当前没有观测到正在执行的任务。"
              : nativeObservationPaused
                ? "原生活动采集已暂停，当前执行状态待确认。"
                : "未开启原生会话读取，当前执行状态待确认。"
            : hasMetricSources
              ? `近 ${summary.windowMs / 1000} 秒可测输出合计，包含正文、已上报的思考和网关工具参数。历史采样单独标记。`
              : "运行状态由 Agent 上报。";
  return (
    <section
      aria-label="实时任务汇总"
      className="space-y-2 rounded-xl border border-border bg-surface-floating p-3"
    >
      <div className="flex items-center justify-between gap-3">
        <h3 className="text-body font-medium">活动性能明细</h3>
        <Badge size="sm" status={ready && hasActivity ? "success" : "neutral"}>
          {ready
            ? !nativeSourceEnabled && (!reportedOnly || hasMetricSources)
              ? "仅网关"
              : hasActivity
                ? "实时"
                : nativeSourceEnabled
                  ? "暂无活动"
                  : "待接入"
            : state === "loading"
              ? "读取中"
              : "待更新"}
        </Badge>
      </div>
      <div className="grid gap-2 sm:grid-cols-3">
        <MetricCard
          className="self-stretch p-2"
          valueClassName="text-title"
          label="执行中的 Session"
          value={metricsAvailable ? summary.sessions : "—"}
          unit="个"
          state={metricState}
          meta={
            ready && summary.unlinked
              ? `另有 ${summary.unlinked} 个网关请求待关联 Session`
              : ready && summary.reportedSessions
                ? `含 ${summary.reportedSessions} 个 Agent 上报的 Session`
                : "同一 Session 的消息与工具步骤只计一次"
          }
        />
        <MetricCard
          className="self-stretch p-2"
          valueClassName="text-title"
          label="当前平均 TTFT"
          value={
            ready
              ? ttft === null
                ? hasMetricSources
                  ? (summary.ttftPendingLabel ?? "等待首 Token")
                  : "—"
                : seconds(ttft)
              : "—"
          }
          unit={ready && ttft !== null ? "秒" : undefined}
          state={metricState}
          meta={
            ready && hasMetricSources
              ? `${summary.ttftSamples}/${summary.sources} 个活动来源已记录 · 原生含事件传递延迟`
              : ready && sourceTtft !== null
                ? `DSH 步骤首出 · ${reportedTtfts.length} 个会话已上报`
                : rateAvailable
                  ? "首个推理或正文返回后更新"
                  : "来源未提供"
          }
        />
        <MetricCard
          className="self-stretch p-2"
          valueClassName="text-title"
          label={
            ready && sourceRate !== null
              ? "DSH 当前步骤均速"
              : ready &&
                  (summary.partial || summary.reportedSessions > 0) &&
                  summary.rate !== null &&
                  rateAvailable
                ? "已测 Token 速率"
                : "总 Token 速率"
          }
          value={
            metricsAvailable && sourceRate !== null
              ? speed(sourceRate)
              : metricsAvailable && rateAvailable && summary.rate !== null
                ? speed(summary.rate)
                : "—"
          }
          unit="Token/s"
          state={metricState}
          footer={
            ready &&
            hasMetricSources &&
            summary.rate === null &&
            summary.recentSample ? (
              <>
                最近单个来源 {speed(summary.recentSample.tokensPerSecond)}{" "}
                Token/s ·{" "}
                <ActivityTime
                  value={new Date(summary.recentSample.at).toISOString()}
                  kind="sample"
                />
              </>
            ) : undefined
          }
          statusMessage={
            ready && hasMetricSources && summary.rate === null
              ? "当前来源未提供可测的 Token 增量"
              : undefined
          }
          meta={
            ready && hasMetricSources
              ? `${summary.samples}/${summary.sources} 个活动来源可测${summary.samples === 0 ? " · 暂无实时采样" : summary.partial ? " · 部分可测" : ""}`
              : ready && sourceRate !== null
                ? `DSH 已上报 · ${reportedRates.length} 个会话`
                : rateAvailable
                  ? "仅统计当前任务"
                  : "来源未提供"
          }
        />
      </div>
      {ready && hasMetricSources && (
        <ModelActivityFlow
          models={summary.models}
          paused={paused}
          colors={nextColors}
        />
      )}
      <div className="flex items-start justify-between gap-3">
        <p
          className="text-label text-fg-muted"
          role={ready ? undefined : "status"}
        >
          {message}
        </p>
        {ready &&
          summary.models.some(
            (model) => model.rate !== null && model.rate > 0,
          ) && (
            <Button
              variant="ghost"
              size="xs"
              aria-label={paused ? "播放活动流动效果" : "暂停活动流动效果"}
              aria-pressed={paused}
              onClick={() => setPaused(!paused)}
            >
              {paused ? "播放动效" : "暂停动效"}
            </Button>
          )}
      </div>
      {ready && !nativeSourceEnabled && !reportedOnly && (
        <p className="text-label text-fg-muted">
          {nativeObservationPaused
            ? "原生活动采集已暂停，当前仅统计经本应用的网关请求。可在插件管理中恢复采集。"
            : "当前仅统计经本应用的网关请求。官方登录及其他直连会话需开启原生观测。"}
          <a
            href={`#${sourceHref}`}
            className="ml-2 rounded-lg text-fg-muted outline-none hover:text-fg-default focus-visible:ring-1 focus-visible:ring-focus-ring"
          >
            {nativeObservationPaused ? "管理活动采集" : "开启原生观测"}
          </a>
        </p>
      )}
    </section>
  );
}
