import type {
  AgentUsage,
  AgentUsageQuery,
  MemorySnapshot,
} from "@/features/agents/types";
import { command } from "@/shared/lib/desktop-command";
export const agentsApi = {
  memory(agentId: string, fileId?: string): Promise<MemorySnapshot> {
    return command("query_agent_memory", { agentId, fileId: fileId ?? null });
  },
  agentUsage(query: AgentUsageQuery): Promise<AgentUsage> {
    return command("query_agent_usage", { query });
  },
};
