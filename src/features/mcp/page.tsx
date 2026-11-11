import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { McpUsageComparison } from "./components/mcp-usage-comparison";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { PageActions, PageBody, PageHeader } from "@/components/ui/page-layout";
import { TabItem, TabPanel, Tabs, TabsList } from "@/components/ui/tabs";
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
import { useEffect, useLayoutEffect, useRef, useState } from "react";
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
  const externalAgent = agentProducts.some(
    (product) => product.id === agent && product.sourceId,
  );
  const unobserved = agentProducts.filter(
    (product) =>
      product.sourceId &&
      (agent === "all" || agent === product.id) &&
      !sourceProvides(product, "mcp"),
  );
  const filters = { agent, days, model, project };
  const key = JSON.stringify([selectedId, agent, days, model, project]);
  const [state, setState] = useState<{
    epoch: symbol | null;
    key: string;
    data: McpSnapshot | null;
    loading: boolean;
    error: string | null;
  }>({ epoch: null, key: "", data: null, loading: true, error: null });
  const [refresh, setRefresh] = useState(0),
    [search, setSearch] = useState("");
  const [view, setView] = useState("services");
  const [more, setMore] = useState(false);
  const sequence = useRef(0);
  const body = useRef<HTMLDivElement>(null);
  const title = useRef<HTMLHeadingElement>(null);
  const listPosition = useRef<{
    id: string;
    top: number;
    filters: Filters;
    epoch: symbol;
  } | null>(null);
  const epoch = useBackendReadEpoch();
  useEffect(() => {
    if (externalAgent) return;
    const token = ++sequence.current;
    let alive = true;
    setMore(false);
    setState((old) => ({
      epoch,
      key,
      data: old.epoch === epoch && old.key === key ? old.data : null,
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
          setState({ epoch, key, data, loading: false, error: null });
      })
      .catch((reason) => {
        if (alive && sequence.current === token && epoch === backendReadEpoch())
          setState((old) => ({
            epoch,
            key,
            data: old.epoch === epoch && old.key === key ? old.data : null,
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
    externalAgent,
  ]);
  const current = state.key === key && state.epoch === epoch;
  const data = current ? state.data : null;
  const loading = !current || state.loading;
  const selected = data?.groups.find((g) => g.id === data.selectedGroupId);
  const error = current ? state.error : null;
  useLayoutEffect(() => {
    if (!selectedId) return;
    body.current?.scrollTo(0, 0);
    title.current?.focus({ preventScroll: true });
  }, [selectedId]);
  useLayoutEffect(() => {
    const previous = listPosition.current;
    if (selectedId || !previous || (!data && !error)) return;
    listPosition.current = null;
    if (previous.epoch !== epoch || document.activeElement !== document.body)
      return;
    body.current?.scrollTo(0, previous.top);
    const trigger = body.current?.querySelector<HTMLButtonElement>(
      `[data-mcp-group="${CSS.escape(previous.id)}"]`,
    );
    (trigger ?? title.current)?.focus({ preventScroll: true });
    trigger?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [selectedId, data, error, epoch]);
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
  function openGroup(id: string) {
    listPosition.current = {
      id,
      top: body.current?.scrollTop ?? 0,
      filters,
      epoch,
    };
    link(id, filters);
  }
  function backToList() {
    const previous = listPosition.current;
    link(
      undefined,
      previous?.epoch === epoch
        ? previous.filters
        : { ...filters, model: "all", project: "all" },
    );
  }
  async function loadMore() {
    if (!data?.nextCursor || loading || more) return;
    const token = sequence.current;
    setMore(true);
    try {
      const next = await mcpApi.mcp({
        ...data.appliedFilter,
        cursor: data.nextCursor,
      });
      if (sequence.current === token && epoch === backendReadEpoch()) {
        const calls = [...data.calls];
        const seen = new Set(calls.map(({ call }) => call.id));
        for (const item of next.calls) {
          if (seen.has(item.call.id)) continue;
          seen.add(item.call.id);
          calls.push(item);
        }
        setState({
          epoch,
          key,
          data: { ...next, calls },
          error: null,
          loading: false,
        });
      }
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
    <PageBody ref={body} className="relative space-y-4 p-4 lg:p-6">
      <section
        className="min-w-0 space-y-4"
        aria-label={selectedId ? "MCP 聚合详情" : "MCP 工作区"}
      >
        <PageHeader className="px-0">
          <h2
            ref={title}
            tabIndex={-1}
            className="min-w-0 break-words text-title font-medium"
          >
            {selected?.name ?? (selectedId ? "MCP 详情" : "MCP")}
          </h2>
          <PageActions>
            {!externalAgent && selectedId && (
              <Button variant="tertiary" onClick={backToList}>
                返回 MCP 列表
              </Button>
            )}
            {!externalAgent && (
              <Button
                variant="tertiary"
                loading={loading}
                disabled={loading}
                onClick={refreshData}
              >
                重新读取 MCP 配置
              </Button>
            )}
          </PageActions>
        </PageHeader>
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
                <SelectItem value="codex-unknown">
                  Codex · 客户端未知
                </SelectItem>
              </SelectContent>
            </Select>
            {!externalAgent && (
              <Select value={days} onValueChange={(v) => change({ days: v })}>
                <SelectTrigger aria-label="MCP 会话开始范围" />
                <SelectContent>
                  <SelectItem value="all">全部会话</SelectItem>
                  <SelectItem value="7">最近 7 天开始的会话</SelectItem>
                  <SelectItem value="30">最近 30 天开始的会话</SelectItem>
                </SelectContent>
              </Select>
            )}
          </div>
        </div>
        {!!unobserved.length && (
          <p role="status" className="text-label text-fg-muted">
            {unobserved.map((product) => product.name).join("、")}
            的来源插件未提供 MCP 记录，调用次数未知。
          </p>
        )}
        <PublishedSources
          key={agent}
          mode="mcp"
          productId={agent === "all" ? undefined : agent}
        />
        {!externalAgent && error && (
          <Alert status="danger">
            <AlertTitle>MCP 数据读取未完成</AlertTitle>
            <AlertDescription>
              {error}
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
        {externalAgent ? null : !data ? (
          loading && (
            <p role="status" className="text-body text-fg-muted">
              正在读取 MCP 与调用…
            </p>
          )
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
                  {rows.filter((g) => g.identity === "server_name").length} 个
                  MCP
                </p>
                {rows.length ? (
                  <Tabs
                    value={view}
                    onValueChange={setView}
                    variant="underline"
                    color="default"
                  >
                    <TabsList aria-label="MCP 视图">
                      <TabItem value="services" label="服务目录" />
                      <TabItem value="analysis" label="调用分析" />
                    </TabsList>
                    <TabPanel value="services" className="pt-3">
                      <McpGroupsTable rows={rows} onOpen={openGroup} />
                    </TabPanel>
                    <TabPanel value="analysis" className="space-y-4 pt-3">
                      {!search.trim() && (
                        <div>
                          <p className="text-label text-fg-muted">
                            关联模型 Token · 已去重
                          </p>
                          <Tokens group={data.usageSummary} />
                        </div>
                      )}
                      <McpUsageComparison rows={rows} onOpen={openGroup} />
                    </TabPanel>
                  </Tabs>
                ) : (
                  <ConnectionEmpty
                    title={
                      search
                        ? "没有匹配的 MCP"
                        : "当前范围未发现 MCP 配置或调用"
                    }
                    description={
                      search ? undefined : "只显示已读取的配置和调用历史。"
                    }
                    action={search ? "清除搜索" : "检查 Agent 读取范围"}
                    onAction={() => {
                      if (search) setSearch("");
                      else window.location.hash = "/agents";
                    }}
                  />
                )}
              </>
            ) : selected ? (
              <>
                <div className="space-y-2">
                  <p className="text-label text-fg-muted">
                    {selected.agentIds
                      .map((a) => agentNames[a] ?? a)
                      .join(" / ") || "调用来源未知"}{" "}
                    · {selected.configurationCount} 份配置 · 连接状态未知
                  </p>
                  <div className="flex flex-wrap gap-2">
                    <Select
                      value={model}
                      onValueChange={(v) => change({ model: v })}
                    >
                      <SelectTrigger
                        aria-label="筛选 MCP 模型"
                        wrapperClassName="min-w-0 max-w-full"
                        className="w-48 max-w-full"
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
                        className="w-56 max-w-full"
                      />
                      <SelectContent className="max-w-[calc(100vw-2rem)]">
                        <SelectItem value="all">全部项目</SelectItem>
                        {project !== "all" &&
                          !data.facets.projects.some(
                            (f) => f.id === project,
                          ) && (
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
                </div>
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
                      disabled={loading}
                      loading={more}
                      onClick={() => void loadMore()}
                    >
                      加载更多调用
                    </Button>
                  )}
                </section>
                <Accordion>
                  <AccordionItem value="metrics">
                    <AccordionTrigger>调用指标</AccordionTrigger>
                    <AccordionContent>
                      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
                        <Metric
                          label="调用次数"
                          value={number(selected.callCount)}
                          note={`${selected.sessionCount} 个关联会话`}
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
                    </AccordionContent>
                  </AccordionItem>
                </Accordion>
                <Configurations
                  services={data.services.filter((s) =>
                    selected.serviceIds.includes(s.id),
                  )}
                />
              </>
            ) : (
              <ConnectionEmpty title="当前范围内没有该 MCP" />
            )}
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
      </section>
    </PageBody>
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
import {
  PublishedSources,
  sourceProvides,
} from "@/features/domain/published-sources";
