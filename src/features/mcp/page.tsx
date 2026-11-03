import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { McpUsageComparison } from "./components/mcp-usage-comparison";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { mcpApi } from "@/features/mcp/api";
import { mcpElapsed } from "@/features/mcp/components/mcp-call-detail";
import type { McpService, McpSnapshot } from "@/features/mcp/types";
import { date, number } from "@/features/sessions/display";

import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import { useEffect, useRef, useState } from "react";
import { useBackendReadEpoch } from "@/shared/hooks/use-backend-read-epoch";
import { backendReadEpoch } from "@/shared/lib/backend-recovery";
import { McpCallsTable } from "./components/mcp-calls-table";
import { McpGroupsTable } from "./components/mcp-groups-table";
import { Tokens } from "./components/mcp-token-summary";

type Filters = { agent: string; days: string; model: string; project: string };
function link(service: string | undefined, filters: Filters) {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(filters))
    if (value !== "all") params.set(key, value);
  window.location.hash = `/mcp${service ? `/${service}` : ""}${params.size ? `?${params}` : ""}`;
}

function Configurations({ services }: { services: McpService[] }) {
  const { agentNames, familyNames } = useAgentDirectory();
  return (
    <Accordion>
      <AccordionItem value="sources">
        <AccordionTrigger>
          配置来源与历史关联 · {services.length} 项
        </AccordionTrigger>
        <AccordionContent>
          <div className="space-y-3 p-3">
            {services.map((service) => (
              <article
                key={service.id}
                className="space-y-1 rounded-lg border border-border p-3 text-label"
              >
                <p className="font-medium">
                  {familyNames[service.agentId]} ·{" "}
                  {service.projectPath ?? "本机 / 跨项目"}
                </p>
                <p className="text-fg-muted">
                  配置适用：
                  {service.applicableAgentIds
                    .map((id) => agentNames[id])
                    .join(" / ") || "未知"}
                  {service.applicableAgentIds.length > 1
                    ? " · 多客户端共享，配置仅计一份；实际调用按记录归属"
                    : ""}
                </p>
                <p>
                  {service.configurationStatus === "present"
                    ? "配置已发现"
                    : "配置来源未知"}{" "}
                  ·{" "}
                  {service.configuredEnabled === false
                    ? "配置已禁用"
                    : service.configuredEnabled === true
                      ? "配置为启用"
                      : "启用状态未知"}{" "}
                  · 连接未知 · {service.transport ?? "传输未知"}
                </p>
                {service.sources.map((s) => (
                  <p
                    key={`${s.scope}:${s.path}`}
                    className="break-all text-fg-muted"
                  >
                    {s.path} · 文件更新 {date(s.modifiedAt)}
                  </p>
                ))}
                {!service.sources.length && (
                  <p className="text-fg-muted">
                    保留原生历史服务名，未确认当时配置或当前 Endpoint。
                  </p>
                )}
              </article>
            ))}
          </div>
        </AccordionContent>
      </AccordionItem>
    </Accordion>
  );
}

