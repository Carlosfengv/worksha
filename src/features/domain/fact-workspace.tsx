import { ObjectContributionSlots } from "@/features/plugins/object-detail-slots";
import { useEffect, useMemo, useRef, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Button } from "@/components/ui/button";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { useSharedQuery } from "@/shared/hooks/use-shared-read";
import { formatDateTime } from "@/shared/lib/date-time";
import { unifiedFactsResource, factConversationResource } from "./queries";
import type {
  FactQuery,
  FactKind,
  FactRecord,
} from "../../../contracts/domain/facts/types";
import type { QueryScope } from "./types";
import { domainApi } from "./api";

const numbers = (value: string | null | undefined) =>
  value == null ? "未知" : BigInt(value).toLocaleString("zh-CN");
const rowIdentity = (record: FactRecord) =>
  JSON.stringify([record.identity.sourceId, record.id]);
const kinds: Record<FactKind, string> = {
  project: "项目证据",
  session: "会话",
  response: "模型响应",
  activity: "历史活动",
  write: "写入报告",
  "mcp-catalog": "MCP 目录",
  "mcp-execution": "MCP 执行",
};
function label(record: FactRecord) {
  const fact = record.value;
  switch (fact.kind) {
    case "project":
      return fact.value.name;
    case "session":
      return fact.value.title;
    case "response":
      return fact.value.model ?? "模型未知";
    case "activity":
      return {
        running: "来源报告运行中",
        thinking: "思考",
        generating: "生成",
        tool: "工具",
        idle: "空闲",
        unknown: "未知",
      }[fact.value.phase];
    case "write":
      return fact.value.relativePath;
    case "mcp-catalog":
      return fact.value.serverName;
    case "mcp-execution":
      return fact.value.toolName;
  }
}
const columns: ColumnDef<FactRecord>[] = [
  {
    id: "title",
    header: "记录",
    cell: ({ row }) => (
      <div className="max-w-80 whitespace-normal break-words">
        <p>{label(row.original)}</p>
        <p className="text-label text-fg-muted">
          {kinds[row.original.value.kind]}
        </p>
      </div>
    ),
  },
  {
    id: "evidence",
    header: "证据",
    cell: ({ row }) => (
      <span className="text-label">
        {row.original.identity.evidence === "builtin-native"
          ? "本机记录"
          : "来源报告"}
      </span>
    ),
  },
];
type Selection = Pick<
  FactQuery,
  "kind" | "sessionId" | "projectId" | "responseId"
