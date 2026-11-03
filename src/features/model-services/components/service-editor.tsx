import { modelServicesApi } from "../api";
import { useMemo, useRef, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Button } from "@/components/ui/button";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import {
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
} from "@/components/ui/combobox";
import { Switch } from "@/components/ui/switch";
import { availableProviderModelIds } from "@/features/providers/catalog";
import type { Provider } from "@/features/providers/types";
import { useServiceWrite } from "../hooks/use-service-write";
import { ServiceWriteFeedback } from "./service-write-feedback";
import {
  defaultCapabilities,
  defaultRoutingPolicy,
  serviceDraft,
  type ModelService,
  type ModelServiceDraft,
} from "../types";
import { RoutingPolicySettings } from "./routing-policy-settings";
import { verifyServiceModels } from "../verification";

type Member = ModelServiceDraft["models"][number];
const orderedIds = (d: ModelServiceDraft): string[] =>
  [
    ...new Set([
      d.defaultModelId,
      ...(d.routingPolicy?.order ?? []),
      ...d.models.map((m) => m.id),
    ]),
  ].filter((id) => d.models.some((m) => m.id === id));
const normalize = (d: ModelServiceDraft): ModelServiceDraft => ({
  ...d,
  routingPolicy: {
    ...(d.routingPolicy ?? defaultRoutingPolicy()),
    order: orderedIds(d),
    disabledModels: (d.routingPolicy?.disabledModels ?? []).filter((id) =>
      d.models.some((m) => m.id === id),
    ),
    limits: (d.routingPolicy?.limits ?? []).filter((l) =>
      d.models.some((m) => m.id === l.modelId),
    ),
  },
});
type MemberRow = Member & {
  priority: number;
  routeEnabled: boolean;
  canUp: boolean;
  canDown: boolean;
  move: (delta: number) => void;
  isDefault: boolean;
  disabled: boolean;
  edit: () => void;
  remove: () => void;
};
const memberColumns: ColumnDef<MemberRow>[] = [
  {
    id: "priority",
    header: "优先级",
    cell: ({ row }) => (
      <div className="flex items-center gap-1 text-label">
        <span className="tabular-nums">
          {row.original.priority}
          {row.original.routeEnabled ? "" : " · 停用"}
        </span>
        <Button
          variant="tertiary"
          size="sm"
          disabled={row.original.disabled || !row.original.canUp}
          onClick={() => row.original.move(-1)}
          aria-label={`提高 ${row.original.name || "未命名模型"} 优先级`}
        >
          上移
        </Button>
        <Button
          variant="tertiary"
          size="sm"
          disabled={row.original.disabled || !row.original.canDown}
          onClick={() => row.original.move(1)}
          aria-label={`降低 ${row.original.name || "未命名模型"} 优先级`}
        >
          下移
        </Button>
      </div>
    ),
  },
  {
    id: "name",
    header: "模型",
    cell: ({ row }) => (
      <Button
        variant="link"
        size="sm"
        disabled={row.original.disabled}
        onClick={row.original.edit}
      >
        {row.original.name || "未命名模型"}
        {row.original.isDefault ? " · 默认" : ""}
      </Button>
    ),
  },
  {
    id: "source",
    header: "来源模型",
    cell: ({ row }) => (
      <span className="block max-w-48 break-words text-label">
        {row.original.model || "待选择"}
      </span>
    ),
  },
  {
    id: "context",
    header: "上下文 / 输出",
    cell: ({ row }) => (
      <span className="text-label tabular-nums">
        {row.original.capabilities.contextWindow.toLocaleString()} /{" "}
        {row.original.capabilities.maxOutputTokens.toLocaleString()}
      </span>
    ),
  },
  {
    id: "actions",
    header: "操作",
    cell: ({ row }) => (
      <Button
        variant="tertiary"
        size="sm"
        disabled={row.original.disabled}
        onClick={row.original.remove}
      >
        移除<span className="sr-only"> {row.original.name}</span>
      </Button>
    ),
  },
];