export function McpPage({
  selectedId,
  initialAgent,
  initialDays,
  initialModel,
  initialProject,
  refreshToken,
}: {
  selectedId?: string;
  initialAgent?: string;
  initialDays?: string;
  initialModel?: string;
  initialProject?: string;
  refreshToken?: number | string;
}) {
  const { agentFamily, agentNames, agentProducts, familyNames } =
    useAgentDirectory();
  const agent = initialAgent ?? "all",
    days = initialDays ?? "all",
    model = initialModel ?? "all",
    project = initialProject ?? "all";
  const filters = { agent, days, model, project };
  const key = JSON.stringify([selectedId, agent, days, model, project]);
  const [state, setState] = useState<{
    key: string;
    data: McpSnapshot | null;
    loading: boolean;
    error: string | null;
  }>({ key: "", data: null, loading: true, error: null });
  const [refresh, setRefresh] = useState(0),
    [search, setSearch] = useState("");
  const [more, setMore] = useState(false),
    [expandedCall, setExpandedCall] = useState("");
  const sequence = useRef(0);
  const epoch = useBackendReadEpoch();
  useEffect(() => {
    const token = ++sequence.current;
    let alive = true;
    setMore(false);
    setState((old) => ({
      key,
      data: old.key === key ? old.data : null,
      loading: true,
      error: null,
    }));
    mcpApi
      .mcp({
        agent: agent === "all" ? undefined : agent,
        days: days === "all" ? undefined : Number(days),
        model: model === "all" ? undefined : model,
        project: project === "all" ? undefined : project,
        service: selectedId,
      })
      .then((data) => {
        if (alive && sequence.current === token && epoch === backendReadEpoch())
          setState({ key, data, loading: false, error: null });
      })
      .catch((reason) => {
        if (alive && sequence.current === token && epoch === backendReadEpoch())
          setState((old) => ({
            key,
            data: old.key === key ? old.data : null,
            loading: false,
            error: reason instanceof Error ? reason.message : String(reason),
          }));
      });
    return () => {
      alive = false;
      sequence.current = token + 1;
    };
  }, [
    key,
    selectedId,
    agent,
    days,
    model,
    project,
    refresh,
    refreshToken,
    epoch,
  ]);
  const data = state.key === key ? state.data : null;
  const loading = state.key !== key || state.loading;
  const selected = data?.groups.find((g) => g.id === data.selectedGroupId);
  const rows =
    data?.groups.filter((g) =>
      `${g.name} ${[...g.agentIds, ...g.configuredAgentIds].map((a) => agentNames[a]).join(" ")} ${g.projectPaths.join(" ")}`
        .toLowerCase()
        .includes(search.trim().toLowerCase()),
    ) ?? [];
  const scope = [
    agent === "all"
      ? "全部 Agents"
      : (familyNames[agent as keyof typeof familyNames] ?? agentNames[agent]),
    days === "all" ? "全部已授权会话" : `最近 ${days} 天开始的已授权会话`,
    model === "all"
      ? "全部模型"
      : (data?.facets.models.find((f) => f.id === model)?.label ?? "所选模型"),
    project === "all"
      ? "全部项目"
      : (data?.facets.projects.find((f) => f.id === project)?.label ??
        "所选项目"),
  ].join(" · ");
  function change(patch: Partial<Filters>) {
    link(selected?.id ?? selectedId, { ...filters, ...patch });
  }
  async function loadMore() {
    if (!data?.nextCursor || more) return;
    const token = sequence.current;
    setMore(true);
    try {
      const next = await mcpApi.mcp({
        ...data.appliedFilter,
        cursor: data.nextCursor,
      });
      if (sequence.current === token && epoch === backendReadEpoch())
        setState({
          key,
          data: {
            ...next,
            calls: [
              ...data.calls,
              ...next.calls.filter(
                (c) => !data.calls.some((old) => old.call.id === c.call.id),
              ),
            ],
          },
          error: null,
          loading: false,
        });
    } catch (reason) {
      if (sequence.current === token && epoch === backendReadEpoch())
        setState((old) => ({
          ...old,
          error: reason instanceof Error ? reason.message : String(reason),
        }));
    } finally {
      if (sequence.current === token && epoch === backendReadEpoch())
        setMore(false);
    }
  }
  function refreshData() {
    setState((old) => ({ ...old, loading: true }));
    setRefresh((n) => n + 1);
  }
  return (
    <div className="min-w-0 space-y-6">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex flex-wrap gap-2">
          <Select value={agent} onValueChange={(v) => change({ agent: v })}>
            <SelectTrigger aria-label="MCP Agent" />
            <SelectContent>
              <SelectItem value="all">全部 Agents</SelectItem>
              {agentProducts.map(({ id, name }) => (
                <SelectItem key={id} value={id}>
                  {name}
                </SelectItem>
              ))}
              <SelectItem value="unknown">客户端未知</SelectItem>
              <SelectItem value="claude-unknown">
                Claude · 客户端未知
              </SelectItem>
              <SelectItem value="codex-unknown">Codex · 客户端未知</SelectItem>
            </SelectContent>
          </Select>
          <Select value={days} onValueChange={(v) => change({ days: v })}>
            <SelectTrigger aria-label="MCP 会话开始范围" />
            <SelectContent>
              <SelectItem value="all">全部会话</SelectItem>
              <SelectItem value="7">最近 7 天开始的会话</SelectItem>
              <SelectItem value="30">最近 30 天开始的会话</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <Button variant="tertiary" loading={loading} onClick={refreshData}>
          刷新 MCP 数据
        </Button>
      </div>
      {state.key === key && state.error && (
        <Alert status="danger">
          <AlertTitle>MCP 数据读取未完成</AlertTitle>
          <AlertDescription>
            {state.error}
            {data ? " 当前显示上次读取的快照。" : ""} 可刷新重试。
          </AlertDescription>
          {project !== "all" && (
            <Button
              variant="tertiary"
              onClick={() => link(selectedId, { ...filters, project: "all" })}
            >
              清除项目筛选
            </Button>
          )}
        </Alert>
      )}
      {selectedId && (
        <Button
          variant="ghost"
          onClick={() =>
            link(undefined, { ...filters, model: "all", project: "all" })
          }
        >
          返回 MCP 列表
        </Button>
      )}
      {!data ? (
        <ConnectionEmpty
          title={loading ? "正在读取 MCP 与调用…" : "MCP 数据暂不可用"}
          description="只读标准配置与已授权历史，不执行工具。"
        />
      ) : (
        <>
          <p className="text-label text-fg-subtle">
            更新于 {date(data.asOf)} · 部分采集
          </p>
          {!selectedId ? (
            <>
              <Input
                aria-label="搜索 MCP"
                placeholder="搜索 MCP、Agent 或项目"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
              />
              <p className="text-label text-fg-muted">
                {rows.filter((g) => g.identity === "server_name").length} 个 MCP
              </p>
              <div>
                <p className="text-label text-fg-muted">
                  关联模型 Token · 已去重
                </p>
                <Tokens group={data.usageSummary} />
              </div>
              {rows.length ? (
                <div className="space-y-4">
                  <McpUsageComparison
                    rows={rows}
                    onOpen={(id) => link(id, filters)}
                  />
                  <McpGroupsTable
                    rows={rows}
                    onOpen={(id) => link(id, filters)}
                  />
                </div>
              ) : (
                <ConnectionEmpty
                  title={
                    search ? "没有匹配的 MCP" : "当前范围未发现 MCP 配置或调用"
                  }
                  description="未发现不代表本机没有 MCP 服务。"
                />
              )}
            </>
          ) : selected ? (
            <>
              <section aria-label="MCP 聚合详情" className="space-y-4">
                <div className="flex flex-wrap items-center gap-2">
                  <h2 className="text-title font-medium">{selected.name}</h2>
                  <Badge>
                    {selected.identity === "unattributed"
                      ? "来源未知"
                      : "按 MCP 名称聚合"}
                  </Badge>
                </div>
                <p className="text-label text-fg-muted">
                  调用来源：
                  {selected.agentIds
                    .map((a) => agentNames[a] ?? a)
                    .join(" / ") || "尚无记录"}{" "}
                  · {selected.configurationCount} 份当前配置 · 当前连接未知 ·
                  原生历史
                </p>
                <div className="flex flex-wrap gap-2">
                  <Select
                    value={model}
                    onValueChange={(v) => change({ model: v })}
                  >
                    <SelectTrigger
                      aria-label="筛选 MCP 模型"
                      wrapperClassName="min-w-0 max-w-full"
                      className="w-64 max-w-full"
                    />
                    <SelectContent className="max-w-[calc(100vw-2rem)]">
                      <SelectItem value="all">全部模型</SelectItem>
                      {model !== "all" &&
                        !data.facets.models.some((f) => f.id === model) && (
                          <SelectItem value={model}>
                            所选模型 · 当前范围无记录
                          </SelectItem>
                        )}
                      {data.facets.models.map((f) => (
                        <SelectItem key={f.id} value={f.id}>
                          {f.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <Select
                    value={project}
                    onValueChange={(v) => change({ project: v })}
                  >
                    <SelectTrigger
                      aria-label="筛选 MCP 项目"
                      wrapperClassName="min-w-0 max-w-full"
                      className="w-80 max-w-full"
                    />
                    <SelectContent className="max-w-[calc(100vw-2rem)]">
                      <SelectItem value="all">全部项目</SelectItem>
                      {project !== "all" &&
                        !data.facets.projects.some((f) => f.id === project) && (
                          <SelectItem value={project}>
                            所选项目 · 当前范围无记录
                          </SelectItem>
                        )}
                      {data.facets.projects.map((f) => (
                        <SelectItem key={f.id} value={f.id}>
                          {f.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  {(agent !== "all" ||
                    model !== "all" ||
                    project !== "all") && (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() =>
                        change({ agent: "all", model: "all", project: "all" })
                      }
                    >
                      清除调用筛选
                    </Button>
                  )}
                </div>
                <p className="break-all text-label text-fg-subtle">{scope}</p>
                <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
                  <Metric
                    label="调用次数"
                    value={number(selected.callCount)}
                    note={`${selected.sessionCount} 个关联 Session`}
                  />
                  <div className="rounded-xl border border-border p-4">
                    <p className="mb-2 text-label text-fg-muted">
                      关联模型 Token
                    </p>
                    <Tokens group={selected} />
                  </div>
                  <Metric
                    label="最近一次调用"
                    value={date(selected.lastCallAt)}
                    note="当前筛选内的原生调用时间"
                  />
                  <Metric
                    label="原生延迟 P95"
                    value={mcpElapsed(selected.p95Ms)}
                    note={`${selected.durationSamples} 个有效样本 · ${selected.errorCount} 次原生错误`}
                  />
                </div>
                <Configurations
                  services={data.services.filter((s) =>
                    selected.serviceIds.includes(s.id),
                  )}
                />
              </section>
              <section aria-label="MCP 历史调用" className="space-y-3">
                <h2 className="text-title font-medium">
                  调用记录 · {number(data.totalCalls)}
                </h2>
                {data.calls.length ? (
                  <McpCallsTable
                    items={data.calls}
                    selected={selected}
                    scope={scope}
                    asOf={data.asOf}
                    expandedCall={expandedCall}
                    onExpandedCallChange={setExpandedCall}
                  />
                ) : (
                  <ConnectionEmpty
                    title="当前筛选没有已记录调用"
                    description={
                      selected.callCount == null
                        ? "来源未授权或读取未完成，实际调用数量未知。"
                        : "可调整 Agent、模型、项目或会话范围。"
                    }
                  />
                )}
                {data.nextCursor && (
                  <Button
                    variant="tertiary"
                    loading={more}
                    onClick={() => void loadMore()}
                  >
                    加载更多调用
                  </Button>
                )}
              </section>
            </>
          ) : null}
          <Accordion className="w-full">
            <AccordionItem value="scope">
              <AccordionTrigger>采集与统计说明</AccordionTrigger>
              <AccordionContent>
                <div className="space-y-2 text-label text-fg-muted">
                  <p>
                    按 MCP 名称聚合，同名服务可能来自不同
                    Endpoint。连接状态与无法确认的调用来源保留未知。
                  </p>
                  <p>
                    Token 来自关联模型响应，包含该响应其他内容，不是 MCP
                    独立用量。汇总已跨 MCP 去重，各行不可直接相加。
                  </p>
                  <p>会话索引更新于 {date(data.historyAsOf)}</p>
                  {data.sources
                    .filter(
                      (s) =>
                        agent === "all" ||
                        agent === "unknown" ||
                        s.agentId === agentFamily(agent),
                    )
                    .map((s) => (
                      <p key={s.agentId}>
                        {familyNames[s.agentId]} ·{" "}
                        {!s.enabled
                          ? "未授权"
                          : s.status === "ready"
                            ? "已接入"
                            : s.status === "partial"
                              ? "部分可读"
                              : "读取中"}
                      </p>
                    ))}
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => {
                      window.location.hash = "/agents";
                    }}
                  >
                    管理会话来源
                  </Button>
                </div>
              </AccordionContent>
            </AccordionItem>
          </Accordion>
          <div className="space-y-2">
            {data.warnings.map((w) => (
              <p key={w} className="break-all text-label text-fg-subtle">
                {w}
              </p>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
function Metric({
  label,
  value,
  note,
}: {
  label: string;
  value: string;
  note: string;
}) {
  return (
    <div className="space-y-2 rounded-xl border border-border p-4">
      <p className="text-label text-fg-muted">{label}</p>
      <p className="break-words text-title font-medium">{value}</p>
      <p className="text-label text-fg-subtle">{note}</p>
    </div>
  );
}
