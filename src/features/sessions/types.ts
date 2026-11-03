import type { AssociatedCosts } from "@/shared/types/cost";
import type { AgentId } from "@/shared/types/agent";
import type { Usage } from "@/shared/types/usage";

export interface SessionSource {
  clientAttribution?: {
    status: string;
    scannedLogFiles: number;
    confirmedToolExecutions: number;
    matchedCalls: number;
    conflictingCalls: number;
    ambiguousExecutions: number;
  } | null;
  agentId: AgentId;
  enabled: boolean;
  locations: string[];
  status: "disabled" | "pending" | "scanning" | "ready" | "partial";
  sessionCount: number;
  scannedFiles: number;
  totalFiles: number;
  skippedFiles: number;
  lastScan: string | null;
  error: string | null;
  versions: string[];
  capabilities: { name: string; status: string; reason: string }[];
}

export interface NativeSession {
  projectHidden?: boolean;
  agentProductId: string | null;
  agentProducts: string[];
  hasUnknownClient: boolean;
  id: string;
  nativeId: string;
  agentId: AgentId;
  agentVersion: string | null;
  title: string;
  titleSource: "native_title" | "first_user_message" | "fallback";
  projectPath: string | null;
  projectName: string | null;
  projectId: string | null;
  projectRoot: string | null;
  gitRemote: string | null;
  createdAt: string | null;
  updatedAt: string | null;
  status: "imported";
  messageCount: number;
  turnCount: number | null;
  modelCalls: number;
  modelErrorCount: number;
  latestFailureId: string | null;
  toolCalls: number;
  mcpCalls: number;
  skillCalls: number;
  usage: Usage;
  usageQuality: "native_cumulative" | "native_requests" | "partial" | "unknown";
  diskBytes: number;
  diskAllocatedBytes: number | null;
  diskScope: "native_transcript_file" | "native_database_payload";
  sourcePath: string;
  partial: boolean;
}

export interface SessionTotals {
  usageScope?: "matched_calls" | "session_cumulative";
  modelCalls: number;
  toolCalls: number;
  mcpCalls: number;
  skillCalls: number;
  usage: Usage;
  usageUnknownSessions: number;
}

export interface NativePage<T> {
  associatedCosts?: AssociatedCosts;
  items: T[];
  total: number;
  nextCursor: string | null;
  summary: SessionTotals | null;
  asOf: string;
  coverage: "native_history" | "partial" | "not_connected";
  missingCounts: number;
}

export interface SessionQuery {
  agent?: string;
  query?: string;
  days?: number;
  status?: string;
  cursor?: string;
}

export type CallKind = "model" | "tool" | "mcp" | "skill" | "diagnostic";

export interface ModelFailure {
  category: string;
  code: string | null;
  httpStatus: number | null;
  retryable: boolean | null;
  message: string | null;
  source: string;
  occurrences: number;
  lastObservedAt: string | null;
}

export interface NativeCall {
  agentAttribution?: string | null;
  agentProductId: string | null;
  id: string;
  sessionId: string;
  sessionTitle: string;
  agentId: AgentId;
  turnId: string;
  kind: CallKind;
  name: string;
  status: string;
  timestamp: string | null;
  model: string | null;
  provider: string | null;
  modelResponseId: string | null;
  usage: Usage | null;
  server: string | null;
  toolName: string | null;
  durationMs: number | null;
  toolSource: ConversationEvent["toolSource"];
  failure: ModelFailure | null;
}

export interface ConversationEvent {
  agentAttribution?: string | null;
  agentProductId: string | null;
  id: string;
  nativeId: string;
  turnId: string;
  kind: CallKind | "message" | "event";
  role: "user" | "assistant" | null;
  name: string;
  timestamp: string | null;
  endTime: string | null;
  status: string;
  text: string | null;
  input: string | null;
  output: string | null;
  model: string | null;
  modelResponseId: string | null;
  provider: string | null;
  usage: Usage | null;
  rawUsage: string | null;
  callId: string | null;
  server: string | null;
  toolSource: {
    status: "native" | "configuration" | "unknown" | "builtin";
    toolName: string | null;
    toolNameQuality: "native" | "normalized" | "unknown";
    transport: "stdio" | "remote" | null;
    configuredEnabled: boolean | null;
    sources: {
      path: string;
      scope: "local" | "project";
      modifiedAt: string | null;
    }[];
    reason: string;
  } | null;
  durationMs: number | null;
  truncated: boolean;
  sourceLine: number;
  failure: ModelFailure | null;
}

export interface Conversation {
  associatedCosts?: AssociatedCosts;
  context?: SessionContext;
  session: NativeSession;
  items: ConversationEvent[];
  total: number;
  nextCursor: string | null;
  asOf: string;
  coverage: "native_history" | "partial";
  warnings: string[];
}
export interface ContextRecord {
  id: string;
  kind: string;
  label: string;
  timestamp: string | null;
  sourceLine: number;
  agentProductId: string | null;
  text: string | null;
  truncated: boolean;
}
export interface SessionContext {
  sampleAgentProductId?: string | null;
  inputTokens: number | null;
  windowTokens: number | null;
  usedPercentage: number | null;
  sampleAt: string | null;
  model: string | null;
  source: string | null;
  compactions: ContextRecord[];
  loaded: ContextRecord[];
  compactionCount: number;
  loadedCount: number;
  limited: boolean;
}
