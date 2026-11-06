import { useEffect, useRef, useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { useIcon } from "@/lib/icon-context";
import { workbenchApi } from "./api";
import { agentLabels } from "./types";
import type { LiveBoard, Phase, TaskSummary } from "./types";

type BoardPhase = Phase | "ended" | "native";

const labels: Record<BoardPhase, string> = {
  ready: "待发送",
  starting: "正在启动",
  running: "执行中",
  approval: "等待审批",
  stopping: "正在停止",
  completed: "本轮完成",
  failed: "本轮失败",
  stopped: "已停止",
  unknown: "待确认",
  ended: "本轮已结束",
  native: "在原生客户端中",
};

export function TaskStatus({ phase }: { phase: BoardPhase }) {
  return (
    <Badge
      size="sm"
      variant="dot"
      status={
        phase === "failed"
          ? "danger"
          : phase === "approval" || phase === "unknown"
            ? "warning"
            : phase === "completed"
              ? "success"
              : "neutral"
      }
    >
      {labels[phase]}
    </Badge>
  );
}

const lanes = [
  { id: "running", title: "进行中", empty: "当前没有运行中的任务" },
  { id: "attention", title: "待处理", empty: "没有待审批或待确认的任务" },
  { id: "ready", title: "待开始", empty: "没有待发送的任务" },
  { id: "ended", title: "已结束", empty: "完成或停止的任务会出现在这里" },
] as const;

function laneFor(phase: BoardPhase) {
  if (phase === "approval" || phase === "unknown" || phase === "native")
    return "attention";
  if (phase === "ready") return "ready";
  if (phase === "running" || phase === "starting" || phase === "stopping")
    return "running";
  return "ended";
}

export function projectName(directory: string) {
  return directory.split("/").filter(Boolean).at(-1) ?? directory;
}

export function TaskBoard({
  tasks,
  loading,
  openTask,
}: {
  tasks: TaskSummary[];
  loading: boolean;
  openTask: (id: string) => void;
}) {
  const [desktop, setDesktop] = useState<{
    board: LiveBoard | null;
    error: string | null;
  }>({ board: null, error: null });
  const [opening, setOpening] = useState<string | null>(null);
  const [openError, setOpenError] = useState<string | null>(null);
  const inFlight = useRef(false);
  const Chevron = useIcon("chevron-right");

  useEffect(() => {
    let mounted = true;
    let timer = 0;
    async function poll() {
      try {
        const board = await workbenchApi.readLiveBoard();
        if (mounted) setDesktop({ board, error: null });
      } catch (reason) {
        if (mounted)
          setDesktop((current) => ({
            ...current,
            error: reason instanceof Error ? reason.message : String(reason),
          }));
      }
      if (mounted) timer = window.setTimeout(() => void poll(), 3000);
    }
    void poll();
    return () => {
      mounted = false;
      window.clearTimeout(timer);
    };
  }, []);

  async function openDesktop(sessionId: string) {
    if (!desktop.board || desktop.error || inFlight.current) return;
    inFlight.current = true;
    setOpening(sessionId);
    setOpenError(null);
    const currentHash = window.location.hash;
    try {
      await workbenchApi.controlLive({
        kind: "select",
        connectionId: desktop.board.connectionId,
        sessionId,
      });
      if (window.location.hash === currentHash)
        window.location.hash = `/tasks/${encodeURIComponent(sessionId)}?tab=live`;
    } catch (reason) {
      setOpenError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      inFlight.current = false;
      setOpening(null);
    }
  }

  const items = [
    ...tasks.map((task) => ({
      ...task,
      phase: task.nativeBusy ? ("native" as const) : task.phase,
      agentName: `${agentLabels[task.agent]} · ${task.nativeHistory ? "原生会话" : "App 会话"}`,
      external: false,
    })),
    ...(desktop.board?.sessions ?? []).map((session) => ({
      ...session,
      phase: desktop.error ? ("unknown" as const) : session.phase,
      agentName: "OpenCode Desktop",
      external: true,
    })),
  ].sort((a, b) => b.updated - a.updated);

  return (
    <>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-label text-fg-muted">
        <Badge
          size="sm"
          variant="plain"
          status={
            desktop.error ? "warning" : desktop.board ? "success" : "neutral"
          }
        >
          {desktop.error
            ? "OpenCode Desktop 暂不可用"
            : desktop.board
              ? "OpenCode Desktop 已连接"
              : "正在查找 OpenCode Desktop"}
        </Badge>
        {desktop.error && <span>{desktop.error}</span>}
      </div>
      {openError && (
        <Alert status="warning">
          <AlertTitle>未能打开会话</AlertTitle>
          <AlertDescription>{openError}</AlertDescription>
        </Alert>
      )}
      <div
        aria-label="任务看板"
        className="grid items-start gap-4 sm:grid-cols-2 lg:grid-cols-4"
      >
        {lanes.map((lane) => {
          const cards = items.filter((item) => laneFor(item.phase) === lane.id);
          return (
            <section
              key={lane.id}
              aria-labelledby={`task-lane-${lane.id}`}
              className="min-w-0 rounded-xl bg-surface-raised p-3"
            >
              <header className="mb-3 flex items-center justify-between gap-2 px-1">
                <h2
                  id={`task-lane-${lane.id}`}
                  className="text-title font-medium"
                >
                  {lane.title}
                </h2>
                <span className="rounded-md bg-emphasis px-2 py-0.5 text-label tabular-nums text-fg-muted">
                  {cards.length}
                </span>
              </header>
              {cards.length ? (
                <ul className="space-y-2">
                  {cards.map((card) => (
                    <li key={card.id}>
                      <button
                        type="button"
                        className="group block w-full min-w-0 space-y-2 rounded-lg border border-border bg-surface p-3 text-left outline-none hover:bg-hover focus-visible:ring-2 focus-visible:ring-focus-ring disabled:opacity-60"
                        disabled={
                          card.external &&
                          (Boolean(desktop.error) || opening !== null)
                        }
                        onClick={() =>
                          card.external
                            ? void openDesktop(card.id)
                            : openTask(card.id)
                        }
                      >
                        <span className="flex flex-wrap items-center gap-x-2 gap-y-1 text-label text-fg-muted">
                          <span className="font-medium">{card.agentName}</span>
                          <TaskStatus phase={card.phase} />
                        </span>
                        <span className="line-clamp-2 break-words text-body font-medium">
                          {card.title === card.id ? "未命名会话" : card.title}
                        </span>
                        <span className="flex items-center justify-between gap-2 text-label text-fg-subtle">
                          <span className="truncate" title={card.directory}>
                            {projectName(card.directory)}
                          </span>
                          {opening === card.id ? (
                            "打开中…"
                          ) : (
                            <Chevron
                              className="size-4 shrink-0"
                              aria-hidden="true"
                            />
                          )}
                        </span>
                      </button>
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="px-1 py-3 text-label text-fg-muted">
                  {loading ? "正在读取任务…" : lane.empty}
                </p>
              )}
            </section>
          );
        })}
      </div>
    </>
  );
}
