import type { McpUsage } from "@/features/mcp/types";
import { number } from "@/features/sessions/display";
import { compactNumber } from "@/shared/lib/format";
export function Tokens({ group }: { group: McpUsage }) {
  return (
    <div className="max-w-56 whitespace-normal wrap-anywhere">
      <p className="tabular-nums" title={number(group.relatedTokens)}>
        {compactNumber(group.relatedTokens)}
      </p>
      <p className="text-label text-fg-muted">
        {group.relatedResponses} 个关联响应
        {group.tokenMissingCalls > 0
          ? ` · ${group.tokenMissingCalls} 次调用用量未知`
          : ""}
      </p>
    </div>
  );
}
