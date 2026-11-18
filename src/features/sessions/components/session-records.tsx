import { useMemo } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import {
  date,
  number,
  projectLabel,
  sessionAgentLabel,
  sessionStorage,
} from "../display";
import type { NativeSession } from "../types";

export function SessionRecords({
  items,
  href,
}: {
  items: NativeSession[];
  href: (session: NativeSession) => string;
}) {
  const { agentNames } = useAgentDirectory();
  const columns = useMemo<ColumnDef<NativeSession>[]>(
    () => [
      {
        id: "title",
        header: "会话",
        cell: ({ row: { original: session } }) => (
          <a
            className="block max-w-96 truncate font-medium hover:underline"
            title={session.title}
            href={href(session)}
            data-workspace-resource
            data-workspace-title={session.title}
          >
            {session.title}
          </a>
        ),
      },
      {
        id: "agent",
        header: "Agent / 项目",
        cell: ({ row: { original: session } }) => (
          <span className="block max-w-56 truncate text-label">
            {sessionAgentLabel(session, agentNames)} ·{" "}
            {session.projectHidden ? "项目已隐藏" : projectLabel(session)}
          </span>
        ),
      },
      {
        id: "time",
        header: "最近更新",
        cell: ({ row }) => date(row.original.updatedAt),
      },
      {
        id: "usage",
        header: "用量与占用",
        cell: ({ row: { original: session } }) =>
          session.status === "observed"
            ? `${number(session.modelCalls)} 条模型请求`
            : `${number(session.usage.totalTokens)} Token · ${sessionStorage(session)}`,
      },
    ],
    [agentNames, href],
  );
  const { table } = useDataTable({
    data: items,
    columns,
    getRowId: (session) => session.id,
    manualPagination: true,
    enableRowSelection: false,
    enableSorting: false,
    enableHiding: false,
  });
  return (
    <DataTable
      table={table}
      role="region"
      aria-label="会话记录"
      showPagination={false}
      onRowActivate={(row) => {
        window.location.hash = href(row.original);
      }}
      emptyMessage="没有匹配会话"
    />
  );
}
