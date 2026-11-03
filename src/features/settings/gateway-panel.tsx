import { useMemo, useState, useLayoutEffect, useRef } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Field, FieldLabel } from "@/components/ui/field";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import {
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
} from "@/components/ui/combobox";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import { availableProviderModelIds } from "@/features/providers/catalog";
import type { Provider } from "@/features/providers/types";
import { useTablePagination } from "@/shared/hooks/use-table-pagination";
import { dataTablePagination } from "@/shared/ui/data-table-labels";
import { gatewayStatusResource } from "./queries";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import type { GatewayClient, GatewayClientDraft } from "./types";
import { useGatewayOperations } from "./use-gateway-operations";
import { GatewayOperationRecovery } from "./components/gateway-operation-recovery";

type Row = GatewayClient & {
  revoke: (client: GatewayClient) => void;
  busy: boolean;
};
const columns: ColumnDef<Row>[] = [
  {
    accessorKey: "name",
    header: "客户端",
    cell: ({ row }) => (
      <>
        <p>{row.original.name}</p>
        <p className="text-label text-fg-muted">{row.original.agent}</p>
      </>
    ),
  },
  {
    id: "models",
    header: "授权模型",
    cell: ({ row }) => (
      <span className="break-all">{row.original.models.join("、")}</span>
    ),
  },
  {
    id: "capacity",
    header: "并发",
    cell: ({ row }) => `${row.original.active} / ${row.original.concurrency}`,
  },
  {
    id: "state",
    header: "状态",
    cell: ({ row }) => (row.original.enabled ? "已启用" : "已撤销"),
  },
  {
    id: "actions",
    header: "操作",
    cell: ({ row }) => (
      <Button
        variant="tertiary"
        size="sm"
        disabled={!row.original.enabled || row.original.busy}
        onClick={() => row.original.revoke(row.original)}
      >
        撤销密钥
      </Button>
    ),
  },
];

