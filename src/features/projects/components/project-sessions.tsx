import { useTablePagination } from "@/shared/hooks/use-table-pagination";
import { Button } from "@/components/ui/button";
import {
  DataTable,
  DataTableColumnHeader,
  useDataTable,
} from "@/components/ui/data-table";
import type { ProjectDetail } from "@/features/projects/types";
import {
  sessionAgentLabel,
  bytes,
  date,
  number,
  sessionLink,
} from "@/features/sessions/display";
import type { NativeSession } from "@/features/sessions/types";
import {
  dataTableControls,
  dataTablePagination,
} from "@/shared/ui/data-table-labels";
import type { ColumnDef } from "@tanstack/react-table";
import { useMemo } from "react";
import { projectLink } from "../links";
import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";

type ProjectSessionRow = { session: NativeSession; detail: ProjectDetail };
const projectSessionColumns = (
  agentNames: Record<string, string>,
): ColumnDef<ProjectSessionRow>[] => [
  {
    id: "session",
    accessorFn: (row) => row.session.title,
    header: ({ column }) => (
      <DataTableColumnHeader
        column={column}
        label="会话 / Agent"
        labels={dataTableControls}
      />
    ),
    cell: ({ row }) => {
      const { session } = row.original;
      return (
        <>
          <p className="max-w-80 break-words font-medium">{session.title}</p>
          <p className="mt-1 text-label text-fg-muted">
            {sessionAgentLabel(session, agentNames)} ·{" "}
            {number(session.turnCount)} 轮 · 已导入 · {date(session.createdAt)}
          </p>
        </>
      );
    },
  },
  {
    id: "tokens",
    header: () => <>累计 Token</>,
    enableSorting: false,
    cell: ({ row }) => {
      const { session } = row.original;
      return (
        <>
          {number(session.usage.totalTokens)}
          <p className="text-label text-fg-subtle">
            {["partial", "unknown"].includes(session.usageQuality)
              ? "未知 / 部分用量"
              : "原生报告"}
          </p>
        </>
      );
    },
  },
  {
    id: "storage",
    header: () => <>会话文件占用</>,
    enableSorting: false,
    cell: ({ row }) => {
      const { session } = row.original;
      return <>{bytes(session.diskAllocatedBytes)}</>;
    },
  },
  {
    id: "writes",
    header: () => <>成功写入</>,
    enableSorting: false,
    cell: ({ row }) => {
      const { session, detail } = row.original;
      return (
        <>
          {detail.writes.filter((w) => w.sessionId === session.id).length}{" "}
          条已确认
        </>
      );
    },
  },
  {
    id: "actions",
    header: () => <>操作</>,
    enableSorting: false,
    cell: ({ row }) => {
      const { session, detail } = row.original;
      return (
        <>
          <Button
            variant="tertiary"
            size="sm"
            onClick={() => {
              window.location.hash = sessionLink(session.id, {
                agent: detail.appliedFilter.agent,
              });
            }}
          >
            打开对话
          </Button>
          {detail.writes.some((w) => w.sessionId === session.id) && (
            <Button
              variant="tertiary"
              size="sm"
              onClick={() => {
                window.location.hash = projectLink(
                  detail.project.id,
                  "changes",
                  detail.writes.find((w) => w.sessionId === session.id)!.id,
                  detail.appliedFilter.agent,
                  detail.appliedFilter.scope,
                );
              }}
            >
              定位写入
            </Button>
          )}
        </>
      );
    },
  },
];
export function ProjectSessions({ detail }: { detail: ProjectDetail }) {
  const { agentNames } = useAgentDirectory();
  const columns = useMemo(
    () => projectSessionColumns(agentNames),
    [agentNames],
  );
  const rows = useMemo(
    () => detail.sessions.map((session) => ({ session, detail })),
    [detail],
  );

  const pagination = useTablePagination(detail.sessions.length, 40);
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (row) => row.session.id,
    enableRowSelection: false,
    enableHiding: false,
    autoResetPageIndex: false,
    state: { pagination: pagination.value },
    onPaginationChange: pagination.onChange,
  });
  return (
    <>
      <p className="text-label text-fg-muted">
        {detail.sessions.length} 个所选会话 ·
        此处用量与占用是整段共享会话的上下文，不分配给每个 Agent；所选 Agent
        的去重 Token 见总览。状态仅表示原生记录已导入。
      </p>
      {detail.sessions.length ? (
        <DataTable
          table={table}
          role="region"
          aria-label="项目会话"
          paginationProps={{
            labels: dataTablePagination,
            pageSizeOptions: [20, 40, 80],
          }}
        />
      ) : (
        <p className="text-body text-fg-muted">
          所选日期没有会话，当前 Git 与端口仍可查看。
        </p>
      )}
    </>
  );
}
