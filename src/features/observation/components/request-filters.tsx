import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Accordion,
  AccordionItem,
  AccordionTrigger,
  AccordionContent,
} from "@/components/ui/accordion";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import {
  Combobox,
  ComboboxInput,
  ComboboxContent,
  ComboboxList,
  ComboboxItem,
  ComboboxEmpty,
} from "@/components/ui/combobox";

import { requestStates, requestIssues } from "../request-display";
import type { RequestDashboard } from "../request-types";

export function RequestFilters({
  params,
  data,
  onChange,
}: {
  params: URLSearchParams;
  data: RequestDashboard | null;
  onChange: (patch: Record<string, string | undefined>) => void;
}) {
  const { agentProducts } = useAgentDirectory();
  const [search, setSearch] = useState(params.get("search") ?? "");
  const localTime = (key: string) => {
    const value = params.get(key);
    const millis = value ? Date.parse(value) : NaN;
    return Number.isFinite(millis)
      ? new Date(millis + 8 * 3_600_000).toISOString().slice(0, 16)
      : "";
  };
  const [start, setStart] = useState(() => localTime("start"));
  const [end, setEnd] = useState(() => localTime("end"));
  const [dateError, setDateError] = useState("");
  const providerOptions = ["", ...(data?.providers.map((p) => p.id) ?? [])];
  const modelOptions = ["", ...(data?.models ?? [])];
  const providerLabels = new Map(data?.providers.map((p) => [p.id, p.label]));
  const advancedCount = ["issue", "provider", "model"].filter((key) =>
    params.has(key),
  ).length;
  return (
    <section className="space-y-3" aria-label="请求筛选">
      <div className="flex flex-wrap gap-2">
        <Select
          value={params.get("range") ?? "today"}
          onValueChange={(v) => onChange({ range: v ?? "today" })}
        >
          <SelectTrigger aria-label="请求时间范围" />
          <SelectContent>
            <SelectItem value="today">今天</SelectItem>
            <SelectItem value="7">7 天</SelectItem>
            <SelectItem value="30">30 天</SelectItem>
            <SelectItem value="all">全部</SelectItem>
            <SelectItem value="custom">自定义</SelectItem>
          </SelectContent>
        </Select>
        <Select
          value={params.get("agent") ?? "all"}
          onValueChange={(v) =>
            onChange({ agent: v === "all" ? undefined : (v ?? undefined) })
          }
        >
          <SelectTrigger aria-label="请求 Agent" />
          <SelectContent>
            <SelectItem value="all">全部 Agents</SelectItem>
            {agentProducts.map((p) => (
              <SelectItem key={p.id} value={p.id}>
                {p.name}
              </SelectItem>
            ))}
            <SelectItem value="unknown">客户端来源未知</SelectItem>
          </SelectContent>
        </Select>
        <Select
          value={params.get("result") ?? "all"}
          onValueChange={(v) =>
            onChange({ result: v === "all" ? undefined : (v ?? undefined) })
          }
        >
          <SelectTrigger aria-label="请求结果" />
          <SelectContent>
            <SelectItem value="all">全部结果</SelectItem>
            {Object.entries(requestStates).map(([id, label]) => (
              <SelectItem key={id} value={id}>
                {label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <form
          className="flex w-full flex-wrap items-center gap-2 xl:w-auto xl:flex-1"
          onSubmit={(e) => {
            e.preventDefault();
            onChange({ search: search.trim() || undefined });
          }}
        >
          <div className="min-w-0 flex-1">
            <Input
              aria-label="搜索请求"
              placeholder="搜索请求 ID、模型、Provider、会话"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
          </div>
          <Button type="submit" variant="tertiary">
            搜索
          </Button>
          {params.get("search") && (
            <Button
              variant="ghost"
              onClick={() => {
                setSearch("");
                onChange({ search: undefined });
              }}
            >
              清除搜索
            </Button>
          )}
          {(params.get("session") || params.get("project")) && (
            <Button
              variant="ghost"
              onClick={() =>
                onChange({ session: undefined, project: undefined })
              }
            >
              查看全部请求（移除关联范围）
            </Button>
          )}
        </form>
      </div>
      <Accordion
        className="w-full"
        type="multiple"
        defaultValue={advancedCount ? ["filters"] : []}
      >
        <AccordionItem value="filters">
          <AccordionTrigger>
            更多筛选{advancedCount ? ` · 已选 ${advancedCount} 项` : ""}
          </AccordionTrigger>
          <AccordionContent>
            <div className="flex flex-wrap gap-2">
              <Select
                value={params.get("issue") ?? "all"}
                onValueChange={(v) =>
                  onChange({
                    issue: v === "all" ? undefined : (v ?? undefined),
                  })
                }
              >
                <SelectTrigger aria-label="请求异常类型" />
                <SelectContent>
                  <SelectItem value="all">全部异常类型</SelectItem>
                  {Object.entries(requestIssues).map(([id, label]) => (
                    <SelectItem key={id} value={id}>
                      {label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <div className="w-52">
                <Combobox<string>
                  items={providerOptions}
                  value={params.get("provider") ?? ""}
                  limit={200}
                  itemToStringLabel={(id) =>
                    id ? (providerLabels.get(id) ?? id) : "全部 Providers"
                  }
                  onValueChange={(id) =>
                    onChange({ provider: id || undefined })
                  }
                >
                  <ComboboxInput
                    aria-label="请求 Provider"
                    placeholder="全部 Providers"
                    triggerAriaLabel="搜索 Provider"
                  />
                  <ComboboxContent>
                    <ComboboxEmpty>无匹配 Provider</ComboboxEmpty>
                    <ComboboxList>
                      {(id: string) => (
                        <ComboboxItem key={id} value={id}>
                          {id
                            ? (providerLabels.get(id) ?? id)
                            : "全部 Providers"}
                        </ComboboxItem>
                      )}
                    </ComboboxList>
                  </ComboboxContent>
                </Combobox>
              </div>
              <div className="w-52">
                <Combobox<string>
                  items={modelOptions}
                  value={params.get("model") ?? ""}
                  limit={200}
                  itemToStringLabel={(id) => id || "全部模型"}
                  onValueChange={(id) => onChange({ model: id || undefined })}
                >
                  <ComboboxInput
                    aria-label="请求模型"
                    placeholder="全部模型"
                    triggerAriaLabel="搜索模型"
                  />
                  <ComboboxContent>
                    <ComboboxEmpty>无匹配模型</ComboboxEmpty>
                    <ComboboxList>
                      {(id: string) => (
                        <ComboboxItem key={id} value={id}>
                          {id || "全部模型"}
                        </ComboboxItem>
                      )}
                    </ComboboxList>
                  </ComboboxContent>
                </Combobox>
              </div>
            </div>
          </AccordionContent>
        </AccordionItem>
      </Accordion>
      {params.get("range") === "custom" && (
        <form
          className="flex flex-wrap items-end gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            const a = new Date(`${start}+08:00`),
              b = new Date(`${end}+08:00`);
            if (
              !start ||
              !end ||
              !Number.isFinite(a.getTime()) ||
              !Number.isFinite(b.getTime()) ||
              a >= b
            ) {
              setDateError("请选择有效起止时间，结束须晚于开始");
              return;
            }
            setDateError("");
            onChange({ start: a.toISOString(), end: b.toISOString() });
          }}
        >
          <label className="space-y-1 text-label">
            开始时间（上海）
            <Input
              type="datetime-local"
              aria-label="开始时间"
              value={start}
              onChange={(e) => setStart(e.target.value)}
              required
            />
          </label>
          <label className="space-y-1 text-label">
            结束时间（上海）
            <Input
              type="datetime-local"
              aria-label="结束时间"
              value={end}
              onChange={(e) => setEnd(e.target.value)}
              required
            />
          </label>
          <Button type="submit" variant="tertiary">
            应用时间
          </Button>
          {dateError && <p role="alert">{dateError}</p>}
        </form>
      )}

      {(params.get("session") || params.get("project")) && (
        <p className="text-label text-fg-muted">
          关联范围：
          {params.get("session")
            ? `Session ${params.get("session")}`
            : `项目 ${params.get("project")}`}
        </p>
      )}
    </section>
  );
}
