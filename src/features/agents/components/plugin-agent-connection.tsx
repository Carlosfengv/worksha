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
import { modelServicesResource } from "@/features/model-services/queries";
import { availableProviderModelIds } from "@/features/providers/catalog";
import type { Provider } from "@/features/providers/types";
import { useSharedQuery, useSharedRead } from "@/shared/hooks/use-shared-read";
import { DesktopError } from "@/shared/lib/desktop-command";
import { createReadResourceFamily } from "@/shared/lib/read-resource";
import type { AgentDiscovery } from "@/shared/types/agent";
import {
  agentConnectionApi,
  type AgentConnectionRecovery,
  type AgentConnectionReview,
  type AgentConnectionStatus,
  type ConnectionTarget,
} from "../api";

const statusResource = createReadResourceFamily(
  (id: string) => agentConnectionApi.status(id),
  { interval: false },
);

type Choice =
  | { kind: "provider"; id: string; name: string; models: string[] }
  | { kind: "service"; id: string; name: string };
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
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const inFlight = useRef(false);
  const authorized = status.data?.state === "authorized" ? status.data : null;
  const recovery = !status.error ? authorized?.recovery : undefined;

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
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
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
          recovery: { reviewId: pending.reviewId, before: pending.before },
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
      const target: ConnectionTarget | null =
        choice.kind === "service"
          ? { kind: "service", id: choice.id }
          : model
            ? { kind: "provider", id: choice.id, model }
            : null;
      if (!target) return;
      if (!authorized) {
        await agentConnectionApi.status(agent.id, { kind: "authorize" });
        await status.invalidate();
      }
      const review = await agentConnectionApi.preview(agent.id, target);
      setFlow({ ...flow, review });
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

  const title =
    flow?.kind === "restore"
      ? "恢复变更前的连接"
      : flow?.kind === "revoke"
        ? "撤销连接授权"
        : flow?.review
          ? "确认切换默认模型"
          : flow?.choice.name;
  const actionLabel =
    flow?.kind === "revoke"
      ? "撤销连接授权"
      : flow?.needsCheck
        ? "核对本次变更"
        : flow?.kind === "restore"
          ? "确认恢复"
          : flow?.review
            ? "确认切换默认模型"
            : authorized
              ? "预览连接变更"
              : "授权并预览";
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
        onSelect={(selection) => {
          if (selection.kind === "provider") {
            const provider = providers.find((item) => item.id === selection.id);
            if (provider)
              open({
                kind: "switch",
                choice: {
                  kind: "provider",
                  id: provider.id,
                  name: provider.name,
                  models: availableProviderModelIds(provider),
                },
                model: null,
              });
          } else if (selection.kind === "service") {
            const service = services.data?.services.find(
              (item) => item.id === selection.id,
            );
            if (service)
              open({
                kind: "switch",
                choice: { kind: "service", id: service.id, name: service.name },
                model: null,
              });
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
                ? `恢复到 ${selectionLabel(flow.recovery.before)}，并撤下本次创建的模型入口。使用该入口的会话会受到影响。`
                : flow?.kind === "revoke"
                  ? "撤销后此插件的本机模型入口将不可用，Agent 原有设置需要在客户端核对。"
                  : flow?.review
                    ? `将 ${agent.name} 新会话的默认模型切换到「${flow.review.targetName}」。已有会话保留各自的选择。`
                    : authorized
                      ? `为 ${agent.name} 的新会话选择默认连接，预览后再确认写入。`
                      : `继续会授权 Local Agent 在本次运行中管理 ${agent.name} 的默认连接。预览后仍需确认写入。`}
            </DialogDescription>
          </DialogHeader>
          {flow?.kind === "switch" &&
            !flow.review &&
            flow.choice.kind === "provider" && (
              <Combobox
                items={flow.choice.models}
                value={flow.model}
                disabled={busy}
                onValueChange={(model) => setFlow({ ...flow, model })}
              >
                <ComboboxInput
                  aria-label="选择模型"
                  placeholder="搜索并选择模型"
                />
                <ComboboxContent>
                  <ComboboxEmpty>没有可用模型。</ComboboxEmpty>
                  <ComboboxList>
                    {(model: string) => (
                      <ComboboxItem key={model} value={model}>
                        {model}
                      </ComboboxItem>
                    )}
                  </ComboboxList>
                </ComboboxContent>
              </Combobox>
            )}
          {flow?.kind === "switch" && flow.review && (
            <p className="text-label text-fg-muted break-words">
              模型 {flow.review.models.join("、")}
            </p>
          )}
          {error && (
            <Alert status="warning" role="alert">
              <AlertTitle>连接操作未完成</AlertTitle>
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}
          <DialogFooter>
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
