import { ObjectContributionSlots } from "@/features/plugins/object-detail-slots";
import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { useBackendReadEpoch } from "@/shared/hooks/use-backend-read-epoch";
import { backendReadEpoch } from "@/shared/lib/backend-recovery";
import { CostDetails } from "@/features/costs/details";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DetailList,
  DetailListItem,
  DetailListLabel,
  DetailListValue,
} from "@/components/ui/detail-list";
import { TabItem, TabPanel, Tabs, TabsList } from "@/components/ui/tabs";
import { SessionMcpLatency } from "@/features/mcp/components/mcp-call-detail";
import { sessionsApi } from "@/features/sessions/api";
import {
  sessionAgentLabel,
  clientAttributionLabel,
  bytes,
  date,
  kindNames,
  mcpReturnPath,
  requestReturnPath,
  number,
  sessionLink,
  sessionStorage,
  status,
} from "@/features/sessions/display";
import type {
  Conversation,
  ConversationEvent,
  NativeSession,
} from "@/features/sessions/types";
import { DesktopError } from "@/shared/lib/desktop-command";
import type { Usage } from "@/shared/types/usage";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import { useEffect, useRef, useState } from "react";
import { ModelFailureDetail, failureLabels } from "./model-failure-detail";
import { SessionContextPanel } from "./session-context";
import { appendUniqueById } from "../session-collections";
import {
  READING_TRAIL_PAGES,
  type ReadingCache,
  type ReadingPage,
  type ReadingState,
} from "../reading-cache";
export type { ReadingCache } from "../reading-cache";

function keepReading(cache: ReadingCache, id: string, view: ReadingState) {
  cache.set(id, view.epoch ? view : { ...view, epoch: backendReadEpoch() });
}

