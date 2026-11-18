import {
  Activity,
  Suspense,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent,
} from "react";
import { pageRegistry } from "./builtin-plugins";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import {
  builtinPluginResource,
  pluginDirectoryResource,
} from "@/features/plugins/queries";
import {
  pluginNavigation,
  resolvePluginView,
} from "@/features/plugins/navigation";
import { requestsViewHref } from "@/features/observation/request-display";
import { useBackendReadEpoch } from "@/shared/hooks/use-backend-read-epoch";
import type { AppRoute, PageContext } from "./plugin-registry";
import { RenderBoundary, retryableLazy } from "./render-boundary";
import { AgentDirectoryProvider } from "@/shared/hooks/use-agent-directory";
import { BuiltinCompositionNotice } from "@/features/plugins/builtin-composition-notice";
import type { Page, Snapshot } from "./types";
import { useSnapshot } from "./use-snapshot";
import { IamSession } from "@/features/iam/session";
import {
  Alert,
  AlertAction,
  AlertDescription,
  AlertTitle,
} from "@/components/ui/alert";
import { AppShell, AppShellMain } from "@/components/ui/app-shell";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  NavItem,
  NavItemContent,
  NavItemLabel,
  NavItemLeading,
  NavItemTrigger,
} from "@/components/ui/nav-item";
import { NavMenu } from "@/components/ui/nav-menu";
import {
  PageBody,
  PageContent,
  PageContentHeader,
  PageLayout,
  PageSidebar,
  PageSubnav,
  PageSubnavList,
  PageSubnavItem,
} from "@/components/ui/page-layout";
import { IconProvider, useIcon, type IconName } from "@/lib/icon-context";
import { WorkspaceNavigationContext } from "@/shared/hooks/use-workspace-navigation";
import { WorkspaceTitlebar } from "./workspace-titlebar";
import {
  activeHash,
  closeTabs,
  homeHash,
  homeSections,
  initialWorkspace,
  openHome,
  openResource,
  pinTab,
  pruneUnavailableTabs,
  resourceTab,
  setTabDirty,
  type WorkspaceResourceTab,
  type WorkspaceTabs,
} from "./workspace-tabs";
import "@/styles/app.css";

const ExtensionsPage = retryableLazy(() =>
  import("@/features/plugins/page").then(({ ExtensionsPage }) => ({
    default: ExtensionsPage,
  })),
);
const MemberWorkspace = retryableLazy(() =>
  import("@/features/iam/member-workspace").then(({ MemberWorkspace }) => ({
    default: MemberWorkspace,
  })),
);

const queryString = (hash: string) => hash.split("?")[1] ?? "";
const defaultHash = "#/overview";
const closeOnDelete = (workspace: WorkspaceTabs, tab: WorkspaceResourceTab) =>
  openHome(closeTabs(workspace, [tab.tabKey]), tab.ownerPage);

function NavigationLink({
  item,
  active,
}: {
  item: { id: string; label: string; icon: IconName; href?: string };
  active: boolean;
}) {
  const Icon = useIcon(item.icon);
  return (
    <NavItem value={item.id} active={active}>
      <NavItemTrigger href={item.href ?? `#/${item.id}`}>
        <NavItemLeading>
          <Icon size={18} />
        </NavItemLeading>
        <NavItemContent>
          <NavItemLabel>{item.label}</NavItemLabel>
        </NavItemContent>
      </NavItemTrigger>
    </NavItem>
  );
}

function describeTab(
  route: AppRoute,
  data: Snapshot | null,
): string | undefined {
  const id = route.objectId;
  if (!id) return pageRegistry.resolve(route.page)?.label;
  if (route.page === "agents")
    return data?.agents.find(
      (agent) =>
        agent.id === id || agent.configId === id || agent.family === id,
    )?.name;
  if (route.page === "providers")
    return data?.providers.find((provider) => provider.id === id)?.name;
  return undefined;
}

function routeWorkspace(
  state: WorkspaceTabs,
  href: string,
  options?: { pin?: boolean; title?: string },
) {
  const route = pageRegistry.readRoute(href);
  const tab = resourceTab(route, href, options?.title);
  return tab
    ? openResource(state, tab, options?.pin ?? tab.pinned)
    : openHome(state, route.page, queryString(href));
}

type CloseRequest = { keys: string[]; goHome?: boolean };

