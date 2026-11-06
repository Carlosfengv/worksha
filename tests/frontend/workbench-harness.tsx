import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { WorkbenchPage } from "@/features/workbench/page";
import { workbenchApi } from "@/features/workbench/api";
import type {
  Task,
  TurnAction,
  WorkspaceSnapshot,
} from "@/features/workbench/types";
import { IconProvider } from "@/lib/icon-context";
import { DesktopError } from "@/shared/lib/desktop-command";
import "@/styles/globals.css";

type Operation =
  { kind: "send"; taskId: string; turnId: string; text: string } | TurnAction;
const reads: {
  taskId: string | null;
  revision: number | null;
  resolve: (value: WorkspaceSnapshot) => void;
}[] = [];
const operations: {
  action: Operation;
  resolve: (task: Task) => void;
  reject: (error: Error) => void;
}[] = [];

function task(id: string, patch: Partial<Task> = {}): Task {
  return {
    id,
    agent: "claude",
    nativeHistory: true,
    nativeBusy: false,
    parentTaskId: null,
    title: `任务 ${id}`,
    directory: `/temporary/${id}`,
    route: "fixture-pinned-model",
    phase: "completed",
    updated: 0,
    revision: 1,
    sessionId: `native-${id}`,
    turnId: `turn-${id}`,
    parts: [
      {
        id: `history-${id}`,
        kind: "assistant",
        text: `历史回复 ${id}`,
        truncated: false,
      },
    ],
    approval: null,
    error: null,
    ...patch,
  };
}

const harness = {
  reads,
  operations,
  completeRead(
    index: number,
    patch: Partial<Task> | null,
    activeTaskId: string | null = null,
  ) {
    const read = reads[index];
    read.resolve({
      tasks: [],
      task: patch === null ? null : task(read.taskId!, patch),
      activeTaskId,
      claudeConnection: {
        requestScope: "current",
        desktopRequestScope: "unknown",
        desktopError: null,
      },
    });
  },
  completeOperation(index: number, patch: Partial<Task>) {
    const operation = operations[index];
    operation.resolve(task(operation.action.taskId, patch));
  },
  failOperation(index: number, uncertain: boolean) {
    operations[index].reject(
      new DesktopError({
        message: "发送结果待核对",
        outcomeUnknown: uncertain,
      }),
    );
  },
};
declare global {
  interface Window {
    workbenchHarness: typeof harness;
  }
}
window.workbenchHarness = harness;
workbenchApi.read = (taskId, revision) =>
  new Promise((resolve) => reads.push({ taskId, revision, resolve }));
workbenchApi.send = (taskId, turnId, text) =>
  new Promise((resolve, reject) =>
    operations.push({
      action: { kind: "send", taskId, turnId, text },
      resolve,
      reject,
    }),
  );
workbenchApi.control = (action) =>
  new Promise((resolve, reject) =>
    operations.push({ action, resolve, reject }),
  );

function Harness() {
  const [selected, setSelected] = useState(location.hash.slice(1) || "a");
  useEffect(() => {
    const changed = () => setSelected(location.hash.slice(1));
    window.addEventListener("hashchange", changed);
    return () => window.removeEventListener("hashchange", changed);
  }, []);
  return (
    <WorkbenchPage selectedId={selected} newTask={false} liveMode={false} />
  );
}
createRoot(document.getElementById("root")!).render(
  <IconProvider>
    <Harness />
  </IconProvider>,
);