export function ConversationReader({
  id,
  anchor,
  cache,
  onSummary,
  returnAgent,
  returnTab,
  returnTo,
  initialView,
}: {
  id: string;
  anchor?: string;
  cache: ReadingCache;
  onSummary: (session: NativeSession) => void;
  returnAgent?: string;
  returnTab?: string;
  returnTo?: string;
  initialView?: "context";
}) {
  const { agentNames, agentFamily } = useAgentDirectory();
  const epoch = useBackendReadEpoch();
  const previousEpoch = useRef(epoch);
  const cached = cache.get(id);
  const initial = !cached?.epoch || cached.epoch === epoch ? cached : undefined;
  const returnMcp = mcpReturnPath(
    returnTo,
    (id) => id === "unknown" || agentFamily(id) !== undefined,
  );
  const returnRequest = requestReturnPath(returnTo);
  const [view, setView] = useState<ReadingState>(
    () =>
      initial ?? {
        epoch,
        data: null,
        expanded: [],
        scroll: 0,
        tab: anchor ? "conversation" : (initialView ?? "conversation"),
      },
  );
  const [loading, setLoading] = useState(false);
  const [awaitingIndex, setAwaitingIndex] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [request, setRequest] = useState(0);
  const [more, setMore] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const live = useRef(false);
  const sequence = useRef(0);
  const paging = useRef(false);
  const lastAnchor = useRef<string | null>(null);
  const patch = (next: ReadingState) => {
    keepReading(cache, id, next);
    setView(next);
  };
  useEffect(() => {
    const recovered = previousEpoch.current !== epoch;
    previousEpoch.current = epoch;
    for (const [cachedId, saved] of cache)
      if (saved.epoch && saved.epoch !== epoch) cache.delete(cachedId);
    live.current = true;
    paging.current = false;
    setMore(false);
    const token = ++sequence.current;
    let retryTimer: ReturnType<typeof setTimeout> | undefined;
    let finishWait: (() => void) | undefined;
    const current = () =>
      live.current &&
      sequence.current === token &&
      epoch === backendReadEpoch();
    let saved = cache.get(id);
    if (request > 0 || saved?.originAnchor !== anchor) saved = undefined;
    if (saved?.data) {
      const data = saved.data;
      saved = { ...saved, epoch: saved.epoch ?? epoch };
      if (anchor && saved.page?.anchor === anchor)
        saved = {
          ...saved,
          tab: "conversation",
          expanded: [...new Set([...saved.expanded, anchor])],
        };
      else if (initialView) saved = { ...saved, tab: initialView, scroll: 0 };
      keepReading(cache, id, saved);
      setView(saved);
      onSummary(data.session);
    } else {
      setLoading(true);
      setAwaitingIndex(false);
      setError(null);
      const position = saved;
      const page = position?.page ?? {
        anchor,
        scroll: 0,
        expanded: anchor ? [anchor] : [],
      };
      const read = async (): Promise<Conversation> => {
        for (let attempt = 0; ; attempt++) {
          try {
            return await sessionsApi.conversation(id, page.cursor, page.anchor);
          } catch (reason) {
            // Live context links can precede the first complete native index.
            // Retry only this bounded missing-record case; consent failures stop.
            if (
              !current() ||
              recovered ||
              initialView !== "context" ||
              anchor ||
              !(reason instanceof DesktopError) ||
              reason.code !== "not_found" ||
              attempt >= 5
            )
              throw reason;
            setAwaitingIndex(true);
            if (attempt === 0) {
              await sessionsApi.refreshSessions();
              if (!current()) throw reason;
            }
            await new Promise<void>((resolve) => {
              finishWait = resolve;
              retryTimer = setTimeout(
                resolve,
                Math.min(1000 * 2 ** attempt, 4000),
              );
            });
            if (!current()) throw reason;
          }
        }
      };
      void read()
        .then((data) => {
          if (!current()) return;
          const next = {
            epoch,
            data: { ...data, items: appendUniqueById([], data.items) },
            originAnchor: anchor,
            page,
            previous: position?.previous ?? [],
            expanded: position?.expanded ?? page.expanded,
            scroll: position?.scroll ?? page.scroll,
            tab: anchor
              ? "conversation"
              : ((request > 0 ? cache.get(id)?.tab : initialView) ??
                cache.get(id)?.tab ??
                initialView ??
                "conversation"),
          };
          keepReading(cache, id, next);
          setView(next);
          onSummary(data.session);
        })
        .catch((reason) => {
          if (!current()) return;
          setError(reason instanceof Error ? reason.message : String(reason));
          if (
            reason instanceof DesktopError &&
            ["not_found", "unauthorized", "stale_snapshot"].includes(
              reason.code,
            )
          ) {
            cache.delete(id);
            setView({
              epoch,
              data: null,
              expanded: [],
              scroll: 0,
              tab: "conversation",
            });
          }
        })
        .finally(() => {
          if (current()) setLoading(false);
        });
    }
    return () => {
      live.current = false;
      sequence.current += 1;
      clearTimeout(retryTimer);
      finishWait?.();
    };
  }, [id, anchor, initialView, cache, onSummary, request, epoch]);
  useEffect(() => {
    const node = scrollRef.current;
    if (!node || !view.data) return;
    const scroll = cache.get(id)?.scroll ?? view.scroll;
    const candidate =
      anchor && lastAnchor.current !== anchor && scroll === 0
        ? document.getElementById(anchor)
        : null;
    const target = candidate && node.contains(candidate) ? candidate : null;
    // Expanded details can change height after the first layout. Restore this
    // page until user interaction; observe only its bounded mounted children.
    let frame = 0;
    const position = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        if (target)
          node.scrollTop +=
            target.getBoundingClientRect().top -
            node.getBoundingClientRect().top -
            12;
        else node.scrollTop = scroll;
      });
    };
    const observer = new ResizeObserver(position);
    if (target) observer.observe(target);
    else for (const child of node.children) observer.observe(child);
    position();
    if (target) lastAnchor.current = anchor!;
    const stop = () => {
      observer.disconnect();
      cancelAnimationFrame(frame);
    };
    const interactions = [
      "wheel",
      "touchstart",
      "pointerdown",
      "keydown",
    ] as const;
    for (const event of interactions)
      node.addEventListener(event, stop, { passive: true, once: true });
    return () => {
      stop();
      for (const event of interactions) node.removeEventListener(event, stop);
    };
  }, [id, anchor, view.data, view.tab, view.scroll, cache]);
  const loadPage = async (back = false) => {
    if (
      !view.data ||
      (back ? !view.previous?.length : !view.data.nextCursor) ||
      paging.current ||
      more ||
      loading ||
      (view.epoch && view.epoch !== epoch)
    )
      return;
    paging.current = true;
    setMore(true);
    setError(null);
    const token = ++sequence.current;
    try {
      const previous = view.previous ?? [];
      const page: ReadingPage = back
        ? previous[previous.length - 1]
        : { cursor: view.data.nextCursor!, scroll: 0, expanded: [] };
      const data = await sessionsApi.conversation(id, page.cursor, page.anchor);
      if (
        !live.current ||
        token !== sequence.current ||
        epoch !== backendReadEpoch()
      )
        return;
      const current = cache.get(id);
      const next = {
        ...view,
        tab: current?.tab ?? view.tab,
        data: { ...data, items: appendUniqueById([], data.items) },
        page,
        previous: back
          ? previous.slice(0, -1)
          : [
              ...previous,
              {
                ...view.page,
                scroll: scrollRef.current?.scrollTop ?? view.scroll,
                expanded: current?.expanded ?? view.expanded,
              },
            ].slice(-READING_TRAIL_PAGES),
        expanded: page.expanded,
        scroll: page.scroll,
      };
      patch(next);
      onSummary(data.session);
      requestAnimationFrame(() => {
        if (live.current && sequence.current === token)
          scrollRef.current?.focus({ preventScroll: true });
      });
    } catch (reason) {
      if (
        live.current &&
        token === sequence.current &&
        epoch === backendReadEpoch()
      ) {
        setError(reason instanceof Error ? reason.message : String(reason));
        if (
          reason instanceof DesktopError &&
          ["not_found", "unauthorized", "stale_snapshot"].includes(reason.code)
        ) {
          cache.delete(id);
          setView({
            epoch,
            data: null,
            expanded: [],
            scroll: 0,
            tab: "conversation",
          });
        }
      }
    } finally {
      if (
        live.current &&
        token === sequence.current &&
        epoch === backendReadEpoch()
      ) {
        paging.current = false;
        setMore(false);
      }
    }
  };
  const data = view.epoch === epoch ? view.data : null;
  const session = data?.session;
  const navigation = data && (
    <nav
      aria-label="对话分页"
      className="shrink-0 flex flex-wrap items-center gap-2 border-b border-border px-4 py-2"
    >
      <Button
        variant="tertiary"
        size="sm"
        disabled={loading || more || !view.previous?.length}
        onClick={() => void loadPage(true)}
      >
        上一页对话
      </Button>
      <Button
        variant="tertiary"
        size="sm"
        disabled={loading || more || !data.nextCursor}
        loading={more}
        onClick={() => void loadPage()}
      >
        下一页对话
      </Button>
      <span role="status" className="text-label text-fg-subtle">
        本页 {data.items.length} 条 · 会话共 {data.total} 条
      </span>
      {(view.page?.cursor || anchor) && (
        <Button
          variant="ghost"
          size="sm"
          disabled={loading || more}
          onClick={() => {
            window.location.hash = window.location.hash
              .replace(/([?&])anchor=[^&]*&?/, "$1")
              .replace(/[?&]$/, "");
            keepReading(cache, id, {
              epoch,
              data: null,
              expanded: [],
              scroll: 0,
              tab: view.tab,
            });
            setRequest((n) => n + 1);
          }}
        >
          从对话开头阅读
        </Button>
      )}
    </nav>
  );
  return (
    <section
      aria-label="会话内容"
      className="flex min-h-0 min-w-0 flex-1 flex-col"
    >
      <header className="shrink-0 space-y-2 border-b border-border p-4">
        <div className="flex items-start justify-between gap-2">
          <h2
            className="line-clamp-2 text-title font-medium"
            title={session?.title}
          >
            {session?.title ?? "读取会话"}
          </h2>
          <Button
            variant="ghost"
            size="sm"
            loading={loading}
            onClick={() => {
              setRequest((n) => n + 1);
            }}
          >
            刷新对话
          </Button>
        </div>
        {session && (
          <>
            <p className="text-label text-fg-muted">
              {sessionAgentLabel(session, agentNames)} ·{" "}
              {session.projectName ?? "项目未知"} · {date(session.updatedAt)}
            </p>
            {session.modelErrorCount > 0 && session.latestFailureId && (
              <Button
                variant="tertiary"
                size="sm"
                onClick={() => {
                  window.location.hash = sessionLink(id, {
                    anchor: session.latestFailureId!,
                    from: returnAgent,
                    tab: returnTab,
                    returnTo,
                  });
                }}
              >
                已记录 {session.modelErrorCount} 条异常 · 查看最近异常
              </Button>
            )}
            <p className="text-label text-fg-subtle">
              {number(session.usage.totalTokens)} Token ·{" "}
              {sessionStorage(session)}
            </p>
          </>
        )}
        {returnRequest && (
          <Button
            variant="tertiary"
            size="sm"
            onClick={() => {
              window.location.hash = returnRequest;
            }}
          >
            返回来源请求
          </Button>
        )}
        {returnMcp && (
          <Button
            variant="tertiary"
            size="sm"
            onClick={() => {
              window.location.hash = returnMcp;
            }}
          >
            返回 MCP 服务与调用
          </Button>
        )}
        {returnAgent && (
          <Button
            variant="tertiary"
            size="sm"
            onClick={() => {
              window.location.hash = `/agents/${returnAgent}?tab=${returnTab === "calls" ? "calls" : "sessions"}`;
            }}
          >
            返回 Agent {returnTab === "calls" ? "调用" : "Sessions"}
          </Button>
        )}
        {session?.projectId && !session.projectHidden && (
          <Button
            variant="tertiary"
            size="sm"
            onClick={() => {
              window.location.hash = `/projects/${encodeURIComponent(session.projectId!)}`;
            }}
          >
            查看项目 · {session.projectName ?? "项目详情"}
          </Button>
        )}
        {session?.projectHidden && (
          <p className="text-label text-fg-muted">
            此项目已隐藏；会话与原始文件仍保留，可在项目页恢复显示。
          </p>
        )}
      </header>
      {loading && awaitingIndex && (
        <p role="status" className="shrink-0 p-3 text-label text-fg-muted">
          会话暂未就绪，正在重试…
        </p>
      )}
      {error && (
        <div className="shrink-0 p-3">
          <Alert>
            <AlertTitle>会话读取未完成</AlertTitle>
            <AlertDescription>
              {error} 可使用“刷新对话”重新读取。
            </AlertDescription>
          </Alert>
        </div>
      )}
      {data ? (
        <Tabs
          value={view.tab}
          onValueChange={(tab) => patch({ ...view, tab, scroll: 0 })}
          variant="underline"
          color="default"
          className="flex min-h-0 flex-1 flex-col"
        >
          <div className="shrink-0 overflow-x-auto border-b border-border px-4">
            <TabsList aria-label="会话详情">
              <TabItem value="conversation" label="对话" />
              <TabItem value="timeline" label="调用时间线" />
              <TabItem value="context" label="上下文" />
              <TabItem value="cost" label="费用" />
              <TabItem value="disk" label="磁盘与项目" />
            </TabsList>
          </div>
          <TabPanel
            value="conversation"
            className="flex min-h-0 flex-1 flex-col"
          >
            {navigation}
            <div
              ref={view.tab === "conversation" ? scrollRef : undefined}
              aria-label="对话滚动区域"
              aria-busy={loading || more}
              tabIndex={0}
              className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4 [overflow-anchor:none]"
              onScroll={(event) => {
                cache.scroll(id, event.currentTarget.scrollTop);
              }}
            >
              {data.warnings.map((warning) => (
                <p
                  key={warning}
                  role="status"
                  className="text-label text-fg-muted"
                >
                  {warning}
                </p>
              ))}
              {data.items.map((event) => (
                <ConversationEntry
                  key={event.id}
                  event={event}
                  agentId={session!.agentId}
                  expanded={view.expanded.includes(event.id)}
                  onExpand={(open) =>
                    patch({
                      ...view,
                      scroll: scrollRef.current?.scrollTop ?? 0,
                      expanded: open
                        ? [...new Set([...view.expanded, event.id])]
                        : view.expanded.filter((id) => id !== event.id),
                    })
                  }
                />
              ))}
              {data.items.length === 0 && (
                <ConnectionEmpty
                  compact
                  title="此会话暂无可读对话"
                  description="原生记录未提供可识别的用户消息或调用内容。"
                />
              )}
              <p className="text-label text-fg-subtle">
                本页 {data.items.length} 条记录 · {date(data.asOf)}
              </p>
              {view.tab === "conversation" && (
                <ObjectContributionSlots
                  object={{
                    target: "session",
                    id,
                    origin: { kind: "builtin" },
                  }}
                />
              )}
            </div>
          </TabPanel>
          <TabPanel value="timeline" className="flex min-h-0 flex-1 flex-col">
            {navigation}
            <div
              ref={view.tab === "timeline" ? scrollRef : undefined}
              aria-label="调用时间线滚动区域"
              tabIndex={0}
              aria-busy={loading || more}
              className="min-h-0 flex-1 space-y-3 overflow-y-auto p-4 [overflow-anchor:none]"
              onScroll={(event) =>
                cache.scroll(id, event.currentTarget.scrollTop)
              }
            >
              {data.items
                .filter((e) =>
                  ["model", "tool", "mcp", "skill", "diagnostic"].includes(
                    e.kind,
                  ),
                )
                .map((event) => (
                  <ConversationEntry
                    key={event.id}
                    event={event}
                    agentId={session!.agentId}
                    expanded={view.expanded.includes(event.id)}
                    onExpand={(open) =>
                      patch({
                        ...view,
                        scroll: scrollRef.current?.scrollTop ?? 0,
                        expanded: open
                          ? [...new Set([...view.expanded, event.id])]
                          : view.expanded.filter((id) => id !== event.id),
                      })
                    }
                  />
                ))}
              {view.tab === "timeline" && (
                <ObjectContributionSlots
                  object={{
                    target: "session",
                    id,
                    origin: { kind: "builtin" },
                  }}
                />
              )}
            </div>
          </TabPanel>
          <TabPanel
            value="context"
            className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4"
          >
            {view.tab === "context" && (
              <SessionContextPanel
                context={data.context}
                partial={data.coverage === "partial"}
              />
            )}
            {view.tab === "context" && (
              <ObjectContributionSlots
                object={{ target: "session", id, origin: { kind: "builtin" } }}
              />
            )}
          </TabPanel>
          <TabPanel
            value="cost"
            className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4"
          >
            {view.tab === "cost" && (
              <CostDetails key={id} scope="session" id={id} />
            )}
            <Button
              variant="tertiary"
              onClick={() => {
                window.location.hash = `/requests?source=gateway&range=all&session=${encodeURIComponent(id)}`;
              }}
            >
              查看网关调用
            </Button>
            {view.tab === "cost" && (
              <ObjectContributionSlots
                object={{ target: "session", id, origin: { kind: "builtin" } }}
              />
            )}
          </TabPanel>
          <TabPanel
            value="disk"
            className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4"
          >
            {session && (
              <>
                <DetailList>
                  <DetailListItem>
                    <DetailListLabel>项目位置</DetailListLabel>
                    <DetailListValue>
                      <span className="break-all">
                        {session.projectPath ?? "原生记录未提供"}
                      </span>
                    </DetailListValue>
                  </DetailListItem>
                  <DetailListItem>
                    <DetailListLabel>Git 远程地址</DetailListLabel>
                    <DetailListValue>
                      {session.gitRemote ?? "原生记录未提供"}
                    </DetailListValue>
                  </DetailListItem>
                  <DetailListItem>
                    <DetailListLabel>原生会话来源</DetailListLabel>
                    <DetailListValue>
                      <span className="break-all">{session.sourcePath}</span>
                    </DetailListValue>
                  </DetailListItem>
                  <DetailListItem>
                    <DetailListLabel>
                      {session.diskScope === "native_database_payload"
                        ? "内容估算 / 独立磁盘占用"
                        : "文件大小 / 实际占用"}
                    </DetailListLabel>
                    <DetailListValue>
                      {bytes(session.diskBytes)} /{" "}
                      {bytes(session.diskAllocatedBytes)}
                    </DetailListValue>
                  </DetailListItem>
                  <DetailListItem>
                    <DetailListLabel>原生 ID / 版本</DetailListLabel>
                    <DetailListValue>
                      <span className="break-all">
                        {session.nativeId} · {session.agentVersion ?? "未知"}
                      </span>
                    </DetailListValue>
                  </DetailListItem>
                  <DetailListItem>
                    <DetailListLabel>会话累计用量</DetailListLabel>
                    <DetailListValue>
                      {number(session.usage.totalTokens)} Token
                    </DetailListValue>
                  </DetailListItem>
                </DetailList>
                <p className="text-body text-fg-muted">
                  {session.diskScope === "native_database_payload"
                    ? "数据库由多个会话共享，此处仅估算该会话记录的内容字节数；无法独立分摊 SQLite 页面、WAL、附件与缓存，实际占用未知。清理能力尚未接入。"
                    : "占用仅统计此原生会话文件，不含附件、子 Agent 和缓存。清理能力尚未接入。"}
                </p>
              </>
            )}
            {view.tab === "disk" && (
              <ObjectContributionSlots
                object={{ target: "session", id, origin: { kind: "builtin" } }}
              />
            )}
          </TabPanel>
        </Tabs>
      ) : (
        <div className="min-h-0 flex-1 overflow-y-auto">
          <ConnectionEmpty
            title={loading ? "正在读取原生会话…" : "会话内容暂不可用"}
            description="从已授权的本机来源读取，不会发起模型或工具调用。"
          />
        </div>
      )}
    </section>
  );
}

