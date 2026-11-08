import { useId, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
} from "@/components/ui/combobox";
import { useSharedQuery, useSharedRead } from "@/shared/hooks/use-shared-read";
import { useTablePagination } from "@/shared/hooks/use-table-pagination";
import { dataTablePagination } from "@/shared/ui/data-table-labels";
import { AccountDialog, PasswordDialog } from "./account-dialog";
import { accountsResource, grantsResource, iamResources } from "./queries";
import { iamApi } from "./api";
import { useIamAction } from "./actions";
import { useIam } from "./context";
import type { Account, Resource } from "./types";

const columns: ColumnDef<Account>[] = [
  { accessorKey: "displayName", header: "显示名" },
  { accessorKey: "username", header: "登录名" },
  {
    accessorKey: "role",
    header: "角色",
    cell: ({ row }) => (row.original.role === "owner" ? "Owner" : "Member"),
  },
  {
    id: "enabled",
    header: "状态",
    cell: ({ row }) => (
      <Badge status={row.original.enabled ? "success" : "neutral"}>
        {row.original.enabled ? "已启用" : "已停用"}
      </Badge>
    ),
  },
];
const empty: Account[] = [];

function ResourceGrants({ account }: { account: Account }) {
  const id = useId();
  const grants = useSharedQuery(grantsResource, account.id);
  const resources = useSharedRead(iamResources);
  const [selected, setSelected] = useState<Resource | null>(null);
  const task = useIamAction();
  const current = grants.data ?? [];
  const all = resources.data ?? [];
  const choices = all.filter((item) => !current.includes(item.id));
  const value = choices.find((item) => item.id === selected?.id) ?? null;
  const change = (serviceId: string, allow: boolean) =>
    void task.run(async () => {
      await iamApi.grant(account.id, serviceId, allow);
      if (allow) setSelected(null);
      await grants.reload();
    });
  return (
    <section className="space-y-3" aria-labelledby={id}>
      <h3 id={id} className="text-body font-medium">
        已关联资源
      </h3>
      <p className="text-label text-fg-muted">
        分配的模型服务可供成员查看摘要。
      </p>
      {(grants.error || resources.error || task.error) && (
        <Alert status="danger">
          <AlertDescription>
            {task.error ?? grants.error ?? resources.error}
          </AlertDescription>
        </Alert>
      )}
      {grants.loading && !grants.data ? (
        <p role="status">正在读取资源关联…</p>
      ) : (
        !grants.error &&
        (current.length ? (
          <ul className="divide-y divide-border">
            {current.map((serviceId) => (
              <li
                key={serviceId}
                className="flex items-center justify-between gap-2 py-2 text-body"
              >
                <span className="min-w-0 break-words">
                  {all.find((item) => item.id === serviceId)?.name ??
                    "模型服务"}
                </span>
                <Button
                  variant="tertiary"
                  size="sm"
                  disabled={task.busy}
                  onClick={() => change(serviceId, false)}
                >
                  撤回
                  <span className="sr-only">
                    {" "}
                    {all.find((item) => item.id === serviceId)?.name}
                  </span>
                </Button>
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-label text-fg-muted">暂无已关联资源。</p>
        ))
      )}
      {!grants.error && !resources.error && grants.data && resources.data && (
        <div className="flex flex-wrap items-center gap-2">
          <Combobox
            items={choices}
            value={value}
            onValueChange={setSelected}
            itemToStringLabel={(item) => item.name}
            itemToStringValue={(item) => item.id}
          >
            <ComboboxInput
              aria-label="选择模型服务"
              placeholder="选择模型服务"
            />
            <ComboboxContent>
              <ComboboxEmpty>没有可分配的模型服务</ComboboxEmpty>
              <ComboboxList>
                {(item: Resource) => (
                  <ComboboxItem key={item.id} value={item}>
                    {item.name}
                  </ComboboxItem>
                )}
              </ComboboxList>
            </ComboboxContent>
          </Combobox>
          <Button
            size="sm"
            disabled={!value || task.busy}
            onClick={() => {
              if (value) change(value.id, true);
            }}
          >
            添加资源
          </Button>
        </div>
      )}
    </section>
  );
}

export function AccountsPage() {
  const session = useIam();
  const accounts = useSharedRead(accountsResource);
  const [selected, setSelected] = useState("");
  const [dialog, setDialog] = useState<
    "new" | "edit" | "password" | "delete" | null
  >(null);
  const task = useIamAction();
  const rows = accounts.data ?? empty;
  const active = rows.find((account) => account.id === selected);
  const pagination = useTablePagination(rows.length, 10);
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (account) => account.id,
    enableSorting: false,
    enableRowSelection: false,
    autoResetPageIndex: false,
    state: { pagination: pagination.value },
    onPaginationChange: pagination.onChange,
  });
  const refresh = async () => {
    await accounts.reload();
    await session.refresh?.();
  };
  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-body text-fg-muted">
          管理本地账号、角色和模型服务访问。
        </p>
        <Button
          disabled={!!accounts.error || !accounts.data}
          onClick={() => setDialog("new")}
        >
          新建账号
        </Button>
      </div>
      {accounts.error ? (
        <Alert status="danger">
          <AlertDescription>
            {accounts.error}{" "}
            <Button
              variant="tertiary"
              size="sm"
              onClick={() => void accounts.reload()}
            >
              重试
            </Button>
          </AlertDescription>
        </Alert>
      ) : (
        <>
          <DataTable
            table={table}
            aria-label="账号"
            activeRowId={selected}
            onRowActivate={(row) => setSelected(row.original.id)}
            isLoading={accounts.loading && !accounts.data}
            loadingMessage="正在读取账号"
            emptyMessage="暂无账号"
            showPagination={rows.length > 10}
            paginationProps={{ labels: dataTablePagination }}
          />
          {active && (
            <section
              className="space-y-4 rounded-xl border border-border p-4"
              aria-label="账号详情"
            >
              <div className="flex flex-wrap items-center justify-between gap-2">
                <h2 className="text-title font-medium">{active.displayName}</h2>
                <div className="flex flex-wrap gap-2">
                  <Button
                    size="sm"
                    variant="secondary"
                    onClick={() => setDialog("edit")}
                  >
                    编辑账号
                  </Button>
                  <Button
                    size="sm"
                    variant="tertiary"
                    onClick={() => setDialog("password")}
                  >
                    {active.id === session.status?.account?.id
                      ? "修改密码"
                      : "重置密码"}
                  </Button>
                  <Button
                    size="sm"
                    variant="tertiary"
                    onClick={() => setDialog("delete")}
                  >
                    删除账号
                  </Button>
                </div>
              </div>
              <ResourceGrants key={active.id} account={active} />
            </section>
          )}
        </>
      )}
      {(dialog === "new" || (dialog === "edit" && active)) && (
        <AccountDialog
          account={dialog === "edit" ? active : undefined}
          onClose={() => setDialog(null)}
          onSaved={async (saved) => {
            setSelected(saved.id);
            await refresh();
          }}
        />
      )}
      {dialog === "password" && active && (
        <PasswordDialog
          account={active}
          onClose={() => setDialog(null)}
          onSaved={refresh}
        />
      )}
      {dialog === "delete" && active && (
        <Dialog
          open
          onOpenChange={(open) => {
            if (!open && !task.busy) setDialog(null);
          }}
        >
          <DialogContent>
            <DialogHeader>
              <DialogTitle>删除账号</DialogTitle>
              <DialogDescription>
                删除「{active.displayName}」将撤销该账号的登录资格与已分配资源。
              </DialogDescription>
            </DialogHeader>
            {task.error && (
              <Alert status="danger">
                <AlertDescription>{task.error}</AlertDescription>
              </Alert>
            )}
            <DialogFooter>
              <Button
                variant="secondary"
                disabled={task.busy}
                onClick={() => setDialog(null)}
              >
                取消
              </Button>
              <Button
                variant="destructive"
                disabled={task.busy}
                onClick={() =>
                  void task.run(async () => {
                    await iamApi.remove(active.id, active.revision);
                    setDialog(null);
                    setSelected("");
                    await refresh();
                  })
                }
              >
                删除账号
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      )}
    </div>
  );
}
