import { useCallback, useEffect, useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { providersApi } from "@/features/providers/api";
import { LocalTokenRecovery } from "@/features/providers/components/local-token-recovery";
import { NativeProviderImport } from "@/features/providers/components/native-provider-import";
import { ProviderConfigControl } from "@/features/providers/components/provider-config-control";
import {
  captureLocalTokenSource,
  LocalTokenPreparationError,
  requireLocalTokenSource,
  type LocalTokenSource,
} from "@/features/providers/local-token-operation";
import { useLocalTokenError } from "@/features/providers/hooks/use-local-token-error";
import type { NativeConfig } from "@/features/providers/types";
import { dynamicBindingFor } from "@/features/providers/types";
import type {
  Provider,
  ProviderStatus,
  ProviderTransaction,
} from "@/features/providers/types";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { providerStatusResource } from "@/features/providers/queries";
import { modelServicesResource } from "../queries";
import type { AgentProductId } from "@/shared/types/agent";
import { modelServicesApi } from "../api";
import {
  commonCapabilities,
  serviceDraft,
  type ModelService,
  type ModelServiceCatalog,
} from "../types";
import { verifyServiceModels } from "../verification";
import { AgentConnectionPicker } from "./agent-connection-picker";

export function AgentServiceControl({
  agentId,
  name,
  providers,
  status,
  catalog,
  unavailable,
  servicesUnavailable,
  checking,
  onProviderSelect,
  showRecovery = true,
  reload,
  compact = false,
}: {
  agentId: string;
  name: string;
  providers: Provider[];
  status: ProviderStatus | null;
  catalog: ModelServiceCatalog | null;
  unavailable?: boolean;
  servicesUnavailable?: boolean;
  checking?: boolean;
  onProviderSelect: (config: NativeConfig, providerId: string) => void;
  showRecovery?: boolean;
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
  const serviceSupported = (
    catalog?.connectionAgents ?? ["claude", "opencode"]
  ).includes(agentId);
  const native = ["claude", "codex"].includes(agentId);
  const activeRoute =
    config?.endpoint &&
    status?.transactions.find(
      (t) =>
        t.agentId === agentId &&
        t.before.path === config.path &&
        t.state === "applied" &&
        (t.routing?.localEndpoint === config.endpoint ||
          (agentId === "opencode" &&
            t.routing &&
            `${t.routing.localEndpoint}/v1` === config.endpoint)),
    );
  const dynamic =
    status && config ? dynamicBindingFor(status, config) : undefined;
  const serviceRoute =
    !!config?.endpoint &&
    status?.dynamicBindings?.some(
      (b) =>
        b.modelService &&
        b.agentId === agentId &&
        b.scope === config.path &&
        b.localEndpoint === config.endpoint,
    );
  const providerName =
    dynamic?.target.provider.name ||
    (activeRoute && activeRoute.provider?.name) ||
    config?.providerName;
  const currentLabel =
    unavailable || config?.error
      ? "当前连接待核对"
      : attachment
        ? `模型服务 · ${current?.name ?? attachment.installed.name}`
        : serviceRoute
          ? "模型服务 · 目录未读取"
          : config?.mode === "native" && native
            ? "官方原生登录"
            : config?.endpoint && providerName
              ? `Provider · ${providerName}`
              : !config
                ? "正在读取当前连接…"
                : "尚未选择连接";
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
    if (!choice || busy) return;
    setBusy(true);
    setError(null);
    try {
      if (pending)
        throw new Error("请先继续处理或撤销已有预览，再准备新的切换。");
      await providersApi.requireCompatibleAgent(agentId);
      if (sourceConfig?.error || !sourceConfig?.revision)
        throw new Error(
          sourceConfig?.error || "无法确认当前配置版本，请重新读取后重试。",
        );
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
  if (!["claude", "opencode", "claude-desktop", "codex"].includes(agentId))
    return <span className="text-label text-fg-muted">暂不支持切换</span>;
  return (
    <div className={compact ? "min-w-48 max-w-72 space-y-1" : "space-y-4"}>
      {showRecovery && !compact && !choice && !transaction && (
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
          <h3 className="text-heading-sm font-semibold">模型连接</h3>
          <p className="text-body text-fg-muted">
            选择已配置的 Provider，或接入已发布的模型服务。
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
      <p
        role="status"
        aria-label={`${name} 当前连接`}
        className="text-label text-fg-muted break-words"
      >
        {currentLabel}
      </p>
      <AgentConnectionPicker
        name={name}
        providers={providers}
        services={
          serviceSupported && !servicesUnavailable
            ? catalog?.services
            : undefined
        }
        emptyServices={
          servicesUnavailable
            ? "模型服务读取未完成"
            : !catalog
              ? "正在读取模型服务…"
              : !serviceSupported
                ? "当前后台尚未适配"
                : "暂无已发布模型服务"
        }
        native={native}
        disabled={!config || checking || unavailable || busy || !!pending}
        onSelect={(selection) => {
          if (!config) return;
          if (selection.kind === "service") {
            const service = catalog?.services.find(
              (s) => s.id === selection.id,
            );
            if (service && serviceSupported && !servicesUnavailable)
              open(service);
          } else {
            onProviderSelect(
              config,
              selection.kind === "native" ? "native" : selection.id,
            );
          }
        }}
      />
      {current ? (
        <p className="text-label text-fg-muted">
          默认：
          {current.models.find((m) => m.id === current.defaultModelId)?.name ??
            "未知"}
          {attachment?.directorySyncRequired ? " · 目录待同步" : ""}
        </p>
      ) : (
        !compact &&
        !config?.endpoint && (
          <p className="text-label text-fg-muted">尚未接入模型服务</p>
        )
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
              {agentId === "codex" &&
                " CLI 与桌面端共用配置，应用前请退出两端。"}
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
            {transaction && (
              <>
                <p className="text-label text-fg-muted">
                  原配置加密备份已保存。
                  {transaction.state === "applied"
                    ? "请重新启动客户端，在新会话中选择模型。调用期间请保持 Local Agent 运行。"
                    : "确认写入后，按原方式重新启动客户端。关闭此窗口会保留待应用事务，可在连接页继续。"}
                </p>
                <p className="break-all text-label text-fg-muted">
                  本机入口：{transaction.routing?.localEndpoint}
                </p>
                {agentId === "codex" && transaction.state === "applied" && (
                  <p className="text-label text-fg-muted">
                    如果启用了系统代理或代理工具，请将 127.0.0.1 设为直连。
                  </p>
                )}
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
                  <Button loading={busy} onClick={() => void action("apply")}>
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

export function ModelServiceSettings({
  agentId,
  agentProductId = "",
  name,
  providers,
  refresh,
}: {
  agentId: string;
  agentProductId?: AgentProductId;
  name: string;
  providers: Provider[];
  refresh?: () => Promise<void>;
}) {
  const status = useSharedRead(providerStatusResource);
  const catalog = useSharedRead(modelServicesResource);
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
  return (
    <section className="space-y-4" aria-label="Agent 模型服务与配置恢复">
      {(status.error || catalog.error) && (
        <Alert status="danger" role="alert">
          <AlertTitle>读取或操作未完成</AlertTitle>
          <AlertDescription>{status.error || catalog.error}</AlertDescription>
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
      {status.data ? (
        <ProviderConfigControl
          status={status.data}
          providers={providers}
          reload={invalidate}
          checking={status.loading || !!status.error}
          onlyAgent={agentId}
          agentProductId={agentProductId}
          renderPicker={(openSwitch, switching) => (
            <AgentServiceControl
              agentId={agentId}
              name={name}
              providers={providers}
              status={status.data}
              catalog={catalog.data}
              unavailable={!!status.error}
              servicesUnavailable={!!catalog.error}
              checking={status.loading || switching}
              onProviderSelect={openSwitch}
              showRecovery={false}
              reload={invalidate}
            />
          )}
        />
      ) : (
        !status.error && (
          <p className="text-body text-fg-muted">正在读取本机配置…</p>
        )
      )}
      {refresh && ["claude", "codex"].includes(agentId) && (
        <NativeProviderImport
          agentId={agentId}
          onImported={async () => {
            await refresh();
            invalidate();
          }}
        />
      )}
    </section>
  );
}
