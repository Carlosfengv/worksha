import { useRef, useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Field, FieldLabel } from "@/components/ui/field";
import { Textarea } from "@/components/ui/textarea";
import { useIcon } from "@/lib/icon-context";
import { DesktopError } from "@/shared/lib/desktop-command";
import { workbenchApi } from "./api";
import { DesktopHandoff } from "./desktop-handoff";
import { SessionThread } from "./session-thread";
import { projectName, TaskStatus } from "./task-board";
import { agentLabels } from "./types";
import type {
  ClaudeConnection,
  Task,
  TaskDraft,
  TaskSummary,
  TurnAction,
} from "./types";

function Issue({ title, text }: { title: string; text: string }) {
  return (
    <Alert status="danger">
      <AlertTitle>{title}</AlertTitle>
      <AlertDescription>{text}</AlertDescription>
    </Alert>
  );
}

export function TaskConversation({
  task,
  claudeConnection,
  activeTaskId,
  unavailable,
  accept,
  initialDraft,
  onHandoff,
  related,
  onOpen,
}: {
  task: Task;
  claudeConnection?: ClaudeConnection;
  activeTaskId: string | null;
  unavailable: boolean;
  accept: (task: Task) => void;
  initialDraft?: TaskDraft;
  onHandoff: () => void;
  related: TaskSummary[];
  onOpen: (id: string) => void;
}) {
  const agentName = agentLabels[task.agent];
  const [text, setText] = useState(initialDraft?.text ?? "");
  const [error, setError] = useState<string | null>(
    initialDraft?.error ?? null,
  );
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const [unknown, setUnknown] = useState<{ id: string; text: string } | null>(
    initialDraft?.uncertainTurn
      ? { id: initialDraft.uncertainTurn, text: initialDraft.text }
      : null,
  );
  const seen = unknown && task.parts.some((part) => part.id === unknown.id);
  const draft = seen && text.trim() === unknown.text ? "" : text;
  const current = activeTaskId === task.id;
  const canSend =
    !activeTaskId &&
    !task.nativeBusy &&
    !busy &&
    !unavailable &&
    !(
      task.agent === "claude" &&
      ["other", "unknown"].includes(claudeConnection?.requestScope ?? "")
    ) &&
    Boolean(draft.trim());
  const Send = useIcon("arrow-up");

  async function perform(
    operation: () => Promise<Task>,
    sending?: { id: string; text: string },
  ) {
    if (pending.current) return;
    pending.current = true;
    setBusy(true);
    setError(null);
    try {
      const result = await operation();
      accept(result);
      if (sending) {
        setText("");
        setUnknown(null);
      }
    } catch (reason) {
      if (sending && reason instanceof DesktopError && reason.outcomeUnknown)
        setUnknown(sending);
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      pending.current = false;
      setBusy(false);
    }
  }

  function send() {
    if (!canSend) return;
    const message = draft.trim();
    if (new TextEncoder().encode(message).length > 24 * 1024) {
      setError("单条消息最多 24 KB，请缩短后再发送。");
      return;
    }
    const id =
      !seen && unknown?.text === message ? unknown.id : crypto.randomUUID();
    void perform(() => workbenchApi.send(task.id, id, message), {
      id,
      text: message,
    });
  }

  function control(
    action:
      | Omit<TurnAction, "taskId" | "turnId">
      | { kind: "decide"; approvalId: string; allow: boolean },
  ) {
    if (!task.turnId || !current || busy || unavailable) return;
    const payload = {
      taskId: task.id,
      turnId: task.turnId,
      ...action,
    } as TurnAction;
    void perform(() => workbenchApi.control(payload));
  }

  return (
    <section className="mx-auto flex min-h-0 w-full min-w-0 max-w-4xl flex-1 flex-col gap-4 rounded-xl border border-border bg-surface-raised p-4 sm:p-5">
      <header className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 space-y-1">
          <div className="flex flex-wrap items-center gap-2">
            <h2 className="break-all text-title font-medium">{task.title}</h2>
            <TaskStatus phase={task.nativeBusy ? "native" : task.phase} />
          </div>
          <p className="text-label text-fg-muted">
            {agentName} · {projectName(task.directory)} ·{" "}
            {task.nativeHistory ? "原生会话" : "App 会话"}
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          {task.sessionId && (
            <Button
              size="sm"
              variant="tertiary"
              onClick={() => {
                const query = new URLSearchParams({
                  source: "gateway",
                  range: "all",
                  agent: task.agent,
                  session: `${task.agent}:${task.sessionId}`,
                });
                window.location.hash = `/requests?${query}`;
              }}
            >
              查看模型请求
            </Button>
          )}
          {!current && task.sessionId && task.phase !== "unknown" && (
            <DesktopHandoff task={task} connection={claudeConnection} />
          )}
          {current && (
            <Button
              variant="secondary"
              size="sm"
              disabled={busy || unavailable || task.phase === "stopping"}
              onClick={() => control({ kind: "stop" })}
            >
              {task.phase === "stopping" ? "正在停止…" : "停止本轮"}
            </Button>
          )}
        </div>
      </header>
      {task.agent === "claude" && claudeConnection && (
        <p className="text-label text-fg-muted" role="status">
          {claudeConnection.requestScope === "current"
            ? "模型请求记录在当前 App，可从上方查看。"
            : claudeConnection.requestScope === "other"
              ? "Claude Code 连接了另一个 App 网关。请在 Agents 中连接当前 App 后再继续，原模型保持不变。"
              : claudeConnection.requestScope === "external"
                ? "Claude Code 直连外部服务，请求不会记录到当前 App 的网关中。"
                : "无法确认 Claude Code 的网关归属，请核对连接后再继续。"}
        </p>
      )}
      <details className="space-y-2">
        <summary className="cursor-pointer text-label text-fg-muted">
          会话详情
        </summary>
        <p className="break-all text-label text-fg-muted">{task.directory}</p>
        <p className="break-all text-label text-fg-subtle">{task.route}</p>
        <div className="flex flex-wrap items-center gap-2">
          {related.map((item) => (
            <Button
              key={item.id}
              size="sm"
              variant="ghost"
              onClick={() => onOpen(item.id)}
            >
              {item.id === task.parentTaskId ? "交接来源" : "接手任务"} ·{" "}
              {agentLabels[item.agent]}
            </Button>
          ))}
          {!current && task.sessionId && task.phase !== "unknown" && (
            <>
              <Button size="sm" variant="tertiary" onClick={onHandoff}>
                交给{" "}
                {agentLabels[task.agent === "opencode" ? "claude" : "opencode"]}
              </Button>
            </>
          )}
        </div>
      </details>
      <SessionThread
        parts={task.parts.map(({ kind, ...part }) => ({ ...part, role: kind }))}
        agent={agentName}
        busy={current && task.phase !== "approval"}
        streaming={current && task.phase === "running"}
        composer={
          <>
            {task.nativeBusy && (
              <p className="text-label text-fg-muted">
                原生 Claude Code
                正在使用这条会话。请先在原生客户端退出这条会话，再回到这里继续。
              </p>
            )}
            {task.error && <Issue title="本轮执行未完成" text={task.error} />}
            {task.approval && (
              <Alert status="warning">
                <AlertTitle>
                  {task.approval.tool === "bash"
                    ? `${agentName} 请求执行命令`
                    : `${agentName} 请求修改文件`}
                </AlertTitle>
                <AlertDescription>
                  <div className="space-y-2">
                    <p className="break-all">{task.directory}</p>
                    <pre className="max-h-32 overflow-auto break-all whitespace-pre-wrap font-mono">
                      {task.approval.targets.join("\n")}
                    </pre>
                    <div className="flex flex-wrap gap-2">
                      <Button
                        size="sm"
                        disabled={busy || unavailable || !current}
                        onClick={() =>
                          task.approval &&
                          control({
                            kind: "decide",
                            approvalId: task.approval.id,
                            allow: true,
                          })
                        }
                      >
                        仅允许这次
                      </Button>
                      <Button
                        size="sm"
                        variant="secondary"
                        disabled={busy || unavailable || !current}
                        onClick={() =>
                          task.approval &&
                          control({
                            kind: "decide",
                            approvalId: task.approval.id,
                            allow: false,
                          })
                        }
                      >
                        拒绝
                      </Button>
                    </div>
                  </div>
                </AlertDescription>
              </Alert>
            )}
            {error && !seen && <Issue title="操作未确认" text={error} />}
            {unknown && !seen && (
              <p className="text-label text-fg-muted">
                正在核对原发送结果。原内容再次发送会沿用同一操作编号。
              </p>
            )}
            <form
              className="space-y-2"
              onSubmit={(event) => {
                event.preventDefault();
                send();
              }}
            >
              <Field>
                <FieldLabel>
                  {task.sessionId ? "继续对话" : "任务内容"}
                </FieldLabel>
                <Textarea
                  aria-label="任务消息"
                  placeholder={
                    task.sessionId
                      ? `继续向 ${agentName} 提问或补充要求`
                      : "例如，分析测试失败的原因"
                  }
                  rows={2}
                  value={draft}
                  onChange={(event) => {
                    if (seen) setUnknown(null);
                    setText(event.target.value);
                  }}
                  onKeyDown={(event) => {
                    if (
                      event.key === "Enter" &&
                      (event.metaKey || event.ctrlKey) &&
                      !event.nativeEvent.isComposing
                    ) {
                      event.preventDefault();
                      send();
                    }
                  }}
                />
              </Field>
              <div className="flex flex-wrap items-center justify-between gap-2">
                <p className="text-label text-fg-subtle">
                  {activeTaskId
                    ? current
                      ? "当前任务执行中，可先写下追问。"
                      : "另一项任务执行中，可先写草稿。"
                    : "⌘ Enter 发送"}
                </p>
                <Button leadingIcon={Send} disabled={!canSend} type="submit">
                  发送
                </Button>
              </div>
            </form>
          </>
        }
      />
    </section>
  );
}
