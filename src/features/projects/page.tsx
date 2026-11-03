import { ObjectContributionSlots } from "@/features/plugins/object-detail-slots";
import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { CostDetails } from "@/features/costs/details";
import { costText } from "@/shared/lib/money";
import { compactNumber } from "@/shared/lib/format";
import type { Snapshot } from "@/app/types";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Input } from "@/components/ui/input";
import {
  Combobox,
  ComboboxInput,
  ComboboxContent,
  ComboboxList,
  ComboboxItem,
  ComboboxEmpty,
} from "@/components/ui/combobox";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { TabItem, TabPanel, Tabs, TabsList } from "@/components/ui/tabs";
import { projectsApi } from "@/features/projects/api";
import { date, number } from "@/features/sessions/display";

import { ProjectCalls } from "./components/project-calls";
import { usePollingResource } from "@/shared/hooks/use-polling-resource";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import { useCallback, useDeferredValue, useState } from "react";
import { GitChanges, Writes } from "./components/changes";
import { CurrentActivity } from "./components/current-activity";
import { ProjectDirectoryAccess } from "./components/directory-access";
import { DiskGit } from "./components/disk-git";
import { Ports } from "./components/ports";
import { AgentTotalsView, Fact, Totals } from "./components/project-facts";
import { ProjectSessions } from "./components/project-sessions";
import { projectLink } from "./links";
import { HideProject, HiddenProjects } from "./components/project-visibility";
import type { HiddenProject } from "./types";

