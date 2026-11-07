import { ObjectContributionSlots } from "@/features/plugins/object-detail-slots";
import { useDeferredValue, useMemo, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import { Input } from "@/components/ui/input";
import { useSharedRead, useSharedQuery } from "@/shared/hooks/use-shared-read";
import { useBackendReadEpoch } from "@/shared/hooks/use-backend-read-epoch";
import { formatDateTime } from "@/shared/lib/date-time";
import {
  domainDirectoryResource,
  unifiedConversationResource,
  unifiedSessionsResource,
} from "./queries";
import type { QueryScope, SessionItem, UnifiedSessionPage } from "./types";
import { FactWorkspace } from "./fact-workspace";
import { ActivityWorkspace } from "./activity-workspace";
import { RequestWorkspace } from "./request-workspace";

export const domainNumber = (value: string | null | undefined) =>
  value === null || value === undefined
    ? "未知"
    : BigInt(value).toLocaleString("zh-CN");
const stateNames: Record<string, string> = {
  authorized: "已授权",
  required: "待授权",
  revoked: "授权已失效",
  unread: "未读取",
  current: "已读取",
  stale: "已过期",
  failed: "读取失败",
  complete: "范围已核查",
  partial: "范围不完整",
  unavailable: "范围不可用",
};
const columnsFor = (
  names: Record<string, string>,
): ColumnDef<SessionItem>[] => [
  {
    id: "title",
    header: "会话",
    cell: ({ row }) => (
      <div className="max-w-80 whitespace-normal break-words">
        <p>{row.original.record.title}</p>
        <p className="text-label text-fg-muted">
          {formatDateTime(
            row.original.record.updatedAt ?? row.original.record.createdAt,
          )}{" "}
          · {domainNumber(row.original.record.usage.totalTokens)} Token
        </p>
      </div>
    ),
  },
  {
    id: "source",
    header: "来源",
    cell: ({ row }) => (
      <span
        title={row.original.sourceId}
        className="block max-w-48 whitespace-normal break-words text-label"
      >
        {names[row.original.sourceId] ?? row.original.sourceId}
      </span>
    ),
  },
];
export function UnifiedWorkspace({
  initialSource,
  initialProduct,
  defaultAll = false,
}: {
  initialSource?: string;
  initialProduct?: string;
  defaultAll?: boolean;
}) {
  const directory = useSharedRead(domainDirectoryResource);
  const epoch = useBackendReadEpoch();
  const [identity, setIdentity] = useState({ epoch, revision: 0 });
  if (identity.epoch !== epoch)
    setIdentity({ epoch, revision: identity.revision + 1 });
  const [selection, setSelection] = useState<string[] | null>(
    initialSource ? [initialSource] : defaultAll ? null : [],
  );
  const sources = useMemo(
    () =>
      selection ??
      (directory.data?.catalog.sources ?? []).slice(0, 16).map((s) => s.id),
    [selection, directory.data],
  );
  const [search, setSearch] = useState("");
  const [choosing, setChoosing] = useState(!initialSource);
  const names = useMemo(
    () =>
      Object.fromEntries(
        (directory.data?.catalog.sources ?? []).map((s) => [
          s.id,
          directory.data?.catalog.products
            .filter((p) => p.sourceId === s.id)
            .map((p) => p.displayName)
            .join(" / ") || s.id,
        ]),
      ),
    [directory.data],
  );
  const query = useDeferredValue(search);
  const ids = useMemo(
    () =>
      sources
        .filter((id) =>
          directory.data?.catalog.sources.some((s) => s.id === id),
        )
        .sort(),
    [sources, directory.data],
  );
  const builtinIds = new Set(
    directory.data?.catalog.providers
      .filter((p) => p.executionKind === "builtin")
      .map((p) => p.providerKey),
  );
  const builtinCount = ids.filter((id) =>
    directory.data?.catalog.sources.some(
      (s) => s.id === id && builtinIds.has(s.providerKey),
    ),
  ).length;
  const scope: QueryScope = {
    kind: builtinCount
      ? builtinCount === ids.length
        ? "builtin"
        : "combined"
      : "external",
    sourceIds: ids,
  };
  return (
    <section aria-label="统一来源会话" className="flex h-full min-h-0 flex-col">
      <div className="shrink-0 space-y-2 border-b border-border p-2">
        {!initialProduct && (
          <details
            open={choosing}
            onToggle={(event) => setChoosing(event.currentTarget.open)}
          >
            <summary className="cursor-pointer text-label font-medium">
              选择来源 · 已选 {ids.length} 个
            </summary>
            <div
              className="flex max-h-24 flex-wrap gap-3 overflow-y-auto py-2"
              aria-label="统一会话来源选择"
            >
              {directory.data?.catalog.sources.map((source) => (
                <label
                  key={source.id}
                  className="flex min-w-0 items-center gap-2 text-label"
                >
                  <Checkbox
                    checked={sources.includes(source.id)}
                    disabled={
                      !sources.includes(source.id) && sources.length >= 16
                    }
                    onCheckedChange={(checked) =>
                      setSelection((old) => {
                        const current = old ?? sources;
                        return checked === true
                          ? [...new Set([...current, source.id])]
                          : current.filter((id) => id !== source.id);
                      })
                    }
                  />
                  <span className="break-words">
                    {names[source.id]} ·{" "}
                    {stateNames[source.status.authorization]}
                  </span>
                </label>
              ))}
              <Button asChild variant="tertiary" size="sm">
                <a href="#/agents">管理原生来源</a>
              </Button>
            </div>
            <p className="text-label text-fg-subtle">
              普通查询不会刷新来源；会话累计用量不计作模型调用次数。
            </p>
            {defaultAll && (
              <Button asChild variant="tertiary" size="sm">
                <a href="#/sessions?scope=local">管理本机客户端会话</a>
              </Button>
            )}
          </details>
        )}
        <div className="flex min-w-0 items-center gap-2">
          <Input
            aria-label="搜索统一会话"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="搜索会话标题或来源记录 ID"
          />
          <Button
            variant="tertiary"
            size="sm"
            onClick={() => void directory.invalidate()}
          >
            重读目录
          </Button>
          <Button asChild variant="tertiary" size="sm">
            <a href="#/plugins">管理授权</a>
          </Button>
        </div>
        {directory.loading && !directory.data && (
          <p role="status" className="text-label text-fg-muted">
            正在读取来源目录…
          </p>
        )}
        {(directory.error || directory.data?.externalProblem) && (
          <p role="alert" className="text-label text-fg-muted">
            扩展目录暂不可用。{directory.error}
          </p>
        )}
      </div>
      {ids.length ? (
        <UnifiedPane
          key={`${identity.revision}:${directory.data?.catalog.revision}:${JSON.stringify(scope)}:${query}`}
          scope={scope}
          search={query}
          names={names}
          productId={initialProduct ?? null}
        />
      ) : (
        <p className="p-4 text-body text-fg-muted">
          请选择要查询的来源。未授权来源保留不可用状态。
        </p>
      )}
    </section>
  );
}
function UnifiedPane({
  scope,
  search,
  names,
  productId,
}: {
  scope: QueryScope;
  search: string;
  names: Record<string, string>;
  productId: string | null;
}) {
  const [trail, setTrail] = useState<
    { snapshotToken: string | null; cursor: string | null }[]
  >([{ snapshotToken: null, cursor: null }]);
  const [selected, setSelected] = useState<string | null>(null);
  const [facts, setFacts] = useState<{ sessionId: string | null } | null>(null);
  const [activity, setActivity] = useState<{ sessionId: string | null } | null>(
    null,
  );
  const [requests, setRequests] = useState<{ sessionId: string | null } | null>(
    null,
  );
  const current = trail[trail.length - 1];
  const resource = useSharedQuery(unifiedSessionsResource, {
    scope,
    filter: { query: search || null, productId, days: null },
    ...current,
  });
  const page = resource.error || resource.loading ? null : resource.data;
  const rows = useMemo(() => page?.items ?? [], [page]);
  const active = rows.find((r) => r.record.id === selected);
  const columns = useMemo(() => columnsFor(names), [names]);
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (r) => r.record.id,
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
  if (requests)
    return (
      <RequestWorkspace
        scope={scope}
        productId={productId}
        sessionId={requests.sessionId}
        names={names}
        onClose={() => setRequests(null)}
      />
    );
  if (activity)
    return (
      <ActivityWorkspace
        scope={scope}
        productId={productId}
        sessionId={activity.sessionId}
        names={names}
        onClose={() => setActivity(null)}
      />
    );
  if (facts)
    return (
      <FactWorkspace
        scope={scope}
        sessionId={facts.sessionId}
        names={names}
        onClose={() => setFacts(null)}
      />
    );
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {page && (
        <div
          tabIndex={0}
          aria-label="统一范围统计"
          className="max-h-24 shrink-0 space-y-1 overflow-y-auto border-b border-border p-2 text-label"
        >
          <p>
            {stateNames[page.coverage]} · 查询于 {formatDateTime(page.asOf)}
          </p>
          {page.sourceStates.map((s) => (
            <p key={s.sourceId} className="break-all text-fg-muted">
              {names[s.sourceId] ?? s.sourceId}：{stateNames[s.authorization]} ·{" "}
              {stateNames[s.freshness]}
              {s.problem ? " · 需要核查来源状态" : ""}
            </p>
          ))}
          <SourceSummaries page={page} names={names} />
        </div>
      )}
      {resource.error && (
        <div role="alert" className="shrink-0 space-y-2 p-3 text-label">
          <p>{resource.error}</p>
          <Button variant="tertiary" onClick={reset}>
            重新查询当前范围
          </Button>
        </div>
      )}
      <div className="grid min-h-0 flex-1 grid-cols-2 overflow-hidden">
        <div className="flex min-h-0 min-w-0 flex-col border-r border-border">
          <DataTable
            table={table}
            showPagination={false}
            isLoading={resource.loading && !page}
            loadingMessage="正在查询统一会话"
            emptyMessage={
              page?.coverage === "complete"
                ? "没有匹配会话"
                : "当前范围尚无可读会话"
            }
            activeRowId={active?.record.id}
            onRowActivate={(row) => setSelected(row.original.record.id)}
            className="min-h-0 flex-1"
          />
          <div className="flex shrink-0 flex-wrap items-center gap-2 border-t border-border p-2">
            <Button
              variant="tertiary"
              size="sm"
              disabled={resource.loading || trail.length < 2}
              onClick={() => {
                setTrail((old) => old.slice(0, -1));
                setSelected(null);
              }}
            >
              上一页会话
            </Button>
            <Button
              variant="tertiary"
              size="sm"
              disabled={resource.loading || !page?.nextCursor}
              onClick={() => {
                const nextCursor = page?.nextCursor;
                if (page && nextCursor) {
                  setTrail((old) => [
                    ...old.map((p) => ({
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
              下一页会话
            </Button>
            <Button variant="tertiary" size="sm" onClick={reset}>
              重新查询
            </Button>
            <Button
              variant="tertiary"
              size="sm"
              onClick={() => setFacts({ sessionId: active?.record.id ?? null })}
            >
              {active ? "查看此会话项目与响应" : "查看项目与响应事实"}
            </Button>
            <Button
              variant="tertiary"
              size="sm"
              onClick={() =>
                setActivity({ sessionId: active?.record.id ?? null })
              }
            >
              {active ? "查看此会话活动" : "查看活动快照"}
            </Button>
            <span className="text-label text-fg-muted">
              <Button
                variant="tertiary"
                size="sm"
                onClick={() =>
                  setRequests({ sessionId: active?.record.id ?? null })
                }
              >
                {active ? "查看此会话请求" : "查看统一请求"}
              </Button>
            </span>
            <span className="text-label text-fg-muted">
              本页 {rows.length} 个
            </span>
          </div>
        </div>
        {page && active ? (
          <UnifiedBody
            key={`${page.snapshotToken}:${active.record.id}`}
            item={active}
            scope={scope}
            snapshotToken={page.snapshotToken}
            sourceName={names[active.sourceId] ?? active.sourceId}
          />
        ) : (
          <p className="p-4 text-body text-fg-muted">
            选择会话后按需读取正文。
          </p>
        )}
      </div>
    </div>
  );
}
function SourceSummaries({
  page,
  names,
}: {
  page: UnifiedSessionPage;
  names: Record<string, string>;
}) {
  return (
    <div className="space-y-1">
      {page.sourceSummaries.map((s) => (
        <p key={s.sourceId} className="break-all">
          {names[s.sourceId] ?? s.sourceId}：{domainNumber(s.sessions)} 个会话 ·
          已知会话累计 {domainNumber(s.usage.totalTokens)} Token ·{" "}
          {domainNumber(s.knownUsageSessions)} 个会话有已知用量
        </p>
      ))}
      {page.overlapUnknown && (
        <p className="text-fg-muted">
          各来源保留独立小计，跨来源重叠情况未知。
        </p>
      )}
    </div>
  );
}
function UnifiedBody({
  item,
  scope,
  snapshotToken,
  sourceName,
}: {
  item: SessionItem;
  scope: QueryScope;
  snapshotToken: string;
  sourceName: string;
}) {
  const [cursors, setCursors] = useState<(string | null)[]>([null]);
  const resource = useSharedQuery(unifiedConversationResource, {
    sessionId: item.record.id,
    snapshotToken,
    cursor: cursors[cursors.length - 1],
    anchor: null,
  });
  const body =
    resource.error || resource.loading
      ? undefined
      : resource.data?.conversation;
  return (
    <section
      className="flex min-h-0 min-w-0 flex-col"
      aria-label="统一会话正文"
    >
      <div className="shrink-0 space-y-2 border-b border-border p-3">
        <h3 className="break-words text-body font-medium">
          {item.record.title}
        </h3>
        <p className="break-all text-label text-fg-muted">
          {sourceName} · {domainNumber(item.record.usage.totalTokens)} Token
        </p>
        <div className="flex flex-wrap gap-2">
          <Button
            variant="tertiary"
            size="sm"
            disabled={resource.loading || cursors.length < 2}
            onClick={() => setCursors((old) => old.slice(0, -1))}
          >
            上一页正文
          </Button>
          <Button
            variant="tertiary"
            size="sm"
            disabled={resource.loading || !body?.nextCursor}
            onClick={() => {
              const next = body?.nextCursor;
              if (next) setCursors((old) => [...old, next]);
            }}
          >
            下一页正文
          </Button>
        </div>
      </div>
      <div
        key={cursors.length}
        tabIndex={0}
        aria-label="统一正文滚动区域"
        className="min-h-0 flex-1 space-y-3 overflow-y-auto p-3"
      >
        <ObjectContributionSlots
          object={{
            target: "session",
            id: item.record.id,
            origin: {
              kind: "unified",
              scope,
              sourceId: item.sourceId,
              snapshotToken,
            },
          }}
        />
        {resource.loading && !body && <p role="status">正在读取正文…</p>}
        {resource.error ? (
          <div role="alert" className="space-y-2">
            <p>{resource.error}</p>
            <Button
              variant="tertiary"
              onClick={() => void resource.invalidate()}
            >
              重试读取正文
            </Button>
          </div>
        ) : (
          body?.events.map((event) => (
            <article
              key={event.id}
              className="space-y-2 rounded-xl border border-border p-3"
            >
              <p className="text-label text-fg-muted">
                {event.role === "assistant"
                  ? "模型回复"
                  : event.role === "user"
                    ? "用户"
                    : event.kind}
              </p>
              {[event.text, event.input, event.output]
                .filter((s): s is string => s !== null)
                .map((text, n) => (
                  <p
                    key={n}
                    className="whitespace-pre-wrap break-words text-body"
                  >
                    {text}
                  </p>
                ))}
            </article>
          ))
        )}
        {body && body.events.length === 0 && !resource.error && (
          <p className="text-body text-fg-muted">此会话没有已记录正文。</p>
        )}
        {body && !resource.error && (
          <p className="text-label text-fg-muted">
            本页 {body.events.length} 条{body.complete ? " · 已到末页" : ""}
          </p>
        )}
      </div>
    </section>
  );
}
