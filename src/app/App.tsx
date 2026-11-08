import { pageRegistry } from "./builtin-plugins";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import {
  builtinPluginResource,
  pluginDirectoryResource,
} from "@/features/plugins/queries";
import { pluginNavigation } from "@/features/plugins/navigation";
import { useBackendReadEpoch } from "@/shared/hooks/use-backend-read-epoch";
import type { IconName } from "@/lib/icon-context";
import type { PageContext, PageContribution } from "./plugin-registry";
import { RenderBoundary, retryableLazy } from "./render-boundary";
import { AgentDirectoryProvider } from "@/shared/hooks/use-agent-directory";
import { BuiltinCompositionNotice } from "@/features/plugins/builtin-composition-notice";
import type { Page } from "@/app/types";
import { useSnapshot } from "@/app/use-snapshot";
import { IamSession } from "@/features/iam/session";
import { useIam } from "@/features/iam/context";
import { useIamAction } from "@/features/iam/actions";
import {
  Alert,
  AlertAction,
  AlertDescription,
  AlertTitle,
} from "@/components/ui/alert";
import {
  AppShell,
  AppShellMain,
  AppShellSidebar,
} from "@/components/ui/app-shell";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  NavItem,
  NavItemContent,
  NavItemLabel,
  NavItemLeading,
  NavItemTrigger,
} from "@/components/ui/nav-item";
import { NavMenu } from "@/components/ui/nav-menu";
import {
  PageActions,
  PageBody,
  PageContent,
  PageContentHeader,
  PageLayout,
  PageTitle,
} from "@/components/ui/page-layout";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarProvider,
  SidebarTrigger,
  useSidebar,
} from "@/components/ui/sidebar";
import { IconProvider, useIcon } from "@/lib/icon-context";
import "@/styles/app.css";
import {
  Suspense,
  useEffect,
  useMemo,
  useRef,
  useState,
  type RefObject,
} from "react";

const navigation = pageRegistry.navigation;
const sidebarWidth = "13.5rem";
const readRoute = () => pageRegistry.readRoute(window.location.hash);
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

function WorkspacePage({
  render,
  context,
  ownsBody,
  bodyRef,
}: {
  render: PageContribution["render"];
  context: PageContext;
  ownsBody: boolean | undefined;
  bodyRef: RefObject<HTMLDivElement | null>;
}) {
  const content = render(context);
  return ownsBody ? (
    content
  ) : (
    <PageBody ref={bodyRef} className="relative p-4 sm:p-6">
      {content}
    </PageBody>
  );
}

function WindowTitlebar({ title, status }: { title: string; status: string }) {
  const { open, isMobile } = useSidebar();
  const iam = useIam();
  const logout = useIamAction();
  return (
    <header
      aria-label="窗口工具栏"
      data-tauri-drag-region
      className="flex h-8 shrink-0 items-center pr-4"
    >
      <div
        data-tauri-drag-region
        className="flex h-full shrink-0 items-center pl-24"
        style={{ width: open && !isMobile ? sidebarWidth : undefined }}
      >
        <SidebarTrigger label="切换导航" variant="ghost" size="sm" />
      </div>
      <div
        data-tauri-drag-region
        className="flex min-w-0 flex-1 items-center self-stretch pl-6 pr-3"
      >
        <PageTitle
          data-tauri-drag-region
          className="truncate text-label font-medium"
        >
          {title}
        </PageTitle>
      </div>
      <Badge variant="dot" status="neutral" role="status">
        {status}
      </Badge>
      {iam.status?.account && (
        <div className="ml-3 flex min-w-0 items-center gap-2">
          <span className="max-w-36 truncate text-label">
            {iam.status.account.displayName}
          </span>
          <Button
            size="sm"
            variant="tertiary"
            disabled={logout.busy}
            onClick={() =>
              void logout.run(async () => {
                await iam.logout?.();
              })
            }
          >
            退出登录
          </Button>
          {logout.error && (
            <span role="alert" className="text-label">
              {logout.error}
            </span>
          )}
        </div>
      )}
    </header>
  );
}

