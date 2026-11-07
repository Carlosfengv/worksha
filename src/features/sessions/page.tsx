import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import type { Page, Snapshot } from "@/app/types";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { MobileDrawer } from "@/components/ui/mobile-drawer";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { sessionsApi } from "@/features/sessions/api";
import { ConversationReader } from "@/features/sessions/components/conversation-reader";
import { ReadingCache } from "./reading-cache";
import { SessionList } from "@/features/sessions/components/session-list";
import { SessionPagination } from "./components/session-pagination";
import { useBackendReadEpoch } from "@/shared/hooks/use-backend-read-epoch";
import { number, sessionLink } from "@/features/sessions/display";
import { useNativePage } from "@/features/sessions/hooks/use-native-page";
import { useSessionPosition } from "./hooks/use-session-position";
import type { NativeSession } from "@/features/sessions/types";
import { groupSessionsByProject } from "./session-collections";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";

import {
  useCallback,
  useEffect,
  useDeferredValue,
  useMemo,
  useRef,
  useState,
  type RefObject,
} from "react";
import { SessionDateFilter } from "./components/session-date-filter";
import { SessionRequests } from "./components/session-requests";
import { UnifiedWorkspace } from "@/features/domain/session-workspace";

interface SessionProps {
  initialUnified?: boolean;
  initialDomainSource?: string;
  nativeOnly?: boolean;
  data: Snapshot | null;
  navigate: (page: Page) => void;
  selectedId?: string;
  initialAgent?: string;
  anchor?: string;
  returnAgent?: string;
  returnTab?: string;
  returnTo?: string;
  initialView?: "context" | "requests";
}
export function SessionsPage(props: SessionProps) {
  const sourceKey =
    props.data?.sessionSources
      .map((s) => `${s.agentId}:${s.enabled}`)
      .join(",") ?? "offline";
  if (
    (props.initialUnified && props.initialDomainSource) ||
    (!props.nativeOnly &&
      !props.selectedId &&
      !props.initialAgent &&
      props.data?.agents.some((agent) => agent.sourceId && agent.connected))
  )
    return (
      <UnifiedWorkspace
        key={props.initialDomainSource}
        initialSource={props.initialDomainSource}
        defaultAll={!props.initialDomainSource}
      />
    );
  return (
    <SessionWorkspace
      key={`${sourceKey}:${JSON.stringify(props.data?.hiddenProjects ?? [])}`}
      {...props}
    />
  );
}
function SessionWorkspace({
  data,
  navigate,
  selectedId,
  initialAgent,
  anchor,
  returnAgent,
  returnTab,
  returnTo,
  initialView,
}: SessionProps) {
  const { agentFamily, agentProducts, familyNames, isSharedSource } =
    useAgentDirectory();
  const [agent, setAgent] = useState(initialAgent ?? "all");
  const [search, setSearch] = useState("");
  const deferredSearch = useDeferredValue(search);
  const [days, setDays] = useState("all");
  const [grouped, setGrouped] = useState(false);
  const [drawer, setDrawer] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const [detailSummary, setDetailSummary] = useState<NativeSession | null>(
    null,
  );
  const cache = useRef(new ReadingCache());
  const epoch = useBackendReadEpoch();
  const queryKey = `${agent}:${deferredSearch}:${days}`;
  const [entry, setEntry] = useState({
    id: selectedId,
    query: queryKey,
    relaxing: false,
  });
  const [choice, setChoice] = useState<{
    id: string;
    query: string;
    epoch: symbol;
    route?: string;
  } | null>(null);
  const drawerTrigger = useRef<HTMLButtonElement>(null);
  const loader = useCallback(
    (cursor?: string) =>
      sessionsApi.sessions({
        agent: agent === "all" ? undefined : agent,
        query: deferredSearch || undefined,
        days: days === "all" ? undefined : Number(days),
        cursor,
        includeGateway: true,
      }),
    [agent, deferredSearch, days],
  );
  const sessions = useNativePage(queryKey, loader, "page");
  const items = sessions.data?.items ?? [];
  const hasFilter = Boolean(search) || days !== "all";
  const knownSelection =
    items.some((item) => item.id === selectedId) ||
    (choice !== null &&
      choice.id === selectedId &&
      choice.query === queryKey &&
      choice.epoch === epoch);
  const incoming = selectedId !== undefined && entry.id !== selectedId;
  const confirmedEmpty =
    sessions.data?.total === 0 &&
    ["native_history", "gateway_history"].includes(sessions.data.coverage);
  const targetId =
    incoming || entry.relaxing
      ? selectedId
      : choice?.query === queryKey &&
          choice.epoch === epoch &&
          choice.route === selectedId
        ? choice.id
        : (selectedId ?? items[0]?.id);
  const explicitAgent = targetId?.split(":")[0];
  const sourceEnabled = data?.sessionSources.some(
    (s) => s.enabled && s.agentId === explicitAgent,
  );
  const selectedInList = items.some((s) => s.id === targetId);
  const requestedRequests =
    initialView === "requests" && targetId === selectedId;
  const retainedChoice =
    choice?.query === queryKey &&
    choice.epoch === epoch &&
    choice.id === targetId;
  const requestedVisible =
    targetId &&
    (!confirmedEmpty || requestedRequests) &&
    !sessions.scopeInvalid &&
    (sourceEnabled ||
      selectedInList ||
      requestedRequests ||
      (retainedChoice && detailSummary?.status === "observed")) &&
    (incoming ||
      entry.relaxing ||
      agent === "all" ||
      agent === "unknown" ||
      explicitAgent === agentFamily(agent)) &&
    (incoming ||
      entry.relaxing ||
      selectedInList ||
      retainedChoice ||
      (!hasFilter && entry.query === queryKey && entry.id === selectedId));
  const effectiveId = sessions.scopeInvalid
    ? undefined
    : requestedVisible
      ? targetId
      : items[0]?.id;
  const selectedSourceEnabled =
    effectiveId &&
    data?.sessionSources.some(
      (s) => s.enabled && s.agentId === effectiveId.split(":")[0],
    );
  const selectedSession =
    items.find((s) => s.id === effectiveId) ??
    (retainedChoice && detailSummary?.id === effectiveId
      ? detailSummary
      : null);
  const showRequests =
    selectedSession?.status === "observed" ||
    (initialView === "requests" && effectiveId === selectedId);
  const canReadConversation = Boolean(
    selectedSourceEnabled && selectedSession?.status !== "observed",
  );
  const pinned =
    requestedVisible &&
    !selectedInList &&
    detailSummary?.id === effectiveId &&
    choice?.epoch === epoch
      ? detailSummary
      : null;
  const {
    desktopList,
    mobileList,
    expandedProjects,
    expandProjects,
    rememberScroll,
    turn,
  } = useSessionPosition({
    scope: queryKey,
    epoch,
    pages: sessions,
    drawer,
    selected: effectiveId,
    summaryId: detailSummary?.id,
  });
  const onSummary = useCallback(
    (session: NativeSession) => {
      setDetailSummary(session);
      setChoice({ id: session.id, query: queryKey, epoch, route: selectedId });
    },
    [queryKey, epoch, selectedId],
  );
  useEffect(() => {
    if (selectedSession?.status === "observed" && !retainedChoice)
      onSummary(selectedSession);
  }, [selectedSession, retainedChoice, onSummary]);
  useEffect(() => {
    const conflict =
      selectedId &&
      agent !== "all" &&
      agent !== "unknown" &&
      selectedId.split(":")[0] !== agentFamily(agent);
    if (entry.id === selectedId && !conflict) {
      if (entry.relaxing && entry.query === queryKey)
        setEntry({ ...entry, relaxing: false });
      return;
    }
    if (selectedId && !knownSelection && (hasFilter || agent !== "all")) {
      setSearch("");
      setDays("all");
      setAgent("all");
      setEntry({ id: selectedId, query: "all::all", relaxing: true });
    } else setEntry({ id: selectedId, query: queryKey, relaxing: false });
  }, [
    selectedId,
    queryKey,
    entry,
    hasFilter,
    agent,
    agentFamily,
    knownSelection,
  ]);
  useEffect(() => {
    if (sessions.scopeInvalid) {
      cache.current = new ReadingCache();
    }
  }, [sessions.scopeInvalid]);
  const openSession = (id: string, view?: "requests") => {
    window.location.hash = sessionLink(id, {
      agent: agent === "all" ? undefined : agent,
      from: returnAgent,
      tab: returnTab,
      returnTo,
      view,
    });
    setDrawer(false);
  };
  const select = (session: NativeSession) =>
    openSession(
      session.id,
      session.status === "observed" ? "requests" : undefined,
    );
  const resetSelection = () => {
    window.location.hash = "/sessions";
  };
  const refresh = async () => {
    if (refreshing) return;
    setRefreshing(true);
    setRefreshError(null);
    try {
      await sessionsApi.refreshSessions();
      await sessions.invalidate();
    } catch (reason) {
      setRefreshError(
        reason instanceof Error ? reason.message : String(reason),
      );
    } finally {
      setRefreshing(false);
    }
  };
  const groups = useMemo(
    () =>
      grouped
        ? groupSessionsByProject(sessions.data?.items ?? [])
        : new Map<string, NativeSession[]>(),
    [grouped, sessions.data?.items],
  );
  const sidebar = (listRef: RefObject<HTMLDivElement | null>) => (
    <aside
      aria-label="会话标题列表"
      className="flex h-full min-h-0 min-w-0 flex-col border-r border-border"
    >
      <div className="shrink-0 space-y-3 border-b border-border p-3">
        <div className="flex items-center justify-between gap-2">
          <h2 className="text-body font-medium">全部会话</h2>
          <Badge size="sm">
            {number(sessions.scopeInvalid ? undefined : sessions.data?.total)}
          </Badge>
        </div>
        <Input
          aria-label="搜索会话"
          placeholder="搜索标题、项目或路径"
          value={search}
          onChange={(event) => {
            setSearch(event.target.value);
            resetSelection();
          }}
        />
        <div className="grid grid-cols-2 gap-2">
          <Select
            value={agent}
            onValueChange={(value) => {
              setAgent(value);
              resetSelection();
            }}
          >
            <SelectTrigger
              aria-label="筛选会话 Agent"
              wrapperClassName="min-w-0"
              className="w-full min-w-0"
            />
            <SelectContent>
              <SelectItem value="all">全部 Agents</SelectItem>
              {agentProducts.map((p) => (
                <SelectItem key={p.id} value={p.id}>
                  {p.name}
                </SelectItem>
              ))}
              <SelectItem value="unknown">客户端来源未知</SelectItem>
              {isSharedSource(agent) && (
                <SelectItem value={agent}>{familyNames[agent]}</SelectItem>
              )}
              {agent.endsWith("-unknown") && (
                <SelectItem value={agent}>
                  {familyNames[agent.slice(0, -8)]?.replace(
                    " · 全部客户端",
                    "",
                  ) ?? agent.slice(0, -8)}{" "}
                  · 客户端来源未知
                </SelectItem>
              )}
            </SelectContent>
          </Select>
          <SessionDateFilter
            value={days}
            onChange={(value) => {
              setDays(value);
              resetSelection();
            }}
          />
        </div>
        <div className="flex flex-wrap gap-2">
          {data?.agents.some((agent) => agent.sourceId && agent.connected) && (
            <Button asChild variant="tertiary" size="sm">
              <a href="#/sessions">查看全部来源</a>
            </Button>
          )}
          <Button
            variant="ghost"
            size="sm"
            active={grouped}
            onClick={() => setGrouped(!grouped)}
          >
            {grouped ? "会话列表" : "按项目分组"}
          </Button>
          <Button
            variant="ghost"
            size="sm"
            loading={refreshing}
            onClick={() => void refresh()}
          >
            重新检查来源
          </Button>
        </div>
        {data?.sessionSources.some(
          (s) => s.enabled && ["pending", "scanning"].includes(s.status),
        ) && (
          <p role="status" className="text-label text-fg-subtle">
            正在读取会话…
          </p>
        )}
      </div>
      <div
        ref={listRef}
        aria-label="会话列表滚动区域"
        tabIndex={0}
        className="min-h-0 flex-1 overflow-y-auto [overflow-anchor:none]"
        aria-busy={sessions.loading}
        onScroll={(event) => rememberScroll(event.currentTarget.scrollTop)}
      >
        {pinned && (
          <div className="border-b border-border">
            <p className="px-3 pt-3 text-label text-fg-subtle">当前会话</p>
            <SessionList
              items={[pinned]}
              selected={effectiveId}
              onSelect={select}
            />
          </div>
        )}
        {grouped ? (
          <Accordion
            type="multiple"
            value={expandedProjects}
            onValueChange={expandProjects}
            className="w-full p-2"
          >
            {[...groups].map(([path, group]) => (
              <AccordionItem key={path} value={path}>
                <AccordionTrigger>
                  <span className="break-all">
                    {path} · 本页 {group.length} 个
                  </span>
                </AccordionTrigger>
                <AccordionContent>
                  <SessionList
                    items={group}
                    selected={effectiveId}
                    onSelect={select}
                  />
                </AccordionContent>
              </AccordionItem>
            ))}
          </Accordion>
        ) : (
          <SessionList items={items} selected={effectiveId} onSelect={select} />
        )}
        {!sessions.loading && sessions.data && items.length === 0 && (
          <ConnectionEmpty
            compact
            title={
              hasFilter || agent !== "all"
                ? "没有匹配会话"
                : sessions.data.coverage === "partial"
                  ? "会话列表尚未完成核查"
                  : sessions.data.coverage === "not_connected"
                    ? "尚未读取会话"
                    : sessions.data.coverage === "native_history"
                      ? "没有匹配会话"
                      : "暂无会话记录"
            }
            description={
              hasFilter || agent !== "all"
                ? "可调整搜索、Agent 或日期。"
                : "有会话标识的模型请求会自动出现在这里。客户端保存的对话可在 Agents 中授权读取。"
            }
            action={hasFilter || agent !== "all" ? "清除筛选" : "查看 Agents"}
            onAction={() => {
              if (hasFilter || agent !== "all") {
                setSearch("");
                setDays("all");
                setAgent("all");
                resetSelection();
              } else navigate("agents");
            }}
          />
        )}
        {sessions.loading && !sessions.data && (
          <p role="status" className="p-4 text-body text-fg-muted">
            正在读取会话列表…
          </p>
        )}
      </div>
      <SessionPagination
        loading={sessions.loading}
        previous={sessions.previousCursors.length > 0}
        next={Boolean(sessions.data?.nextCursor)}
        first={sessions.cursor === undefined}
        count={items.length}
        total={sessions.scopeInvalid ? undefined : sessions.data?.total}
        partial={sessions.data?.coverage === "partial"}
        onNext={() => turn("next")}
        onPrevious={() => turn("previous")}
        onFirst={() => turn("first")}
      />
      {(sessions.error || refreshError) && (
        <div className="shrink-0 space-y-2 border-t border-border p-3">
          <p role="alert" className="text-label text-fg-muted">
            {sessions.error ?? refreshError}
          </p>
          <Button
            variant="tertiary"
            size="sm"
            onClick={() => void sessions.reload()}
          >
            刷新列表
          </Button>
        </div>
      )}
    </aside>
  );
  return (
    <div className="session-workspace">
      <div className="session-desktop-list min-h-0 min-w-0">
        {sidebar(desktopList)}
      </div>
      <div className="flex min-h-0 min-w-0 flex-col">
        <div className="session-mobile-switch shrink-0 border-b border-border p-2">
          <Button
            ref={drawerTrigger}
            variant="tertiary"
            size="sm"
            onClick={() => setDrawer(true)}
          >
            切换会话 / 项目
          </Button>
        </div>
        {effectiveId && canReadConversation && (
          <div
            className="flex shrink-0 gap-2 border-b border-border p-2"
            aria-label="会话视图"
          >
            <Button
              size="sm"
              variant="ghost"
              active={!showRequests}
              onClick={() => openSession(effectiveId)}
            >
              对话
            </Button>
            <Button
              size="sm"
              variant="ghost"
              active={showRequests}
              onClick={() => openSession(effectiveId, "requests")}
            >
              模型请求
            </Button>
          </div>
        )}
        {effectiveId && showRequests ? (
          <SessionRequests
            key={effectiveId}
            id={effectiveId}
            title={selectedSession?.title}
            returnTo={returnTo}
            canReadConversation={canReadConversation}
          />
        ) : effectiveId && canReadConversation ? (
          <ConversationReader
            key={effectiveId}
            id={effectiveId}
            anchor={selectedId === effectiveId ? anchor : undefined}
            initialView={
              selectedId === effectiveId && initialView === "context"
                ? "context"
                : undefined
            }
            cache={cache.current}
            onSummary={onSummary}
            returnAgent={returnAgent}
            returnTab={returnTab}
            returnTo={returnTo}
          />
        ) : (
          <section
            aria-label="会话内容"
            className="min-h-0 flex-1 overflow-y-auto"
          >
            <ConnectionEmpty
              title={confirmedEmpty ? "没有匹配会话" : "从会话标题列表开始"}
              description="选择会话后，可以查看相关模型请求和已授权读取的对话。"
              action="查看 Agents"
              onAction={() => navigate("agents")}
              icon="message-circle"
            />
          </section>
        )}
      </div>
      <MobileDrawer
        open={drawer}
        onClose={() => setDrawer(false)}
        triggerRef={drawerTrigger}
        ariaLabel="选择会话或项目"
        panelClassName="w-80 max-w-full"
      >
        {sidebar(mobileList)}
      </MobileDrawer>
    </div>
  );
}
