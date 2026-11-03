import type { Snapshot } from "@/app/types";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
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
import { observationApi } from "@/features/observation/api";
import type { ObservationBinding } from "@/features/observation/types";
import type { AgentId } from "@/shared/types/agent";
import { useState } from "react";

export function ObservationConnection({
  agentId,
  data,
  onSaved,
}: {
  agentId: AgentId;
  data: Snapshot;
  onSaved?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<ObservationBinding | null>(null);
  const existing =
    saved ?? data.observationBindings.find((b) => b.agentId === agentId);
  const choices = data.providers.filter(
    (p) => p.protocol === (agentId === "claude" ? "messages" : "responses"),
  );
  const [providerId, setProviderId] = useState(
    existing?.provider.id ?? choices[0]?.id ?? "",
  );
  const provider = choices.find((p) => p.id === providerId);
  if (agentId === "opencode") return null;
  const commit = async (enabled: boolean) => {
    setBusy(true);
    setError(null);
    try {
      const bindings = await observationApi.observationBinding(
        agentId,
        providerId,
        existing?.revision ?? 0,
        enabled,
      );
      setSaved(bindings.find((b) => b.agentId === agentId) ?? null);
      onSaved?.();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <Button variant="tertiary" size="sm" onClick={() => setOpen(true)}>
        {existing?.enabled ? "观测入口" : "接入流式观测"}
      </Button>
      <Dialog
        open={open}
        onOpenChange={(next) => {
          if (!busy) {
            setOpen(next);
            setError(null);
          }
        }}
      >
        <DialogContent size="lg">
          <DialogHeader>
            <DialogTitle>
              {agentId === "claude" ? "Claude Code CLI" : "Codex CLI"} 调用观测
            </DialogTitle>
            <DialogDescription>
              为新启动的 API 会话生成独立入口，实测流式计时和速度。
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4 text-body text-fg-muted">
            {existing?.enabled ? (
              <>
                <p>
                  入口已就绪，收到真实请求后显示计时。目标：
                  {existing.provider.name} · {existing.provider.endpoint}
                </p>
                <p>
                  在已设置{" "}
                  {agentId === "claude"
                    ? "ANTHROPIC_API_KEY"
                    : "OPENAI_API_KEY"}{" "}
                  的终端中，执行以下入口；原有 CLI 参数可追加在后面。
                </p>
                <p className="break-all rounded-lg border border-border p-3 font-mono text-label">
                  &quot;{existing.launcherPath}&quot;
                </p>
                <p>
                  保持桌面 App
                  运行。应用重新启动后需要重新启动此入口；进行中的原生会话保持原连接。
                </p>
                <p>
                  当前仍有 {existing.activeRequests}{" "}
                  个观测请求。停止入口只拒绝后续请求，已开始的请求继续完成。
                </p>
              </>
            ) : (
              <>
                <Select value={providerId} onValueChange={setProviderId}>
                  <SelectTrigger aria-label="观测目标 Provider" />
                  <SelectContent>
                    {choices.map((p) => (
                      <SelectItem key={p.id} value={p.id}>
                        {p.name} · {p.protocol}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <p className="break-all">
                  目标服务：{provider?.endpoint ?? "请先配置兼容 Provider"}
                </p>
                <p>
                  仅生成应用数据目录中的启动脚本，通过当前终端的 API Key
                  调用所选服务商。消息和认证头仅用于该次转发，观测库只保留模型、计时、用量和关联
                  ID。
                </p>
                <p>
                  现有 Agent
                  配置不会被改写。订阅账号继续使用原生命令和只读活动观测，不提取订阅登录令牌；原生缺少流式时序时不会补算速度。
                </p>
                <p>
                  Claude 使用 Messages；Codex 使用 Responses HTTP
                  流。此独立观测入口保持同协议；跨协议连接请使用 Agent 详情中的
                  Provider 切换。WebSocket 与后台响应尚不支持。
                </p>
              </>
            )}
            {error && (
              <Alert>
                <AlertTitle>入口未完成更新</AlertTitle>
                <AlertDescription>
                  {error} 请关闭后重新打开以核对最新配置。
                </AlertDescription>
              </Alert>
            )}
          </div>
          <DialogFooter>
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() => setOpen(false)}
            >
              关闭
            </Button>
            {existing?.enabled ? (
              <Button
                variant="secondary"
                loading={busy}
                onClick={() => void commit(false)}
              >
                停止接收新请求
              </Button>
            ) : (
              <Button
                disabled={!provider}
                loading={busy}
                onClick={() => void commit(true)}
              >
                确认范围并生成入口
              </Button>
            )}
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
