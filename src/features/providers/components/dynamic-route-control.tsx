import { useState } from "react";
import { Alert, AlertTitle, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import { providersApi } from "../api";
import {
  dynamicBindingFor,
  type DynamicChange,
  type NativeConfig,
  type ProviderStatus,
} from "../types";

export function DynamicRouteControl({
  status,
  config,
  preview,
  setPreview,
  reload,
}: {
  status: ProviderStatus;
  config: NativeConfig | undefined;
  preview: DynamicChange | null;
  setPreview: (value: DynamicChange | null) => void;
  reload: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const binding = config ? dynamicBindingFor(status, config) : undefined;
  const changes =
    status.dynamicChanges?.filter((c) => c.bindingId === binding?.id) ?? [];
  const pending = preview ?? changes.find((c) => c.state === "pending") ?? null;
  const last = changes.find((c) => c.state === "applied");
  const open = !!pending && pending.id !== dismissed;
  const act = async (
    change: DynamicChange,
    action: "commit" | "cancel" | "restore",
  ) => {
    setBusy(true);
    setError(null);
    try {
      const result = await providersApi.dynamicRouteAction(change, action);
      setDismissed(null);
      setPreview(result.state === "pending" ? result : null);
      setNotice(
        action === "commit"
          ? "路由已保存，等待后续调用确认。工具续调保持原路由；历史不兼容时请新建会话。"
          : action === "cancel"
            ? "已撤销动态预览。"
            : "恢复预览已生成，确认后发布新的路由版本。",
      );
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
      reload();
      window.dispatchEvent(new Event("provider-status-changed"));
    }
  };
  return (
    <>
      {binding && (
        <Alert>
          <AlertTitle>已连接固定动态入口</AlertTitle>
          <AlertDescription>
            <p>
              默认目标：{binding.target.provider.name} ·{" "}
              {binding.target.provider.model}
            </p>
            {binding.lastObserved && (
              <p>
                最近调用：{binding.lastObserved.providerName} ·{" "}
                {binding.lastObserved.model ?? "模型未知"} ·{" "}
                {binding.lastObserved.status}。最近调用不代表所有会话都已切换。
              </p>
            )}
            <p>
              Agent 使用“Local Agent ·
              动态模型”时跟随此选择。可确认的下一用户轮次切换；工具续调、历史思考和未知轮次保留原路由。实际目标请核对调用记录。
            </p>
            <p>
              新增模型能力或退出网关仍需重新加载；恢复原生配置使用下方加密备份记录。
            </p>
            {last && (
              <Button
                variant="tertiary"
                size="sm"
                disabled={busy || !!pending}
                onClick={() => void act(last, "restore")}
              >
                预览恢复上次路由
              </Button>
            )}
            {pending && !open && (
              <Button
                variant="tertiary"
                size="sm"
                onClick={() => setDismissed(null)}
              >
                继续确认动态切换
              </Button>
            )}
          </AlertDescription>
        </Alert>
      )}
      {notice && (
        <p role="status" className="text-body text-fg-muted">
          {notice}
        </p>
      )}
      {error && !open && (
        <Alert status="danger">
          <AlertTitle>动态操作未完成</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      <Dialog
        open={open}
        onOpenChange={(open) => {
          if (!open && !busy) {
            setDismissed(pending?.id ?? null);
            setPreview(null);
          }
        }}
      >
        <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-y-auto">
          <DialogHeader>
            <DialogTitle>确认动态路由切换</DialogTitle>
            <DialogDescription>
              {pending?.before.provider.name} · {pending?.before.provider.model}{" "}
              → {pending?.target.name} · {pending?.target.model}。固定入口和
              Agent
              配置不变；新会话或可确认且历史兼容的下一轮使用目标。现有工具循环继续原路由。
            </DialogDescription>
          </DialogHeader>
          {error && (
            <Alert status="danger">
              <AlertTitle>动态切换未完成</AlertTitle>
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}
          <p className="break-all text-body text-fg-muted">
            {pending?.target.endpoint} · {pending?.target.protocol}
          </p>
          <DialogFooter>
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() => pending && void act(pending, "cancel")}
            >
              撤销动态预览
            </Button>
            <Button
              loading={busy}
              onClick={() => pending && void act(pending, "commit")}
            >
              确认发布路由
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
