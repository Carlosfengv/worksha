import { useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Button } from "@/components/ui/button";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { AppShell, AppShellMain } from "@/components/ui/app-shell";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { useTablePagination } from "@/shared/hooks/use-table-pagination";
import { dataTablePagination } from "@/shared/ui/data-table-labels";
import { iamResources } from "./queries";
import { PasswordDialog } from "./account-dialog";
import { IamResourceDetail } from "./resource-detail";
import { useIam } from "./context";
import { useIamAction } from "./actions";
import type { Account, Resource } from "./types";

const columns: ColumnDef<Resource>[] = [
  { accessorKey: "name", header: "模型服务" },
];
const empty: Resource[] = [];

export function MemberWorkspace({ account }: { account: Account }) {
  const session = useIam();
  const resource = useSharedRead(iamResources);
  const [selected, setSelected] = useState("");
  const [password, setPassword] = useState(false);
  const task = useIamAction();
  const rows = resource.data ?? empty;
  const pagination = useTablePagination(rows.length, 10);
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (item) => item.id,
    enableRowSelection: false,
    autoResetPageIndex: false,
    state: { pagination: pagination.value },
    onPaginationChange: pagination.onChange,
  });
  return (
    <div className="flex h-dvh flex-col bg-surface-base">
      <div className="h-9 shrink-0" data-tauri-drag-region />
      <AppShell layout="stacked" className="min-h-0 flex-1">
        <AppShellMain className="overflow-y-auto">
          <section className="mx-auto max-w-5xl space-y-5 p-5 sm:p-8">
            <header className="flex flex-wrap items-center justify-between gap-3">
              <div className="min-w-0">
                <h1 className="text-title font-medium">我的资源</h1>
                <p className="break-words text-label text-fg-muted">
                  {account.displayName} · Member
                </p>
              </div>
              <div className="flex gap-2">
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={() => setPassword(true)}
                >
                  修改密码
                </Button>
                <Button
                  size="sm"
                  variant="tertiary"
                  disabled={task.busy}
                  onClick={() =>
                    void task.run(async () => {
                      await session.logout?.();
                    })
                  }
                >
                  退出登录
                </Button>
              </div>
            </header>
            {(task.error || resource.error) && (
              <Alert status="danger">
                <AlertDescription>
                  {task.error ?? resource.error}{" "}
                  <Button
                    variant="tertiary"
                    size="sm"
                    onClick={() => void resource.reload()}
                  >
                    重试
                  </Button>
                </AlertDescription>
              </Alert>
            )}
            {!resource.error && (
              <DataTable
                table={table}
                aria-label="我的模型服务"
                onRowActivate={(row) => setSelected(row.original.id)}
                isLoading={resource.loading && !resource.data}
                loadingMessage="正在读取资源"
                emptyMessage="还没有分配给你的模型服务"
                showPagination={rows.length > 10}
                paginationProps={{ labels: dataTablePagination }}
              />
            )}
          </section>
        </AppShellMain>
      </AppShell>
      {selected && (
        <IamResourceDetail id={selected} onClose={() => setSelected("")} />
      )}
      {password && (
        <PasswordDialog account={account} onClose={() => setPassword(false)} />
      )}
    </div>
  );
}