function TextBlock({ text, label }: { text: string; label?: string }) {
  return (
    <div className="min-w-0 space-y-1">
      {label && (
        <h4 className="text-label font-medium text-fg-muted">{label}</h4>
      )}
      <pre className="max-w-full overflow-x-auto whitespace-pre-wrap break-words rounded-lg border border-border p-3 font-mono text-label">
        {text}
      </pre>
    </div>
  );
}
function UsageDetail({ usage }: { usage: Usage }) {
  return (
    <dl className="grid grid-cols-2 gap-2 text-label sm:grid-cols-3">
      {[
        ["输入总量", usage.inputTokens],
        ["输出总量", usage.outputTokens],
        ["缓存命中（输入子集）", usage.cachedInputTokens],
        ["缓存写入", usage.cacheWriteInputTokens],
        ["推理（输出子集）", usage.reasoningOutputTokens],
        ["总 Token", usage.totalTokens],
      ].map(([label, value]) => (
        <div key={String(label)}>
          <dt className="text-fg-muted">{label}</dt>
          <dd className="mt-1 font-medium">{number(value as number | null)}</dd>
        </div>
      ))}
    </dl>
  );
}
function ConversationEntry({
  event,
  agentId,
  expanded,
  onExpand,
}: {
  event: ConversationEvent;
  agentId: NativeSession["agentId"];
  expanded: boolean;
  onExpand: (open: boolean) => void;
}) {
  const { agentNames } = useAgentDirectory();
  const isCall = ["model", "tool", "mcp", "skill", "diagnostic"].includes(
    event.kind,
  );
  const isContext = event.kind === "event" && Boolean(event.text);
  const summary = (
    <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-label">
      <span className="font-medium">{event.name}</span>
      <span className="text-fg-subtle">
        {event.agentProductId
          ? agentNames[event.agentProductId]
          : "客户端来源未知"}
      </span>
      <span className="text-fg-subtle">{date(event.timestamp)}</span>
      {isCall && (
        <Badge size="sm">
          {event.kind in kindNames
            ? kindNames[event.kind as keyof typeof kindNames]
            : "记录"}
        </Badge>
      )}
      {isCall && (
        <span className="text-fg-muted">
          {event.status === "error" &&
          (event.kind === "mcp" || event.toolSource?.status === "unknown")
            ? "原生错误（细分未知）"
            : status(event.status)}
        </span>
      )}
      {event.toolSource?.status === "configuration" && (
        <span className="text-fg-muted">{event.server} · 当前配置关联</span>
      )}
      {event.toolSource?.status === "unknown" && (
        <span className="text-fg-muted">来源未知</span>
      )}
      {event.failure && (
        <span className="text-fg-muted">
          {failureLabels[event.failure.category] ?? failureLabels.unknown}
        </span>
      )}
    </div>
  );
  return (
    <article
      id={event.id}
      aria-label={event.name}
      className="min-w-0 space-y-2 rounded-xl border border-border p-3"
    >
      {summary}
      {event.text && !isContext && (
        <p className="whitespace-pre-wrap break-words text-body leading-relaxed">
          {event.text}
        </p>
      )}
      {(isCall || isContext) && (
        <Accordion
          value={expanded ? event.id : ""}
          onValueChange={(value: string) => onExpand(Boolean(value))}
          className="w-full"
        >
          <AccordionItem value={event.id}>
            <AccordionTrigger>
              {event.failure
                ? `${event.model ?? "模型未知"} · 异常原因与详情`
                : isContext
                  ? "查看会话上下文"
                  : event.kind === "model"
                    ? `${event.model ?? "模型未知"} · ${number(event.usage?.totalTokens)} Token · 调用详情`
                    : "参数与结果详情"}
            </AccordionTrigger>
            <AccordionContent>
              <div className="min-w-0 space-y-3 p-3">
                <p className="break-all text-label text-fg-muted">
                  {event.failure?.source === "opencode_log"
                    ? `日志聚合 ID：${event.nativeId} · 原生消息 ID 未关联`
                    : `原生 ID：${event.callId ?? event.nativeId} · 来源序号 ${event.sourceLine}`}
                </p>
                <p className="break-all text-label text-fg-subtle">
                  轮次：{event.turnId}
                </p>
                {isCall && (
                  <p className="text-label text-fg-muted">
                    客户端归属：
                    {clientAttributionLabel(event.agentAttribution, agentId)}
                  </p>
                )}
                {event.failure && <ModelFailureDetail event={event} />}
                {(event.server || event.toolSource?.status === "unknown") && (
                  <p className="text-label">
                    MCP 服务：{event.server ?? "未知"}
                  </p>
                )}
                {event.toolSource && (
                  <div className="space-y-2 text-label">
                    <p>
                      {event.toolSource.toolNameQuality === "normalized"
                        ? "工具名称（规范化记录）"
                        : "工具名称"}
                      ：{event.toolSource.toolName ?? "未知"}
                    </p>
                    <p className="text-fg-muted">
                      来源：
                      {event.toolSource.status === "native"
                        ? "原生 MCP 标记"
                        : event.toolSource.status === "configuration"
                          ? "当前配置关联"
                          : event.toolSource.status === "builtin"
                            ? "OpenCode 原生工具 / Skill"
                            : "未知"}{" "}
                      · {event.toolSource.reason}
                    </p>
                    {event.toolSource.transport && (
                      <p className="text-fg-muted">
                        当前配置类型：
                        {event.toolSource.transport === "stdio"
                          ? "本地 stdio"
                          : "远程（具体传输未知）"}
                        {event.toolSource.configuredEnabled === false
                          ? " · 当前已禁用"
                          : event.toolSource.configuredEnabled === true
                            ? " · 当前配置为启用"
                            : " · 启用字段未提供"}
                      </p>
                    )}
                    {event.toolSource.sources.map((source) => (
                      <p key={source.path} className="break-all text-fg-muted">
                        {source.scope === "local" ? "本机配置" : "项目配置"}：
                        {source.path}
                        {" · 文件更新："}
                        {date(source.modifiedAt)}
                      </p>
                    ))}
                  </div>
                )}
                {isCall &&
                  event.kind !== "model" &&
                  event.kind !== "diagnostic" && (
                    <p className="text-label">
                      本次调用延迟（原生耗时）：
                      {event.durationMs != null
                        ? `${(event.durationMs / 1000).toFixed(3)} 秒`
                        : "未知（原生记录未提供完整起止时间）"}
                    </p>
                  )}
                {expanded &&
                  (event.kind === "mcp" ||
                    event.toolSource?.status === "unknown") && (
                    <SessionMcpLatency
                      key={event.id}
                      event={event}
                      agentId={agentId}
                    />
                  )}
                {(event.kind === "model" || event.kind === "diagnostic") && (
                  <>
                    <p className="text-label text-fg-muted">
                      {event.provider
                        ? `Provider：${event.provider}`
                        : "Provider 未提供"}{" "}
                      · TTFT / 生成速度 / 费用未知
                    </p>
                    {event.usage && <UsageDetail usage={event.usage} />}
                    {event.rawUsage && (
                      <TextBlock label="原生 usage" text={event.rawUsage} />
                    )}
                  </>
                )}
                {isContext && (
                  <TextBlock label="原生上下文" text={event.text ?? ""} />
                )}
                {isCall &&
                  event.kind !== "model" &&
                  event.kind !== "diagnostic" && (
                    <>
                      <TextBlock
                        label="Input · 参数 / 指令"
                        text={event.input ?? "来源未提供可读参数。"}
                      />
                      <TextBlock
                        label="Output · 结果 / 原始错误"
                        text={
                          event.output ??
                          (event.status === "returned" ||
                          event.status === "error"
                            ? "来源已返回，未提供可读的结果内容。"
                            : "尚未在原生文件中记录结果，不能据此认定调用仍在执行。")
                        }
                      />
                    </>
                  )}
              </div>
            </AccordionContent>
          </AccordionItem>
        </Accordion>
      )}
      {!isCall && !event.text && (
        <p className="text-label text-fg-muted">{status(event.status)}</p>
      )}
      {event.truncated && (
        <p className="text-label text-fg-muted">
          此条内容超过 64 Ki 字符，当前展示已截断；完整内容仍在原生会话文件中。
        </p>
      )}
    </article>
  );
}