export function GatewayPanel({
  providers,
  storeId,
}: {
  providers: Provider[];
  storeId?: string;
}) {
  const resource = useSharedRead(gatewayStatusResource);
  const status = resource.data;
  const [name, setName] = useState("");
  const [agent, setAgent] = useState<GatewayClientDraft["agent"]>("claude");
  const [providerId, setProviderId] = useState<string | null>(null);
  const [model, setModel] = useState<string | null>(null);
  const [concurrency, setConcurrency] = useState("2");
  const [draftScope, setDraftScope] = useState<string>();
  const [revoking, setRevoking] = useState<{
    client: GatewayClient;
    storeId: string;
  } | null>(null);
  const currentScope =
    !resource.loading && !resource.error && status?.writeProtocol?.version === 1
      ? status.writeProtocol.storeId
      : undefined;
  const formScope = currentScope === storeId ? currentScope : undefined;
  const formIdentity =
    status?.writeProtocol?.storeId === storeId ? storeId : undefined;
  const operations = useGatewayOperations(currentScope, resource.invalidate);
  const { busy, error, notice: message, pending } = operations;
  const ownPending = pending.requests.some(
    (r) => r.request.storeId === currentScope,
  );
  const canWrite =
    !!currentScope &&
    !!status?.persistent &&
    pending.available &&
    !pending.invalid &&
    !pending.atCapacity &&
    !ownPending;
  const fieldsDisabled =
    busy ||
    !canWrite ||
    !formScope ||
    (!!draftScope && draftScope !== formScope);
  const committed = useRef({
    scope: formIdentity,
    draftScope,
    draft: {
      name,
      agent,
      providerId,
      models: model ? [model] : [],
      concurrency: Number(concurrency),
    },
  });
  useLayoutEffect(() => {
    committed.current = {
      scope: formIdentity,
      draftScope,
      draft: {
        name,
        agent,
        providerId,
        models: model ? [model] : [],
        concurrency: Number(concurrency),
      },
    };
  });
  const beginEdit = () => {
    if (fieldsDisabled || !formScope || committed.current.scope !== formScope)
      return false;
    setDraftScope((old) => old ?? formScope);
    return true;
  };
  const resetDraft = () => {
    setName("");
    setAgent("claude");
    setProviderId(null);
    setModel(null);
    setConcurrency("2");
    setDraftScope(undefined);
  };
  const provider = providers.find((p) => p.id === providerId);
  const models = provider ? availableProviderModelIds(provider) : [];
  const rows = useMemo(
    () =>
      (status?.clients ?? []).map((c) => ({
        ...c,
        revoke: (client: GatewayClient) => {
          if (currentScope) setRevoking({ client, storeId: currentScope });
        },
        busy: busy || !canWrite,
      })),
    [status?.clients, busy, canWrite, currentScope],
  );
  const pagination = useTablePagination(rows.length, 10);
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (row) => row.id,
    autoResetPageIndex: false,
    enableRowSelection: false,
    enableHiding: false,
    state: { pagination: pagination.value },
    onPaginationChange: pagination.onChange,
  });
  return (
    <section className="space-y-4" aria-label="独立网关与局域网客户端">
      <div className="flex items-center justify-between gap-3">
        <h2 className="text-title font-medium">独立网关与局域网客户端</h2>
        <Button
          variant="tertiary"
          disabled={busy || resource.loading}
          onClick={() => void resource.invalidate()}
        >
          刷新状态
        </Button>
      </div>
      {(error || resource.error) && (
        <p role="alert" className="text-body text-fg-muted">
          {error ?? resource.error}；已有状态可能已过期。
        </p>
      )}
      {message && <p role="status">{message}</p>}
      <GatewayOperationRecovery
        operations={operations}
        storeId={currentScope}
      />
      {!status ? (
        <p role="status">
          {error || resource.error ? "网关状态不可用" : "正在读取网关状态…"}
        </p>
      ) : (
        <>
          {resource.loading && (
            <p role="status">正在更新网关状态，以下为上次记录。</p>
          )}
          <p className="text-body text-fg-muted">
            {status.persistent
              ? "常驻服务运行中，退出桌面应用后继续提供模型调用。"
              : "当前服务随桌面应用运行。"}
          </p>
          {status.listen ? (
            <>
              <p className="break-all text-body">
                模型地址：http://{status.listen}/v1 · 局域网并发 {status.active}{" "}
                / {status.concurrency}
              </p>
              <p className="text-body text-fg-muted">
                每个客户端使用独立密钥和并发额度。管理接口只接受本机访问；本机请求使用独立的并发额度。
              </p>
              <DataTable
                table={table}
                paginationProps={{ labels: dataTablePagination }}
                emptyMessage="尚未创建局域网客户端"
              />
              <form
                className="space-y-3"
                onSubmit={(event) => {
                  event.preventDefault();
                  if (
                    !provider ||
                    !model ||
                    !models.includes(model) ||
                    !draftScope ||
                    draftScope !== formScope ||
                    !canWrite ||
                    busy
                  )
                    return;
                  const draft: GatewayClientDraft = {
                    name,
                    agent,
                    providerId: provider.id,
                    models: [model],
                    concurrency: Number(concurrency),
                  };
                  const originalScope = draftScope;
                  void operations
                    .startCreate(draft, originalScope)
                    .then((saved) => {
                      const now = committed.current;
                      if (
                        saved &&
                        now.scope === originalScope &&
                        now.draftScope === originalScope &&
                        JSON.stringify(now.draft) === JSON.stringify(draft)
                      )
                        resetDraft();
                    });
                }}
              >
                <h3 className="text-body font-medium">添加客户端</h3>
                <fieldset
                  disabled={fieldsDisabled}
                  className="grid gap-3 sm:grid-cols-2"
                >
                  <Field>
                    <FieldLabel htmlFor="gateway-client-name">
                      客户端名称
                    </FieldLabel>
                    <Input
                      id="gateway-client-name"
                      value={name}
                      required
                      maxLength={80}
                      onChange={(e) => {
                        if (beginEdit()) setName(e.target.value);
                      }}
                      placeholder="例如：工作室 Mac"
                    />
                  </Field>
                  <Field>
                    <FieldLabel>Agent</FieldLabel>
                    <Select
                      value={agent}
                      onValueChange={(v) => {
                        if (
                          beginEdit() &&
                          (v === "claude" || v === "opencode" || v === "codex")
                        )
                          setAgent(v);
                      }}
                    >
                      <SelectTrigger aria-label="客户端 Agent" />
                      <SelectContent>
                        <SelectItem value="claude">Claude Code</SelectItem>
                        <SelectItem value="opencode">OpenCode</SelectItem>
                        <SelectItem value="codex">Codex</SelectItem>
                      </SelectContent>
                    </Select>
                  </Field>
                  <Field>
                    <FieldLabel>Provider</FieldLabel>
                    <Combobox<string>
                      items={providers.map((p) => p.id)}
                      value={providerId}
                      itemToStringLabel={(id) =>
                        providers.find((p) => p.id === id)?.name ?? id
                      }
                      onValueChange={(id) => {
                        if (beginEdit()) {
                          setProviderId(id);
                          setModel(null);
                        }
                      }}
                      limit={200}
                    >
                      <ComboboxInput
                        aria-label="客户端 Provider"
                        placeholder="搜索供应商"
                      />
                      <ComboboxContent>
                        <ComboboxEmpty>没有已保存的供应商</ComboboxEmpty>
                        <ComboboxList>
                          {(id: string) => (
                            <ComboboxItem key={id} value={id}>
                              {providers.find((p) => p.id === id)?.name ?? id}
                            </ComboboxItem>
                          )}
                        </ComboboxList>
                      </ComboboxContent>
                    </Combobox>
                  </Field>
                  <Field>
                    <FieldLabel>可调用模型</FieldLabel>
                    <Combobox<string>
                      items={models}
                      value={model}
                      onValueChange={(value) => {
                        if (beginEdit()) setModel(value);
                      }}
                      limit={200}
                    >
                      <ComboboxInput
                        aria-label="客户端模型"
                        placeholder="选择一个已启用模型"
                      />
                      <ComboboxContent>
                        <ComboboxEmpty>
                          请先选择供应商及已启用模型
                        </ComboboxEmpty>
                        <ComboboxList>
                          {(id: string) => (
                            <ComboboxItem key={id} value={id}>
                              {id}
                            </ComboboxItem>
                          )}
                        </ComboboxList>
                      </ComboboxContent>
                    </Combobox>
                  </Field>
                  <Field>
                    <FieldLabel>客户端并发上限</FieldLabel>
                    <Select
                      value={concurrency}
                      onValueChange={(v) => {
                        if (v && beginEdit()) setConcurrency(v);
                      }}
                    >
                      <SelectTrigger aria-label="客户端并发上限" />
                      <SelectContent>
                        {[1, 2, 4, 8, 16].map((n) => (
                          <SelectItem key={n} value={String(n)}>
                            {n}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </Field>
                </fieldset>
                {!formScope && (
                  <p role="status" className="text-body text-fg-muted">
                    {currentScope
                      ? "客户端列表来源已确认，供应商草稿来源尚未确认；新的签发已停用，原操作可显式核查。"
                      : "客户端来源或回执协议尚未确认，请重新读取原后台；新的签发与撤销已停用。"}
                  </p>
                )}
                {draftScope && draftScope !== formScope && (
                  <p
                    role={
                      currentScope && draftScope !== currentScope
                        ? "alert"
                        : "status"
                    }
                    className="text-body text-fg-muted"
                  >
                    {currentScope && draftScope !== currentScope
                      ? "客户端草稿属于原后台，未带到当前后台。请返回原后台，或明确放弃这份未发送草稿。"
                      : "客户端来源尚在核对，原草稿已保留，暂不发送新的签发。"}
                  </p>
                )}
                {draftScope && (
                  <Button
                    type="button"
                    variant="tertiary"
                    disabled={busy}
                    onClick={resetDraft}
                  >
                    放弃未发送的客户端草稿
                  </Button>
                )}
                <p className="text-label text-fg-muted">
                  连接文件包含此客户端的密钥，保存后交给对应设备。撤销后拒绝新请求，已开始的请求可继续完成。
                </p>
                <Button
                  type="submit"
                  disabled={
                    fieldsDisabled ||
                    !draftScope ||
                    !name.trim() ||
                    !model ||
                    !models.includes(model)
                  }
                  loading={busy}
                >
                  创建并保存连接文件
                </Button>
              </form>
            </>
          ) : (
            <p className="text-body text-fg-muted">
              此服务尚未开启局域网共享。
            </p>
          )}
        </>
      )}
      <Dialog
        open={revoking !== null}
        onOpenChange={(open) => {
          if (!open && !busy) setRevoking(null);
        }}
      >
        <DialogContent>
          <DialogTitle>撤销客户端密钥</DialogTitle>
          <DialogDescription>
            撤销“{revoking?.client.name}
            ”后，此设备不能再发起新请求。其他客户端和已有请求继续运行。
          </DialogDescription>
          <DialogFooter>
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() => setRevoking(null)}
            >
              取消
            </Button>
            <Button
              disabled={busy || !canWrite || revoking?.storeId !== currentScope}
              onClick={() => {
                const original = revoking;
                if (original)
                  void operations
                    .startRevoke(original.client.id, original.storeId)
                    .then(() => setRevoking(null));
              }}
            >
              确认撤销
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
