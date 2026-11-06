import { useState } from "react";
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
import { Field, FieldLabel } from "@/components/ui/field";
import { Textarea } from "@/components/ui/textarea";
import { workbenchApi } from "./api";
import {
  agentLabels,
  desktopLabels,
  type Task,
  type ClaudeConnection,
} from "./types";

export function handoffText(task: Task): string {
  const first = task.parts.find((part) => part.kind === "user");
  const recent = [...task.parts]
    .reverse()
    .find((part) => part.kind === "assistant");
  return [
    `项目目录：${task.directory}`,
    `原执行 Agent：${agentLabels[task.agent]}`,
    first ? `任务目标：\n${first.text.slice(0, 4500)}` : "",
    recent ? `上次回复：\n${recent.text.slice(0, 4500)}` : "",
    "待接续的工作：",
  ]
    .filter(Boolean)
    .join("\n\n");
}

export function DesktopHandoff({
  task,
  connection,
}: {
  task: Task;
  connection?: ClaudeConnection;
}) {
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ text: string; error: boolean } | null>(
    null,
  );
  const target = desktopLabels[task.agent];

  async function run(action: "export" | "desktop") {
    setBusy(true);
    setResult(null);
    try {
      if (action === "export") {
        const saved = await workbenchApi.exportOpenCode(task.id);
        if (saved)
          setResult({
            text: "已保存会话副本。用 opencode import <文件路径> 导入官方 OpenCode 本机配置后，可在连接该配置的桌面客户端中查找。",
            error: false,
          });
      } else {
        await workbenchApi.openDesktop(task.agent, draft);
        setResult({
          text: `已复制交接内容并请求打开 ${target}。在对应项目的新会话中粘贴后，由你决定是否发送。`,
          error: false,
        });
      }
    } catch (reason) {
      setResult({
        text: reason instanceof Error ? reason.message : String(reason),
        error: true,
      });
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <Button
        size="sm"
        variant="tertiary"
        onClick={() => {
          setDraft(handoffText(task));
          setResult(null);
          setOpen(true);
        }}
      >
        {task.agent === "claude" ? "CLI / Desktop 接续" : "桌面端交接"}
      </Button>
      <Dialog
        open={open}
        onOpenChange={(next) => {
          if (!busy) setOpen(next);
        }}
      >
        <DialogContent size="lg" className="max-h-[85dvh] overflow-y-auto">
          <DialogHeader>
            <DialogTitle>交给 {target}</DialogTitle>
            <DialogDescription>
              {task.agent === "claude"
                ? task.nativeHistory
                  ? "这条任务已保存为原生 Claude Code 会话，可在同一项目中按会话 ID 恢复。当前任务通过 API / 网关执行，Claude Desktop 的原会话接续要求兼容的订阅登录。也可以复制上下文，在 Desktop 新建会话。"
                  : "这条任务保存在 App 的独立会话目录。复制确认过的内容后，可在 Claude Desktop 的 Code 页面新建会话接续。"
                : "可复制确认过的内容，在 OpenCode Desktop 中开启新会话。也可以导出原 OpenCode 会话的副本。"}
            </DialogDescription>
          </DialogHeader>
          {task.agent === "claude" && connection && (
            <p role="status" className="text-label text-fg-muted">
              {connection.desktopRequestScope === "current"
                ? "Claude Desktop 的本机配置已连接当前 App 网关；重启 Desktop 后新请求会记录在这里。"
                : connection.desktopRequestScope === "other"
                  ? "Claude Desktop 仍连接另一个 App 网关，请在 Agents 中连接当前 App，并重启 Desktop。"
                  : connection.desktopRequestScope === "external"
                    ? "Claude Desktop 当前直连外部服务，请求不会记录到当前 App 网关。"
                    : (connection.desktopError ??
                      "Claude Desktop 的网关连接尚未确认。")}
            </p>
          )}
          {task.agent === "claude" && task.cliResumeCommand && (
            <Field>
              <FieldLabel>在 CLI 继续同一会话</FieldLabel>
              <Textarea
                rows={3}
                readOnly
                aria-label="CLI 续聊命令"
                value={task.cliResumeCommand}
              />
              <p className="text-label text-fg-muted">
                先结束当前客户端中的本轮，再在终端运行；会话 ID 和模型保持不变。
              </p>
              <Button
                size="sm"
                variant="tertiary"
                disabled={task.nativeBusy}
                onClick={async () => {
                  try {
                    await navigator.clipboard.writeText(task.cliResumeCommand!);
                    setResult({ text: "已复制 CLI 续聊命令。", error: false });
                  } catch {
                    setResult({
                      text: "未能复制，请选择上方命令手动复制。",
                      error: true,
                    });
                  }
                }}
              >
                复制 CLI 续聊命令
              </Button>
            </Field>
          )}
          <Field>
            <FieldLabel>交接内容</FieldLabel>
            <Textarea
              rows={8}
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
            />
          </Field>
          {result && (
            <Alert status={result.error ? "danger" : "default"}>
              <AlertTitle>{result.error ? "操作未完成" : "下一步"}</AlertTitle>
              <AlertDescription>{result.text}</AlertDescription>
            </Alert>
          )}
          <DialogFooter>
            {task.agent === "opencode" && task.sessionId && (
              <Button
                variant="tertiary"
                disabled={busy}
                onClick={() => void run("export")}
              >
                导出 OpenCode 会话
              </Button>
            )}
            <Button
              disabled={
                busy ||
                !draft.trim() ||
                new TextEncoder().encode(draft).length > 24 * 1024
              }
              onClick={() => void run("desktop")}
            >
              复制内容并打开桌面端
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
