import { useMemo, useState, useLayoutEffect, useRef, useEffect } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Button } from "@/components/ui/button";
import { InputCopy } from "@/components/ui/input-copy";
import { providerCosts } from "@/features/providers/usage-display";
import { ClientRequestAudit } from "@/features/observation/components/client-request-audit";
import { useWorkspaceNavigation } from "@/shared/hooks/use-workspace-navigation";
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
import type {
  GatewayClient,
  GatewayClientDraft,
  GatewayClientUsage,
} from "./types";
import { useGatewayOperations } from "./use-gateway-operations";
import { GatewayOperationRecovery } from "./components/gateway-operation-recovery";

type Row = GatewayClient & { usage?: GatewayClientUsage; known: boolean };
const gatewayHref = (client: GatewayClient) =>
  `#/lan-gateway/${encodeURIComponent(client.id)}`;
const count = (value: number | null | undefined) =>
  value?.toLocaleString("zh-CN") ?? "未知";
const columns: ColumnDef<Row>[] = [
  {
    accessorKey: "name",
    header: "客户端",
    cell: ({ row: { original: client } }) => (
      <div>
        <a
          className="font-medium hover:underline"
          href={gatewayHref(client)}
          data-workspace-resource
          data-workspace-title={`密钥 · ${client.name}`}
        >
          {client.name}
        </a>
        <p className="text-label text-fg-muted">
          {client.id.slice(0, 8)} · {client.usage?.lastPeer ?? "最近 IP 未知"}
        </p>
      </div>
    ),
  },
  {
    id: "state",
    header: "状态 / 并发",
    cell: ({ row: { original: client } }) =>
      `${client.enabled ? "已启用" : "已撤销"} · ${client.active} / ${client.concurrency}`,
  },
  {
    id: "usage",
    header: "近 7 天请求 / Token",
    cell: ({ row: { original: client } }) =>
      client.known
        ? `${count(client.usage?.requests7d ?? 0)} 次 · ${count(client.usage ? client.usage.usage7d.totalTokens : 0)} tok`
        : "未知",
  },
  {
    id: "cost",
    header: "近 7 天费用",
    cell: ({ row: { original: client } }) =>
      client.usage
        ? providerCosts(client.usage.cost7d)
        : client.known
          ? "无请求"
          : "未知",
  },
];

