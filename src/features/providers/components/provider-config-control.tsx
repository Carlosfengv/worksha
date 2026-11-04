import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
} from "@/components/ui/combobox";
import {
  DetailList,
  DetailListItem,
  DetailListLabel,
  DetailListValue,
} from "@/components/ui/detail-list";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { providersApi } from "@/features/providers/api";
import {
  availableProviderModelIds,
  providerCatalog,
  providerVendor,
} from "@/features/providers/catalog";
import { NativeProviderImport } from "@/features/providers/components/native-provider-import";
import { DynamicRouteControl } from "./dynamic-route-control";
import { LocalTokenRecovery } from "./local-token-recovery";
import {
  captureLocalTokenSource,
  LocalTokenPreparationError,
  requireLocalTokenSource,
  type LocalTokenSource,
} from "../local-token-operation";
import { useLocalTokenError } from "../hooks/use-local-token-error";
import { dynamicBindingFor, type DynamicChange } from "../types";
import type {
  NativeConfig,
  Provider,
  ProviderStatus,
  ProviderTransaction,
} from "@/features/providers/types";
import type { AgentProductId } from "@/shared/types/agent";
import { date } from "@/features/sessions/display";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { providerStatusResource } from "../queries";
import { useEffect, useState, type ReactNode } from "react";
import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";

export function AgentProviderSettings({
  agentId,
  agentProductId,
  providers,
  refresh,
}: {
  agentId: string;
  agentProductId: AgentProductId;
  providers: Provider[];
  refresh: () => Promise<void>;
}) {
  const status = useSharedRead(
    providerStatusResource,
    agentId !== "antigravity",
  );
  if (agentId === "antigravity") {
    return (
      <section className="space-y-3" aria-label="Antigravity 模型连接">
        <Alert>
          <AlertTitle>模型连接由 Antigravity 管理</AlertTitle>
          <AlertDescription>
            桌面 App 尚无已验证的自定义 Provider / 网关配置入口。CLI 与 SDK
            的端点配置属于不同产品接口，不能用于此桌面入口。
          </AlertDescription>
        </Alert>
        <p className="text-body text-fg-muted">
          请在 Antigravity
          输入框下方选择模型。当前轮次会继续使用原模型，结束或取消后，后续消息才使用新选择。
          授权读取后可在调用与用量中核对已落盘的实际模型和
          Token；历史记录不代表当前模型选择。
        </p>
      </section>
    );
  }
  return (
    <section className="space-y-3" aria-label="本机 Agent 配置">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-title font-medium">Provider 与配置切换</h3>
        <Button
          variant="tertiary"
          size="sm"
          loading={status.loading}
          onClick={status.reload}
        >
          重新读取配置
        </Button>
      </div>
      <p className="text-body text-fg-muted">
        选择已配置的供应商连接，自动匹配 Agent 协议，并保留切换前备份。
      </p>
      {agentId === "codex" && (
        <Alert>
          <AlertTitle>CLI 与桌面共用配置</AlertTitle>
          <AlertDescription>
            切换和恢复会影响 Codex CLI 与 Codex 桌面端。预览将备份同一份
            config.toml；应用前需要退出两端。自动重启只请求桌面端正常退出，CLI
            需在原终端退出。来源无法区分的网关调用保留未知。
          </AlertDescription>
        </Alert>
      )}
      {["claude", "codex"].includes(agentId) && (
        <NativeProviderImport
          agentId={agentId}
          onImported={async () => {
            await refresh();
            void status.invalidate();
          }}
        />
      )}
      {status.error ? (
        <Alert status="danger">
          <AlertTitle>配置读取未完成</AlertTitle>
          <AlertDescription>
            {status.error}。重新读取后再切换。
          </AlertDescription>
        </Alert>
      ) : status.data ? (
        <ProviderConfigControl
          status={status.data}
          providers={providers}
          reload={status.invalidate}
          checking={status.loading}
          onlyAgent={agentId}
          agentProductId={agentProductId}
        />
      ) : (
        <p className="text-body text-fg-muted">正在读取本机配置…</p>
      )}
    </section>
  );
}

