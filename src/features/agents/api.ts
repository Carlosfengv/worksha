import { command } from "@/shared/lib/desktop-command";
import type {
  AgentUsage,
  AgentUsageQuery,
  MemorySnapshot,
} from "@/features/agents/types";

export const agentsApi = {
  memory(agentId: string, fileId?: string): Promise<MemorySnapshot> {
    return command("query_agent_memory", { agentId, fileId: fileId ?? null });
  },
  agentUsage(query: AgentUsageQuery): Promise<AgentUsage> {
    return command("query_agent_usage", { query });
  },
};

export type ConnectionTarget =
  | { kind: "provider"; id: string; model: string }
  | { kind: "service"; id: string };

type Selection = {
  provider: string;
  model: string;
  reasoningEffort?: string | null;
};
export type AgentConnectionRecovery = {
  reviewId: string;
  before: Selection;
  kind?: "account";
};
export type DshAccountCatalog = {
  signedIn: boolean;
  models: { id: string; name: string }[];
};
export type AgentConnectionStatus =
  | { state: "authorization-required" }
  | {
      state: "authorized";
      selected: Selection | null;
      configured: boolean;
      selectedTarget: boolean;
      currentTarget?: {
        kind: "provider" | "service" | "account";
        name: string;
        model: string;
      };
      recovery?: AgentConnectionRecovery;
    };

export type AgentConnectionReview = {
  reviewId: string;
  before: Selection | null;
  targetName: string;
  models: string[];
  scope: "new-sessions";
};

type StatusAction =
  | { kind: "status" | "authorize" | "revoke" }
  | { kind: "apply" | "check" | "restore"; reviewId: string };

function connectionCommand<T>(
  agentId: string,
  action:
    | StatusAction
    | { kind: "preview"; target: ConnectionTarget }
    | { kind: "account-models" }
    | { kind: "preview-account"; model: string },
) {
  return command<T>("perform_agent_model_connection", {
    input: { agentId, action },
  });
}

export const agentConnectionApi = {
  status(agentId: string, action: StatusAction = { kind: "status" }) {
    return connectionCommand<AgentConnectionStatus>(agentId, action);
  },
  preview(agentId: string, target: ConnectionTarget) {
    return connectionCommand<AgentConnectionReview>(agentId, {
      kind: "preview",
      target,
    });
  },
  accountModels(agentId: string) {
    return connectionCommand<DshAccountCatalog>(agentId, {
      kind: "account-models",
    });
  },
  previewAccount(agentId: string, model: string) {
    return connectionCommand<AgentConnectionReview>(agentId, {
      kind: "preview-account",
      model,
    });
  },
};

export type LocalBinding = {
  bindingId: string;
  name: string;
  serviceId: string;
  baseUrl: string;
  keyVersion: number;
  revoked: boolean;
};
export type LocalBindingReceipt = {
  bindingId: string;
  operationId: string;
  keyVersion: number;
  state: "pending" | "active" | "cancelled" | "revoked";
};
type LocalOperation = { storeId: string; operationId: string };
export type BudgetWindow = "session" | "daily" | "monthly";
export type BudgetAmounts = { tokens: number | null; usdMicros: number | null };
export type BudgetPolicy = {
  revision: number;
  timezone: string;
  fallbackServiceId: string | null;
  warnPercent: number;
  windows: {
    kind: BudgetWindow;
    primary: BudgetAmounts;
    fallback: BudgetAmounts;
  }[];
};
type Counter = { committed: number; reserved: number; uncertain: number };
export type BudgetAttempt = {
  attemptKey: string;
  observationId: string;
  bindingId: string;
  revision: number;
  createdAt: string;
  model: string;
  providerId: string;
  upstreamRequestId: string | null;
  tier: "primary" | "fallback";
  frozen: BudgetAmounts;
  windows: { kind: BudgetWindow; key: string; reserved: BudgetAmounts }[];
};
export type BudgetOverview = {
  storeId: string;
  policy: BudgetPolicy | null;
  attempts: BudgetAttempt[];
  windows: {
    kind: BudgetWindow;
    key: string;
    tier: "primary" | "fallback";
    limit: BudgetAmounts;
    ledger: { tokens: Counter; usdMicros: Counter };
    warned: boolean;
  }[];
};
type ReviewMetric =
  { kind: "settle"; amount: number } | { kind: "release" } | null;
export type BudgetReview = LocalOperation & {
  bindingId: string;
  attemptKey: string;
  expectedRevision: number;
  upstreamRequestId: string;
  source: string;
  tokens: ReviewMetric;
  usdMicros: ReviewMetric;
};
type ChatInput =
  | { kind: "list" | "budgetBindings" | "budgetWarnings" }
  | {
      kind: "create";
      query: LocalOperation;
      name: string;
      serviceId: string;
      serviceVersion: string;
    }
  | {
      kind: "rotate" | "revoke";
      query: LocalOperation;
      bindingId: string;
      expectedKeyVersion: number;
    }
  | { kind: "resume" | "cancel"; query: LocalOperation }
  | { kind: "readBudget"; bindingId: string }
  | {
      kind: "saveBudget";
      bindingId: string;
      storeId: string;
      policy: BudgetPolicy;
    }
  | { kind: "reviewBudget"; review: BudgetReview }
  | { kind: "requestBudget"; requestId: string };

function localConnection<T>(input: ChatInput) {
  return command<T>("local_chat_connection", { input });
}

export const localConnectionsApi = {
  warnings: () =>
    localConnection<{
      warnings: { id: string; bindingId: string; createdAt: string }[];
    }>({ kind: "budgetWarnings" }),
  list: () =>
    localConnection<{
      storeId: string;
      items: LocalBinding[];
      pending: LocalBindingReceipt[];
    }>({ kind: "list" }),
  write: (input: Exclude<ChatInput, { kind: "list" | "budgetBindings" }>) =>
    localConnection<{
      receipt?: LocalBindingReceipt;
      baseUrl?: string;
      cancelled?: boolean;
      saveError?: string;
    }>(input),
  budgets: () =>
    localConnection<{
      storeId: string;
      items: {
        bindingId: string;
        agentId: string;
        model: string;
        revoked: boolean;
      }[];
    }>({ kind: "budgetBindings" }),
  budget: (bindingId: string) =>
    localConnection<BudgetOverview>({ kind: "readBudget", bindingId }),
  saveBudget: (bindingId: string, storeId: string, policy: BudgetPolicy) =>
    localConnection<BudgetPolicy>({
      kind: "saveBudget",
      bindingId,
      storeId,
      policy,
    }),
  review: (review: BudgetReview) =>
    localConnection<{ attempt: BudgetAttempt }>({
      kind: "reviewBudget",
      review,
    }),
  attempt: (requestId: string) =>
    localConnection<{ storeId: string; attempt: BudgetAttempt | null }>({
      kind: "requestBudget",
      requestId,
    }),
};