function NavigationLink({
  item,
  active,
}: {
  item: { id: string; label: string; icon: IconName; href?: string };
  active: boolean;
}) {
  const Icon = useIcon(item.icon);
  const sidebar = useSidebar();
  return (
    <NavItem value={item.id} active={active}>
      <NavItemTrigger
        href={item.href ?? `#/${item.id}`}
        onClick={() => sidebar.closeMobile()}
      >
        <NavItemLeading>
          <Icon size={20} />
        </NavItemLeading>
        <NavItemContent>
          <NavItemLabel>{item.label}</NavItemLabel>
        </NavItemContent>
      </NavItemTrigger>
    </NavItem>
  );
}

function Workspace() {
  const pageBodyRef = useRef<HTMLDivElement>(null);
  const [requestedRoute, setRoute] = useState(readRoute);
  const [pageRefresh, setPageRefresh] = useState(0);
  const { data, error, loading, refresh } = useSnapshot();
  const builtin = useSharedRead(builtinPluginResource);
  const uploaded = useSharedRead(pluginDirectoryResource);
  const readEpoch = useBackendReadEpoch();
  const directory =
    !uploaded.error && uploaded.data?.readEpoch === readEpoch
      ? uploaded.data
      : undefined;
  const featureAvailable = (page: Page) => {
    if (page === "plugins" || page === "settings") return true;
    return (
      !builtin.error &&
      !!builtin.data?.host.plugins.find(
        (plugin) => plugin.id === pageRegistry.owner(page),
      )?.runtimeActive
    );
  };
  const route =
    requestedRoute.defaultEntry &&
    (builtin.data || builtin.error) &&
    !featureAvailable(requestedRoute.page)
      ? { ...requestedRoute, page: "plugins" as const }
      : requestedRoute;
  const extensionNavigation = useMemo(
    () => pluginNavigation(directory),
    [directory],
  );
  const extension = extensionNavigation.find(
    (item) =>
      item.pluginId === route.extensionPlugin &&
      item.viewId === route.extensionView,
  );
  const RefreshIcon = useIcon("rotate-ccw");
  useEffect(() => {
    const handle = () => setRoute(readRoute());
    window.addEventListener("hashchange", handle);
    return () => window.removeEventListener("hashchange", handle);
  }, []);
  useEffect(() => {
    if (
      route.extensionPlugin ||
      pageRegistry.resolve(route.page)?.resetScrollOnObject
    )
      pageBodyRef.current?.scrollTo({ top: 0 });
  }, [route.page, route.objectId, route.extensionPlugin, route.extensionView]);
  const current = pageRegistry.resolve(route.page);
  const { extensionPlugin, extensionView } = route;
  const renderPage: PageContribution["render"] | undefined =
    extensionPlugin && extensionView
      ? ({ data, refresh }) => (
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
        )
      : featureAvailable(route.page)
        ? current?.render
        : undefined;
  const title =
    extension?.label ??
    (route.extensionPlugin ? "插件页面" : (current?.label ?? "功能不可用"));
  const navigate = (page: Page) => {
    window.location.hash = `/${page}`;
  };
  const status =
    loading && !data
      ? "正在连接本地服务"
      : data
        ? error
          ? "数据连接中断"
          : "本地服务已连接"
        : "本地服务未连接";
  const context: PageContext = {
    route,
    routeSearch: window.location.hash.split("?")[1] ?? "",
    data,
    refresh,
    navigate,
    pageRefresh,
  };
  return (
    <AgentDirectoryProvider products={data?.agents}>
      <SidebarProvider
        defaultOpen
        persistenceKey="local-agent.sidebar"
        keyboardShortcut="mod+b"
      >
        <div className="flex h-dvh flex-col overflow-hidden bg-surface-base">
          <WindowTitlebar title={title} status={status} />
          <AppShell className="min-h-0 flex-1 overflow-hidden">
            <AppShellSidebar>
              <Sidebar
                width={sidebarWidth}
                collapsible="offcanvas"
                ariaLabel="应用导航"
                className="h-full max-xl:pt-8"
              >
                <SidebarContent>
                  <NavMenu
                    activeValue={extension?.id ?? route.page}
                    aria-label="主导航"
                  >
                    {navigation
                      .filter(
                        (item) => !item.footer && featureAvailable(item.id),
                      )
                      .map((item) => (
                        <NavigationLink
                          key={item.id}
                          item={item}
                          active={
                            !route.extensionPlugin && route.page === item.id
                          }
                        />
                      ))}
                    {extensionNavigation.map((item) => (
                      <NavigationLink
                        key={item.id}
                        item={item}
                        active={extension?.id === item.id}
                      />
                    ))}
                  </NavMenu>
                </SidebarContent>
                <SidebarFooter>
                  <NavMenu activeValue={route.page} aria-label="偏好设置">
                    <NavigationLink
                      item={navigation.find((item) => item.footer)!}
                      active={route.page === "settings"}
                    />
                  </NavMenu>
                </SidebarFooter>
              </Sidebar>
            </AppShellSidebar>
            <AppShellMain className="flex min-h-0 flex-col">
              <PageLayout size="full">
                {data?.agentDirectoryError && (
                  <Alert status="warning">
                    <AlertTitle>Agent 来源目录暂不可读取</AlertTitle>
                    <AlertDescription>
                      当前来源列表可能不完整。已有客户端的观测继续保留，请稍后重试。
                    </AlertDescription>
                  </Alert>
                )}
                <BuiltinCompositionNotice
                  composition={data?.builtinComposition}
                  className="px-3 pb-3"
                />
                {error && (
                  <div className="px-3 pb-3">
                    <Alert status="default">
                      <AlertTitle>
                        {data
                          ? "连接暂时中断，保留上次读取的配置"
                          : "本机数据尚未连接"}
                      </AlertTitle>
                      <AlertDescription>
                        <p>{error}</p>
                      </AlertDescription>
                      {route.page !== "settings" && (
                        <AlertAction>
                          <Button
                            variant="secondary"
                            size="sm"
                            onClick={() => navigate("settings")}
                          >
                            查看连接诊断
                          </Button>
                        </AlertAction>
                      )}
                    </Alert>
                  </div>
                )}
                <PageContent>
                  {(!data || error || current?.refreshInHeader) && (
                    <PageContentHeader>
                      <PageActions>
                        <Button
                          variant="tertiary"
                          size="sm"
                          leadingIcon={RefreshIcon}
                          loading={loading}
                          onClick={() => {
                            setPageRefresh((n) => n + 1);
                            void refresh();
                          }}
                        >
                          刷新
                        </Button>
                      </PageActions>
                    </PageContentHeader>
                  )}
                  <RenderBoundary resetKey={window.location.hash}>
                    <Suspense
                      fallback={
                        <div
                          role="status"
                          className="p-4 text-body text-fg-muted"
                        >
                          正在加载页面…
                        </div>
                      }
                    >
                      {renderPage ? (
                        <WorkspacePage
                          render={renderPage}
                          context={context}
                          ownsBody={!extensionPlugin && current?.ownsBody}
                          bodyRef={pageBodyRef}
                        />
                      ) : (
                        <PageBody className="p-4 sm:p-6">
                          <Alert>
                            <AlertTitle>
                              {builtin.loading && !builtin.data
                                ? "正在读取插件状态"
                                : builtin.error
                                  ? "功能状态待核查"
                                  : "此功能已停用或卸载"}
                            </AlertTitle>
                            <AlertDescription>
                              请在插件中心检查依赖并恢复。历史数据和配置仍保留，未继续调用此功能。
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
                </PageContent>
              </PageLayout>
            </AppShellMain>
          </AppShell>
        </div>
      </SidebarProvider>
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
