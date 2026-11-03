import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { useBackendReadEpoch } from "@/shared/hooks/use-backend-read-epoch";
import { backendReadEpoch } from "@/shared/lib/backend-recovery";
import { useEffect, useMemo, useRef, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { agentsApi } from "../api";
import type { MemoryFile, MemorySnapshot } from "../types";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { bytes, date } from "@/features/sessions/display";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import { dataTablePagination } from "@/shared/ui/data-table-labels";

export function AgentMemory({
  agentId,
  enabled,
}: {
  agentId: string;
  enabled: boolean;
}) {
  const { agentNames } = useAgentDirectory();
  const epoch = useBackendReadEpoch(enabled);
  const [fileId, setFileId] = useState<string>();
  const previewRef = useRef<HTMLElement>(null);
  const [search, setSearch] = useState("");
  const [revision, setRevision] = useState(0);
  const [state, setState] = useState<{
    key: string;
    data: MemorySnapshot | null;
    error: string | null;
    loading: boolean;
  }>({ key: "", data: null, error: null, loading: false });
  const key = `${agentId}:${fileId ?? ""}:${enabled}`;
  useEffect(() => {
    if (!enabled) return;
    let alive = true;
    setState((old) => ({
      key,
      data:
        old.data?.agentId === agentId ? { ...old.data, selected: null } : null,
      error: null,
      loading: true,
    }));
    void agentsApi
      .memory(agentId, fileId)
      .then((data) => {
        if (alive && epoch === backendReadEpoch())
          setState({ key, data, error: null, loading: false });
      })
      .catch((reason: unknown) => {
        if (alive && epoch === backendReadEpoch())
          setState({
            key,
            data: null,
            error: reason instanceof Error ? reason.message : String(reason),
            loading: false,
          });
      });
    return () => {
      alive = false;
    };
  }, [agentId, fileId, enabled, revision, key, epoch]);
  const data = enabled && state.key === key ? state.data : null;
  useEffect(() => {
    if (data?.selected?.id)
      previewRef.current?.scrollIntoView({
        block: "nearest",
        inline: "nearest",
      });
  }, [data?.selected?.id]);
  const items = useMemo(
    () =>
      (data?.files ?? []).filter((f) =>
        `${f.name} ${f.path} ${f.scope} ${f.project ?? ""}`
          .toLocaleLowerCase()
          .includes(search.toLocaleLowerCase()),
      ),
    [data, search],
  );
  const columns = useMemo<ColumnDef<MemoryFile>[]>(
    () => [
      {
        accessorKey: "name",
        header: "文件",
        cell: ({ row }) => (
          <>
            <span className="block max-w-64 whitespace-normal break-words">
              {row.original.name}
            </span>
            <p
              className="max-w-96 truncate text-label text-fg-muted"
              title={row.original.path}
            >
              {row.original.path}
            </p>
          </>
        ),
      },
      {
        accessorKey: "scope",
        header: "范围",
        cell: ({ row }) => (
          <>
            {row.original.scope}
            <p className="text-label text-fg-muted">已发现 · 加载情况未知</p>
          </>
        ),
      },
      {
        accessorKey: "modifiedAt",
        header: "更新时间",
        cell: ({ row }) => (
          <>
            {date(row.original.modifiedAt)}
            <p className="text-label text-fg-muted">
              {bytes(row.original.sizeBytes)}
            </p>
          </>
        ),
      },
      {
        id: "read",
        header: "操作",
        cell: ({ row }) => (
          <Button
            variant="tertiary"
            size="sm"
            aria-label={`查看 ${row.original.name}`}
            onClick={() => setFileId(row.original.id)}
          >
            查看
          </Button>
        ),
      },
    ],
    [],
  );
  const { table } = useDataTable({
    data: items,
    columns,
    getRowId: (f) => f.id,
    enableRowSelection: false,
    initialState: { pagination: { pageIndex: 0, pageSize: 10 } },
  });
  if (!enabled)
    return (
      <ConnectionEmpty
        title="尚未授权记忆来源"
        description="在连接与观测中启用只读来源后查看。"
      />
    );
  return (
    <section className="space-y-4" aria-label="Agent 记忆">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h3 className="text-title font-medium">记忆与规则</h3>
        <Button
          variant="tertiary"
          size="sm"
          onClick={() => setRevision((r) => r + 1)}
          loading={state.loading}
        >
          重新读取记忆
        </Button>
      </div>
      {state.key === key && state.error && (
        <p role="alert" className="text-body text-fg-warning">
          {state.error}
        </p>
      )}
      {data && (
        <>
          <p className="text-label text-fg-muted">
            {data.files.length} 个文件 · {date(data.asOf)}
            {data.limited ? " · 部分目录，已达读取上限" : ""}
          </p>
          {data.sharedAgents.length > 1 && (
            <p className="text-label text-fg-muted">
              共享来源：
              {data.sharedAgents.map((id) => agentNames[id] ?? id).join("、")}
              ；不代表两端均已加载。
            </p>
          )}
          <Accordion>
            <AccordionItem value="coverage">
              <AccordionTrigger>读取范围</AccordionTrigger>
              <AccordionContent>
                <div className="space-y-2">
                  {data.warnings.map((warning) => (
                    <p key={warning} className="text-label text-fg-muted">
                      {warning}
                    </p>
                  ))}
                </div>
              </AccordionContent>
            </AccordionItem>
          </Accordion>
        </>
      )}
      <Input
        aria-label="搜索记忆文件"
        placeholder="搜索文件或路径"
        value={search}
        onChange={(e) => setSearch(e.target.value)}
      />
      <DataTable
        table={table}
        role="region"
        aria-label="记忆文件列表"
        isLoading={state.loading && !data}
        loadingMessage="正在发现记忆文件"
        emptyMessage={state.error ? "记忆读取失败" : "未发现匹配的记忆文件"}
        activeRowId={fileId}
        onRowActivate={(row) => setFileId(row.original.id)}
        paginationProps={{
          labels: dataTablePagination,
        }}
      />
      {data?.selected && data.selected.id === fileId && (
        <section
          ref={previewRef}
          aria-label="记忆正文"
          className="space-y-3 rounded-xl border border-border p-4"
        >
          <div className="flex items-start justify-between gap-3">
            <h4 className="break-all text-title font-medium">
              {data.files.find((f) => f.id === fileId)?.name}
            </h4>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setFileId(undefined)}
            >
              关闭正文
            </Button>
          </div>
          <p className="text-label text-fg-muted">
            真实采集 · 当前文件，只读；不代表历史会话使用了这个版本。
          </p>
          {data.selected.truncated && (
            <p className="text-label text-fg-warning">
              正文仅显示前部，已截断。
            </p>
          )}
          <pre className="max-h-96 overflow-auto whitespace-pre-wrap break-words text-code">
            {data.selected.text || "空文件"}
          </pre>
        </section>
      )}
    </section>
  );
}
