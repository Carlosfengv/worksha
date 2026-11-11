import { useEffect, useMemo, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { useSharedQuery } from "@/shared/hooks/use-shared-read";
import { formatDateTime } from "@/shared/lib/date-time";
import type {
  ActivityKind,
  ActivityRecord,
  UnifiedActivityQuery,
} from "../../../contracts/domain/facts/types";
import type { QueryScope } from "./types";
import { FactWorkspace } from "./fact-workspace";
import { unifiedActivityResource } from "./queries";

const kindNames: Record<ActivityKind, string> = {
  "native-message": "原生消息",
  "gateway-attempt": "网关尝试",
  "source-event": "来源历史事件",
};
const counts = (n: string | null | undefined) =>
  n == null ? "未知" : BigInt(n).toLocaleString("zh-CN");
const identity = (r: ActivityRecord) =>
  JSON.stringify([r.sourceId, r.kind, r.id]);
function liveRate(row: ActivityRecord, now: number) {
  const rate = row.measurements?.liveRate;
  if (
    row.state !== "current" ||
    !rate ||
    !Number.isFinite(rate.tokensPerSecond)
  )
    return null;
  const age = BigInt(now) - BigInt(rate.atMs);
  return age >= -1000n && age <= 5000n ? rate.tokensPerSecond : null;
}
type DisplayRecord = ActivityRecord & {
  sourceName: string;
  stateLabel: string;
  rateLabel: string;
};
const columns: ColumnDef<DisplayRecord>[] = [
  {
    id: "title",
    header: "活动 / 来源",
    cell: ({ row }) => (
      <div className="max-w-64 whitespace-normal break-words">
        <p>{row.original.title}</p>
        <p className="text-label text-fg-muted">
          {row.original.sourceName} · {kindNames[row.original.kind]}
        </p>
        <p className="text-label text-fg-subtle">
          {row.original.model ?? "模型未知"}
          {row.original.unknownClient ? " · 客户端未知" : ""}
        </p>
      </div>
    ),
  },
  {
    id: "state",
    header: "状态与采样",
    cell: ({ row }) => (
      <div className="max-w-48 whitespace-normal break-words">
        <p>{row.original.stateLabel}</p>
        <p className="text-label text-fg-muted">{row.original.rateLabel}</p>
        <p className="text-label text-fg-subtle">
          {formatDateTime(row.original.occurredAt)}
        </p>
      </div>
    ),
  },
];
export function ActivityWorkspace({
  scope,
  sessionId = null,
  productId = null,
  names,
  onClose,
  closeLabel = "返回会话列表",
}: {
  scope: QueryScope;
  sessionId?: string | null;
  productId?: string | null;
  names: Record<string, string>;
  onClose: () => void;
  closeLabel?: string;
}) {
  const [range, setRange] = useState(10800);
  const [gateway, setGateway] = useState(false);
  const [historical, setHistorical] = useState(true);
  const [session, setSession] = useState(sessionId);
  const [selected, setSelected] = useState<string | null>(null);
  const [facts, setFacts] = useState<string | null>(null);
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);
  const input: UnifiedActivityQuery = {
    scope,
    rangeSeconds: range,
    productId,
    sessionId: session,
    includeGateway: scope.kind !== "external" && gateway,
    includeHistorical: historical,
  };
  const resource = useSharedQuery(
    unifiedActivityResource,
    input,
    facts === null,
  );
  const page = resource.error ? null : resource.data;
  const rows = useMemo<DisplayRecord[]>(
    () =>
      (page?.items ?? []).map((row) => {
        const staleSample =
          row.kind !== "source-event" &&
          row.state === "current" &&
          Date.parse(row.observedAt) < now - 15000;
        const rate = staleSample ? null : liveRate(row, now);
        const reportedRunning =
          row.state === "current" && Date.parse(page?.asOf ?? "") >= now - 15000
            ? row.reportedMeasurements?.runningTokensPerSecond
            : null;
        const reportedRate =
          reportedRunning ?? row.reportedMeasurements?.completedTokensPerSecond;
        return {
          ...row,
          sourceName: names[row.sourceId] ?? row.sourceId,
          stateLabel:
            row.state === "historical"
              ? "历史记录"
              : row.state === "expired" || staleSample
                ? "观测已过期"
                : row.kind === "source-event"
                  ? "来源报告运行中"
                  : "当前观测",
          rateLabel:
            row.kind === "source-event"
              ? reportedRate == null
                ? "不提供实时采样"
                : `${reportedRate.toLocaleString("zh-CN", { maximumFractionDigits: 2 })} Token/s · DSH ${reportedRunning != null ? "当前步骤均速" : "已完成步骤均速"}`
              : rate === null
                ? "实时速率未知"
                : `${rate.toLocaleString("zh-CN", { maximumFractionDigits: 2 })} Token/s · 5 秒窗口`,
        };
      }),
    [page, names, now],
  );
  const active = rows.find((r) => identity(r) === selected);
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: identity,
    manualPagination: true,
    manualSorting: true,
    manualFiltering: true,
    enableSorting: false,
    enableRowSelection: false,
    enableHiding: false,
  });
  if (facts)
    return (
      <FactWorkspace
        scope={scope}
        sessionId={facts}
        names={names}
        onClose={() => setFacts(null)}
      />
    );
  return (
    <section aria-label="统一活动快照" className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border p-2">
        <Button variant="tertiary" size="sm" onClick={onClose}>
          {closeLabel}
        </Button>
        <Select
          value={String(range)}
          onValueChange={(v) => setRange(Number(v))}
        >
          <SelectTrigger aria-label="活动时间范围" />
          <SelectContent>
            <SelectItem value="300">最近 5 分钟</SelectItem>
            <SelectItem value="3600">最近 1 小时</SelectItem>
            <SelectItem value="10800">最近 3 小时</SelectItem>
            <SelectItem value="86400">最近 24 小时</SelectItem>
          </SelectContent>
        </Select>
        <label className="flex items-center gap-2 text-label">
          <Checkbox
            checked={gateway && scope.kind !== "external"}
            disabled={scope.kind === "external"}
            onCheckedChange={(v) => setGateway(v === true)}
          />
          包含所选 Agent 的网关观测
        </label>
        <label className="flex items-center gap-2 text-label">
          <Checkbox
            checked={historical}
            onCheckedChange={(v) => setHistorical(v === true)}
          />
          包含来源历史事件
        </label>
        <Button
          variant="tertiary"
          size="sm"
          onClick={() => void resource.invalidate()}
        >
          重新查询活动
        </Button>
        {session ? (
          <Button
            variant="tertiary"
            size="sm"
            onClick={() => {
              setSession(null);
              setSelected(null);
            }}
          >
            查看所选来源全部活动
          </Button>
        ) : null}
      </div>
      <div
        tabIndex={0}
        aria-label="活动范围与单位说明"
        className="max-h-32 shrink-0 space-y-2 overflow-y-auto border-b border-border p-2 text-label"
      >
        <p>
          {session ? "单个会话范围" : "所选来源范围"} ·
          查询已发布观测，不触发来源读取。原生消息、网关尝试与来源历史事件分别计数，重叠未核实，不合并为总任务数。
        </p>
        {page?.sourceSummaries.map((s) => (
          <p key={JSON.stringify([s.sourceId, s.kind])}>
            {names[s.sourceId] ?? s.sourceId} · {kindNames[s.kind]}：已观测{" "}
            {counts(s.observedRecords)} 条；当前 {counts(s.current)}，过期{" "}
            {counts(s.expired)}，历史 {counts(s.historical)}，时间未知{" "}
            {counts(s.unknownTime)}
            {s.logicalRequests === null
              ? ""
              : `；逻辑请求 ${counts(s.logicalRequests)} 个`}
            。
          </p>
        ))}
        {page?.sourceStates.map((s) => (
          <p key={JSON.stringify([s.state.sourceId, s.kind])}>
            {names[s.state.sourceId] ?? s.state.sourceId} · {kindNames[s.kind]}
            ：
            {s.state.authorization !== "authorized"
              ? "待授权"
              : s.state.problem === "unsupported"
                ? "来源不支持活动"
                : s.state.problem === "history_not_selected"
                  ? "未选择历史事件"
                  : s.state.freshness === "failed" ||
                      s.state.problem === "partial_source"
                    ? s.state.coverage === "unavailable"
                      ? "来源读取失败，当前范围未知"
                      : "来源读取失败，保留已发布观测"
                    : s.state.coverage === "unavailable"
                      ? "暂不可读取"
                      : s.state.freshness === "stale"
                        ? "观测已过期"
                        : "已发布观测，覆盖范围可能不完整"}
            。
          </p>
        ))}
        {page?.limited ? (
          <p role="status">
            本次仅显示最近 256
            条，以上小计保留完整已观测范围；可缩小时间或会话范围。
          </p>
        ) : null}
        {page ? (
          <p>
            查询于 {formatDateTime(page.asOf)}
            。时间未知的来源事件单独保留，不能据此确认属于所选时间段。
          </p>
        ) : null}
        {resource.error ? (
          <p role="alert">活动快照暂不可读取：{resource.error}</p>
        ) : null}
      </div>
      <div className="grid min-h-0 flex-1 grid-cols-2 overflow-hidden">
        <div
          tabIndex={0}
          aria-label="活动记录滚动区域"
          className="min-h-0 min-w-0 overflow-y-auto"
        >
          <DataTable
            table={table}
            showPagination={false}
            isLoading={resource.loading && !page}
            loadingMessage="正在查询已发布活动"
            emptyState={
              <p className="p-3 text-label text-fg-muted">
                {resource.error
                  ? "活动信息暂不可读取"
                  : page?.sourceStates.some(
                        (s) => s.state.coverage !== "unavailable",
                      )
                    ? "当前已发布范围没有匹配记录，未接入部分保持未知。"
                    : "所选来源暂未提供活动，未授权或不支持的能力保持未知。"}
              </p>
            }
            activeRowId={selected}
            onRowActivate={(r) => setSelected(identity(r.original))}
          />
        </div>
        <div
          tabIndex={0}
          aria-label="活动观测详情"
          className="min-h-0 min-w-0 space-y-3 overflow-y-auto p-3"
        >
          {active ? (
            <>
              <h3 className="break-words text-heading-sm">{active.title}</h3>
              <p className="text-label text-fg-muted">
                {kindNames[active.kind]} · {active.stateLabel}
              </p>
              <p className="text-label">{active.rateLabel}</p>
              {active.reportedMeasurements?.firstTokenMs != null && (
                <p className="text-label">
                  DSH 步骤首出{" "}
                  {active.reportedMeasurements.firstTokenMs.toLocaleString(
                    "zh-CN",
                  )}{" "}
                  ms
                </p>
              )}
              {active.kind === "source-event" ? (
                <p className="text-label">
                  来源报告耗时：{counts(active.historicalDurationMs)}{" "}
                  ms，不计作运行采样。
                </p>
              ) : null}
              <p className="text-label">
                {active.measurements?.reportedTtftMs == null
                  ? "原生事件首出时长未知"
                  : `原生事件首出 ${active.measurements.reportedTtftMs.toLocaleString("zh-CN")} ms`}
              </p>
              <p className="text-label">
                {active.measurements?.firstTokenMs == null
                  ? "运行采样 TTFT 未知"
                  : `运行采样 TTFT ${active.measurements.firstTokenMs.toLocaleString("zh-CN")} ms`}
              </p>
              <p className="text-label">
                历史正文均速：
                {active.measurements?.historicalAverageTextRate == null
                  ? "未知"
                  : `${active.measurements.historicalAverageTextRate.toLocaleString("zh-CN", { maximumFractionDigits: 2 })} Token/s`}
                ，不计作实时速率。
              </p>
              <p className="break-words text-label">
                工具：{active.toolNames.join("、") || "未报告"}
              </p>
              {active.sessionId ? (
                <Button
                  variant="tertiary"
                  size="sm"
                  onClick={() => setFacts(active.sessionId ?? null)}
                >
                  查看会话证据
                </Button>
              ) : (
                <p className="text-label text-fg-muted">会话关联未确认。</p>
              )}
              {active.kind === "gateway-attempt" ? (
                <Button asChild variant="tertiary" size="sm">
                  <a href={`#/requests/${encodeURIComponent(active.id)}`}>
                    查看网关请求
                  </a>
                </Button>
              ) : null}
              <p className="text-label text-fg-subtle">
                宿主观察于 {formatDateTime(active.observedAt)}；事件发生于{" "}
                {formatDateTime(active.occurredAt)}。
              </p>
            </>
          ) : (
            <p className="text-label text-fg-muted">
              选择记录查看采样状态和关联。历史事件不能证明当前仍在运行。
            </p>
          )}
        </div>
      </div>
    </section>
  );
}
