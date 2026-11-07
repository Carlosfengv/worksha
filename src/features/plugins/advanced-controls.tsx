import { useCallback, useMemo, useState } from "react";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { pluginBackupResource } from "./queries";
import type { ColumnDef } from "@tanstack/react-table";
import { Alert, AlertTitle, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Combobox,
  ComboboxInput,
  ComboboxContent,
  ComboboxList,
  ComboboxItem,
  ComboboxEmpty,
} from "@/components/ui/combobox";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import { Field, FieldLabel } from "@/components/ui/field";
import { usePollingResource } from "@/shared/hooks/use-polling-resource";
import { dataTablePagination } from "@/shared/ui/data-table-labels";
import type { Provider } from "@/features/providers/types";
import type { PluginStatus } from "./types";
import type {
  ExtensionAction,
  ExtensionCall,
  ExtensionPlan,
  PlanCapacity,
} from "./advanced-types";
import { pluginsApi } from "./api";

const stateNames: Record<string, string> = {
  prepared: "等待确认",
  applying: "结果待核查",
  applied: "已应用",
  restoring: "恢复结果待核查",
  restored: "已恢复",
  success: "成功",
  tool_error: "工具错误",
  protocol_error: "协议错误",
  transport_error: "传输错误",
  cancelled: "已取消",
  timeout: "超时",
  incomplete: "未完成",
};
type PlanAction = "apply" | "restore" | "discard";
function configAction(
  plan: ExtensionPlan,
  action: PlanAction,
): ExtensionAction {
  if (action === "discard")
    return { kind: "discard-config", planId: plan.planId };
  if (action === "restore")
    return { kind: "restore-config", operationId: plan.planId };
  return { kind: "apply-config", planId: plan.planId, revision: plan.revision };
}
const planActionNotices: Record<PlanAction, string> = {
  apply: "配置计划已执行并回读核对。",
  restore: "配置备份已恢复。",
  discard: "未执行预览已放弃，配置文件未修改，名额已释放。",
};
function PlanCapacityNotice({ capacity }: { capacity?: PlanCapacity }) {
  return (
    <p className="text-label text-fg-muted" role="status">
      {capacity
        ? `配置记录名额：${capacity.used} / ${capacity.limit}，剩余 ${capacity.remaining}。相同的有效预览会复用。可放弃不再需要的未执行预览；已应用、待核查和带备份的记录会继续保留。全部记录可在“配置备份与恢复”查看。`
        : "当前后台未提供配置记录容量，尚未确认是否支持预览复用或放弃；请核对后台版本。"}
    </p>
  );
}
const planColumns: ColumnDef<ExtensionPlan>[] = [
  { accessorKey: "providerName", header: "连接" },
  {
    accessorKey: "path",
    header: "目标文件",
    cell: ({ row }) => (
      <p className="max-w-80 break-all">{row.original.path}</p>
    ),
  },
  {
    id: "state",
    header: "状态",
    cell: ({ row }) => stateNames[row.original.state] ?? "待核查",
  },
];
const callColumns: ColumnDef<ExtensionCall>[] = [
  { accessorKey: "tool", header: "工具" },
  {
    id: "status",
    header: "结果",
    cell: ({ row }) => stateNames[row.original.status] ?? "未知",
  },
  {
    id: "duration",
    header: "观测耗时",
    cell: ({ row }) =>
      row.original.durationMs === null
        ? "未知"
        : `${row.original.durationMs.toFixed(1)} ms`,
  },
  { accessorKey: "rpcId", header: "JSON-RPC ID" },
];
function Plans({
  items,
  onSelect,
}: {
  items: ExtensionPlan[];
  onSelect: (p: ExtensionPlan) => void;
}) {
  const { table } = useDataTable({
    data: items,
    columns: planColumns,
    getRowId: (p) => p.planId,
    enableSorting: false,
    enableRowSelection: false,
  });
  return (
    <DataTable
      table={table}
      aria-label="扩展配置计划"
      emptyMessage="尚未生成配置计划"
      paginationProps={{ labels: dataTablePagination }}
      onRowActivate={(r) => onSelect(r.original)}
    />
  );
}
function Calls({ items }: { items: ExtensionCall[] }) {
  const { table } = useDataTable({
    data: items,
    columns: callColumns,
    getRowId: (p) => p.identity,
    enableSorting: false,
    enableRowSelection: false,
  });
  return (
    <DataTable
      table={table}
      aria-label="扩展 MCP 调用"
      emptyMessage="尚未记录工具执行；握手与目录请求不计入"
      paginationProps={{ labels: dataTablePagination }}
    />
  );
}
function PlanDetails({
  plan,
  onClose,
  onAction,
  busy,
  reading = false,
  readError,
  allowApply = true,
}: {
  plan: ExtensionPlan | null;
  onClose: () => void;
  onAction: (p: ExtensionPlan, action: PlanAction) => void;
  busy: boolean;
  reading?: boolean;
  readError?: string | null;
  allowApply?: boolean;
}) {
  return (
    <Dialog
      open={plan !== null}
      onOpenChange={(open) => !open && !busy && onClose()}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>配置计划与恢复</DialogTitle>
          <DialogDescription>
            执行前复核所选文件、连接版本和授权。凭据由宿主写入指定占位字段，插件不能读取原始密钥。含凭据的文件仅当前用户可读写；恢复会还原原权限。备份由宿主加密保护；恢复不会覆盖后续修改。
          </DialogDescription>
        </DialogHeader>
        {readError ? (
          <Alert status="danger">
            <AlertTitle>配置目录读取未完成</AlertTitle>
            <AlertDescription>{readError}</AlertDescription>
          </Alert>
        ) : reading ? (
          <p role="status">正在更新配置目录，暂不能执行操作。</p>
        ) : null}
        {plan && (
          <div className="max-h-96 space-y-3 overflow-y-auto">
            <p className="break-all">目标：{plan.path}</p>
            <p>
              连接：{plan.providerName} · {stateNames[plan.state] ?? "待核查"}
            </p>
            <ul className="space-y-2">
              {plan.edits.map((e) => (
                <li key={e.pointer} className="break-all text-label">
                  {e.pointer} →{" "}
                  {typeof e.value === "string"
                    ? e.value
                    : JSON.stringify(e.value)}
                </li>
              ))}
            </ul>
          </div>
        )}
        <DialogFooter>
          <Button variant="tertiary" disabled={busy} onClick={onClose}>
            关闭
          </Button>
          {allowApply && plan?.state === "prepared" && (
            <Button
              loading={busy}
              disabled={reading || !!readError}
              onClick={() => onAction(plan, "apply")}
            >
              确认执行配置计划
            </Button>
          )}
          {plan?.backupAvailable && plan.state !== "restored" && (
            <Button
              loading={busy}
              disabled={reading || !!readError}
              onClick={() => onAction(plan, "restore")}
            >
              确认恢复配置备份
            </Button>
          )}
          {plan?.canDiscard === true && (
            <Button
              variant="tertiary"
              loading={busy}
              disabled={reading || !!readError}
              onClick={() => onAction(plan, "discard")}
            >
              确认放弃未执行预览
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function AdvancedControls({
  status,
  providers,
  onChanged,
}: {
  status: PluginStatus;
  providers: Provider[];
  onChanged: () => void;
}) {
  const query = useMemo(
    () => ({ pluginId: status.manifest.id, digest: status.digest }),
    [status.manifest.id, status.digest],
  );
  const loader = useCallback(() => pluginsApi.extensionStatus(query), [query]);
  const resource = usePollingResource(
    `plugin-advanced:${query.pluginId}:${query.digest}:${status.activationEpoch}:${status.grantRevision}`,
    loader,
  );
  const [providerId, setProviderId] = useState<string | null>(null);
  const [allowPrivate, setAllowPrivate] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [plan, setPlan] = useState<ExtensionPlan | null>(null);
  const [confirmation, setConfirmation] = useState<"bind" | "start" | null>(
    null,
  );
  const data = resource.data;
  const names = new Map(
    providers.map((p) => [p.id, `${p.name} · ${p.endpoint}`]),
  );
  const run = async (work: () => Promise<unknown>, message: string) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const result = await work();
      setNotice(result === null ? "已取消，授权未变化。" : message);
      setPlan(null);
      setConfirmation(null);
      void resource.invalidate();
      onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  const act = (action: ExtensionAction) =>
    pluginsApi.extensionAction({ ...query, action });
  return (
    <section className="space-y-4" aria-label="高级扩展能力">
      {(error || resource.error) && (
        <Alert status="danger">
          <AlertTitle>高级扩展操作未完成</AlertTitle>
          <AlertDescription>{error ?? resource.error}</AlertDescription>
        </Alert>
      )}
      {!data && resource.loading && <p>正在读取高级扩展能力…</p>}
      {data?.policy.protocol && (
        <div className="space-y-3">
          <p className="text-label text-fg-muted">
            协议绑定只作用于此 Provider
            的受管网关路由；转换完整模型请求与响应。停止或升级插件后旧绑定失效，需先解除再重新绑定。
          </p>
          <p className="text-label">
            协议路径：{data.policy.protocol.path} · 基础协议：
            {data.policy.protocol.baseProtocol}
          </p>
          <p className="text-label">
            已绑定连接：
            {data.protocolBindings
              .map((id) => names.get(id) ?? id)
              .join("、") || "尚未绑定"}
          </p>
        </div>
      )}
      {(data?.policy.protocol || data?.policy.config) && (
        <Field>
          <FieldLabel>扩展使用的连接</FieldLabel>
          <Combobox<string>
            items={[...names.keys()]}
            value={providerId}
            onValueChange={setProviderId}
            itemToStringLabel={(id) => names.get(id) ?? id}
            limit={200}
          >
            <ComboboxInput
              aria-label="高级扩展连接"
              placeholder="搜索已有连接"
            />
            <ComboboxContent>
              <ComboboxEmpty>请先在 Providers 中添加连接</ComboboxEmpty>
              <ComboboxList>
                {(id: string) => (
                  <ComboboxItem key={id} value={id}>
                    {names.get(id)}
                  </ComboboxItem>
                )}
              </ComboboxList>
            </ComboboxContent>
          </Combobox>
        </Field>
      )}
      {data?.policy.protocol && (
        <>
          <label className="flex items-center gap-2 text-label">
            <Checkbox
              checked={allowPrivate}
              onCheckedChange={(v) => setAllowPrivate(v === true)}
            />
            允许此协议连接访问本机或局域网
          </label>
          <div className="flex flex-wrap gap-2">
            <Button
              disabled={busy || !status.enabled || !providerId}
              onClick={() => setConfirmation("bind")}
            >
              授权协议绑定
            </Button>
            <Button
              variant="tertiary"
              disabled={busy || data.protocolBindings.length === 0}
              onClick={() =>
                void run(
                  () => act({ kind: "unbind-protocol" }),
                  "协议绑定已解除。",
                )
              }
            >
              解除协议绑定
            </Button>
          </div>
        </>
      )}
      {(data?.policy.config || data?.policy.connector) && (
        <div className="space-y-3">
          <p className="break-all text-label">
            资源文件：{data.resourceLabel ?? "尚未选择"} ·{" "}
            {data.resourceAuthorized ? "已授权" : "未授权或已失效"}
          </p>
          {data.resourceTarget && (
            <p className="break-all text-label">
              连接目标：{data.resourceTarget}
            </p>
          )}
          <Button
            variant="tertiary"
            disabled={busy || !status.enabled}
            onClick={() =>
              void run(
                () => pluginsApi.authorizeResource(query),
                "文件已授权；尚未执行修改或启动连接。",
              )
            }
          >
            {data.policy.config
              ? "选择并授权配置文件"
              : "选择并授权 MCP 连接文件"}
          </Button>
        </div>
      )}
      {data?.policy.config && (
        <>
          <PlanCapacityNotice capacity={data.planCapacity} />
          <Button
            disabled={
              busy || !status.enabled || !data.resourceAuthorized || !providerId
            }
            loading={busy}
            onClick={() =>
              void run(
                () => act({ kind: "prepare-config", providerId: providerId! }),
                "配置预览已生成，尚未写入。",
              )
            }
          >
            生成配置预览
          </Button>
          <Plans items={data.plans} onSelect={setPlan} />
        </>
      )}
      {data?.policy.connector && (
        <div className="space-y-3">
          <p className="text-label">
            观测入口：
            {data.connector?.observationEnabled ? "已启用" : "尚未启动"} · MCP
            连接：{data.connector?.transportConnected ? "握手成功" : "尚未确认"}{" "}
            · 工具：{data.connector?.tools ?? "未知"}
          </p>
          <p className="text-label text-fg-muted">
            stdio
            服务以当前用户权限运行。启用入口不执行工具；请将导出的连接交给可信客户端。此来源独立展示，缺少跨源关联证据时不与原生
            MCP 次数或模型 Token 相加。
          </p>
          <div className="flex flex-wrap gap-2">
            <Button
              disabled={
                busy ||
                !status.enabled ||
                !data.resourceAuthorized ||
                !!data.connector?.observationEnabled
              }
              onClick={() => setConfirmation("start")}
            >
              启动 MCP 观测入口
            </Button>
            <Button
              variant="tertiary"
              disabled={busy || !data.connector?.observationEnabled}
              onClick={() =>
                void run(
                  () => act({ kind: "stop-connector" }),
                  "MCP 观测入口已停止。",
                )
              }
            >
              停止 MCP 观测入口
            </Button>
            <Button
              variant="tertiary"
              disabled={busy || !data.connector?.observationEnabled}
              onClick={() =>
                void run(
                  () => pluginsApi.exportConnector(query),
                  "连接已保存，访问凭据未返回界面。",
                )
              }
            >
              导出 MCP 连接
            </Button>
          </div>
          {data.connector && data.connector.observationIncomplete !== false && (
            <Alert status="warning">
              <AlertTitle>观测记录完整性尚未确认</AlertTitle>
              <AlertDescription>
                {data.connector.observationIncomplete === true
                  ? `${typeof data.connector.unconfirmedCalls === "number" ? `有 ${data.connector.unconfirmedCalls} 次调用` : "部分调用"}的观测记录尚未确认，当前列表和次数可能不完整。`
                  : "当前后台未提供观测完整性信息，不能据此确认没有工具调用。"}
              </AlertDescription>
            </Alert>
          )}
          {data.connector?.lastError &&
            data.connector.lastError !== "observation_unavailable" && (
              <p className="text-label text-warning-fg">
                传输需要检查：{data.connector.lastError}
              </p>
            )}
          <p className="text-label text-fg-muted">
            最近 100
            条执行记录；输入与结果正文不持久化，模型用量未知。握手、目录、通知不计入执行。
          </p>
          {data.calls.length === 0 &&
          data.connector?.observationIncomplete !== false ? (
            <p className="text-label text-fg-muted">
              暂无已确认的工具观测记录。
            </p>
          ) : (
            <Calls items={data.calls} />
          )}
        </div>
      )}
      {notice && (
        <p className="text-label text-fg-muted" role="status">
          {notice}
        </p>
      )}
      <PlanDetails
        plan={
          plan
            ? (data?.plans.find((p) => p.planId === plan.planId) ?? null)
            : null
        }
        onClose={() => setPlan(null)}
        busy={busy}
        reading={resource.loading}
        readError={resource.error}
        onAction={(p, action) =>
          void run(
            () =>
              pluginsApi.extensionAction({
                pluginId: p.pluginId,
                digest: p.digest,
                action: configAction(p, action),
              }),
            planActionNotices[action],
          )
        }
      />
      <Dialog
        open={confirmation !== null}
        onOpenChange={(open) => !open && !busy && setConfirmation(null)}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {confirmation === "bind" ? "授权协议绑定" : "启动 MCP 观测入口"}
            </DialogTitle>
            <DialogDescription>
              {confirmation === "bind"
                ? `将完整模型请求与响应交给此插件转换。连接：${providerId ? names.get(providerId) : "未选择"}；协议路径：${data?.policy.protocol?.path ?? "未知"}；本机 / 局域网访问：${allowPrivate ? "允许" : "不允许"}。授权本身不会调用模型。`
                : `启动你选择的 MCP 服务或 HTTP 连接。stdio 程序以当前用户权限运行；本机观测入口仅供持有导出凭据的客户端使用。不会自动发起工具调用。目标：${data?.resourceTarget ?? "未确认"}。文件：${data?.resourceLabel ?? "未选择"}`}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() => setConfirmation(null)}
            >
              取消
            </Button>
            <Button
              loading={busy}
              onClick={() =>
                void run(
                  () =>
                    act(
                      confirmation === "bind"
                        ? {
                            kind: "bind-protocol",
                            providerId: providerId!,
                            allowPrivateNetwork: allowPrivate,
                          }
                        : { kind: "start-connector" },
                    ),
                  confirmation === "bind"
                    ? "协议绑定已授权。"
                    : "MCP 观测入口已启用，等待客户端握手。",
                )
              }
            >
              确认
              {confirmation === "bind" ? "授权协议绑定" : "启动 MCP 观测入口"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}

export function PluginOperationHistory() {
  const [opened, setOpened] = useState(false);
  const resource = useSharedRead(pluginBackupResource, opened);
  const items = resource.data?.plans;
  const [plan, setPlan] = useState<ExtensionPlan | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const load = async () => {
    setError(null);
    if (opened) await resource.invalidate();
    else setOpened(true);
  };
  return (
    <section className="space-y-3" aria-label="插件配置备份与恢复">
      <Button
        variant="tertiary"
        disabled={busy || resource.loading}
        loading={resource.loading}
        onClick={() => void load()}
      >
        配置备份与恢复
      </Button>
      <p className="text-label text-fg-muted">
        插件停用或卸载后，已保存的配置备份仍可恢复。
      </p>
      {(error || resource.error) && (
        <Alert status="danger">
          <AlertTitle>
            {error ? "配置计划操作未完成" : "备份目录读取未完成"}
          </AlertTitle>
          <AlertDescription>{error ?? resource.error}</AlertDescription>
        </Alert>
      )}
      {items && (
        <>
          <PlanCapacityNotice capacity={resource.data?.planCapacity} />
          <Plans items={items} onSelect={setPlan} />
        </>
      )}
      <PlanDetails
        plan={
          plan
            ? (items?.find((item) => item.planId === plan.planId) ?? null)
            : null
        }
        onClose={() => setPlan(null)}
        busy={busy}
        reading={resource.loading}
        readError={resource.error}
        allowApply={false}
        onAction={(p, action) => {
          if (action === "apply" || busy) return;
          setBusy(true);
          void pluginsApi
            .extensionAction({
              pluginId: p.pluginId,
              digest: p.digest,
              action: configAction(p, action),
            })
            .then(() => {
              setPlan(null);
              return load();
            })
            .catch((e) => setError(e instanceof Error ? e.message : String(e)))
            .finally(() => setBusy(false));
        }}
      />
    </section>
  );
}