function Workspace() {
  const [workspace, setWorkspace] = useState(() =>
    routeWorkspace(initialWorkspace, window.location.hash || defaultHash),
  );
  const stateRef = useRef(workspace);
  const [pageRefresh, setPageRefresh] = useState(0);
  const [closeRequest, setCloseRequest] = useState<CloseRequest | null>(null);
  const openDelay = useRef<number | undefined>(undefined);
  const returnLink = useRef<{ home: string; href: string } | undefined>(
    undefined,
  );
  const { data, error, loading, refresh } = useSnapshot();
  const builtin = useSharedRead(builtinPluginResource);
  const uploaded = useSharedRead(pluginDirectoryResource);
  const readEpoch = useBackendReadEpoch();
  const directory =
    !uploaded.error && uploaded.data?.readEpoch === readEpoch
      ? uploaded.data
      : undefined;
  const extensionNavigation = useMemo(
    () => pluginNavigation(directory),
    [directory],
  );
  const activeTab = workspace.tabs.find(
    ({ tabKey }) => tabKey === workspace.activeTabKey,
  );
  const featureAvailable = useCallback(
    (page: Page) => {
      if (page === "plugins" || page === "settings") return true;
      if (page === "lan-gateway") {
        const owner = builtin.data?.host.plugins.find(
          (plugin) => plugin.id === "builtin.settings",
        );
        return owner?.runtimeActive ?? true;
      }
      return (
        !builtin.error &&
        !!builtin.data?.host.plugins.find(
          (plugin) => plugin.id === pageRegistry.owner(page),
        )?.runtimeActive
      );
    },
    [builtin.data, builtin.error],
  );
  const update = useCallback(
    (
      transition: (state: WorkspaceTabs) => WorkspaceTabs,
      historyMode: "push" | "replace" | "none" = "push",
    ) => {
      const before = stateRef.current;
      const next = transition(before);
      if (next === before) return;
      stateRef.current = next;
      setWorkspace(next);
      const hash = activeHash(next);
      if (historyMode !== "none" && window.location.hash !== hash) {
        if (historyMode === "replace")
          window.history.replaceState(null, "", hash);
        else window.history.pushState(null, "", hash);
      }
    },
    [],
  );

  const open = useCallback(
    (href: string, options?: { pin?: boolean; title?: string }) => {
      window.clearTimeout(openDelay.current);
      const hash = href.startsWith("#") ? href : `#${href}`;
      update((state) => routeWorkspace(state, hash, options));
    },
    [update],
  );
  const activate = useCallback(
    (key: string) => {
      window.clearTimeout(openDelay.current);
      update((state) =>
        key === "home" || state.tabs.some((tab) => tab.tabKey === key)
          ? { ...state, activeTabKey: key }
          : state,
      );
    },
    [update],
  );
  const requestClose = useCallback(
    (keys: string[], goHome?: boolean) => {
      if (
        stateRef.current.tabs.some(
          (tab) => keys.includes(tab.tabKey) && tab.dirty,
        )
      ) {
        setCloseRequest({ keys, goHome });
      } else update((state) => closeTabs(state, keys, goHome));
    },
    [update],
  );

  useEffect(() => {
    const sync = () => {
      window.clearTimeout(openDelay.current);
      update(
        (state) => routeWorkspace(state, window.location.hash || defaultHash),
        "none",
      );
    };
    window.addEventListener("hashchange", sync);
    return () => {
      window.removeEventListener("hashchange", sync);
      window.clearTimeout(openDelay.current);
    };
  }, [update]);
  useEffect(() => {
    if (!builtin.data && !builtin.error) return;
    if (
      pageRegistry.readRoute(window.location.hash).defaultEntry &&
      !featureAvailable("overview")
    ) {
      update((state) => routeWorkspace(state, "#/plugins"), "replace");
    }
    update(
      (state) =>
        pruneUnavailableTabs(state, (tab) => {
          if (tab.kind !== "extension")
            return builtin.error ? true : featureAvailable(tab.ownerPage);
          const { extensionPlugin, extensionView } = pageRegistry.readRoute(
            tab.routeHash,
          );
          return (
            !directory ||
            resolvePluginView(directory, extensionPlugin!, extensionView!)
              .reason === "ready"
          );
        }),
      "none",
    );
  }, [builtin.data, builtin.error, directory, featureAvailable, update]);

  const navigate = (page: Page) => open(`#/${page}`);
  const navigateFromLink = (event: MouseEvent, doubleClick = false) => {
    if (
      event.defaultPrevented ||
      event.button !== 0 ||
      event.altKey ||
      event.shiftKey ||
      !(event.target instanceof Element)
    )
      return;
    const target = event.target;
    const link =
      target.closest<HTMLAnchorElement>("a[href]") ??
      (!target.closest(
        "button,input,select,textarea,[role=button],[role=switch]",
      )
        ? target
            .closest("tr")
            ?.querySelector<HTMLAnchorElement>("a[data-workspace-resource]")
        : null);
    const href = link?.getAttribute("href");
    if (
      !href ||
      !href.startsWith("#") ||
      link?.target ||
      link?.hasAttribute("download")
    )
      return;
    const route = pageRegistry.readRoute(href);
    if (route.defaultEntry || !resourceTab(route, href)) return;
    if (
      !activeTab &&
      event.detail === 0 &&
      link?.hasAttribute("data-workspace-resource")
    ) {
      returnLink.current = { home: homeHash(workspace), href };
    }
    event.preventDefault();
    event.stopPropagation();
    const title = link?.dataset.workspaceTitle || describeTab(route, data);
    const pin =
      doubleClick ||
      event.metaKey ||
      event.ctrlKey ||
      link?.hasAttribute("data-workspace-pin");
    window.clearTimeout(openDelay.current);
    if (pin || event.detail === 0 || !route.objectId)
      open(href, { pin: pin || undefined, title });
    else
      openDelay.current = window.setTimeout(() => open(href, { title }), 250);
  };

  const renderPage = (route: AppRoute, hash: string) => {
    const current = pageRegistry.resolve(route.page);
    const { extensionPlugin, extensionView } = route;
    const context: PageContext = {
      route,
      routeSearch: queryString(hash),
      data,
      refresh,
      navigate,
      pageRefresh,
      featureAvailable,
    };
    const content =
      extensionPlugin && extensionView ? (
        <ExtensionsPage
          key={`${extensionPlugin}:${extensionView}`}
          pluginId={extensionPlugin}
          viewId={extensionView}
          providers={data?.providers ?? []}
          refreshSnapshot={refresh}
          directory={{
            data: directory ?? null,
            error: uploaded.error,
            loading:
              !directory &&
              !uploaded.error &&
              (uploaded.loading || !!uploaded.data),
          }}
          reloadDirectory={uploaded.invalidate}
        />
      ) : !extensionPlugin && featureAvailable(route.page) ? (
        current?.render(context)
      ) : undefined;
    return (
      <RenderBoundary resetKey={hash}>
        <Suspense
          fallback={
            <div role="status" className="p-4 text-body text-fg-muted">
              正在加载页面…
            </div>
          }
        >
          {content ? (
            !extensionPlugin && current?.ownsBody ? (
              <>
                {!route.objectId &&
                  (route.page === "requests" || route.page === "mcp") && (
                    <PageContentHeader className="px-4 py-0">
                      <PageSubnav aria-label="调用与工具视图">
                        <PageSubnavList
                          activeValue={
                            route.page === "mcp"
                              ? "mcp"
                              : route.tab === "trends"
                                ? "trends"
                                : "records"
                          }
                        >
                          {featureAvailable("requests") && (
                            <PageSubnavItem
                              value="records"
                              href={requestsViewHref(
                                route.page === "requests"
                                  ? context.routeSearch
                                  : "",
                                false,
                              )}
                            >
                              模型请求
                            </PageSubnavItem>
                          )}
                          {featureAvailable("mcp") && (
                            <PageSubnavItem value="mcp" href="#/mcp">
                              MCP 工具目录与调用
                            </PageSubnavItem>
                          )}
                          {featureAvailable("requests") && (
                            <PageSubnavItem
                              value="trends"
                              href={requestsViewHref(
                                route.page === "requests"
                                  ? context.routeSearch
                                  : "",
                                true,
                              )}
                            >
                              趋势与热力图
                            </PageSubnavItem>
                          )}
                        </PageSubnavList>
                      </PageSubnav>
                    </PageContentHeader>
                  )}
                {content}
              </>
            ) : (
              <PageBody className="relative max-w-none p-4">{content}</PageBody>
            )
          ) : (
            <PageBody className="p-4">
              <Alert>
                <AlertTitle>
                  {extensionPlugin && !directory && !uploaded.error
                    ? "正在读取插件状态"
                    : builtin.loading && !builtin.data
                      ? "正在读取插件状态"
                      : builtin.error || (uploaded.error && extensionPlugin)
                        ? "功能状态待核查"
                        : "此功能已停用或卸载"}
                </AlertTitle>
                <AlertDescription>
                  可以在插件中心检查依赖和恢复功能。历史数据和配置仍保留。
                </AlertDescription>
                <AlertAction>
                  <Button asChild variant="secondary">
                    <a href="#/plugins">打开插件中心</a>
                  </Button>
                </AlertAction>
              </Alert>
            </PageBody>
          )}
        </Suspense>
      </RenderBoundary>
    );
  };

  const collectionHash = homeHash(workspace);
  useLayoutEffect(() => {
    const pending = returnLink.current;
    if (activeTab || !pending || pending.home !== collectionHash) return;
    const frame = window.requestAnimationFrame(() => {
      returnLink.current = undefined;
      const link = [
        ...document.querySelectorAll<HTMLAnchorElement>(
          "a[data-workspace-resource]",
        ),
      ].find(
        (element) =>
          element.getAttribute("href") === pending.href &&
          element.getClientRects().length > 0,
      );
      link?.focus({ preventScroll: true });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [activeTab, collectionHash]);
  const status =
    loading && !data
      ? "正在连接"
      : error
        ? "连接中断"
        : data
          ? "本地服务已连接"
          : "本地服务未连接";
  return (
    <AgentDirectoryProvider products={data?.agents}>
      <AppShell layout="stacked" className="h-dvh overflow-hidden">
        <WorkspaceTitlebar
          workspace={workspace}
          extensions={extensionNavigation}
          activate={activate}
          pin={(key) => update((state) => pinTab(state, key), "none")}
          close={requestClose}
          open={open}
          status={status}
          loading={loading}
          refresh={() => {
            setPageRefresh((n) => n + 1);
            void refresh();
          }}
        />
        <AppShellMain className="flex min-h-0 flex-col overflow-hidden px-3 pb-3 pt-0">
          <PageLayout
            size="full"
            gutter="none"
            data-workspace-layout
            className="overflow-hidden rounded-xl border border-border bg-surface-floating"
            style={
              !activeTab
                ? { gridTemplateColumns: "auto minmax(0, 1fr)" }
                : undefined
            }
          >
            {!activeTab && (
              <PageSidebar
                key="sidebar"
                width="216px"
                aria-label="应用导航"
                className="rounded-none border-0 border-r border-border p-2 max-lg:w-[216px]"
              >
                {homeSections.map(([group, pages]) => {
                  const extensions =
                    group === "观测" ? extensionNavigation : [];
                  const items = pages.flatMap((page) => {
                    const visible =
                      page === "requests"
                        ? featureAvailable("requests") ||
                          featureAvailable("mcp")
                        : featureAvailable(page);
                    const item = pageRegistry.resolve(page);
                    return visible && item ? [item] : [];
                  });
                  return (
                    (items.length > 0 || extensions.length > 0) && (
                      <section key={group} className="mb-4">
                        <h2 className="px-2 py-1 text-label text-fg-muted">
                          {group}
                        </h2>
                        <NavMenu
                          activeValue={workspace.homeSection}
                          aria-label={group}
                        >
                          {items.map((item) => (
                            <NavigationLink
                              key={item.id}
                              item={{
                                ...item,
                                href: homeHash(
                                  workspace,
                                  item.id === "requests" &&
                                    !featureAvailable("requests")
                                    ? "mcp"
                                    : item.id,
                                ),
                              }}
                              active={workspace.homeSection === item.id}
                            />
                          ))}
                          {extensions.map((item) => (
                            <NavigationLink
                              key={item.id}
                              item={item}
                              active={false}
                            />
                          ))}
                        </NavMenu>
                      </section>
                    )
                  );
                })}
              </PageSidebar>
            )}
            <PageContent
              key="content"
              className="rounded-none border-0"
              onClickCapture={navigateFromLink}
              onDoubleClickCapture={(event) => navigateFromLink(event, true)}
            >
              {data?.agentDirectoryError && (
                <Alert status="warning">
                  <AlertTitle>Agent 来源目录暂不可读取</AlertTitle>
                  <AlertDescription>
                    已有客户端的观测继续保留，请稍后重试。
                  </AlertDescription>
                </Alert>
              )}
              <BuiltinCompositionNotice
                composition={data?.builtinComposition}
                className="px-3 pb-3"
              />
              {error && (
                <Alert status="default">
                  <AlertTitle>
                    {data
                      ? "连接暂时中断，保留上次读取的配置"
                      : "本机数据尚未连接"}
                  </AlertTitle>
                  <AlertDescription>{error}</AlertDescription>
                  <AlertAction>
                    <Button
                      variant="secondary"
                      size="sm"
                      onClick={() => open("#/settings?tab=connection")}
                    >
                      查看连接诊断
                    </Button>
                  </AlertAction>
                </Alert>
              )}
              <Activity mode={activeTab ? "hidden" : "visible"}>
                <WorkspaceNavigationContext.Provider
                  value={{
                    open,
                    close: () => undefined,
                    dismiss: () => undefined,
                    setDirty: () => undefined,
                    setTitle: () => undefined,
                  }}
                >
                  {renderPage(
                    pageRegistry.readRoute(collectionHash),
                    collectionHash,
                  )}
                </WorkspaceNavigationContext.Provider>
              </Activity>
              {workspace.tabs.map((tab) => {
                const route = pageRegistry.readRoute(tab.routeHash);
                const extension = route.extensionPlugin
                  ? extensionNavigation.find(
                      (item) =>
                        item.pluginId === route.extensionPlugin &&
                        item.viewId === route.extensionView,
                    )
                  : undefined;
                const routeName = describeTab(route, data);
                const ownerPage = extension ? "overview" : tab.ownerPage;
                const title =
                  extension?.label ??
                  (tab.title !== (route.objectId ?? route.page)
                    ? tab.title
                    : (routeName ?? tab.title));
                const back = () =>
                  update((state) =>
                    openHome(
                      state,
                      ["plugins", "settings", "iam"].includes(ownerPage)
                        ? undefined
                        : ownerPage,
                    ),
                  );
                return (
                  <Activity
                    key={tab.tabKey}
                    mode={
                      activeTab?.tabKey === tab.tabKey ? "visible" : "hidden"
                    }
                  >
                    <WorkspaceNavigationContext.Provider
                      value={{
                        open,
                        close: (goHome = true) =>
                          update((state) =>
                            goHome
                              ? closeOnDelete(state, tab)
                              : closeTabs(state, [tab.tabKey]),
                          ),
                        dismiss: () => requestClose([tab.tabKey]),
                        setTitle: (title) =>
                          update((state) => {
                            if (
                              state.tabs.find(
                                (item) => item.tabKey === tab.tabKey,
                              )?.title === title
                            )
                              return state;
                            return {
                              ...state,
                              tabs: state.tabs.map((item) =>
                                item.tabKey === tab.tabKey
                                  ? { ...item, title }
                                  : item,
                              ),
                            };
                          }, "none"),
                        setDirty: (dirty) =>
                          update(
                            (state) => setTabDirty(state, tab.tabKey, dirty),
                            "none",
                          ),
                      }}
                    >
                      <PageContentHeader className="px-4 py-2">
                        <Breadcrumb aria-label="资源位置">
                          <BreadcrumbList>
                            <BreadcrumbItem>
                              <BreadcrumbLink
                                render={<button type="button" onClick={back} />}
                              >
                                ←{" "}
                                {["plugins", "settings", "iam"].includes(
                                  ownerPage,
                                )
                                  ? "Home"
                                  : pageRegistry.resolve(ownerPage)?.label}
                              </BreadcrumbLink>
                            </BreadcrumbItem>
                            <BreadcrumbSeparator />
                            <BreadcrumbItem>
                              <BreadcrumbPage>{title}</BreadcrumbPage>
                            </BreadcrumbItem>
                          </BreadcrumbList>
                        </Breadcrumb>
                      </PageContentHeader>
                      {renderPage(route, tab.routeHash)}
                    </WorkspaceNavigationContext.Provider>
                  </Activity>
                );
              })}
            </PageContent>
          </PageLayout>
        </AppShellMain>
        <Dialog
          open={closeRequest !== null}
          onOpenChange={(visible) => {
            if (!visible) setCloseRequest(null);
          }}
        >
          <DialogContent>
            <DialogTitle>标签含有未保存修改</DialogTitle>
            <DialogDescription>
              关闭标签将丢弃其中尚未发布的变更。
            </DialogDescription>
            <DialogFooter>
              <Button variant="secondary" onClick={() => setCloseRequest(null)}>
                继续编辑
              </Button>
              <Button
                variant="destructive"
                onClick={() => {
                  if (!closeRequest) return;
                  update((state) =>
                    closeTabs(state, closeRequest.keys, closeRequest.goHome),
                  );
                  setCloseRequest(null);
                }}
              >
                放弃未保存修改
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      </AppShell>
    </AgentDirectoryProvider>
  );
}

export default function App() {
  return (
    <RenderBoundary scope="application" resetKey="application">
      <IconProvider>
        <IamSession
          member={(account) => (
            <Suspense fallback={<p role="status">正在加载资源工作区…</p>}>
              <MemberWorkspace account={account} />
            </Suspense>
          )}
        >
          <Workspace />
        </IamSession>
      </IconProvider>
    </RenderBoundary>
  );
}