const tabs = [
  ["overview", "总览"],
  ["sessions", "Sessions"],
  ["cost", "费用"],
  ["changes", "修改记录"],
  ["ports", "端口与服务"],
  ["disk", "磁盘与 Git"],
];
function projectListLink(agent: string, scope: string) {
  const params = new URLSearchParams();
  if (agent !== "all") params.set("agent", agent);
  if (scope !== "workspace") params.set("scope", scope);
  return `/projects${params.size ? `?${params}` : ""}`;
}
export function ProjectsPage({
  data,
  selectedId,
  selectedTab,
  anchor,
  initialAgent,
  initialScope,
  refresh,
}: {
  data: Snapshot | null;
  selectedId?: string;
  selectedTab?: string;
  anchor?: string;
  initialAgent?: string;
  initialScope?: string;
  refresh: () => Promise<void>;
}) {
  const { agentNames, agentProducts } = useAgentDirectory();
  const agent = initialAgent ?? "all";
  const scope = initialScope ?? "workspace";
  const [days, setDays] = useState("all");
  const [search, setSearch] = useState("");
  const query = useDeferredValue(search);
  const [limit, setLimit] = useState(40);
  const visibilityBase = JSON.stringify(data?.hiddenProjects ?? []);
  const [visibility, setVisibility] = useState<{
    base: string;
    items: HiddenProject[];
  } | null>(null);
  const hidden =
    visibility?.base === visibilityBase
      ? visibility.items
      : (data?.hiddenProjects ?? []);
  const visibilityKey = JSON.stringify(hidden);
  const selectedHidden = hidden.find((p) => p.id === selectedId);
  const sourceKey =
    data?.sessionSources.map((s) => `${s.agentId}:${s.enabled}`).join(":") ??
    "none";
  const loader = useCallback(async () => {
    const filter = {
      scope,
      agent: agent === "all" ? undefined : agent,
      days: days === "all" ? undefined : Number(days),
      query: selectedId ? undefined : query,
    };
    let result = await projectsApi.projects(filter);
    while (result.nextCursor && (selectedId || result.items.length < limit)) {
      const next = await projectsApi.projects({
        ...filter,
        cursor: result.nextCursor,
      });
      result = { ...next, items: [...result.items, ...next.items] };
    }
    return result;
  }, [agent, scope, days, query, limit, selectedId]);
  const list = usePollingResource(
    `projects:${scope}:${agent}:${days}:${query}:${limit}:${sourceKey}:${visibilityKey}`,
    loader,
    Boolean(data),
  );
  const detailLoader = useCallback(
    () =>
      projectsApi.project(selectedId!, {
        scope,
        agent: agent === "all" ? undefined : agent,
        days: days === "all" ? undefined : Number(days),
      }),
    [selectedId, days, agent, scope],
  );
  const detail = usePollingResource(
    `project:${selectedId}:${scope}:${agent}:${days}:${sourceKey}:${visibilityKey}`,
    detailLoader,
    Boolean(selectedId && data && !selectedHidden),
  );
  const tab = tabs.some(([id]) => id === selectedTab)
    ? selectedTab!
    : "overview";
  const open = (id: string, next = tab) => {
    window.location.hash = projectLink(id, next, undefined, agent, scope);
  };
  const changeAgent = (value: string) => {
    setLimit(40);
    window.location.hash = selectedId
      ? projectLink(selectedId, tab, undefined, value, scope)
      : projectListLink(value, scope);
  };
  const reloadList = list.invalidate;
  const reloadDetail = detail.invalidate;
  const visibilityChanged = useCallback(
    async (items: HiddenProject[]) => {
      setVisibility({ base: visibilityBase, items });
      setLimit(40);
      if (items.some((p) => p.id === selectedId))
        window.location.hash = projectListLink(agent, scope);
      await refresh();
      reloadList();
      reloadDetail();
    },
    [
      visibilityBase,
      selectedId,
      agent,
      scope,
      refresh,
      reloadList,
      reloadDetail,
    ],
  );
  if (!data)
    return (
      <ConnectionEmpty
        title="尚未连接本机数据"
        description="请在桌面应用中查看项目。"
        icon="folder"
      />
    );
  const current = detail.data;
  const options = new Map((list.data?.items ?? []).map((p) => [p.id, p]));
  if (
    current &&
    !hidden.some((p) => p.id === current.project.id) &&
    !options.has(current.project.id)
  )
    options.set(current.project.id, current.project);
  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div className="flex min-w-0 flex-wrap items-center gap-3">
          {selectedId && (
            <Button
              variant="tertiary"
              onClick={() => {
                window.location.hash = projectListLink(agent, scope);
              }}
            >
              全部项目
            </Button>
          )}
          {selectedId && !selectedHidden && options.size ? (
            <div className="w-80 min-w-0 max-w-full">
              <Combobox<string>
                items={[...options.keys()]}
                value={selectedId}
                limit={200}
                itemToStringLabel={(id) => {
                  const p = options.get(id);
                  return p ? `${p.name} · ${p.path}` : id;
                }}
                onValueChange={(id) => {
                  if (id && options.has(id)) open(id, "overview");
                }}
              >
                <ComboboxInput
                  aria-label="切换项目"
                  placeholder="搜索项目名称或路径"
                  triggerAriaLabel="展开项目目录"
                />
                <ComboboxContent>
                  <ComboboxEmpty>没有匹配的项目</ComboboxEmpty>
                  <ComboboxList>
                    {(id: string) => (
                      <ComboboxItem key={id} value={id}>
                        <span className="max-w-96 truncate">
                          {options.get(id)?.name} · {options.get(id)?.path}
                        </span>
                      </ComboboxItem>
                    )}
                  </ComboboxList>
                </ComboboxContent>
              </Combobox>
            </div>
          ) : null}
          <Select
            value={scope}
            onValueChange={(value) => {
              setSearch("");
              setLimit(40);
              window.location.hash = projectListLink(agent, value);
            }}
          >
            <SelectTrigger aria-label="项目收录范围" />
            <SelectContent>
              <SelectItem value="workspace">常规项目</SelectItem>
              <SelectItem value="temporary">临时与验证目录</SelectItem>
              <SelectItem value="all">全部会话目录</SelectItem>
            </SelectContent>
          </Select>
          <Select value={agent} onValueChange={changeAgent}>
            <SelectTrigger aria-label="项目 Agent" />
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
          <Select
            value={days}
            onValueChange={(value) => {
              setDays(value);
              setLimit(40);
            }}
          >
            <SelectTrigger
              aria-label="项目历史日期"
              wrapperClassName="max-w-52"
            />
            <SelectContent>
              <SelectItem value="all">全部日期</SelectItem>
              <SelectItem value="1">今天</SelectItem>
              <SelectItem value="7">近 7 天</SelectItem>
              <SelectItem value="30">近 30 天</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <HiddenProjects items={hidden} onChanged={visibilityChanged} />
        <Button
          variant="tertiary"
          loading={list.loading || detail.loading}
          onClick={() => {
            list.reload();
            detail.reload();
          }}
        >
          刷新项目
        </Button>
      </div>
      {(list.error || (!selectedHidden && detail.error)) && (
        <Alert>
          <AlertTitle>项目读取未完成</AlertTitle>
          <AlertDescription>
            {selectedHidden ? list.error : (detail.error ?? list.error)}{" "}
            已保留的快照可能过期，可刷新重试。
          </AlertDescription>
        </Alert>
      )}
      {selectedId ? (
        selectedHidden ? (
          <Alert>
            <AlertTitle>此项目已隐藏</AlertTitle>
            <AlertDescription>
              原始 Session
              和磁盘文件仍保留。在“已隐藏项目”中恢复后即可重新查看。
            </AlertDescription>
          </Alert>
        ) : current ? (
          <div key={current.project.id} className="space-y-6">
            <div className="space-y-2">
              <div className="flex flex-wrap items-center justify-between gap-3">
                <h2 className="text-heading font-medium">
                  {current.project.name}
                </h2>
                <HideProject
                  project={current.project}
                  onChanged={visibilityChanged}
                />
              </div>
              <p className="break-all text-body text-fg-muted">
                {current.project.path}
              </p>
              <div className="flex flex-wrap gap-2">
                {current.project.projectKind !== "workspace" && (
                  <Badge status="neutral">
                    {current.project.projectKind === "verification"
                      ? "本应用验证目录"
                      : "临时工作目录"}
                  </Badge>
                )}
                <Badge status="neutral">
                  {current.git.state === "not_git"
                    ? "非 Git 项目"
                    : (current.git.branch ?? "Git 元数据待确认")}
                </Badge>
                <Badge status="neutral">
                  {current.project.locationState === "available"
                    ? "目录可访问"
                    : "目录当前不可访问"}
                </Badge>
              </div>
              {current.project.locationState === "available" &&
                current.project.gitRoot &&
                current.git.state !== "ready" && (
                  <ProjectDirectoryAccess
                    projectId={current.project.id}
                    onSelected={detail.invalidate}
                  />
                )}
            </div>
            <Tabs
              value={tab}
              onValueChange={(value) => open(selectedId, value)}
              variant="underline"
              color="default"
            >
              <div className="overflow-x-auto">
                <TabsList aria-label="项目详情">
                  {tabs.map(([id, label]) => (
                    <TabItem key={id} value={id} label={label} />
                  ))}
                </TabsList>
              </div>
              <TabPanel value="cost" className="min-w-0 pt-5">
                {tab === "cost" && (
                  <CostDetails
                    key={`${selectedId}:${agent}:${days}:${sourceKey}`}
                    scope="project"
                    id={selectedId}
                    agent={agent === "all" ? undefined : agent}
                    days={days === "all" ? undefined : Number(days)}
                  />
                )}
              </TabPanel>
              <TabPanel value="overview" className="space-y-6 pt-5">
                <Totals
                  totals={current.project.totals}
                  listener={current.project.listenerCount}
                />
                <AgentTotalsView
                  project={current.project}
                  onFilter={changeAgent}
                />
                <CurrentActivity detail={current} />
                <ProjectCalls
                  key={`${selectedId}:${agent}:${days}:${sourceKey}`}
                  projectId={selectedId}
                  agent={agent}
                  days={days}
                  sourceKey={sourceKey}
                />
                {current.listeners.items.length > 0 && (
                  <section className="space-y-3">
                    <h3 className="text-title font-medium">
                      当前开发服务 · {date(current.listeners.asOf)}
                    </h3>
                    <div className="flex flex-wrap gap-2">
                      {current.listeners.items.slice(0, 5).map((port) => (
                        <Button
                          key={port.id}
                          variant="tertiary"
                          onClick={() => {
                            window.location.hash = projectLink(
                              selectedId,
                              "ports",
                              port.id,
                              agent,
                              scope,
                            );
                          }}
                        >
                          {port.commandRedacted} · {port.address}:{port.port}
                        </Button>
                      ))}
                    </div>
                  </section>
                )}
                <div className="flex flex-wrap gap-2">
                  <Button
                    variant="tertiary"
                    onClick={() => open(selectedId, "sessions")}
                  >
                    查看关联会话
                  </Button>
                  <Button
                    variant="tertiary"
                    onClick={() => open(selectedId, "changes")}
                  >
                    查看修改记录
                  </Button>
                  <Button
                    variant="tertiary"
                    onClick={() => open(selectedId, "ports")}
                  >
                    查看当前监听端口
                  </Button>
                </div>
                <p className="text-label text-fg-subtle">
                  写入仅统计明确成功结果；缺少记录不能推断没有修改。
                </p>
              </TabPanel>
              <TabPanel value="sessions" className="space-y-4 pt-5">
                <ProjectSessions detail={current} />
              </TabPanel>
              <TabPanel value="changes" className="space-y-6 pt-5">
                <Writes
                  key={`${selectedId}:${agent}:${days}:${anchor ?? "none"}`}
                  projectId={selectedId}
                  agent={agent}
                  scope={scope}
                  items={current.writes}
                  anchor={anchor}
                />
                <GitChanges projectId={selectedId} detail={current} />
              </TabPanel>
              <TabPanel value="ports" className="space-y-4 pt-5">
                <Ports detail={current} anchor={anchor} />
              </TabPanel>
              <TabPanel value="disk" className="space-y-6 pt-5">
                <DiskGit detail={current} />
              </TabPanel>
            </Tabs>
            <ObjectContributionSlots
              object={{
                target: "project",
                id: current.project.id,
                origin: { kind: "builtin" },
              }}
            />
          </div>
        ) : (
          <ConnectionEmpty
            compact
            title={detail.loading ? "正在读取项目…" : "项目暂不可用"}
            description="项目需要来自已授权的真实会话；可返回列表或重新读取。"
            icon="folder"
          />
        )
      ) : (
        <>
          <Input
            aria-label="搜索项目名称或目录"
            placeholder="搜索项目名称或目录"
            value={search}
            onChange={(event) => {
              setSearch(event.target.value);
              setLimit(40);
            }}
          />
          {list.data && (
            <>
              <div className="flex flex-wrap justify-between gap-2 text-label text-fg-muted">
                <span>
                  {list.data.total} 个项目 · {list.data.summary.sessionCount}{" "}
                  个会话 · {number(list.data.summary.usage.totalTokens)} 已知
                  Token
                </span>
                <span>
                  更新于 {date(list.data.asOf)}
                  {list.data.unassignedSessions > 0
                    ? ` · ${list.data.unassignedSessions} 个会话项目未知`
                    : ""}
                  {list.data.dateUnknownExcluded > 0
                    ? ` · 日期未知已排除 ${list.data.dateUnknownExcluded} 个会话`
                    : ""}
                </span>
              </div>
              {list.data.summary.usageUnknownSessions > 0 && (
                <p className="text-label text-fg-subtle">
                  {list.data.summary.usageUnknownSessions} 个会话用量不完整
                </p>
              )}
            </>
          )}
          {list.data?.items.length ? (
            <div className="grid gap-4 lg:grid-cols-2">
              {list.data.items.map((project) => (
                <article
                  key={project.id}
                  className="space-y-4 rounded-xl border border-border p-5"
                >
                  <div className="flex items-start justify-between gap-3">
                    <div className="min-w-0">
                      <h2 className="text-title font-medium">{project.name}</h2>
                      <p className="mt-1 break-all text-label text-fg-muted">
                        {project.path}
                      </p>
                    </div>
                    <div className="flex shrink-0 flex-wrap gap-2">
                      <HideProject
                        project={project}
                        onChanged={visibilityChanged}
                      />
                      <Button
                        variant="tertiary"
                        size="sm"
                        onClick={() => open(project.id, "overview")}
                      >
                        查看项目
                      </Button>
                    </div>
                  </div>
                  <div className="flex flex-wrap gap-2">
                    {project.agents.map((agent) => (
                      <Badge key={agent.agentId} status="neutral">
                        {agentNames[agent.agentId] ?? agent.agentId}
                      </Badge>
                    ))}
                    <Badge status="neutral">
                      {project.locationState !== "available"
                        ? "目录不可用 · Git 未知"
                        : project.gitRoot
                          ? "Git / worktree"
                          : "非 Git 目录"}
                    </Badge>
                  </div>
                  <dl className="grid grid-cols-3 gap-4">
                    <Fact
                      label="会话"
                      value={String(project.totals.sessionCount)}
                    />
                    <Fact
                      label="Token · 已知"
                      value={compactNumber(project.totals.usage.totalTokens)}
                      exact={number(project.totals.usage.totalTokens)}
                    />
                    <Fact
                      label="监听端口"
                      value={
                        project.listenerCount === null
                          ? `未知 · 已知 ${project.knownListenerCount}`
                          : String(project.listenerCount)
                      }
                    />
                  </dl>
                  <Fact
                    label="关联费用"
                    value={costText(project.totals.associatedCosts?.cost)}
                  />
                  <p className="text-label text-fg-subtle">
                    {project.totals.associatedCosts?.unlinkedNativeResponses
                      ? "费用原生覆盖未确认 · "
                      : ""}
                    {project.totals.usageMissingCalls > 0
                      ? `${project.totals.usageMissingCalls} 个响应用量不完整 · `
                      : ""}
                    最近活动 {date(project.lastActivity)}
                  </p>
                </article>
              ))}
            </div>
          ) : (
            <ConnectionEmpty
              compact
              title={
                list.loading
                  ? "正在关联项目…"
                  : search
                    ? "没有匹配的项目"
                    : "尚未关联项目"
              }
              description="项目来自已授权会话的工作目录，不扫描其他项目文件夹。"
              icon="folder"
            />
          )}
          {list.data?.nextCursor && (
            <Button
              variant="tertiary"
              loading={list.loading}
              onClick={() => setLimit((value) => value + 40)}
            >
              加载更多项目
            </Button>
          )}
        </>
      )}
      <Accordion className="w-full">
        <AccordionItem value="scope">
          <AccordionTrigger>统计说明</AccordionTrigger>
          <AccordionContent>
            <div className="space-y-2 text-label text-fg-muted">
              <p>
                项目来自会话工作目录。常规项目不含临时与验证目录；原始会话仍保留。
              </p>
              <p>
                日期按会话开始时间筛选，修改按发生时间筛选。Token
                按模型响应去重。
              </p>
              <p>
                Git 与端口显示项目当前状态，不按历史日期筛选，也不归属某个
                Agent。仅采集当前用户可见的 TCP 监听端口。
              </p>
            </div>
          </AccordionContent>
        </AccordionItem>
      </Accordion>
    </div>
  );
}