const stateName: Record<string, string> = {
  pending: "待重启后生效",
  applying: "正在确认写入",
  applied: "已写入配置 · 下次启动使用",
  cancelled: "已撤销",
  conflict: "配置冲突",
};
function ConfigDetails({ config }: { config: NativeConfig }) {
  return (
    <DetailList>
      <DetailListItem>
        <DetailListLabel>配置文件</DetailListLabel>
        <DetailListValue className="break-all">{config.path}</DetailListValue>
      </DetailListItem>
      <DetailListItem>
        <DetailListLabel>连接方式</DetailListLabel>
        <DetailListValue>
          {config.mode === "native" ? "官方原生登录" : config.providerName}
        </DetailListValue>
      </DetailListItem>
      <DetailListItem>
        <DetailListLabel>端点</DetailListLabel>
        <DetailListValue className="break-all">
          {config.endpoint ?? "使用 Agent 默认端点"}
        </DetailListValue>
      </DetailListItem>
      <DetailListItem>
        <DetailListLabel>模型</DetailListLabel>
        <DetailListValue>{config.model ?? "由 Agent 管理"}</DetailListValue>
      </DetailListItem>
      <DetailListItem>
        <DetailListLabel>鉴权</DetailListLabel>
        <DetailListValue>{config.credential}</DetailListValue>
      </DetailListItem>
    </DetailList>
  );
}
export function ProviderConfigControl({
  status,
  providers,
  reload,
  checking = false,
  onlyAgent,
  agentProductId,
  renderPicker,
}: {
  status: ProviderStatus;
  providers: Provider[];
  reload: () => void;
  checking?: boolean;
  onlyAgent?: string;
  agentProductId: AgentProductId;
  renderPicker?: (
    openSwitch: (config: NativeConfig, providerId: string) => void,
    busy: boolean,
  ) => ReactNode;
}) {
  const { configAgentName: agentName } = useAgentDirectory();
  const [draft, setDraft] = useState<NativeConfig | null>(null);
  const [tokenSource, setTokenSource] = useState<LocalTokenSource | null>(null);
  useEffect(
    () => () => {
      if (tokenSource) tokenSource.active = false;
    },
    [tokenSource],
  );
  const captureSource = () => {
    try {
      setTokenSource(captureLocalTokenSource(status.localTokenProtocol));
    } catch {
      setTokenSource(null);
    }
  };
  const [enrollDynamic, setEnrollDynamic] = useState(false);
  const [fixedSelection, setFixedSelection] = useState(false);
  const [dynamicPreview, setDynamicPreview] = useState<DynamicChange | null>(
    null,
  );
  const [selection, setSelection] = useState("native");
  const [model, setModel] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useLocalTokenError();
  const [notice, setNotice] = useState<string | null>(null);
  const [verificationProgress, setVerificationProgress] = useState<
    string | null
  >(null);
  const [restart, setRestart] = useState<ProviderTransaction | null>(null);
  const desktopRestart = [
    "claude-code-desktop",
    "codex-desktop",
    "opencode",
  ].includes(agentProductId);
  const [feedback, setFeedback] = useState<{
    transactionId: string;
    status: "default" | "danger" | "warning";
    title: string;
    message: string;
  } | null>(null);
  const needsVerification = (transaction: ProviderTransaction) =>
    transaction.state === "pending" &&
    !transaction.restoreOf &&
    !!transaction.provider &&
    !(
      transaction.routing?.models?.length
        ? transaction.routing.models
        : [transaction.model ?? transaction.provider?.model]
    ).every((modelId) =>
      status.verifications.some(
        (verification) =>
          verification.providerId === transaction.provider?.id &&
          (verification.model ?? transaction.provider?.model) === modelId &&
          verification.state === "verified",
      ),
    );
  const reverify = async (transaction: ProviderTransaction) => {
    if (busy || !transaction.provider) return;
    const provider = providers.find(
      (item) => item.id === transaction.provider?.id,
    );
    if (!provider) return;
    setBusy(transaction.id);
    setFeedback({
      transactionId: transaction.id,
      status: "default",
      title: "正在验证目标模型",
      message: "正在验证所选模型，当前 Agent 配置尚未修改。",
    });
    try {
      const models = transaction.routing?.models?.length
        ? transaction.routing.models
        : [transaction.model ?? transaction.provider.model];
      for (const [index, modelId] of models.entries()) {
        setFeedback({
          transactionId: transaction.id,
          status: "default",
          title: "正在验证目标模型",
          message: `${index + 1}/${models.length} · ${modelId}；当前配置尚未修改。`,
        });
        const result = await providersApi.verifyProvider(
          provider.id,
          provider.revision,
          modelId,
        );
        if (result.state !== "verified") {
          setFeedback({
            transactionId: transaction.id,
            status: "danger",
            title: "目标模型验证未通过",
            message: `${modelId}：${result.message}`,
          });
          return;
        }
      }
      setFeedback({
        transactionId: transaction.id,
        status: "default",
        title: "目标模型验证通过",
        message: `已验证 ${models.length} 个模型，结果保留 15 分钟，可继续重启或手动应用；当前配置尚未修改。`,
      });
    } catch (reason) {
      setFeedback({
        transactionId: transaction.id,
        status: "danger",
        title: "目标模型验证未完成",
        message: reason instanceof Error ? reason.message : String(reason),
      });
    } finally {
      setBusy(null);
      reload();
      window.dispatchEvent(new Event("provider-status-changed"));
    }
  };
  const verificationControl = (transaction: ProviderTransaction) =>
    needsVerification(transaction) && (
      <Alert status="warning">
        <AlertTitle>目标模型需要重新验证</AlertTitle>
        <AlertDescription>
          <p>
            验证已过期或当前版本尚未验证。将向本次发布的每个模型发送最多 16
            个输出 Token 的短请求，可能产生少量费用；验证不会写入 Agent 配置。
          </p>
          <Button
            variant="neutral"
            size="sm"
            loading={busy === transaction.id}
            disabled={
              !!busy ||
              !providers.some(
                (provider) => provider.id === transaction.provider?.id,
              )
            }
            onClick={() => void reverify(transaction)}
          >
            重新验证目标模型
          </Button>
        </AlertDescription>
      </Alert>
    );
  const act = async (
    id: string,
    action: "apply" | "cancel" | "restore" | "defer" | "restart",
  ) => {
    if (busy) return;
    setBusy(id);
    setError(null);
    setNotice(null);
    setFeedback({
      transactionId: id,
      status: "default",
      title:
        action === "restart"
          ? "正在重启并应用配置"
          : action === "apply"
            ? "正在应用配置"
            : "正在处理配置事务",
      message:
        action === "restart"
          ? "正在检查配置和进程、请求客户端正常退出，再写入并启动。请处理客户端的保存提示；退出等待最多 15 秒。"
          : action === "apply"
            ? "正在复核运行进程、配置版本和备份，完成后重新读取配置。"
            : "完成后将重新读取切换与恢复记录。",
    });
    try {
      const result =
        action === "restart"
          ? await providersApi.restartAgentProvider(id, agentProductId)
          : await providersApi.providerSwitchAction(id, action);
      setRestart(null);
      setFeedback({
        transactionId: id,
        status:
          action === "apply" && result.state !== "applied"
            ? "warning"
            : "default",
        title:
          action === "defer"
            ? "已保留待重启切换"
            : action === "restart"
              ? "配置已写入并已请求启动"
              : action === "cancel"
                ? "已撤销待应用切换"
                : action === "restore"
                  ? "恢复预览已生成"
                  : result.state === "applied"
                    ? "已写入配置"
                    : "需要核对事务结果",
        message:
          action === "defer"
            ? "待应用目标和加密备份已保留，当前配置未修改。退出相关 Agent 后，在此确认应用，再按原方式启动。"
            : action === "restart"
              ? "系统已接受启动请求。请在新调用中确认实际模型；启动请求本身不代表调用验证通过。"
              : action === "cancel"
                ? "待应用切换已撤销，Agent 配置未修改。"
                : action === "restore"
                  ? "恢复预览与当前配置备份已生成。核对后应用。"
                  : result.state === "applied"
                    ? "配置已写入。请在新启动的 Agent 中确认登录和实际模型；旧 Session 的路由记录保持原样。"
                    : "请重新读取事务结果。",
      });
    } catch (reason) {
      setFeedback({
        transactionId: id,
        status: "danger",
        title: "操作未完成",
        message: reason instanceof Error ? reason.message : String(reason),
      });
    } finally {
      setBusy(null);
      reload();
      window.dispatchEvent(new Event("provider-status-changed"));
    }
  };
  const prepare = async () => {
    if (
      !draft ||
      busy ||
      (!nativeSelection && (!chosen || !modelIds.includes(model)))
    )
      return;
    if (draft.agentId === "claude-desktop" && modelIds.length > 200) {
      setError(
        "Claude 桌面最多支持 200 个模型，请先在供应商详情调整已启用模型；尚未发送验证请求。",
      );
      return;
    }
    setBusy("prepare");
    setError(null);
    setNotice(null);
    try {
      if (!(dynamicBinding && chosen && usingDynamic)) {
        await providersApi.requireCompatibleAgent(draft.agentId);
      }
      if (draft.error || !draft.revision) {
        throw new Error(
          draft.error || "无法确认当前配置版本，请重新读取后重试。",
        );
      }
      if (!nativeSelection && !(dynamicBinding && chosen && usingDynamic))
        requireLocalTokenSource(tokenSource);
      const modelsToVerify =
        !usingDynamic &&
        ["opencode", "claude-desktop", "codex"].includes(draft.agentId)
          ? modelIds
          : [model];
      for (const [index, modelId] of modelsToVerify.entries()) {
        if (
          !chosen ||
          status.verifications.some(
            (v) =>
              v.providerId === chosen.id &&
              (v.model ?? chosen.model) === modelId &&
              v.state === "verified",
          )
        )
          continue;
        setVerificationProgress(
          `正在验证 ${index + 1}/${modelsToVerify.length} · ${modelId}`,
        );
        const result = await providersApi.verifyProvider(
          chosen.id,
          chosen.revision,
          modelId,
        );
        if (result.state !== "verified") {
          setError(`${modelId}：${result.message}`);
          return;
        }
      }
      if (dynamicBinding && chosen && usingDynamic) {
        const change = await providersApi.prepareDynamicRoute({
          operationId: crypto.randomUUID(),
          bindingId: dynamicBinding.id,
          expectedGeneration: dynamicBinding.generation,
          providerId: chosen.id,
          expectedProviderRevision: chosen.revision,
          model,
        });
        setDynamicPreview(change);
        setDraft(null);
        return;
      }
      const transaction =
        enrollDynamic && chosen
          ? await providersApi.prepareDynamicConnection(
              {
                agentId: draft.agentId,
                expectedRevision: draft.revision,
                providerId: chosen.id,
                expectedProviderRevision: chosen.revision,
                model,
              },
              tokenSource ?? undefined,
            )
          : await providersApi.prepareProviderSwitch(
              draft.agentId,
              draft.revision,
              selection === "native" ? null : selection,
              chosen?.revision,
              chosen ? model : undefined,
              tokenSource ?? undefined,
            );
      setDraft(null);
      setRestart(transaction);
      setNotice(
        "切换预览与原配置加密备份已生成，待重启后生效。请选择重启并生效，或稍后手动重启。",
      );
    } catch (reason) {
      setError(
        reason instanceof Error ? reason.message : String(reason),
        reason instanceof LocalTokenPreparationError
          ? reason.identity
          : undefined,
      );
    } finally {
      setVerificationProgress(null);
      setBusy(null);
      reload();
      window.dispatchEvent(new Event("provider-status-changed"));
    }
  };
  const chosen = providers.find((p) => p.id === selection && p.configured);
  const dynamicBinding =
    draft && !fixedSelection ? dynamicBindingFor(status, draft) : undefined;
  const usingDynamic =
    selection !== "native" && (!!dynamicBinding || enrollDynamic);
  const modelIds = chosen ? availableProviderModelIds(chosen) : [];

  const nativeSelection =
    selection === "native" &&
    ["claude", "codex"].includes(draft?.agentId ?? "");
  const converted =
    !!draft &&
    !!chosen &&
    chosen.protocol !== (draft.agentId === "codex" ? "responses" : "messages");
  const verified =
    nativeSelection ||
    (!!chosen &&
      (!usingDynamic &&
      ["opencode", "claude-desktop", "codex"].includes(draft?.agentId ?? "")
        ? modelIds
        : [model]
      ).every((modelId) =>
        status.verifications.some(
          (v) =>
            v.providerId === selection &&
            (v.model ?? chosen?.model) === modelId &&
            v.state === "verified",
        ),
      ));
  const transactions = status.transactions.filter((t) =>
    renderPicker
      ? t.agentId === onlyAgent
      : !onlyAgent || t.agentId === onlyAgent,
  );
  const openSwitch = (
    config: NativeConfig,
    providerId: string,
    enroll = false,
    fixed = false,
  ) => {
    if (busy || checking) return;
    setNotice(null);
    const provider = providers.find(
      (item) => item.id === providerId && item.configured,
    );
    if (
      providerId &&
      providerId !== "native" &&
      (!provider || !availableProviderModelIds(provider).length)
    )
      return;
    if (
      providerId === "native" &&
      !["claude", "codex"].includes(config.agentId)
    )
      return;
    setDraft(config);
    captureSource();
    setEnrollDynamic(enroll);
    setFixedSelection(fixed);
    setSelection(providerId);
    setModel(provider ? (availableProviderModelIds(provider)[0] ?? "") : "");
    setError(null);
  };
  return (
    <section className="space-y-4" aria-label="Agent 原生配置与切换">
      {!draft && !restart && (
        <LocalTokenRecovery
          protocol={status.localTokenProtocol}
          agentId={onlyAgent}
          onPreview={(preview) => {
            setRestart(preview);
            reload();
          }}
        />
      )}
      {renderPicker?.(
        (config, id) => openSwitch(config, id, false, true),
        !!busy,
      )}
      <DynamicRouteControl
        status={status}
        config={status.configs.find((c) => c.agentId === onlyAgent)}
        preview={dynamicPreview}
        setPreview={setDynamicPreview}
        reload={reload}
      />
      {notice && (
        <Alert>
          <AlertTitle>配置事务</AlertTitle>
          <AlertDescription>{notice}</AlertDescription>
        </Alert>
      )}
      {error && !draft && (
        <Alert status="danger">
          <AlertTitle>操作未完成</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      {!renderPicker && (
        <div className={onlyAgent ? "space-y-4" : "grid gap-4 lg:grid-cols-2"}>
          {status.configs
            .filter((c) => !onlyAgent || c.agentId === onlyAgent)
            .map((config) => {
              const currentRoute = status.transactions.find(
                (t) =>
                  t.agentId === config.agentId &&
                  t.state === "applied" &&
                  t.routing?.localEndpoint === config.endpoint,
              );
              return (
                <article
                  key={config.agentId}
                  className="min-w-0 space-y-3 rounded-xl border border-border p-4"
                >
                  <div className="flex flex-wrap items-start justify-between gap-2">
                    <h3 className="text-title font-medium">
                      {agentName(config.agentId)}
                    </h3>
                    {!config.error && (
                      <Badge status="neutral">
                        {config.state === "absent"
                          ? "默认文件未创建"
                          : "用户级配置已读取"}
                      </Badge>
                    )}
                  </div>
                  {!config.error && <ConfigDetails config={config} />}
                  {currentRoute?.provider &&
                    !dynamicBindingFor(status, config) && (
                      <p className="text-body text-fg-muted">
                        当前路由：{currentRoute.provider.name} ·{" "}
                        {currentRoute.routing?.models?.length
                          ? `默认 ${currentRoute.model ?? currentRoute.provider.model} · ${currentRoute.routing.models.length} 个已发布模型`
                          : (currentRoute.model ??
                            currentRoute.provider.model)}{" "}
                        · {currentRoute.routing?.clientProtocol} →{" "}
                        {currentRoute.routing?.upstreamProtocol}
                        。此连接使用切换时快照，目录修改后需重新切换。
                      </p>
                    )}
                  <p className="text-label text-fg-subtle">
                    {config.warnings.join(" ")}
                  </p>
                  <div className="flex flex-wrap gap-2">
                    <Button
                      variant="neutral"
                      size="sm"
                      disabled={checking || !!busy}
                      onClick={() =>
                        openSwitch(
                          config,
                          config.mode === "native" &&
                            ["claude", "codex"].includes(config.agentId)
                            ? "native"
                            : "",
                        )
                      }
                    >
                      切换 Provider
                      <span className="sr-only">
                        {" "}
                        {agentName(config.agentId)}
                      </span>
                    </Button>
                    {["claude", "opencode"].includes(config.agentId) &&
                      !dynamicBindingFor(status, config) && (
                        <Button
                          variant="tertiary"
                          size="sm"
                          disabled={checking || !!busy}
                          onClick={() => {
                            const provider = providers.find(
                              (p) =>
                                p.configured &&
                                availableProviderModelIds(p).length,
                            );
                            openSwitch(config, provider?.id ?? "", true);
                          }}
                        >
                          接入固定入口
                        </Button>
                      )}
                    <Button
                      variant="tertiary"
                      size="sm"
                      onClick={() => {
                        window.location.hash = "/providers";
                      }}
                    >
                      管理供应商
                    </Button>
                  </div>
                </article>
              );
            })}
        </div>
      )}
      {!renderPicker && (
        <p className="text-label text-fg-subtle">
          {status.backupProtection}
          。原生配置写入与恢复先生成待应用事务。桌面端可确认后重启；CLI
          需先在原终端退出，确认应用后再启动。
        </p>
      )}
      {transactions.length > 0 && (
        <section className="space-y-3" aria-label="配置切换与备份记录">
          <h3 className="text-title font-medium">切换与恢复记录</h3>
          {transactions.slice(0, 20).map((t) => {
            const currentConfig = status.configs.find(
              (item) => item.agentId === t.agentId,
            );
            const operation =
              feedback?.transactionId === t.id ? feedback : null;
            return (
              <article
                key={t.id}
                className="space-y-3 rounded-xl border border-border p-4"
                aria-label={`配置事务 ${t.targetName}`}
              >
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <p className="text-body font-medium">
                    {agentName(t.agentId)} · {t.targetName}
                  </p>
                  <Badge status={t.state === "pending" ? "warning" : "neutral"}>
                    {t.state === "applied" &&
                    t.activation?.state === "launch_requested"
                      ? "已请求启动 · 待调用确认"
                      : (stateName[t.state] ?? t.state)}
                  </Badge>
                </div>
                <p className="text-label text-fg-muted">
                  {date(t.createdAt)} · 原文件
                  {t.before.state === "absent" ? "不存在" : "已加密备份"} ·{" "}
                  {t.restoreOf ? "恢复事务" : "切换事务"}
                </p>
                <p className="break-all text-label text-fg-subtle">
                  {t.backupPath}
                </p>
                {t.provider && (
                  <p className="break-all text-label text-fg-muted">
                    {t.provider.protocol} · {t.provider.endpoint} ·{" "}
                    {t.model ?? t.provider.model} · 凭据：
                    {t.provider.credentialLabel ?? "本地无鉴权"}
                  </p>
                )}
                {t.routing && (
                  <p className="break-all text-label text-fg-muted">
                    {t.routing.clientProtocol === t.routing.upstreamProtocol
                      ? "本机协议转发"
                      : "自动协议转换"}
                    ：{t.routing.clientProtocol} → {t.routing.upstreamProtocol}{" "}
                    · 本机入口 {t.routing.localEndpoint}。使用时保持桌面 App
                    运行。
                    {!!t.routing.models?.length &&
                      ` ${t.state === "pending" ? "待发布" : t.state === "applied" ? "已写入" : "预览包含"} ${t.routing.models.length} 个模型：${t.routing.models.join("、")}。${["pending", "applied"].includes(t.state) ? `应用并重启后，在 ${agentName(t.agentId)} 菜单中确认并选择。` : ""}`}
                  </p>
                )}
                {t.error && (
                  <p className="text-label text-fg-muted">{t.error}</p>
                )}
                {t.state === "pending" && (
                  <Alert status="warning">
                    <AlertTitle>切换尚未生效</AlertTitle>
                    <AlertDescription>
                      <p>
                        当前配置：{currentConfig?.providerName ?? "待重新读取"}{" "}
                        · {currentConfig?.model ?? "由 Agent 管理"}
                      </p>
                      <p>
                        待应用目标：{t.targetName}。应用成功并重新启动 Agent
                        后使用。
                      </p>
                    </AlertDescription>
                  </Alert>
                )}
                {operation && (
                  <Alert
                    status={operation.status}
                    role={operation.status === "danger" ? "alert" : "status"}
                  >
                    <AlertTitle>{operation.title}</AlertTitle>
                    <AlertDescription>{operation.message}</AlertDescription>
                  </Alert>
                )}
                {verificationControl(t)}
                {t.agentId === "codex" && (
                  <p className="text-label text-fg-muted">
                    影响范围：Codex CLI 与 Codex
                    桌面端；应用及恢复共用同一配置文件和版本检查。
                  </p>
                )}
                <div className="flex flex-wrap gap-2">
                  {t.state === "pending" && (
                    <>
                      <Button
                        variant="neutral"
                        size="sm"
                        loading={busy === t.id}
                        disabled={!!busy || checking}
                        onClick={() => setRestart(t)}
                      >
                        重启 Agent 并生效
                      </Button>
                      <Button
                        variant="tertiary"
                        size="sm"
                        disabled={!!busy}
                        onClick={() => void act(t.id, "defer")}
                      >
                        稍后手动重启
                      </Button>
                      <Button
                        variant="tertiary"
                        size="sm"
                        loading={checking}
                        disabled={!!busy}
                        onClick={reload}
                      >
                        重新检查运行状态
                      </Button>
                      <Button
                        variant="tertiary"
                        size="sm"
                        disabled={!!busy}
                        onClick={() => void act(t.id, "cancel")}
                      >
                        撤销待应用
                      </Button>
                    </>
                  )}
                  {t.state === "applied" && (
                    <>
                      {t.activation?.state === "awaiting_restart" &&
                        desktopRestart && (
                          <Button
                            variant="neutral"
                            size="sm"
                            disabled={!!busy}
                            onClick={() => setRestart(t)}
                          >
                            启动 Agent
                          </Button>
                        )}
                      <Button
                        variant="tertiary"
                        size="sm"
                        loading={busy === t.id}
                        disabled={!!busy}
                        onClick={() => void act(t.id, "restore")}
                      >
                        预览恢复原配置
                      </Button>
                    </>
                  )}
                </div>
              </article>
            );
          })}
          {transactions.length > 20 && (
            <p className="text-label text-fg-subtle">
              当前显示最近 20 条记录，完整加密备份保留在本机。
            </p>
          )}
        </section>
      )}
      <Dialog
        open={!!restart}
        onOpenChange={(open) => {
          if (!open && !busy) setRestart(null);
        }}
      >
        <DialogContent
          size="lg"
          className="max-h-[calc(100dvh-2rem)] overflow-y-auto"
        >
          <DialogHeader>
            <DialogTitle>重启 Agent 并生效</DialogTitle>
            <DialogDescription>
              {restart?.targetName}。
              {!!restart?.routing?.models?.length &&
                `同一 Provider 分组包含 ${restart.routing.models.length} 个模型：${restart.routing.models.join("、")}。`}
              原配置已加密备份；配置写入后需要重新启动 Agent。
            </DialogDescription>
          </DialogHeader>
          {restart?.agentId === "codex" && (
            <Alert status="warning">
              <AlertTitle>同时影响 Codex CLI 与桌面端</AlertTitle>
              <AlertDescription>
                两端共用配置，请先保存两端任务并退出
                CLI。自动操作只请求桌面端正常退出。
              </AlertDescription>
            </Alert>
          )}
          <p className="text-body text-fg-muted">
            {desktopRestart
              ? "请先保存该客户端所有窗口中的任务。确认后将请求整个桌面客户端正常退出，检查相关进程已退出，再写入配置并重新打开。客户端拒绝退出、仍有 CLI 或状态未知时会停止，不会强制结束进程。"
              : "请先保存任务，在原终端退出 CLI；确认应用后，在原项目目录按原方式重新启动，以保留工作目录和启动参数。"}
          </p>
          {restart && verificationControl(restart)}
          {feedback && restart && feedback.transactionId === restart.id && (
            <Alert
              status={feedback.status}
              role={feedback.status === "danger" ? "alert" : "status"}
            >
              <AlertTitle>{feedback.title}</AlertTitle>
              <AlertDescription>{feedback.message}</AlertDescription>
            </Alert>
          )}
          <DialogFooter className="flex-wrap">
            <Button
              variant="tertiary"
              disabled={!!busy}
              onClick={() => {
                if (restart?.state === "applied") setRestart(null);
                else if (restart) void act(restart.id, "defer");
              }}
            >
              稍后手动重启
            </Button>
            <Button
              variant="neutral"
              disabled={
                !!busy || checking || (!!restart && needsVerification(restart))
              }
              onClick={() => restart && void act(restart.id, "apply")}
            >
              {desktopRestart ? "已退出，应用后手动启动" : "已退出，应用配置"}
            </Button>
            <Button
              variant="tertiary"
              loading={checking}
              disabled={!!busy}
              onClick={reload}
            >
              重新检查运行状态
            </Button>
            {desktopRestart && (
              <Button
                loading={!!busy}
                disabled={
                  !!busy ||
                  checking ||
                  (!!restart && needsVerification(restart))
                }
                onClick={() => restart && void act(restart.id, "restart")}
              >
                确认重启并生效
              </Button>
            )}
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog
        open={!!draft}
        onOpenChange={(open) => {
          if (!open && !busy) setDraft(null);
        }}
      >
        <DialogContent
          size="lg"
          className="max-h-[calc(100dvh-2rem)] overflow-y-auto"
        >
          <DialogHeader>
            <DialogTitle>
              切换 {draft ? agentName(draft.agentId) : "Agent"} 的 Provider
            </DialogTitle>
            <DialogDescription>
              {usingDynamic && dynamicBinding
                ? "日常切换只更新上游路由，不改写 Agent 配置。工具循环保留原目标，历史兼容时下一轮使用新目标。"
                : enrollDynamic
                  ? "首次接入固定入口会生成原配置加密备份，并需要重新加载 Agent；之后日常切换只更新路由。Claude 已受管档位、OpenCode 主模型与小模型共同使用动态槽位。"
                  : "影响当前用户级配置。预览会创建加密备份，应用时再次检查文件与服务商版本。"}
            </DialogDescription>
            {draft?.agentId === "codex" && (
              <p className="text-body text-fg-muted">
                影响范围：Codex CLI 和 Codex
                桌面端。两端共用配置，应用和恢复都会修改同一文件；已有会话和历史用量不会重新归属。
              </p>
            )}
          </DialogHeader>
          {draft && (
            <div className="space-y-4">
              <LocalTokenRecovery
                protocol={status.localTokenProtocol}
                agentId={draft.agentId}
                onPreview={(preview) => {
                  setDraft(null);
                  setRestart(preview);
                  reload();
                }}
              />
              {!draft.error && <ConfigDetails config={draft} />}
              {error && (
                <Alert status="danger">
                  <AlertTitle>未生成预览</AlertTitle>
                  <AlertDescription>{error}</AlertDescription>
                </Alert>
              )}
              <Field>
                <FieldLabel>目标连接</FieldLabel>
                <Select
                  value={selection}
                  disabled={!!busy}
                  onValueChange={(value) => {
                    const id = String(value);
                    const next = providers.find(
                      (p) => p.id === id && p.configured,
                    );
                    setSelection(id);
                    setModel(
                      (next ? availableProviderModelIds(next)[0] : "") || "",
                    );
                    setError(null);
                  }}
                >
                  <SelectTrigger
                    aria-label="目标 Provider"
                    placeholder="选择已配置的 Provider 或官方登录"
                  />
                  <SelectContent>
                    {draft.agentId !== "opencode" &&
                      draft.agentId !== "claude-desktop" && (
                        <SelectItem value="native">
                          官方原生登录 · 模型由 Agent 管理
                        </SelectItem>
                      )}
                    {providers
                      .filter((p) => p.configured)
                      .map((p) => (
                        <SelectItem key={p.id} value={p.id}>
                          {providerCatalog[providerVendor(p)]?.name ?? p.name}
                          {p.name !== providerCatalog[providerVendor(p)]?.name
                            ? ` · ${p.name}`
                            : ""}{" "}
                          · {availableProviderModelIds(p).length} 个已启用模型
                        </SelectItem>
                      ))}
                  </SelectContent>
                </Select>
                <FieldDescription>
                  官方 API、第三方及本地连接均从已保存的服务商中选择。
                </FieldDescription>
              </Field>
              {!providers.some((p) => p.configured) && (
                <p className="text-body text-fg-muted">
                  尚无已配置的供应商连接。可先在 Providers 保存
                  Endpoint、模型和凭据，再回到此处切换。
                </p>
              )}
              {chosen && (
                <>
                  <Field>
                    <FieldLabel>
                      {!usingDynamic &&
                      ["opencode", "claude-desktop", "codex"].includes(
                        draft.agentId,
                      )
                        ? "分组默认模型"
                        : "目标模型"}
                    </FieldLabel>
                    <Combobox<string>
                      items={modelIds}
                      value={modelIds.includes(model) ? model : null}
                      disabled={!!busy || !modelIds.length}
                      limit={200}
                      onValueChange={(value) => {
                        if (value && modelIds.includes(value)) {
                          setModel(value);
                          setError(null);
                        }
                      }}
                    >
                      <ComboboxInput
                        aria-label="目标模型"
                        placeholder="搜索已启用的模型 ID"
                        triggerAriaLabel="展开可用模型"
                      />
                      <ComboboxContent>
                        <ComboboxEmpty>没有匹配的已启用模型。</ComboboxEmpty>
                        <ComboboxList>
                          {(id: string) => (
                            <ComboboxItem key={id} value={id}>
                              {id}
                              {id === chosen.model ? " · Provider 默认" : ""}
                            </ComboboxItem>
                          )}
                        </ComboboxList>
                      </ComboboxContent>
                    </Combobox>
                    <FieldDescription>
                      {modelIds.length
                        ? `已启用 ${modelIds.length} 个模型。输入 ID 搜索，每次最多显示 200 项；当前选择在筛选时保留。`
                        : "此连接没有已启用模型，请先到供应商详情启用模型。"}
                      {!usingDynamic &&
                      ["opencode", "claude-desktop", "codex"].includes(
                        draft.agentId,
                      )
                        ? "选择分组默认值，全部已启用模型将同时发布；不改变 Provider 默认模型。"
                        : "仅用于本次 Agent 切换，不改变 Provider 默认模型。"}
                    </FieldDescription>
                  </Field>
                  <p className="break-all text-body text-fg-muted">
                    {chosen.endpoint} · {model || "未选择模型"} · 凭据：
                    {chosen.credentialLabel ?? "本地无鉴权"}
                  </p>
                  <Button
                    variant="tertiary"
                    size="sm"
                    disabled={!!busy}
                    onClick={() => {
                      setDraft(null);
                      window.location.hash = `/providers/${encodeURIComponent(chosen.id)}`;
                    }}
                  >
                    管理供应商可用模型
                  </Button>
                </>
              )}
              {chosen && draft.agentId === "claude" && !usingDynamic && (
                <p className="text-label text-fg-muted">
                  Claude 主模型、后台模型档位及子 Agent
                  默认模型统一使用所选模型。经本地网关采集 TTFT
                  和文本速度；同协议保留原始请求。调用时保持 Local Agent 运行。
                </p>
              )}
              {draft.agentId === "opencode" && !usingDynamic && (
                <p className="text-label text-fg-muted">
                  按连接名称生成 Local Agent 分组，逐个验证并发布此 Provider
                  全部已启用模型；应用后可在 OpenCode 直接切换，无需逐个回到 App
                  应用。目录或启用状态变更后需重新应用以同步分组。保留其他分组。OpenCode
                  可能记住先前的模型选择；重启后请在会话模型菜单确认。上游密钥保存在钥匙串，客户端仅保存本机网关令牌；调用时保持
                  Local Agent 运行。
                </p>
              )}
              {draft.agentId === "claude-desktop" && (
                <p className="text-label text-fg-muted">
                  一次发布此 Provider 全部已启用模型，按连接名称连续显示“供应商
                  · 模型”，可在 Claude 桌面菜单直接切换，无需逐个回到 App 应用。
                  当前客户端使用独立兼容模型
                  ID，标签和调用记录显示真实供应商及模型；最多支持 200
                  个模型。目录或启用状态变更后需重新应用同步。 仅更新当前本机
                  Profile，不修改 CLI 或组织策略；原 Profile
                  自动加密备份，凭据通过钥匙串助手读取。使用时保持 Local Agent
                  运行。
                </p>
              )}
              {draft.agentId === "codex" && (
                <Alert status="warning">
                  <AlertTitle>Codex CLI 与桌面端共享模型目录</AlertTitle>
                  <AlertDescription>
                    一次发布此 Provider 全部已启用模型，菜单按“供应商 ·
                    模型”显示。
                    两端共用用户配置、默认模型和加密备份，应用前需退出两端；重启后可在会话中直接选择已发布模型，无需回
                    App 重新应用。
                    会话选择可覆盖共享默认值，项目或启动参数也可能覆盖用户配置。
                    目录变更后需重新应用。此连接支持文本、
                    {chosen?.vendor === "deepseek" &&
                    chosen.protocol === "chat-completions"
                      ? "DeepSeek 思考模式"
                      : "非思考模式"}
                    及标准工具。 使用时保持 Local Agent 运行。
                  </AlertDescription>
                </Alert>
              )}
              {converted && !usingDynamic && (
                <Alert>
                  <AlertTitle>自动转换协议</AlertTitle>
                  <AlertDescription>
                    {draft.agentId === "codex" ? "Responses" : "Messages"} →{" "}
                    {chosen?.protocol}
                    。本机网关负责转换请求和流式响应，无需修改供应商配置。使用时保持桌面
                    App
                    运行。支持文本、图片输入、标准函数与命名空间工具；自由格式工具通过
                    input 字符串封装，语法约束由 Agent
                    检查。原生托管工具与其他服务商的思考签名不跨协议重放。
                    {chosen?.vendor === "deepseek" &&
                      chosen.protocol === "chat-completions" &&
                      "DeepSeek 支持思考模式与工具调用，思考内容可在当前连接中继续回传。"}
                  </AlertDescription>
                </Alert>
              )}
              {!verified && chosen && (
                <Alert>
                  <AlertTitle>切换前验证所选模型</AlertTitle>
                  <AlertDescription>
                    将向所选服务商的
                    {!usingDynamic &&
                    ["opencode", "claude-desktop", "codex"].includes(
                      draft.agentId,
                    )
                      ? `${modelIds.length} 个已启用模型分别`
                      : "所选模型"}
                    发送最多 16 个输出 Token
                    的短请求，可能产生少量费用；验证成功后生成预览，不立即改写
                    Agent 配置。
                  </AlertDescription>
                </Alert>
              )}
              {(!dynamicBinding || selection === "native") && (
                <p className="text-label text-fg-subtle">
                  应用前需关闭相关
                  Agent，防止原生配置热重载影响进行中的工具循环。已有登录保留，由
                  Agent 在下次启动时确认。
                </p>
              )}
              {verificationProgress && (
                <p role="status" className="text-body text-fg-muted">
                  {verificationProgress}
                </p>
              )}
              <DialogFooter>
                <Button
                  variant="tertiary"
                  disabled={!!busy}
                  onClick={() => setDraft(null)}
                >
                  取消
                </Button>
                <Button
                  variant="neutral"
                  loading={busy === "prepare"}
                  disabled={
                    !!busy ||
                    (!nativeSelection && (!chosen || !modelIds.includes(model)))
                  }
                  onClick={() => void prepare()}
                >
                  {usingDynamic && verified
                    ? "生成动态路由预览"
                    : verified
                      ? "生成切换预览与备份"
                      : "验证并生成预览"}
                </Button>
              </DialogFooter>
            </div>
          )}
        </DialogContent>
      </Dialog>
    </section>
  );
}