export function ServiceEditor({
  service,
  providers,
  originStoreId,
  onClose,
}: {
  service: ModelService | null;
  providers: Provider[];
  originStoreId: string | undefined;
  onClose: () => void;
}) {
  const origin = useRef(originStoreId).current;
  const [draft, setDraft] = useState<ModelServiceDraft>(() =>
    service
      ? serviceDraft(service)
      : {
          id: crypto.randomUUID(),
          revision: 0,
          name: "",
          defaultModelId: "",
          models: [],
          routingPolicy: defaultRoutingPolicy(),
        },
  );
  const [selected, setSelected] = useState(draft.models[0]?.id ?? "");
  const operation = useServiceWrite(onClose);
  const { busy, progress } = operation;
  const locked = busy || operation.uncertain;
  const member = draft.models.find((m) => m.id === selected);
  const source = providers.find((p) => p.id === member?.providerId);
  const sources = providers.filter((p) => p.configured);
  const models = source ? availableProviderModelIds(source) : [];
  const update = (change: Partial<Member>) =>
    setDraft((d) => ({
      ...d,
      models: d.models.map((m) =>
        m.id === selected ? { ...m, ...change } : m,
      ),
    }));
  const rows = useMemo(
    () =>
      orderedIds(draft).map((id, index) => {
        const m = draft.models.find((m) => m.id === id)!;
        return {
          ...m,
          isDefault: m.id === draft.defaultModelId,
          priority: index + 1,
          routeEnabled: !draft.routingPolicy?.disabledModels.includes(m.id),
          canUp: index > 0,
          canDown: index < draft.models.length - 1,
          move: (delta: number) =>
            setDraft((d) => {
              const order = orderedIds(d);
              const other = index + delta;
              [order[index], order[other]] = [order[other], order[index]];
              return {
                ...d,
                defaultModelId: order[0],
                routingPolicy: {
                  ...(d.routingPolicy ?? defaultRoutingPolicy()),
                  order,
                },
              };
            }),
          disabled: locked,
          edit: () => setSelected(m.id),
          remove: () =>
            setDraft((d) =>
              normalize({
                ...d,
                defaultModelId:
                  d.defaultModelId === m.id
                    ? (d.models.find((other) => other.id !== m.id)?.id ?? "")
                    : d.defaultModelId,
                models: d.models.filter((other) => other.id !== m.id),
              }),
            ),
        };
      }),
    [draft, locked],
  );
  const { table } = useDataTable({
    data: rows,
    columns: memberColumns,
    getRowId: (m) => m.id,
    manualPagination: true,
    enableSorting: false,
    enableHiding: false,
    enableRowSelection: false,
  });
  const add = () => {
    const provider = sources[0];
    const id = crypto.randomUUID();
    setDraft((d) => ({
      ...d,
      defaultModelId: d.defaultModelId || id,
      models: [
        ...d.models,
        {
          id,
          name: "",
          providerId: provider?.id ?? "",
          model: "",
          capabilities: defaultCapabilities(),
        },
      ],
    }));
    setSelected(id);
  };
  const policy = draft.routingPolicy ?? defaultRoutingPolicy();
  const validPolicy =
    Number.isInteger(policy.maxAttempts) &&
    policy.maxAttempts >= 1 &&
    policy.maxAttempts <= 5 &&
    Number.isInteger(policy.deadlineSeconds) &&
    policy.deadlineSeconds >= 5 &&
    policy.deadlineSeconds <= 300 &&
    Number.isInteger(policy.cooldownSeconds) &&
    policy.cooldownSeconds >= 1 &&
    policy.cooldownSeconds <= 3600 &&
    policy.disabledModels.length < draft.models.length &&
    policy.limits.every(
      (l) =>
        Number.isInteger(l.maxConcurrent) &&
        l.maxConcurrent >= 0 &&
        l.maxConcurrent <= 1024 &&
        Number.isInteger(l.requestsPerMinute) &&
        l.requestsPerMinute >= 0 &&
        l.requestsPerMinute <= 1000000,
    );
  const valid =
    validPolicy &&
    draft.name.trim() &&
    draft.models.length &&
    draft.models.every(
      (m) =>
        m.name.trim() &&
        m.providerId &&
        m.model &&
        Number.isInteger(m.capabilities.contextWindow) &&
        Number.isInteger(m.capabilities.maxOutputTokens) &&
        m.capabilities.maxOutputTokens < m.capabilities.contextWindow,
    );
  const save = async () => {
    if (locked || !valid) return;
    const publication = normalize(draft);
    await operation.start(
      { kind: "save", draft: publication },
      origin,
      (progress, isCurrent, storeId) =>
        verifyServiceModels(
          publication,
          providers,
          progress,
          service,
          isCurrent,
          (providerId, expectedRevision, model) =>
            modelServicesApi.verifyMember({
              storeId,
              providerId,
              expectedRevision,
              model,
            }),
        ),
    );
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !busy) onClose();
      }}
    >
      <DialogContent size="lg" className="flex max-h-[85dvh] flex-col">
        <DialogHeader>
          <DialogTitle>{service ? "编辑模型服务" : "创建模型服务"}</DialogTitle>
          <DialogDescription>
            统一组织不同 Provider
            的模型。发布后，已接入的默认入口在兼容的新轮次跟随服务默认值。
          </DialogDescription>
        </DialogHeader>
        <div className="min-h-0 space-y-5 overflow-y-auto px-1">
          <ServiceWriteFeedback operation={operation} />
          <Field>
            <FieldLabel htmlFor="service-name">服务名称</FieldLabel>
            <Input
              id="service-name"
              value={draft.name}
              maxLength={80}
              disabled={locked}
              onChange={(e) =>
                setDraft((d) => ({ ...d, name: e.target.value }))
              }
            />
          </Field>
          <RoutingPolicySettings
            value={draft.routingPolicy}
            disabled={locked}
            onChange={(routingPolicy) =>
              setDraft((d) => ({ ...d, routingPolicy }))
            }
          />
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h3 className="text-body font-medium">服务模型</h3>
            <Button
              variant="secondary"
              size="sm"
              disabled={locked || draft.models.length >= 32 || !sources.length}
              onClick={add}
            >
              添加模型
            </Button>
          </div>
          {!sources.length && (
            <p className="text-body text-fg-muted">
              先在 Providers 中配置模型来源，再添加服务模型。
            </p>
          )}
          <DataTable
            table={table}
            showPagination={false}
            aria-label="模型服务成员"
            role="region"
            emptyMessage="添加一个模型开始，模型来源可以跨 Provider。"
            onRowActivate={(row) => {
              if (!busy) setSelected(row.original.id);
            }}
            activeRowId={selected}
          />
          {member && (
            <section className="space-y-4" aria-label="编辑服务模型">
              {draft.routingPolicy?.enabled && (
                <>
                  <Switch
                    label="参与路由候选"
                    checked={
                      !draft.routingPolicy.disabledModels.includes(member.id)
                    }
                    disabled={locked}
                    onCheckedChange={(enabled) =>
                      setDraft((d) => {
                        const p = d.routingPolicy ?? defaultRoutingPolicy();
                        return {
                          ...d,
                          routingPolicy: {
                            ...p,
                            disabledModels: enabled
                              ? p.disabledModels.filter(
                                  (id) => id !== member.id,
                                )
                              : [...new Set([...p.disabledModels, member.id])],
                          },
                        };
                      })
                    }
                  />
                  <div className="grid gap-4 md:grid-cols-2">
                    {(
                      [
                        ["maxConcurrent", "来源模型最大并发"],
                        ["requestsPerMinute", "来源模型每分钟请求数"],
                      ] as const
                    ).map(([key, label]) => (
                      <Field key={key}>
                        <FieldLabel htmlFor={`limit-${key}`}>
                          {label}
                        </FieldLabel>
                        <Input
                          id={`limit-${key}`}
                          type="number"
                          min={0}
                          max={key === "maxConcurrent" ? 1024 : 1000000}
                          disabled={locked}
                          value={
                            draft.routingPolicy?.limits.find(
                              (l) => l.modelId === member.id,
                            )?.[key] ?? 0
                          }
                          onChange={(e) =>
                            setDraft((d) => {
                              const p =
                                d.routingPolicy ?? defaultRoutingPolicy();
                              const limit = p.limits.find(
                                (l) => l.modelId === member.id,
                              ) ?? {
                                modelId: member.id,
                                maxConcurrent: 0,
                                requestsPerMinute: 0,
                              };
                              return {
                                ...d,
                                routingPolicy: {
                                  ...p,
                                  limits: [
                                    ...p.limits.filter(
                                      (l) => l.modelId !== member.id,
                                    ),
                                    { ...limit, [key]: Number(e.target.value) },
                                  ],
                                },
                              };
                            })
                          }
                        />
                        <FieldDescription>
                          0 表示不设置本机额度；同一来源模型共享计数。
                        </FieldDescription>
                      </Field>
                    ))}
                  </div>
                </>
              )}
              <Field>
                <FieldLabel htmlFor="service-model-name">显示名称</FieldLabel>
                <Input
                  id="service-model-name"
                  value={member.name}
                  disabled={locked}
                  maxLength={80}
                  onChange={(e) => update({ name: e.target.value })}
                />
              </Field>
              <div className="grid gap-4 md:grid-cols-2">
                <Field>
                  <FieldLabel>Provider 来源</FieldLabel>
                  <Combobox<string>
                    items={sources.map((p) => p.id)}
                    value={member.providerId}
                    onValueChange={(value) => {
                      if (value) update({ providerId: value, model: "" });
                    }}
                    itemToStringLabel={(id) =>
                      sources.find((p) => p.id === id)?.name ?? id
                    }
                    disabled={locked}
                  >
                    <ComboboxInput
                      aria-label="Provider 来源"
                      placeholder="搜索 Provider"
                    />
                    <ComboboxContent>
                      <ComboboxEmpty>没有已配置来源</ComboboxEmpty>
                      <ComboboxList>
                        {(id) => (
                          <ComboboxItem key={id} value={id}>
                            {sources.find((p) => p.id === id)?.name ?? id}
                          </ComboboxItem>
                        )}
                      </ComboboxList>
                    </ComboboxContent>
                  </Combobox>
                </Field>
                <Field>
                  <FieldLabel>来源模型</FieldLabel>
                  <Combobox<string>
                    items={models}
                    value={member.model || null}
                    onValueChange={(value) => {
                      if (value)
                        update({ model: value, name: member.name || value });
                    }}
                    disabled={locked || !source}
                  >
                    <ComboboxInput
                      aria-label="来源模型"
                      placeholder="搜索已启用模型"
                    />
                    <ComboboxContent>
                      <ComboboxEmpty>请先在 Provider 中启用模型</ComboboxEmpty>
                      <ComboboxList>
                        {(id) => (
                          <ComboboxItem key={id} value={id}>
                            {id}
                          </ComboboxItem>
                        )}
                      </ComboboxList>
                    </ComboboxContent>
                  </Combobox>
                </Field>
              </div>
              <div className="grid gap-4 md:grid-cols-2">
                <Field>
                  <FieldLabel htmlFor="service-context">
                    上下文窗口（Token）
                  </FieldLabel>
                  <Input
                    id="service-context"
                    type="number"
                    min={2048}
                    max={2000000}
                    value={member.capabilities.contextWindow}
                    disabled={locked}
                    onChange={(e) =>
                      update({
                        capabilities: {
                          ...member.capabilities,
                          contextWindow: Number(e.target.value),
                        },
                      })
                    }
                  />
                </Field>
                <Field>
                  <FieldLabel htmlFor="service-output">
                    最大输出（Token）
                  </FieldLabel>
                  <Input
                    id="service-output"
                    type="number"
                    min={16}
                    max={65536}
                    value={member.capabilities.maxOutputTokens}
                    disabled={locked}
                    onChange={(e) =>
                      update({
                        capabilities: {
                          ...member.capabilities,
                          maxOutputTokens: Number(e.target.value),
                        },
                      })
                    }
                  />
                </Field>
              </div>
              <Switch
                label="支持标准工具调用"
                checked={member.capabilities.tools}
                disabled={locked}
                onCheckedChange={(tools) =>
                  update({ capabilities: { ...member.capabilities, tools } })
                }
              />
              {source &&
                /^https:\/\/api\.deepseek\.com(?:\/|$)/.test(
                  source.endpoint,
                ) && (
                  <Button
                    variant="tertiary"
                    size="sm"
                    disabled={locked}
                    onClick={() =>
                      update({
                        capabilities: {
                          ...member.capabilities,
                          contextWindow: 1000000,
                        },
                      })
                    }
                  >
                    使用 DeepSeek 官方 1M 上下文参考
                  </Button>
                )}
              <p className="text-label text-fg-muted">
                能力与上限为服务维护者声明，请依据来源模型填写。首批发布文本与标准工具，关闭思考，不发布图片或文件能力；短请求验证不代表完整能力验收。
              </p>
            </section>
          )}
          {!!draft.models.length && (
            <Field>
              <FieldLabel>服务默认模型</FieldLabel>
              <Select
                value={draft.defaultModelId}
                onValueChange={(value) => {
                  if (value)
                    setDraft((d) => normalize({ ...d, defaultModelId: value }));
                }}
                disabled={locked}
              >
                <SelectTrigger
                  aria-label="服务默认模型"
                  placeholder="选择默认模型"
                />
                <SelectContent>
                  {draft.models.map((m) => (
                    <SelectItem key={m.id} value={m.id}>
                      {m.name || "未命名模型"}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <FieldDescription>
                “默认”入口采用组内共同能力范围。首位为默认首选，其他成员按顺序候补；回退不改用户选择。增删条目或改变能力需要同步
                Agent 目录。
              </FieldDescription>
            </Field>
          )}
          <p className="text-label text-fg-muted" role="status">
            {progress ||
              "来源或能力变化会发送短请求验证；仅调整策略时沿用已发布的来源快照。"}
          </p>
        </div>
        <DialogFooter>
          <Button variant="tertiary" disabled={busy} onClick={onClose}>
            取消
          </Button>
          <Button
            loading={busy}
            disabled={!valid || locked}
            onClick={() => void save()}
          >
            验证并发布
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
