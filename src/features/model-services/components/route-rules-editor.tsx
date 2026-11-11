import { useMemo, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Button } from "@/components/ui/button";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { ProjectPicker } from "@/features/projects/project-picker";
import { modelServicesApi } from "../api";
import { routingAgents, type RouteRule } from "../types";

const agentNames: Record<(typeof routingAgents)[number], string> = {
  claude: "Claude Code",
  opencode: "OpenCode · 全部客户端",
  "opencode-cli": "OpenCode CLI",
  "opencode-desktop": "OpenCode Desktop",
  codex: "Codex",
};
type Model = { id: string; name: string };
type RuleRow = {
  rule: RouteRule;
  modelName: string;
  priority: number;
  selected: boolean;
  edit: () => void;
};
const columns: ColumnDef<RuleRow>[] = [
  { accessorKey: "priority", header: "顺序" },
  {
    id: "rule",
    header: "条件规则",
    cell: ({ row: { original } }) => (
      <Button
        variant="link"
        size="sm"
        aria-pressed={original.selected}
        onClick={original.edit}
      >
        {original.rule.name.trim() || "未命名规则"}
      </Button>
    ),
  },
  { accessorKey: "modelName", header: "目标模型" },
];

export function routeRuleReady(rule: RouteRule, models: Model[]) {
  return (
    !!rule.name.trim() &&
    !/\p{Cc}/u.test(rule.name) &&
    models.some((model) => model.id === rule.modelId) &&
    (rule.intent === undefined ||
      (!!rule.intent.trim() &&
        [...rule.intent.trim()].length <= 48 &&
        !/\p{Cc}/u.test(rule.intent))) &&
    (rule.minInputTokens === undefined ||
      (Number.isInteger(rule.minInputTokens) &&
        rule.minInputTokens >= 1 &&
        rule.minInputTokens <= 2000000)) &&
    [
      rule.agent,
      rule.projectId,
      rule.tools,
      rule.stream,
      rule.minInputTokens,
      rule.intent,
    ].some((value) => value !== undefined)
  );
}

export function RouteRulesEditor({
  rules,
  models,
  disabledModels,
  disabled,
  onChange,
}: {
  rules: RouteRule[];
  models: Model[];
  disabledModels: string[];
  disabled: boolean;
  onChange: (rules: RouteRule[]) => void;
}) {
  const [selectedId, select] = useState("");
  const index = rules.findIndex((rule) => rule.id === selectedId);
  const selected = rules[index];
  const rows = useMemo(
    () =>
      rules.map((rule, i) => ({
        rule,
        priority: i + 1,
        modelName:
          models.find((model) => model.id === rule.modelId)?.name ||
          "目标待选择",
        selected: i === index,
        edit: () => select(i === index ? "" : rule.id),
      })),
    [rules, models, index],
  );
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (row) => row.rule.id,
    manualPagination: true,
    enableSorting: false,
    enableHiding: false,
    enableRowSelection: false,
  });
  const move = (delta: number) => {
    const next = [...rules];
    [next[index], next[index + delta]] = [next[index + delta], next[index]];
    onChange(next);
  };
  return (
    <section className="space-y-3" aria-label="条件路由">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-body font-medium">条件路由</h3>
        <Button
          variant="secondary"
          size="sm"
          disabled={disabled || !models.length || rules.length >= 16}
          onClick={() => {
            const id = crypto.randomUUID();
            onChange([...rules, { id, name: "新规则", modelId: "" }]);
            select(id);
          }}
        >
          添加规则
        </Button>
      </div>
      <p className="text-label text-fg-muted">
        默认模型在可确认的新轮次从上到下检查。规则内的条件须同时满足，首条匹配且目标已启用、能力足够的规则优先。
      </p>
      {!!rules.length && <DataTable table={table} showPagination={false} />}
      {selected && (
        <div className="space-y-3 rounded-lg border p-3">
          <RuleFields
            key={selected.id}
            rule={selected}
            models={models}
            disabledModels={disabledModels}
            disabled={disabled}
            update={(patch) =>
              onChange(
                rules.map((rule) =>
                  rule.id === selected.id ? { ...rule, ...patch } : rule,
                ),
              )
            }
          />
          <div className="flex flex-wrap gap-2">
            <Button
              variant="tertiary"
              size="sm"
              disabled={disabled || index === 0}
              onClick={() => move(-1)}
            >
              上移规则
            </Button>
            <Button
              variant="tertiary"
              size="sm"
              disabled={disabled || index === rules.length - 1}
              onClick={() => move(1)}
            >
              下移规则
            </Button>
            <Button
              variant="tertiary"
              size="sm"
              disabled={disabled}
              onClick={() => {
                onChange(rules.filter((rule) => rule.id !== selected.id));
                select("");
              }}
            >
              移除规则
            </Button>
          </div>
        </div>
      )}
      {rules.some((rule) => !routeRuleReady(rule, models)) && (
        <p className="text-label text-fg-muted" role="status">
          每条规则须填写名称、目标模型及至少一个条件。
        </p>
      )}
    </section>
  );
}

