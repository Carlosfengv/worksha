import { useRef, useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
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
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { AgentConnectionPicker } from "@/features/model-services/components/agent-connection-picker";
import { modelServicesApi } from "@/features/model-services/api";
import { modelServicesResource } from "@/features/model-services/queries";
import {
  serviceDraft,
  type ModelService,
} from "@/features/model-services/types";
import { verifyServiceModels } from "@/features/model-services/verification";
import { pluginUploadsApi } from "@/features/plugins/api";
import { providersApi } from "@/features/providers/api";
import { availableProviderModelIds } from "@/features/providers/catalog";
import type { Provider } from "@/features/providers/types";
import { useSharedQuery, useSharedRead } from "@/shared/hooks/use-shared-read";
import { DesktopError } from "@/shared/lib/desktop-command";
import { createReadResourceFamily } from "@/shared/lib/read-resource";
import type { AgentDiscovery } from "@/shared/types/agent";
import { bundledDshAgent } from "../bundled-source";
import {
  agentConnectionApi,
  type AgentConnectionRecovery,
  type AgentConnectionReview,
  type AgentConnectionStatus,
  type DshAccountCatalog,
} from "../api";

const statusResource = createReadResourceFamily(
  (id: string) => agentConnectionApi.status(id),
  { interval: false },
);

type Choice =
  | { kind: "provider"; provider: Provider }
  | { kind: "service"; service: ModelService }
  | { kind: "account"; catalog?: DshAccountCatalog };
const accountLabel = "DeepSeek 官方账号";
type Flow =
  | {
      kind: "switch";
      choice: Choice;
      model: string | null;
      review?: AgentConnectionReview;
      needsCheck?: boolean;
    }
  | { kind: "restore"; recovery: AgentConnectionRecovery; needsCheck?: boolean }
  | { kind: "revoke" };

function selectionLabel(selection: { provider: string; model: string } | null) {
  if (!selection) return "尚未选择连接";
  return selection.provider.startsWith("local-agent-")
    ? "托管连接待核对"
    : `${selection.provider} · ${selection.model}`;
}

function currentLabel(status: AgentConnectionStatus | null) {
  if (!status) return "正在读取当前连接…";
  if (status.state === "authorization-required") return "切换时授权";
  if (!status.currentTarget) return selectionLabel(status.selected);
  const { kind, name, model } = status.currentTarget;
  if (kind === "account") return `${name} · ${model}`;
  return `${kind === "provider" ? "Provider" : "模型服务"} · ${name} · ${model}`;
}

export function PluginAgentConnection({
  agent,
  providers,
  compact = false,
}: {
  agent: AgentDiscovery;
  providers: Provider[];
  compact?: boolean;
}) {
  const status = useSharedQuery(statusResource, agent.id);
  const services = useSharedRead(modelServicesResource);
  const [flow, setFlow] = useState<Flow | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const [progress, setProgress] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  const inFlight = useRef(false);
  const authorized = status.data?.state === "authorized" ? status.data : null;
  const recovery = !status.error ? authorized?.recovery : undefined;
  const bundled = agent.id === bundledDshAgent.id;

  function open(next: Flow | null) {
    setFlow(next);
    setError(null);
    setNotice(null);
  }

  async function run(task: () => Promise<void>) {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setError(null);
    try {
      await task();
    } catch (reason) {
      setError(reason instanceof Error ? reason : new Error(String(reason)));
    } finally {
      inFlight.current = false;
      setBusy(false);
      setProgress("");
    }
  }

  async function authorize() {
    if (!authorized) {
      await agentConnectionApi.status(agent.id, { kind: "authorize" });
      await status.invalidate();
    }
  }

  async function readAccountModels(
    switchFlow: Extract<Flow, { kind: "switch" }>,
  ) {
    await authorize();
    const catalog = await agentConnectionApi.accountModels(agent.id);
    setFlow({
      ...switchFlow,
      choice: { kind: "account", catalog },
      model: null,
    });
  }

  async function submit() {
    if (!flow) return;
    if (flow.kind === "revoke") {
      await agentConnectionApi.status(agent.id, { kind: "revoke" });
      await status.invalidate();
      open(null);
      return;
    }
    if (flow.needsCheck) {
      const pending = flow.kind === "switch" ? flow.review : flow.recovery;
      if (!pending) return;
      const value = await agentConnectionApi.status(agent.id, {
        kind: "check",
        reviewId: pending.reviewId,
      });
      if (value.state !== "authorized") throw Error("连接授权已失效。");
      await status.invalidate();
      if (flow.kind === "switch" && value.configured && value.selectedTarget) {
        open(null);
        setNotice("已回读连接配置 · 默认模型已选中");
      } else if (value.configured && pending.before) {
        setFlow({
          kind: "restore",
          recovery: {
            reviewId: pending.reviewId,
            before: pending.before,
            kind:
              flow.kind === "restore"
                ? flow.recovery.kind
                : flow.choice.kind === "account"
                  ? "account"
                  : undefined,
          },
        });
      } else if (flow.kind === "restore") {
        if (
          value.selected?.provider !== flow.recovery.before.provider ||
          value.selected.model !== flow.recovery.before.model ||
          (value.selected.reasoningEffort ?? null) !==
            (flow.recovery.before.reasoningEffort ?? null)
        )
          throw Error("未读到恢复目标。请核对 Agent 当前连接。");
        open(null);
        setNotice("已恢复变更前的连接");
      } else {
        setFlow({ ...flow, review: undefined, needsCheck: false });
      }
      return;
    }
    if (flow.kind === "switch" && !flow.review) {
      const { choice, model } = flow;
      if (choice.kind === "account") {
        if (!model) {
          await readAccountModels(flow);
          return;
        }
        await authorize();
        const review = await agentConnectionApi.previewAccount(agent.id, model);
        setFlow({ ...flow, review });
        return;
      }
      if (choice.kind === "provider" && !model) return;
      await authorize();
      if (choice.kind === "provider") {
        if (!model) return;
        setProgress(`正在验证 ${model}…`);
        const { id, revision } = choice.provider;
        const result = await providersApi.verifyProvider(id, revision, model);
        if (result.state !== "verified") throw Error(result.message);
        const review = await agentConnectionApi.preview(agent.id, {
          kind: "provider",
          id,
          model,
        });
        setFlow({ ...flow, review });
      } else {
        await verifyServiceModels(
          serviceDraft(choice.service),
          providers,
          setProgress,
          async (providerId, expectedRevision, model, upstreamAccount) =>
            upstreamAccount
              ? modelServicesApi.verifyMember({
                  storeId: await modelServicesApi.requireWriteProtocol(
                    services.data?.writeProtocol?.storeId,
                  ),
                  providerId,
                  expectedRevision,
                  model,
                  upstreamAccount,
                })
              : providersApi.verifyProvider(
                  providerId,
                  expectedRevision,
                  model,
                ),
        );
        const review = await agentConnectionApi.preview(agent.id, {
          kind: "service",
          id: choice.service.id,
        });
        setFlow({ ...flow, review });
      }
      return;
    }
    const reviewId =
      flow.kind === "restore" ? flow.recovery.reviewId : flow.review?.reviewId;
    if (!reviewId) return;
    try {
      await agentConnectionApi.status(agent.id, {
        kind: flow.kind === "restore" ? "restore" : "apply",
        reviewId,
      });
      await status.invalidate();
      open(null);
      setNotice(
        flow.kind === "restore"
          ? "已恢复变更前的连接"
          : "已回读连接配置 · 默认模型已选中",
      );
    } catch (reason) {
      const code = reason instanceof DesktopError ? reason.code : undefined;
      if (
        flow.kind === "switch" &&
        [
          "stale_snapshot",
          "invalid_input",
          "connection_unverified",
          "permission_denied",
          "grant_revoked",
        ].includes(code ?? "")
      ) {
        setFlow({ ...flow, review: undefined });
      } else if (code !== "busy") {
        setFlow({ ...flow, needsCheck: true });
      }
      await status.invalidate();
      throw reason;
    }
  }

  const choice = flow?.kind === "switch" ? flow.choice : null;
  const account = choice?.kind === "account" ? choice : null;
  const modelChoices =
    choice?.kind === "provider"
      ? availableProviderModelIds(choice.provider).map((id) => ({
          id,
          name: id,
        }))
      : (account?.catalog?.models ?? []);
  const title =
    flow?.kind === "restore"
      ? "恢复变更前的连接"
      : flow?.kind === "revoke"
        ? "撤销连接授权"
        : flow?.review
          ? "确认切换默认模型"
          : choice?.kind === "provider"
            ? choice.provider.name
            : choice?.kind === "service"
              ? choice.service.name
              : account
                ? accountLabel
                : undefined;
  const actionLabel =
    flow?.kind === "revoke"
      ? "撤销连接授权"
      : flow?.needsCheck
        ? "核对本次变更"
        : flow?.kind === "restore"
          ? "确认恢复"
          : flow?.review
            ? "确认切换默认模型"
            : account
              ? flow?.model
                ? "生成预览"
                : authorized
                  ? "读取账号模型"
                  : "授权并读取模型"
              : authorized
                ? "验证并生成预览"
                : "授权、验证并预览";
  return (
    <section
      aria-label={`${agent.name} 模型连接操作`}
      className={compact ? "min-w-48 max-w-72 space-y-1" : "space-y-3"}
    >
      {!compact && <h3 className="text-title font-medium">模型连接</h3>}
      <p
        role="status"
        aria-label={`${agent.name} 当前连接`}
        className="text-label text-fg-muted break-words"
        title={status.error ?? undefined}
      >
        {status.error ? "当前连接待核对" : currentLabel(status.data)}
      </p>
      <AgentConnectionPicker
        name={agent.name}
        providers={providers}
        services={services.error ? undefined : services.data?.services}
        emptyServices={
          services.error
            ? "模型服务读取未完成"
            : services.loading
              ? "正在读取模型服务…"
              : "暂无已发布模型服务"
        }
        disabled={!status.data || !!status.error || busy}
        native={bundled}
        nativeLabel={accountLabel}
        onSelect={(selection) => {
          if (selection.kind === "provider") {
            const provider = providers.find((item) => item.id === selection.id);
            if (provider)
              open({
                kind: "switch",
                choice: { kind: "provider", provider },
                model: null,
              });
          } else if (selection.kind === "service") {
            const service = services.data?.services.find(
              (item) => item.id === selection.id,
            );
            if (service)
              open({
                kind: "switch",
                choice: { kind: "service", service },
                model: null,
              });
          } else if (bundled) {
            open({ kind: "switch", choice: { kind: "account" }, model: null });
          }
        }}
      />
      {status.error && (
        <Button
          size="sm"
          variant="tertiary"
          loading={status.loading}
          onClick={status.reload}
        >
          重新读取连接
        </Button>
      )}
      {recovery && (
        <Button
          size="sm"
          variant="tertiary"
          disabled={busy}
          onClick={() => open({ kind: "restore", recovery })}
        >
          恢复变更前的连接
        </Button>
      )}
      {authorized && !compact && (
        <Button
          size="sm"
          variant="tertiary"
          disabled={busy}
          onClick={() => open({ kind: "revoke" })}
        >
          撤销连接授权
        </Button>
      )}
      {notice && (
        <p role="status" className="text-label text-fg-muted">
          {notice}
        </p>
      )}
      <Dialog
        open={flow !== null}
        onOpenChange={(next) => {
          if (!next && !inFlight.current) open(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{title}</DialogTitle>
            <DialogDescription>
              {flow?.kind === "restore"
                ? flow.recovery.kind === "account"
                  ? `恢复到 ${selectionLabel(flow.recovery.before)}，供新会话使用。`
                  : `恢复到 ${selectionLabel(flow.recovery.before)}，并撤下本次创建的模型入口。使用该入口的会话会受到影响。`
                : flow?.kind === "revoke"
                  ? "撤销后此插件的本机模型入口将不可用，Agent 原有设置需要在客户端核对。"
                  : flow?.review
                    ? `将 ${agent.name} 新会话的默认模型切换到「${flow.review.targetName}」。已有会话保留各自的选择。`
                    : authorized
                      ? `为 ${agent.name} 的新会话选择默认连接，预览后再确认写入。`
                      : `继续会授权 Local Agent 在本次运行中管理 ${agent.name} 的默认连接。预览后仍需确认写入。`}
              {flow?.kind === "switch" &&
                !flow.review &&
                (account
                  ? " 登录与套餐由 DeepSeek Harness 管理。"
                  : " 模型验证会发送短请求，可能产生费用。")}
            </DialogDescription>
          </DialogHeader>
          {flow?.kind === "switch" &&
            !flow.review &&
            modelChoices.length > 0 && (
              <Combobox
                items={modelChoices}
                itemToStringValue={(item) =>
                  item.name === item.id ? item.id : `${item.name} ${item.id}`
                }
                itemToStringLabel={(item) => item.name}
                value={modelChoices.find(({ id }) => id === flow.model) ?? null}
                disabled={busy}
                onValueChange={(item) =>
                  setFlow({ ...flow, model: item?.id ?? null })
                }
              >
                <ComboboxInput
                  aria-label="选择模型"
                  placeholder="搜索并选择模型"
                />
                <ComboboxContent>
                  <ComboboxEmpty>没有可用模型。</ComboboxEmpty>
                  <ComboboxList>
                    {(item: { id: string; name: string }) => (
                      <ComboboxItem key={item.id} value={item}>
                        {item.name}
                      </ComboboxItem>
                    )}
                  </ComboboxList>
                </ComboboxContent>
              </Combobox>
            )}
          {flow?.kind === "switch" &&
            !flow.review &&
            account?.catalog &&
            (account.catalog.signedIn ? (
              !modelChoices.length && (
                <p role="status">当前账号暂无可用模型。</p>
              )
            ) : (
              <p role="status">
                请先在 DeepSeek Harness 登录官方账号，再读取模型。
              </p>
            ))}
          {flow?.kind === "switch" && flow.review && (
            <p className="text-label text-fg-muted break-words">
              模型 {flow.review.models.join("、")}
            </p>
          )}
          {progress && <p role="status">{progress}</p>}
          {error && (
            <Alert status="warning" role="alert">
              <AlertTitle>连接操作未完成</AlertTitle>
              <AlertDescription>{error.message}</AlertDescription>
              {bundled &&
                error instanceof DesktopError &&
                ["bridge_unavailable", "account_unsupported"].includes(
                  error.code,
                ) && (
                  <Button
                    variant="tertiary"
                    size="sm"
                    disabled={busy}
                    onClick={() => void run(pluginUploadsApi.openDshBridges)}
                  >
                    打开 DSH 桥接文件夹
                  </Button>
                )}
            </Alert>
          )}
          <DialogFooter>
            {flow?.kind === "switch" &&
              !flow.review &&
              account &&
              flow.model && (
                <Button
                  variant="tertiary"
                  size="sm"
                  disabled={busy}
                  onClick={() => void run(() => readAccountModels(flow))}
                >
                  刷新模型
                </Button>
              )}
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() => open(null)}
            >
              取消
            </Button>
            <Button
              loading={busy}
              disabled={
                flow?.kind === "switch" &&
                flow.choice.kind === "provider" &&
                !flow.model
              }
              onClick={() => void run(submit)}
            >
              {actionLabel}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
