import type { AssociatedCosts } from "@/shared/types/cost";
import type {
  GatewayActivity,
  NativeActivity,
} from "@/features/observation/types";
import type { NativeSession } from "@/features/sessions/types";
import type { AgentId, AgentProductId } from "@/shared/types/agent";
import type { Usage } from "@/shared/types/usage";

export interface ProjectQuery {
  scope?: string;
  agent?: string;
  days?: number;
  query?: string;
  cursor?: string;
}

export interface HiddenProject {
  id: string;
  name: string;
  path: string;
  hiddenAt: string;
}

export interface ProjectTotals {
  associatedCosts?: AssociatedCosts;
  sessionCount: number;
  usage: Usage;
  usageScope: "model_responses";
  modelCalls: number;
  usageMissingCalls: number;
  usageUnknownSessions: number;
  dateUnknownSessions: number;
  diskBytes: number;
  diskAllocatedBytes: number | null;
  diskUnknownSessions: number;
  writeCount: number;
  distinctFiles: number;
}

export interface ProjectIdentity {
  projectKind: "workspace" | "temporary" | "verification";
  id: string;
  name: string;
  path: string;
  gitRoot: string | null;
  locationState: string;
}

export interface RoutingProjectQuery {
  query?: string;
  cursor?: string;
  selectedId?: string;
}

export type RoutingProject = Pick<
  ProjectIdentity,
  "id" | "name" | "path" | "projectKind"
>;

export interface RoutingProjectPage {
  items: RoutingProject[];
  nextCursor: string | null;
  selected: RoutingProject | null;
}

export interface ProjectSummary extends ProjectIdentity {
  totals: ProjectTotals;
  agents: { agentId: string; totals: ProjectTotals }[];
  lastActivity: string | null;
  listenerCount: number | null;
  knownListenerCount: number;
}

export interface ProjectListener {
  id: string;
  protocol: string;
  address: string;
  port: number;
  pid: number;
  processIdentity: string;
  startedAt: string;
  commandRedacted: string;
  cwd: string;
  attribution: string;
  evidence: string;
}

export interface ListenerSnapshot {
  asOf: string | null;
  state: string;
  items: ProjectListener[];
  unresolvedProcesses: number;
  error: string | null;
}

export interface ProjectPage {
  items: ProjectSummary[];
  total: number;
  nextCursor: string | null;
  summary: ProjectTotals;
  unassignedSessions: number;
  dateUnknownExcluded: number;
  missingCounts: number;
  asOf: string;
  coverage: string;
  appliedFilter: ProjectQuery;
  listeners: ListenerSnapshot;
}

export interface FileWrite {
  id: string;
  projectId: string;
  sessionId: string;
  agentId: AgentId;
  agentProductId: AgentProductId | null;
  turnId: string;
  callId: string;
  tool: string;
  occurredAt: string | null;
  sourceLine: number;
  evidence: string;
  files: {
    path: string;
    operation: string;
    added: number | null;
    removed: number | null;
  }[];
}

export interface GitFile {
  id: string;
  path: string;
  previousPath: string | null;
  indexStatus: string;
  worktreeStatus: string;
  attribution: string;
}

export interface GitSnapshot {
  asOf: string;
  state: string;
  branch: string | null;
  head: string | null;
  remote: string | null;
  gitDir: string | null;
  commonDir: string | null;
  repositoryId: string | null;
  worktreeId: string | null;
  files: GitFile[];
  commits: { id: string; occurredAt: string; subject: string }[];
  error: string | null;
}

export interface GitDiff {
  asOf: string;
  path: string;
  state: string;
  text: string | null;
  reason: string | null;
}

export interface ProjectDetail {
  project: ProjectSummary;
  sessions: NativeSession[];
  writes: FileWrite[];
  git: GitSnapshot;
  listeners: ListenerSnapshot;
  asOf: string;
  coverage: string;
  appliedFilter: ProjectQuery;
  activities: { native: NativeActivity[]; gateway: GatewayActivity[] };
}
import type { Snapshot } from "@/app/types";

export type ProjectsPageProps = {
  data: Snapshot | null;
  selectedId?: string;
  selectedTab?: string;
  anchor?: string;
  initialAgent?: string;
  initialScope?: string;
  refresh: () => Promise<void>;
};
