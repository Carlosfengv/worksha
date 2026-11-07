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
export type AgentConnectionRecovery = { reviewId: string; before: Selection };
export type AgentConnectionStatus =
  | { state: "authorization-required" }
  | {
      state: "authorized";
      selected: Selection | null;
      configured: boolean;
      selectedTarget: boolean;
      currentTarget?: {
        kind: "provider" | "service";
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
  action: StatusAction | { kind: "preview"; target: ConnectionTarget },
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
};
