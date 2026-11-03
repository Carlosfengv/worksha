import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";

import type { ProviderUsageQuery } from "../types";

export type UsageFilters = Pick<
  ProviderUsageQuery,
  "source" | "agent" | "days"
>;
export function ProviderUsageFilters({
  value,
  onChange,
}: {
  value: UsageFilters;
  onChange: (value: UsageFilters) => void;
}) {
  const { agentProducts } = useAgentDirectory();
  return (
    <div className="flex flex-wrap items-center gap-3">
      <Select
        value={value.source}
        onValueChange={(source) =>
          onChange({ ...value, source: source as UsageFilters["source"] })
        }
      >
        <SelectTrigger aria-label="供应商调用来源" />
        <SelectContent>
          <SelectItem value="gateway">网关请求</SelectItem>
          <SelectItem value="native">原生模型响应</SelectItem>
        </SelectContent>
      </Select>
      <Select
        value={value.days?.toString() ?? "all"}
        onValueChange={(days) =>
          onChange({
            ...value,
            days: days === "all" ? undefined : Number(days),
          })
        }
      >
        <SelectTrigger aria-label="供应商调用日期" />
        <SelectContent>
          <SelectItem value="all">全部调用日期</SelectItem>
          <SelectItem value="1">最近 24 小时</SelectItem>
          <SelectItem value="7">最近 7 天</SelectItem>
          <SelectItem value="30">最近 30 天</SelectItem>
        </SelectContent>
      </Select>
      <Select
        value={value.agent ?? "all"}
        onValueChange={(agent) =>
          onChange({ ...value, agent: agent === "all" ? undefined : agent })
        }
      >
        <SelectTrigger aria-label="供应商调用 Agent" />
        <SelectContent>
          <SelectItem value="all">全部 Agents</SelectItem>
          {agentProducts.map((p) => (
            <SelectItem key={p.id} value={p.id}>
              {p.name}
            </SelectItem>
          ))}
          <SelectItem value="unknown">客户端未知</SelectItem>
        </SelectContent>
      </Select>
    </div>
  );
}
