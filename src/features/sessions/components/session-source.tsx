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
import { sessionsApi } from "@/features/sessions/api";
import { sourceLabel } from "@/features/sessions/display";
import type { SessionSource } from "@/features/sessions/types";
import { useState } from "react";
import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";

export function SessionSourceControl({
  source,
  refresh,
}: {
  source: SessionSource;
  refresh: () => Promise<void>;
}) {
  const { isSharedSource, sourceName } = useAgentDirectory();
  const shared = isSharedSource(source.agentId);
  const [open, setOpen] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const save = async (enabled: boolean) => {
    setSaving(true);
    setError(null);
    try {
      await sessionsApi.setSessionSource(source.agentId, enabled);
      await refresh();
      setOpen(false);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setSaving(false);
    }
  };
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <Badge variant="dot" status="neutral">
          {sourceLabel(source)}
        </Badge>
        <Button
          variant="tertiary"
          size="sm"
          loading={saving}
          onClick={() => {
            if (source.enabled) void save(false);
            else setOpen(true);
          }}
        >
          {source.enabled ? "停止读取" : "读取本机会话"}
        </Button>
      </div>
      {source.enabled && (
        <p className="text-label text-fg-muted">
          {["pending", "scanning"].includes(source.status)
            ? `正在检查 ${source.scannedFiles} / ${source.totalFiles} ${source.agentId === "opencode" ? "个会话" : "个文件"}`
            : shared
              ? `共享历史 ${source.sessionCount} 个会话 · 含 CLI、桌面及未知来源`
              : `已读取 ${source.sessionCount} 个历史会话`}
        </p>
      )}
      {(source.error || error) && (
        <Alert>
          <AlertTitle>会话来源状态</AlertTitle>
          <AlertDescription>{error ?? source.error}</AlertDescription>
        </Alert>
      )}
      {source.enabled &&
        source.agentId === "codex" &&
        source.clientAttribution && (
          <p className="text-label text-fg-muted">
            桌面本机工具执行日志：已读{" "}
            {source.clientAttribution.scannedLogFiles} 个日志，关联{" "}
            {source.clientAttribution.matchedCalls} 次调用；
            {source.clientAttribution.conflictingCalls} 次证据冲突，
            {source.clientAttribution.ambiguousExecutions} 条关联不唯一。
            {source.clientAttribution.status === "partial"
              ? "部分日志不可读或超出范围。"
              : source.clientAttribution.status === "not_found"
                ? "未发现受支持的执行日志。"
                : ""}
            仅补充已确认的工具执行，不据此分配整轮或模型响应 Token。
          </p>
        )}
      <Dialog
        open={open}
        onOpenChange={(next) => {
          if (!saving) setOpen(next);
        }}
      >
        <DialogContent size="lg">
          <DialogHeader>
            <DialogTitle>
              读取 {sourceName(source.agentId)} 本机会话
            </DialogTitle>
            <DialogDescription>
              查看本机已有对话、模型用量和工具调用。
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-3 text-body text-fg-muted">
            <p>只读以下原生会话及关联来源；可随时停止读取。</p>
            {shared && (
              <p>
                此来源由 CLI
                与桌面端共用。启用会读取两端及来源未知的本机记录；停止读取会同时影响两端。共享会话只索引一次，来源不明的用量不会分配给任一端。
              </p>
            )}
            {source.agentId === "antigravity" && (
              <p>
                读取 Antigravity App 的 conversations/*.db（SQLite / Protobuf
                step_format=0），
                包含原生对话、模型用量、工具结果与明确错误；有界读取最新步骤状态，30
                秒后过期。 同时读取 mcp_config.json 服务定义，以及全局
                .gemini/config 和已知会话项目的规则、Skill 定义。 不读取 IDE /
                CLI、旧
                .pb、登录凭据、隐藏推理、附件或外置结果，不修改原应用配置。
                原生历史不提供实时 TTFT、Token/s、账单或账户额度。
              </p>
            )}
            {source.agentId === "opencode" && (
              <p>
                同时只读本机及会话项目的 opencode.json / jsonc、.opencode
                配置中的 MCP
                服务定义，用于关联工具来源；不会启动服务或读取配置引用的文件。
                同时读取当前 opencode.log 末尾最多 2 MiB 中明确关联 Session
                的模型错误，补充尚未写入消息的连接异常；不展示其他日志内容或读取凭据文件。
              </p>
            )}
            {source.agentId === "codex" && (
              <p>
                同时只读桌面工具日志和 logs_2.sqlite
                中的执行元数据。历史工具按调用
                ID、轮次和工具名关联；当前活动需核对会话、轮次与执行进程。桌面核对直属父进程，TUI
                核对同一运行的启动记录和终端身份。文本日志最多 256
                MiB，执行库文件上限 512
                MiB、仅按相关索引读取。不保存日志正文或读取进程参数、环境；消息展示、创建者和进程存在本身不作为来源证据。
              </p>
            )}
            {source.locations.map((path) => (
              <p
                key={path}
                className="break-all rounded-lg border border-border p-3 font-mono text-label"
              >
                {path}
              </p>
            ))}
            <p>
              原始正文和工具结果按需读取，不额外保存副本。不会修改 Agent
              配置或会话文件，也不读取登录凭据。
            </p>
            <p>
              此通道提供历史会话。实时 TTFT、生成速度和 API 账单需另行接入。
            </p>
          </div>
          {error && (
            <p role="alert" className="mt-3 text-body text-fg-muted">
              {error}
            </p>
          )}
          <DialogFooter>
            <Button
              variant="ghost"
              disabled={saving}
              onClick={() => setOpen(false)}
            >
              取消
            </Button>
            <Button
              variant="neutral"
              loading={saving}
              onClick={() => void save(true)}
            >
              开始读取
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
