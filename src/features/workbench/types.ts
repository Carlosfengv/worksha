export type Agent = "opencode" | "claude";
export type RequestScope = "current" | "other" | "external" | "unknown";

export type TaskDraft = {
  text: string;
  error?: string;
  uncertainTurn?: string;
};

export const agentLabels: Record<Agent, string> = {
  opencode: "OpenCode",
  claude: "Claude Code CLI",
};

export const desktopLabels: Record<Agent, string> = {
  opencode: "OpenCode Desktop",
  claude: "Claude Desktop",
};

export type Phase =
  | "ready"
  | "starting"
  | "running"
  | "approval"
  | "stopping"
  | "completed"
  | "failed"
  | "stopped"
  | "unknown";

export interface TaskSummary {
  id: string;
  agent: Agent;
  nativeHistory: boolean;
  nativeBusy: boolean;
  parentTaskId: string | null;
  title: string;
  directory: string;
  route: string;
  phase: Phase;
  updated: number;
}

export interface Task extends TaskSummary {
  revision: number;
  sessionId: string | null;
  turnId: string | null;
  parts: {
    id: string;
    kind: "user" | "assistant" | "tool";
    text: string;
    truncated: boolean;
  }[];
  approval: { id: string; tool: "bash" | "edit"; targets: string[] } | null;
  error: string | null;
  cliResumeCommand?: string;
}

export interface ClaudeConnection {
  requestScope: RequestScope;
  desktopRequestScope: RequestScope;
  desktopError: string | null;
}

export interface WorkspaceSnapshot {
  tasks: TaskSummary[];
  task: Task | null;
  activeTaskId: string | null;
  claudeConnection?: ClaudeConnection;
}

export interface ModelRoutes {
  installed: boolean;
  routes:
    | {
        id: string;
        label: string;
        isDefault: boolean;
        requestScope?: RequestScope | null;
      }[]
    | null;
  error: string | null;
}

export type TurnAction = {
  taskId: string;
  turnId: string;
} & ({ kind: "stop" } | { kind: "decide"; approvalId: string; allow: boolean });

export interface LiveSession {
  id: string;
  title: string;
  directory: string;
}

export interface LiveBoard {
  connectionId: string;
  sessions: (LiveSession & {
    updated: number;
    phase: "running" | "approval" | "ended" | "unknown";
  })[];
}

export interface LiveConnection {
  id: string;
  version: string;
  sessions: LiveSession[];
  selected: {
    session: LiveSession;
    phase: "busy" | "retry" | "idle" | "unknown";
    live: boolean;
    parts: { id: string; role: "user" | "assistant" | "tool"; text: string }[];
    approvals: NonNullable<Task["approval"]>[];
    otherApproval: boolean;
    model: string | null;
    requestScope?: RequestScope;
    problem: "route" | "execution" | null;
  } | null;
}

export type LiveAction =
  | { kind: "discover" }
  | {
      kind: "connect";
      port: number;
      user: string;
      password: string;
      directory: string | null;
    }
  | { kind: "disconnect"; connectionId: string }
  | ({ connectionId: string; sessionId: string } & (
      | { kind: "select" }
      | { kind: "stop" }
      | { kind: "send"; text: string; messageId?: string }
      | { kind: "decide"; approvalId: string; allow: boolean }
    ));
