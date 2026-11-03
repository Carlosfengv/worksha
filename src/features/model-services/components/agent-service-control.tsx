import { useCallback, useEffect, useMemo, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { providersApi } from "@/features/providers/api";
import { LocalTokenRecovery } from "@/features/providers/components/local-token-recovery";
import {
  captureLocalTokenSource,
  LocalTokenPreparationError,
  requireLocalTokenSource,
  type LocalTokenSource,
} from "@/features/providers/local-token-operation";
import { useLocalTokenError } from "@/features/providers/hooks/use-local-token-error";
import type { NativeConfig } from "@/features/providers/types";
import type {
  Provider,
  ProviderStatus,
  ProviderTransaction,
} from "@/features/providers/types";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { providerStatusResource } from "@/features/providers/queries";
import { modelServicesResource } from "../queries";
import { modelServicesApi } from "../api";
import {
  commonCapabilities,
  serviceDraft,
  type ModelService,
  type ModelServiceCatalog,
} from "../types";
import { verifyServiceModels } from "../verification";

export function AgentServiceControl({
  agentId,
  name,
  providers,
  status,
  catalog,
  unavailable,
  reload,
  compact = false,
}: {
  agentId: string;
  name: string;
  providers: Provider[];
  status: ProviderStatus | null;
  catalog: ModelServiceCatalog | null;
  unavailable?: boolean;
  reload: () => void;
  compact?: boolean;
}) {
  const [choice, setChoice] = useState<ModelService | null>(null);
  const [tokenSource, setTokenSource] = useState<LocalTokenSource | null>(null);
  useEffect(
    () => () => {
      if (tokenSource) tokenSource.active = false;
    },
    [tokenSource],
  );
  const [sourceConfig, setSourceConfig] = useState<NativeConfig | null>(null);
  const [transaction, setTransaction] = useState<ProviderTransaction | null>(
    null,
  );
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState("");
  const [error, setError] = useLocalTokenError();
  const config = status?.configs.find((c) => c.agentId === agentId);
  const runtime = status?.runtimes.find((r) => r.agentId === agentId);
  const attachment = catalog?.attachments.find(
    (a) => a.agentId === agentId && a.attached,
  );
  const current = catalog?.services.find((s) => s.id === attachment?.serviceId);
  const pending = status?.transactions.find(
    (t) =>
      t.agentId === agentId &&
      t.state === "pending" &&
      !(transaction?.id === t.id && transaction.state !== "pending"),
  );
  const pendingAttachment = catalog?.attachments.find(
    (a) => !pending?.restoreOf && a.bindingId === pending?.routing?.id,
  );
  const supported = ["claude", "opencode"].includes(agentId);
  const open = (service: ModelService) => {
    setSourceConfig(config ? { ...config } : null);
    try {
      setTokenSource(
        captureLocalTokenSource(
          status?.localTokenProtocol,
          catalog?.writeProtocol?.storeId ?? "",
        ),
      );
    } catch {
      setTokenSource(null);
    }
    if (pending && pendingAttachment?.serviceId === service.id) {
      setChoice(pendingAttachment.installed);
      setTransaction(pending);
      setError(null);
      return;
    }
    setChoice(service);
    setTransaction(null);
    setError(null);
  };
  const changed = () => {
    reload();
    window.dispatchEvent(new Event("provider-status-changed"));
    window.dispatchEvent(new Event("model-services-changed"));
  };
  const prepare = async () => {
    if (!choice || !sourceConfig?.revision || busy) return;
    setBusy(true);
    setError(null);
    try {
      if (pending)
        throw new Error("请先继续处理或撤销已有预览，再准备新的切换。");
      const source = requireLocalTokenSource(tokenSource);
      const defaultProvider = choice.models.find(
        (model) => model.id === choice.defaultModelId,
      )?.provider;
      await verifyServiceModels(serviceDraft(choice), providers, setProgress);
      setProgress("正在生成配置预览与加密备份…");
      setTransaction(
        await modelServicesApi.prepareConnection(
          {
            agentId,
            expectedRevision: sourceConfig.revision,
            serviceId: choice.id,
            expectedServiceRevision: choice.revision,
          },
          source,
          defaultProvider,
        ),
      );
      changed();
    } catch (e) {
      setError(
        e instanceof Error ? e.message : String(e),
        e instanceof LocalTokenPreparationError ? e.identity : undefined,
      );
    } finally {
      setBusy(false);
      setProgress("");
    }
  };
  const action = async (kind: "apply" | "cancel", target = transaction) => {
    if (!target || busy) return;
    setBusy(true);
    setError(null);
    try {
      const result = await providersApi.providerSwitchAction(target.id, kind);
      setTransaction(result);
      changed();
      if (kind === "cancel") {
        setChoice(null);
        setTransaction(null);
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  if (!supported)
    return (
      <span className="text-label text-fg-muted">
        兼容连接 · 尚未适配模型服务
      </span>
    );
  return (
    <div className={compact ? "min-w-48 max-w-72 space-y-1" : "space-y-4"}>
      {!compact && !choice && !transaction && (
        <LocalTokenRecovery
          protocol={status?.localTokenProtocol}
          agentId={agentId}
          onPreview={(preview) => {
            setTransaction(preview);
            reload();
          }}
        />
      )}
      {!compact && (
        <div className="space-y-2">
          <h3 className="text-heading-sm font-semibold">模型服务</h3>
          <p className="text-body text-fg-muted">
            选择服务后，在 Agent 内使用“默认”或指定模型。Provider
            由服务统一管理。
          </p>
        </div>
      )}
      {error && !choice && (
        <Alert status="danger" role="alert">
          <AlertTitle>操作未完成</AlertTitle>
          <AlertDescription className="whitespace-normal break-words">
            {error}
          </AlertDescription>
        </Alert>
      )}
      {pending && (
        <Alert status="warning" role="status">
          <AlertTitle>有未完成的切换</AlertTitle>
          <AlertDescription className="whitespace-normal break-words">
            {pending.targetName}。继续完成，或撤销后选择其他服务。
            {!pendingAttachment && " 该预览可在配置备份与恢复中查看。"}
            <div className="mt-2 flex flex-wrap gap-2">
              {pendingAttachment && (
                <Button
                  size="sm"
                  disabled={busy || unavailable}
                  onClick={() => open(pendingAttachment.installed)}
                >
                  继续处理
                </Button>
              )}
              <Button
                variant="tertiary"
                size="sm"
                disabled={busy || unavailable}
                onClick={() => void action("cancel", pending)}
              >
                撤销待应用切换
              </Button>
            </div>
          </AlertDescription>
        </Alert>
      )}
      <Select
        value={current?.id ?? ""}
        onValueChange={(value) => {
          const service = catalog?.services.find((s) => s.id === value);
          if (service) open(service);
        }}
        disabled={
          !catalog ||
          !status ||
          unavailable ||
          busy ||
          !!pending ||
          !runtime?.supported ||
          !!config?.error ||
          !config?.revision
        }
      >
        <SelectTrigger
          aria-label={`${name} 模型服务`}
          placeholder={
            catalog?.services.length ? "选择模型服务" : "尚无模型服务"
          }
        />
        <SelectContent>
          {catalog?.services.map((service) => (
            <SelectItem key={service.id} value={service.id}>
              {service.name}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      {current ? (
        <p className="text-label text-fg-muted">
          默认：
          {current.models.find((m) => m.id === current.defaultModelId)?.name ??
            "未知"}
          {attachment?.directorySyncRequired ? " · 目录待同步" : ""}
        </p>
      ) : (
        <p className="text-label text-fg-muted">
          {config?.endpoint ? "现有兼容连接保留" : "尚未接入模型服务"}
        </p>
      )}
      {!compact && (
        <>
          {current && (
            <div className="space-y-2">
              <Badge status="neutral">
                固定服务入口 · 目录版本 {attachment?.installed.revision}
              </Badge>
              <p className="text-body">
                {current.models.map((m) => m.name).join(" · ")}
              </p>
              <p className="text-label text-fg-muted">
                默认入口{" "}
                {commonCapabilities(
                  attachment?.installed ?? current,
                ).contextWindow.toLocaleString()}{" "}
                Token；指定模型保持选择。同轮工具续调和重试保持原路由。
              </p>
              <Button
                variant="secondary"
                disabled={busy || unavailable || !!pending}
                onClick={() => open(current)}
              >
                同步模型目录
              </Button>
            </div>
          )}
          {!catalog?.services.length && (
            <Button variant="link" asChild>
              <a href="#/model-services">创建模型服务</a>
            </Button>
          )}
          {agentId === "claude" && (
            <p className="text-label text-fg-muted">
              Claude Code
              菜单显示各模型能力声明；实际上下文与输出预算使用组内共同保守上限。OpenCode
              按模型注入独立上限。Claude
              仍可能显示通用思考选项，请保持关闭；网关会拒绝未发布能力。首批仅发布文本与标准工具。
            </p>
          )}
        </>
      )}
      <Dialog
        open={!!choice || !!transaction}
        onOpenChange={(open) => {
          if (!open && !busy) {
            setChoice(null);
            setTransaction(null);
          }
        }}
      >
        <DialogContent className="flex max-h-[85dvh] flex-col">
          <DialogHeader>
            <DialogTitle>
              {transaction?.state === "applied"
                ? "模型服务配置已写入"
                : "接入模型服务"}
            </DialogTitle>
            <DialogDescription>
              {name} → {choice?.name ?? transaction?.targetName}
              。首次接入或目录同步会写入原生配置并保存加密备份；日常默认目标调整不改文件。
            </DialogDescription>
          </DialogHeader>
          <div className="min-h-0 space-y-4 overflow-y-auto px-1">
            <LocalTokenRecovery
              protocol={status?.localTokenProtocol}
              agentId={agentId}
              onPreview={(preview) => {
                setTransaction(preview);
                reload();
              }}
            />
            {error && (
              <Alert status="danger" role="alert">
                <AlertTitle>操作未完成</AlertTitle>
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
            {choice && (
              <p className="text-body">
                注入“默认”及 {choice?.models.length ?? 0} 个指定模型；保持现有
                MCP、权限及其他配置。
              </p>
            )}
            {!!runtime?.processes && (
              <Alert status="warning">
                <AlertTitle>先退出 {name}</AlertTitle>
                <AlertDescription>
                  检测到 {runtime.processes}{" "}
                  个相关进程。可以准备预览，全部退出后才能写入；不会强制结束进程。
                </AlertDescription>
              </Alert>
            )}
            {transaction && (
              <>
                <p className="text-label text-fg-muted">
                  原配置加密备份已保存。
                  {transaction.state === "applied"
                    ? "下次启动客户端后使用新目录；请在新会话核对实际模型。"
                    : "确认写入后，按原方式重新启动客户端。关闭此窗口会保留待应用事务，可在连接页继续。"}
                </p>
                <p className="break-all text-label text-fg-muted">
                  本机入口：{transaction.routing?.localEndpoint}
                </p>
              </>
            )}
            <p role="status" className="text-label text-fg-muted">
              {progress ||
                (!transaction
                  ? "准备时会向服务内模型发送连接验证短请求。"
                  : "")}
            </p>
          </div>
          <DialogFooter>
            {transaction?.state === "applied" ? (
              <Button
                onClick={() => {
                  setChoice(null);
                  setTransaction(null);
                }}
              >
                完成
              </Button>
            ) : (
              <>
                <Button
                  variant="tertiary"
                  disabled={busy}
                  onClick={() => {
                    if (transaction) void action("cancel");
                    else setChoice(null);
                  }}
                >
                  取消
                </Button>
                {transaction ? (
                  <Button
                    loading={busy}
                    disabled={runtime?.processes !== 0}
                    onClick={() => void action("apply")}
                  >
                    写入配置
                  </Button>
                ) : (
                  <Button loading={busy} onClick={() => void prepare()}>
                    验证并生成预览
                  </Button>
                )}
              </>
            )}
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

type BackupRow = {
  transaction: ProviderTransaction;
  busy: boolean;
  act: (action: "apply" | "cancel" | "restore") => void;
};
const backupColumns: ColumnDef<BackupRow>[] = [
  {
    id: "target",
    header: "配置记录",
    cell: ({ row }) => (
      <div className="space-y-1">
        <p className="text-body">{row.original.transaction.targetName}</p>
        <p className="text-label text-fg-muted">
          {formatDateTime(row.original.transaction.createdAt, "full")}
        </p>
      </div>
    ),
  },
  {
    id: "state",
    header: "状态",
    cell: ({ row }) => (
      <span className="text-label">
        {{
          applied: "已写入 · 备份保留",
          pending: "待应用",
          cancelled: "已撤销",
          failed: "未完成",
        }[row.original.transaction.state] ?? row.original.transaction.state}
      </span>
    ),
  },
  {
    id: "actions",
    header: "操作",
    cell: ({ row }) => (
      <div className="flex flex-wrap gap-1">
        {row.original.transaction.state === "pending" ? (
          <>
            <Button
              variant="secondary"
              size="sm"
              disabled={row.original.busy}
              onClick={() => row.original.act("apply")}
            >
              应用预览
            </Button>
            <Button
              variant="tertiary"
              size="sm"
              disabled={row.original.busy}
              onClick={() => row.original.act("cancel")}
            >
              撤销
            </Button>
          </>
        ) : row.original.transaction.state === "applied" ? (
          <Button
            variant="tertiary"
            size="sm"
            disabled={row.original.busy}
            onClick={() => row.original.act("restore")}
          >
            预览恢复原配置
          </Button>
        ) : null}
      </div>
    ),
  },
];

export function ModelServiceSettings({
  agentId,
  name,
  providers,
}: {
  agentId: string;
  name: string;
  providers: Provider[];
}) {
  const status = useSharedRead(providerStatusResource);
  const catalog = useSharedRead(modelServicesResource);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const reloadProviderStatus = status.reload;
  const reloadServiceCatalog = catalog.reload;
  const invalidateProviderStatus = status.invalidate;
  const invalidateServiceCatalog = catalog.invalidate;
  const reload = useCallback(() => {
    reloadProviderStatus();
    reloadServiceCatalog();
  }, [reloadProviderStatus, reloadServiceCatalog]);
  const invalidate = useCallback(() => {
    void invalidateProviderStatus();
    void invalidateServiceCatalog();
  }, [invalidateProviderStatus, invalidateServiceCatalog]);
  const transact = useCallback(
    async (id: string, action: "apply" | "cancel" | "restore") => {
      setBusy(true);
      setError(null);
      try {
        await providersApi.providerSwitchAction(id, action);
        window.dispatchEvent(new Event("provider-status-changed"));
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setBusy(false);
      }
    },
    [],
  );
  const rows = useMemo(
    () =>
      (status.data?.transactions ?? [])
        .filter((t) => t.agentId === agentId)
        .map((transaction) => ({
          transaction,
          busy,
          act: (action: "apply" | "cancel" | "restore") => {
            void transact(transaction.id, action);
          },
        }))
        .reverse(),
    [status.data, agentId, busy, transact],
  );
  const { table } = useDataTable({
    data: rows,
    columns: backupColumns,
    getRowId: (row) => row.transaction.id,
    manualPagination: true,
    enableRowSelection: false,
    enableSorting: false,
    enableHiding: false,
  });
  return (
    <section className="space-y-4" aria-label="Agent 模型服务与配置恢复">
      {(error || status.error || catalog.error) && (
        <Alert status="danger" role="alert">
          <AlertTitle>读取或操作未完成</AlertTitle>
          <AlertDescription>
            {error || status.error || catalog.error}
          </AlertDescription>
        </Alert>
      )}
      <Button
        variant="tertiary"
        size="sm"
        loading={status.loading || catalog.loading}
        onClick={reload}
      >
        重新读取服务与配置
      </Button>
      <AgentServiceControl
        agentId={agentId}
        name={name}
        providers={providers}
        status={status.data}
        catalog={catalog.data}
        unavailable={!!status.error || !!catalog.error}
        reload={invalidate}
      />
      <details className="space-y-3">
        <summary className="cursor-pointer text-body font-medium">
          配置备份与恢复（{rows.length}）
        </summary>
        <p className="text-label text-fg-muted">
          包括此前的兼容连接。AES-256-GCM
          加密备份保留；恢复先生成预览，原文件变化会拒绝覆盖。首次接入、目录同步和恢复需退出相关客户端。
        </p>
        <DataTable
          table={table}
          showPagination={false}
          aria-label="模型服务配置备份"
          role="region"
          emptyMessage="还没有配置事务。"
        />
      </details>
    </section>
  );
}
import { formatDateTime } from "@/shared/lib/date-time";
