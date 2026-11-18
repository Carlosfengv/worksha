import { useMemo, type ReactNode } from "react";
import { FilterQueryInput } from "@/components/ui/filter-query-input";
import {
  DateTimeRangePicker,
  parseISODateTime,
} from "@/components/ui/temporal-picker";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { TimeRangeHistogram } from "@/components/ui/time-range-histogram";
import type { FilterField } from "@/lib/filter-core";
import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { requestIssues, requestStates } from "../request-display";
import type { RequestDashboard } from "../request-types";

const sources = {
  unified: "全部来源",
  gateway: "网关",
  native: "原生",
  linked: "已关联",
};
const scopes = { local: "本机", lan: "局域网" };
const labels = {
  source: "来源",
  scope: "范围",
  client: "客户端 UUID",
  agent: "Agent",
  model: "模型",
  provider: "供应商",
  session: "会话",
  project: "项目",
  result: "结果",
  issue: "故障",
  search: "搜索",
};
const keys = Object.keys(labels) as (keyof typeof labels)[];
const series = [
  { dataKey: "requests", label: "可测记录", color: "var(--chart-1)" },
];

export function RequestToolbar({
  params,
  data,
  onChange,
  actions,
}: {
  params: URLSearchParams;
  data: RequestDashboard | null;
  onChange: (changes: Record<string, string | undefined>) => void;
  actions?: ReactNode;
}) {
  const { agentNames } = useAgentDirectory();
  const fields = useMemo<FilterField[]>(() => {
    const enums: Partial<Record<keyof typeof labels, Record<string, string>>> =
      {
        source: sources,
        scope: scopes,
        result: requestStates,
        issue: requestIssues,
        agent: { ...agentNames, unknown: "未知" },
      };
    return keys.map((id) => {
      const entries = enums[id];
      return entries
        ? {
            id,
            label: labels[id],
            type: "select",
            operators: [{ value: "is", label: "是" }],
            options: Object.entries(entries).map(([value, label]) => ({
              value,
              label,
            })),
          }
        : {
            id,
            label: labels[id],
            type: "text",
            operators: [
              {
                value: id === "search" ? "contains" : "equals",
                label: id === "search" ? "包含" : "等于",
              },
            ],
          };
    });
  }, [agentNames]);
  const filters = keys.flatMap((key) => {
    const value = params.get(key === "scope" ? "clientScope" : key);
    return value
      ? [
          {
            id: key,
            field: key,
            operator:
              key === "search"
                ? "contains"
                : fields.find((field) => field.id === key)?.type === "select"
                  ? "is"
                  : "equals",
            value,
          },
        ]
      : [];
  });
  const histogram =
    data?.buckets.flatMap((bucket) => {
      const start = Date.parse(bucket.start);
      const end = Date.parse(bucket.end);
      return Number.isFinite(start) &&
        Number.isFinite(end) &&
        end > start &&
        Number.isFinite(bucket.samples)
        ? [
            {
              start,
              end,
              requests: bucket.samples,
              label: new Date(start).toLocaleString("zh-CN"),
            },
          ]
        : [];
    }) ?? [];
  const from = parseISODateTime(params.get("start") ?? "");
  const to = parseISODateTime(params.get("end") ?? "");
  return (
    <div data-request-toolbar className="space-y-1.5">
      <div className="flex flex-wrap items-center gap-2">
        <div className="min-w-40 flex-[1_1_12rem]">
          <FilterQueryInput
            className="min-w-0"
            size="sm"
            fields={fields}
            queryFields={keys.map((fieldId) => ({
              fieldId,
              key: fieldId,
              suggest: fieldId !== "search",
            }))}
            filters={filters}
            freeText={{ fieldId: "search" }}
            hotkey={false}
            allowDuplicateFields={false}
            commitMode="submit"
            messages={{
              placeholder: "搜索请求或输入 client:、scope:、model:…",
              filters: "请求筛选",
              values: "可选值",
              history: "历史筛选",
              invalidQuery: "请检查筛选条件",
            }}
            onFiltersChange={(filters) => {
              const patch: Record<string, string | undefined> =
                Object.fromEntries(
                  keys.map((key) => [
                    key === "scope" ? "clientScope" : key,
                    undefined,
                  ]),
                );
              for (const filter of filters) {
                if (typeof filter.value === "string")
                  patch[
                    filter.field === "scope" ? "clientScope" : filter.field
                  ] = filter.value;
              }
              patch.scope = undefined;
              onChange(patch);
            }}
          />
        </div>
        <Select
          size="sm"
          value={params.get("range") ?? "today"}
          onValueChange={(value) => {
            if (value)
              onChange({ range: value, start: undefined, end: undefined });
          }}
        >
          <SelectTrigger aria-label="请求时间范围" contentSized />
          <SelectContent>
            {Object.entries({
              today: "今天",
              7: "最近 7 天",
              30: "最近 30 天",
              all: "全部时间",
              custom: "自定义时间",
            }).map(([value, label]) => (
              <SelectItem key={value} value={value}>
                {label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <DateTimeRangePicker
          aria-label="选择请求起止时间"
          placeholder="选择日期"
          locale="zh-CN"
          messages={{
            selectDateTime: "选择起止时间",
            start: "开始",
            end: "结束",
            clear: "清除",
            cancel: "取消",
            apply: "应用",
          }}
          size="sm"
          value={from && to ? { from, to } : undefined}
          onValueChange={(range) =>
            onChange({
              range: range ? "custom" : "today",
              start: range?.from,
              end: range?.to,
            })
          }
        />
        {actions}
      </div>
      {histogram.length > 1 && (
        <TimeRangeHistogram
          ariaLabel="请求时间分布"
          data={histogram}
          series={series}
          value={{ start: histogram[0].start, end: histogram.at(-1)!.end }}
          chartClassName="h-5"
          onValueChange={({ start, end }) =>
            onChange({
              range: "custom",
              start: new Date(start).toISOString(),
              end: new Date(end).toISOString(),
            })
          }
        />
      )}
    </div>
  );
}
