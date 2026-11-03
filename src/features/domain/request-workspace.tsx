import { ObjectContributionSlots } from "@/features/plugins/object-detail-slots";
import { useDeferredValue, useMemo, useState, useRef, useEffect } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
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
  UnifiedRequestItem,
  UnifiedRequestQuery,
  RequestUnit,
  RequestSort,
} from "../../../contracts/domain/facts/types";
import type { QueryScope } from "./types";
import { FactWorkspace } from "./fact-workspace";
import { unifiedRequestsResource } from "./queries";
import { RequestExport } from "./request-export";
import {
  RequestEvidenceDetails,
  RequestEvidenceTotals,
} from "./request-evidence";

const sortNames: Record<RequestSort, string> = {
  time: "发生时间",
  tokens: "总 Token",
  duration: "记录耗时",
  ttft: "记录首出时长",
  "cost-usd": "已知 USD 费用",
};

const number = (value: string | null | undefined) =>
  value == null ? "未知" : BigInt(value).toLocaleString("zh-CN");
const identity = (row: UnifiedRequestItem) =>
  JSON.stringify([row.record.sourceId, row.record.id]);
const columns: ColumnDef<UnifiedRequestItem>[] = [
  {
    id: "request",
    header: "请求记录",
    cell: ({ row }) => (
      <div className="max-w-64 whitespace-normal break-words">
        <p>
          {row.original.record.sessionTitle ??
            row.original.record.model ??
            "未命名请求"}
        </p>
        <p className="text-label text-fg-muted">
          {row.original.evidence === "source-reported"
            ? "来源报告响应"
            : row.original.record.unit === "model-response"
              ? "原生模型响应"
              : "网关逻辑请求"}
        </p>
        <p className="text-label text-fg-subtle">
          {formatDateTime(row.original.record.occurredAt)}
        </p>
      </div>
    ),
  },
  {
    id: "usage",
    header: "状态与用量",
    cell: ({ row }) => (
      <div className="max-w-48 whitespace-normal break-words">
        <p>{row.original.record.outcome}</p>
        <p className="text-label text-fg-muted">
          {number(row.original.record.usage.totalTokens)} Token
        </p>
        <p className="text-label text-fg-subtle">
          尝试 {number(row.original.record.attempts)}
        </p>
      </div>
    ),
  },
];
export function RequestWorkspace({
  scope,
  sessionId = null,
  names,
  onClose,
}: {
  scope: QueryScope;
  sessionId?: string | null;
  names: Record<string, string>;
  onClose: () => void;
}) {
  const [unit, setUnit] = useState<RequestUnit>("model-response");
  const [search, setSearch] = useState("");
  const [session, setSession] = useState(sessionId);
  const [descending, setDescending] = useState(true);
  const [sort, setSort] = useState<RequestSort>("time");
  const deferred = useDeferredValue(search);
  const base: Omit<UnifiedRequestQuery, "snapshotToken" | "cursor"> = {
    scope,
    unit,
    descending,
    sort,
    includeEvidence: true,
    filter: {
      start: null,
      end: null,
      productId: null,
      sessionId: session,
      projectId: null,
      providerKey: null,
      model: null,
      outcome: null,
      search: deferred || null,
    },
  };
  return (
    <section aria-label="统一请求" className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border p-2">
        <Button variant="tertiary" size="sm" onClick={onClose}>
          返回会话列表
        </Button>
        <Select value={unit} onValueChange={(v) => setUnit(v as RequestUnit)}>
          <SelectTrigger aria-label="请求统计单位" />
          <SelectContent>
            <SelectItem value="model-response">模型响应</SelectItem>
            <SelectItem value="logical-request">网关逻辑请求</SelectItem>
          </SelectContent>
        </Select>
        <Select
          value={`${sort}:${descending ? "desc" : "asc"}`}
          onValueChange={(v) => {
            const [metric, direction] = v.split(":");
            if (metric in sortNames) {
              setSort(metric as RequestSort);
              setDescending(direction === "desc");
            }
          }}
        >
          <SelectTrigger aria-label="请求排序" />
          <SelectContent>
            {Object.entries(sortNames).flatMap(([metric, name]) => [
              <SelectItem key={`${metric}:desc`} value={`${metric}:desc`}>
                {name}
                {metric === "time" ? "从新到旧" : "从高到低"}
              </SelectItem>,
              <SelectItem key={`${metric}:asc`} value={`${metric}:asc`}>
                {name}
                {metric === "time" ? "从旧到新" : "从低到高"}
              </SelectItem>,
            ])}
          </SelectContent>
        </Select>
        <Input
          aria-label="搜索统一请求"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="搜索模型、会话或请求 ID"
        />
        {session ? (
          <Button variant="tertiary" size="sm" onClick={() => setSession(null)}>
            查看所选来源全部请求
          </Button>
        ) : null}
      </div>
      <RequestResults key={JSON.stringify(base)} base={base} names={names} />
    </section>
  );
}
function RequestResults({
  base,
  names,
}: {
  base: Omit<UnifiedRequestQuery, "snapshotToken" | "cursor">;
  names: Record<string, string>;
}) {
  const [trail, setTrail] = useState<
    { snapshotToken: string | null; cursor: string | null }[]
  >([{ snapshotToken: null, cursor: null }]);
  const [selected, setSelected] = useState<string | null>(null);
  const [facts, setFacts] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null),
    detailRef = useRef<HTMLDivElement>(null);
  const restore = useRef<{ list: number; detail: number; key: string } | null>(
    null,
  );
  const current = trail[trail.length - 1];
  const resource = useSharedQuery(
    unifiedRequestsResource,
    { ...base, ...current },
    facts === null,
  );
  const page = resource.error || resource.loading ? null : resource.data;
  const rows = useMemo(() => page?.items ?? [], [page]);
  const active = rows.find((r) => identity(r) === selected);
  const openFacts = (id: string, key: string) => {
    restore.current = {
      list: listRef.current?.scrollTop ?? 0,
      detail: detailRef.current?.scrollTop ?? 0,
      key,
    };
    setFacts(id);
  };
  useEffect(() => {
    if (facts || !page || !restore.current) return;
    const frame = window.requestAnimationFrame(() => {
      const saved = restore.current;
      if (!saved || !listRef.current || !detailRef.current) return;
      listRef.current.scrollTop = saved.list;
      detailRef.current.scrollTop = saved.detail;
      const target = [
        ...detailRef.current.querySelectorAll<HTMLButtonElement>(
          "[data-request-navigation]",
        ),
      ].find((e) => e.dataset.requestNavigation === saved.key);
      if (target) {
        target.focus({ preventScroll: true });
        restore.current = null;
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [facts, page]);
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
  const reset = () => {
    setTrail([{ snapshotToken: null, cursor: null }]);
    setSelected(null);
    void resource.invalidate();
  };
  if (facts)
    return (
      <FactWorkspace
        scope={base.scope}
        sessionId={facts}
        names={names}
        onClose={() => setFacts(null)}
        returnLabel="返回请求列表"
      />
    );
  return (
    <>
      <div
        tabIndex={0}
        aria-label="请求范围统计"
        className="max-h-32 shrink-0 space-y-2 overflow-y-auto border-b border-border p-2 text-label"
      >
        {page ? (
          <RequestExport
            key={page.snapshotToken}
            snapshotToken={page.snapshotToken}
          />
        ) : null}
        <p>
          仅查询已发布记录，不刷新来源或读取正文。
          {base.unit === "model-response" ? "模型响应" : "网关逻辑请求"}
          与尝试分别计数，各来源重叠未核实。当前排序的未知值排在末尾；来源报告耗时不作宿主耗时排序，费用仅按已知
          USD 金额排序。
        </p>
        {page?.sourceSummaries.map((s) => (
          <div key={s.sourceId} className="space-y-1">
            <p>
              {names[s.sourceId] ?? s.sourceId}：{number(s.records)}{" "}
              {s.unit === "model-response" ? "个响应" : "个逻辑请求"}；已知{" "}
              {number(s.usage.totalTokens)} Token，{number(s.usageMissing)}{" "}
              条记录总用量不完整；尝试 {number(s.attempts)}，时间未知{" "}
              {number(s.unknownTime)}。
              {s.coverage === "unavailable"
                ? "当前能力不可用，保持未知。"
                : s.coverage === "partial"
                  ? "覆盖范围不完整。"
                  : ""}
            </p>
            <RequestEvidenceTotals value={s.evidenceSummary} />
          </div>
        ))}
        {page?.sourceStates.map((s) => (
          <p key={s.sourceId} className="text-fg-muted">
            {names[s.sourceId] ?? s.sourceId}：
            {s.authorization !== "authorized"
              ? "待授权"
              : s.problem === "unsupported"
                ? "不支持当前统计单位"
                : s.problem === "counter_overflow"
                  ? "部分用量合计超出上限，保持未知"
                  : s.coverage === "unavailable"
                    ? "暂不可读取"
                    : s.freshness === "stale"
                      ? "记录已过期"
                      : s.freshness === "failed"
                        ? "读取失败，保留已发布记录"
                        : "已发布记录"}
            。
          </p>
        ))}
        {page ? (
          <p>
            冻结于 {formatDateTime(page.asOf)}
            ；小计与关联、费用证据覆盖完整匹配范围，保留原生与网关各自口径；完整尝试明细可进入原请求详情查看。
          </p>
        ) : null}
        {resource.error ? (
          <p role="alert">请求查询暂不可用：{resource.error}</p>
        ) : null}
      </div>
      <div className="grid min-h-0 flex-1 grid-cols-2 overflow-hidden">
        <div className="flex min-h-0 min-w-0 flex-col border-r border-border">
          <div
            ref={listRef}
            tabIndex={0}
            aria-label="请求记录滚动区域"
            className="min-h-0 flex-1 overflow-y-auto"
          >
            <DataTable
              table={table}
              showPagination={false}
              isLoading={resource.loading && !page}
              loadingMessage="正在查询已发布请求"
              emptyMessage={
                resource.error
                  ? "请求信息暂不可读取"
                  : "当前已发布范围没有匹配记录；未接入部分保持未知"
              }
              activeRowId={selected}
              onRowActivate={(r) => setSelected(identity(r.original))}
            />
          </div>
          <div className="flex shrink-0 flex-wrap gap-2 border-t border-border p-2">
            <Button
              variant="tertiary"
              size="sm"
              disabled={resource.loading || trail.length < 2}
              onClick={() => {
                setTrail((t) => t.slice(0, -1));
                setSelected(null);
              }}
            >
              上一页请求
            </Button>
            <Button
              variant="tertiary"
              size="sm"
              disabled={resource.loading || !page?.nextCursor}
              onClick={() => {
                const nextCursor = page?.nextCursor;
                if (page && nextCursor) {
                  setTrail((t) => [
                    ...t.map((p) => ({
                      ...p,
                      snapshotToken: page.snapshotToken,
                    })),
                    {
                      snapshotToken: page.snapshotToken,
                      cursor: nextCursor,
                    },
                  ]);
                  setSelected(null);
                }
              }}
            >
              下一页请求
            </Button>
            <Button variant="tertiary" size="sm" onClick={reset}>
              重新查询请求
            </Button>
            <span className="text-label text-fg-muted">
              本页 {rows.length} 条
            </span>
          </div>
        </div>
        <div
          ref={detailRef}
          tabIndex={0}
          aria-label="统一请求详情"
          className="min-h-0 min-w-0 space-y-3 overflow-y-auto p-3 text-label"
        >
          {active ? (
            <>
              <h3 className="break-words text-heading-sm">
                {active.record.sessionTitle ??
                  active.record.model ??
                  "请求记录"}
              </h3>
              <p>
                {names[active.record.sourceId] ?? active.record.sourceId} ·{" "}
                {active.evidence === "source-reported"
                  ? "来源报告"
                  : "宿主观测"}
              </p>
              <p className="break-all">记录：{active.record.id}</p>
              {page && (
                <ObjectContributionSlots
                  object={{
                    target: "request",
                    id: active.record.id,
                    unit: active.record.unit,
                    origin: {
                      kind: "unified",
                      scope: base.scope,
                      sourceId: active.record.sourceId,
                      snapshotToken: page.snapshotToken,
                    },
                  }}
                />
              )}
              <p>发生于 {formatDateTime(active.record.occurredAt)}</p>
              <p>
                已知用量 {number(active.record.usage.totalTokens)} Token；尝试{" "}
                {number(active.record.attempts)}。
              </p>
              <p>
                原请求记录耗时 {active.record.durationMs ?? "未知"} ms；首出时长{" "}
                {active.record.ttftMs ?? "未知"} ms。
              </p>
              {active.evidence === "source-reported" ? (
                <>
                  <p>
                    来源报告耗时 {number(active.reportedDurationMs)}{" "}
                    ms，不计作宿主计时。
                  </p>
                  <p className="break-all">
                    来源报告响应 ID：{active.reportedResponseId ?? "未知"}
                    。跨来源关联未经核实，费用未知。
                  </p>
                </>
              ) : null}
              <RequestEvidenceDetails
                value={active.evidenceDetail}
                onSession={openFacts}
              />
              {active.hostRequestId ? (
                <Button asChild variant="tertiary" size="sm">
                  <a
                    href={`#/requests/${encodeURIComponent(active.hostRequestId)}`}
                  >
                    查看原请求与费用明细
                  </a>
                </Button>
              ) : null}
              {active.record.unit === "model-response" &&
              active.record.sessionRef ? (
                <Button
                  variant="tertiary"
                  size="sm"
                  data-request-navigation="session"
                  onClick={() => {
                    if (active.record.sessionRef)
                      openFacts(active.record.sessionRef, "session");
                  }}
                >
                  查看会话证据
                </Button>
              ) : null}
            </>
          ) : (
            <p className="text-fg-muted">
              选择记录查看详情。外部响应不提供网关尝试或精确关联。
            </p>
          )}
        </div>
      </div>
    </>
  );
}
