import { useEffect, useState } from "react";
import { workbenchApi } from "./api";
import type { Task, WorkspaceSnapshot } from "./types";

type State = {
  data: WorkspaceSnapshot | null;
  error: string | null;
};

export function useWorkbench(taskId: string | null) {
  const [state, setState] = useState<State>({ data: null, error: null });
  useEffect(() => {
    let mounted = true;
    let timer = 0;
    let revision: number | null = null;

    async function read() {
      let delay = 1500;
      try {
        const since = revision;
        const next = await workbenchApi.read(taskId, since);
        if (!mounted) return;
        if (next.task) revision = next.task.revision;
        if (next.activeTaskId) delay = 280;
        setState((previous) => {
          const current =
            previous.data?.task?.id === taskId ? previous.data.task : null;
          if (
            current?.id === taskId &&
            current.revision > (next.task?.revision ?? since ?? 0)
          )
            return previous;
          return {
            error: null,
            data: { ...next, task: next.task ?? current ?? null },
          };
        });
      } catch (error) {
        if (mounted)
          setState((previous) => ({ ...previous, error: String(error) }));
      }
      if (mounted) timer = window.setTimeout(() => void read(), delay);
    }

    void read();
    return () => {
      mounted = false;
      window.clearTimeout(timer);
    };
  }, [taskId]);

  function accept(task: Task) {
    if (task.id !== taskId) return;
    setState((previous) => {
      if (
        previous.data?.task?.id === task.id &&
        previous.data.task.revision > task.revision
      )
        return previous;
      const active = ["starting", "running", "approval", "stopping"].includes(
        task.phase,
      );
      return {
        error: null,
        data: {
          ...previous.data,
          tasks: previous.data?.tasks ?? [],
          activeTaskId: active
            ? task.id
            : previous.data?.activeTaskId === task.id
              ? null
              : (previous.data?.activeTaskId ?? null),
          task,
        },
      };
    });
  }
  return { ...state, accept };
}