function RuleFields({
  rule,
  models,
  disabledModels,
  disabled,
  update,
}: {
  rule: RouteRule;
  models: Model[];
  disabledModels: string[];
  disabled: boolean;
  update: (patch: Partial<RouteRule>) => void;
}) {
  const [projectOpen, showProject] = useState(!!rule.projectId);
  const agents = [
    { value: "any", label: "任意客户端" },
    ...routingAgents.map((agent) => ({
      value: agent,
      label: agentNames[agent],
    })),
  ];
  return (
    <>
      <div className="grid gap-3 md:grid-cols-2">
        <Field>
          <FieldLabel htmlFor="route-rule-name">规则名称</FieldLabel>
          <Input
            id="route-rule-name"
            autoFocus
            maxLength={80}
            value={rule.name}
            disabled={disabled}
            onChange={(e) => update({ name: e.target.value })}
          />
        </Field>
        <Field>
          <FieldLabel>规则目标模型</FieldLabel>
          <Select
            value={
              models.some((model) => model.id === rule.modelId)
                ? rule.modelId
                : ""
            }
            onValueChange={(value) => {
              if (value) update({ modelId: value });
            }}
            disabled={disabled}
          >
            <SelectTrigger
              aria-label="规则目标模型"
              placeholder="选择服务内模型"
            />
            <SelectContent>
              {models.map((model) => (
                <SelectItem key={model.id} value={model.id}>
                  {model.name || "未命名模型"}
                  {disabledModels.includes(model.id) ? " · 已停用" : ""}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>
        <Field>
          <FieldLabel>客户端条件</FieldLabel>
          <Select
            value={rule.agent ?? "any"}
            onValueChange={(value) =>
              update({ agent: routingAgents.find((agent) => agent === value) })
            }
            disabled={disabled}
          >
            <SelectTrigger aria-label="客户端条件" />
            <SelectContent>
              {agents.map(({ value, label }) => (
                <SelectItem key={value} value={value}>
                  {label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>
        <Field>
          <FieldLabel htmlFor="route-rule-tokens">
            最低输入 Token 数（按网关估算或计数）
          </FieldLabel>
          <Input
            id="route-rule-tokens"
            type="number"
            min={1}
            max={2000000}
            value={rule.minInputTokens ?? ""}
            placeholder="不限"
            disabled={disabled}
            onChange={(e) =>
              update({
                minInputTokens:
                  e.target.value === "" ? undefined : Number(e.target.value),
              })
            }
          />
        </Field>
        <Flag
          label="工具定义"
          yes="包含可用工具"
          no="不含可用工具"
          value={rule.tools}
          disabled={disabled}
          onChange={(tools) => update({ tools })}
        />
        <Flag
          label="响应方式"
          yes="流式"
          no="非流式"
          value={rule.stream}
          disabled={disabled}
          onChange={(stream) => update({ stream })}
        />
      </div>
      <Field>
        <FieldLabel htmlFor="route-rule-intent">请求意图</FieldLabel>
        <Input
          id="route-rule-intent"
          value={rule.intent ?? ""}
          placeholder="例如修复错误，留空表示不限"
          maxLength={48}
          disabled={disabled}
          onChange={(event) =>
            update({ intent: event.target.value || undefined })
          }
          onBlur={(event) =>
            update({ intent: event.target.value.trim() || undefined })
          }
        />
      </Field>
      <Field>
        <FieldLabel>项目条件</FieldLabel>
        {projectOpen ? (
          <ProjectPicker
            value={rule.projectId ?? null}
            loadRoutingProjects={modelServicesApi.routingProjects}
            onChange={(id) => {
              update({ projectId: id ?? undefined });
              if (!id) showProject(false);
            }}
            disabled={disabled}
            hint="只匹配已授权且能关联到该项目的本机会话。"
            clearLabel="清除项目条件"
          />
        ) : (
          <Button
            variant="tertiary"
            size="sm"
            disabled={disabled}
            onClick={() => showProject(true)}
          >
            选择项目
          </Button>
        )}
      </Field>
    </>
  );
}

function Flag({
  label,
  yes,
  no,
  value,
  disabled,
  onChange,
}: {
  label: string;
  yes: string;
  no: string;
  value: boolean | undefined;
  disabled: boolean;
  onChange: (value?: boolean) => void;
}) {
  const items = [
    { value: "any", label: "不限" },
    { value: "yes", label: yes },
    { value: "no", label: no },
  ];
  return (
    <Field>
      <FieldLabel>{label}</FieldLabel>
      <Select
        value={value === undefined ? "any" : value ? "yes" : "no"}
        onValueChange={(option) =>
          onChange(
            option === "yes" ? true : option === "no" ? false : undefined,
          )
        }
        disabled={disabled}
      >
        <SelectTrigger aria-label={label} />
        <SelectContent>
          {items.map(({ value, label }) => (
            <SelectItem key={value} value={value}>
              {label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </Field>
  );
}