>;
type FactView = {
  selection: Selection;
  pages: { snapshotToken: string | null; cursor: string | null }[];
  selected: string | null;
  snapshotToken: string | null;
};
const initialView = (selection: Selection): FactView => ({
  selection,
  pages: [{ snapshotToken: null, cursor: null }],
  selected: null,
  snapshotToken: null,
});
export function FactWorkspace({
  scope,
  sessionId = null,
  names,
  onClose,
  returnLabel = "返回会话列表",
  initialKind = "project",
}: {
  scope: QueryScope;
  sessionId?: string | null;
  names: Record<string, string>;
  onClose: () => void;
  returnLabel?: string;
  initialKind?: FactKind;
}) {
  const [history, setHistory] = useState<FactView[]>([
    initialView({
      kind: initialKind,
      sessionId,
      projectId: null,
      responseId: null,
    }),
  ]);
  const view = history[history.length - 1];
  const { selection } = view;
  const navigate = (next: Selection) =>
    setHistory((old) => {
      const current = old[old.length - 1];
      return [
        ...old.slice(0, -1),
        {
          ...current,
          pages: current.pages.map((page) => ({
            ...page,
            snapshotToken: current.snapshotToken ?? page.snapshotToken,
          })),
        },
        initialView(next),
      ];
    });
  return (
    <section className="flex min-h-0 flex-1 flex-col" aria-label="统一来源事实">
      <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border p-2">
        <span className="text-label text-fg-muted">
          {selection.responseId
            ? "单个响应关联范围"
            : selection.sessionId
              ? "单个会话关联范围"
              : selection.projectId
                ? "单个项目关联范围"
                : "所选来源全部事实"}
        </span>
        <Button variant="tertiary" size="sm" onClick={onClose}>
          {returnLabel}
        </Button>
        <Button
          variant="tertiary"
          size="sm"
          disabled={history.length < 2}
          onClick={() => setHistory((old) => old.slice(0, -1))}
        >
          返回上个事实范围
        </Button>
        <Select
          value={selection.kind}
          onValueChange={(kind) => {
            if (kind in kinds)
              navigate({ ...selection, kind: kind as FactKind });
          }}
        >
          <SelectTrigger aria-label="事实种类" />
          <SelectContent>
            {Object.entries(kinds).map(([kind, title]) => (
              <SelectItem key={kind} value={kind}>
                {title}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Button
          variant="tertiary"
          size="sm"
          onClick={() =>
            navigate({
              kind: selection.kind,
              sessionId: null,
              projectId: null,
              responseId: null,
            })
          }
        >
          查看所选来源全部事实
        </Button>
      </div>
      <FactPane
        key={JSON.stringify([scope, selection, history.length])}
        scope={scope}
        view={view}
        onChange={(change) =>
          setHistory((old) => [
            ...old.slice(0, -1),
            { ...old[old.length - 1], ...change },
          ])
        }
        names={names}
        navigate={navigate}
      />
    </section>
  );
}
function FactPane({
  scope,
  view,
  onChange,
  names,
  navigate,
}: {
  scope: QueryScope;
  view: FactView;
  onChange: (change: Partial<Omit<FactView, "selection">>) => void;
  names: Record<string, string>;
  navigate: (selection: Selection) => void;
}) {
  const { pages, selected, selection } = view;
  const pane = useRef<HTMLDivElement>(null);
  const restoreFocus = useRef(selected !== null);
  const resource = useSharedQuery(unifiedFactsResource, {
    scope,
    ...selection,
    ...pages[pages.length - 1],
  });
  const page = resource.loading || resource.error ? null : resource.data;
  const rows = useMemo(() => page?.items ?? [], [page]);
  const active = rows.find((r) => rowIdentity(r) === selected);
  useEffect(() => {
    if (page && page.snapshotToken !== view.snapshotToken)
      onChange({ snapshotToken: page.snapshotToken });
  }, [page, view.snapshotToken, onChange]);
  useEffect(() => {
    if (!restoreFocus.current || !active) return;
    const row = pane.current?.querySelector<HTMLElement>(
      'tr[aria-selected="true"]',
    );
    if (row) {
      row.focus();
      restoreFocus.current = false;
    }
  }, [active]);
  const scopedColumns = useMemo<ColumnDef<FactRecord>[]>(
    () => [
      ...columns,
      {
        id: "source",
        header: "来源",
        cell: ({ row }) => (
          <span className="block max-w-40 whitespace-normal break-words text-label">
            {names[row.original.identity.sourceId] ??
              row.original.identity.sourceId}
          </span>
        ),
      },
    ],
    [names],
  );
  const { table } = useDataTable({
    data: rows,
    columns: scopedColumns,
    getRowId: rowIdentity,
    manualPagination: true,
    manualSorting: true,
    manualFiltering: true,
    enableSorting: false,
    enableRowSelection: false,
    enableHiding: false,
  });
  const reset = () => {
    onChange({
      pages: [{ snapshotToken: null, cursor: null }],
      selected: null,
      snapshotToken: null,
    });
    void resource.invalidate();
  };
  return (
    <div ref={pane} className="flex min-h-0 flex-1 flex-col">
      <div
        tabIndex={0}
        aria-label="完整事实范围摘要"
        className="max-h-32 shrink-0 space-y-2 overflow-y-auto border-b border-border p-2 text-label"
      >
        <p>
          各来源保留独立小计。会话累计量和逐响应量分别计算，外部自报响应 ID
          不建立网关关联。
        </p>
        {page?.sourceStates.map((s) => (
          <p key={s.sourceId} className="break-words text-fg-muted">
            {names[s.sourceId] ?? s.sourceId} ·{" "}
            {s.coverage === "complete"
              ? "当前范围已核查"
              : s.coverage === "partial"
                ? "范围不完整或已过期"
                : "此类事实不可用"}
            {s.problem ? " · 需要核查来源能力或授权" : ""}
          </p>
        ))}
        {page?.sourceSummaries.map((s) => (
          <p key={s.sourceId} className="break-words">
            {names[s.sourceId] ?? s.sourceId}：项目 {numbers(s.projects)} · 会话{" "}
            {numbers(s.sessions.records)}（累计{" "}
            {numbers(s.sessions.usage.totalTokens)} Token） · 模型响应{" "}
            {numbers(s.responses.records)}（逐响应{" "}
            {numbers(s.responses.usage.totalTokens)} Token） · 历史活动{" "}
            {numbers(s.activities)} · 成功写入报告{" "}
            {numbers(s.successfulWriteReports)} · MCP 目录{" "}
            {numbers(s.mcpCatalogs)} / 执行 {numbers(s.mcpExecutions)}
            。缺少用量字段时只汇总已知值。
          </p>
        ))}
        {!!page?.hiddenProjects.length && (
          <div className="space-y-1">
            <p>当前来源已隐藏的项目：</p>
            {page.hiddenProjects.map((project) => (
              <ProjectVisibilityButton
                key={project.id}
                sourceId={project.sourceId}
                projectId={project.id}
                hidden={false}
                title={`恢复显示 ${project.name}`}
                onChanged={reset}
              />
            ))}
          </div>
        )}
      </div>
      {resource.error && (
        <div role="alert" className="shrink-0 p-2 text-label">
          <p>{resource.error}</p>
          <Button variant="tertiary" onClick={reset}>
            重新查询事实范围
          </Button>
        </div>
      )}
      <div className="grid min-h-0 flex-1 grid-cols-2 overflow-hidden">
        <div className="flex min-h-0 min-w-0 flex-col border-r border-border">
          <DataTable
            table={table}
            showPagination={false}
            className="min-h-0 flex-1"
            isLoading={resource.loading}
            aria-label="来源事实记录"
            emptyMessage={
              page?.sourceStates.every((s) => s.coverage === "complete")
                ? "当前范围没有此类事实"
                : "当前范围尚无可读事实"
            }
            activeRowId={active ? rowIdentity(active) : undefined}
            onRowActivate={(row) =>
              onChange({ selected: rowIdentity(row.original) })
            }
          />
          <div className="flex shrink-0 flex-wrap gap-2 border-t border-border p-2">
            <Button
              variant="tertiary"
              size="sm"
              disabled={resource.loading || pages.length < 2}
              onClick={() => {
                onChange({ pages: pages.slice(0, -1), selected: null });
              }}
            >
              上一页事实
            </Button>
            <Button
              variant="tertiary"
              size="sm"
              disabled={resource.loading || !page?.nextCursor}
              onClick={() => {
                const next = page?.nextCursor;
                if (page && next) {
                  onChange({
                    pages: [
                      ...pages.map((p) => ({
                        ...p,
                        snapshotToken: page.snapshotToken,
                      })),
                      {
                        snapshotToken: page.snapshotToken,
                        cursor: next,
                      },
                    ],
                    selected: null,
                  });
                }
              }}
            >
              下一页事实
            </Button>
            <Button variant="tertiary" size="sm" onClick={reset}>
              重查事实
            </Button>
          </div>
        </div>
        <div
          className="min-h-0 min-w-0 overflow-y-auto p-3"
          tabIndex={0}
          aria-label="来源事实详情"
        >
          {active ? (
            <FactDetail
              key={rowIdentity(active)}
              record={active}
              scope={scope}
              snapshotToken={page?.snapshotToken ?? ""}
              navigate={navigate}
              onChanged={reset}
            />
          ) : (
            <p className="text-label text-fg-muted">
              选择记录查看证据和关联。普通查询不会读取正文或刷新来源。
            </p>
          )}
        </div>
      </div>
    </div>
  );
}
function FactDetail({
  record,
  scope,
  snapshotToken,
  navigate,
  onChanged,
}: {
  record: FactRecord;
  scope: QueryScope;
  snapshotToken: string;
  navigate: (selection: Selection) => void;
  onChanged: () => void;
}) {
  const fact = record.value;
  const links: { kind: FactKind; id: string; title: string }[] = [];
  if (fact.kind === "session") {
    if (fact.value.projectRef)
      links.push({
        kind: "project",
        id: fact.value.projectRef,
        title: "查看关联项目",
      });
    links.push({ kind: "response", id: record.id, title: "查看会话响应" });
  }
  if (fact.kind === "project")
    links.push({ kind: "session", id: record.id, title: "查看项目会话" });
  if ("sessionRef" in fact.value && fact.value.sessionRef)
    links.push({
      kind: "session",
      id: fact.value.sessionRef,
      title: "定位所属会话",
    });
  if ("responseRef" in fact.value && fact.value.responseRef)
    links.push({
      kind: "response",
      id: fact.value.responseRef,
      title: "定位关联响应",
    });
  const go = (link: (typeof links)[number]) => {
    navigate({
      kind: link.kind,
      projectId:
        fact.kind === "project"
          ? record.id
          : link.kind === "project"
            ? link.id
            : null,
      sessionId:
        fact.kind === "session" && link.kind === "response"
          ? record.id
          : link.kind === "session" && fact.kind !== "project"
            ? link.id
            : null,
      responseId:
        link.kind === "response" && fact.kind !== "session" ? link.id : null,
    });
  };
  return (
    <article className="space-y-3 text-label">
      <h3 className="break-words text-body font-medium">{label(record)}</h3>
      <p className="break-words text-fg-muted">
        {record.identity.evidence === "source-reported"
          ? "来源报告，未核实本机项目或网关执行"
          : "本机读取证据"}{" "}
        · 采集于 {formatDateTime(record.identity.observedAt)}
      </p>
      {fact.kind === "project" && (
        <p className="break-all">
          {record.identity.evidence === "source-reported"
            ? "来源报告路径："
            : "本机项目位置："}
          {fact.value.pathHint ?? "未知"}
          {record.identity.evidence === "source-reported"
            ? "。此路径不用于本机文件、Git 或监听读取。"
            : "。此视图读取已有索引。"}
        </p>
      )}
      {fact.kind === "project" &&
        record.identity.evidence === "source-reported" && (
          <ProjectVisibilityButton
            sourceId={record.identity.sourceId}
            projectId={record.id}
            hidden
            title="隐藏此来源报告项目"
            onChanged={onChanged}
          />
        )}
      {fact.kind === "response" && (
        <>
          <p>
            结果：
            {fact.value.outcome === "success"
              ? "成功"
              : fact.value.outcome === "failure"
                ? "失败"
                : "未知"}{" "}
            · {numbers(fact.value.usage.totalTokens)} Token · 耗时{" "}
            {numbers(fact.value.durationMs)} 毫秒
          </p>
          <p className="break-all">
            来源自报响应 ID：{fact.value.reportedResponseId ?? "未知"}
            ；网关关联未核实。
          </p>
        </>
      )}
      {fact.kind === "write" && (
        <p>
          来源报告写入结果：
          {fact.value.outcome === "success"
            ? "成功"
            : fact.value.outcome === "failure"
              ? "失败"
              : "未知"}
          ，不据此声称 Git 已提交。
        </p>
      )}
      {fact.kind === "activity" && (
        <p>
          历史活动时长 {numbers(fact.value.durationMs)}{" "}
          毫秒；不能据此生成实时速率。
        </p>
      )}
      {fact.kind === "mcp-catalog" && (
        <p className="break-words">
          目录工具：
          {fact.value.toolNames
            ? fact.value.toolNames.join("、") || "无"
            : "未知，尚未读取工具目录"}
          ；配置或目录不代表已连接或发生调用。
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        {links.map((link, i) => (
          <Button key={i} variant="tertiary" size="sm" onClick={() => go(link)}>
            {link.title}
          </Button>
        ))}
      </div>
      {fact.kind === "session" && (
        <FactSessionBody record={record} snapshotToken={snapshotToken} />
      )}
      {(fact.kind === "project" || fact.kind === "session") && (
        <ObjectContributionSlots
          object={{
            target: fact.kind,
            id: record.id,
            origin: {
              kind: "unified",
              scope,
              sourceId: record.identity.sourceId,
              snapshotToken,
            },
          }}
        />
      )}
    </article>
  );
}
function ProjectVisibilityButton({
  sourceId,
  projectId,
  hidden,
  title,
  onChanged,
}: {
  sourceId: string;
  projectId: string;
  hidden: boolean;
  title: string;
  onChanged: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const save = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await domainApi.projectVisibility({ sourceId, projectId, hidden });
      onChanged();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  return (
    <span className="inline-flex max-w-full flex-wrap items-center gap-2">
      <Button
        variant="tertiary"
        size="sm"
        disabled={busy}
        onClick={() => void save()}
      >
        {title}
      </Button>
      {error && (
        <span role="alert" className="break-words text-label text-fg-muted">
          {error}
        </span>
      )}
    </span>
  );
}
function FactSessionBody({
  record,
  snapshotToken,
}: {
  record: FactRecord;
  snapshotToken: string;
}) {
  const [requested, setRequested] = useState(false);
  const [cursors, setCursors] = useState<(string | null)[]>([null]);
  const body = useSharedQuery(
    factConversationResource,
    {
      sessionId: record.id,
      snapshotToken,
      cursor: cursors[cursors.length - 1],
      anchor: null,
    },
    requested,
  );
  const conversation =
    body.loading || body.error ? null : body.data?.conversation;
  return (
    <section className="space-y-3" aria-label="事实会话正文">
      <Button
        variant="tertiary"
        size="sm"
        disabled={requested}
        onClick={() => setRequested(true)}
      >
        按当前授权读取此会话正文
      </Button>
      {body.loading && requested && (
        <p role="status">正在核查会话并读取正文…</p>
      )}
      {body.error && <p role="alert">{body.error}。请重新查询事实范围。</p>}
      {conversation?.events.map((event) => (
        <div key={event.id} className="space-y-1 border-b border-border py-2">
          <p className="text-fg-muted">{event.role}</p>
          {[event.text, event.input, event.output]
            .filter((text): text is string => text !== null)
            .map((text, index) => (
              <p key={index} className="whitespace-pre-wrap break-words">
                {text}
              </p>
            ))}
        </div>
      ))}
      <div className="flex flex-wrap gap-2">
        {requested && (
          <Button
            variant="tertiary"
            size="sm"
            disabled={body.loading || cursors.length < 2}
            onClick={() => setCursors((old) => old.slice(0, -1))}
          >
            上一页正文
          </Button>
        )}
        {conversation?.nextCursor && (
          <Button
            variant="tertiary"
            size="sm"
            disabled={body.loading}
            onClick={() => {
              const next = conversation.nextCursor;
              if (next) setCursors((old) => [...old, next]);
            }}
          >
            下一页正文
          </Button>
        )}
      </div>
    </section>
  );
}
