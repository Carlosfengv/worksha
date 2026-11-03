import type { McpQuery, McpSnapshot } from "@/features/mcp/types";
import { command } from "@/shared/lib/desktop-command";
export const mcpApi = {
  mcp(query: McpQuery): Promise<McpSnapshot> {
    return command("query_mcp", { query });
  },
};
