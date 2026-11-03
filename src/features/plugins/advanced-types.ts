import type { Policy } from "../../../contracts/plugins/extensions/types";
export type {
  Policy,
  Query as ExtensionQuery,
  Action as ExtensionAction,
  Request as ExtensionRequest,
} from "../../../contracts/plugins/extensions/types";
export interface ExtensionPlan {
  planId: string;
  pluginId: string;
  digest: string;
  path: string;
  revision: string;
  providerId: string;
  providerName: string;
  state: string;
  createdAt: string;
  edits: { pointer: string; value: unknown }[];
  backupAvailable: boolean;
  canDiscard?: boolean;
  credentialPlaceholder: string;
}
export interface PlanCapacity {
  used: number;
  limit: number;
  remaining: number;
}
export interface ExtensionHistory {
  plans: ExtensionPlan[];
  planCapacity?: PlanCapacity;
}
export interface ExtensionCall {
  identity: string;
  rpcId: string | number;
  mcpConnection: string;
  tool: string;
  status: string;
  durationMs: number | null;
  observedAt: string;
  agentSession: null;
  content: string;
  modelTokens: null;
}
export interface ExtensionStatus {
  apiVersion: string;
  policy: Policy;
  resourceLabel: string | null;
  resourceTarget: string | null;
  resourceAuthorized: boolean;
  protocolBindings: string[];
  plans: ExtensionPlan[];
  planCapacity?: PlanCapacity;
  calls: ExtensionCall[];
  connector: {
    host: string;
    transportConnected: boolean;
    observationEnabled: boolean;
    tools: number | null;
    notifications: number;
    calls: number;
    observationIncomplete?: boolean;
    unconfirmedCalls?: number;
    lastError: string | null;
    content: string;
    note: string;
  } | null;
}
