import { command } from "@/shared/lib/desktop-command";
import type {
  Agent,
  LiveAction,
  LiveBoard,
  LiveConnection,
  ModelRoutes,
  OperationKind,
  OperationReceipt,
  OperationScope,
  Task,
  TurnAction,
  WorkspaceSnapshot,
} from "./types";

export const workbenchApi = {
  sharedRoutes: () => command<ModelRoutes>("get_workbench_shared_routes"),
  createShared: (
    taskId: string,
    projectId: string | null,
    route: string,
    text: string,
    scope?: OperationScope,
    connectionId?: string,
  ) =>
    command<{
      sessionId: string;
      connectionId: string;
      delivered: boolean;
      error: string | null;
    } | null>("create_workbench_shared", {
      taskId,
      projectId,
      route,
      text,
      scope,
      connectionId,
    }),
  readLive: () => command<LiveConnection | null>("read_workbench_live"),
  readLiveBoard: () => command<LiveBoard>("read_workbench_live_board"),
  controlLive: (action: LiveAction) =>
    command("control_workbench_live", { action }),
  findSession: (sessionId: string) =>
    command<{ taskId: string | null }>("find_workbench_session", { sessionId }),
  openLinkedSession: async (sessionId: string): Promise<string | null> => {
    if (
      /^claude:[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(
        sessionId,
      )
    ) {
      const result = await workbenchApi.findSession(sessionId);
      return result.taskId
        ? `/tasks/${encodeURIComponent(result.taskId)}`
        : null;
    }
    const native = /^opencode:(ses_[A-Za-z0-9_-]{1,192})$/.exec(sessionId)?.[1];
    if (!native) return null;
    const board = await workbenchApi.readLiveBoard();
    if (!board.sessions.some((session) => session.id === native)) return null;
    await workbenchApi.controlLive({
      kind: "select",
      connectionId: board.connectionId,
      sessionId: native,
    });
    return `/tasks/${encodeURIComponent(native)}?tab=live`;
  },
  routes: (agent: Agent) =>
    command<ModelRoutes>("get_workbench_routes", { agent }),
  read: (taskId: string | null, revision: number | null) =>
    command<WorkspaceSnapshot>("read_workbench_tasks", { taskId, revision }),
  operation: async (
    kind: OperationKind,
    operationId: string,
    storeId: string,
  ) => {
    const result = await command<{
      storeId: string;
      operationId: string;
      kind: OperationKind;
      receipt: OperationReceipt | null;
    }>("read_workbench_operation", { kind, operationId, storeId });
    if (
      result.storeId !== storeId ||
      result.operationId !== operationId ||
      result.kind !== kind
    ) {
      throw new Error("原操作的编号或后台不匹配");
    }
    return result.receipt;
  },
  create: (
    taskId: string,
    projectId: string | null,
    route: string,
    agent: Agent,
    parentTaskId: string | null,
    scope?: OperationScope,
  ) =>
    command<Task | null>("create_workbench_task", {
      taskId,
      projectId,
      route,
      agent,
      parentTaskId,
      scope,
    }),
  openDesktop: (agent: Agent, handoff: string) =>
    command<{ opened: boolean }>("open_workbench_desktop", { agent, handoff }),
  exportOpenCode: (taskId: string) =>
    command<boolean>("export_workbench_opencode", { taskId }),
  send: (
    taskId: string,
    turnId: string,
    text: string,
    scope?: OperationScope,
    expectedRevision?: number,
  ) =>
    command<Task>("send_workbench_message", {
      taskId,
      turnId,
      text,
      scope,
      expectedRevision,
    }),
  control: (action: TurnAction) =>
    command<Task>("control_workbench_turn", { action }),
};