export function GatewayPanel({
  providers,
  storeId,
  selectedId,
}: {
  providers: Provider[];
  storeId?: string;
  selectedId?: string;
}) {
  const resource = useSharedRead(gatewayStatusResource);
  const status = resource.data;
  const selected = status?.clients.find((client) => client.id === selectedId);
  const { setTitle } = useWorkspaceNavigation();
  useEffect(() => {
    if (selected) setTitle(`密钥 · ${selected.name}`);
  }, [selected, setTitle]);
  const [issuing, setIssuing] = useState(false);
  const [receipt, setReceipt] = useState<{
    key: string;
    listen: string;
    agent: GatewayClientDraft["agent"];
    storeId: string;
  } | null>(null);
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
  const rows = useMemo<Row[]>(
    () =>
      (status?.clients ?? []).map((client) => ({
        ...client,
        usage: status?.clientUsage?.[client.id],
        known: status?.clientUsage !== undefined,
      })),
    [status],
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
    <section className="space-y-3" aria-label="常驻网关与密钥">
      {!selectedId && (
        <div className="flex flex-wrap items-center justify-between gap-3">
          <h2 className="text-title font-medium">常驻网关与密钥</h2>
          {status?.listen && (
            <InputCopy
              label="监听地址"
              value={`http://${status.listen}/v1`}
              className="max-w-72"
            />
          )}
          <Button
            size="sm"
            disabled={busy || !status?.listen || !formScope || !canWrite}
            onClick={() => setIssuing(true)}
          >
            + 签发客户端密钥
          </Button>
          <Button
            variant="tertiary"
            disabled={busy || resource.loading}
            onClick={() => void resource.invalidate()}
          >
            刷新状态
          </Button>
        </div>
      )}
      {(error || resource.error) && (
        <p role="alert" className="text-body text-fg-muted">
          {error ?? resource.error}；已有状态可能已过期。
        </p>
      )}
      {message && <p role="status">{message}</p>}
      {status && !formScope && (
        <p role="status" className="text-body text-fg-muted">
          {currentScope
            ? "客户端列表来源已确认，供应商草稿来源尚未确认；新的签发已停用，原操作可显式核查。"
            : "客户端来源或回执协议尚未确认，请重新读取原后台；新的签发与撤销已停用。"}
        </p>
      )}
      <GatewayOperationRecovery
        operations={operations}
        storeId={currentScope}
      />
      {!status ? (
        <p role="status">
          {resource.error ? "网关状态不可用" : "正在读取网关状态…"}
        </p>
      ) : selectedId ? (
        selected ? (
          <>
            <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
              {[
                [
                  "近 7 天请求",
                  status.clientUsage
                    ? count(status.clientUsage[selected.id]?.requests7d ?? 0)
                    : "未知",
                ],
                [
                  "近 7 天错误",
                  status.clientUsage
                    ? count(status.clientUsage[selected.id]?.errors7d ?? 0)
                    : "未知",
                ],
                [
                  "近 7 天 429",
                  status.clientUsage
                    ? count(
                        status.clientUsage[selected.id]?.throttled429Count7d ??
                          0,
                      )
                    : "未知",
                ],
                [
                  "近 7 天费用",
                  status.clientUsage?.[selected.id]
                    ? providerCosts(status.clientUsage[selected.id].cost7d)
                    : status.clientUsage
                      ? "无请求"
                      : "未知",
                ],
              ].map(([label, value]) => (
                <div
                  key={label}
                  className="rounded-lg border border-border p-3"
                >
                  <p className="text-label text-fg-muted">{label}</p>
                  <p className="text-title">{value}</p>
                </div>
              ))}
            </div>
            <div className="flex flex-wrap items-center gap-3">
              <InputCopy label="客户端 UUID" value={selected.id} />
              <p className="text-body">
                {selected.enabled ? "已启用" : "已撤销"} ·{" "}
                {providers.find((p) => p.id === selected.providerId)?.name ??
                  selected.providerId}{" "}
                · {selected.models.join("、")}
              </p>
              <Button
                variant="secondary"
                disabled={
                  !selected.enabled || !currentScope || !canWrite || busy
                }
                onClick={() => {
                  if (currentScope)
                    setRevoking({ client: selected, storeId: currentScope });
                }}
              >
                撤销此密钥
              </Button>
            </div>
            <ClientRequestAudit key={selectedId} clientId={selectedId} />
          </>
        ) : (
          <p role="status">找不到此客户端密钥</p>
        )
      ) : (
        <>
          {!status.listen && (
            <p className="text-body text-fg-muted">
              此服务尚未开启局域网共享。
            </p>
          )}
          <DataTable
            table={table}
            paginationProps={{ labels: dataTablePagination }}
            emptyMessage="尚未创建局域网客户端"
          />
        </>
      )}
      <Dialog
        open={issuing}
        onOpenChange={(open) => {
          if (!open && !busy) setIssuing(false);
        }}
      >
        <DialogContent
          size="lg"
          aria-label="签发客户端密钥"
          className="max-h-[calc(100dvh-2rem)] overflow-y-auto"
        >
          <DialogTitle>签发客户端密钥</DialogTitle>
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
                .startCreate(
                  draft,
                  originalScope,
                  (event.nativeEvent as SubmitEvent).submitter?.getAttribute(
                    "value",
                  ) === "file"
                    ? undefined
                    : "receipt",
                )
                .then((saved) => {
                  if (
                    saved &&
                    typeof saved === "object" &&
                    saved.outcome.receipt.storeId === committed.current.scope
                  ) {
                    setReceipt({
                      key: saved.key,
                      listen: saved.outcome.receipt.listen,
                      agent: draft.agent,
                      storeId: originalScope,
                    });
                  }
                  const now = committed.current;
                  if (
                    saved &&
                    now.scope === originalScope &&
                    now.draftScope === originalScope &&
                    JSON.stringify(now.draft) === JSON.stringify(draft)
                  ) {
                    resetDraft();
                    setIssuing(false);
                  }
                });
            }}
          >
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
                    <ComboboxEmpty>请先选择供应商及已启用模型</ComboboxEmpty>
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
            {error && <p role="alert">{error}</p>}
            <DialogFooter>
              <Button
                type="button"
                variant="tertiary"
                disabled={busy}
                onClick={() => setIssuing(false)}
              >
                返回密钥列表
              </Button>
              <Button
                type="submit"
                name="delivery"
                value="file"
                variant="secondary"
                disabled={
                  fieldsDisabled ||
                  !draftScope ||
                  !name.trim() ||
                  !model ||
                  !models.includes(model)
                }
              >
                创建并保存连接文件
              </Button>
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
                签发密钥
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
      <Dialog
        open={!!receipt && receipt.storeId === formIdentity}
        onOpenChange={(open) => {
          if (!open) setReceipt(null);
        }}
      >
        <DialogContent size="lg">
          <DialogTitle>客户端密钥已签发</DialogTitle>
          <DialogDescription>
            后台仅保存密钥的 SHA-256 摘要。关闭后无法再次查看明文。
          </DialogDescription>
          {receipt && receipt.storeId === formIdentity && (
            <>
              <InputCopy label="客户端密钥" value={receipt.key} />
              <InputCopy
                label="终端环境变量"
                value={
                  receipt.agent === "claude"
                    ? `export ANTHROPIC_BASE_URL='http://${receipt.listen}'\nexport ANTHROPIC_AUTH_TOKEN='${receipt.key}'`
                    : `export OPENAI_BASE_URL='http://${receipt.listen}/v1'\nexport OPENAI_API_KEY='${receipt.key}'`
                }
              />
            </>
          )}
          <DialogFooter>
            <Button onClick={() => setReceipt(null)}>完成</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
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
