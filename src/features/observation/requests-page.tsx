import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useBackendReadEpoch } from "@/shared/hooks/use-backend-read-epoch";
import { backendReadEpoch } from "@/shared/lib/backend-recovery";
import { Button } from "@/components/ui/button";
import { Tabs, TabsList, TabItem, TabPanel } from "@/components/ui/tabs";
import {
  Accordion,
  AccordionItem,
  AccordionTrigger,
  AccordionContent,
} from "@/components/ui/accordion";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { date } from "@/features/sessions/display";
import { observationApi } from "./api";
import { calendarRange, requestParams } from "./request-display";
import type { RequestDashboard, RequestQuery } from "./request-types";
import { RequestFilters } from "./components/request-filters";
import { RequestMetrics } from "./components/request-metrics";
import { RequestTrends } from "./components/request-trends";
import { RequestsTable } from "./components/requests-table";
import { RequestSources } from "./components/request-sources";
import { RequestHeatmap } from "./components/request-heatmap";
import { RequestDetail } from "./components/request-detail";
import { PublishedSources } from "@/features/domain/published-sources";

export function RequestsPage({
  selectedId,
  routeSearch = "",
}: {
  selectedId?: string;
  routeSearch?: string;
}) {
  const params = useMemo(() => new URLSearchParams(routeSearch), [routeSearch]);
  const range = params.get("range") ?? "today";
  const query = useMemo<RequestQuery>(() => {
    const q: RequestQuery = {};
    for (const key of [
      "source",
      "start",
      "end",
      "agent",
      "result",
      "provider",
      "model",
      "session",
      "project",
      "search",
      "issue",
      "sort",
      "metric",
      "dimension",
      "currency",
      "snapshot",
      "cursor",
    ] as const) {
      const value = params.get(key);
      if (value) q[key] = value;
    }
    q.descending = params.get("descending") !== "false";
    return q;
  }, [params]);
  const nativeView = query.source === "native";
  const unit = nativeView ? "模型响应记录" : "请求";
  const listRef = useRef<HTMLDivElement>(null);
  const exporting = useRef(false);
  const epoch = useBackendReadEpoch(!selectedId);
  const [readingPaused, setReadingPaused] = useState(false);
  const [view, setView] = useState("records");
  const [interval, setIntervalSeconds] = useState("5");
  const [revision, setRevision] = useState(0);
  const [state, setState] = useState<{
    key: string;
    data: RequestDashboard | null;
    error: string | null;
    loading: boolean;
  }>({ key: "", data: null, error: null, loading: true });
  const [exportState, setExportState] = useState({
    running: false,
    message: "",
    error: false,
  });
  const key = JSON.stringify([query, range]);
  const data = state.key === key ? state.data : null;
  const change = useCallback(
    (patch: Record<string, string | undefined>) => {
      const next = new URLSearchParams(params);
      next.delete("snapshot");
      next.delete("cursor");
      if ((patch.range ?? next.get("range")) !== "custom") {
        next.delete("start");
        next.delete("end");
      }
      for (const [k, v] of Object.entries(patch)) {
        if (v === undefined) next.delete(k);
        else next.set(k, v);
      }
      window.location.hash = `/requests?${next}`;
    },
    [params],
  );
  useEffect(() => {
    if (selectedId) return;
    let alive = true,
      running = false;
    const load = async () => {
      if (running) return;
      running = true;
      setState((old) => ({
        key,
        data: old.key === key ? old.data : null,
        error: null,
        loading: true,
      }));
      try {
        const bounds =
          query.snapshot || range === "custom" ? {} : calendarRange(range);
        const result = await observationApi.requestDashboard({
          ...query,
          ...bounds,
        });
        if (alive && epoch === backendReadEpoch())
          setState({ key, data: result, error: null, loading: false });
      } catch (reason) {
        if (alive && epoch === backendReadEpoch())
          setState((old) => ({
            key,
            data: old.key === key ? old.data : null,
            error: reason instanceof Error ? reason.message : String(reason),
            loading: false,
          }));
      } finally {
        running = false;
      }
    };
    if (range !== "custom" || (query.start && query.end)) void load();
    const timer =
      interval !== "0" && !query.snapshot
        ? window.setInterval(
            () => {
              const rect = listRef.current?.getBoundingClientRect();
              const reading =
                !!rect &&
                rect.height > 0 &&
                rect.top < window.innerHeight &&
                rect.bottom > 0;
              setReadingPaused(reading);
              if (
                !exporting.current &&
                !document.hidden &&
                document.hasFocus() &&
                !reading
              )
                void load();
            },
            Number(interval) * 1000,
          )
        : null;
    return () => {
      alive = false;
      if (timer) window.clearInterval(timer);
    };
  }, [key, query, range, revision, interval, selectedId, epoch]);
  const open = useCallback(
    (id: string) => {
      const search = data
        ? requestParams(data.query, range, data.snapshot, query.cursor)
        : routeSearch;
      window.location.hash = `/requests/${encodeURIComponent(id)}?${search}`;
    },
    [data, query.cursor, range, routeSearch],
  );
  const refresh = () => {
    if (query.snapshot) change({});
    else setRevision((n) => n + 1);
  };
  if (selectedId)
    return <RequestDetail id={selectedId} returnSearch={routeSearch} />;
  return (
    <div className="min-w-0 space-y-6">
      <PublishedSources mode="requests" productId={query.agent} />
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <p className="text-body font-medium">
            {nativeView
              ? "原生模型响应"
              : query.source === "linked"
                ? "已关联请求"
                : "网关请求"}
          </p>
          <p className="mt-1 text-label text-fg-muted">
            查看模型请求、消耗与性能，定位失败和等待。
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Select
            value={interval}
            onValueChange={(v) => setIntervalSeconds(v ?? "5")}
          >
            <SelectTrigger aria-label="请求刷新间隔" />
            <SelectContent>
              <SelectItem value="5">5 秒刷新</SelectItem>
              <SelectItem value="15">15 秒刷新</SelectItem>
              <SelectItem value="0">暂停刷新</SelectItem>
            </SelectContent>
          </Select>
          <Button
            variant="tertiary"
            loading={state.key === key && state.loading}
            onClick={refresh}
          >
            刷新
          </Button>
          <Button
            variant="tertiary"
            disabled={!data}
            loading={exportState.running}
            onClick={() => {
              if (!data || exporting.current) return;
              exporting.current = true;
              setExportState({ running: true, message: "", error: false });
              void observationApi
                .exportRequests(data.snapshot)
                .then((result) =>
                  setExportState({
                    running: false,
                    error: false,
                    message:
                      result.status === "saved"
                        ? `已导出 ${result.count} 条${unit}至 ${result.fileName}`
                        : "已取消导出",
                  }),
                )
                .catch((reason) =>
                  setExportState({
                    running: false,
                    error: true,
                    message:
                      reason instanceof Error ? reason.message : String(reason),
                  }),
                )
                .finally(() => {
                  exporting.current = false;
                });
            }}
          >
            导出 CSV
          </Button>
        </div>
      </div>
      {exportState.message && (
        <p role={exportState.error ? "alert" : "status"} className="text-label">
          {exportState.message}
        </p>
      )}
      <RequestSources
        source={query.source ?? "gateway"}
        report={data?.sourceReport}
        onChange={(source) => change({ source, provider: undefined })}
      />
      <RequestFilters
        key={`${params.get("search") ?? ""}:${params.get("range") ?? ""}:${params.get("start") ?? ""}:${params.get("end") ?? ""}`}
        params={params}
        data={data}
        onChange={change}
      />
      {state.key === key && state.error && (
        <p role="alert" className="text-body">
          {data ? "数据更新失败，保留旧快照：" : "请求读取失败："}
          {state.error}。可点击刷新重试。
        </p>
      )}
      {range === "custom" && !(query.start && query.end) ? (
        <p className="text-body text-fg-muted">选择起止时间后应用筛选。</p>
      ) : !data ? (
        <p role="status" className="text-body text-fg-muted">
          {state.error ? "当前没有可用快照" : "正在读取请求…"}
        </p>
      ) : (
        <>
          <div className="space-y-1 text-label text-fg-subtle">
            <p>
              当前筛选 {data.total} 条 · 更新于 {date(data.asOf)}
              {query.snapshot
                ? " · 当前正在阅读固定快照，刷新可获取新请求"
                : ""}
            </p>
            <p>
              已记录历史：{date(data.recordedStart)} 至 {date(data.recordedEnd)}
              {data.summary.provisional
                ? ` · ${data.summary.provisional} 条数据暂定`
                : ""}
            </p>
            {!!data.missingTime && (
              <p>
                {data.missingTime}{" "}
                条记录缺少时间，限定日期时无法纳入；全部范围单列，趋势与热力图不分配日期。
              </p>
            )}
            {data.sourceError && <p role="alert">{data.sourceError}</p>}
          </div>
          <RequestMetrics
            summary={data.summary}
            nativeView={nativeView}
            onResult={(result) => change({ result })}
            onIssue={(issue) => change({ issue })}
          />
          <Tabs
            value={view}
            onValueChange={setView}
            variant="underline"
            color="default"
          >
            <TabsList aria-label="请求工作区">
              <TabItem
                value="records"
                label={nativeView ? "响应记录" : "请求记录"}
              />
              <TabItem value="analysis" label="趋势分析" />
            </TabsList>
            <TabPanel value="analysis" className="space-y-6 pt-4">
              <RequestTrends
                data={data}
                onChange={change}
                onBucket={(start, end) => {
                  change({ range: "custom", start, end });
                  setView("records");
                }}
                onGroup={(id) => {
                  change({ [data.query.dimension ?? "agent"]: id });
                  setView("records");
                }}
              />
              <RequestHeatmap
                data={data}
                onDay={(start, end) => {
                  change({ range: "custom", start, end });
                  setView("records");
                }}
                onAllDates={() => change({ range: "all" })}
              />
            </TabPanel>
            <TabPanel value="records" className="space-y-4 pt-4">
              {readingPaused && !query.snapshot && (
                <p className="text-label text-fg-muted">
                  正在阅读列表，自动刷新已暂停；可点击刷新获取最新请求。
                </p>
              )}
              <div ref={listRef} className="min-w-0">
                <RequestsTable
                  key={query.source ?? "gateway"}
                  nativeView={nativeView}
                  rows={data.items}
                  onOpen={open}
                  sort={data.query.sort ?? "time"}
                  descending={data.query.descending ?? true}
                  onSort={(sort, descending) =>
                    change({ sort, descending: String(descending) })
                  }
                />
              </div>
              <div className="flex flex-wrap items-center justify-between gap-3">
                <p className="text-label text-fg-muted">
                  共 {data.total} 条 · 本页 {data.items.length} 条 ·{" "}
                  {query.cursor ? "当前为历史分页" : "第一页"}
                </p>
                <div className="flex gap-2">
                  {query.cursor && (
                    <Button
                      variant="tertiary"
                      onClick={() => {
                        const next = requestParams(
                          data.query,
                          range,
                          data.snapshot,
                        );
                        window.location.hash = `/requests?${next}`;
                      }}
                    >
                      返回第一页
                    </Button>
                  )}
                  {data.nextCursor && (
                    <Button
                      variant="tertiary"
                      onClick={() => {
                        window.location.hash = `/requests?${requestParams(data.query, range, data.snapshot, data.nextCursor!)}`;
                      }}
                    >
                      下一页请求
                    </Button>
                  )}
                </div>
              </div>
              {data.total === 0 && (
                <div className="space-y-2">
                  <p className="text-body text-fg-muted">
                    当前范围没有已记录的{unit}
                    。请调整筛选，并查看来源授权与读取状态。
                  </p>
                  <div className="flex flex-wrap gap-2">
                    {range !== "all" && (
                      <Button
                        size="sm"
                        variant="tertiary"
                        onClick={() => change({ range: "all" })}
                      >
                        查看全部日期
                      </Button>
                    )}
                    {!nativeView &&
                      (data.sourceReport?.native.requests ?? 0) > 0 && (
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() =>
                            change({ source: "native", provider: undefined })
                          }
                        >
                          查看原生响应
                        </Button>
                      )}
                  </div>
                </div>
              )}
            </TabPanel>
          </Tabs>
          <Accordion className="w-full" type="multiple">
            <AccordionItem value="coverage">
              <AccordionTrigger>记录范围与统计口径</AccordionTrigger>
              <AccordionContent>
                <p className="text-label text-fg-muted">{data.coverage}</p>
              </AccordionContent>
            </AccordionItem>
          </Accordion>
        </>
      )}
    </div>
  );
}
