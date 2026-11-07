import { useEffect, useRef, useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { PageBody } from "@/components/ui/page-layout";
import { Textarea } from "@/components/ui/textarea";
import { useIcon } from "@/lib/icon-context";
import { workbenchApi } from "./api";
import { projectName } from "./task-board";
import { SessionThread } from "./session-thread";
import type { LiveAction, LiveConnection, OperationScope } from "./types";

export function LiveDesktop({ sessionId }: { sessionId?: string }) {
  const [view, setView] = useState<LiveConnection | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [readError, setReadError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const epoch = useRef(0);
  const operation = useRef(false);
  const [port, setPort] = useState("");
  const [user, setUser] = useState("opencode");
  const [password, setPassword] = useState("");
  const [directory, setDirectory] = useState("");
  const [draft, setDraft] = useState("");
  const sending = useRef<{
    id: string;
    text: string;
    connectionId: string;
    sessionId: string;
    scope?: OperationScope;
  } | null>(null);

  useEffect(() => {
    let mounted = true;
    let timer = 0;
    async function poll() {
      const generation = epoch.current;
      let delay = 1500;
      try {
        const next = await workbenchApi.readLive();
        if (mounted && generation === epoch.current && !operation.current) {
          setView(next);
          setReadError(null);
          setLoaded(true);
          delay = next?.selected ? 600 : 1800;
        }
      } catch (reason) {
        if (mounted && generation === epoch.current && !operation.current) {
          setReadError(String(reason));
          setLoaded(true);
        }
      }
      if (mounted) timer = window.setTimeout(() => void poll(), delay);
    }
    void poll();
    return () => {
      mounted = false;
      window.clearTimeout(timer);
    };
  }, [refresh]);

  async function act(action: LiveAction, retry = false) {
    if (operation.current) return;
    operation.current = true;
    epoch.current++;
    setBusy(true);
    setActionError(null);
    setNotice(null);
    try {
      if (retry && action.kind === "send" && action.scope && action.messageId) {
        const prior = await workbenchApi.operation(
          "liveSend",
          action.messageId,
          action.scope.storeId,
        );
        if (prior?.accepted && prior.targetId === action.sessionId) {
          setDraft("");
          sending.current = null;
          setNotice("已查到原消息回执。");
          return;
        }
      }
      await workbenchApi.controlLive(action);
      if (action.kind === "connect") setPassword("");
      if (action.kind === "disconnect") {
        setView(null);
        setDraft("");
        setReadError(null);
      }
      if (action.kind === "send") {
        setDraft("");
        sending.current = null;
        setNotice("OpenCode 已接收消息，等待原会话更新。");
      }
      if (action.kind === "stop") setNotice("OpenCode 已确认停止请求。");
      if (action.kind === "decide") setNotice("审批已交给原会话。");
    } catch (reason) {
      setActionError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      operation.current = false;
      setBusy(false);
      setRefresh((value) => value + 1);
    }
  }

  async function openNative() {
    if (!selected || busy) return;
    setBusy(true);
    setActionError(null);
    try {
      await workbenchApi.openDesktop("opencode", selected.session.title);
      setNotice(
        "已打开 OpenCode Desktop，并复制会话标题。可以在 Desktop 搜索这个标题，进入同一条会话。",
      );
    } catch (reason) {
      setActionError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  }

  const selected =
    view?.selected?.session.id === sessionId ? view?.selected : null;
  const id = view?.id;
  const canControl = Boolean(selected?.live && !busy && !readError);
  const portNumber = Number(port);
  const Back = useIcon("arrow-left");
  const Send = useIcon("arrow-up");
  const canSend =
    canControl &&
    selected?.phase === "idle" &&
    selected.requestScope !== "other" &&
    Boolean(selected.model && draft.trim());
  function send() {
    if (canSend && id && selected) {
      const retry = sending.current?.text === draft;
      if (!sending.current || sending.current.text !== draft) {
        sending.current = {
          id: crypto.randomUUID(),
          text: draft,
          connectionId: id,
          sessionId: selected.session.id,
          scope: view?.operationScope,
        };
      }
      void act(
        {
          kind: "send",
          connectionId: sending.current.connectionId,
          sessionId: sending.current.sessionId,
          text: draft,
          messageId: sending.current.id,
          scope: sending.current.scope,
        },
        retry,
      );
    }
  }
  return (
    <PageBody className="flex flex-col gap-4 p-4 sm:p-6">
      <header className="flex flex-wrap items-center justify-between gap-3">
        <Button
          leadingIcon={Back}
          variant="ghost"
          onClick={() => {
            window.location.hash = "/tasks";
          }}
        >
          返回看板
        </Button>
        {!sessionId && <h1 className="text-heading font-medium">连接设置</h1>}
      </header>
      {!view && !sessionId && (
        <section className="space-y-4 rounded-xl border border-border bg-surface-raised p-5">
          <h2 className="text-title font-medium">连接 OpenCode Desktop</h2>
          <p className="text-body text-fg-muted">
            如果 OpenCode Desktop
            提供默认本机服务的连接记录，可以直接连接并选择会话。
          </p>
          <Button
            disabled={!loaded || busy}
            onClick={() => void act({ kind: "discover" })}
          >
            连接默认本机服务
          </Button>
          <h3 className="text-title font-medium">手动连接共享服务</h3>
          <p className="text-body text-fg-muted">
            如果 Desktop
            已连接到你配置的共享服务，可在此填写服务信息。此入口使用 OpenCode
            旧版共享服务接口。
          </p>
          <form
            className="space-y-4"
            onSubmit={(event) => {
              event.preventDefault();
              if (
                loaded &&
                !busy &&
                Number.isInteger(portNumber) &&
                portNumber > 0 &&
                portNumber <= 65535 &&
                user.trim() &&
                password
              )
                void act({
                  kind: "connect",
                  port: portNumber,
                  user: user.trim(),
                  password,
                  directory: directory.trim() || null,
                });
            }}
          >
            <div className="grid gap-3 sm:grid-cols-3">
              <Field>
                <FieldLabel>本机端口</FieldLabel>
                <Input
                  inputMode="numeric"
                  value={port}
                  onChange={(event) => setPort(event.target.value)}
                  placeholder="127.0.0.1 上的端口"
                />
              </Field>
              <Field>
                <FieldLabel>用户名</FieldLabel>
                <Input
                  value={user}
                  onChange={(event) => setUser(event.target.value)}
                />
              </Field>
              <Field>
                <FieldLabel>服务密码</FieldLabel>
                <Input
                  type="password"
                  autoComplete="off"
                  value={password}
                  onChange={(event) => setPassword(event.target.value)}
                />
              </Field>
            </div>
            <Field>
              <FieldLabel>Desktop 项目目录（可选）</FieldLabel>
              <Input
                value={directory}
                onChange={(event) => setDirectory(event.target.value)}
                placeholder="/绝对路径/项目"
              />
              <FieldDescription>
                留空使用 OpenCode 服务的默认目录。连接凭据仅在本次运行中使用。
              </FieldDescription>
            </Field>
            <Button
              type="submit"
              disabled={
                !loaded ||
                busy ||
                !Number.isInteger(portNumber) ||
                portNumber <= 0 ||
                portNumber > 65535 ||
                !user.trim() ||
                !password
              }
            >
              {busy ? "正在连接…" : "连接共享服务"}
            </Button>
          </form>
          <Alert status="default">
            <AlertTitle>Claude Desktop</AlertTitle>
            <AlertDescription>
              正在运行的 Claude Desktop
              会话请在官方客户端内继续。工作台可以通过「桌面端交接」传递你确认过的内容。
            </AlertDescription>
          </Alert>
        </section>
      )}
      {view && !sessionId && (
        <div className="flex flex-wrap items-center gap-2">
          <Badge variant="dot" status={readError ? "warning" : "neutral"}>
            OpenCode {view.version}
          </Badge>
          <Button
            size="sm"
            variant="tertiary"
            onClick={() => setRefresh((value) => value + 1)}
          >
            刷新状态
          </Button>
          <Button
            size="sm"
            variant="tertiary"
            disabled={busy}
            onClick={() =>
              void act({ kind: "disconnect", connectionId: view.id })
            }
          >
            断开连接
          </Button>
          <p className="text-label text-fg-muted">
            断开只关闭工作台连接，桌面端会话继续由原服务运行。
          </p>
        </div>
      )}
      {readError && (
        <Alert status="warning">
          <AlertTitle>实时状态暂不可用</AlertTitle>
          <AlertDescription>{readError}</AlertDescription>
        </Alert>
      )}
      {actionError && (
        <Alert status="danger">
          <AlertTitle>操作未完成</AlertTitle>
          <AlertDescription>{actionError}</AlertDescription>
        </Alert>
      )}
      {notice && (
        <p role="status" className="text-label text-fg-muted">
          {notice}
        </p>
      )}
      {view && !sessionId && (
        <p className="text-body text-fg-muted">
          已连接的活动会话会显示在任务看板。
        </p>
      )}
      {sessionId && !selected && !readError && (
        <p role="status" className="text-body text-fg-muted">
          {loaded ? "连接已变化，请从看板重新打开会话。" : "正在打开会话…"}
        </p>
      )}
      {selected && id && (
        <section className="mx-auto flex min-h-0 w-full min-w-0 max-w-4xl flex-1 flex-col gap-4 rounded-xl border border-border bg-surface-raised p-5">
          <header className="flex flex-wrap items-start justify-between gap-3">
            <div className="min-w-0 space-y-1">
              <h2 className="break-words text-title font-medium">
                {selected.session.title === selected.session.id
                  ? "未命名会话"
                  : selected.session.title}
              </h2>
              <p className="text-label text-fg-muted">
                OpenCode Desktop · {projectName(selected.session.directory)}
              </p>
              <Badge
                status={selected.live && !readError ? "neutral" : "warning"}
              >
                {!selected.live || readError
                  ? "实时连接待确认"
                  : selected.approvals.length || selected.otherApproval
                    ? "等待审批"
                    : selected.phase === "busy"
                      ? "执行中"
                      : selected.phase === "retry"
                        ? "OpenCode 正在重试"
                        : selected.phase === "idle"
                          ? "本轮已结束"
                          : "状态未知"}
              </Badge>
            </div>
            <div className="flex flex-wrap gap-2">
              <Button
                variant="tertiary"
                size="sm"
                onClick={() => {
                  const query = new URLSearchParams({
                    source: "gateway",
                    range: "all",
                    agent: "opencode",
                    session: `opencode:${selected.session.id}`,
                  });
                  window.location.hash = `/requests?${query}`;
                }}
              >
                查看模型请求
              </Button>
              <Button
                variant="tertiary"
                size="sm"
                disabled={busy || !selected.live}
                onClick={() => void openNative()}
              >
                打开 OpenCode Desktop
              </Button>
              <Button
                variant="secondary"
                size="sm"
                disabled={
                  !canControl ||
                  (selected.phase !== "busy" && selected.phase !== "retry")
                }
                onClick={() =>
                  void act({
                    kind: "stop",
                    connectionId: id,
                    sessionId: selected.session.id,
                  })
                }
              >
                停止原会话本轮
              </Button>
            </div>
          </header>
          <details className="space-y-2">
            <summary className="cursor-pointer text-label text-fg-muted">
              会话详情
            </summary>
            <p className="break-all text-label text-fg-muted">
              {selected.session.directory}
            </p>
            <p className="break-all text-label text-fg-subtle">
              {selected.session.id} · {selected.model ?? "原模型待确认"}
            </p>
          </details>
          {selected.requestScope && (
            <p className="text-label text-fg-muted" role="status">
              {selected.requestScope === "current"
                ? "模型请求记录在当前 App，可从上方查看。"
                : selected.requestScope === "other"
                  ? "当前模型连接了另一个 App 的网关。请先在 Agents 中连接当前 App，续聊后才能在这里查看请求记录。"
                  : selected.requestScope === "external"
                    ? "当前模型直连外部服务，请求不会记录到当前 App 的网关中。"
                    : "模型请求的网关归属暂时无法确认，请核对连接。"}
            </p>
          )}
          {selected.problem && (
            <Alert status="warning">
              <AlertTitle>
                {selected.problem === "route"
                  ? "原会话的模型路由不可用"
                  : "OpenCode 报告本轮执行失败"}
              </AlertTitle>
              <AlertDescription>
                {selected.problem === "route"
                  ? "工作台保留原模型。请在 OpenCode 中查看服务和模型连接。"
                  : "请在 OpenCode 中查看原会话详情。"}
              </AlertDescription>
            </Alert>
          )}
          <SessionThread
            parts={selected.parts}
            agent="OpenCode"
            busy={
              selected.phase === "busy" &&
              !selected.approvals.length &&
              !selected.otherApproval
            }
            streaming={
              selected.phase === "busy" &&
              !selected.approvals.length &&
              !selected.otherApproval
            }
            composer={
              <>
                {(selected.approvals.length > 0 || selected.otherApproval) && (
                  <div className="max-h-56 space-y-2 overflow-y-auto">
                    {selected.approvals.map((approval) => (
                      <Alert key={approval.id} status="warning">
                        <AlertTitle>
                          {approval.tool === "bash"
                            ? "请求执行命令"
                            : "请求修改文件"}
                        </AlertTitle>
                        <AlertDescription>
                          <pre className="mb-2 max-h-24 overflow-auto break-all whitespace-pre-wrap font-mono">
                            {approval.targets.join("\n")}
                          </pre>
                          <div className="flex gap-2">
                            <Button
                              size="sm"
                              disabled={!canControl}
                              onClick={() =>
                                void act({
                                  kind: "decide",
                                  connectionId: id,
                                  sessionId: selected.session.id,
                                  approvalId: approval.id,
                                  allow: true,
                                })
                              }
                            >
                              仅允许这次
                            </Button>
                            <Button
                              size="sm"
                              variant="secondary"
                              disabled={!canControl}
                              onClick={() =>
                                void act({
                                  kind: "decide",
                                  connectionId: id,
                                  sessionId: selected.session.id,
                                  approvalId: approval.id,
                                  allow: false,
                                })
                              }
                            >
                              拒绝
                            </Button>
                          </div>
                        </AlertDescription>
                      </Alert>
                    ))}
                    {selected.otherApproval && (
                      <p className="text-label text-fg-muted">
                        还有需要在 OpenCode Desktop 处理的审批。
                      </p>
                    )}
                  </div>
                )}
                <form
                  className="space-y-2"
                  onSubmit={(event) => {
                    event.preventDefault();
                    send();
                  }}
                >
                  <Field>
                    <FieldLabel>继续对话</FieldLabel>
                    <Textarea
                      rows={2}
                      value={draft}
                      onChange={(event) => setDraft(event.target.value)}
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
                      placeholder="向 OpenCode 提问或补充要求"
                    />
                  </Field>
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <p className="text-label text-fg-subtle">
                      {selected.phase === "idle"
                        ? "⌘ Enter 发送"
                        : "本轮运行中，可先写草稿。"}
                    </p>
                    <Button
                      leadingIcon={Send}
                      type="submit"
                      disabled={!canSend}
                    >
                      发送
                    </Button>
                  </div>
                </form>
              </>
            }
          />
        </section>
      )}
    </PageBody>
  );
}
