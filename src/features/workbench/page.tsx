import { useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { PageBody } from "@/components/ui/page-layout";
import { useIcon } from "@/lib/icon-context";
import { LiveDesktop } from "./live-desktop";
import { TaskBoard } from "./task-board";
import { TaskConversation } from "./task-conversation";
import { TaskSetup } from "./task-setup";
import type { Task, TaskDraft } from "./types";
import { useWorkbench } from "./use-workbench";

export function WorkbenchPage({
  selectedId,
  projectId,
  newTask,
  liveMode,
}: {
  selectedId?: string;
  projectId?: string;
  newTask: boolean;
  liveMode: boolean;
}) {
  const [prepared, setPrepared] = useState<{
    id: string;
    draft: TaskDraft;
  } | null>(null);
  if (liveMode)
    return (
      <LiveDesktop key={selectedId ?? "connections"} sessionId={selectedId} />
    );
  return (
    <Workbench
      key={selectedId ?? projectId ?? (newTask ? "new" : "board")}
      selected={selectedId ?? null}
      newTask={newTask}
      projectId={projectId}
      prepared={prepared}
      setPrepared={setPrepared}
    />
  );
}

function Workbench({
  selected,
  newTask,
  projectId,
  prepared,
  setPrepared,
}: {
  selected: string | null;
  newTask: boolean;
  projectId?: string;
  prepared: { id: string; draft: TaskDraft } | null;
  setPrepared: (prepared: { id: string; draft: TaskDraft } | null) => void;
}) {
  const { data, error, accept } = useWorkbench(selected);
  const [handoff, setHandoff] = useState<Task | null>(null);
  const Plus = useIcon("plus");
  const Back = useIcon("arrow-left");
  const board = !selected && !newTask && !projectId;
  function open(id: string, draft?: TaskDraft) {
    setHandoff(null);
    if (draft) setPrepared({ id, draft });
    window.location.hash = `/tasks/${encodeURIComponent(id)}`;
  }
  return (
    <PageBody className="flex flex-col gap-4 p-4 sm:p-6">
      <header className="flex flex-wrap items-center justify-between gap-3">
        {board ? (
          <>
            <div>
              <h1 className="text-heading font-medium">任务看板</h1>
              <p className="mt-1 text-label text-fg-muted">
                点开一张卡片查看进度或继续对话。
              </p>
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <Button
                variant="tertiary"
                onClick={() => {
                  window.location.hash = "/tasks?tab=live";
                }}
              >
                连接设置
              </Button>
              <Button
                leadingIcon={Plus}
                onClick={() => {
                  window.location.hash = "/tasks?tab=new";
                }}
              >
                新建任务
              </Button>
            </div>
          </>
        ) : (
          <Button
            leadingIcon={Back}
            variant="ghost"
            onClick={() => {
              window.location.hash = "/tasks";
            }}
          >
            返回看板
          </Button>
        )}
      </header>
      {error && (
        <Alert status="danger">
          <AlertTitle>任务状态暂不可用</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      {board ? (
        <TaskBoard
          tasks={data?.tasks ?? []}
          loading={!data && !error}
          openTask={open}
        />
      ) : handoff || newTask || projectId ? (
        <div className="mx-auto w-full max-w-2xl">
          <TaskSetup
            source={handoff ?? undefined}
            projectId={projectId}
            onCreated={open}
            onCancel={handoff ? () => setHandoff(null) : undefined}
          />
        </div>
      ) : data?.task ? (
        <TaskConversation
          task={data.task}
          claudeConnection={data.claudeConnection}
          activeTaskId={data.activeTaskId}
          unavailable={Boolean(error)}
          accept={(task) => {
            accept(task);
            if (
              prepared?.id === task.id &&
              task.parts.some((part) => part.kind === "user")
            )
              setPrepared(null);
          }}
          initialDraft={
            prepared?.id === data.task.id ? prepared.draft : undefined
          }
          onHandoff={() => data.task && setHandoff(data.task)}
          related={data.tasks.filter(
            (item) =>
              item.id === data.task?.parentTaskId ||
              item.parentTaskId === data.task?.id,
          )}
          onOpen={open}
        />
      ) : (
        !error && (
          <p role="status" className="text-label text-fg-muted">
            正在打开会话…
          </p>
        )
      )}
    </PageBody>
  );
}
